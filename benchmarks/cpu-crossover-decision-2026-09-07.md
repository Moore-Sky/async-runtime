# CPU crossover decision pass — 2026-09-07

## Decision

For a batch of 64 independent `Priority::Normal` CPU tasks on the measured
Ryzen 7 8845H machine, async-runtime's multi-threaded Runtime becomes worthwhile
between zero work and approximately 2.1 us of measured inline work per task.

At the first non-zero formal workload, all 2/4/8-worker configurations beat
inline execution in all five observed process rounds, their conservative bounds
also exceeded 1.0 in all five rounds, and their median speedups exceeded 1.05x.
Round 1 overlapped unrelated validation load; excluding it leaves all three
worker configurations passing in 4/4 clean rounds and does not change the
decision. The 8-worker result is only 1.14x at this boundary but grows to 4.40x
at 5.3 us per task. Tiny-task scaling is non-monotonic at 8 workers, but the
effect is no longer visible at the tested 5.3 and 10.7 us workloads. This pass
does not isolate the mechanism and does not provide evidence for a scheduler
change.

This threshold is specific to 64 ready, independent, non-yielding tasks and to
this machine. It is not a portable rule for dependent tasks, I/O work, smaller
batches, or other CPUs.

## Formal crossover results

The target labels are approximate. `Actual/task` is the five-round median
inline batch mean divided by 64. Batch values and speedups are five-round
medians.

| Target | Actual/task | Workers | Inline batch | Runtime batch | Median speedup | Minimum conservative bound | Stable |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | :---: |
| 0 us | ~0 us | 2 | 0.09 us | 21.42 us | 0.004x | 0.004x | no |
| 0 us | ~0 us | 4 | 0.09 us | 41.81 us | 0.002x | 0.002x | no |
| 0 us | ~0 us | 8 | 0.09 us | 163.70 us | 0.001x | 0.001x | no |
| 2 us | 2.13 us | 2 | 136.59 us | 79.84 us | 1.710x | 1.680x | yes (5/5) |
| 2 us | 2.13 us | 4 | 136.59 us | 49.38 us | 2.777x | 2.710x | yes (5/5) |
| 2 us | 2.13 us | 8 | 136.59 us | 118.69 us | 1.144x | 1.060x | yes (5/5) |
| 5 us | 5.33 us | 2 | 341.14 us | 211.62 us | 1.616x | 1.590x | yes (5/5) |
| 5 us | 5.33 us | 4 | 341.14 us | 116.67 us | 2.925x | 2.890x | yes (5/5) |
| 5 us | 5.33 us | 8 | 341.14 us | 77.74 us | 4.398x | 4.330x | yes (5/5) |
| 10 us | 10.70 us | 2 | 684.64 us | 405.09 us | 1.690x | 1.660x | yes (5/5) |
| 10 us | 10.70 us | 4 | 684.64 us | 214.58 us | 3.190x | 3.130x | yes (5/5) |
| 10 us | 10.70 us | 8 | 684.64 us | 127.79 us | 5.358x | 5.270x | yes (5/5) |

The conservative value is calculated independently in each round as:

```text
inline mean 99% CI lower / runtime mean 99% CI upper
```

The table shows the minimum of those five values. It is deliberately described
as an engineering bound, not as a confidence interval for the ratio. Raw
samples are not pooled across rounds.

All three worker counts therefore cross in the tested interval `(0 us,
2.13 us]`. No interpolation is attempted. For 8 workers, where the boundary
gain is smallest, 5.33 us/task provides the required next stable workload.

## Supporting diagnostics

The Rapid zero-work diagnostic measured the complete 64-task spawn, schedule,
and completion path at 12.869 us for one worker (99% Criterion mean interval
12.540–13.206 us). Runtime creation, startup, warm-up, and shutdown are not in
the new measurement.

As historical API-path context, the v0.3 release baseline measured one trivial
`Priority::Normal` task at 1.0863 us spawn-to-complete on one worker under the
Balanced power scheme. That path includes task construction, submission,
scheduling, completion, and caller wait; it is not a pure ready-queue or
scheduler cost and is not directly comparable with this campaign's 64-task
High-performance measurement.

