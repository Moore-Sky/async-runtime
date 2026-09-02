//! Run with `cargo bench --bench v030_latency_sampling`.
//!
//! This is deliberately not a Criterion benchmark: it records every probe and
//! prints order-statistic p50/p95/p99 values. Criterion's estimates describe
//! an iteration's central tendency, not individual task latency percentiles.

use async_runtime::{Priority, RuntimeBuilder};
use futures_lite::future;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const HIGH_LOAD: [usize; 3] = [128, 1_024, 8_192];
const DEFAULT_SAMPLES: usize = 10_000;
const WARMUP: usize = 1_000;

fn percentile(samples: &mut [Duration], numerator: usize, denominator: usize) -> Duration {
    samples.sort_unstable();
    let rank = (samples.len() * numerator + denominator - 1) / denominator;
    let index = rank.saturating_sub(1);
    samples[index]
}

fn samples_from_env() -> usize {
    std::env::var("ASYNC_RUNTIME_LATENCY_SAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|&value| value > 0)
        .unwrap_or(DEFAULT_SAMPLES)
}

fn main() {
    let samples = samples_from_env();
    eprintln!("priority probe latency samples={samples}, warmup={WARMUP}");
    println!("probe,high_load,p50_ns,p95_ns,p99_ns");

    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).unwrap())
        .build()
        .expect("runtime");
    for priority in [Priority::High, Priority::Normal, Priority::Background] {
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

            for _ in 0..WARMUP {
                let task = runtime.spawn(priority, async {}).expect("warmup spawn");
                future::block_on(task);
            }
            let mut observed = Vec::with_capacity(samples);
            for _ in 0..samples {
                let started = Instant::now();
                let task = runtime.spawn(priority, async {}).expect("probe spawn");
                future::block_on(task);
                observed.push(started.elapsed());
            }
            let p50 = percentile(&mut observed.clone(), 50, 100);
            let p95 = percentile(&mut observed.clone(), 95, 100);
            let p99 = percentile(&mut observed, 99, 100);
            println!(
                "{priority:?},{high_load},{},{},{}",
                p50.as_nanos(),
                p95.as_nanos(),
                p99.as_nanos()
            );

            stop.store(true, Ordering::Release);
            for task in high_tasks {
                future::block_on(task);
            }
        }
    }
    runtime.shutdown_graceful().expect("shutdown");
}
