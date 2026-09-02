# async-runtime v0.3 performance review

## Decision

The `High=8 / Normal=16 / Background=64` local-burst policy is accepted by
the shortened five-run decision matrix. High probe p95/p99 meets the 25% rule
at every tested High load, and no required throughput benchmark ID regressed
by more than 10%.

This is not the full default-Criterion release baseline: at the user's
direction, Criterion was shortened to a one-second warm-up, one-second
measurement window, and 30 samples per ID. Five independent runs and the full
10,000-probe latency samplers were retained. A future release record may repeat
the same matrix with Criterion defaults.

## Run metadata

| Field | Before | After |
| --- | --- | --- |
| Date / run identifier | 2026-09-01 19:39–19:53 JST; `before-fast-r01..r05` | 2026-09-01 19:53–20:07 JST; `after-fast-r01..r05` |
| CPU and power mode | AMD Ryzen 7 8845H, 8C/16T; Windows Balanced | Same machine and power mode |
| OS and target | Windows 11 10.0.26200; `x86_64-pc-windows-msvc` | Same |
| Rust toolchain | `rustc 1.97.0-nightly (507271bc1 2026-05-17)` | Same |
| Git revision | `ea42e6e132dcf343ff4082acd94e6d94f02ee602` plus measurement-only bench harness | `ea42e6e` plus the uncommitted v0.3 review changes |
| Environment | `RUSTFLAGS` and `CARGO_INCREMENTAL` unset | Same |

Criterion command, repeated five times per state and target:

```text
cargo bench --locked --bench <target> -- \
  --save-baseline <run-id> \
  --warm-up-time 1 --measurement-time 1 --sample-size 30 --noplot
```

Targets were `general_spawn`, `yield_storm`, `v030_nested_locality`,
`priority`, `v030_external_producers`, and
`v030_completion_contention`. Each result below is the median of the five
Criterion mean point estimates. The completion benchmark keeps its explicit
eight-second group measurement time. Its four rows were repeated after the
Rust 1.71-compatible registration fix on 2026-09-02 as
`before-final4-r01..r05` and `after-final4-r01..r05`.

Sampler commands, also repeated five times per state:

```text
ASYNC_RUNTIME_LATENCY_SAMPLES=10000 cargo bench --locked --bench v030_latency_sampling
cargo bench --locked --bench v030_spawn_latency_sampling
```

## Criterion results

Positive time delta is a regression; negative is an improvement.

| Workload / decisive parameter | Before | After | Time delta | Decision / notes |
| --- | ---: | ---: | ---: | --- |
| general spawn, 1 task | 1.710 µs | 1.809 µs | +5.84% | 5–10% observation; 64 improved 1.27%, 1024 improved 4.99% |
| yield storm, 200 tasks | 85.313 µs | 89.881 µs | +5.35% | 5–10% observation in unchanged LocalDomain; 1000 +3.23%, 10000 +4.12% |
| nested locality, worst ID (workers 8, tasks 100) | 346.355 µs | 349.794 µs | +0.99% | Pass; other 15 IDs improved 0.62–5.34% |
| mixed priority, worst ID (batch 64) | 255.940 µs | 268.406 µs | +4.87% | Pass; batch 1 improved 5.22%, batch 1024 improved 3.61% |
| external producers, 1 producer | 226.065 µs | 239.716 µs | +6.04% | 5–10% observation |
| external producers, 2 producers | 1.296 ms | 394.485 µs | -69.56% | Improved |
| external producers, 4 producers | 3.800 ms | 572.865 µs | -84.92% | Improved |
| external producers, 8 producers | 6.539 ms | 1.236 ms | -81.10% | Improved |
| external producers, 16 producers | 9.357 ms | 2.389 ms | -74.46% | Improved |
| concentrated completion, workers 1 | 857.317 µs | 693.772 µs | -19.08% | Improved |
| concentrated completion, workers 2 | 1.326 ms | 878.674 µs | -33.72% | Improved |
| concentrated completion, workers 4 | 2.105 ms | 1.430 ms | -32.03% | Improved |
| concentrated completion, workers 8 | 11.101 ms | 8.223 ms | -25.92% | Improved |

The three 5–10% observations occur only at the smallest general/external
submission cases and in the unchanged LocalDomain control. The unchanged
`yield_storm` shift supplies evidence of short-window OS scheduling noise at
roughly the same scale; the shortened matrix cannot prove that explanation, so
the observations remain explicitly recorded rather than being treated as
improvements or silently discarded. No required group has an ID above the 10%
rejection threshold.

The completion target measures an end-to-end batch: task creation,
registration, concentrated release, and joins are all inside each iteration.
Its change therefore must not be attributed solely to the atomic completion
path. Registration uses a mutex-wrapped standard channel so the harness remains
compatible with Rust 1.71; before and after use the identical harness.

## Priority probe latency

Values are five-run medians of each run's nearest-rank percentile, in
nanoseconds.