The new single-task external-wake diagnostic starts only after the Pending task
has registered its waker. Its wake-to-complete mean was 11.594 us, with a 99%
Criterion mean interval of 11.191–12.017 us. This includes the external wake,
scheduler handoff, second poll, and task completion; it is not a pure
`Waker::wake` measurement.

Parked-worker wake was not rerun because no scheduler code changed. The existing
v0.3 release baseline reports full parked cycles of 1.0069/13.459/15.332/15.711
us for 1/2/4/8 workers respectively. Those measurements include submission,
completion, and readiness for the next parked cycle and are not directly
subtracted from the continuously active crossover runs.

Timing alone does not prove a particular task was stolen. Existing `ThreadId`
and `stolen > 0` diagnostics can prove that cross-worker execution occurred,
but cannot isolate a per-hop latency. The non-monotonic 8-worker tiny-task shape
could reflect worker activation, queueing, stealing, contention, or OS effects;
this pass does not attribute it. The effect is no longer observed at the larger
tested workloads and gives no product evidence for a verified-steal diagnostic
or a scheduler candidate in this pass.

## Method and evidence

- Kernel calibration probe: 1,000,000 rounds had a 1,954,200 ns median; frozen
  rounds are 0/1,023/2,559/5,117/10,234/25,586/51,172/102,344 for the Rapid
  0/2/5/10/20/50/100/200 us labels.
- Rapid screening used one process, 0.5 s warm-up, 2 s measurement, 20 flat
  samples, all eight workloads, and inline/1/2/4/8 workers to select 0/2/5/10 us
  for Formal. Only the zero-work Rapid evidence was retained in version control.
- Formal: five processes, 3 s warm-up, 5 s measurement, 100 flat samples, 99%
  confidence, 1% significance, inline plus 2/4/8 workers.
- The five fixed case permutations were recorded and reduced fixed-position
  bias, but were not perfectly position-balanced. Mean positions are inline=2.2,
  workers-2=2.8, workers-4=2.4, workers-8=2.6. Workload order remained fixed at
  0/2/5/10 us in every process. The clean-round sensitivity check did not change
  the decision.
- Formal run ID: `20260907-formal-02`.
- Source revision before this campaign: `81a2c7a2a646ec3b5a34441893f5b7c17ef5d16e`.
- Measurement snapshot was dirty because benchmark code and data are delivered
  together. Each manifest records SHA-256 for `Cargo.toml`, the crossover
  benchmark, wake benchmark, and runner; these hashes were checked against the
  final files before commit.
- Toolchain: `rustc 1.97.0-nightly (507271bc1 2026-05-17)`, target
  `x86_64-pc-windows-msvc`.
- Machine: AMD Ryzen 7 8845H, 8 physical/16 logical cores, Windows 11 build
  26200, High performance power scheme.

Versioned evidence is under
`benchmarks/results/cpu-crossover/20260907-formal-02/`: only Criterion
`sample.json`, `estimates.json`, `benchmark.json`, round logs, manifests, and
derived CSV are retained. HTML, plots, and Criterion cache are excluded. The
one-worker zero-work Rapid evidence is in the sibling `20260907-rapid-zero-02`
directory.

Reproduce the Formal neighborhood with:

```powershell
.\benchmarks\run-cpu-crossover.ps1 `
  -Mode Formal `
  -RunId <unique-run-id> `
  -Workloads 0us,2us,5us,10us `
  -OutputRoot .\benchmarks\results\cpu-crossover
```

The existing guard values cited above come from
`benchmarks/baseline-v0.3-release.md`; they were not rerun and do not vote on
this no-scheduler-change decision.

## Validation

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --all-targets --all-features -- -D warnings`: passed. Rustc
  emitted the repository's existing Windows linker-message warning for the loom
  test binary; Clippy emitted no lint failure.
- `cargo test --all-features -- --test-threads=1`: passed; the manual ten-second
  stress test remains ignored as designed.
- `cargo test --doc --all-features`: passed (no doctests).
- Locked release no-run builds for `cpu_crossover` and
  `v030_yield_wake_storm`: passed.
- The default parallel all-features suite was attempted twice and stalled once
  in a shutdown cancellation test and once in a task cancellation test. Both
  stalled cases passed immediately in isolation, and the complete serial suite
  passed. No library/runtime source changed in this campaign; this is recorded
  as an existing parallel-suite flake rather than silently reported as a clean
  default-parallel pass.
