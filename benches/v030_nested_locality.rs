//! Run with `cargo bench --bench v030_nested_locality`.
//! Measures nested general-runtime spawn and completion. A v0.3 scheduler should
//! make this path benefit from worker-local submission; use `--features stats`
//! and scheduler tests to prove the local-queue mechanism itself.

use async_runtime::{Priority, RuntimeBuilder};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures_lite::future;
use std::num::NonZeroUsize;

const WORKERS: [usize; 4] = [1, 2, 4, 8];
const CHILDREN: [usize; 4] = [1, 10, 100, 1_000];

fn nested_locality(c: &mut Criterion) {
    let mut group = c.benchmark_group("v030/nested-locality");
    for workers in WORKERS {
        let runtime = RuntimeBuilder::new(NonZeroUsize::new(workers).expect("non-zero workers"))
            .build()
            .expect("runtime");
        let spawner = runtime.spawner();
        for children in CHILDREN {
            group.throughput(Throughput::Elements(children as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("workers-{workers}"), children),
                &children,
                |b, &children| {
                    b.iter(|| {
                        let child_spawner = spawner.clone();
                        let parent = runtime
                            .spawn(Priority::Normal, async move {
                                let children = (0..children)
                                    .map(|_| {
                                        child_spawner
                                            .spawn(Priority::Normal, async { 1_usize })
                                            .expect("nested spawn")
                                    })
                                    .collect::<Vec<_>>();
                                let mut sum = 0;
                                for child in children {
                                    sum += child.await;
                                }
                                sum
                            })
                            .expect("parent spawn");
                        std::hint::black_box(future::block_on(parent));
                    });
                },
            );
        }
        runtime.shutdown_graceful().expect("shutdown");
    }
    group.finish();
}

criterion_group!(benches, nested_locality);
criterion_main!(benches);
