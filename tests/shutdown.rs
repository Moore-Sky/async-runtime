use async_runtime::{Priority, RuntimeBuilder, ShutdownError, ShutdownOutcome, SpawnError};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, mpsc};
use std::time::Duration;

fn runtime(workers: usize) -> async_runtime::Runtime {
    RuntimeBuilder::new(NonZeroUsize::new(workers).unwrap())
        .build()
        .unwrap()
}

#[test]
fn graceful_waits_for_accepted_detached_tasks() {
    let runtime = runtime(1);
    let (tx, rx) = mpsc::channel();
    runtime
        .spawn(Priority::Normal, async move {
            async_io::Timer::after(Duration::from_millis(30)).await;
            tx.send(()).unwrap();
        })
        .unwrap()
        .detach();

    runtime.shutdown_graceful().unwrap();
    rx.recv_timeout(Duration::from_millis(1))
        .expect("graceful shutdown must drain accepted detached work");
}

#[test]
fn timeout_cancels_remaining_work_and_reports_it() {
    let runtime = runtime(1);
    runtime
        .spawn(Priority::Normal, async {
            std::future::pending::<()>().await;
        })
        .unwrap()
        .detach();
    assert!(matches!(
        runtime.shutdown_timeout(Duration::from_millis(30)).unwrap(),
        ShutdownOutcome::TimedOut {
            remaining_tasks: 1..
        }
    ));
}

#[test]
fn shutdown_now_cancels_task_and_old_spawner_is_closed() {
    let runtime = runtime(1);
    let spawner = runtime.spawner();
    let pending = spawner
        .spawn(Priority::Normal, async {
            std::future::pending::<u8>().await
        })
        .unwrap();
    runtime.shutdown_now().unwrap();
    assert_eq!(async_io::block_on(pending.fallible()), None);
    assert!(matches!(
        spawner.spawn(Priority::Normal, async {}),
        Err(SpawnError::Closed)
    ));
}

#[test]
fn explicit_shutdown_from_own_worker_returns_called_from_worker() {
    let runtime = runtime(2);
    let (runtime_tx, runtime_rx) = async_channel::bounded::<async_runtime::Runtime>(1);
    let (result_tx, result_rx) = mpsc::channel();
    runtime
        .spawn(Priority::Normal, async move {
            let runtime = runtime_rx.recv().await.unwrap();
            result_tx.send(runtime.shutdown_now()).unwrap();
        })
        .unwrap()
        .detach();
    runtime_tx.send_blocking(runtime).unwrap();

    assert_eq!(
        result_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Err(ShutdownError::CalledFromWorker)
    );
}

#[test]
fn dropping_last_runtime_owner_on_its_worker_does_not_self_join() {
    let runtime = runtime(2);
    let (runtime_tx, runtime_rx) = async_channel::bounded::<async_runtime::Runtime>(1);
    let (done_tx, done_rx) = mpsc::channel();
    runtime
        .spawn(Priority::Normal, async move {
            let runtime = runtime_rx.recv().await.unwrap();
            drop(runtime);
            done_tx.send(()).unwrap();
        })
        .unwrap()
        .detach();
    runtime_tx.send_blocking(runtime).unwrap();

    done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("dropping Runtime from its own worker must not self-join");
}

#[test]
fn concurrent_spawn_and_graceful_close_drains_every_success() {
    let runtime = runtime(2);
    let spawner = runtime.spawner();
    let start = Arc::new(Barrier::new(5));
    let accepted = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let mut submitters = Vec::new();

    for _ in 0..4 {
        let spawner = spawner.clone();
        let start = Arc::clone(&start);
        let accepted = Arc::clone(&accepted);
        let completed = Arc::clone(&completed);
        submitters.push(std::thread::spawn(move || {
            start.wait();
            for _ in 0..200 {
                let completed = Arc::clone(&completed);
                match spawner.spawn(Priority::Normal, async move {
                    completed.fetch_add(1, Ordering::Relaxed);
                }) {
                    Ok(task) => {
                        accepted.fetch_add(1, Ordering::Relaxed);
                        task.detach();
                    }
                    Err(SpawnError::Closed) => break,
                }
            }
        }));
    }

    start.wait();
    runtime.shutdown_graceful().unwrap();
    for submitter in submitters {
        submitter.join().unwrap();
    }
    assert_eq!(
        completed.load(Ordering::Relaxed),
        accepted.load(Ordering::Relaxed)
    );
}

#[test]
fn stale_drain_notification_does_not_finish_a_later_graceful_shutdown() {
    let runtime = runtime(1);
    let first = runtime.spawn(Priority::Normal, async {}).unwrap();
    async_io::block_on(first);

    let (done_tx, done_rx) = mpsc::channel();
    runtime
        .spawn(Priority::Normal, async move {
            async_io::Timer::after(Duration::from_millis(20)).await;
            done_tx.send(()).unwrap();
        })
        .unwrap()
        .detach();

    runtime.shutdown_graceful().unwrap();
    done_rx
        .try_recv()
        .expect("graceful shutdown must re-check the accepted task count");
}
