use crate::error::{ShutdownError, ShutdownOutcome, SpawnError};
use crate::priority::{Priority, PriorityWeights};
use crate::task::Task;
use crate::worker;
use async_channel::{Receiver, Sender};
use async_executor::Executor;
use futures_lite::future;
use std::future::Future;
use std::io;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle, ThreadId};
use std::time::Duration;

pub struct RuntimeBuilder {
    worker_threads: NonZeroUsize,
    weights: PriorityWeights,
}

impl RuntimeBuilder {
    /// Creates a builder for an explicit, non-zero worker count.
    pub fn new(worker_threads: NonZeroUsize) -> Self {
        Self {
            worker_threads,
            weights: PriorityWeights::default(),
        }
    }
    /// Replaces the default `8:4:1` priority weights.
    #[must_use]
    pub fn priority_weights(mut self, weights: PriorityWeights) -> Self {
        self.weights = weights;
        self
    }
    /// Starts all worker threads and returns the owning runtime handle.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if any worker thread cannot be created. Workers
    /// created earlier in the same build attempt are stopped and joined.
    pub fn build(self) -> io::Result<Runtime> {
        let (shutdown_tx, shutdown_rx) = async_channel::unbounded();
        let (drained_tx, drained_rx) = async_channel::bounded(1);
        let state = Arc::new(RuntimeState {
            high: Executor::new(),
            normal: Executor::new(),
            background: Executor::new(),
            gate: Mutex::new(Gate::Running),
            accepted_tasks: AtomicUsize::new(0),
            stopping: AtomicBool::new(false),
            shutdown_tx,
            drained_tx,
            drained_rx,
            worker_ids: Mutex::new(Vec::with_capacity(self.worker_threads.get())),
            worker_count: self.worker_threads.get(),
        });
        let mut workers = Vec::with_capacity(self.worker_threads.get());
        for number in 0..self.worker_threads.get() {
            let worker_state = Arc::clone(&state);
            let receiver = shutdown_rx.clone();
            match thread::Builder::new()
                .name(format!("async-runtime-{number}"))
                .spawn(move || worker::run(&worker_state, &receiver, self.weights))
            {
                Ok(handle) => workers.push(handle),
                Err(error) => {
                    state.request_stop();
                    for handle in workers {
                        let _ = handle.join();
                    }
                    return Err(error);
                }
            }
        }
        Ok(Runtime {
            state,
            workers: Mutex::new(workers),
            closed: false,
        })
    }
}

pub struct Runtime {
    pub(crate) state: Arc<RuntimeState>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    closed: bool,
}

impl Runtime {
    /// Returns a weak capability for submitting work to this runtime.
    pub fn spawner(&self) -> Spawner {
        Spawner {
            state: Arc::downgrade(&self.state),
        }
    }
    /// Submits a task to one priority queue.
    ///
    /// # Errors
    ///
    /// Returns [`SpawnError::Closed`] after shutdown begins.
    pub fn spawn<F, T>(&self, priority: Priority, future: F) -> Result<Task<T>, SpawnError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        self.state.spawn(priority, future)
    }
    /// Rejects new tasks, drains every accepted task, and joins all workers.
    ///
    /// # Errors
    ///
    /// Returns [`ShutdownError::CalledFromWorker`] when invoked by this
    /// runtime's worker, or [`ShutdownError::WorkerPanicked`] if a joined
    /// worker panicked.
    pub fn shutdown_graceful(mut self) -> Result<(), ShutdownError> {
        if self.state.is_current_worker() {
            self.state.begin_close();
            self.state.request_stop();
            self.closed = true;
            let _ = self.join_workers();
            self.state.finish_close();
            return Err(ShutdownError::CalledFromWorker);
        }
        self.state.begin_close();
        self.state.wait_for_drain();
        self.state.request_stop();
        self.closed = true;
        let result = self.join_workers();
        self.state.finish_close();
        result
    }
    /// Drains accepted tasks until `timeout`, then cancels the remainder.
    ///
    /// # Errors
    ///
    /// Returns [`ShutdownError::CalledFromWorker`] when invoked by this
    /// runtime's worker, or [`ShutdownError::WorkerPanicked`] if a joined
    /// worker panicked.
    pub fn shutdown_timeout(mut self, timeout: Duration) -> Result<ShutdownOutcome, ShutdownError> {
        if self.state.is_current_worker() {
            self.state.begin_close();
            self.state.request_stop();
            self.closed = true;
            let _ = self.join_workers();
            self.state.finish_close();
            return Err(ShutdownError::CalledFromWorker);
        }
        self.state.begin_close();
        let outcome = if self.state.wait_for_drain_timeout(timeout) {
            ShutdownOutcome::Completed
        } else {
            ShutdownOutcome::TimedOut {
                remaining_tasks: self.state.accepted_tasks.load(Ordering::Acquire),
            }
        };
        self.state.request_stop();
        self.closed = true;
        let result = self.join_workers();
        self.state.finish_close();
        result?;
        Ok(outcome)
    }
    /// Cancels remaining work and joins all workers.
    ///
    /// # Errors
    ///
    /// Returns [`ShutdownError::CalledFromWorker`] when invoked by this
    /// runtime's worker, or [`ShutdownError::WorkerPanicked`] if a joined
    /// worker panicked.
    pub fn shutdown_now(mut self) -> Result<(), ShutdownError> {
        let called_from_worker = self.state.is_current_worker();
        self.state.begin_close();
        self.state.request_stop();
        self.closed = true;
        let result = self.join_workers();
        self.state.finish_close();
        if called_from_worker {
            Err(ShutdownError::CalledFromWorker)
        } else {
            result
        }
    }
    fn join_workers(&self) -> Result<(), ShutdownError> {
        let current = thread::current().id();
        let workers = {
            let mut workers = self.workers.lock().expect("runtime worker list poisoned");
            std::mem::take(&mut *workers)
        };
        let mut panicked = false;
        for worker in workers {
            if worker.thread().id() == current {
                continue;
            }
            if worker.join().is_err() {
                panicked = true;
            }
        }
        if panicked {
            Err(ShutdownError::WorkerPanicked)
        } else {
            Ok(())
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if !self.closed {
            self.state.begin_close();
            self.state.request_stop();
            let _ = self.join_workers();
            self.state.finish_close();
            self.closed = true;
        }
    }
}

