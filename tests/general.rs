use async_runtime::{Priority, RuntimeBuilder};
use futures_lite::future;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

    assert_eq!(future::block_on(task), 42);
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

#[test]
fn external_task_is_not_starved_by_same_priority_local_requeues() {
    let runtime = runtime(1);
    let stop = Arc::new(AtomicBool::new(false));
    let polls = Arc::new(AtomicUsize::new(0));
    let spinner = runtime
        .spawn(Priority::High, {
            let stop = Arc::clone(&stop);
            let polls = Arc::clone(&polls);
            async move {
                while !stop.load(Ordering::Acquire) {
                    polls.fetch_add(1, Ordering::Relaxed);
                    future::yield_now().await;
                }
            }
        })
        .unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while polls.load(Ordering::Relaxed) < 128 {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    let (done_tx, done_rx) = mpsc::channel();
    runtime
        .spawn(Priority::High, async move { done_tx.send(()).unwrap() })
        .unwrap()
        .detach();
    done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("global injector must receive bounded service");

    stop.store(true, Ordering::Release);
    future::block_on(spinner);
    runtime.shutdown_graceful().unwrap();
}
