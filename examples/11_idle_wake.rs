//! Wake an idle worker by submitting external work.
//!
//! Run with `cargo run --example 11_idle_wake`.
//! The runtime parks idle workers instead of spinning. An external `Spawner`
//! submission places a runnable in a global injector and wakes one worker.

use async_runtime::{Priority, RuntimeBuilder};
use futures_lite::future;
use std::num::NonZeroUsize;
use std::thread;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).expect("non-zero")).build()?;
    let producer = runtime.spawner();

    // Give the worker an opportunity to enter its idle park path. This is a
    // demonstration, not a latency benchmark; use `cargo bench` for data.
    thread::sleep(Duration::from_millis(20));
    let submitted = Instant::now();
    let task = producer.spawn(Priority::High, async move { Instant::now() })?;
    let started = future::block_on(task);
    println!(
        "idle-worker wake latency: {:?}",
        started.duration_since(submitted)
    );

    runtime.shutdown_graceful()?;
    Ok(())
}
