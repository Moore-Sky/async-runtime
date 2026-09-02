//! Run with `cargo bench --bench v030_spawn_latency_sampling`.
//!
//! Reports per-call return latency of concurrent external `Spawner::spawn`
//! calls. End-to-end producer throughput remains in `v030_external_producers`.

use async_runtime::{Priority, RuntimeBuilder, Spawner};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

const PRODUCERS: [usize; 5] = [1, 2, 4, 8, 16];
const CALLS_PER_PRODUCER: usize = 2_000;

fn percentile(samples: &mut [Duration], numerator: usize, denominator: usize) -> Duration {
    samples.sort_unstable();
    let rank = (samples.len() * numerator + denominator - 1) / denominator;
    samples[rank.saturating_sub(1)]
}

fn run(spawner: Spawner, producers: usize) -> Vec<Duration> {
    let barrier = Arc::new(Barrier::new(producers));
    let completed = Arc::new(AtomicUsize::new(0));
    let (sender, receiver) = mpsc::channel();
    for _ in 0..producers {
        let barrier = Arc::clone(&barrier);
        let completed = Arc::clone(&completed);
        let sender = sender.clone();
        let spawner = spawner.clone();
        thread::spawn(move || {
            barrier.wait();
            let mut samples = Vec::with_capacity(CALLS_PER_PRODUCER);
            for _ in 0..CALLS_PER_PRODUCER {
                let started = Instant::now();
                let completed = Arc::clone(&completed);
                spawner
                    .spawn(Priority::Normal, async move {
                        completed.fetch_add(1, Ordering::Release);
                    })
                    .expect("runtime alive")
                    .detach();
                samples.push(started.elapsed());
            }
            sender.send(samples).expect("owner alive");
        });
    }
    drop(sender);
    let samples = receiver.into_iter().flatten().collect::<Vec<_>>();
    let expected = producers * CALLS_PER_PRODUCER;
    while completed.load(Ordering::Acquire) != expected {
        thread::yield_now();
    }
    samples
}

fn main() {
    eprintln!("concurrent external spawn-return latency, calls_per_producer={CALLS_PER_PRODUCER}");
    println!("producers,samples,p50_ns,p95_ns,p99_ns");
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(4).unwrap())
        .build()
        .expect("runtime");
    for producers in PRODUCERS {
        let mut samples = run(runtime.spawner(), producers);
        let p50 = percentile(&mut samples.clone(), 50, 100);
        let p95 = percentile(&mut samples.clone(), 95, 100);
        let p99 = percentile(&mut samples, 99, 100);
        println!(
            "{producers},{},{},{},{}",
            producers * CALLS_PER_PRODUCER,
            p50.as_nanos(),
            p95.as_nanos(),
            p99.as_nanos()
        );
    }
    runtime.shutdown_graceful().expect("shutdown");
}
