# async-runtime

`async-runtime` is a priority-aware native Rust runtime for the smol ecosystem,
with a work-stealing general worker pool and host-driven local domains. It
combines a custom `async-task` scheduler with worker-local queues, priority
global injectors, work stealing, and parked-worker wake-up with budgeted local
driving and lightweight cross-thread dispatch.

```rust,no_run
use async_runtime::{Priority, RuntimeBuilder};
use std::num::NonZeroUsize;

let runtime = RuntimeBuilder::new(NonZeroUsize::new(4).unwrap()).build()?;
runtime.spawn(Priority::High, async { /* Send work */ })?.detach();
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Smol ecosystem

The general runtime uses `async-task` for task machinery and
`crossbeam-deque` for its queues; `LocalDomain` continues to use
`async-executor`. `async-channel` and `futures-lite` support the local-domain
and task APIs. The crate schedules futures but does not own or drive an I/O
reactor. Applications remain responsible for driving their chosen I/O runtime;
`async-io` futures compose naturally when the host drives `async-io`.

Calling `smol::spawn` still targets smol's own global executor. Submit work
through this crate's `Runtime` / `Spawner` when it must participate in priority
scheduling, task accounting, or runtime shutdown.

Use `Runtime` for movable `Send` work. Use one `LocalDomain` per fixed host
thread when tasks must remain on that thread; its `LocalSpawner` accepts only
`Send` work submitted from elsewhere.

The three priorities are `High`, `Normal`, and `Background`. Each general
worker uses an independent weighted selector, defaulting to 8:4:1. This is
fair scheduling opportunity, not a global execution order or throughput SLA.

## General scheduler

The general `Runtime` has one FIFO local queue per priority for every
worker, plus one global injector per priority. A runnable scheduled by that
runtime's worker goes to that worker's matching local queue; a runnable
scheduled by another thread goes to the matching global injector. This gives
nested and self-woken work a locality preference without making a worker
thread-affine.

For each weighted priority opportunity (the default remains `8:4:1`), a worker
normally tries its local queue first, then takes a batch from the matching
global injector, and finally tries other workers as rotating steal victims.
After a bounded local burst it checks the global injector first, preventing a
continuously self-waking local source from starving same-priority external
submissions. Stealing
can move work between workers, so callers must not infer an execution thread
from a general task's submission thread. Use `LocalDomain` for genuine thread
affinity.

Idle workers park on a condition variable after checking the queues. Submitting
work wakes one parked worker; shutdown wakes all workers. This avoids busy
waiting when the general pool is idle, but it is an implementation mechanism,
not a latency or power-use guarantee.

Priority is a scheduling preference, not a strict global order, completion
ratio, throughput SLA, or starvation-proof deadline service. Work stealing and
the per-worker selector improve availability of queued work; applications that
need realtime behavior must still keep polls short, bound their own work, and
measure their target host.

The runtime does not implement an I/O reactor or prescribe one to the host.

### Optional scheduler statistics

Enable the `stats` feature to expose a point-in-time `RuntimeStats` snapshot:

```toml
[dependencies]
async-runtime = { version = "0.3", features = ["stats"] }
```

```rust,no_run
# use async_runtime::RuntimeBuilder;
# use std::num::NonZeroUsize;
let runtime = RuntimeBuilder::new(NonZeroUsize::new(2).unwrap()).build()?;
let stats = runtime.stats();
println!("workers={}, executed={}", stats.workers, stats.executed);
# runtime.shutdown_now()?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`RuntimeStats` includes approximate runnable queue counts, sleeping workers,
executions, steals, submission origin, parks, and wake notifications. It is
intended for diagnostics and benchmark interpretation: concurrent activity can
change values while the snapshot is read, and queue counts are not task
completion or liveness counts. Without the feature, neither `RuntimeStats` nor
`Runtime::stats()` is part of the public API.

### API stability and limits

`0.3.0` is the first version published to crates.io. Earlier `0.1` and
`0.2` revisions exist only in the repository's development history; they were
not registry releases and do not define a crates.io migration path.

