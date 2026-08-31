# async-runtime 实施计划

## 1. 目标

构建独立、通用的 `async-runtime` library crate：

- General Runtime 管理通用 worker pool。
- General Runtime 使用 High、Normal、Background 三个独立 executor。
- worker 数量必须由调用方显式配置。
- 默认采用可配置的 `8:4:1` 加权公平调度。
- 固定线程任务使用宿主线程驱动的 `LocalDomain`。
- crate 不内建 Render、Logic 等业务名称。

业务层的典型映射：

```text
当前主线程（Render Loop） -> LocalDomain / render_rt
线程 1（Logic）           -> LocalDomain / local_rt
其余通用 worker           -> Runtime / general
```

## 2. 公共 API

### General Runtime

```rust
pub enum Priority {
    High,
    Normal,
    Background,
}

pub struct PriorityWeights; // 默认 8:4:1
pub struct RuntimeBuilder;
pub struct Runtime;

#[derive(Clone)]
pub struct Spawner;

impl RuntimeBuilder {
    pub fn new(worker_threads: NonZeroUsize) -> Self;
    pub fn priority_weights(self, weights: PriorityWeights) -> Self;
    pub fn build(self) -> io::Result<Runtime>;
}

impl Runtime {
    pub fn spawner(&self) -> Spawner;

    pub fn spawn<F, T>(
        &self,
        priority: Priority,
        future: F,
    ) -> Result<Task<T>, SpawnError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static;

    pub fn shutdown_graceful(self) -> Result<(), ShutdownError>;
    pub fn shutdown_timeout(
        self,
        timeout: Duration,
    ) -> Result<ShutdownOutcome, ShutdownError>;
    pub fn shutdown_now(self) -> Result<(), ShutdownError>;
}

impl Spawner {
    pub fn spawn<F, T>(
        &self,
        priority: Priority,
        future: F,
    ) -> Result<Task<T>, SpawnError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static;
}
```

### Local Domain

```rust
pub struct LocalDomain;       // !Send + !Sync

#[derive(Clone)]
pub struct LocalSpawner;      // Send + Sync

impl LocalDomain {
    pub fn new() -> Self;
    pub fn spawner(&self) -> LocalSpawner;

    pub fn spawn_local<F, T>(&self, future: F)
        -> Result<Task<T>, SpawnError>
    where
        F: Future<Output = T> + 'static,
        T: 'static;

    pub fn is_empty(&self) -> bool;
    pub fn try_tick(&self) -> bool;
    pub async fn tick(&self);
    pub async fn run<F: Future>(&self, future: F) -> F::Output;

    pub async fn shutdown_graceful(self);
    pub async fn shutdown_timeout(
        self,
        timeout: Duration,
    ) -> ShutdownOutcome;
    pub fn shutdown_now(self);
}

impl LocalSpawner {
    pub fn spawn<F, T>(&self, future: F)
        -> Result<Task<T>, SpawnError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static;
}
```

每个固定线程创建并驱动自己的 `LocalDomain`。跨线程调用方只持有对应的
`LocalSpawner`。

### Task

```rust
Task<T>: Future<Output = T>
FallibleTask<T>: Future<Output = Option<T>>

Task::detach(self)
Task::cancel(self) -> impl Future<Output = Option<T>>
Task::fallible(self) -> FallibleTask<T>
Task::is_finished(&self) -> bool
```

语义与 smol 一致：

- `Task<T>` 被丢弃时取消任务。
- 只有调用 `.detach()`，任务才会在后台继续。
- 普通 await 遇到 runtime 强制取消时 panic。
- fallible await 遇到取消时返回 `None`。
- 任务 panic 会传播给等待它的调用方，但不会终止 worker。

## 3. General Runtime 实现

Runtime 内部维护三个 `async_executor::Executor`：

```text
High Executor
Normal Executor
Background Executor
```

