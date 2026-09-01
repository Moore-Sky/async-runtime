# async-runtime

`async-runtime` 是一个面向 smol 生态的、支持优先级的原生 Rust 异步运行时：通用任务使用
三个优先级队列；固定线程任务由宿主线程驱动的 `LocalDomain` 执行。v0.2 新增了适合宿主循环的
预算驱动，以及轻量的跨线程 dispatch。

```rust,no_run
use async_runtime::{Priority, RuntimeBuilder};
use std::num::NonZeroUsize;

let runtime = RuntimeBuilder::new(NonZeroUsize::new(4).unwrap()).build()?;
runtime.spawn(Priority::High, async { /* Send 任务 */ })?.detach();
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Smol 生态

运行时直接构建于 `async-executor`、`async-task`、`async-channel` 和 `futures-lite`。
它只调度 future，不拥有或驱动 I/O reactor。应用负责驱动自己选择的 I/O runtime；宿主
驱动 `async-io` 时，其 I/O future 可以自然地在本运行时任务中 await。

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

worker 使用 `futures_lite::future::block_on` 等待 executor 与 shutdown future。运行时既不
实现 I/O reactor，也不要求宿主选择特定 I/O runtime。

## 关闭与线程安全

丢弃 `Task` 会取消任务，调用 `detach()` 才会让任务后台继续。graceful shutdown 拒绝新任务并
等待所有已接受任务结束；`shutdown_now()` 取消剩余任务。

`LocalDomain` 只能在其 owner thread 创建和驱动。跨线程 inbox 只传递 `Send` spawn command，
绝不传递 local runnable 或 `!Send` 数据。

## 宿主驱动的 `LocalDomain`（v0.2）

`LocalDomain` 面向 UI、渲染、游戏等由宿主拥有的固定线程。必须在同一个 owner thread 上创建并
调用其驱动方法；它不会自行创建后台线程，也不会在后台自动运行。

当宿主循环按“驱动步数”而不是时间分配工作时，使用 `run_n`：

```rust,no_run
# use async_runtime::LocalDomain;
let domain = LocalDomain::new();

// 一次宿主循环中，最多进行 64 个非阻塞驱动步骤。
let progressed = domain.run_n(64);
// 接着更新 UI、渲染帧或执行宿主循环中的其他工作。
# let _ = progressed;
```

`run_n(max_steps)` 不阻塞。它的限制和返回值都是**驱动步骤数**，不是完成的 future 数量，也不是
精确的 `Future::poll` 次数。每一步遵循 `try_tick` 的推进策略：可能 materialize 一个远程 inbox
command，并给已经本地化的 runnable 一次执行机会。因此同一个任务可能需要多次调用才完成；
`run_n(0)` 不执行任何工作并返回 `0`。

当事件循环或一帧有明确时间额度时，使用 `run_for`：

```rust,no_run
# use async_runtime::LocalDomain;
# use std::time::Duration;
let domain = LocalDomain::new();

let stats = domain.run_for(Duration::from_micros(500));
// `stats.drive_steps` 为总驱动推进量；`stats.inbox_commands` 为本次
// materialize 的远程 command 数量。
debug_assert!(stats.elapsed >= Duration::ZERO);
```

`run_for(budget)` 会在每个驱动步骤之前检查预算，并在 domain 空闲或预算耗尽时停止。
`Duration::ZERO` 不执行任何工作，返回零进度。这是**软时间预算**：Rust 无法安全抢占一个正在被
poll 的 future，故 `RunStats::elapsed` 可以因某一次 poll 超过请求的预算。对帧延迟敏感时，应让
每次 poll 足够短并主动协作让出执行权。

`RunStats` 提供 `drive_steps`、`inbox_commands` 和 `elapsed`，可用于展示每帧推进量或发现 backlog 趋势；
它是运行反馈，不是实时 deadline 保证。

### 跨线程 fire-and-forget dispatch

当调用者需要 `Task<T>`、结果、取消或观察 panic 时，仍应使用 `LocalSpawner::spawn`。对于 owner
thread callback 或单向异步移交，使用更轻量的 fire-and-forget API：

```rust,no_run
# use async_runtime::LocalDomain;
let domain = LocalDomain::new();
let local = domain.spawner();

local.dispatch(|| {
    // 稍后在 `domain` 的 owner thread 上运行。
    // 在这里提交 UI、GPU 或其他线程亲和状态。
})?;

