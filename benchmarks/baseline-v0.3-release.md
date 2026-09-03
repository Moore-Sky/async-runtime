# async-runtime v0.3 release baseline

This is the repeatable, five-round release baseline for the v0.3 scheduler.
It is a same-machine regression reference, not a cross-machine performance
claim and not a latency SLA.

## Capture environment

```text
Captured: 2026-09-02 20:21:26–21:24:25 +09:00
CPU: AMD Ryzen 7 8845H (8 physical cores / 16 logical processors)
OS: Windows 11 10.0.26200 (x86_64-pc-windows-msvc)
Power scheme: Balanced (381b4222-f694-41f0-9685-ff5bb260df2e)
Rust: rustc 1.97.0-nightly (507271bc1 2026-05-17)
Cargo: cargo 1.97.0-nightly (4d1f98451 2026-05-15)
Capture commit: 5fbd936a28b26def6f6129bd92acaab5a8c74822
Dependencies: Cargo.lock, --locked
Rounds: 5
Latency samples per case: 10000
```

The capture working tree contained the v0.3 release-candidate benchmark,
example, CI, manifest, and documentation additions recorded by the final v0.3
commit. Runtime sources under `src/` were clean relative to the capture commit.
This note is important because the capture commit alone does not contain the
new benchmark targets.

The initial five one-second total-CPU samples were 10.13%, 15.19%, 20.31%,
14.00%, and 19.63%. No other build or benchmark was run during capture. The
first round had transient elevation in several very short Criterion cases;
the following four rounds converged, while the CPU-kernel throughput was stable
across all five rounds. The specified five-round median makes the isolated
first-round spike observation-only rather than the recorded baseline.

## Reproduction and aggregation

```powershell
./benchmarks/run-v0.3-release-baseline.ps1 -Rounds 5 -LatencySamples 10000
```

The run produced all 50 expected non-empty logs with no panic, benchmark
failure, or error marker. Raw logs and capture metadata are under
`target/v030-release-baseline/` and are build artifacts, not version-controlled
release files.

- Criterion tables report the median of the five rounds' central estimates.
  They are not task-latency percentiles.
- Sampler p50/p95/p99 columns report the median of the corresponding percentile
  from each of the five rounds.
- CPU throughput first takes the median of five batches inside each round, then
  the median across rounds. Speedup is relative to the recorded one-worker
  throughput; efficiency is speedup divided by worker count.
- Maximums and Criterion outliers are observation-only and are excluded from
  regression gates.

## General spawn and priority

All values in these Criterion tables are central elapsed-time estimates.

| General spawn + complete | 1 | 64 | 1024 |
| --- | ---: | ---: | ---: |
| Normal tasks | 1.0863 us | 16.814 us | 205.62 us |

| Nested children | 1 | 10 | 100 |
| --- | ---: | ---: | ---: |
| Parent spawn + child completion | 1.6255 us | 3.8665 us | 33.767 us |

| Priority | 1 task | 64 tasks | 1024 tasks |
| --- | ---: | ---: | ---: |
| High | 1.0629 us | 17.188 us | 199.27 us |
| Normal | 1.0865 us | 16.028 us | 198.50 us |
| Background | 1.1225 us | 15.835 us | 195.90 us |

The mixed workload submits 8 High, 4 Normal, and 1 Background task per
repetition.

| Repetitions | Total tasks | Central time |
| ---: | ---: | ---: |
| 1 | 13 | 2.9682 us |
| 64 | 832 | 164.31 us |
| 1024 | 13312 | 2.6103 ms |

## Nested locality

This measures end-to-end parent spawn, nested child spawn, and child
completion. Coordination dominates these trivial children as worker count
rises; use the CPU workload below to evaluate compute scaling.

| Workers | 1 child | 10 children | 100 children | 1000 children |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 1.5588 us | 3.8591 us | 32.231 us | 188.08 us |
| 2 | 6.8688 us | 19.035 us | 46.959 us | 237.30 us |
| 4 | 24.761 us | 32.236 us | 80.275 us | 537.42 us |
| 8 | 25.573 us | 47.284 us | 251.86 us | 2.1768 ms |

## External producers and concentrated completion

External-producer cases use a four-worker runtime and submit 256 tasks per
producer.

| Producers | Total tasks | Central time |
| ---: | ---: | ---: |
| 1 | 256 | 217.12 us |
| 2 | 512 | 284.09 us |
| 4 | 1024 | 416.15 us |
| 8 | 2048 | 941.99 us |
| 16 | 4096 | 1.8610 ms |

Concentrated-completion cases pre-register 1024 tasks, release them together,
and include submission, registration, release, and drain in the measured
iteration.

| Workers | Central time |
| ---: | ---: |
| 1 | 322.69 us |
| 2 | 475.72 us |
| 4 | 724.31 us |
| 8 | 4.9097 ms |

## Yield, external wake, and idle wake

The general-yield workload performs eight cooperative yields per task. The
external-wake timer starts after every custom future acknowledges waker
registration.

| Tasks | General yield storm | External wake storm |
| ---: | ---: | ---: |
| 200 | 138.72 us | 54.716 us |
| 1000 | 642.40 us | 174.39 us |
| 10000 | 6.2783 ms | 1.4558 ms |

Idle wake measures a complete parked-worker submission, completion, and
readiness for the next parked cycle with the `stats` feature enabled.

| Workers | Cycle time |
| ---: | ---: |
| 1 | 1.0069 us |
| 2 | 13.459 us |
| 4 | 15.332 us |
| 8 | 15.711 us |

These values do not establish idle CPU consumption; use an OS profiler for
power and CPU claims.

## Priority-probe latency sampler

One worker runs a fixed population of yielding High tasks. Each cell is the
five-round median of that case's measured task-latency percentile.

