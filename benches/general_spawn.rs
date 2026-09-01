//! Run with `cargo bench --bench general_spawn`.
//! Measures general-runtime spawn, scheduling, and completion together. Task
//! construction is intentionally included because it is part of this API path.

use async_runtime::{Priority, RuntimeBuilder};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures_lite::future;
use std::num::NonZeroUsize;

const BATCH_SIZES: [usize; 3] = [1, 64, 1024];
const NESTED_CHILDREN: [usize; 3] = [1, 10, 100];

fn general_spawn(c: &mut Criterion) {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).expect("non-zero workers"))
        .build()
        .expect("runtime");
    let mut group = c.benchmark_group("general/spawn-and-complete");

    for batch_size in BATCH_SIZES {
        group.throughput(Throughput::Elements(batch_size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &batch_size| {
                b.iter(|| {
                    let tasks = (0..batch_size)
                        .map(|_| {
                            runtime
                                .spawn(Priority::Normal, async { 1_u8 })
                                .expect("spawn")
                        })
                        .collect::<Vec<_>>();
                    for task in tasks {
                        std::hint::black_box(future::block_on(task));
                    }
                });
            },
        );
    }
    group.finish();

    let spawner = runtime.spawner();
    let mut nested = c.benchmark_group("general/nested-spawn-and-complete");
    for children in NESTED_CHILDREN {
        nested.throughput(Throughput::Elements(children as u64));
        nested.bench_with_input(
            BenchmarkId::from_parameter(children),
            &children,
            |b, &children| {
                b.iter(|| {
                    let child_spawner = spawner.clone();
                    let outer = runtime
                        .spawn(Priority::Normal, async move {
                            let children = (0..children)
                                .map(|_| {
                                    child_spawner
                                        .spawn(Priority::Normal, async { 1_usize })
                                        .expect("nested spawn")
                                })
                                .collect::<Vec<_>>();
                            let mut completed = 0;
                            for child in children {
                                completed += child.await;
                            }
                            completed
                        })
                        .expect("outer spawn");
                    std::hint::black_box(future::block_on(outer));
                });
            },
        );
    }
    nested.finish();
    runtime.shutdown_graceful().expect("shutdown");
}

criterion_group!(benches, general_spawn);
criterion_main!(benches);
