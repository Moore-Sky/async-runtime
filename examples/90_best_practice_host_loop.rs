//! A practical host-loop pattern for UI, render, and game threads.
//!
//! Run with `cargo run --example 90_best_practice_host_loop`.
//! Keep the `LocalDomain` on its owner thread, submit cross-thread input using
//! its spawner, and give it a bounded slice once per host frame.

use async_runtime::LocalDomain;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

fn main() {
    let local = LocalDomain::new();
    let submitted = Arc::new(AtomicUsize::new(0));
    let observed = Rc::new(Cell::new(0));
    let remote = local.spawner();
    let submitted_by_worker = Arc::clone(&submitted);

    thread::spawn(move || {
        remote
            .dispatch(move || {
                submitted_by_worker.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
    })
    .join()
    .unwrap();

    for frame in 0..3 {
        let observed_by_task = Rc::clone(&observed);
        local
            .spawn_local(async move { observed_by_task.set(frame + 1) })
            .unwrap()
            .detach();
        let stats = local.run_for(Duration::from_millis(1));
        println!(
            "frame {frame}: drove {} steps and {} inbox command(s)",
            stats.drive_steps, stats.inbox_commands
        );
    }

    assert_eq!(submitted.load(Ordering::SeqCst), 1);
    assert_eq!(observed.get(), 3);
    futures_lite::future::block_on(local.shutdown_graceful());
}
