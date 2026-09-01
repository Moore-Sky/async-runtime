use async_runtime::{LocalDomain, ShutdownOutcome};
use futures_lite::future;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{mpsc, Arc, Barrier};
use std::time::Duration;

struct DropNotifies {
    dropped: Option<mpsc::Sender<()>>,
}

impl std::future::Future for DropNotifies {
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::task::Poll::Pending
    }
}

impl Drop for DropNotifies {
    fn drop(&mut self) {
        self.dropped
            .take()
            .expect("future must be dropped exactly once")
            .send(())
            .unwrap();
    }
}

#[test]
fn spawn_local_accepts_rc_and_polls_on_owner_thread() {
    let local = LocalDomain::new();
    let owner = std::thread::current().id();
    let polls = Rc::new(Cell::new(0));
    let value = Rc::new(RefCell::new(41_u32));

    let task = local
        .spawn_local({
            let polls = polls.clone();
            let value = value.clone();
            async move {
                assert_eq!(std::thread::current().id(), owner);
                polls.set(polls.get() + 1);
                *value.borrow() + 1
            }
        })
        .unwrap();

    assert_eq!(future::block_on(local.run(task)), 42);
    assert_eq!(polls.get(), 1);
    future::block_on(local.shutdown_graceful());
}

#[test]
fn remote_spawner_delivers_send_future_to_owner_driver() {
    let local = LocalDomain::new();
    let remote = local.spawner();
    let (done_tx, done_rx) = mpsc::channel();

    std::thread::spawn(move || {
        remote
            .spawn(async move { done_tx.send(std::thread::current().id()).unwrap() })
            .unwrap()
            .detach();
    })
    .join()
    .unwrap();

    // The owner is responsible for driving received commands as well as local work.
    let executed_on = future::block_on(async {
        loop {
            match done_rx.try_recv() {
                Ok(thread_id) => break thread_id,
                Err(mpsc::TryRecvError::Empty) => local.tick().await,
                Err(mpsc::TryRecvError::Disconnected) => panic!("remote task did not run"),
            }
        }
    });
    assert_eq!(executed_on, std::thread::current().id());
    future::block_on(local.shutdown_graceful());
}

#[test]
fn remote_local_panic_is_not_reported_as_cancellation() {
    let local = LocalDomain::new();
    let remote = local.spawner();
    let task = std::thread::spawn(move || {
        remote
            .spawn(async { panic!("remote local panic payload") })
            .unwrap()
    })
    .join()
    .unwrap();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        future::block_on(local.run(task))
    }));
    let payload = result.expect_err("the original task panic must be rethrown");
    let message = payload.downcast_ref::<&str>().copied().unwrap_or_default();
    assert_eq!(message, "remote local panic payload");
    future::block_on(local.shutdown_graceful());
}

#[test]
fn remote_local_fallible_task_still_propagates_panic() {
    let local = LocalDomain::new();
    let remote = local.spawner();
    let task = std::thread::spawn(move || {
        remote
            .spawn(async { panic!("panic is not cancellation") })
            .unwrap()
            .fallible()
    })
    .join()
    .unwrap();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        future::block_on(local.run(task))
    }));
    assert!(result.is_err());
    future::block_on(local.shutdown_graceful());
}

#[test]
fn local_try_tick_is_non_blocking_when_empty() {
    let local = LocalDomain::new();
    assert!(local.is_empty());
    assert!(!local.try_tick());
    local.shutdown_now();
}

#[test]
fn run_accepts_a_future_borrowing_from_the_owner_stack() {
    let local = LocalDomain::new();
    let value = String::from("borrowed");

    let observed = future::block_on(local.run(async { value.as_str() }));
    assert_eq!(observed, "borrowed");
    future::block_on(local.shutdown_graceful());
}

#[test]
fn graceful_materializes_commands_accepted_before_closing() {
    let local = LocalDomain::new();
    let remote = local.spawner();
    let (done_tx, done_rx) = mpsc::channel();

    remote
        .spawn(async move { done_tx.send(42_u8).unwrap() })
        .unwrap()
        .detach();

    future::block_on(local.shutdown_graceful());
    assert_eq!(done_rx.recv().unwrap(), 42);
}

