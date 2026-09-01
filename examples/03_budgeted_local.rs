//! A host-driven local domain for thread-affine work.
//!
//! Run with `cargo run --example 03_budgeted_local`.
//! This is the shape to use in a render, UI, game, or other host-owned loop:
//! give the domain a small amount of work each frame and inspect the work that
//! was actually performed. `run_for` uses a soft deadline: one user future
//! poll is never forcibly interrupted.

use async_runtime::LocalDomain;
use futures_lite::future;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

fn main() {
    // Create this on the same thread that will drive it.
    let local_rt = LocalDomain::new();
    let state = Rc::new(Cell::new(0));
    let state_for_task = Rc::clone(&state);

    local_rt
        .spawn_local(async move { state_for_task.set(1) })
        .expect("domain is running")
        .detach();

    // A render loop can cap the number of scheduling opportunities per frame.
    // `run_n(0)` is a no-op, which is useful when the host has no spare budget.
    assert_eq!(local_rt.run_n(0), 0);
    let steps = local_rt.run_n(1);
    assert_eq!(steps, 1);
    assert_eq!(state.get(), 1);

    // A time-budgeted host loop gets a little more detail. In a real host this
    // would be called once per frame, for example with a 1-2 ms budget.
    let state_for_task = Rc::clone(&state);
    local_rt
        .spawn_local(async move { state_for_task.set(2) })
        .expect("domain is running")
        .detach();
    let stats = local_rt.run_for(Duration::from_millis(1));
    println!(
        "host slice: {} drive steps, {} inbox commands, elapsed {:?}",
        stats.drive_steps, stats.inbox_commands, stats.elapsed
    );
    assert!(stats.drive_steps >= 1);
    assert_eq!(state.get(), 2);

    // A zero-duration slice never starts another user poll.
    let zero_budget = local_rt.run_for(Duration::ZERO);
    assert_eq!(zero_budget.drive_steps, 0);

    future::block_on(local_rt.shutdown_graceful());
}
