use async_runtime::{Priority, RuntimeBuilder};
use futures_lite::future;
use std::num::NonZeroUsize;
use std::sync::mpsc;
use std::time::Duration;

fn runtime() -> async_runtime::Runtime {
    RuntimeBuilder::new(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap()
}

#[test]
fn await_returns_value() {
    let runtime = runtime();
    let task = runtime.spawn(Priority::Normal, async { 7 }).unwrap();
    assert_eq!(future::block_on(task), 7);
    runtime.shutdown_graceful().unwrap();
}

#[test]
fn is_finished_is_false_for_pending_task() {
    let runtime = runtime();
    let task = runtime
        .spawn(Priority::Normal, async {
            std::future::pending::<()>().await;
        })
        .unwrap();
    assert!(!task.is_finished());
    assert_eq!(future::block_on(task.cancel()), None);
    runtime.shutdown_graceful().unwrap();
}

#[test]
fn dropping_handle_cancels_pending_task() {
    let runtime = runtime();
    let (tx, rx) = mpsc::channel();
    let task = runtime
        .spawn(Priority::Normal, async move {
            std::future::pending::<()>().await;
            tx.send(()).unwrap();
        })
        .unwrap();
    drop(task);
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    runtime.shutdown_graceful().unwrap();
}

#[test]
fn detach_keeps_task_running() {
    let runtime = runtime();
    let (tx, rx) = mpsc::channel();
    runtime
        .spawn(Priority::Normal, async move { tx.send(42).unwrap() })
        .unwrap()
        .detach();
    assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap(), 42);
    runtime.shutdown_graceful().unwrap();
}

#[test]
fn explicit_cancel_returns_none_for_pending_task() {
    let runtime = runtime();
    let task = runtime
        .spawn(Priority::Normal, async {
            std::future::pending::<u8>().await
        })
        .unwrap();
    assert_eq!(future::block_on(task.cancel()), None);
    runtime.shutdown_graceful().unwrap();
}

#[test]
fn fallible_returns_some_for_completed_task() {
    let runtime = runtime();
    let task = runtime.spawn(Priority::Normal, async { 42_u8 }).unwrap();
    assert_eq!(future::block_on(task.fallible()), Some(42));
    runtime.shutdown_graceful().unwrap();
}

#[test]
fn ordinary_await_panics_after_runtime_cancellation() {
    let runtime = runtime();
    let task = runtime
        .spawn(Priority::Normal, std::future::pending::<u8>())
        .unwrap();
    runtime.shutdown_now().unwrap();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| future::block_on(task)));
    assert!(result.is_err());
}

#[test]
fn panic_reaches_awaiter_but_worker_survives() {
    let runtime = runtime();
    let panicking = runtime
        .spawn(Priority::Normal, async { panic!("expected task panic") })
        .unwrap();
    let panic =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| future::block_on(panicking)));
    assert!(panic.is_err());

    let healthy = runtime.spawn(Priority::Normal, async { 42 }).unwrap();
    assert_eq!(future::block_on(healthy), 42);
    runtime.shutdown_graceful().unwrap();
}