所有 general worker 共同驱动三个 executor。任务不绑定 worker，同一个任务可以在
不同 poll 之间迁移到不同 general worker。

### 明确的 v0.1 调度取舍

为了在三个 priority 之间实现 weighted scheduling，general worker 使用
`Executor::try_tick()` / `Executor::tick()` 驱动 executor，而不调用
`Executor::run()`。因此 v0.1 暂不使用 `Executor::run()` 创建的 Runner、per-runner
local queue 和 work stealing 优化。

实际执行模型是：

```text
3 个 executor global queue
        +
每个 worker 独立的 weighted selector
        +
N 个 general workers
```

这是有意的取舍：v0.1 优先保证正确性和清晰的 priority 调度语义。benchmark 将测量
spawn、调度、wake/reschedule 和多 worker 竞争开销；如果该模型成为瓶颈，后续再评估
直接基于 `async-task` 实现自定义三队列 executor，而不是假定已经获得
`async-executor::run()` 的 local-queue/work-stealing 优化。

每个 worker 拥有独立的 weighted-round-robin 状态：

```text
High       8 次调度机会
Normal     4 次调度机会
Background 1 次调度机会
```

空优先级会立即尝试下一个优先级。当三个队列在选择时都非空，单个 worker 的 selector
按 weights 提供调度机会。多个 worker 各自维护 selector，任务耗时也可能不同，因此不保证：

- 绝对全局优先顺序；
- 已完成任务数、CPU 时间或吞吐量严格等于 `8:4:1`；
- 抢占正在执行的 future poll；
- 固定延迟或吞吐 SLA。

worker 使用 `async_io::block_on` 驱动循环。三个 executor 都暂时无任务时，同时等待：

- `High::tick()`；
- `Normal::tick()`；
- `Background::tick()`；
- shutdown 信号。

这样 `async-io` reactor 的 I/O wake 可以直接唤醒任务。Runtime 不自行实现 worker
park/unpark 机制；idle waiting 统一交由 async future 与 `async_io::block_on` 驱动。
idle wait 使用可取消的 race；未获胜的 `tick()` future 被 drop 时会注销对应 sleeper，
并通过压力测试验证不会遗留 stale waiter、丢失 wake 或产生忙等。

## 4. LocalDomain 实现

`LocalDomain` 在创建它的线程中持有 `async_executor::LocalExecutor`：

- `spawn_local` 直接创建本地任务，允许 `!Send` future 和结果。
- `LocalSpawner::spawn` 只接受 `Send` future 和结果。
- 跨线程任务先进入 inbox，再由 owner 线程创建真正的 local task。

**安全性不变量：** inbox 只承载包含 `Send` future/result 的 spawn command；其中绝不
传递 local runnable，也不跨线程移动 `!Send` 数据。只有 owner 线程可以调用
`LocalExecutor::spawn()`，并执行或销毁 local runnable。该不变量必须写入实现源码注释，
并由类型约束和测试共同保护。

`LocalSpawner::spawn` 成功返回代表任务已经被 domain 接受。实际执行仍要求 owner
线程调用：

```text
try_tick / tick / run / shutdown_graceful / shutdown_timeout
```

Render Loop 每帧使用 `try_tick()` 非阻塞地处理任务。专用 Logic 线程可使用 `run()`
持续驱动任务。

远程 local task 使用延迟桥接状态机统一成 `Task<T>`，正确处理以下竞争：

- command 尚未被 owner 线程接收时，Task 被 drop；
- command 尚未被接收时，Task 被 detach；
- task 创建、cancel 与 domain shutdown 同时发生；
- domain 关闭后，旧 `LocalSpawner` 再次 spawn。

桥接层必须显式保留用户 future 的三种终态，不能把 panic 混同为 channel 关闭或取消：

```text
Completed(T)
Cancelled
Panicked(payload)
```

