//! Run with `cargo bench --bench v030_steal_imbalance`.
//! A parent creates all children from within a worker, then the children yield
//! repeatedly. This is the closest portable public-API approximation of an
//! imbalanced local queue. Exact initial worker placement and steal counts need
//! the v0.3 `stats` feature or a test-only scheduler hook.

use async_runtime::{Priority, RuntimeBuilder};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures_lite::future;
use std::num::NonZeroUsize;

const WORKERS: [usize; 4] = [1, 2, 4, 8];
const TASKS: [usize; 3] = [256, 1_024, 4_096];
const YIELDS: usize = 8;

fn steal_imbalance(c: &mut Criterion) {
    let mut group = c.benchmark_group("v030/steal-imbalance");
    for workers in WORKERS {
        let runtime = RuntimeBuilder::new(NonZeroUsize::new(workers).expect("non-zero workers"))
            .build()
            .expect("runtime");
        let spawner = runtime.spawner();
        for tasks in TASKS {
            group.throughput(Throughput::Elements((tasks * YIELDS) as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("workers-{workers}"), tasks),
                &tasks,
                |b, &tasks| {
                    b.iter(|| {
                        let child_spawner = spawner.clone();
                        let parent = runtime
                            .spawn(Priority::Normal, async move {
                                let children = (0..tasks)
                                    .map(|_| {
                                        child_spawner
                                            .spawn(Priority::Normal, async {
                                                for _ in 0..YIELDS {
                                                    future::yield_now().await;
                                                }
                                            })
                                            .expect("child spawn")
                                    })
                                    .collect::<Vec<_>>();
                                for child in children {
                                    child.await;
                                }
                            })
                            .expect("parent spawn");
                        future::block_on(parent);
                    });
                },
            );
        }
        runtime.shutdown_graceful().expect("shutdown");
    }
    group.finish();
}

criterion_group!(benches, steal_imbalance);
criterion_main!(benches);
