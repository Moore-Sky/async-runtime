use async_runtime::{LocalDomain, Priority, RuntimeBuilder};
use criterion::{Criterion, criterion_group, criterion_main};
use std::num::NonZeroUsize;

fn executor_costs(c: &mut Criterion) {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).expect("non-zero"))
        .build()
        .expect("runtime");

    c.bench_function("general/spawn-and-schedule", |b| {
        b.iter(|| {
            let task = runtime
                .spawn(Priority::Normal, async { 1_u8 })
                .expect("spawn");
            std::hint::black_box(async_io::block_on(task));
        });
    });

    c.bench_function("general/priority-cycle", |b| {
        b.iter(|| {
            let tasks = [Priority::High, Priority::Normal, Priority::Background]
                .map(|priority| runtime.spawn(priority, async {}).expect("spawn"));
            for task in tasks {
                async_io::block_on(task);
            }
        });
    });

    runtime.shutdown_graceful().expect("shutdown");

    let local = LocalDomain::new();
    c.bench_function("local/spawn-and-drive", |b| {
        b.iter(|| {
            let task = local.spawn_local(async { 1_u8 }).expect("local spawn");
            std::hint::black_box(async_io::block_on(local.run(task)));
        });
    });

    let remote = local.spawner();
    c.bench_function("local/remote-inbox", |b| {
        b.iter(|| {
            let task = remote.spawn(async { 1_u8 }).expect("remote spawn");
            std::hint::black_box(async_io::block_on(local.run(task)));
        });
    });
    async_io::block_on(local.shutdown_graceful());
}

criterion_group!(benches, executor_costs);
criterion_main!(benches);
