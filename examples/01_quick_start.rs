//! The smallest complete general-runtime program.
//!
//! Run with `cargo run --example 01_quick_start`.
//! Use `Runtime` for `Send + 'static` work that may run on a worker thread.

use async_runtime::{Priority, RuntimeBuilder};
use futures_lite::future;
use std::num::NonZeroUsize;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).expect("non-zero")).build()?;
    let task = runtime.spawn(Priority::High, async { "Hello from high priority work" })?;

    println!("{}", future::block_on(task));
    runtime.shutdown_graceful()?;
    Ok(())
}
