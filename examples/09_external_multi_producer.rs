//! Submit work from several external producer threads.
//!
//! Run with `cargo run --example 09_external_multi_producer`.
//! Clone `Spawner` for producers that do not own the runtime. Their work enters
//! the global priority injectors and wakes workers as necessary.

use async_runtime::{Priority, RuntimeBuilder};
use std::num::NonZeroUsize;
use std::sync::{mpsc, Arc, Barrier};
use std::thread;

const PRODUCERS: usize = 4;
const TASKS_PER_PRODUCER: usize = 25;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(4).expect("non-zero")).build()?;
    let start = Arc::new(Barrier::new(PRODUCERS));
    let (done_tx, done_rx) = mpsc::channel();
    let mut producers = Vec::new();

    for producer in 0..PRODUCERS {
        let spawner = runtime.spawner();
        let start = Arc::clone(&start);
        let done_tx = done_tx.clone();
        producers.push(thread::spawn(move || {
            start.wait();
            for task in 0..TASKS_PER_PRODUCER {
                let done_tx = done_tx.clone();
                spawner
                    .spawn(Priority::Normal, async move {
                        done_tx
                            .send((producer, task, thread::current().id()))
                            .unwrap();
                    })
                    .expect("runtime is running")
                    .detach();
            }
        }));
    }
    drop(done_tx);

    for producer in producers {
        producer.join().expect("producer did not panic");
    }
    let completed: Vec<_> = done_rx.iter().collect();
    assert_eq!(completed.len(), PRODUCERS * TASKS_PER_PRODUCER);
    println!("{} externally submitted tasks completed", completed.len());

    runtime.shutdown_graceful()?;
    Ok(())
}
