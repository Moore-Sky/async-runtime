//! Run with `cargo bench --bench v030_local_budget_latency`.
//!
//! This is a per-call latency sampler, not a Criterion benchmark. It measures
//! the soft-budget behavior of `LocalDomain::run_for` for already-ready local
//! work and for commands entering through the remote inbox. A single future
//! poll is cooperative and cannot be preempted, so poll time may appear as
//! budget overshoot.
//!
//! `max` is printed as an observation only. OS preemption makes it unsuitable
//! as a portable regression gate; compare p95/p99 across repeated runs on the
//! same machine instead.

use async_runtime::LocalDomain;
use std::hint::black_box;
use std::time::{Duration, Instant};

const BUDGETS_US: [u64; 3] = [100, 500, 1_000];
const POLL_TARGETS_US: [u64; 3] = [20, 100, 500];
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

fn queued_polls(budget: Duration, observed_poll: Duration) -> usize {
    let poll_ns = observed_poll.as_nanos().max(1);
    let needed = budget.as_nanos().saturating_add(poll_ns - 1) / poll_ns;
    usize::try_from(needed.saturating_add(2)).unwrap_or(usize::MAX)
}

fn prepare_domain(
    scenario: Scenario,
    polls: usize,
    iterations: u64,
    seed_base: u64,
) -> LocalDomain {
    let domain = LocalDomain::new();
    match scenario {
        Scenario::ReadyQueue => {
            for offset in 0..polls {
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
            for offset in 0..polls {
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
    target_poll: Duration,
    iterations: u64,
    observed_poll: Duration,
    samples: usize,
) {
    let polls = queued_polls(budget, observed_poll);
    let mut elapsed = Vec::with_capacity(samples);
    let mut overshoot = Vec::with_capacity(samples);
    let mut seed = 0_u64;

    for sample in 0..samples + WARMUP_SAMPLES {
        let domain = prepare_domain(scenario, polls, iterations, seed);
        seed = seed.wrapping_add(polls as u64);

        let outer_started = Instant::now();
        let stats = domain.run_for(budget);
        let outer_elapsed = outer_started.elapsed();
        black_box((stats.drive_steps, stats.inbox_commands, outer_elapsed));

        if sample >= WARMUP_SAMPLES {
            elapsed.push(stats.elapsed);
            overshoot.push(stats.elapsed.saturating_sub(budget));
        }
    }

    elapsed.sort_unstable();
    overshoot.sort_unstable();
    println!(
        "{},{},{},{},{},{},{},{},{},{},{},{},{}",
        scenario.label(),
        budget.as_micros(),
        target_poll.as_micros(),
        observed_poll.as_nanos(),
        samples,
        percentile(&elapsed, 50, 100).as_nanos(),
        percentile(&elapsed, 95, 100).as_nanos(),
        percentile(&elapsed, 99, 100).as_nanos(),
        elapsed.last().expect("elapsed sample").as_nanos(),
        percentile(&overshoot, 50, 100).as_nanos(),
        percentile(&overshoot, 95, 100).as_nanos(),
        percentile(&overshoot, 99, 100).as_nanos(),
        overshoot.last().expect("overshoot sample").as_nanos(),
    );
}

fn main() {
    let samples = samples_from_env();
    eprintln!(
        "local budget latency samples={samples}, warmup={WARMUP_SAMPLES}; max is observation-only"
    );
    println!(
        "scenario,budget_us,poll_target_us,observed_poll_ns,samples,elapsed_p50_ns,elapsed_p95_ns,elapsed_p99_ns,elapsed_max_observation_ns,overshoot_p50_ns,overshoot_p95_ns,overshoot_p99_ns,overshoot_max_observation_ns"
    );

    for target_us in POLL_TARGETS_US {
        let target = Duration::from_micros(target_us);
        let (iterations, observed_poll) = calibrate(target);
        eprintln!(
            "poll target={}us iterations={} observed={}ns",
            target_us,
            iterations,
            observed_poll.as_nanos()
        );
        for scenario in [Scenario::ReadyQueue, Scenario::RemoteInbox] {
            for budget_us in BUDGETS_US {
                sample_case(
                    scenario,
                    Duration::from_micros(budget_us),
                    target,
                    iterations,
                    observed_poll,
                    samples,
                );
            }
        }
    }
}
