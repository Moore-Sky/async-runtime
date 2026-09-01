//! Run with `cargo bench --bench local_dispatch`.
//! Measures cross-thread inbox submission plus owner-thread draining. A fixed
//! producer thread and request/acknowledgement coordination are used for every
//! path, so none of these results labels same-thread submission as "remote".

use async_runtime::{LocalDomain, LocalSpawner};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

const BATCH_SIZES: [usize; 3] = [1, 64, 1024];

#[derive(Clone, Copy)]
enum Submission {
    SpawnBridge,
    DispatchClosure,
    DispatchFuture,
}

struct CrossThreadProducer {
    requests: Sender<Option<usize>>,
    completed: Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl CrossThreadProducer {
    fn new(spawner: LocalSpawner, submission: Submission) -> Self {
        let (requests, request_rx) = mpsc::channel::<Option<usize>>();
        let (completed_tx, completed) = mpsc::channel();
        let thread = thread::spawn(move || {
            while let Ok(Some(batch_size)) = request_rx.recv() {
                for _ in 0..batch_size {
                    match submission {
                        Submission::SpawnBridge => spawner
                            .spawn(async {})
                            .expect("domain open during benchmark")
                            .detach(),
                        Submission::DispatchClosure => spawner
                            .dispatch(|| {})
                            .expect("domain open during benchmark"),
                        Submission::DispatchFuture => spawner
                            .dispatch_future(async {})
                            .expect("domain open during benchmark"),
                    }
                }
                completed_tx.send(()).expect("benchmark owner is alive");
            }
        });
        Self {
            requests,
            completed,
            thread: Some(thread),
        }
    }

    fn submit(&self, batch_size: usize) {
        self.requests
            .send(Some(batch_size))
            .expect("producer thread is alive");
        self.completed.recv().expect("producer acknowledgement");
    }
}

impl Drop for CrossThreadProducer {
    fn drop(&mut self) {
        let _ = self.requests.send(None);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("producer thread panicked");
        }
    }
}

fn drain_to_idle(domain: &LocalDomain, max_steps: usize) -> usize {
    let mut driven = 0;
    while !domain.is_empty() {
        let steps = domain.run_n(max_steps);
        assert!(steps > 0, "ready cross-thread work must make progress");
        driven += steps;
    }
    driven
}

fn local_dispatch(c: &mut Criterion) {
    let domain = LocalDomain::new();
    let mut group = c.benchmark_group("local/cross-thread-inbox");
    for (name, submission) in [
        ("spawn-bridge", Submission::SpawnBridge),
        ("dispatch-closure", Submission::DispatchClosure),
        ("dispatch-future", Submission::DispatchFuture),
    ] {
        let producer = CrossThreadProducer::new(domain.spawner(), submission);
        for batch_size in BATCH_SIZES {
            group.throughput(Throughput::Elements(batch_size as u64));
            group.bench_with_input(
                BenchmarkId::new(name, batch_size),
                &batch_size,
                |b, &batch_size| {
                    b.iter(|| {
                        producer.submit(batch_size);
                        std::hint::black_box(drain_to_idle(&domain, batch_size));
                    });
                },
            );
        }
        drop(producer);
    }
    group.finish();
}

criterion_group!(benches, local_dispatch);
criterion_main!(benches);
