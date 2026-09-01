//! Configure a different three-priority scheduling ratio.
//!
//! Run with `cargo run --example 12_custom_priority_weights`.
//! Weights are scheduling opportunities per worker, not completion ratios or a
//! global ordering contract. Keep all three values non-zero to retain eventual
//! progress for every priority.

use async_runtime::{Priority, PriorityWeights, RuntimeBuilder};
use std::num::NonZeroUsize;
use std::sync::mpsc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let weights = PriorityWeights::new(
        NonZeroUsize::new(5).unwrap(),
        NonZeroUsize::new(2).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(2).expect("non-zero"))
        .priority_weights(weights)
        .build()?;

    let (tx, rx) = mpsc::channel();
    for priority in [Priority::High, Priority::Normal, Priority::Background] {
        let tx = tx.clone();
        runtime
            .spawn(priority, async move { tx.send(priority).unwrap() })?
            .detach();
    }
    drop(tx);

    let completed: Vec<_> = rx.iter().collect();
    assert_eq!(completed.len(), 3);
    println!("configured High:Normal:Background = 5:2:1; all priorities completed");

    runtime.shutdown_graceful()?;
    Ok(())
}
