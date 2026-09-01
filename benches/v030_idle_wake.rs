//! Run with `cargo bench --bench v030_idle_wake`.
//! Waits until every worker is observably parked, then measures a full external
//! submit/wake/complete/re-park cycle. Requires `--features stats`. CPU
//! percentage requires an OS profiler, not Criterion.

use async_runtime::{Priority, RuntimeBuilder};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use std::num::NonZeroUsize;
use std::thread;
use std::time::Duration;

const WORKERS: [usize; 4] = [1, 2, 4, 8];

fn idle_wake(c: &mut Criterion) {
    let mut group = c.benchmark_group("v030/idle-wake-latency");
    group.measurement_time(Duration::from_secs(8));
    for workers in WORKERS {
        let runtime = RuntimeBuilder::new(NonZeroUsize::new(workers).expect("non-zero workers"))
            .build()
            .expect("runtime");
        let spawner = runtime.spawner();
        group.bench_with_input(BenchmarkId::from_parameter(workers), &workers, |b, _| {
            b.iter(|| {
                while runtime.stats().sleeping_workers != workers {
                    thread::yield_now();
                }
                let task = spawner
                    .spawn(Priority::High, async { std::hint::black_box(()) })
                    .expect("runtime open");
                futures_lite::future::block_on(task);
            });
        });
        runtime.shutdown_graceful().expect("shutdown");
    }
    group.finish();
}

criterion_group!(benches, idle_wake);
criterion_main!(benches);
