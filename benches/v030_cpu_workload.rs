//! Run with `cargo bench --bench v030_cpu_workload`.
//!
//! This is a sampler, rather than a Criterion benchmark. It establishes a
//! realistic scheduler baseline for CPU-bound futures: every task performs one
//! non-yielding, fixed-iteration poll. Such a poll is deliberately
//! non-preemptible, so a High task that is already running can delay every
//! other priority until its kernel returns.
//!
//! The kernel is fixed iteration rather than `Instant` busy-waiting. Its
//! duration is machine-dependent (measure and record it with the results), but
//! it avoids making clock reads part of the workload. On the release test
//! machine this is intended to be a representative tens-to-hundreds of
//! microseconds task, not a portable duration guarantee.

use async_runtime::{Priority, Runtime, RuntimeBuilder, Spawner};
use futures_lite::future;
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

const WORKERS: [usize; 4] = [1, 2, 4, 8];
const TOTAL_NESTED_TASKS: usize = 1_024;
const THROUGHPUT_RUNS: usize = 5;
const WARMUP_RUNS: usize = 2;
const MIXED_BATCHES: usize = 10_000;
const KERNEL_ROUNDS: usize = 100_000;

#[derive(Clone, Copy)]
struct Sample {
    priority: Priority,
    latency: Duration,
}

/// A dependency-carrying integer kernel that cannot be reduced to a constant.
#[inline(never)]
fn cpu_kernel(mut state: u64) -> u64 {
    for _ in 0..KERNEL_ROUNDS {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state = state.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    std::hint::black_box(state)
}

fn percentile(samples: &mut [Duration], numerator: usize, denominator: usize) -> Duration {
    samples.sort_unstable();
    let rank = (samples.len() * numerator + denominator - 1) / denominator;
    samples[rank.saturating_sub(1)]
}

fn run_nested_batch(runtime: &Runtime, parents: usize) -> Duration {
    assert_eq!(TOTAL_NESTED_TASKS % parents, 0);
    let children_per_parent = TOTAL_NESTED_TASKS / parents;
    let spawner = runtime.spawner();
    let started = Instant::now();
    let parents = (0..parents)
        .map(|parent_number| {
            let child_spawner = spawner.clone();
            runtime
                .spawn(Priority::Normal, async move {
                    let children = (0..children_per_parent)
                        .map(|child_number| {
                            let seed = ((parent_number as u64) << 32) | child_number as u64;
                            child_spawner
                                .spawn(Priority::Normal, async move { cpu_kernel(seed) })
                                .expect("nested CPU spawn")
                        })
                        .collect::<Vec<_>>();
                    let mut sum = 0_u64;
                    for child in children {
                        sum = sum.wrapping_add(child.await);
                    }
                    sum
                })
                .expect("parent CPU spawn")
        })
        .collect::<Vec<_>>();
    for parent in parents {
        std::hint::black_box(future::block_on(parent));
    }
    started.elapsed()
}

fn print_throughput_scaling() {
    println!("throughput,workers,tasks,elapsed_ns,tasks_per_second");
    for workers in WORKERS {
        let runtime = RuntimeBuilder::new(NonZeroUsize::new(workers).expect("non-zero workers"))
            .build()
            .expect("runtime");
        for _ in 0..WARMUP_RUNS {
            std::hint::black_box(run_nested_batch(&runtime, workers));
        }
        for _ in 0..THROUGHPUT_RUNS {
            let elapsed = run_nested_batch(&runtime, workers);
            let tasks_per_second = TOTAL_NESTED_TASKS as f64 / elapsed.as_secs_f64();
            println!(
                "throughput,{workers},{TOTAL_NESTED_TASKS},{},{tasks_per_second:.3}",
                elapsed.as_nanos()
            );
        }
        runtime.shutdown_graceful().expect("shutdown");
    }
}

fn submit_sample(spawner: &Spawner, priority: Priority, seed: u64) -> async_runtime::Task<Sample> {
    let submitted = Instant::now();
    spawner
        .spawn(priority, async move {
            std::hint::black_box(cpu_kernel(seed));
            Sample {
                priority,
                latency: submitted.elapsed(),
            }
        })
        .expect("mixed CPU spawn")
}

fn print_mixed_priority_latency() {
    // One worker makes the priority ordering and the non-preemptible-poll cost
    // directly observable. Scaling is measured separately above.
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).expect("non-zero workers"))
        .build()
        .expect("runtime");
    let spawner = runtime.spawner();
    let mut high = Vec::with_capacity(MIXED_BATCHES * 8);
    let mut normal = Vec::with_capacity(MIXED_BATCHES * 4);
    let mut background = Vec::with_capacity(MIXED_BATCHES);

    for batch in 0..MIXED_BATCHES {
        let mut tasks = Vec::with_capacity(13);
        for (priority, count) in [
            (Priority::High, 8_usize),
            (Priority::Normal, 4),
            (Priority::Background, 1),
        ] {
            for offset in 0..count {
                tasks.push(submit_sample(
                    &spawner,
                    priority,
                    (batch * 13 + offset) as u64,
                ));
            }
        }
        for task in tasks {
            let sample = future::block_on(task);
            match sample.priority {
                Priority::High => high.push(sample.latency),
                Priority::Normal => normal.push(sample.latency),
                Priority::Background => background.push(sample.latency),
            }
        }
    }

    println!("priority_latency,priority,samples,p50_ns,p95_ns,p99_ns");
    for (priority, mut samples) in [
        (Priority::High, high),
        (Priority::Normal, normal),
        (Priority::Background, background),
    ] {
        let p50 = percentile(&mut samples.clone(), 50, 100);
        let p95 = percentile(&mut samples.clone(), 95, 100);
        let p99 = percentile(&mut samples, 99, 100);
        println!(
            "priority_latency,{priority:?},{},{},{},{}",
            samples.len(),
            p50.as_nanos(),
            p95.as_nanos(),
            p99.as_nanos()
        );
    }
    runtime.shutdown_graceful().expect("shutdown");
}

fn main() {
    let kernel_started = Instant::now();
    std::hint::black_box(cpu_kernel(0x1234_5678_9abc_def0));
    let observed_kernel = kernel_started.elapsed();
    eprintln!(
        "CPU kernel rounds={KERNEL_ROUNDS}; throughput runs={THROUGHPUT_RUNS}; \
         mixed batches={MIXED_BATCHES} (8:4:1, one worker); observed kernel={}ns",
        observed_kernel.as_nanos()
    );
    print_throughput_scaling();
    print_mixed_priority_latency();
}
