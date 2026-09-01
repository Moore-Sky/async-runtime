//! Run with `cargo bench --bench v030_priority_latency`.
//! Measures submission-to-completion latency for probes while a fixed set of
//! High tasks continuously yields. Each case stops and drains its load before
//! the next case, so samples cannot accumulate detached work.

use async_runtime::{Priority, RuntimeBuilder};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use futures_lite::future;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const HIGH_LOAD: [usize; 3] = [128, 1_024, 8_192];

fn priority_latency(c: &mut Criterion) {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).expect("non-zero workers"))
        .build()
        .expect("runtime");
    let mut group = c.benchmark_group("v030/priority-probe-latency");
    group.measurement_time(Duration::from_secs(8));
    for probe_priority in [Priority::High, Priority::Normal, Priority::Background] {
        for high_load in HIGH_LOAD {
            let stop = Arc::new(AtomicBool::new(false));
            let high_tasks = (0..high_load)
                .map(|_| {
                    let stop = Arc::clone(&stop);
                    runtime
                        .spawn(Priority::High, async move {
                            while !stop.load(Ordering::Relaxed) {
                                future::yield_now().await;
                            }
                        })
                        .expect("high spawn")
                })
                .collect::<Vec<_>>();
            group.bench_with_input(
                BenchmarkId::new(format!("{probe_priority:?}"), high_load),
                &high_load,
                |b, &_high_load| {
                    b.iter(|| {
                        let probe = runtime
                            .spawn(probe_priority, async { std::hint::black_box(()) })
                            .expect("probe spawn");
                        future::block_on(probe);
                    });
                },
            );
            stop.store(true, Ordering::Relaxed);
            for task in high_tasks {
                future::block_on(task);
            }
        }
    }
    group.finish();
    runtime.shutdown_graceful().expect("shutdown");
}

criterion_group!(benches, priority_latency);
criterion_main!(benches);
