//! Regression coverage for service of global work while a worker keeps
//! replenishing its local high-priority queue.

use async_runtime::{Priority, Runtime, RuntimeBuilder};
use futures_lite::future;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(2);
// The scheduler checks its global high queue after at most 8 local high
// dequeues. Leave room for selector turns and platform scheduling noise
// while still making a loss of the local-burst escape hatch visible.
const MAX_POLLS_BEFORE_SERVICE: usize = 192;

struct LocalHighLoad {
    runtime: Runtime,
    stop: Arc<AtomicBool>,
    polls: Arc<AtomicUsize>,
    spinner: async_runtime::Task<()>,
}

impl LocalHighLoad {
    fn start() -> Self {
        let runtime = RuntimeBuilder::new(NonZeroUsize::new(1).unwrap())
            .build()
            .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let polls = Arc::new(AtomicUsize::new(0));
        let spinner = runtime
            .spawn(Priority::High, {
                let stop = Arc::clone(&stop);
                let polls = Arc::clone(&polls);
                async move {
                    while !stop.load(Ordering::Acquire) {
                        polls.fetch_add(1, Ordering::Relaxed);
                        // Once initially run from the injector, this wake is
                        // scheduled by the worker and therefore stays local.
                        future::yield_now().await;
                    }
                }
            })
            .unwrap();

        let deadline = Instant::now() + TIMEOUT;
        while polls.load(Ordering::Acquire) < 128 {
            assert!(
                Instant::now() < deadline,
                "local high-priority load did not start"
            );
            std::thread::yield_now();
        }
        Self {
            runtime,
            stop,
            polls,
            spinner,
        }
    }

    fn finish(self) {
        self.stop.store(true, Ordering::Release);
        future::block_on(self.spinner);
        self.runtime.shutdown_graceful().unwrap();
    }
}

#[test]
fn global_high_receives_bounded_service_amid_continuous_local_high_requeues() {
    let load = LocalHighLoad::start();
    let (served_tx, served_rx) = mpsc::channel();
    let polls = Arc::clone(&load.polls);

    // This submission originates off-worker, so it must escape the local
    // high queue and receive service through the high-priority injector.
    load.runtime
        .spawn(Priority::High, async move {
            served_tx.send(polls.load(Ordering::Acquire)).unwrap();
        })
        .unwrap()
        .detach();
    // Sampling after spawn avoids charging time when this submitting OS thread
    // was descheduled before the runnable actually reached the injector. If
    // the task already ran, saturating_sub below correctly records zero wait.
    let submitted_at = load.polls.load(Ordering::Acquire);

    let served_at = served_rx
        .recv_timeout(TIMEOUT)
        .expect("global High task was starved by local High requeues");
    assert!(
        served_at.saturating_sub(submitted_at) <= MAX_POLLS_BEFORE_SERVICE,
        "global High task waited for too many local High polls: {}",
        served_at.saturating_sub(submitted_at)
    );
    load.finish();
}

#[test]
fn background_makes_progress_amid_continuous_local_high_requeues() {
    let load = LocalHighLoad::start();
    let (served_tx, served_rx) = mpsc::channel();
    let polls = Arc::clone(&load.polls);

    load.runtime
        .spawn(Priority::Background, async move {
            served_tx.send(polls.load(Ordering::Acquire)).unwrap();
        })
        .unwrap()
        .detach();
    let submitted_at = load.polls.load(Ordering::Acquire);

    let served_at = served_rx
        .recv_timeout(TIMEOUT)
        .expect("Background task made no progress under local High load");
    assert!(
        served_at.saturating_sub(submitted_at) <= MAX_POLLS_BEFORE_SERVICE,
        "Background task waited for too many local High polls: {}",
        served_at.saturating_sub(submitted_at)
    );
    load.finish();
}
