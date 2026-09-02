//! Manual/nightly stress coverage for concurrent submission and forced shutdown.

use async_runtime::{Priority, RuntimeBuilder, SpawnError};
use futures_lite::future;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

const PRODUCERS: usize = 16;
const RUN_FOR: Duration = Duration::from_secs(10);

#[test]
#[ignore = "manual/nightly 10-second shutdown-race stress test"]
fn mixed_producers_cancel_and_shutdown_race() {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(8).unwrap())
        .build()
        .unwrap();
    let spawner = runtime.spawner();
    let start = Arc::new(Barrier::new(PRODUCERS + 1));
    let keep_producing = Arc::new(AtomicBool::new(true));
    let accepted = Arc::new(AtomicUsize::new(0));
    let nested = Arc::new(AtomicUsize::new(0));
    let cancelled = Arc::new(AtomicUsize::new(0));
    let mut producers = Vec::with_capacity(PRODUCERS);

    for producer in 0..PRODUCERS {
        let spawner = spawner.clone();
        let start = Arc::clone(&start);
        let keep_producing = Arc::clone(&keep_producing);
        let accepted = Arc::clone(&accepted);
        let nested = Arc::clone(&nested);
        let cancelled = Arc::clone(&cancelled);
        producers.push(std::thread::spawn(move || {
            start.wait();
            let mut sequence = producer;
            while keep_producing.load(Ordering::Acquire) {
                let priority = match sequence % 3 {
                    0 => Priority::High,
                    1 => Priority::Normal,
                    _ => Priority::Background,
                };
                let result = match sequence % 4 {
                    // Detached yielding work exercises wake/requeue paths.
                    0 => spawner.spawn(priority, async {
                        future::yield_now().await;
                        future::yield_now().await;
                    }),
                    // Nested work is submitted from a worker, exercising local
                    // routing while external producers are also submitting.
                    1 => {
                        let inner_spawner = spawner.clone();
                        let nested = Arc::clone(&nested);
                        spawner.spawn(priority, async move {
                            if let Ok(task) = inner_spawner.spawn(Priority::Background, async {
                                future::yield_now().await;
                            }) {
                                nested.fetch_add(1, Ordering::Relaxed);
                                task.detach();
                            }
                            future::yield_now().await;
                        })
                    }
                    // Explicit cancellation races both an unstarted task and a
                    // task that has just requeued itself.
                    2 => match spawner.spawn(priority, async {
                        future::yield_now().await;
                        std::future::pending::<()>().await;
                    }) {
                        Ok(task) => {
                            accepted.fetch_add(1, Ordering::Relaxed);
                            let _ = future::block_on(task.cancel());
                            cancelled.fetch_add(1, Ordering::Relaxed);
                            sequence = sequence.wrapping_add(1);
                            continue;
                        }
                        Err(error) => Err(error),
                    },
                    _ => spawner.spawn(priority, async {}),
                };

                match result {
                    Ok(task) => {
                        accepted.fetch_add(1, Ordering::Relaxed);
                        task.detach();
                    }
                    Err(SpawnError::Closed) => break,
                }
                sequence = sequence.wrapping_add(1);
                std::thread::yield_now();
            }
        }));
    }

    start.wait();
    let deadline = Instant::now() + RUN_FOR;
    while Instant::now() < deadline {
        std::thread::yield_now();
    }

    // Do not first stop the producers: their current submissions must race
    // with the lifecycle transition.  They exit when `Closed` is observed.
    runtime.shutdown_now().unwrap();
    keep_producing.store(false, Ordering::Release);
    for producer in producers {
        producer.join().unwrap();
    }

    assert!(accepted.load(Ordering::Relaxed) > 0, "no work was accepted");
    assert!(
        cancelled.load(Ordering::Relaxed) > 0,
        "cancellation path was not exercised"
    );
    assert!(
        nested.load(Ordering::Relaxed) > 0,
        "nested spawn path was not exercised"
    );
}