`LocalSpawner::spawn` 的用户 future 在 owner thread 上被执行，并将上述终态写入 bridge
completion state；panic 保存为 `Box<dyn Any + Send + 'static>`。普通 `Task<T>` await
收到 `Panicked(payload)` 时使用 `resume_unwind(payload)` 重新传播原 panic；`Cancelled`
保持普通 await panic、`FallibleTask<T>` await 返回 `None` 的统一 Task 语义。fallible await
也必须重新传播 `Panicked`，不能将它降级为 `None`。终态只允许发布一次，且必须覆盖
command 尚未 materialize、task 正在运行和 domain 正在关闭时的竞争。

## 5. 生命周期与关闭

Runtime 和 LocalDomain 都使用统一生命周期：

```text
Running -> Closing -> Closed
```

spawn 与关闭通过同一个线性化入口同步：

- 关闭前成功接受的任务参与 drain 或 cancel。
- 进入 Closing 后的 spawn 返回 `SpawnError::Closed`。
- `Spawner` 和 `LocalSpawner` 使用弱生命周期能力，不阻止 runtime/domain 销毁。

General Runtime 和每个 LocalDomain 各自维护 `accepted_tasks: AtomicUsize`，计数范围只属于
该执行域：

```text
spawn 成功线性化                 -> +1
Completed / Cancelled / Panicked -> -1（且只减一次）
```

General task 在进入对应 executor 前计数；远程 local task 在 inbox command 被接受时计数，
而不是等 owner thread 创建真正的 local task 后才计数。因此尚未 materialize 的 command、
detached task 和 bridge task 都属于 graceful drain。`executor.is_empty()`、inbox 是否为空或
Task handle 是否仍存在，都不能替代该计数。任务包装中的 completion guard 负责在正常完成、
取消清理和 panic unwind 后准确减一并唤醒 shutdown waiter。

原子计数器本身不承担 spawn/close 线性化。实现必须在同一个 lifecycle gate 内完成
“确认仍为 `Running`、计数加一、把任务所有权提交给 executor 或 local inbox”；提交失败时
在返回 `SpawnError::Closed` 前回滚计数。只要 spawn 返回成功，执行域就必须已经持有任务、
command 或明确的取消记录，不能因并发 Drop/Closing 将它静默遗失。

关闭方式：

- `shutdown_graceful`：拒绝新任务，等待 `accepted_tasks == 0`。
- `shutdown_timeout`：等待到期限；超时后取消剩余任务。
- `shutdown_now`：拒绝新任务并立即取消剩余任务。
- Drop：执行不可报告错误的 `shutdown_now` 语义，避免遗留 worker。

General shutdown 最后会停止并 join 所有 worker。从 runtime 自己的 worker 内同步调用
显式 shutdown 时返回 `ShutdownError::CalledFromWorker`，且绝不尝试 self-join。由于这些
API 消耗 `Runtime`，该错误路径仍会发出 shutdown 信号、处理其他 worker 的 JoinHandle，
并分离当前 worker 的 JoinHandle，让它在当前 poll 返回后自行退出。

`Runtime::drop` 无法报告 `CalledFromWorker`：它总是先发出 shutdown 信号；如果当前线程
属于自身 worker，则不 join 当前 worker，只处理其他 JoinHandle 并分离当前 handle；从
外部线程 Drop 时正常 join 全部 worker。这样即使最后一个 owning `Runtime` handle 在自身
worker 上被 drop，也不会死锁或 self-join。

Rust async 无法抢占卡在单次同步 `poll` 中的代码；任何 shutdown 都必须等待该 poll
返回后才能完成线程回收。

## 6. 项目结构

```text
src/
  lib.rs
  priority.rs
  runtime.rs
  worker.rs
  local.rs
  task.rs
  lifecycle.rs
  error.rs

tests/
  general.rs
  priority.rs
  local.rs
  cross_domain.rs
  task.rs
  shutdown.rs
  async_io.rs
  compile_fail.rs
  ui/

examples/
  priority.rs
  local_domain.rs

benches/
  executor.rs
```

