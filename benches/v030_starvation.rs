//! Run with `cargo bench --bench v030_starvation`.
//! Measures Background probe progress while a fixed High population continually
//! requeues itself. It is a bounded performance scenario, not a scheduler SLA;
//! permanent-starvation semantics belong to the functional test suite.

use async_runtime::{Priority, RuntimeBuilder};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use futures_lite::future;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const HIGH_BACKLOG: [usize; 3] = [256, 2_048, 16_384];
const YIELDS_PER_HIGH: usize = 4;

fn starvation(c: &mut Criterion) {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).expect("non-zero workers"))
        .build()
        .expect("runtime");
    let mut group = c.benchmark_group("v030/background-progress-under-high-load");
    for backlog in HIGH_BACKLOG {
        let stop = Arc::new(AtomicBool::new(false));
        let high_tasks = (0..backlog)
            .map(|_| {
                let stop = Arc::clone(&stop);
                runtime
                    .spawn(Priority::High, async move {
                        while !stop.load(Ordering::Relaxed) {
                            for _ in 0..YIELDS_PER_HIGH {
                                future::yield_now().await;
                            }
                        }
                    })
                    .expect("high spawn")
            })
            .collect::<Vec<_>>();
        group.bench_with_input(
            BenchmarkId::from_parameter(backlog),
            &backlog,
            |b, &_backlog| {
                b.iter(|| {
                    let probe = runtime
                        .spawn(Priority::Background, async { std::hint::black_box(()) })
                        .expect("background spawn");
                    future::block_on(probe);
                });
            },
        );
        stop.store(true, Ordering::Relaxed);
        for task in high_tasks {
            future::block_on(task);
        }
    }
    group.finish();
    runtime.shutdown_graceful().expect("shutdown");
}

criterion_group!(benches, starvation);
criterion_main!(benches);
