//! Return CPU-work results to a thread-affine host on its next frame.
//!
//! Run with `cargo run --example 15_cpu_result_to_local_frame`.
//! A general-runtime worker computes a `Send` result, then uses
//! [`LocalSpawner::dispatch`](async_runtime::LocalSpawner::dispatch) to put it
//! into a `Send` mailbox. The owner drains that mailbox after its bounded local
//! slice and applies the results to `!Send` render state.

use async_runtime::{LocalDomain, Priority, RuntimeBuilder, SpawnError};
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Debug)]
struct CpuResult {
    job: usize,
    checksum: u64,
}

#[derive(Default)]
struct RenderState {
    completed_jobs: Vec<usize>,
    last_checksum: u64,
}

/// A deliberately synchronous CPU job. It runs in one poll, so it is suitable
/// for modest, bounded jobs; long jobs should be split into yielding chunks.
fn build_mesh_checksum(job: usize) -> CpuResult {
    let mut checksum = job as u64 + 1;
    for index in 0..200_000_u64 {
        checksum = checksum
            .wrapping_mul(6364136223846793005)
            .wrapping_add(index ^ job as u64);
        std::hint::black_box(checksum);
    }
    CpuResult { job, checksum }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    const JOBS: usize = 3;

    let runtime = RuntimeBuilder::new(NonZeroUsize::new(2).expect("non-zero workers")).build()?;
    let local = LocalDomain::new();
    let local_sender = local.spawner();

    // This is only a cross-thread result handoff. The actual render state stays
    // `Rc<RefCell<_>>` on this owner thread and never enters `dispatch`.
    let mailbox = Arc::new(Mutex::new(Vec::<CpuResult>::new()));
    let render_state = Rc::new(RefCell::new(RenderState::default()));

    for job in 0..JOBS {
        let mailbox = Arc::clone(&mailbox);
        let local_sender = local_sender.clone();
        runtime
            .spawn(Priority::Normal, async move {
                let result = build_mesh_checksum(job);

                // `dispatch` requires its closure to be `Send`, so it cannot
                // capture the owner's `Rc<RefCell<RenderState>>`. It does run
                // this mailbox push on the LocalDomain owner thread.
                if let Err(SpawnError::Closed) = local_sender.dispatch(move || {
                    mailbox
                        .lock()
                        .expect("result mailbox poisoned")
                        .push(result);
                }) {
                    // A host may have started local shutdown before a late CPU
                    // result arrives. Dropping that result is intentional.
                    eprintln!("local domain closed; dropped CPU result for job {job}");
                }
            })?
            .detach();
    }

    // This is the render/UI loop. It never blocks waiting for workers: each
    // frame gives local work a bounded opportunity, then applies completed
    // results before the next frame's rendering.
    while render_state.borrow().completed_jobs.len() < JOBS {
        let _stats = local.run_for(Duration::from_micros(500));
        let completed = std::mem::take(&mut *mailbox.lock().expect("result mailbox poisoned"));
        if !completed.is_empty() {
            let mut state = render_state.borrow_mut();
            for result in completed {
                state.completed_jobs.push(result.job);
                state.last_checksum = result.checksum;
            }
            println!(
                "next frame applies {} completed CPU job(s)",
                state.completed_jobs.len()
            );
        }
        thread::yield_now();
    }

    // Close producers first: graceful Runtime shutdown guarantees no worker can
    // enqueue another local command. Then drain and close the owner domain.
    runtime.shutdown_graceful()?;
    futures_lite::future::block_on(local.shutdown_graceful());
    Ok(())
}