| Probe | High load | Before p50/p95/p99 | After p50/p95/p99 | After High check |
| --- | ---: | ---: | ---: | --- |
| High | 128 | 3500 / 11300 / 21200 | 900 / 1600 / 2100 | p95 1.00×, p99 1.00× slower-priority maximum |
| Normal | 128 | 900 / 1500 / 1900 | 900 / 1600 / 2100 | — |
| Background | 128 | 1000 / 1500 / 1800 | 900 / 1500 / 1800 | — |
| High | 1024 | 3600 / 11500 / 19800 | 900 / 1500 / 1800 | p95 1.00×, p99 0.86× |
| Normal | 1024 | 900 / 1500 / 2000 | 900 / 1500 / 2100 | — |
| Background | 1024 | 900 / 1600 / 2000 | 900 / 1500 / 2000 | — |
| High | 8192 | 3800 / 12200 / 20300 | 900 / 1600 / 2100 | p95 0.94×, p99 0.95× |
| Normal | 8192 | 1000 / 1700 / 2100 | 900 / 1600 / 2000 | — |
| Background | 8192 | 1000 / 1700 / 2100 | 1000 / 1700 / 2200 | — |

All after High p95/p99 ratios are at most 1.00, comfortably inside the 1.25
limit. Relative to before, High p95 fell by roughly 86–87% across the three
loads.

## Concurrent public spawn-call return latency

Five-run median percentiles, in nanoseconds:

| Producers | Before p50/p95/p99 | After p50/p95/p99 |
| ---: | ---: | ---: |
| 1 | 600 / 1500 / 4000 | 600 / 2600 / 4400 |
| 2 | 1000 / 2100 / 6100 | 800 / 1300 / 1900 |
| 4 | 700 / 3200 / 22000 | 900 / 1800 / 2500 |
| 8 | 1300 / 24700 / 91600 | 1000 / 2700 / 4600 |
| 16 | 1700 / 60800 / 152300 | 1100 / 3100 / 5500 |

Contention-tail latency improves sharply from two producers onward. The
one-producer p95/p99 observation is consistent with the small-case Criterion
noise recorded above and does not override the multi-producer result.

## Raw sampler output

The blocks below preserve every latency and spawn-return sampler run used for
the medians.

### Before

```text
## before-fast-r01-latency
probe,high_load,p50_ns,p95_ns,p99_ns
High,128,3500,11400,22500
High,1024,3600,13400,25500
High,8192,3900,12200,20300
Normal,128,900,1400,1600
Normal,1024,900,1500,1700
Normal,8192,1000,1800,2200
Background,128,1000,1500,1800
Background,1024,1000,1700,2100
Background,8192,1000,1700,2100
## before-fast-r02-latency
probe,high_load,p50_ns,p95_ns,p99_ns
High,128,3400,13800,21000
High,1024,3600,10800,19200
High,8192,3800,11400,19300
Normal,128,900,1700,2000
Normal,1024,1000,1700,2100
Normal,8192,1000,1600,1900
Background,128,900,1500,1700
Background,1024,900,1600,1900
Background,8192,1100,1900,2300
## before-fast-r03-latency
probe,high_load,p50_ns,p95_ns,p99_ns
High,128,3500,5400,17300
High,1024,3600,11500,21800
High,8192,3800,12200,22300
Normal,128,1000,1500,1800
Normal,1024,900,1500,2900
Normal,8192,1000,1900,2300
Background,128,900,1500,1700
Background,1024,900,1500,1900
Background,8192,1000,1500,1700
## before-fast-r04-latency
probe,high_load,p50_ns,p95_ns,p99_ns
High,128,3500,11300,22300
High,1024,3600,10400,19800
High,8192,3800,11100,20300
Normal,128,900,1400,2200
Normal,1024,900,1400,2000
Normal,8192,900,1400,1900
Background,128,1000,2000,2200
Background,1024,900,1400,2100
Background,8192,1000,1400,2200
## before-fast-r05-latency
probe,high_load,p50_ns,p95_ns,p99_ns
High,128,3500,11200,21200
High,1024,3600,12200,19800
High,8192,3900,14400,25200
Normal,128,1100,1600,1900
Normal,1024,900,1500,1900
Normal,8192,1000,1700,2100
Background,128,1000,1800,2100
Background,1024,1000,1700,2000
Background,8192,1100,1700,2100
## before-fast-r01-spawn-latency
concurrent external spawn-return latency, calls_per_producer=2000
producers,samples,p50_ns,p95_ns,p99_ns
1,2000,600,1200,4000
2,4000,900,2400,13700
4,8000,1500,26100,57900
8,16000,1500,32900,91600
16,32000,2300,60800,146300
## before-fast-r02-spawn-latency
concurrent external spawn-return latency, calls_per_producer=2000
producers,samples,p50_ns,p95_ns,p99_ns
1,2000,600,1500,4400
2,4000,1100,2100,6100
4,8000,1000,3000,22000
8,16000,1000,24700,117900
16,32000,1700,67900,167000
## before-fast-r03-spawn-latency
concurrent external spawn-return latency, calls_per_producer=2000
producers,samples,p50_ns,p95_ns,p99_ns
1,2000,600,3100,3500
2,4000,1000,1600,2300
4,8000,700,1600,3400
8,16000,1200,5300,49800
16,32000,700,4700,87500
## before-fast-r04-spawn-latency
concurrent external spawn-return latency, calls_per_producer=2000
producers,samples,p50_ns,p95_ns,p99_ns
1,2000,600,1000,3200
2,4000,1000,1700,2600
4,8000,700,3200,9700
8,16000,1300,5400,32100
16,32000,1100,6000,173300
## before-fast-r05-spawn-latency
concurrent external spawn-return latency, calls_per_producer=2000
producers,samples,p50_ns,p95_ns,p99_ns
1,2000,700,1800,4200
2,4000,1100,3700,19000
4,8000,700,18700,57900
8,16000,1600,25000,108400
16,32000,1800,61900,152300
```

