# async-runtime

`async-runtime` 是一个面向 smol 生态的、支持优先级的原生 Rust 异步运行时：通用 worker pool
支持 work stealing；固定线程任务由宿主线程驱动的 `LocalDomain` 执行。v0.3 新增基于
`async-task` 的自定义 scheduler、worker-local queue、priority global injector、work stealing
与 parked-worker 唤醒；v0.2 的预算驱动和轻量跨线程 dispatch 仍保持可用。

```rust,no_run
use async_runtime::{Priority, RuntimeBuilder};
use std::num::NonZeroUsize;

let runtime = RuntimeBuilder::new(NonZeroUsize::new(4).unwrap()).build()?;
runtime.spawn(Priority::High, async { /* Send 任务 */ })?.detach();
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Smol 生态

general runtime 使用 `async-task` 提供 task machinery，使用 `crossbeam-deque` 提供队列；
`LocalDomain` 继续使用 `async-executor`。`async-channel` 和 `futures-lite` 支持 local-domain
和 task API。运行时只调度 future，不拥有或驱动 I/O reactor。应用负责驱动自己选择的 I/O
runtime；宿主驱动 `async-io` 时，其 I/O future 可以自然地在本运行时任务中 await。

需要注意，调用 `smol::spawn` 仍会提交到 smol 自己的全局 executor。需要参与本 crate 的
优先级调度、任务计数或 shutdown 时，应通过 `Runtime` / `Spawner` 提交。

没有线程亲和要求的 `Send` 任务提交给 `Runtime` / `Spawner`。每个固定宿主线程各自创建一个
`LocalDomain`；远程调用方只持有它的 `LocalSpawner`，只能提交 `Send` 任务。

## General scheduler（v0.3）

优先级为 `High`、`Normal`、`Background`。每个 general worker 有独立的加权选择器，默认
权重为 `8:4:1`。它表示持续负载下的调度机会，不表示全局严格优先级、完成数比例或吞吐 SLA。

v0.3 的 general `Runtime` 中，每个 worker 的每个优先级各有一个 FIFO local queue，同时每个
优先级有一个 global injector。由该 runtime 的 worker 调度的 runnable 会进入本 worker 对应的
local queue；由其他线程调度的 runnable 会进入对应的 global injector。因此 nested task 和
self-wake task 会优先获得 locality，但 general task 并不具备 worker thread affinity。

每次按权重选择优先级时，worker 通常依次尝试自己的 local queue、从同优先级 global injector
批量取得任务、以及轮转地从其他 worker 窃取任务。在一个有界 local burst 后会先检查 global
injector，避免持续 self-wake 的 local source 让同优先级外部提交长期饥饿。stealing 能使任务在 worker 间迁移，因此不能
从 general task 的提交线程推导它的执行线程；需要真正线程亲和时请使用 `LocalDomain`。

空闲 worker 在检查队列后通过 condition variable park。提交工作时会唤醒一个 parked worker；
shutdown 时会唤醒全部 worker。它避免 general pool 空闲时 busy waiting，但这是实现机制，
不是延迟或功耗保证。

priority 是调度偏好，不是全局严格顺序、完成比例、吞吐 SLA，也不是无 starvation 的 deadline
服务。work stealing 与 per-worker selector 用于提高已排队工作的可获得性；若需要 realtime 行为，
应用仍须保持每次 poll 很短、自己限制工作量并在目标宿主上实测。

运行时既不实现 I/O reactor，也不要求宿主选择特定 I/O runtime。

### 可选 scheduler 统计

启用 `stats` feature 后可取得某一时刻的 `RuntimeStats` 快照：

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

`RuntimeStats` 包含近似的 runnable queue 数量、sleeping worker、执行次数、steal、提交来源、
park 和 wake 通知。它用于诊断和解读 benchmark：读取快照时并发活动仍可改变数值，queue 数量也
不是任务完成数或存活数。未启用该 feature 时，公共 API 中不会出现 `RuntimeStats` 和
`Runtime::stats()`。

### v0.3 API 兼容性与限制

`RuntimeBuilder`、`Runtime`、`Spawner`、`Task`、`FallibleTask`、priority 与 shutdown API 均保持
v0.2 的公共形状。v0.3 改变的是 general scheduler 内部实现；代码应依赖已文档化的 priority 与
task lifecycle 语义，而非旧的 queue 或 worker 选择细节。普通的 `spawn(priority, future)` 调用不
需要迁移。

general queue 仍然无界，不对提交提供背压。task 是协作式的：一次过长的同步 `Future::poll` 会延迟
priority 选择、stealing、shutdown 推进和 wake 处理。task panic 会按既有 `Task` 语义通过 task
handle 报告；需要观察时请保留 handle 或配置应用自己的 panic hook。

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
8. [08_nested_worker_locality](examples/08_nested_worker_locality.rs) — nested
   general work 优先进入所在 worker 的 local queue；它不是线程亲和，也可能被 steal。
9. [09_external_multi_producer](examples/09_external_multi_producer.rs) — 多个
   线程通过克隆的 `Spawner` 向 global injector 提交工作。
10. [10_priority_fairness](examples/10_priority_fairness.rs) — High work 持续
    yield 时 Background work 仍会推进；它演示按权重的机会，不是 realtime 保证。
11. [11_idle_wake](examples/11_idle_wake.rs) — 外部提交会唤醒 idle worker；其打印的
    时间不应用作测量结论，应使用 benchmark。
12. [12_custom_priority_weights](examples/12_custom_priority_weights.rs) — 设置
    非零的三档 priority 比例，同时保留各 priority 最终获得机会的条件。
13. [13_scheduler_stats](examples/13_scheduler_stats.rs) — 查看可选的近似计数器
    （`cargo run --example 13_scheduler_stats --features stats`）。
14. [90_best_practice_host_loop](examples/90_best_practice_host_loop.rs) — UI、
   Render、Game 宿主循环的实际形态：每帧使用预算驱动。

## 性能场景

性能 workload 按问题拆成独立文件，而不是隐藏在一个巨型 benchmark 中。运行方式：
完整运行使用 `cargo bench`；单个场景使用 `cargo bench --bench <名称>`。

- `general_spawn`：批量 spawn/await 与 nested spawn；nested work 也覆盖 worker-local routing。
- `priority`：scheduler 下的单优先级及 8:4:1 混合吞吐。
- `local_driving`：排除 setup 后的 `run_n` / `run_for` 驱动成本。
- `local_dispatch`：真实跨线程 producer，对比完整结果 bridge 与两种 fire-and-forget 路径。
- `frame_like`：100 us、500 us、1 ms 预算下的预装 inbox 工作。
- `shutdown`：优雅 drain 与立即取消。
- `yield_storm`：大量任务反复 yield 和重新入队；可用于压力观察 local routing、global injection
  与 stealing。
- `v030_external_producers`：1–16 个外部 producer 的提交竞争。
- `v030_nested_locality`：不同 worker 与 child 数量下的 nested spawn/completion。
- `v030_steal_imbalance`：parent 创建可 yield 的 child，通过公共 API 近似不均衡 local queue。
- `v030_yield_wake_storm`：分别覆盖 cooperative yield 与外部唤醒 pending task 的 storm。
- `v030_priority_latency` 与 `v030_starvation`：High work 已排队时的有界 probe-progress
  场景；它们不是 SLA，也不证明无限任务流的语义。
- `v030_idle_wake`：完整的 park/submit/wake/re-park 周期（使用
  `cargo bench --bench v030_idle_wake --features stats`）；CPU 使用情况仍需要 OS profiler。

比较 scheduler 版本时，应保持机器、Rust toolchain、workload 参数和 release profile 相同。
这些 benchmark 描述的是场景，不宣称 v0.3 一定比旧版本或其他 runtime 更快。环境与计时边界见
[v0.2 性能基线](benchmarks/baseline-v0.2.md)和
[v0.3 性能基线](benchmarks/baseline-v0.3.md)。

功能测试运行 `cargo test`；包含可选 observability 的测试运行 `cargo test --all-features`；
文档示例测试运行 `cargo test --doc`。

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