其他文件：

- `README.md`：英文说明。
- `docs/README_ZH.md`：对应中文说明。
- `docs/api_example.md`：Render、Logic、General 的最简使用示例。
- `docs/Async_Runtime_规划.md`：保留原始设计输入。
- `LICENSE-MIT` 与 `LICENSE-APACHE`：双许可证。
- `.github/workflows/ci.yml`：基础 CI。

## 7. 依赖基线

```text
async-executor 1.14
async-task     4.7
async-io       2.6
async-channel  2.5
futures-lite   2.6
criterion      0.7
trybuild       MSRV 兼容版本
```

- package 名：`async-runtime`。
- Rust crate 名：`async_runtime`。
- 版本：`0.1.0`。
- Edition：2024。
- MSRV：Rust 1.85。
- 许可证：`Apache-2.0 OR MIT`。
- 支持原生 Windows、Linux、macOS。

## 8. 测试与验收

### General Runtime

- 三个 executor 队列互相隔离。
- 多 worker 可以并发执行。
- general task 可在不同 worker 之间迁移。
- worker 内可以嵌套 spawn 并 await。
- 只对纯 selector 做精确测试：三个 queue 均视为 non-empty 时，连续 13 次选择必须为 `H H H H H H H H N N N N B`。
- 不对多 worker 的已完成任务数、CPU 时间或吞吐量作 `8:4:1` 精确断言。
- 持续 High 负载下，Normal 和 Background 最终仍能获得调度机会。
- idle 后可被 channel、timer 和 loopback I/O 可靠唤醒。

### LocalDomain

- local future 始终在 owner 线程 poll。
- `Rc` 等 `!Send` future 可通过 `spawn_local` 执行。
- 跨线程 `LocalSpawner` 投递可以唤醒空闲 local driver。
- 通过实现审查和类型约束确认 inbox command 不包含 local runnable 或 `!Send` payload。
- General -> Local、Local -> General 均能正常 await。
- 子任务完成后，父任务回到自己的原执行域。
- 远程 `LocalSpawner` task panic 后，普通 await 收到原 panic payload，而不是
  `Cancelled`；对应 fallible await 也不能把 panic 降级成 `None`。

### Task 与关闭

- await、drop-cancel、detach、显式 cancel、fallible。
- panic 传播且 worker 在任务 panic 后继续工作。
- graceful、timeout、shutdown-now。
- spawn/close 并发线性化。
- pending I/O 与 detached task 的关闭行为。
- worker 内错误调用 shutdown。
- 最后一个 Runtime owner 在自身 worker 上 drop 时不死锁、不 self-join，其他 worker
  能正常回收，当前 worker 在 poll 返回后退出。
- worker 创建部分失败时清理已创建线程。

### 编译期约束

- General spawn 拒绝 `!Send` future/result。
- `LocalSpawner::spawn` 拒绝 `!Send` future/result。
- `LocalDomain::spawn_local` 接受 `!Send` future/result。
- `LocalDomain` 为 `!Send + !Sync`。
- `LocalSpawner` 为 `Send + Sync`。

## 9. 实施顺序

1. 完成 error、priority、lifecycle 与统一 Task。
2. 完成 general executor、worker 调度和 Runtime shutdown。
3. 完成 LocalDomain inbox、远程 Task 桥接和 local shutdown。
4. 添加行为测试与 compile-fail 测试。
5. 添加示例、README、许可证、benchmark 和 CI。
6. 执行 fmt、clippy、tests、doc tests 和 MSRV 验收。

## 10. 明确不支持

首个版本不提供：

- 指定 general worker ID；
- OS CPU pinning 或 NUMA 调度；
- Render、Logic 等内建业务 domain；
- 万能 `Target`、`Affinity` 或 `spawn_on`；
- WASM；
- 自定义 I/O reactor；
- blocking pool；
- 抢占式调度；
- 动态修改 priority weights。
