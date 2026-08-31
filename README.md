# async-runtime

`async-runtime` is a small native Rust runtime with three general-purpose
priority queues and host-driven local domains.

```rust,no_run
use async_runtime::{Priority, RuntimeBuilder};
use std::num::NonZeroUsize;

let runtime = RuntimeBuilder::new(NonZeroUsize::new(4).unwrap()).build()?;
runtime.spawn(Priority::High, async { /* Send work */ })?.detach();
# Ok::<(), Box<dyn std::error::Error>>(())
```

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

Workers are driven by async futures through `async_io::block_on`; the runtime
does not implement its own park/unpark mechanism. This lets async I/O wakes
participate in idle waiting.

## Lifecycle

Tasks are cancelled when their `Task` handle is dropped; call `detach()` for
background work. Graceful shutdown rejects new work and drains every accepted
task. `shutdown_now()` cancels outstanding work.

`LocalDomain` must be created and driven on its owner thread. Its inbox carries
only `Send` spawn commands: local runnables and `!Send` data never cross a
thread boundary.

See [the minimal API example](docs/api_example.md) and
[the Chinese README](docs/README_ZH.md).

## Status

The crate targets Rust 1.85+ on Windows, Linux, and macOS. It is licensed under
either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
