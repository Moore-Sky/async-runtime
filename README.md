# async-runtime

`async-runtime` is a priority-aware native Rust runtime for the smol ecosystem,
with three general-purpose priority queues and host-driven local domains.

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

See [the Chinese README](README_ZH.md).

## Status

The crate uses Edition 2021 and requires Rust 1.71 or newer. Its native targets
are Windows, Linux, macOS, Android, and iOS.

CI maintains two compatibility lines: the MSRV lane uses Rust 1.71 with the
committed, known-compatible `Cargo.lock`, while the Latest lane runs
`cargo update` and tests the newest allowed dependencies on latest stable Rust.
This lets releases keep using the last compatible smol ecosystem versions until
the crate deliberately raises its MSRV.

WASM is unsupported by design in v0.1. The runtime's semantics depend on a
native multi-threaded general worker pool and message passing between execution
domains; reducing it to single-threaded WASM would be a different runtime model.

Mobile hosts remain responsible for app lifecycle and for driving a
`LocalDomain` from the appropriate UI, render, or logic thread. The crate is
cross-checked for ARM64 Android and iOS; CI also runs the core suite in an
Android emulator and links the suite for an ARM64 iOS Simulator. Running it on
iOS still requires an XCTest host app. The crate is licensed under either
[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
