//! Run with `cargo bench --bench priority`.
//! Measures priority-aware submission and completion throughput, including an
//! 8:4:1 High/Normal/Background mix. It does not claim p99 latency or fairness.

use async_runtime::{Priority, RuntimeBuilder};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures_lite::future;
use std::num::NonZeroUsize;

const BATCH_SIZES: [usize; 3] = [1, 64, 1024];

fn priority_costs(c: &mut Criterion) {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).expect("non-zero workers"))
        .build()
        .expect("runtime");
    let mut group = c.benchmark_group("general/priority-batch");

    for priority in [Priority::High, Priority::Normal, Priority::Background] {
        for batch_size in BATCH_SIZES {
            group.throughput(Throughput::Elements(batch_size as u64));
            group.bench_with_input(
                BenchmarkId::new(format!("{priority:?}"), batch_size),
                &batch_size,
                |b, &batch_size| {
                    b.iter(|| {
                        let tasks = (0..batch_size)
                            .map(|_| runtime.spawn(priority, async { 1_u8 }).expect("spawn"))
                            .collect::<Vec<_>>();
                        for task in tasks {
                            std::hint::black_box(future::block_on(task));
                        }
                    });
                },
            );
        }
    }
    group.finish();

    let mut mixed = c.benchmark_group("general/priority-mixed-throughput");
    for batches in BATCH_SIZES {
        const MIXED_TASKS: usize = 8 + 4 + 1;
        mixed.throughput(Throughput::Elements((batches * MIXED_TASKS) as u64));
        mixed.bench_with_input(
            BenchmarkId::new("high-8_normal-4_background-1", batches),
            &batches,
            |b, &batches| {
                b.iter(|| {
                    let mut tasks = Vec::with_capacity(batches * MIXED_TASKS);
                    for _ in 0..batches {
                        for (priority, count) in [
                            (Priority::High, 8),
                            (Priority::Normal, 4),
                            (Priority::Background, 1),
                        ] {
                            tasks.extend(
                                (0..count).map(|_| {
                                    runtime.spawn(priority, async { 1_u8 }).expect("spawn")
                                }),
                            );
                        }
                    }
                    for task in tasks {
                        std::hint::black_box(future::block_on(task));
                    }
                });
            },
        );
    }
    mixed.finish();
    runtime.shutdown_graceful().expect("shutdown");
}

criterion_group!(benches, priority_costs);
criterion_main!(benches);
