# async-runtime v0.3 benchmark baseline

Recorded on 2026-09-01 with:

```text
CPU: AMD Ryzen 7 8845H (8 cores / 16 logical processors)
OS: Windows (x86_64-pc-windows-msvc)
Rust: rustc 1.97.0-nightly (507271bc1 2026-05-17)
Criterion: 0.5.1, quick mode
Command: cargo bench --bench <target> -- --quick --noplot
Idle wake: cargo bench --bench v030_idle_wake --features stats -- --quick --noplot
```

These are local engineering numbers, not cross-machine claims. Quick mode was
used to exercise every new scenario and obtain an initial baseline; use the
default Criterion sample count before making release or regression decisions.
Tables show Criterion's central estimate.

## General scheduler regression workloads

`general_spawn` includes submission, scheduling, and completion.

| Workload | 1 | 64 | 1024 |
| --- | ---: | ---: | ---: |
| Normal spawn + complete | 1.8324 us | 20.078 us | 270.08 us |

| Nested children | 1 | 10 | 100 |
| --- | ---: | ---: | ---: |
| Parent spawn + child completion | 2.0765 us | 4.2709 us | 26.226 us |

Compared with the recorded v0.2 central estimates, the one-task external path
is slower, while batches of 64/1024 and nested batches are generally similar or
faster on this run. Treat this as directional because v0.2 used 100 samples and
this baseline used quick mode.

| Priority | 1 task | 64 tasks | 1024 tasks |
| --- | ---: | ---: | ---: |
| High | 1.7756 us | 19.886 us | 286.28 us |
| Normal | 2.6661 us | 25.543 us | 314.70 us |
| Background | 2.3358 us | 26.135 us | 337.08 us |

| Mixed repetitions | Total tasks | Time | Throughput |
| ---: | ---: | ---: | ---: |
| 1 | 13 | 5.3145 us | 2.4462 Melem/s |
| 64 | 832 | 265.04 us | 3.1392 Melem/s |
| 1024 | 13312 | 5.4508 ms | 2.4422 Melem/s |

## Nested locality

End-to-end parent spawn, nested child spawn, and child completion. Timing alone
does not prove routing; `tests/v030_stats.rs` verifies `local_spawned` and victim
steals through scheduler counters.

| Workers | 1 child | 10 children | 100 children | 1000 children |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 2.1601 us | 3.7592 us | 26.803 us | 193.18 us |
| 2 | 2.9345 us | 14.080 us | 40.468 us | 307.37 us |
| 4 | 18.579 us | 39.746 us | 114.95 us | 817.96 us |
| 8 | 29.576 us | 59.967 us | 370.95 us | 3.4042 ms |

For these trivial children, coordination dominates as worker count rises. This
does not predict scaling for CPU-heavy tasks.

## External producers

Each producer submits 256 Normal tasks to a four-worker runtime.

| Producers | Total tasks | Time | Throughput |
| ---: | ---: | ---: | ---: |
| 1 | 256 | 202.21 us | 1.2660 Melem/s |
| 2 | 512 | 492.18 us | 1.0403 Melem/s |
| 4 | 1024 | 2.5412 ms | 402.95 Kelem/s |
| 8 | 2048 | 4.3348 ms | 472.46 Kelem/s |
| 16 | 4096 | 5.8001 ms | 706.20 Kelem/s |

## Imbalanced nested queue / stealing workload

Each child yields eight times. The public benchmark measures the whole workload;
the stats test separately proves that victim stealing occurs.

| Workers | 256 tasks | 1024 tasks | 4096 tasks |
| ---: | ---: | ---: | ---: |
| 1 | 206.46 us | 674.45 us | 2.7653 ms |
| 2 | 194.62 us | 679.27 us | 2.7280 ms |
| 4 | 200.21 us | 687.57 us | 2.6386 ms |
| 8 | 349.66 us | 1.0964 ms | 4.2670 ms |

## Priority probe latency under continuous High work

A fixed population of High tasks continually yields. Every sample submits and
waits for one probe; the population is stopped and drained between cases.

| Probe | 128 High tasks | 1024 High tasks | 8192 High tasks |
| --- | ---: | ---: | ---: |
| High | 4.2725 us | 4.2201 us | 5.8606 us |
| Normal | 1.0290 us | 1.0097 us | 1.0930 us |
| Background | 1.0331 us | 1.0082 us | 1.0545 us |

The higher High-probe result reflects the bounded local-burst policy: a global
same-priority submission is periodically admitted rather than allowed to starve
behind continuously requeued local High work. Priority weights are opportunities,
not a completion-ratio or latency SLA.

## Background progress / starvation guard

| Continuous High population | Background probe |
| ---: | ---: |
| 256 | 989.39 ns |
| 2048 | 954.56 ns |
| 16384 | 1.0345 us |

This is a bounded observed workload, not a proof about realtime deadlines or an
infinite adversarial stream. Functional tests enforce progress with deadlines.

## General yield and external wake storms

The yield workload performs eight yields per task. External wake timing starts
after every custom future has acknowledged waker registration.

| Tasks | General yield storm | Throughput | External wake storm | Throughput |
| ---: | ---: | ---: | ---: | ---: |
| 200 | 141.13 us | 11.337 Melem/s | 51.679 us | 3.8700 Melem/s |
| 1000 | 590.00 us | 13.559 Melem/s | 191.31 us | 5.2272 Melem/s |
| 10000 | 6.1696 ms | 12.967 Melem/s | 1.9513 ms | 5.1248 Melem/s |

## Idle park/wake cycle

The benchmark waits until all workers are reported sleeping, then measures a
High submission, completion, and readiness for the next parked cycle.

| Workers | Cycle time |
| ---: | ---: |
| 1 | 1.7490 us |
| 2 | 10.373 us |
| 4 | 10.709 us |
| 8 | 9.5025 us |

Criterion cannot establish idle CPU percentage; use an OS profiler for power or
CPU claims. The functional suite instead verifies park/wake counters and repeats
bursts to exercise the missed-wake boundary.

## Interpretation rules

- Compare only the same target, parameters, feature set, toolchain, and machine.
- Run default Criterion sampling before accepting a performance-sensitive change.
- Use stats-backed tests to prove locality, stealing, and parking mechanisms;
  timing alone is insufficient.
- Re-run `general_spawn`, `priority`, `v030_priority_latency`, and
  `v030_yield_wake_storm` after scheduler hot-path changes.
- Keep the v0.2 LocalDomain benchmarks as regression coverage; v0.3 does not
  replace the host-driven local executor.
