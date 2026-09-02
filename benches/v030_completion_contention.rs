//! Run with `cargo bench --bench v030_completion_contention`.
//! Measures an end-to-end batch whose pre-registered tasks are released
//! together, across worker counts. Submission and registration remain inside
//! each measured iteration, so changes must not be attributed solely to the
//! completion/drain path.

use async_runtime::{Priority, RuntimeBuilder};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures_lite::future;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

const WORKERS: [usize; 4] = [1, 2, 4, 8];
const TASKS: usize = 1_024;

struct Gate {
    open: AtomicBool,
    wakers: Mutex<Vec<Waker>>,
    registered: Mutex<mpsc::Sender<()>>,
}
struct WaitGate(Arc<Gate>);

impl Gate {
    fn open(&self) {
        self.open.store(true, Ordering::Release);
        for waker in std::mem::take(&mut *self.wakers.lock().expect("waker lock")) {
            waker.wake();
        }
    }
}
impl Future for WaitGate {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0.open.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        self.0
            .wakers
            .lock()
            .expect("waker lock")
            .push(cx.waker().clone());
        if self.0.open.load(Ordering::Acquire) {
            Poll::Ready(())
        } else {
            self.0
                .registered
                .lock()
                .expect("registration lock")
                .send(())
                .expect("owner alive");
            Poll::Pending
        }
    }
}

fn completion_contention(c: &mut Criterion) {
    for workers in WORKERS {
        let runtime = RuntimeBuilder::new(NonZeroUsize::new(workers).unwrap())
            .build()
            .expect("runtime");
        let mut group = c.benchmark_group("v030/concentrated-completion");
        group.measurement_time(Duration::from_secs(8));
        group.throughput(Throughput::Elements(TASKS as u64));
        group.bench_with_input(BenchmarkId::from_parameter(workers), &workers, |b, _| {
            b.iter(|| {
                let (sender, receiver) = mpsc::channel();
                let gate = Arc::new(Gate {
                    open: AtomicBool::new(false),
                    wakers: Mutex::new(Vec::with_capacity(TASKS)),
                    registered: Mutex::new(sender),
                });
                let tasks = (0..TASKS)
                    .map(|_| {
                        runtime
                            .spawn(Priority::Normal, WaitGate(Arc::clone(&gate)))
                            .expect("spawn")
                    })
                    .collect::<Vec<_>>();
                for _ in 0..TASKS {
                    receiver.recv().expect("task registers");
                }
                gate.open();
                for task in tasks {
                    future::block_on(task);
                }
            });
        });
        group.finish();
        runtime.shutdown_graceful().expect("shutdown");
    }
}

criterion_group!(benches, completion_contention);
criterion_main!(benches);
