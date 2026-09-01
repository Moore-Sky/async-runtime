//! Priority preference with eventual background progress.
//!
//! Run with `cargo run --example 10_priority_fairness`.
//! A priority weight controls opportunities, not a strict global ordering. Even
//! while high-priority work repeatedly yields, background work must eventually
//! receive an opportunity.

use async_runtime::{Priority, RuntimeBuilder};
use futures_lite::future;
use std::num::NonZeroUsize;
use std::sync::mpsc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A single worker makes the weighted scheduling policy easy to reason about.
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).expect("non-zero")).build()?;
    runtime
        .spawn(Priority::High, async {
            for _ in 0..10_000 {
                future::yield_now().await;
            }
        })?
        .detach();

    let (background_tx, background_rx) = mpsc::channel();
    let background = runtime.spawn(Priority::Background, async move {
        background_tx.send("background made progress").unwrap();
    })?;

    future::block_on(background);
    assert_eq!(background_rx.recv()?, "background made progress");
    println!("background ran while high-priority work remained runnable");

    runtime.shutdown_now()?;
    Ok(())
}
