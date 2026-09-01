//! Run with `cargo bench --bench v030_yield_wake_storm`.
//! Contains separate cooperative-yield and externally-woken pending workloads.
//! The manual future uses a registration acknowledgement before producers wake it,
//! avoiding a lost-wake race without exposing scheduler internals.

use async_runtime::{Priority, RuntimeBuilder};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures_lite::future;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

const TASKS: [usize; 3] = [200, 1_000, 10_000];
const YIELDS: usize = 8;

struct ManualWake {
    ready: AtomicBool,
    waker: Mutex<Option<Waker>>,
    // `std::sync::mpsc::Sender` was not `Sync` on the crate's Rust 1.71
    // baseline, so protect the one-shot registration signal explicitly.
    registered: Mutex<mpsc::Sender<()>>,
}

struct WaitForWake(Arc<ManualWake>);

impl ManualWake {
    fn wake(&self) {
        self.ready.store(true, Ordering::Release);
        if let Some(waker) = self.waker.lock().expect("waker lock poisoned").take() {
            waker.wake();
        }
    }
}

impl Future for WaitForWake {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0.ready.swap(false, Ordering::AcqRel) {
            Poll::Ready(())
        } else {
            *self.0.waker.lock().expect("waker lock poisoned") = Some(cx.waker().clone());
            self.0
                .registered
                .lock()
                .expect("registration lock poisoned")
                .send(())
                .expect("benchmark owner is alive");
            Poll::Pending
        }
    }
}

fn yield_and_wake_storm(c: &mut Criterion) {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(4).expect("non-zero workers"))
        .build()
        .expect("runtime");

    let mut yields = c.benchmark_group("v030/general-yield-storm");
    for tasks in TASKS {
        yields.throughput(Throughput::Elements((tasks * YIELDS) as u64));
        yields.bench_with_input(BenchmarkId::from_parameter(tasks), &tasks, |b, &tasks| {
            b.iter(|| {
                let tasks = (0..tasks)
                    .map(|_| {
                        runtime
                            .spawn(Priority::Normal, async {
                                for _ in 0..YIELDS {
                                    future::yield_now().await;
                                }
                            })
                            .expect("spawn")
                    })
                    .collect::<Vec<_>>();
                for task in tasks {
                    future::block_on(task);
                }
            });
        });
    }
    yields.finish();

    let mut wakes = c.benchmark_group("v030/external-wake-storm");
    wakes.measurement_time(Duration::from_secs(8));
    for tasks in TASKS {
        wakes.throughput(Throughput::Elements(tasks as u64));
        wakes.bench_with_input(BenchmarkId::from_parameter(tasks), &tasks, |b, &tasks| {
            b.iter_custom(|iterations| {
                let mut elapsed = Duration::ZERO;
                for _ in 0..iterations {
                    let (registered_tx, registered_rx) = mpsc::channel();
                    let mut wakers = Vec::with_capacity(tasks);
                    let mut handles = Vec::with_capacity(tasks);
                    for _ in 0..tasks {
                        let wake = Arc::new(ManualWake {
                            ready: AtomicBool::new(false),
                            waker: Mutex::new(None),
                            registered: Mutex::new(registered_tx.clone()),
                        });
                        wakers.push(Arc::clone(&wake));
                        handles.push(
                            runtime
                                .spawn(Priority::Normal, WaitForWake(wake))
                                .expect("spawn"),
                        );
                    }
                    for _ in 0..tasks {
                        registered_rx.recv().expect("task must register its waker");
                    }
                    let started = Instant::now();
                    for wake in wakers {
                        wake.wake();
                    }
                    for handle in handles {
                        future::block_on(handle);
                    }
                    elapsed += started.elapsed();
                }
                elapsed
            });
        });
    }
    wakes.finish();
    runtime.shutdown_graceful().expect("shutdown");
}

criterion_group!(benches, yield_and_wake_storm);
criterion_main!(benches);
