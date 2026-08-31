# async-runtime

`async-runtime` 是一个面向 smol 生态的、支持优先级的原生 Rust 异步运行时：通用任务使用
三个优先级队列；固定线程任务由宿主线程驱动的 `LocalDomain` 执行。

```rust,no_run
use async_runtime::{Priority, RuntimeBuilder};
use std::num::NonZeroUsize;

let runtime = RuntimeBuilder::new(NonZeroUsize::new(4).unwrap()).build()?;
runtime.spawn(Priority::High, async { /* Send 任务 */ })?.detach();
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Smol 生态

运行时直接构建于 `async-executor`、`async-task`、`async-io`、`async-channel` 和
`futures-lite`。这些 crate 提供的 future、channel 与异步 I/O 类型可以直接在任务中
await，因此能够自然配合面向 smol 生态的库使用，不要求 Tokio runtime。

需要注意，调用 `smol::spawn` 仍会提交到 smol 自己的全局 executor。需要参与本 crate 的
优先级调度、任务计数或 shutdown 时，应通过 `Runtime` / `Spawner` 提交。

没有线程亲和要求的 `Send` 任务提交给 `Runtime` / `Spawner`。每个固定宿主线程各自创建一个
`LocalDomain`；远程调用方只持有它的 `LocalSpawner`，只能提交 `Send` 任务。

## 调度语义

优先级为 `High`、`Normal`、`Background`。每个 general worker 有独立的加权选择器，默认
权重为 `8:4:1`。它表示持续负载下的调度机会，不表示全局严格优先级、完成数比例或吞吐 SLA。

为跨优先级调度，v0.1 使用 `Executor::try_tick()` / `tick()` 驱动三个 executor。因此实际模型
是“三个共享 global queue + N 个 worker”，不会使用 `Executor::run()` 创建的 per-runner local
queue 与 work stealing 优化。这是有意的正确性和语义优先取舍；基准结果将决定以后是否基于
`async-task` 改为自定义调度器。

worker 由 `async_io::block_on` 驱动，运行时不自行实现 worker 的 park/unpark；空闲等待统一
交由 async future 和 `async-io` 的 I/O wake 机制处理。

## 关闭与线程安全

丢弃 `Task` 会取消任务，调用 `detach()` 才会让任务后台继续。graceful shutdown 拒绝新任务并
等待所有已接受任务结束；`shutdown_now()` 取消剩余任务。

`LocalDomain` 只能在其 owner thread 创建和驱动。跨线程 inbox 只传递 `Send` spawn command，
绝不传递 local runnable 或 `!Send` 数据。

英文说明见 [README.md](README.md)。

项目使用 Edition 2021，MSRV 为 Rust 1.71。原生支持目标为 Windows、Linux、macOS、
Android 和 iOS。

CI 维护两条兼容线：MSRV lane 使用 Rust 1.71 和仓库中已知兼容的 `Cargo.lock`；Latest
lane 在最新 stable Rust 上执行 `cargo update`，验证依赖允许范围内的最新版本。这样即使
smol 生态以后提高 MSRV，本 crate 也能继续使用最后一个兼容版本，直到主动决定提高 MSRV。

v0.1 明确不支持 WASM。这不是尚未补齐的平台适配，而是因为本运行时的语义依赖原生多线程
general worker pool，以及不同执行域之间的消息互投；裁剪成单线程 WASM 会变成另一种
runtime 模型。

移动端宿主仍需负责 App 生命周期，并在正确的 UI、Render 或 Logic 线程驱动
`LocalDomain`。CI 会交叉检查 Android/iOS ARM64 目标、在 Android 模拟器中运行核心测试，
并为 ARM64 iOS Simulator 编译和链接测试套件；在 iOS 中实际执行仍需要 XCTest 宿主 App。
许可证为 MIT 或 Apache-2.0 二选一。