#[derive(Clone)]
pub struct Spawner {
    state: Weak<RuntimeState>,
}
impl Spawner {
    /// Submits a task to one priority queue.
    ///
    /// # Errors
    ///
    /// Returns [`SpawnError::Closed`] if the runtime no longer exists or has
    /// begun shutting down.
    pub fn spawn<F, T>(&self, priority: Priority, future: F) -> Result<Task<T>, SpawnError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        self.state
            .upgrade()
            .ok_or(SpawnError::Closed)?
            .spawn(priority, future)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Gate {
    Running,
    Closing,
    Closed,
}

/// Shared worker state. Its gate serializes task acceptance with transition to closing.
pub(crate) struct RuntimeState {
    pub(crate) high: Executor<'static>,
    pub(crate) normal: Executor<'static>,
    pub(crate) background: Executor<'static>,
    gate: Mutex<Gate>,
    pub(crate) accepted_tasks: AtomicUsize,
    pub(crate) stopping: AtomicBool,
    shutdown_tx: Sender<()>,
    drained_tx: Sender<()>,
    drained_rx: Receiver<()>,
    worker_ids: Mutex<Vec<ThreadId>>,
    worker_count: usize,
}

impl RuntimeState {
    fn spawn<F, T>(self: &Arc<Self>, priority: Priority, future: F) -> Result<Task<T>, SpawnError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let gate = self.gate.lock().expect("runtime lifecycle gate poisoned");
        if *gate != Gate::Running {
            return Err(SpawnError::Closed);
        }
        self.accepted_tasks.fetch_add(1, Ordering::AcqRel);
        let completion = CompletionGuard {
            // Tasks are owned by an executor inside this state. Keeping only a weak
            // reference here is essential: a queued task must not keep the executor
            // (and therefore itself) alive during shutdown_now or Runtime::drop.
            state: Arc::downgrade(self),
        };
        let tracked = async move {
            let _completion = completion;
            future.await
        };
        // A successful return means the runtime owns the queued task or its cancellation cleanup.
        let task = match priority {
            Priority::High => self.high.spawn(tracked),
            Priority::Normal => self.normal.spawn(tracked),
            Priority::Background => self.background.spawn(tracked),
        };
        drop(gate);
        Ok(Task::direct(task))
    }
    pub(crate) fn register_worker(&self, id: ThreadId) {
        self.worker_ids
            .lock()
            .expect("runtime worker id list poisoned")
            .push(id);
    }
    fn is_current_worker(&self) -> bool {
        self.worker_ids
            .lock()
            .expect("runtime worker id list poisoned")
            .contains(&thread::current().id())
    }
    fn begin_close(&self) {
        let mut gate = self.gate.lock().expect("runtime lifecycle gate poisoned");
        if *gate == Gate::Running {
            *gate = Gate::Closing;
        }
    }
    fn finish_close(&self) {
        *self.gate.lock().expect("runtime lifecycle gate poisoned") = Gate::Closed;
    }
    pub(crate) fn request_stop(&self) {
        if !self.stopping.swap(true, Ordering::AcqRel) {
            for _ in 0..self.worker_count {
                let _ = self.shutdown_tx.try_send(());
            }
        }
    }
    fn wait_for_drain(&self) {
        async_io::block_on(async {
            while self.accepted_tasks.load(Ordering::Acquire) != 0 {
                let _ = self.drained_rx.recv().await;
            }
        });
    }
    fn wait_for_drain_timeout(&self, timeout: Duration) -> bool {
        async_io::block_on(async {
            if self.accepted_tasks.load(Ordering::Acquire) == 0 {
                return true;
            }
            future::race(
                async {
                    while self.accepted_tasks.load(Ordering::Acquire) != 0 {
                        let _ = self.drained_rx.recv().await;
                    }
                    true
                },
                async {
                    async_io::Timer::after(timeout).await;
                    false
                },
            )
            .await
        })
    }
    fn complete_task(&self) {
        if self.accepted_tasks.fetch_sub(1, Ordering::AcqRel) == 1 {
            let _ = self.drained_tx.try_send(());
        }
    }
}

struct CompletionGuard {
    state: Weak<RuntimeState>,
}
impl Drop for CompletionGuard {
    fn drop(&mut self) {
        // Graceful shutdown keeps the runtime state alive, so every accepted task
        // contributes to its drain count. During forced teardown the state may
        // already be gone; there is then no waiter left to notify.
        if let Some(state) = self.state.upgrade() {
            state.complete_task();
        }
    }
}
