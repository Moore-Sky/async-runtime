use async_runtime::{LocalDomain, Priority, RuntimeBuilder};
use std::num::NonZeroUsize;

#[test]
fn general_task_can_submit_to_local_and_await_completion() {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let local = LocalDomain::new();
    let local_spawner = local.spawner();

    let task = runtime
        .spawn(Priority::Normal, async move {
            local_spawner.spawn(async { 42_u32 }).unwrap().await
        })
        .unwrap();
    assert_eq!(async_io::block_on(local.run(task)), 42);

    async_io::block_on(local.shutdown_graceful());
    runtime.shutdown_graceful().unwrap();
}

#[test]
fn local_task_can_submit_to_general_and_returns_to_owner() {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let local = LocalDomain::new();
    let spawner = runtime.spawner();
    let owner = std::thread::current().id();

    let task = local
        .spawn_local(async move {
            let value = spawner
                .spawn(Priority::High, async { 42_u32 })
                .unwrap()
                .await;
            assert_eq!(std::thread::current().id(), owner);
            value
        })
        .unwrap();
    assert_eq!(async_io::block_on(local.run(task)), 42);

    async_io::block_on(local.shutdown_graceful());
    runtime.shutdown_graceful().unwrap();
}