`RuntimeBuilder`, `Runtime`, `Spawner`, `Task`, `FallibleTask`, priorities,
and shutdown APIs are the public surface of the initial release. Code should
rely on the documented priority and task-lifecycle semantics rather than
internal queue or worker-selection details.

The v0.3 scheduler architecture is intentionally frozen. Except for
correctness fixes, the priority selector, local-burst limits, stealing,
park/wake, and admission protocols will change only when real application
traces, a reproducible workload, and a regression against the recorded
baseline all point to the same problem. This keeps speculative scheduler work
out of the stable foundation while applications establish their real needs.

General queues remain unbounded and do not provide submission backpressure.
Tasks are cooperative: a long synchronous `Future::poll` can delay priority
selection, stealing, shutdown progress, and wake handling. A task panic is
reported through its task handle according to the existing `Task` semantics;
use task handles or an application panic hook when that observation matters.

## Lifecycle

Tasks are cancelled when their `Task` handle is dropped; call `detach()` for
background work. Graceful shutdown rejects new work and drains every accepted
task. `shutdown_now()` cancels outstanding work.

`LocalDomain` must be created and driven on its owner thread. Its inbox carries
only `Send` spawn commands: local runnables and `!Send` data never cross a
thread boundary.

## Host-driven `LocalDomain`

`LocalDomain` is intended for a UI, render, game, or other host-owned thread.
Create it and call its driving methods from that one owner thread. It does not
create a thread or run itself in the background.

Use `run_n` when the host loop allocates work in drive steps rather than time:

```rust,no_run
# use async_runtime::LocalDomain;
let domain = LocalDomain::new();

// During one host-loop iteration, make at most 64 non-blocking drive steps.
let progressed = domain.run_n(64);
// Update UI / render a frame / run the rest of the host loop.
# let _ = progressed;
```

`run_n(max_steps)` is non-blocking. Its limit and return value are **drive
steps**, not completed futures and not a precise count of `Future::poll` calls.
One step follows the domain's `try_tick` progress policy: it may materialize one
remote inbox command and gives already-local runnable work an opportunity to
run. Therefore a task may need several calls to finish, and `run_n(0)` performs
no work and returns `0`.

Use `run_for` for a frame or event-loop budget:

```rust,no_run
# use async_runtime::LocalDomain;
# use std::time::Duration;
let domain = LocalDomain::new();

let stats = domain.run_for(Duration::from_micros(500));
// `stats.drive_steps` is total drive progress; `stats.inbox_commands` is the
// number of accepted remote commands materialized during this call.
debug_assert!(stats.elapsed >= Duration::ZERO);
```

`run_for(budget)` checks the budget before each drive step and stops when the
domain is idle or the budget expires. `Duration::ZERO` performs no work and
returns zero progress. This is a **soft** time budget: a step may execute one
remote inbox command and then give one local runnable an opportunity to poll.
The budget check cannot preempt either phase, so time spent in either may cause
`RunStats::elapsed` to exceed the requested budget. Keep individual command
closures and future polls short when frame latency matters.

`RunStats` reports `drive_steps`, `inbox_commands`, and `elapsed`; use it to expose
per-frame progress or to detect backlog trends. It is operational feedback, not
a realtime deadline guarantee.

### Fire-and-forget cross-thread dispatch

`LocalSpawner::spawn` remains the right API when a caller needs a `Task<T>`, a
result, cancellation, or panic observation. For owner-thread callbacks and
one-way asynchronous hand-offs, use the lighter fire-and-forget APIs instead:

```rust,no_run
# use async_runtime::LocalDomain;
let domain = LocalDomain::new();
let local = domain.spawner();

local.dispatch(|| {
    // Runs later on `domain`'s owner thread.
    // Commit UI, GPU, or thread-affine state here.
})?;

local.dispatch_future(async move {
    // A Send future that eventually completes on the owner thread.
})?;
# Ok::<(), async_runtime::SpawnError>(())
```

Both methods accept only `Send + 'static` work, return `SpawnError::Closed`
once shutdown starts (or the domain is gone), and enqueue work for the owner
thread to materialize. They return no task handle: there is no result channel,
cancellation handle, or completion notification. Dispatches are processed in
the inbox's FIFO order, but that is not a completion-order guarantee for async
futures.

