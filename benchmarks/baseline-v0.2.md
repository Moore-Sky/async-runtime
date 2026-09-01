# async-runtime v0.2 benchmark baseline

Recorded on 2026-09-01 with:

```text
OS: Windows (x86_64-pc-windows-msvc)
Rust: rustc 1.97.0-nightly (507271bc1 2026-05-17)
Criterion: 0.5.1, 100 samples per benchmark
Command: cargo bench --bench <target> -- --noplot
```

These numbers are a local engineering baseline, not a cross-machine performance
claim. Compare changes on the same machine, toolchain, power profile, and
background-load conditions. Tables record Criterion's central estimate.

## `general_spawn`

Submission, scheduling, and completion are included.

| Workload | 1 | 64 | 1024 |
| --- | ---: | ---: | ---: |
| Normal spawn + complete | 1.2127 us | 20.893 us | 325.25 us |

| Nested children | 1 | 10 | 100 |
| --- | ---: | ---: | ---: |
| Parent spawn + child completion | 2.3215 us | 5.1400 us | 35.118 us |

## `priority`

This measures throughput, not latency percentiles or a fairness SLA.

| Priority | 1 task | 64 tasks | 1024 tasks |
| --- | ---: | ---: | ---: |
| High | 1.1870 us | 22.553 us | 266.63 us |
| Normal | 6.9370 us | 28.360 us | 361.85 us |
| Background | 6.7096 us | 29.775 us | 374.82 us |

The mixed workload repeats an 8 High / 4 Normal / 1 Background batch:

| Repetitions | Total tasks | Time | Throughput |
| ---: | ---: | ---: | ---: |
| 1 | 13 | 6.8287 us | 1.9037 Melem/s |
| 64 | 832 | 272.60 us | 3.0521 Melem/s |
| 1024 | 13312 | 4.7785 ms | 2.7858 Melem/s |

## `local_driving`

Domain construction and task submission are excluded from the timed interval.

| Ready tasks | `run_n` time | Throughput |
| ---: | ---: | ---: |
| 1 | 98.183 ns | 10.185 Melem/s |
| 64 | 3.7880 us | 16.895 Melem/s |
| 1024 | 62.867 us | 16.288 Melem/s |

Each `run_for` sample starts with 64 ready tasks:

| Soft budget | Time | Throughput |
| ---: | ---: | ---: |
| 100 us | 6.4384 us | 9.9403 Melem/s |
| 500 us | 6.3092 us | 10.144 Melem/s |
| 1 ms | 6.2590 us | 10.225 Melem/s |

## `local_dispatch`

A persistent background producer submits through `LocalSpawner`; the owner
thread then drains the domain. Request/acknowledgement coordination is identical
for all three paths and is included in the timing.

| Batch | Result-bearing `spawn` bridge | `dispatch` closure | `dispatch_future` |
| ---: | ---: | ---: | ---: |
| 1 | 6.1094 us | 1.5638 us | 2.1975 us |
| 64 | 129.31 us | 33.442 us | 33.331 us |
| 1024 | 1.9377 ms | 294.52 us | 293.62 us |

For batch 1024, both fire-and-forget paths are about 6.6x faster than the
result-bearing bridge in this run. For one item, fixed cross-thread coordination
dominates and the measured improvement is about 2.8x–3.9x.

## `frame_like`

Each sample preloads 64 remote-capable inbox commands. Submission and cleanup
are excluded; only budgeted owner driving is timed.

| Soft budget | Time | Throughput |
| ---: | ---: | ---: |
| 100 us | 12.971 us | 4.9339 Melem/s |
| 500 us | 13.524 us | 4.7322 Melem/s |
| 1 ms | 14.002 us | 4.5706 Melem/s |

All ready work fits within each tested budget. A single slow poll can still
overshoot because futures are not preemptible; that property is covered by a
functional test rather than this microbenchmark.

## `shutdown`

Setup is excluded; the timer covers draining or cancelling ready local work.

| Accepted tasks | Graceful drain | `shutdown_now` cancellation |
| ---: | ---: | ---: |
| 1 | 460.99 ns | 591.86 ns |
| 64 | 4.4030 us | 3.2493 us |
| 1024 | 63.264 us | 47.687 us |

## `yield_storm`

Each task executes eight `yield_now` calls; setup is excluded. Throughput counts
the resulting eight scheduling opportunities per task.

| Tasks | Time | Throughput |
| ---: | ---: | ---: |
| 200 | 95.147 us | 16.816 Melem/s |
| 1000 | 474.29 us | 16.867 Melem/s |
| 10000 | 5.4484 ms | 14.683 Melem/s |

## Interpretation rules

- Re-run the affected target and then the full suite before accepting a hot-path
  optimization.
- Treat comparisons against stale Criterion history as noise until reproduced.
- Use `local_dispatch` to guard the result-free fast path.
- Use `local_driving`, `frame_like`, and `yield_storm` when changing fairness or
  host-loop driving.
- No wake-storm benchmark exists in v0.2 because the public API has no reliable,
  controllable external-wake construction hook; adding a synthetic one only for
  a benchmark would distort the API.
