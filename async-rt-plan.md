# async-runtime CPU Crossover Decision Pass

## 决策与时间盒

本轮只回答一个工程问题：固定提交 64 个独立 `Priority::Normal` CPU tasks 时，单 task 多大才值得使用 async-runtime 多线程？

时间盒为 45–60 分钟，最迟 75 分钟停止。成功标准是得到类似“crossover 位于 `(20 us, 50 us]`，当前 scheduler 没有明显异常”的可复核结论，而不是建设通用性能平台。

本轮不修改 runtime/scheduler，不升级版本，不新增依赖，不全量重跑旧 guards，不做 bootstrap 框架、verified-steal 或 scheduler candidate。

## Benchmark

新增 Criterion target `cpu_crossover`：

- `tasks = 64`；
- 对比 inline 与 Runtime 1/2/4/8 workers；
- Rapid 档位目标为 `0/2/5/10/20/50/100/200 us/task`；
- 使用固定迭代整数 kernel，task 内不读时钟；
- 在本机 release 模式快速校准后，将 rounds 固化到源码；
- runtime 创建、worker 启动、预热和 shutdown 位于计时外；
- timed region 从首次 spawn 到 64 个 task 全部完成并消费 checksum；
- runtime 在 samples 间持续复用，不强制 worker park；
- Criterion 使用 `SamplingMode::Flat`。

workers=1 和零工作档只用于 Rapid 解释固定调度税。Formal 只测 inline 与 2/4/8 workers。

现有 `v030_cpu_workload` 保持 scaling diagnostic，不参与 crossover 决策，因为它随 worker 数改变 parent topology。

在 `v030_yield_wake_storm` 的 external-wake workload 中增加单 task case，测已注册 Pending task 的 `wake -> complete`；不向 yield-storm 添加单 task case。

## 执行步骤

### 1. 实现与 Smoke

- 新增 benchmark target 和 external wake-once case；
- release 编译；
- Criterion `--test` 各 case 执行一次；
- 验证 checksum、无 hang、无 lost wake。

### 2. Rapid 粗扫

- warmup 0.5 秒；
- measurement 2 秒；
- 20 samples；
- 99% Criterion confidence；
- 1% significance；
- 扫描全部 8 个 workload × inline/1/2/4/8 workers。

Rapid 用来选 Formal 档位，不作为最终 crossover 证据。选择各 worker crossover 附近的统一最小集合，通常为“最后一个输/接近、首个赢、下一个更大档位”共 3 个；只有边界不清楚时扩为 4 个。

### 3. Formal 邻域验证

- 5 个独立进程轮次；
- 每轮 warmup 3 秒、measurement 5 秒、100 samples；
- 99% Criterion confidence、1% significance；
- 只测选出的 3–4 个 workload × inline/2/4/8 workers；
- 预计总计 60–80 case-runs。

五轮必须轮换 case 顺序。使用固定 seed 的五个排列，实际顺序与 workload filter 写入 manifest。4 个 case 在 5 轮中不可能完全位置平衡，因此 manifest 同时记录各 case 的平均位置；选择让 inline 略靠前的保守排列，避免 8-worker 因总在冷机早位而受益。不得每轮固定按 inline、2、4、8 执行。

## 简化判定

每轮使用 Criterion `estimates.json` 的 mean point estimate 和 99% confidence interval：

```text
speedup = inline_mean / runtime_mean
speedup_lower_bound = inline_mean_CI_lower / runtime_mean_CI_upper
```

某 workload 对某 worker 数“稳定胜出”需要：

- 5 轮中至少 4 轮 `runtime_mean < inline_mean`；
- 5 轮 point speedup 中位数至少 `1.05x`；
- 至少 4 轮 `speedup_lower_bound > 1.0x`。

`speedup_lower_bound` 是便宜、保守的工程判断，不宣称为 ratio 的严格 99% confidence interval。不 pool 五轮 samples，也不对 5 个 point estimates 计算 CI。

crossover 是第一个稳定胜出且下一个更大 Formal workload 也稳定胜出的档位，报告为相邻测试档位区间，不插值。如果邻域数据不足以支持连续两档，则明确报告未定位。

## Runner 与证据

在现有 v0.3 PowerShell runner 风格上做最小增量，不建设通用 pipeline：

- 必填唯一 run ID；
- 目标目录存在则拒绝覆盖；
- 记录 commit、工作区、Cargo.lock 与本轮源文件 SHA-256、rustc、target、CPU/核心数、OS、电源方案、flags、命令、workload filter、case order 和平均位置；
- 五轮独立启动 `cargo bench`；
- 简单脚本从 Criterion estimates 汇总 CSV 并应用上述规则。

只提交以下白名单证据：

```text
raw/<round>/<case>/sample.json
raw/<round>/<case>/estimates.json
raw/<round>/<case>/benchmark.json
raw/<round>/stdout.log
derived/cases.csv
derived/crossover.csv
manifest/<round>/environment.json
```

不提交 Criterion HTML、plots、cache 或整个 target 目录。

## 验证与提交

本轮最低验证：

- `cargo fmt --all -- --check`；
- `cargo clippy --all-targets --all-features -- -D warnings`；
- `cargo test --all-features`；
- `cargo test --doc --all-features`；
- `cargo bench --locked --bench cpu_crossover --no-run`；
- `cargo bench --locked --bench v030_yield_wake_storm --no-run`。

只做一个提交，包含 benchmark、runner、冻结 rounds、白名单数据和简短报告。报告回答 crossover 区间、1-worker/zero-work 固定税、wake-once 固定税，以及证据是否支持启动 scheduler candidate。
