//! Run with `cargo bench --bench yield_storm`.
//! Measures owner-thread scheduling and wake-up churn from a fixed number of
//! `yield_now` calls. Domain creation and task submission are outside the timed
//! interval; only driving the preloaded ready task set is timed.

use async_runtime::LocalDomain;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures_lite::future;
use std::time::{Duration, Instant};

const TASK_COUNTS: [usize; 3] = [200, 1000, 10_000];
const YIELDS_PER_TASK: usize = 8;

fn yield_storm(c: &mut Criterion) {
    let mut group = c.benchmark_group("local/yield-storm-drive-only");
    for task_count in TASK_COUNTS {
        group.throughput(Throughput::Elements((task_count * YIELDS_PER_TASK) as u64));
        group.bench_with_input(
            BenchmarkId::new("yield-now-8", task_count),
            &task_count,
            |b, &task_count| {
                b.iter_custom(|iterations| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iterations {
                        let domain = LocalDomain::new();
                        for _ in 0..task_count {
                            domain
                                .spawn_local(async {
                                    for _ in 0..YIELDS_PER_TASK {
                                        future::yield_now().await;
                                    }
                                })
                                .expect("local spawn")
                                .detach();
                        }

                        let started = Instant::now();
                        while !domain.is_empty() {
                            assert!(
                                domain.run_n(task_count) > 0,
                                "yielding work must make progress"
                            );
                        }
                        elapsed += started.elapsed();
                    }
                    elapsed
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, yield_storm);
criterion_main!(benches);
