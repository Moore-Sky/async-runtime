use crate::error::{ShutdownError, ShutdownOutcome, SpawnError};
use crate::priority::{Priority, PriorityWeights};
use crate::scheduler::Scheduler;
use crate::task::Task;
use crate::worker;
use async_task::Builder as TaskBuilder;
use std::future::Future;
use std::io;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
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
        let (scheduler, worker_queues) = Scheduler::new(self.worker_threads.get());
        let state = Arc::new(RuntimeState {
            scheduler,
            gate: Mutex::new(Gate::Running),
            accepted_tasks: AtomicUsize::new(0),
            drain_lock: Mutex::new(()),
            drained: Condvar::new(),
            worker_ids: Mutex::new(Vec::with_capacity(self.worker_threads.get())),
        });
        let mut workers = Vec::with_capacity(self.worker_threads.get());
        for (number, queues) in worker_queues.into_iter().enumerate() {
            let worker_state = Arc::clone(&state);
            match thread::Builder::new()
                .name(format!("async-runtime-{number}"))
                .spawn(move || worker::run(&worker_state, number, queues, self.weights))
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

    /// Returns a relaxed snapshot of scheduler observability counters.
    #[cfg(feature = "stats")]
    pub fn stats(&self) -> crate::RuntimeStats {
        self.state.scheduler.stats()
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
    pub(crate) scheduler: Arc<Scheduler>,
    gate: Mutex<Gate>,
    pub(crate) accepted_tasks: AtomicUsize,
    drain_lock: Mutex<()>,
    drained: Condvar,
    worker_ids: Mutex<Vec<ThreadId>>,
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
            // Tasks are owned by the scheduler inside this state. Keeping only a weak
            // reference here is essential: a queued task must not keep the scheduler
            // (and therefore itself) alive during shutdown_now or Runtime::drop.
            state: Arc::downgrade(self),
        };
        let tracked = async move {
            let _completion = completion;
            future.await
        };
        let scheduler = Arc::downgrade(&self.scheduler);
        let schedule = move |runnable| {
            if let Some(scheduler) = scheduler.upgrade() {
                scheduler.schedule(priority, runnable);
            }
        };
        let (runnable, task) = TaskBuilder::new()
            .propagate_panic(true)
            .spawn(|()| tracked, schedule);
        self.scheduler
            .record_spawn(self.scheduler.is_current_worker());
        // A successful return means the runtime owns the queued task or its
        // cancellation cleanup. Initial scheduling follows the same local vs
        // global routing rule as every later wake.
        runnable.schedule();
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
        self.scheduler.stop();
    }
    fn wait_for_drain(&self) {
        let guard = self.drain_lock.lock().expect("runtime drain lock poisoned");
        drop(
            self.drained
                .wait_while(guard, |()| self.accepted_tasks.load(Ordering::Acquire) != 0)
                .expect("runtime drain lock poisoned"),
        );
    }
    fn wait_for_drain_timeout(&self, timeout: Duration) -> bool {
        let guard = self.drain_lock.lock().expect("runtime drain lock poisoned");
        let (guard, _) = self
            .drained
            .wait_timeout_while(guard, timeout, |()| {
                self.accepted_tasks.load(Ordering::Acquire) != 0
            })
            .expect("runtime drain lock poisoned");
        let drained = self.accepted_tasks.load(Ordering::Acquire) == 0;
        drop(guard);
        drained
    }
    fn complete_task(&self) {
        let _guard = self.drain_lock.lock().expect("runtime drain lock poisoned");
        if self.accepted_tasks.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.drained.notify_all();
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