#[test]
fn local_timeout_reports_and_cancels_remaining_work() {
    let local = LocalDomain::new();
    let task = local
        .spawn_local(std::future::pending::<u8>())
        .unwrap()
        .fallible();

    let outcome = async_io::block_on(
        local.shutdown_until(async_io::Timer::after(std::time::Duration::from_millis(20))),
    );
    assert!(matches!(
        outcome,
        ShutdownOutcome::TimedOut { remaining_tasks: 1 }
    ));
    assert_eq!(future::block_on(task), None);
}

#[test]
fn dropped_remote_handle_is_cancelled_before_materialization() {
    let local = LocalDomain::new();
    let task = local.spawner().spawn(std::future::pending::<u8>()).unwrap();
    drop(task);

    assert_eq!(
        async_io::block_on(
            local.shutdown_until(async_io::Timer::after(std::time::Duration::from_secs(1))),
        ),
        ShutdownOutcome::Completed
    );
}

#[test]
fn a_busy_remote_inbox_does_not_starve_local_runnables() {
    let local = LocalDomain::new();
    let remote = local.spawner();
    let (local_tx, local_rx) = mpsc::channel();

    local
        .spawn_local(async move { local_tx.send(()).unwrap() })
        .unwrap()
        .detach();
    for _ in 0..100 {
        remote.spawn(async {}).unwrap().detach();
    }

    assert!(local.try_tick());
    local_rx
        .try_recv()
        .expect("processing an inbox command must also give local work an opportunity");
    future::block_on(local.shutdown_graceful());
}

#[test]
fn local_shutdown_rejects_old_spawner() {
    let local = LocalDomain::new();
    let spawner = local.spawner();
    local.shutdown_now();
    assert!(matches!(
        spawner.spawn(async {}),
        Err(async_runtime::SpawnError::Closed)
    ));
}

#[test]
fn run_n_is_non_blocking_and_counts_drive_steps() {
    let local = LocalDomain::new();
    let (done_tx, done_rx) = mpsc::channel();

    local
        .spawn_local(async move { done_tx.send(()).unwrap() })
        .unwrap()
        .detach();

    assert_eq!(local.run_n(0), 0);
    assert!(done_rx.try_recv().is_err());
    assert_eq!(local.run_n(1), 1);
    done_rx
        .try_recv()
        .expect("one drive step must make the scheduled runnable progress");
    assert_eq!(local.run_n(1), 0);
    local.shutdown_now();
}

#[test]
fn run_n_stops_at_the_requested_step_limit_and_leaves_remaining_work() {
    let local = LocalDomain::new();
    let (done_tx, done_rx) = mpsc::channel();

    for value in 0..4 {
        let done_tx = done_tx.clone();
        local
            .spawn_local(async move { done_tx.send(value).unwrap() })
            .unwrap()
            .detach();
    }
    drop(done_tx);

    assert_eq!(local.run_n(2), 2);
    assert_eq!(done_rx.try_iter().count(), 2);
    assert_eq!(local.run_n(usize::MAX), 2);
    assert_eq!(done_rx.try_iter().count(), 2);
    assert_eq!(local.run_n(usize::MAX), 0);
    local.shutdown_now();
}

#[test]
fn run_for_zero_budget_does_not_make_progress() {
    let local = LocalDomain::new();
    let (done_tx, done_rx) = mpsc::channel();

    local
        .spawn_local(async move { done_tx.send(()).unwrap() })
        .unwrap()
        .detach();

    let stats = local.run_for(Duration::ZERO);
    assert_eq!(stats.drive_steps, 0);
    assert_eq!(stats.inbox_commands, 0);
    assert!(done_rx.try_recv().is_err());

    assert_eq!(local.run_n(1), 1);
    done_rx.try_recv().unwrap();
    local.shutdown_now();
}

#[test]
fn run_for_reports_inbox_and_local_drive_progress() {
    let local = LocalDomain::new();
    let remote = local.spawner();
    let (done_tx, done_rx) = mpsc::channel();

    remote.dispatch(move || done_tx.send(()).unwrap()).unwrap();

    let stats = local.run_for(Duration::from_secs(1));
    assert_eq!(stats.inbox_commands, 1);
    assert!(stats.drive_steps >= 1);
    done_rx.try_recv().unwrap();
    local.shutdown_now();
}

