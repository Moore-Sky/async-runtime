# async-runtime

`async-runtime` is a priority-aware native Rust runtime for the smol ecosystem,
with three general-purpose priority queues and host-driven local domains. v0.2
adds budgeted, host-loop-friendly local driving and lightweight cross-thread
dispatch.

```rust,no_run
use async_runtime::{Priority, RuntimeBuilder};
use std::num::NonZeroUsize;

let runtime = RuntimeBuilder::new(NonZeroUsize::new(4).unwrap()).build()?;
runtime.spawn(Priority::High, async { /* Send work */ })?.detach();
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Smol ecosystem

The runtime is built directly on `async-executor`, `async-task`,
`async-channel`, and `futures-lite`. It schedules futures but does not own or
drive an I/O reactor. Applications remain responsible for driving their chosen
I/O runtime; `async-io` futures compose naturally when the host drives
`async-io`.

Calling `smol::spawn` still targets smol's own global executor. Submit work
through this crate's `Runtime` / `Spawner` when it must participate in priority
scheduling, task accounting, or runtime shutdown.

Use `Runtime` for movable `Send` work. Use one `LocalDomain` per fixed host
thread when tasks must remain on that thread; its `LocalSpawner` accepts only
`Send` work submitted from elsewhere.

The three priorities are `High`, `Normal`, and `Background`. Each general
worker uses an independent weighted selector, defaulting to 8:4:1. This is
fair scheduling opportunity, not a global execution order or throughput SLA.

## Scheduling trade-off

To schedule across priorities, v0.1 drives each `async_executor::Executor`
with `try_tick()` / `tick()`. Consequently it uses three shared global queues
and does not use `Executor::run()`'s per-runner local queues or work stealing.
This is an intentional correctness-first trade-off; benchmarks determine
whether a later custom `async-task` scheduler is warranted.

Workers wait on executor and shutdown futures through
`futures_lite::future::block_on`. The runtime does not implement an I/O reactor
or prescribe one to the host.

## Lifecycle

Tasks are cancelled when their `Task` handle is dropped; call `detach()` for
background work. Graceful shutdown rejects new work and drains every accepted
task. `shutdown_now()` cancels outstanding work.

`LocalDomain` must be created and driven on its owner thread. Its inbox carries
only `Send` spawn commands: local runnables and `!Send` data never cross a
thread boundary.

## Host-driven `LocalDomain` (v0.2)

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
returns zero progress. This is a **soft** time budget: Rust cannot safely
preempt a future that is already being polled, so `RunStats::elapsed` can exceed
the requested budget by the duration of that poll. Keep individual polls short
and cooperative when frame latency matters.

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

The cross-thread inbox is deliberately **unbounded** in v0.2. `dispatch`,
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
8. [90_best_practice_host_loop](examples/90_best_practice_host_loop.rs) — a
   practical UI/render/game host-loop shape with per-frame budgeted driving.

## Performance scenarios

Performance workloads are split by question instead of being hidden in one
large benchmark. Run one with `cargo bench --bench <name>`:

- `general_spawn`: batch spawn/await and nested spawn.
- `priority`: per-priority and mixed 8:4:1 throughput.
- `local_driving`: drive-only `run_n` and `run_for` cost.
- `local_dispatch`: real cross-thread producer, result bridge versus both
  fire-and-forget paths.
- `frame_like`: preloaded inbox work under 100 us, 500 us, and 1 ms budgets.
- `shutdown`: graceful drain and immediate cancellation.
- `yield_storm`: many tasks repeatedly yielding and being requeued.

See the recorded environment, measurement boundaries, and results in the
[v0.2 baseline](benchmarks/baseline-v0.2.md).

See [the Chinese README](README_ZH.md).

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
