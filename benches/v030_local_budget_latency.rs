//! Run with `cargo bench --bench v030_local_budget_latency`.
//!
//! This is a per-call latency sampler, not a Criterion benchmark. It measures
//! the soft-budget behavior of `LocalDomain::run_for` for already-ready local
//! work and for commands entering through the remote inbox. Ready-queue work
//! is a `Future::poll`; remote-inbox work is command-closure execution. Either
//! kind of work is cooperative and cannot be preempted, so its execution time
//! may appear as budget overshoot.
//!
//! `max` is printed as an observation only. OS preemption makes it unsuitable
//! as a portable regression gate; compare p95/p99 across repeated runs on the
//! same machine instead.

use async_runtime::LocalDomain;
use std::hint::black_box;
use std::time::{Duration, Instant};

const BUDGETS_US: [u64; 3] = [100, 500, 1_000];
const WORK_TARGETS_US: [u64; 3] = [20, 100, 500];
const DEFAULT_SAMPLES: usize = 10_000;
const WARMUP_SAMPLES: usize = 100;

#[derive(Clone, Copy)]
enum Scenario {
    ReadyQueue,
    RemoteInbox,
}

impl Scenario {
    fn label(self) -> &'static str {
        match self {
            Self::ReadyQueue => "ready_queue",
            Self::RemoteInbox => "remote_inbox",
        }
    }
}

#[inline(never)]
fn cpu_kernel(iterations: u64, seed: u64) -> u64 {
    let mut value = seed ^ 0x9e37_79b9_7f4a_7c15;
    for index in 0..iterations {
        value ^= index.wrapping_add(0x517c_c1b7_2722_0a95);
        value = value.rotate_left(17).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^= value >> 29;
    }
    black_box(value)
}

fn measure_kernel(iterations: u64) -> Duration {
    const REPEATS: u64 = 16;
    let started = Instant::now();
    for seed in 0..REPEATS {
        black_box(cpu_kernel(iterations, seed));
    }
    started.elapsed() / REPEATS as u32
}

fn calibrate(target: Duration) -> (u64, Duration) {
    let target_ns = target.as_nanos();
    let mut iterations = 4_096_u64;
    let mut best = (iterations, Duration::ZERO);
    let mut best_error = u128::MAX;

    for _ in 0..8 {
        let measured = measure_kernel(iterations);
        let measured_ns = measured.as_nanos().max(1);
        let error = measured_ns.abs_diff(target_ns);
        if error < best_error {
            best = (iterations, measured);
            best_error = error;
        }
        if error.saturating_mul(100) <= target_ns.saturating_mul(3) {
            break;
        }

        let proposed = (iterations as u128)
            .saturating_mul(target_ns)
            .saturating_add(measured_ns / 2)
            / measured_ns;
        let proposed = proposed.min(u64::MAX as u128) as u64;
        if proposed == iterations || proposed == 0 {
            break;
        }
        iterations = proposed;
    }
    best
}

fn percentile(sorted: &[Duration], numerator: usize, denominator: usize) -> Duration {
    let rank = (sorted.len() * numerator + denominator - 1) / denominator;
    sorted[rank.saturating_sub(1)]
}

