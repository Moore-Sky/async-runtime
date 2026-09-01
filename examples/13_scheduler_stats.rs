//! Inspect approximate scheduler counters (requires the `stats` feature).
//!
//! Run with `cargo run --example 13_scheduler_stats --features stats`.
//! Counters are point-in-time operational observations, not synchronization or
//! task-completion guarantees. Keep this feature disabled in a minimal build.

#[cfg(feature = "stats")]
use async_runtime::{Priority, RuntimeBuilder};
#[cfg(feature = "stats")]
use futures_lite::future;
#[cfg(feature = "stats")]
use std::num::NonZeroUsize;

#[cfg(feature = "stats")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(2).expect("non-zero")).build()?;
    let task = runtime.spawn(Priority::High, async {
        future::yield_now().await;
        "high-priority task"
    })?;

    let before = runtime.stats();
    println!("before await: {before:#?}");
    assert_eq!(future::block_on(task), "high-priority task");
    let after = runtime.stats();
    println!("after await:  {after:#?}");
    println!(
        "executed={}, local_spawned={}, external_spawned={}, stolen={}",
        after.executed, after.local_spawned, after.external_spawned, after.stolen
    );

    runtime.shutdown_graceful()?;
    Ok(())
}

#[cfg(not(feature = "stats"))]
fn main() {
    eprintln!(
        "enable scheduler counters with: cargo run --example 13_scheduler_stats --features stats"
    );
}