| Probe | High population | p50 | p95 | p99 |
| --- | ---: | ---: | ---: | ---: |
| High | 128 | 0.9 us | 1.5 us | 1.7 us |
| High | 1024 | 0.9 us | 1.5 us | 1.7 us |
| High | 8192 | 0.9 us | 1.5 us | 1.9 us |
| Normal | 128 | 0.9 us | 1.5 us | 1.7 us |
| Normal | 1024 | 0.9 us | 1.5 us | 1.7 us |
| Normal | 8192 | 1.0 us | 1.5 us | 1.9 us |
| Background | 128 | 0.9 us | 1.4 us | 1.7 us |
| Background | 1024 | 0.9 us | 1.3 us | 1.5 us |
| Background | 8192 | 0.9 us | 1.5 us | 1.7 us |

Priority weights are scheduling opportunities, not a completion-ratio or
realtime-latency guarantee.

## CPU workload

The fixed 100000-iteration integer kernel had a five-round median observed
duration of 265.3 us. Every task executes it in one deliberately non-yielding
poll, so a running poll is not preemptible.

| Workers | Tasks/s | Speedup | Scaling efficiency |
| ---: | ---: | ---: | ---: |
| 1 | 3692.644 | 1.000x | 100.0% |
| 2 | 7376.889 | 1.998x | 99.9% |
| 4 | 14200.665 | 3.846x | 96.1% |
| 8 | 27888.609 | 7.552x | 94.4% |

The one-worker 8:4:1 mixed workload records submit-to-complete latency. Because
tasks are submitted and awaited in batches and a poll cannot be preempted,
these numbers describe this workload rather than a general priority SLA.

| Priority | Samples/round | p50 | p95 | p99 |
| --- | ---: | ---: | ---: | ---: |
| High | 80000 | 1.8774 ms | 3.4738 ms | 3.8515 ms |
| Normal | 40000 | 1.8803 ms | 3.4813 ms | 3.8586 ms |
| Background | 10000 | 1.8699 ms | 3.4944 ms | 3.8716 ms |

## Local budget latency

Observed median work targets were 20.037 us, 100.331 us, and 503.162 us. The
historical v0.3 tables report `RunStats::elapsed` and its `elapsed - budget`
overshoot in microseconds; caller-side wall-clock elapsed was not retained in
this capture. Ready-queue work is a future poll, while remote-inbox work is a
dispatch command. A call cannot interrupt the work item executing when its
budget expires.

| Queue | Budget | Work target | Stats elapsed p50 | p95 | p99 | Stats overshoot p50 | p95 | p99 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Ready | 100 us | 20 us | 118.8 | 119.8 | 136.6 | 18.8 | 19.8 | 36.6 |
| Ready | 500 us | 20 us | 513.0 | 519.6 | 541.9 | 13.0 | 19.6 | 41.9 |
| Ready | 1000 us | 20 us | 1010.5 | 1019.7 | 1043.2 | 10.5 | 19.7 | 43.2 |
| Ready | 100 us | 100 us | 198.1 | 201.9 | 220.7 | 98.1 | 101.9 | 120.7 |
| Ready | 500 us | 100 us | 579.5 | 600.4 | 637.9 | 79.5 | 100.4 | 137.9 |
| Ready | 1000 us | 100 us | 1070.3 | 1099.1 | 1144.6 | 70.3 | 99.1 | 144.6 |
| Ready | 100 us | 500 us | 494.5 | 574.4 | 749.2 | 394.5 | 474.4 | 649.2 |
| Ready | 500 us | 500 us | 977.2 | 1013.3 | 1148.1 | 477.2 | 513.3 | 648.1 |
| Ready | 1000 us | 500 us | 1465.9 | 1509.2 | 1646.2 | 465.9 | 509.2 | 646.2 |
| Remote | 100 us | 20 us | 101.2 | 114.3 | 131.8 | 1.2 | 14.3 | 31.8 |
| Remote | 500 us | 20 us | 508.5 | 519.7 | 539.6 | 8.5 | 19.7 | 39.6 |
| Remote | 1000 us | 20 us | 1010.3 | 1019.8 | 1039.9 | 10.3 | 19.8 | 39.9 |
| Remote | 100 us | 100 us | 110.1 | 201.0 | 219.5 | 10.1 | 101.0 | 119.5 |
| Remote | 500 us | 100 us | 527.2 | 600.6 | 634.8 | 27.2 | 100.6 | 134.8 |
| Remote | 1000 us | 100 us | 1039.4 | 1099.3 | 1142.5 | 39.4 | 99.3 | 142.5 |
| Remote | 100 us | 500 us | 495.0 | 591.9 | 765.3 | 395.0 | 491.9 | 665.3 |
| Remote | 500 us | 500 us | 978.2 | 1013.5 | 1120.0 | 478.2 | 513.5 | 620.0 |
| Remote | 1000 us | 500 us | 1466.9 | 1508.8 | 1649.1 | 466.9 | 508.8 | 649.1 |

## Comparison rules

- Compare only the same machine, toolchain, locked dependencies, feature set,
  power scheme, workload parameters, and release profile.
- Re-run all five rounds after a scheduler hot-path change; do not compare a
  single run against these medians.
- Use stats-backed tests to prove locality, stealing, and parking mechanisms;
  timing alone is insufficient.
- Revisit the frozen scheduler only when an application trace, a reproducible
  workload, and a regression against this baseline identify the same issue.

The earlier [v0.3 engineering baseline](baseline-v0.3.md) used Criterion quick
mode and remains a pre-release diagnostic record. The
[v0.3 scheduler review](review-v0.3.md) used shortened five-round measurements
to select the local-burst policy. Neither document replaces this release
baseline.