fn samples_from_env() -> usize {
    std::env::var("ASYNC_RUNTIME_LOCAL_BUDGET_SAMPLES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|&value| value > 0)
        .unwrap_or(DEFAULT_SAMPLES)
}

fn queued_work_items(budget: Duration, observed_work: Duration) -> usize {
    let work_ns = observed_work.as_nanos().max(1);
    let needed = budget.as_nanos().saturating_add(work_ns - 1) / work_ns;
    usize::try_from(needed.saturating_add(2)).unwrap_or(usize::MAX)
}

fn prepare_domain(
    scenario: Scenario,
    work_items: usize,
    iterations: u64,
    seed_base: u64,
) -> LocalDomain {
    let domain = LocalDomain::new();
    match scenario {
        Scenario::ReadyQueue => {
            for offset in 0..work_items {
                domain
                    .spawn_local(async move {
                        black_box(cpu_kernel(iterations, seed_base + offset as u64));
                    })
                    .expect("ready-queue spawn")
                    .detach();
            }
        }
        Scenario::RemoteInbox => {
            let local = domain.spawner();
            for offset in 0..work_items {
                local
                    .dispatch(move || {
                        black_box(cpu_kernel(iterations, seed_base + offset as u64));
                    })
                    .expect("remote-inbox dispatch");
            }
        }
    }
    domain
}

fn sample_case(
    scenario: Scenario,
    budget: Duration,
    target_work: Duration,
    iterations: u64,
    observed_work: Duration,
    samples: usize,
) {
    let work_items = queued_work_items(budget, observed_work);
    let mut stats_elapsed = Vec::with_capacity(samples);
    let mut outer_elapsed = Vec::with_capacity(samples);
    let mut stats_overshoot = Vec::with_capacity(samples);
    let mut outer_overshoot = Vec::with_capacity(samples);
    let mut seed = 0_u64;

    for sample in 0..samples + WARMUP_SAMPLES {
        let domain = prepare_domain(scenario, work_items, iterations, seed);
        seed = seed.wrapping_add(work_items as u64);

        let outer_started = Instant::now();
        let stats = domain.run_for(budget);
        let caller_elapsed = outer_started.elapsed();
        black_box((stats.drive_steps, stats.inbox_commands, caller_elapsed));

        if sample >= WARMUP_SAMPLES {
            stats_elapsed.push(stats.elapsed);
            outer_elapsed.push(caller_elapsed);
            stats_overshoot.push(stats.elapsed.saturating_sub(budget));
            outer_overshoot.push(caller_elapsed.saturating_sub(budget));
        }
    }

    stats_elapsed.sort_unstable();
    outer_elapsed.sort_unstable();
    stats_overshoot.sort_unstable();
    outer_overshoot.sort_unstable();
    println!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
        scenario.label(),
        budget.as_micros(),
        target_work.as_micros(),
        observed_work.as_nanos(),
        samples,
        percentile(&stats_elapsed, 50, 100).as_nanos(),
        percentile(&stats_elapsed, 95, 100).as_nanos(),
        percentile(&stats_elapsed, 99, 100).as_nanos(),
        stats_elapsed
            .last()
            .expect("stats elapsed sample")
            .as_nanos(),
        percentile(&outer_elapsed, 50, 100).as_nanos(),
        percentile(&outer_elapsed, 95, 100).as_nanos(),
        percentile(&outer_elapsed, 99, 100).as_nanos(),
        outer_elapsed
            .last()
            .expect("outer elapsed sample")
            .as_nanos(),
        percentile(&stats_overshoot, 50, 100).as_nanos(),
        percentile(&stats_overshoot, 95, 100).as_nanos(),
        percentile(&stats_overshoot, 99, 100).as_nanos(),
        stats_overshoot
            .last()
            .expect("stats overshoot sample")
            .as_nanos(),
        percentile(&outer_overshoot, 50, 100).as_nanos(),
        percentile(&outer_overshoot, 95, 100).as_nanos(),
        percentile(&outer_overshoot, 99, 100).as_nanos(),
        outer_overshoot
            .last()
            .expect("outer overshoot sample")
            .as_nanos(),
    );
}

fn main() {
    let samples = samples_from_env();
    eprintln!(
        "local budget latency samples={samples}, warmup={WARMUP_SAMPLES}; max is observation-only"
    );
    println!(
        "scenario,budget_us,work_target_us,observed_work_ns,samples,stats_elapsed_p50_ns,stats_elapsed_p95_ns,stats_elapsed_p99_ns,stats_elapsed_max_observation_ns,outer_elapsed_p50_ns,outer_elapsed_p95_ns,outer_elapsed_p99_ns,outer_elapsed_max_observation_ns,stats_overshoot_p50_ns,stats_overshoot_p95_ns,stats_overshoot_p99_ns,stats_overshoot_max_observation_ns,outer_overshoot_p50_ns,outer_overshoot_p95_ns,outer_overshoot_p99_ns,outer_overshoot_max_observation_ns"
    );

    for target_us in WORK_TARGETS_US {
        let target = Duration::from_micros(target_us);
        let (iterations, observed_work) = calibrate(target);
        eprintln!(
            "work target={}us iterations={} observed={}ns",
            target_us,
            iterations,
            observed_work.as_nanos()
        );
        for scenario in [Scenario::ReadyQueue, Scenario::RemoteInbox] {
            for budget_us in BUDGETS_US {
                sample_case(
                    scenario,
                    Duration::from_micros(budget_us),
                    target,
                    iterations,
                    observed_work,
                    samples,
                );
            }
        }
    }
}
