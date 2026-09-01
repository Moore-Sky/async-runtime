//! Run with `cargo bench --bench local_driving`.
//! Measures only host-driven `run_n` and `run_for` work. Domain construction and
//! local task submission happen outside Criterion's timed interval.

use async_runtime::LocalDomain;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::time::{Duration, Instant};

const BATCH_SIZES: [usize; 3] = [1, 64, 1024];

fn drive_to_idle(domain: &LocalDomain, max_steps: usize) {
    while !domain.is_empty() {
        assert!(domain.run_n(max_steps) > 0, "ready work must make progress");
    }
}

fn local_driving(c: &mut Criterion) {
    let mut run_n = c.benchmark_group("local/drive-only-run-n");
    for batch_size in BATCH_SIZES {
        run_n.throughput(Throughput::Elements(batch_size as u64));
        run_n.bench_with_input(
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
                        std::hint::black_box(domain.run_n(batch_size));
                        elapsed += started.elapsed();
                        drive_to_idle(&domain, batch_size);
                    }
                    elapsed
                });
            },
        );
    }
    run_n.finish();

    let mut run_for = c.benchmark_group("local/drive-only-run-for");
    for budget in [
        Duration::from_micros(100),
        Duration::from_micros(500),
        Duration::from_millis(1),
    ] {
        run_for.throughput(Throughput::Elements(64));
        run_for.bench_with_input(
            BenchmarkId::from_parameter(format!("{}us", budget.as_micros())),
            &budget,
            |b, &budget| {
                b.iter_custom(|iterations| {
                    let mut elapsed = Duration::ZERO;
                    for _ in 0..iterations {
                        let domain = LocalDomain::new();
                        for _ in 0..64 {
                            domain.spawn_local(async {}).expect("local spawn").detach();
                        }
                        let started = Instant::now();
                        let stats = domain.run_for(budget);
                        elapsed += started.elapsed();
                        std::hint::black_box((
                            stats.drive_steps,
                            stats.inbox_commands,
                            stats.elapsed,
                        ));
                        drive_to_idle(&domain, 64);
                    }
                    elapsed
                });
            },
        );
    }
    run_for.finish();
}

criterion_group!(benches, local_driving);
criterion_main!(benches);