### After

```text
## after-fast-r01-latency
probe,high_load,p50_ns,p95_ns,p99_ns
High,128,900,1600,1900
High,1024,900,1500,1800
High,8192,900,1600,2000
Normal,128,1000,1700,2000
Normal,1024,1100,1800,2200
Normal,8192,900,1600,1900
Background,128,900,1500,1900
Background,1024,900,1500,1900
Background,8192,1200,1800,2300
## after-fast-r02-latency
probe,high_load,p50_ns,p95_ns,p99_ns
High,128,900,1700,2100
High,1024,900,1500,1700
High,8192,1000,1600,1900
Normal,128,900,1400,1900
Normal,1024,900,1500,2800
Normal,8192,900,1800,2100
Background,128,900,1500,1700
Background,1024,900,1700,2000
Background,8192,1100,1800,2400
## after-fast-r03-latency
probe,high_load,p50_ns,p95_ns,p99_ns
High,128,1000,1500,2000
High,1024,900,1500,1900
High,8192,900,1600,2200
Normal,128,900,1600,2200
Normal,1024,900,1300,1700
Normal,8192,1000,1600,2000
Background,128,900,1400,1800
Background,1024,1000,1500,1800
Background,8192,1000,1500,2200
## after-fast-r04-latency
probe,high_load,p50_ns,p95_ns,p99_ns
High,128,1000,1800,2400
High,1024,900,1600,2100
High,8192,900,1700,2400
Normal,128,900,1400,2100
Normal,1024,900,1400,1600
Normal,8192,900,1500,1800
Background,128,900,1400,1800
Background,1024,900,1700,2100
Background,8192,900,1700,1900
## after-fast-r05-latency
probe,high_load,p50_ns,p95_ns,p99_ns
High,128,900,1600,2100
High,1024,900,1500,1700
High,8192,1100,1800,2100
Normal,128,900,1600,2100
Normal,1024,900,1500,2100
Normal,8192,1000,1700,2300
Background,128,900,1600,1800
Background,1024,900,1500,2000
Background,8192,900,1600,2100
## after-fast-r01-spawn-latency
concurrent external spawn-return latency, calls_per_producer=2000
producers,samples,p50_ns,p95_ns,p99_ns
1,2000,700,3200,6400
2,4000,900,1300,1600
4,8000,900,1800,2700
8,16000,1200,3300,5700
16,32000,1100,3100,5500
## after-fast-r02-spawn-latency
concurrent external spawn-return latency, calls_per_producer=2000
producers,samples,p50_ns,p95_ns,p99_ns
1,2000,600,3200,5800
2,4000,800,1400,1900
4,8000,800,1700,2500
8,16000,900,2000,3600
16,32000,1100,3500,6800
## after-fast-r03-spawn-latency
concurrent external spawn-return latency, calls_per_producer=2000
producers,samples,p50_ns,p95_ns,p99_ns
1,2000,600,1700,3600
2,4000,800,1300,2200
4,8000,900,2000,3500
8,16000,1000,2700,4600
16,32000,800,2000,3900
## after-fast-r04-spawn-latency
concurrent external spawn-return latency, calls_per_producer=2000
producers,samples,p50_ns,p95_ns,p99_ns
1,2000,600,2600,4400
2,4000,800,1200,2100
4,8000,1100,1800,2200
8,16000,1000,2700,5100
16,32000,1100,3300,6100
## after-fast-r05-spawn-latency
concurrent external spawn-return latency, calls_per_producer=2000
producers,samples,p50_ns,p95_ns,p99_ns
1,2000,600,1600,4000
2,4000,700,1100,1500
4,8000,800,1400,2000
8,16000,800,1500,2100
16,32000,800,2300,4800
```

## Acceptance summary

- High p95/p99 versus the slower Normal/Background probe: pass at all loads.
- General spawn: one small-case ID in the documented 5–10% band; no rejection.
- Yield storm: one unchanged-control ID in the documented 5–10% band; no
  rejection.
- Nested locality: pass, worst regression +0.99%.
- Mixed priority throughput: pass, worst regression +4.87%.
- External producer contention and concentrated completion: material
  improvements in the contended cases.
- Decision: retain `High=8 / Normal=16 / Background=64`; do not introduce the
  deferred urgent-global-check design in this round.
