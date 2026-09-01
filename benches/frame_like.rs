//! Run with `cargo bench --bench frame_like`.
//! Simulates a host frame with 64 preloaded remote-capable inbox commands, then
//! spends a bounded slice in `run_for`. Submission and post-budget cleanup are
//! excluded from timing; this reports budget-driving cost, not frame setup.

use async_runtime::LocalDomain;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::time::{Duration, Instant};

const FRAME_TASKS: usize = 64;

fn drain_to_idle(domain: &LocalDomain) {
    while !domain.is_empty() {
        assert!(
            domain.run_n(FRAME_TASKS) > 0,
            "ready frame work must progress"
        );
    }
}

fn frame_like(c: &mut Criterion) {
    let mut group = c.benchmark_group("local/frame-like-run-for");
    for budget in [
        Duration::from_micros(100),
        Duration::from_micros(500),
        Duration::from_millis(1),
    ] {
        group.throughput(Throughput::Elements(FRAME_TASKS as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{}us", budget.as_micros())),
            &budget,
            |b, &budget| {
                b.iter_custom(|iterations| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iterations {
                        let domain = LocalDomain::new();
                        let spawner = domain.spawner();
                        for _ in 0..FRAME_TASKS {
                            spawner.dispatch_future(async {}).expect("frame dispatch");
                        }

                        let started = Instant::now();
                        let stats = domain.run_for(budget);
                        elapsed += started.elapsed();
                        std::hint::black_box((
                            stats.drive_steps,
                            stats.inbox_commands,
                            stats.elapsed,
                        ));
                        drain_to_idle(&domain);
                    }
                    elapsed
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, frame_like);
criterion_main!(benches);
