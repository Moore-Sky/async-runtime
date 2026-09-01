//! Graceful, timed, and immediate shutdown paths.
//!
//! Run with `cargo run --example 07_shutdown`.
//! Use graceful shutdown when accepted work must finish, a timeout when the
//! caller has a deadline, and immediate shutdown when remaining work must be
//! cancelled.

use async_runtime::{LocalDomain, Priority, RuntimeBuilder, ShutdownOutcome, SpawnError};
use futures_lite::future;
use std::num::NonZeroUsize;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let domain = LocalDomain::new();
    let spawner = domain.spawner();
    let completed = Arc::new(AtomicUsize::new(0));
    let completed_by_callback = Arc::clone(&completed);

    // Work accepted before shutdown is drained by graceful shutdown, even if
    // it has not yet been materialized from the cross-thread inbox.
    spawner
        .dispatch(move || {
            completed_by_callback.fetch_add(1, Ordering::SeqCst);
        })
        .expect("domain is running");
    future::block_on(domain.shutdown_graceful());
    assert_eq!(completed.load(Ordering::SeqCst), 1);

    // The capability remains usable as a value, but the closed domain rejects
    // every late fire-and-forget submission deterministically.
    let late = spawner.dispatch(|| unreachable!("closed work must not run"));
    assert_eq!(late, Err(SpawnError::Closed));
    println!("graceful shutdown drained accepted work and rejected late work");

    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).unwrap()).build()?;
    runtime
        .spawn(Priority::Normal, std::future::pending::<()>())?
        .detach();
    let outcome = runtime.shutdown_timeout(Duration::from_millis(10))?;
    assert!(matches!(outcome, ShutdownOutcome::TimedOut { .. }));
    println!("timed shutdown cancelled work that missed its deadline");

    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).unwrap()).build()?;
    runtime
        .spawn(Priority::Background, std::future::pending::<()>())?
        .detach();
    runtime.shutdown_now()?;
    println!("shutdown_now cancelled remaining work immediately");
    Ok(())
}
