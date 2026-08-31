use async_runtime::{Priority, RuntimeBuilder};
use std::num::NonZeroUsize;
use std::sync::{mpsc, Arc, Barrier};
use std::time::Duration;

fn runtime(workers: usize) -> async_runtime::Runtime {
    RuntimeBuilder::new(NonZeroUsize::new(workers).unwrap())
        .build()
        .unwrap()
}

#[test]
fn runs_tasks_on_multiple_workers() {
    let runtime = runtime(2);
    let (started_tx, started_rx) = mpsc::channel();
    let release = Arc::new(Barrier::new(3));

    for _ in 0..2 {
        runtime
            .spawn(Priority::Normal, {
                let started_tx = started_tx.clone();
                let release = Arc::clone(&release);
                async move {
                    started_tx.send(std::thread::current().id()).unwrap();
                    // Deliberately occupy this worker until both tasks have
                    // started, forcing the second task onto another worker.
                    release.wait();
                }
            })
            .unwrap()
            .detach();
    }
    drop(started_tx);

    let first = started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    let second = started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    release.wait();
    assert_ne!(
        first, second,
        "two blocking tasks should occupy two workers"
    );
    runtime.shutdown_graceful().unwrap();
}

#[test]
fn supports_nested_spawn_and_await() {
    let runtime = runtime(2);
    let spawner = runtime.spawner();
    let task = runtime
        .spawn(Priority::High, async move {
            spawner
                .spawn(Priority::Background, async { 40_u32 + 2 })
                .unwrap()
                .await
        })
        .unwrap();

    assert_eq!(async_io::block_on(task), 42);
    runtime.shutdown_graceful().unwrap();
}

#[test]
fn distinct_priority_queues_all_make_progress() {
    let runtime = runtime(1);
    let (tx, rx) = mpsc::channel();

    for priority in [Priority::High, Priority::Normal, Priority::Background] {
        let tx = tx.clone();
        runtime
            .spawn(priority, async move { tx.send(priority).unwrap() })
            .unwrap()
            .detach();
    }
    drop(tx);

    let mut seen = Vec::new();
    for _ in 0..3 {
        seen.push(rx.recv_timeout(Duration::from_secs(1)).unwrap());
    }
    assert!(seen.contains(&Priority::High));
    assert!(seen.contains(&Priority::Normal));
    assert!(seen.contains(&Priority::Background));
    runtime.shutdown_graceful().unwrap();
}
