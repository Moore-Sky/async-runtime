//! Compose movable general work with a thread-affine local domain.
//!
//! Run with `cargo run --example 05_domain_composition`.
//! The main thread owns and drives `LocalDomain`; only `Send` futures cross its
//! inbox. Do not block that owner while general work is waiting on local work.

use async_runtime::{LocalDomain, Priority, RuntimeBuilder};
use std::num::NonZeroUsize;
use std::sync::mpsc;
use std::thread;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(2).expect("non-zero")).build()?;
    let general = runtime.spawner();
    let local = LocalDomain::new();
    let local_sender = local.spawner();
    let owner = thread::current().id();
    let (done_tx, done_rx) = mpsc::channel::<(&'static str, thread::ThreadId)>();

    // general -> general: a worker submits and awaits another movable task.
    runtime
        .spawn(Priority::Normal, {
            let general = general.clone();
            let done_tx = done_tx.clone();
            async move {
                let value = general
                    .spawn(Priority::High, async { 21_u8 })
                    .unwrap()
                    .await;
                assert_eq!(value * 2, 42);
                done_tx
                    .send(("general -> general", thread::current().id()))
                    .unwrap();
            }
        })?
        .detach();

    // general -> local: the bridge task completes only while the owner drives.
    runtime
        .spawn(Priority::Normal, {
            let local_sender = local_sender.clone();
            let done_tx = done_tx.clone();
            async move {
                let local_thread = local_sender
                    .spawn(async { thread::current().id() })
                    .unwrap()
                    .await;
                done_tx.send(("general -> local", local_thread)).unwrap();
            }
        })?
        .detach();

    // local -> general: awaiting general work does not stop the host loop.
    local
        .spawn_local({
            let general = general.clone();
            let done_tx = done_tx.clone();
            async move {
                let value = general
                    .spawn(Priority::Normal, async { 40_u8 })
                    .unwrap()
                    .await;
                assert_eq!(value + 2, 42);
                done_tx
                    .send(("local -> general", thread::current().id()))
                    .unwrap();
            }
        })?
        .detach();

    // local -> local: create the child on the owner, then let another local
    // task await its handle. Neither task is ever sent across a thread.
    let child = local.spawn_local(async { 42_u8 })?;
    local
        .spawn_local({
            let done_tx = done_tx.clone();
            async move {
                assert_eq!(child.await, 42);
                done_tx
                    .send(("local -> local", thread::current().id()))
                    .unwrap();
            }
        })?
        .detach();
    drop(done_tx);

    let mut completed = Vec::new();
    while completed.len() < 4 {
        // `run_n` is non-blocking. Repeated host iterations keep both bridge
        // directions live and avoid a circular wait between the two domains.
        local.run_n(32);
        completed.extend(done_rx.try_iter());
        thread::yield_now();
    }
    for (path, executed_on) in completed {
        println!("{path} completed");
        if path.contains("local") {
            assert_eq!(
                executed_on, owner,
                "{path} must execute local code on owner"
            );
        }
    }

    futures_lite::future::block_on(local.shutdown_graceful());
    runtime.shutdown_graceful()?;
    Ok(())
}
