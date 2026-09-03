# Local-budget caller-side spot check (v0.3.3)

This supplementary check was captured on 2026-09-03 after caller-side timing
was added to `v030_local_budget_latency`. It used one round with 2,000 samples
per case. It is not part of the repeatable five-round
[v0.3 release baseline](baseline-v0.3-release.md), whose original capture did
not retain caller-side elapsed distributions.

The selected rows below compare the p99 reported by `RunStats::elapsed` with
the p99 observed around the complete `run_for` call. Times are microseconds.

| Source | Budget | Work target | Stats p99 | Outer p99 | Outer overshoot p99 |
| --- | ---: | ---: | ---: | ---: | ---: |
| Ready queue | 100 us | 20 us | 149.0 | 149.5 | 49.5 |
| Ready queue | 500 us | 100 us | 706.5 | 706.7 | 206.7 |
| Ready queue | 1000 us | 500 us | 1987.0 | 1987.1 | 987.1 |
| Remote inbox | 100 us | 20 us | 154.6 | 157.5 | 57.5 |
| Remote inbox | 500 us | 100 us | 706.0 | 706.0 | 206.0 |
| Remote inbox | 1000 us | 500 us | 3660.0 | 3660.2 | 2660.2 |

The two timers closely track one another in these samples, indicating that the
measured overshoot occurred within the `run_for` driving interval rather than
after it returned. The check does not attribute that overshoot to a single
cause. A drive step cannot be interrupted by the budget check, and OS
scheduling, CPU frequency changes, and per-run calibration may also affect the
observed distribution.

Ready-queue work in this sampler is a `Future::poll`; remote-inbox work is
command-closure execution. Printed maximums remain observations only and are
not portable regression gates.