#[test]
fn run_for_is_non_blocking_when_idle_and_reports_no_progress() {
    let local = LocalDomain::new();

    let stats = local.run_for(Duration::from_secs(1));
    assert_eq!(stats.drive_steps, 0);
    assert_eq!(stats.inbox_commands, 0);
    assert!(stats.elapsed < Duration::from_millis(100));
    local.shutdown_now();
}

#[test]
fn run_for_budget_is_soft_when_one_poll_runs_long() {
    let local = LocalDomain::new();
    local
        .spawn_local(async { std::thread::sleep(Duration::from_millis(5)) })
        .unwrap()
        .detach();

    let stats = local.run_for(Duration::from_millis(1));
    assert_eq!(stats.drive_steps, 1);
    assert!(stats.elapsed >= Duration::from_millis(5));
    local.shutdown_now();
}

#[test]
fn dispatch_and_dispatch_future_run_on_the_owner_thread() {
    let local = LocalDomain::new();
    let spawner = local.spawner();
    let owner = std::thread::current().id();
    let (done_tx, done_rx) = mpsc::channel();

    std::thread::spawn(move || {
        let closure_tx = done_tx.clone();
        spawner
            .dispatch(move || closure_tx.send(std::thread::current().id()).unwrap())
            .unwrap();
        spawner
            .dispatch_future(async move {
                done_tx.send(std::thread::current().id()).unwrap();
            })
            .unwrap();
    })
    .join()
    .unwrap();

    let mut observed = Vec::new();
    for _ in 0..8 {
        observed.extend(done_rx.try_iter());
        if observed.len() == 2 {
            break;
        }
        assert_eq!(local.run_n(1), 1);
    }
    assert_eq!(observed, vec![owner, owner]);
    local.shutdown_now();
}

#[test]
fn fire_and_forget_dispatch_isolates_panics_and_rejects_after_shutdown() {
    let local = LocalDomain::new();
    let spawner = local.spawner();
    let (done_tx, done_rx) = mpsc::channel();

    spawner
        .dispatch(|| panic!("fire-and-forget panic"))
        .unwrap();
    spawner.dispatch(move || done_tx.send(()).unwrap()).unwrap();

    assert_eq!(local.run_n(1), 1);
    assert_eq!(local.run_n(1), 1);
    done_rx
        .try_recv()
        .expect("a panic in one dispatched callback must not stop the domain");

    local.shutdown_now();
    assert!(matches!(
        spawner.dispatch(|| {}),
        Err(async_runtime::SpawnError::Closed)
    ));
    assert!(matches!(
        spawner.dispatch_future(async {}),
        Err(async_runtime::SpawnError::Closed)
    ));
}

#[test]
fn dispatch_preserves_inbox_order_and_graceful_shutdown_drains_acceptances() {
    let local = LocalDomain::new();
    let spawner = local.spawner();
    let (done_tx, done_rx) = mpsc::channel();

    for value in 0..1_000 {
        let done_tx = done_tx.clone();
        spawner
            .dispatch(move || done_tx.send(value).unwrap())
            .unwrap();
    }
    drop(done_tx);

    future::block_on(local.shutdown_graceful());
    assert_eq!(
        done_rx.into_iter().collect::<Vec<_>>(),
        (0..1_000).collect::<Vec<_>>()
    );
    assert!(matches!(
        spawner.dispatch(|| {}),
        Err(async_runtime::SpawnError::Closed)
    ));
}

