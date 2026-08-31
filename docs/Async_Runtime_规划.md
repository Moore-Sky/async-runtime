# Async Runtime 原始设计输入

> 本文件归档实现开始前的设计边界；当前实施契约以 [`plan.md`](plan.md) 为准。

## 目标模型

- package 名为 `async-runtime`，Rust crate 名为 `async_runtime`。
- General Runtime 创建并管理显式大小的 worker pool。
- General 使用 High、Normal、Background 三个 `async_executor::Executor`。
- 默认采用每 worker 独立的 `8:4:1` weighted-round-robin。
- 固定线程任务由宿主线程驱动的通用 `LocalDomain` 执行。
- `Runtime` 持有线程；`Spawner`、`LocalSpawner` 是可克隆的能力句柄。
- 不提供 worker ID spawn、CPU pinning、WASM、自定义 I/O reactor、blocking pool、
  抢占式调度、动态权重或万能 `spawn_on`。

## Task 语义

crate 提供统一的 `Task<T>` / `FallibleTask<T>`：

- `Task<T>` drop 即取消，`detach()` 后任务才继续在后台运行；
- `cancel()` 等待取消完成并返回 `Option<T>`；
- 普通 await 遇到取消时 panic，fallible await 遇到取消时返回 `None`；
- 用户 future 的 panic 传播给等待者，但不终止 executor worker。

## General 调度

三个 priority executor 共用 N 个 general workers。每个 worker 持有独立的加权选择器，
按调度机会而非完成任务数提供 `8:4:1` 公平性。任务不绑定 worker，future 可在不同 poll
之间由不同 worker 执行；调度是非抢占式的，不承诺全局绝对优先顺序。

worker 使用 `async_io::block_on` 驱动。idle 时同时等待三个 executor 的 `tick()` 和
shutdown future，使 async I/O wake 能参与唤醒。

为跨 priority 调度，v0.1 使用 `try_tick()` / `tick()`，不调用 `Executor::run()`，因此
不使用其 per-runner local queue 与 work stealing。后续是否基于 `async-task` 自建三队列
executor，由 benchmark 结果决定。

## LocalDomain 安全边界

`LocalDomain` 是 `!Send + !Sync`，在 owner thread 持有 `LocalExecutor`。`spawn_local`
允许 `!Send` future/result。`LocalSpawner` 是 `Send + Sync`，只接受 `Send` future/result。

跨线程 inbox 只传递 `Send` spawn command，绝不传递 local runnable，也不移动 `!Send`
数据。owner thread 收到命令后才调用 `LocalExecutor::spawn()`。

远程 local task 通过 bridge 暴露统一 `Task<T>`，终态必须区分：

```text
Completed(T)
Cancelled
Panicked(payload)
```

## 生命周期

Runtime 与每个 LocalDomain 分别使用：

```text
Running -> Closing -> Closed
```

spawn 成功线性化时增加本执行域的 accepted task 计数；任务 Completed、Cancelled 或
Panicked 时只减一次。graceful shutdown 先拒绝新任务，再等待计数归零；timeout 和 now
取消剩余任务。

显式 general shutdown 若在自身 worker 内调用，返回 `CalledFromWorker`。`Runtime::drop`
在自身 worker 内发生时发送 shutdown 信号但不 self-join，当前 worker 在 poll 返回后退出。

## 工程基线

- Rust 1.85，Edition 2024；
- Windows、Linux、macOS；
- `async-executor 1.14`、`async-task 4.7`、`async-io 2.6`、
  `async-channel 2.5`、`futures-lite 2.6`；
- `Apache-2.0 OR MIT` 双许可证。
