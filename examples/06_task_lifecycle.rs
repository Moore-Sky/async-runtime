//! Task handles: await, cancellation, panics, completion, and detaching.
//!
//! Run with `cargo run --example 06_task_lifecycle`.
//! A dropped handle cancels its task; detach only work the runtime should own
//! until shutdown. `fallible()` changes cancellation to `None`, not panics.

use async_runtime::{Priority, RuntimeBuilder};
use futures_lite::future;
use std::num::NonZeroUsize;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).expect("non-zero")).build()?;

    let pending = runtime.spawn(Priority::Normal, std::future::pending::<u8>())?;
    assert!(!pending.is_finished());
    assert_eq!(future::block_on(pending.cancel()), None);
    println!("explicit cancellation returns None");

    let completed = runtime.spawn(Priority::Normal, async { 42_u8 })?;
    assert_eq!(future::block_on(completed.fallible()), Some(42));
    println!("fallible handles successful completion as Some(value)");

    let panicking = runtime.spawn(Priority::Normal, async { panic!("example task panic") })?;
    let panic = catch_unwind(AssertUnwindSafe(|| future::block_on(panicking.fallible())));
    assert!(panic.is_err(), "fallible does not hide task panics");
    let healthy = runtime.spawn(Priority::Normal, async { "worker survived" })?;
    assert_eq!(future::block_on(healthy), "worker survived");
    println!("task panic reaches its awaiter; the worker remains usable");

    let (done_tx, done_rx) = mpsc::channel();
    runtime
        .spawn(Priority::Background, async move {
            done_tx.send("detached work").unwrap()
        })?
        .detach();
    runtime.shutdown_graceful()?;
    assert_eq!(
        done_rx
            .recv()
            .expect("graceful shutdown drains detached task"),
        "detached work"
    );
    println!("graceful shutdown drained detached work");
    Ok(())
}