#[test]
fn graceful_shutdown_executes_every_dispatch_accepted_during_multi_producer_race() {
    const PRODUCERS: usize = 4;
    const ATTEMPTS_PER_PRODUCER: usize = 64;

    let local = LocalDomain::new();
    let spawner = local.spawner();
    let start = Arc::new(Barrier::new(PRODUCERS + 1));
    let initial_ready = Arc::new(Barrier::new(PRODUCERS + 1));
    let race_close = Arc::new(Barrier::new(PRODUCERS + 1));
    let (outcome_tx, outcome_rx) = mpsc::channel();
    let (executed_tx, executed_rx) = mpsc::channel();
    let mut producers = Vec::new();

    for _ in 0..PRODUCERS {
        let spawner = spawner.clone();
        let start = Arc::clone(&start);
        let initial_ready = Arc::clone(&initial_ready);
        let race_close = Arc::clone(&race_close);
        let outcome_tx = outcome_tx.clone();
        let executed_tx = executed_tx.clone();
        producers.push(std::thread::spawn(move || {
            start.wait();
            let initial_tx = executed_tx.clone();
            outcome_tx
                .send(
                    spawner
                        .dispatch(move || initial_tx.send(()).unwrap())
                        .is_ok(),
                )
                .unwrap();
            initial_ready.wait();
            race_close.wait();

            for _ in 0..ATTEMPTS_PER_PRODUCER {
                let executed_tx = executed_tx.clone();
                outcome_tx
                    .send(
                        spawner
                            .dispatch(move || executed_tx.send(()).unwrap())
                            .is_ok(),
                    )
                    .unwrap();
            }
        }));
    }
    drop(outcome_tx);
    drop(executed_tx);

    start.wait();
    initial_ready.wait();
    race_close.wait();
    future::block_on(local.shutdown_graceful());

    for producer in producers {
        producer.join().unwrap();
    }
    let outcomes = outcome_rx.into_iter().collect::<Vec<_>>();
    assert_eq!(outcomes.len(), PRODUCERS * (ATTEMPTS_PER_PRODUCER + 1));
    let accepted = outcomes.into_iter().filter(|accepted| *accepted).count();
    assert!(accepted >= PRODUCERS, "initial submissions precede closing");
    assert_eq!(executed_rx.into_iter().count(), accepted);
    assert!(matches!(
        spawner.dispatch(|| {}),
        Err(async_runtime::SpawnError::Closed)
    ));
}

#[test]
fn shutdown_now_discards_accepted_dispatch_futures_during_multi_producer_race() {
    const PRODUCERS: usize = 4;
    const ATTEMPTS_PER_PRODUCER: usize = 64;

    let local = LocalDomain::new();
    let spawner = local.spawner();
    let (dropped_tx, dropped_rx) = mpsc::channel();
    spawner
        .dispatch_future(DropNotifies {
            dropped: Some(dropped_tx),
        })
        .unwrap();

    let start = Arc::new(Barrier::new(PRODUCERS + 1));
    let race_close = Arc::new(Barrier::new(PRODUCERS + 1));
    let (outcome_tx, outcome_rx) = mpsc::channel();
    let (executed_tx, executed_rx) = mpsc::channel();
    let mut producers = Vec::new();

    for _ in 0..PRODUCERS {
        let spawner = spawner.clone();
        let start = Arc::clone(&start);
        let race_close = Arc::clone(&race_close);
        let outcome_tx = outcome_tx.clone();
        let executed_tx = executed_tx.clone();
        producers.push(std::thread::spawn(move || {
            start.wait();
            race_close.wait();
            for _ in 0..ATTEMPTS_PER_PRODUCER {
                let executed_tx = executed_tx.clone();
                outcome_tx
                    .send(
                        spawner
                            .dispatch_future(async move {
                                executed_tx.send(()).unwrap();
                            })
                            .is_ok(),
                    )
                    .unwrap();
            }
        }));
    }
    drop(outcome_tx);
    drop(executed_tx);

    start.wait();
    race_close.wait();
    local.shutdown_now();

    for producer in producers {
        producer.join().unwrap();
    }
    assert_eq!(
        outcome_rx.into_iter().count(),
        PRODUCERS * ATTEMPTS_PER_PRODUCER
    );
    dropped_rx
        .try_recv()
        .expect("an accepted, unmaterialized dispatch future must be dropped by shutdown_now");
    assert!(executed_rx.try_recv().is_err());
    assert!(matches!(
        spawner.dispatch_future(async {}),
        Err(async_runtime::SpawnError::Closed)
    ));
}
