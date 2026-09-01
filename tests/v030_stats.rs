#![cfg(feature = "stats")]

//! Black-box contracts for the v0.3 scheduler counters.
//!
//! These tests deliberately assert counter *relationships*, rather than timing
//! or a global task-completion order.  The scheduler is concurrent, so queue
//! depths are only snapshots; spawn origin and successful steal counters are
//! stable facts after the submitted work has completed.

use async_runtime::{Priority, RuntimeBuilder, SpawnError};
use futures_lite::future;
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(5);

fn runtime(workers: usize) -> async_runtime::Runtime {
    RuntimeBuilder::new(NonZeroUsize::new(workers).expect("non-zero worker count"))
        .build()
        .expect("runtime starts")
}

fn wait_until(mut predicate: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + DEADLINE;
    while !predicate() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn stats_distinguish_external_and_worker_local_spawns() {
    const CHILDREN: usize = 32;
    let runtime = runtime(1);
    let spawner = runtime.spawner();

    // The parent comes from this host thread, while every child is submitted
    // while the sole worker is polling the parent.  This is the public,
    // observable contract for worker-local routing.
    let parent = runtime
        .spawn(Priority::Normal, async move {
            for value in 0..CHILDREN {
                assert_eq!(
                    spawner
                        .spawn(Priority::High, async move { value })
                        .expect("runtime remains open")
                        .await,
                    value
                );
            }
        })
        .expect("parent is accepted");
    future::block_on(parent);

    let stats = runtime.stats();
    assert_eq!(
        stats.external_spawned, 1,
        "only the parent is host-submitted"
    );
    assert_eq!(stats.local_spawned, CHILDREN as u64);
    assert!(stats.executed >= (CHILDREN + 1) as u64);
    runtime
        .shutdown_graceful()
        .expect("graceful shutdown succeeds");
}

#[test]
fn stats_count_host_submissions_as_external() {
    const TASKS: usize = 24;
    let runtime = runtime(1);
    let mut tasks = Vec::with_capacity(TASKS);
    for value in 0..TASKS {
        tasks.push(
            runtime
                .spawn(Priority::Normal, async move { value })
                .expect("host task is accepted"),
        );
    }
    for (value, task) in tasks.into_iter().enumerate() {
        assert_eq!(future::block_on(task), value);
    }

    let stats = runtime.stats();
    assert_eq!(stats.external_spawned, TASKS as u64);
    assert_eq!(stats.local_spawned, 0);
    assert!(stats.executed >= TASKS as u64);
    runtime
        .shutdown_graceful()
        .expect("graceful shutdown succeeds");
}

#[test]
fn idle_workers_park_and_a_submission_wakes_one() {
    let runtime = runtime(2);
    wait_until(
        || runtime.stats().sleeping_workers == 2,
        "both workers to park",
    );
    let before = runtime.stats();
    assert!(before.parks >= 2);

    let (done_tx, done_rx) = mpsc::channel();
    runtime
        .spawn(Priority::High, async move {
            done_tx.send(()).expect("receiver remains alive");
        })
        .expect("submission wakes a worker")
        .detach();
    done_rx
        .recv_timeout(DEADLINE)
        .expect("parked worker runs high-priority work after wake");

    let after = runtime.stats();
    assert!(
        after.wakes > before.wakes,
        "new work must wake a parked worker"
    );
    assert!(after.executed > before.executed);
    runtime
        .shutdown_graceful()
        .expect("graceful shutdown succeeds");
}

#[test]
fn repeated_bursts_leave_every_worker_registered_for_future_wakes() {
    const BURSTS: usize = 64;
    let runtime = runtime(2);
    let (done_tx, done_rx) = mpsc::channel();

    for _ in 0..BURSTS {
        wait_until(
            || runtime.stats().sleeping_workers == 2,
            "both workers to register as sleeping between bursts",
        );
        for _ in 0..2 {
            let done_tx = done_tx.clone();
            runtime
                .spawn(Priority::Normal, async move {
                    done_tx.send(()).expect("receiver remains alive");
                })
                .expect("burst task is accepted")
                .detach();
        }
        done_rx
            .recv_timeout(DEADLINE)
            .expect("first burst task completes");
        done_rx
            .recv_timeout(DEADLINE)
            .expect("second burst task completes");
    }

    wait_until(
        || runtime.stats().sleeping_workers == 2,
        "both workers to remain wakeable after repeated bursts",
    );
    runtime
        .shutdown_graceful()
        .expect("graceful shutdown succeeds");
}

#[test]
fn nested_backlog_is_stolen_by_an_idle_worker() {
    const CHILDREN: usize = 128;
    let runtime = runtime(2);
    let spawner = runtime.spawner();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = async_channel::bounded::<()>(1);
    let (done_tx, done_rx) = mpsc::channel();
    let executed_by = Arc::new(Mutex::new(HashSet::new()));
    let completed = Arc::new(AtomicUsize::new(0));

    let parent = runtime
        .spawn(Priority::Normal, {
            let executed_by = Arc::clone(&executed_by);
            let completed = Arc::clone(&completed);
            async move {
                for _ in 0..CHILDREN {
                    let executed_by = Arc::clone(&executed_by);
                    let completed = Arc::clone(&completed);
                    let done_tx = done_tx.clone();
                    spawner
                        .spawn(Priority::Normal, async move {
                            // A short blocking payload holds the thief long enough
                            // to make a successful batch steal observable without
                            // relying on task completion order.
                            std::thread::sleep(Duration::from_millis(1));
                            executed_by
                                .lock()
                                .expect("worker id set is not poisoned")
                                .insert(std::thread::current().id());
                            if completed.fetch_add(1, Ordering::AcqRel) + 1 == CHILDREN {
                                done_tx.send(()).expect("receiver remains alive");
                            }
                        })
                        .expect("parent submits child locally")
                        .detach();
                }
                ready_tx.send(()).expect("test waits for backlog");
                release_rx.recv().await.expect("test releases parent");
            }
        })
        .expect("parent is accepted");

    ready_rx
        .recv_timeout(DEADLINE)
        .expect("worker created nested local backlog");
    done_rx
        .recv_timeout(DEADLINE)
        .expect("all nested children complete");
    release_tx
        .send_blocking(())
        .expect("parent is still waiting");
    future::block_on(parent);

    let stats = runtime.stats();
    assert_eq!(stats.local_spawned, CHILDREN as u64);
    assert!(
        stats.steal_attempts > 0,
        "idle worker probes a victim queue"
    );
    assert!(stats.stolen > 0, "nested local backlog is stolen");
    assert_eq!(completed.load(Ordering::Acquire), CHILDREN);
    assert!(
        executed_by
            .lock()
            .expect("worker id set is not poisoned")
            .len()
            >= 2,
        "both workers execute the imbalanced nested backlog"
    );
    runtime
        .shutdown_graceful()
        .expect("graceful shutdown succeeds");
}

#[test]
fn background_progresses_while_high_queue_is_continuously_replenished() {
    const HIGH_CHAIN: usize = 96;
    let runtime = runtime(1);
    let spawner = runtime.spawner();
    let (background_tx, background_rx) = mpsc::channel();
    let (high_done_tx, high_done_rx) = mpsc::channel();

    // Each high task creates its successor from the worker, maintaining a
    // local high-priority source while the background task is waiting.
    fn submit_high_chain(
        spawner: async_runtime::Spawner,
        remaining: usize,
        done: mpsc::Sender<()>,
    ) {
        spawner
            .clone()
            .spawn(Priority::High, async move {
                if remaining == 0 {
                    done.send(()).expect("test waits for high chain");
                } else {
                    submit_high_chain(spawner, remaining - 1, done);
                }
            })
            .expect("runtime stays open while chain is active")
            .detach();
    }

    submit_high_chain(spawner, HIGH_CHAIN, high_done_tx);
    runtime
        .spawn(Priority::Background, async move {
            background_tx.send(()).expect("test waits for background");
        })
        .expect("background task is accepted")
        .detach();

    background_rx
        .recv_timeout(DEADLINE)
        .expect("background task must not starve behind high replenishment");
    high_done_rx
        .recv_timeout(DEADLINE)
        .expect("high chain completes");
    let stats = runtime.stats();
    assert!(stats.local_spawned >= HIGH_CHAIN as u64);
    assert!(stats.executed >= (HIGH_CHAIN + 2) as u64);
    runtime
        .shutdown_graceful()
        .expect("graceful shutdown succeeds");
}

#[test]
fn external_work_progresses_beside_a_continuously_local_same_priority_task() {
    use std::sync::atomic::AtomicBool;

    let runtime = runtime(1);
    let stop = Arc::new(AtomicBool::new(false));
    let local_stop = Arc::clone(&stop);
    let spinner = runtime
        .spawn(Priority::High, async move {
            while !local_stop.load(Ordering::Acquire) {
                future::yield_now().await;
            }
        })
        .expect("local spinner is accepted");

    wait_until(
        || runtime.stats().executed >= 128,
        "self-waking task to establish a local queue source",
    );
    let (done_tx, done_rx) = mpsc::channel();
    runtime
        .spawn(Priority::High, async move {
            done_tx.send(()).expect("test remains alive");
        })
        .expect("external same-priority task is accepted")
        .detach();
    done_rx
        .recv_timeout(DEADLINE)
        .expect("global injector must not starve behind a local queue");

    stop.store(true, Ordering::Release);
    future::block_on(spinner);
    runtime
        .shutdown_graceful()
        .expect("graceful shutdown succeeds");
}

#[test]
fn graceful_shutdown_after_parking_rejects_old_spawners() {
    let runtime = runtime(2);
    let spawner = runtime.spawner();
    wait_until(
        || runtime.stats().sleeping_workers == 2,
        "workers to park before shutdown",
    );

    runtime
        .shutdown_graceful()
        .expect("shutdown wakes and joins parked workers");
    assert!(matches!(
        spawner.spawn(Priority::Normal, async {}),
        Err(SpawnError::Closed)
    ));
}
