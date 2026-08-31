//! General runtime priorities.

use async_runtime::{Priority, RuntimeBuilder};
use std::num::NonZeroUsize;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(4).expect("non-zero")).build()?;

    runtime
        .spawn(Priority::High, async { println!("latency-sensitive work") })?
        .detach();
    runtime
        .spawn(Priority::Normal, async { println!("ordinary work") })?
        .detach();
    runtime
        .spawn(Priority::Background, async { println!("maintenance work") })?
        .detach();

    runtime.shutdown_graceful()?;
    Ok(())
}
