# async-runtime

`async-runtime` 是一个原生 Rust 异步运行时：通用任务使用三个优先级队列；固定线程任务由
宿主线程驱动的 `LocalDomain` 执行。

```rust,no_run
use async_runtime::{Priority, RuntimeBuilder};
use std::num::NonZeroUsize;

let runtime = RuntimeBuilder::new(NonZeroUsize::new(4).unwrap()).build()?;
runtime.spawn(Priority::High, async { /* Send 任务 */ })?.detach();
# Ok::<(), Box<dyn std::error::Error>>(())
```

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

最简业务接线见 [API 示例](api_example.md)。完整设计和验收约束见 [实施计划](plan.md)。

支持 Rust 1.85+、Windows、Linux、macOS；许可证为 MIT 或 Apache-2.0 二选一。
