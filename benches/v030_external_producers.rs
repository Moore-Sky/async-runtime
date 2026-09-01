//! Run with `cargo bench --bench v030_external_producers`.
//! Measures external producer contention on the general runtime's submission
//! path. Thread creation is deliberately outside the timed interval.

use async_runtime::{Priority, RuntimeBuilder, Spawner};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

const PRODUCERS: [usize; 5] = [1, 2, 4, 8, 16];
const TASKS_PER_PRODUCER: usize = 256;

struct Producers {
    requests: Vec<Sender<Option<Arc<AtomicUsize>>>>,
    ready: Receiver<()>,
    threads: Vec<JoinHandle<()>>,
}

impl Producers {
    fn new(spawner: Spawner, count: usize) -> Self {
        let (ready_tx, ready) = mpsc::channel();
        let mut requests = Vec::with_capacity(count);
        let mut threads = Vec::with_capacity(count);
        for _ in 0..count {
            let (request_tx, request_rx) = mpsc::channel::<Option<Arc<AtomicUsize>>>();
            let ready_tx = ready_tx.clone();
            let spawner = spawner.clone();
            threads.push(thread::spawn(move || {
                while let Ok(Some(completed)) = request_rx.recv() {
                    for _ in 0..TASKS_PER_PRODUCER {
                        let completed = Arc::clone(&completed);
                        spawner
                            .spawn(Priority::Normal, async move {
                                completed.fetch_add(1, Ordering::Release);
                            })
                            .expect("runtime is alive")
                            .detach();
                    }
                    ready_tx.send(()).expect("benchmark owner is alive");
                }
            }));
            requests.push(request_tx);
        }
        Self {
            requests,
            ready,
            threads,
        }
    }

    fn submit_and_wait(&self) {
        let expected = self.requests.len() * TASKS_PER_PRODUCER;
        let completed = Arc::new(AtomicUsize::new(0));
        for request in &self.requests {
            request
                .send(Some(Arc::clone(&completed)))
                .expect("producer is alive");
        }
        for _ in &self.requests {
            self.ready.recv().expect("producer acknowledgement");
        }
        while completed.load(Ordering::Acquire) != expected {
            std::thread::yield_now();
        }
    }
}

impl Drop for Producers {
    fn drop(&mut self) {
        for request in &self.requests {
            let _ = request.send(None);
        }
        for thread in self.threads.drain(..) {
            thread.join().expect("producer panicked");
        }
    }
}

fn external_producers(c: &mut Criterion) {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(4).expect("non-zero workers"))
        .build()
        .expect("runtime");
    let mut group = c.benchmark_group("v030/external-producers");
    for producers in PRODUCERS {
        let workers = Producers::new(runtime.spawner(), producers);
        group.throughput(Throughput::Elements(
            (producers * TASKS_PER_PRODUCER) as u64,
        ));
        group.bench_with_input(
            BenchmarkId::from_parameter(producers),
            &producers,
            |b, _| b.iter(|| workers.submit_and_wait()),
        );
        drop(workers);
    }
    group.finish();
    runtime.shutdown_graceful().expect("shutdown");
}

criterion_group!(benches, external_producers);
criterion_main!(benches);
