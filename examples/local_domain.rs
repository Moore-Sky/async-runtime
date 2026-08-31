//! A host-driven local domain for thread-affine work.

use async_runtime::LocalDomain;
use std::cell::Cell;
use std::rc::Rc;

fn main() {
    // Create this on the same thread that will drive it.
    let local_rt = LocalDomain::new();
    let state = Rc::new(Cell::new(0));
    let state_for_task = Rc::clone(&state);

    local_rt
        .spawn_local(async move { state_for_task.set(1) })
        .expect("domain is running")
        .detach();

    // A render loop would normally call this each frame.
    while local_rt.try_tick() {}
    assert_eq!(state.get(), 1);

    async_io::block_on(local_rt.shutdown_graceful());
}