A panic in dispatched work is isolated so that the `LocalDomain` remains
driveable. Rust's installed panic hook still runs, so applications should set a
hook or logging integration if they need to observe such failures.

The cross-thread inbox is deliberately **unbounded**. `dispatch`,
`dispatch_future`, and remote `spawn` do not apply backpressure; a producer
that outruns the owner thread can grow memory without limit. Bound production
at the caller, coalesce redundant updates, or drain the domain more often.

## Examples

The numbered examples are a guided path from a general worker pool to a
host-driven local domain. Run any of them with `cargo run --example <name>`.

1. [01_quick_start](examples/01_quick_start.rs) — build a general `Runtime`,
   spawn `Send` work, await its result, and shut down gracefully.
2. [02_priority](examples/02_priority.rs) — submit `High`, `Normal`, and
   `Background` work; priorities express scheduling preference, not a global
   ordering guarantee.
3. [03_budgeted_local](examples/03_budgeted_local.rs) — keep `!Send` state on
   an owner thread and drive it with `run_n` or a soft `run_for` frame budget.
4. [04_cross_thread_dispatch](examples/04_cross_thread_dispatch.rs) — submit
   callbacks and `Send` futures from another thread; the owner must drive the
   domain for them to execute.
5. [05_domain_composition](examples/05_domain_composition.rs) — compose
   general and local work in both directions without blocking the owner loop.
6. [06_task_lifecycle](examples/06_task_lifecycle.rs) — await, cancel, detach,
   inspect completion, and observe task panics.
7. [07_shutdown](examples/07_shutdown.rs) — graceful local draining, timed
   general shutdown, immediate cancellation, and rejection of late work.
8. [08_nested_worker_locality](examples/08_nested_worker_locality.rs) — nested
   general work prefers its worker's local queue; it is not thread affinity and
   may be stolen.
9. [09_external_multi_producer](examples/09_external_multi_producer.rs) — many
   threads submit through cloned `Spawner`s into global injectors.
10. [10_priority_fairness](examples/10_priority_fairness.rs) — background work
    makes progress while High work yields; this demonstrates weighted
    opportunity, not a realtime guarantee.
11. [11_idle_wake](examples/11_idle_wake.rs) — external submission wakes an
    idle worker; use benchmarks rather than its printed time for measurement.
12. [12_custom_priority_weights](examples/12_custom_priority_weights.rs) — set
    a non-zero three-priority ratio while preserving eventual opportunities.
13. [13_scheduler_stats](examples/13_scheduler_stats.rs) — inspect the
    optional approximate counters (`cargo run --example 13_scheduler_stats
    --features stats`).
14. [15_cpu_result_to_local_frame](examples/15_cpu_result_to_local_frame.rs) —
    the canonical General Runtime → `Send` mailbox → `LocalDomain` → next-frame
    owner-state handoff, including the `Rc<RefCell<_>>` affinity boundary.
15. [90_best_practice_host_loop](examples/90_best_practice_host_loop.rs) — a
    practical UI/render/game host-loop shape with per-frame budgeted driving.

## Performance scenarios

Performance workloads are split by question instead of being hidden in one
large benchmark. Run the suite with `cargo bench`, or one scenario with
`cargo bench --bench <name>`:

- `general_spawn`: batch spawn/await and nested spawn; nested work also
  exercises worker-local routing.
- `priority`: per-priority and mixed 8:4:1 throughput under the scheduler.
- `local_driving`: drive-only `run_n` and `run_for` cost.
- `local_dispatch`: real cross-thread producer, result bridge versus both
  fire-and-forget paths.
- `frame_like`: preloaded inbox work under 100 us, 500 us, and 1 ms budgets.
- `shutdown`: graceful drain and immediate cancellation.
- `yield_storm`: many tasks repeatedly yielding and being requeued; it is a
  useful stress scenario for local routing, global injection, and stealing.
- `v030_external_producers`: submission contention from 1–16 external
  producers.
- `v030_nested_locality`: nested spawn/completion under different worker and
  child counts.
