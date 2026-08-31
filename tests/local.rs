use async_runtime::{LocalDomain, ShutdownOutcome};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;

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

    assert_eq!(async_io::block_on(local.run(task)), 42);
    assert_eq!(polls.get(), 1);
    async_io::block_on(local.shutdown_graceful());
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
    let executed_on = async_io::block_on(async {
        loop {
            match done_rx.try_recv() {
                Ok(thread_id) => break thread_id,
                Err(mpsc::TryRecvError::Empty) => local.tick().await,
                Err(mpsc::TryRecvError::Disconnected) => panic!("remote task did not run"),
            }
        }
    });
    assert_eq!(executed_on, std::thread::current().id());
    async_io::block_on(local.shutdown_graceful());
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
        async_io::block_on(local.run(task))
    }));
    let payload = result.expect_err("the original task panic must be rethrown");
    let message = payload.downcast_ref::<&str>().copied().unwrap_or_default();
    assert_eq!(message, "remote local panic payload");
    async_io::block_on(local.shutdown_graceful());
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
        async_io::block_on(local.run(task))
    }));
    assert!(result.is_err());
    async_io::block_on(local.shutdown_graceful());
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

    let observed = async_io::block_on(local.run(async { value.as_str() }));
    assert_eq!(observed, "borrowed");
    async_io::block_on(local.shutdown_graceful());
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

    async_io::block_on(local.shutdown_graceful());
    assert_eq!(done_rx.recv().unwrap(), 42);
}

#[test]
fn local_timeout_reports_and_cancels_remaining_work() {
    let local = LocalDomain::new();
    let task = local
        .spawn_local(std::future::pending::<u8>())
        .unwrap()
        .fallible();

    let outcome = async_io::block_on(local.shutdown_timeout(std::time::Duration::from_millis(20)));
    assert!(matches!(
        outcome,
        ShutdownOutcome::TimedOut { remaining_tasks: 1 }
    ));
    assert_eq!(async_io::block_on(task), None);
}

#[test]
fn dropped_remote_handle_is_cancelled_before_materialization() {
    let local = LocalDomain::new();
    let task = local.spawner().spawn(std::future::pending::<u8>()).unwrap();
    drop(task);

    assert_eq!(
        async_io::block_on(local.shutdown_timeout(std::time::Duration::from_secs(1),)),
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
    async_io::block_on(local.shutdown_graceful());
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
