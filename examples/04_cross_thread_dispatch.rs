//! Submit fire-and-forget work from another thread to a `LocalDomain`.
//!
//! Run with `cargo run --example 04_cross_thread_dispatch`.
//! Both messages below are submitted by a background thread but execute on the
//! domain owner thread. The owner must keep driving its domain.

use async_runtime::LocalDomain;
use std::sync::mpsc;
use std::thread;

fn main() {
    let domain = LocalDomain::new();
    let owner_thread = thread::current().id();
    let spawner = domain.spawner();
    let (completed_tx, completed_rx) = mpsc::channel();

    let submitter = thread::spawn(move || {
        let callback_tx = completed_tx.clone();
        spawner
            .dispatch(move || {
                callback_tx
                    .send(("closure", thread::current().id()))
                    .unwrap()
            })
            .expect("domain is still accepting work");

        spawner
            .dispatch_future(async move {
                completed_tx
                    .send(("future", thread::current().id()))
                    .unwrap();
            })
            .expect("domain is still accepting work");
    });
    submitter.join().expect("submitter thread did not panic");

    // The owner must drive the domain. Use a work-counted loop here instead of
    // assuming a wall-clock slice will always materialize both commands.
    while domain.run_n(1) != 0 {}

    let mut completed = Vec::new();
    while completed.len() < 2 {
        domain.run_n(1);
        completed.extend(completed_rx.try_iter());
    }
    for (kind, actual_thread) in completed {
        println!("{kind} executed on the LocalDomain owner thread");
        assert_eq!(actual_thread, owner_thread);
    }

    futures_lite::future::block_on(domain.shutdown_graceful());
}