local.dispatch_future(async move {
    // 一个 Send future，最终在 owner thread 上完成。
})?;
# Ok::<(), async_runtime::SpawnError>(())
```

两种方法都只接收 `Send + 'static` 工作；domain 开始关闭或已不存在时返回
`SpawnError::Closed`，并把工作放入 inbox，等待 owner thread materialize。它们不返回 task
handle，因此没有结果 channel、取消 handle 或完成通知。dispatch 在 inbox 中按 FIFO 顺序处理，
但异步 future 的**完成顺序**不因此得到保证。

被 dispatch 的工作 panic 时会被隔离，`LocalDomain` 仍可继续驱动。Rust 已安装的 panic hook
仍会被调用；需要观测此类失败的应用应配置 panic hook 或日志集成。

v0.2 的跨线程 inbox 有意保持**无界**。`dispatch`、`dispatch_future` 和远程 `spawn` 都不提供
背压；若生产者快于 owner thread，队列可能无限增长并持续占用内存。应在调用方限制生产速度、
合并重复更新，或更频繁地驱动 domain。

## 示例导航

编号示例从 general worker pool 到宿主驱动的 local domain 逐步展开。任一示例可通过
`cargo run --example <名称>` 运行。

1. [01_quick_start](examples/01_quick_start.rs) — 创建 general `Runtime`、
   提交 `Send` 任务、等待结果并优雅关闭。
2. [02_priority](examples/02_priority.rs) — 提交 `High`、`Normal`、`Background`
   任务；优先级表达调度偏好，不保证全局执行顺序。
3. [03_budgeted_local](examples/03_budgeted_local.rs) — 将 `!Send` 状态留在
   owner thread，用 `run_n` 或带软预算的 `run_for` 驱动。
4. [04_cross_thread_dispatch](examples/04_cross_thread_dispatch.rs) — 从另一
   线程提交 callback 和 `Send` future；必须由 owner 驱动 domain 后才会执行。
5. [05_domain_composition](examples/05_domain_composition.rs) — 双向组合 general
   和 local 任务，避免阻塞 owner loop。
6. [06_task_lifecycle](examples/06_task_lifecycle.rs) — 演示等待、取消、detach、
   完成状态和 task panic。
7. [07_shutdown](examples/07_shutdown.rs) — local 优雅 drain、general 超时关闭、
   立即取消，以及关闭后提交被稳定拒绝的情形。
8. [90_best_practice_host_loop](examples/90_best_practice_host_loop.rs) — UI、
   Render、Game 宿主循环的实际形态：每帧使用预算驱动。

## 性能场景

性能 workload 按问题拆成独立文件，而不是隐藏在一个巨型 benchmark 中。运行方式：
`cargo bench --bench <名称>`。

- `general_spawn`：批量 spawn/await 与 nested spawn。
- `priority`：单优先级及 8:4:1 混合吞吐。
- `local_driving`：排除 setup 后的 `run_n` / `run_for` 驱动成本。
- `local_dispatch`：真实跨线程 producer，对比完整结果 bridge 与两种 fire-and-forget 路径。
- `frame_like`：100 us、500 us、1 ms 预算下的预装 inbox 工作。
- `shutdown`：优雅 drain 与立即取消。
- `yield_storm`：大量任务反复 yield 和重新入队。

测试环境、计时边界及结果见 [v0.2 性能基线](benchmarks/baseline-v0.2.md)。

英文说明见 [README.md](README.md)。

项目使用 Edition 2021，MSRV 为 Rust 1.71。原生支持目标为 Windows、Linux、macOS、
Android 和 iOS。

CI 维护两条兼容线：MSRV lane 使用 Rust 1.71 和仓库中已知兼容的 `Cargo.lock`；Latest
lane 在最新 stable Rust 上执行 `cargo update`，验证依赖允许范围内的最新版本。这样即使
smol 生态以后提高 MSRV，本 crate 也能继续使用最后一个兼容版本，直到主动决定提高 MSRV。

本运行时明确不支持 WASM。这不是尚未补齐的平台适配，而是因为本运行时的语义依赖原生多线程
general worker pool，以及不同执行域之间的消息互投；裁剪成单线程 WASM 会变成另一种
runtime 模型。

移动端宿主仍需负责 App 生命周期，并在正确的 UI、Render 或 Logic 线程驱动
`LocalDomain`。CI 会交叉检查 Android/iOS ARM64 目标、在 Android 模拟器中运行核心测试，
并为 ARM64 iOS Simulator 编译和链接测试套件；在 iOS 中实际执行仍需要 XCTest 宿主 App。
许可证为 MIT 或 Apache-2.0 二选一。
