//! Nested general-runtime work and worker locality.
//!
//! Run with `cargo run --example 08_nested_worker_locality`.
//! Children spawned by a general-runtime worker are first placed in that
//! worker's local queue. This improves cache locality, but it is not thread
//! affinity: an idle worker may steal a child to balance the pool.

use async_runtime::{Priority, RuntimeBuilder};
use futures_lite::future;
use std::num::NonZeroUsize;
use std::thread;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(2).expect("non-zero")).build()?;
    let parent = runtime.spawn(Priority::Normal, {
        let nested = runtime.spawner();
        async move {
            let parent_worker = thread::current().id();
            let mut children = Vec::new();

            for _ in 0..16 {
                children.push(
                    nested
                        .spawn(Priority::Normal, async { thread::current().id() })
                        .expect("runtime is still running"),
                );
            }

            let mut ran_on_parent = 0;
            for child in children {
                if child.await == parent_worker {
                    ran_on_parent += 1;
                }
            }
            (parent_worker, ran_on_parent)
        }
    })?;

    let (parent_worker, ran_on_parent) = future::block_on(parent);
    println!("parent ran on {parent_worker:?}; {ran_on_parent}/16 children also ran there");
    println!("local enqueue is preferred; cross-worker stealing remains allowed");
    runtime.shutdown_graceful()?;
    Ok(())
}