- `v030_steal_imbalance`: a parent creates yielding children, approximating an
  imbalanced local queue through the public API.
- `v030_yield_wake_storm`: separate cooperative-yield and externally-woken
  pending-task storms.
- `v030_priority_latency` and `v030_starvation`: bounded probe-progress
  scenarios while High work is queued; they are not SLA or infinite-stream
  proofs.
- `v030_idle_wake`: complete park/submit/wake/re-park cycles (run with
  `cargo bench --bench v030_idle_wake --features stats`); CPU use still needs
  an OS profiler.
- `v030_cpu_workload`: a fixed-iteration, single-poll CPU kernel; records
  1→2→4→8 nested-work scaling and per-priority submit-to-complete p50/p95/p99.
- `v030_local_budget_latency`: both `RunStats::elapsed` and caller-observed
  wall-clock `run_for` elapsed, with overshoot p50/p95/p99 for ready-queue and
  remote-inbox work. Ready-queue work is a future poll; remote-inbox work is
  dispatch-command execution. Printed maximums are observations only, not
  regression gates.

### Performance snapshot

These numbers give an initial feel for the runtime on one machine; they are
scenario measurements, not cross-machine claims or latency SLAs. The full
methodology and tables are in the
[v0.3 release baseline](benchmarks/baseline-v0.3-release.md).

```text
CPU: AMD Ryzen 7 8845H (8 physical cores / 16 logical processors)
Memory: 59.8 GiB
OS: Windows 11 10.0.26200, 64-bit (x86_64-pc-windows-msvc)
Power scheme: Balanced
Rust: rustc 1.97.0-nightly (507271bc1 2026-05-17)
Cargo: cargo 1.97.0-nightly (4d1f98451 2026-05-15)
```

The five-round v0.3 release capture measured a trivial normal-task
spawn-and-complete at 1.0863 us for one task and 205.62 us for 1024 tasks. Its
fixed CPU kernel scaled from 3,692.6 tasks/s on one worker to 27,888.6 tasks/s
on eight workers: 7.552x speedup and 94.4% scaling efficiency. A 10,000-task
cooperative-yield storm with eight yields per task completed in 6.2783 ms.

An additional caller-side spot check closely tracked `RunStats::elapsed`. Its
[supplementary record](benchmarks/local-budget-caller-spot-check-v0.3.3.md) is
separate from the repeatable five-round release baseline, whose capture did not
retain caller-side elapsed distributions.

Use the same machine, Rust toolchain, workload parameters, and release profile
when comparing scheduler revisions. These benchmarks describe scenarios, not
an assertion that the current release is faster than another runtime. The
recorded release environment, aggregation rules, and five-round results are in
the [v0.3 release baseline](benchmarks/baseline-v0.3-release.md). Historical
pre-release records remain available in the
[v0.3 engineering baseline](benchmarks/baseline-v0.3.md) and the
[v0.2 engineering baseline](benchmarks/baseline-v0.2.md); they are development
measurements, not release baselines.

For the functional suite, run `cargo test`; include optional observability with
`cargo test --all-features`, and documentation examples with `cargo test --doc`.

## Status

The crate uses Edition 2021 and requires Rust 1.71 or newer. Its native targets
are Windows, Linux, macOS, Android, and iOS.

CI maintains two compatibility lines: the MSRV lane uses Rust 1.71 with the
committed, known-compatible `Cargo.lock`, while the Latest lane runs
`cargo update` and tests the newest allowed dependencies on latest stable Rust.
This lets releases keep using the last compatible smol ecosystem versions until
the crate deliberately raises its MSRV.

WASM is unsupported by design. The runtime's semantics depend on a
native multi-threaded general worker pool and message passing between execution
domains; reducing it to single-threaded WASM would be a different runtime model.

Mobile hosts remain responsible for app lifecycle and for driving a
`LocalDomain` from the appropriate UI, render, or logic thread. The crate is
cross-checked for ARM64 Android and iOS; CI also runs the core suite in an
Android emulator and links the suite for an ARM64 iOS Simulator. Running it on
iOS still requires an XCTest host app. The crate is licensed under either
[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
