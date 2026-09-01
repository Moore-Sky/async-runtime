//! Run with `cargo bench --bench shutdown`.
//! Measures graceful shutdown while draining already accepted, immediately-ready
//! local work. Domain creation and task submission are excluded from the timer.

use async_runtime::LocalDomain;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures_lite::future;
use std::time::{Duration, Instant};

const BATCH_SIZES: [usize; 3] = [1, 64, 1024];

fn shutdown(c: &mut Criterion) {
    let mut group = c.benchmark_group("local/graceful-shutdown-ready-work");
    for batch_size in BATCH_SIZES {
        group.throughput(Throughput::Elements(batch_size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &batch_size| {
                b.iter_custom(|iterations| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iterations {
                        let domain = LocalDomain::new();
                        for _ in 0..batch_size {
                            domain.spawn_local(async {}).expect("local spawn").detach();
                        }
                        let started = Instant::now();
                        future::block_on(domain.shutdown_graceful());
                        elapsed += started.elapsed();
                    }
                    elapsed
                });
            },
        );
    }
    group.finish();

    let mut immediate = c.benchmark_group("local/shutdown-now-cancellation");
    for batch_size in BATCH_SIZES {
        immediate.throughput(Throughput::Elements(batch_size as u64));
        immediate.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &batch_size| {
                b.iter_custom(|iterations| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iterations {
                        let domain = LocalDomain::new();
                        for _ in 0..batch_size {
                            domain
                                .spawn_local(async { std::future::pending::<()>().await })
                                .expect("local spawn")
                                .detach();
                        }
                        let started = Instant::now();
                        domain.shutdown_now();
                        elapsed += started.elapsed();
                    }
                    elapsed
                });
            },
        );
    }
    immediate.finish();
}

criterion_group!(benches, shutdown);
criterion_main!(benches);
