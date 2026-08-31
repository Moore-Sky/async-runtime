use async_runtime::{Priority, RuntimeBuilder};
use std::num::NonZeroUsize;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn lower_priorities_are_not_starved_by_continuous_high_work() {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).unwrap())
        .build()
        .unwrap();
    let spawner = runtime.spawner();
    let (tx, rx) = mpsc::channel();

    // Keep the high queue supplied without asserting any global 8:4:1 completion
    // ratio: workers have independent selectors and task durations are variable.
    for _ in 0..200 {
        spawner
            .spawn(Priority::High, async { std::thread::yield_now() })
            .unwrap()
            .detach();
    }
    for priority in [Priority::Normal, Priority::Background] {
        let tx = tx.clone();
        spawner
            .spawn(priority, async move { tx.send(priority).unwrap() })
            .unwrap()
            .detach();
    }
    drop(tx);

    let first = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let second = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_ne!(first, second);
    runtime.shutdown_graceful().unwrap();
}

// Exact 8:4:1 verification belongs to the pure, per-worker selector unit test,
// because public task completion order is intentionally not a scheduling contract.
// The implementation should expose that selector to `src/worker.rs` unit tests and
// assert: H H H H H H H H N N N N B when all queues are non-empty.
