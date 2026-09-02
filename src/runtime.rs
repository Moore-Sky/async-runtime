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
            #[cfg(test)]
            admission_pause: Mutex::new(None),
            #[cfg(test)]
            last_completion_pause: Mutex::new(None),
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
    ///
    /// A successful return means the task was admitted into the runtime's
    /// lifecycle. A concurrent forced or timed shutdown may cancel it before
    /// its first poll; graceful shutdown still waits for it to reach a terminal
    /// state.
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
    /// A successful return means the task was admitted into the runtime's
    /// lifecycle. A concurrent forced or timed shutdown may cancel it before
    /// its first poll; graceful shutdown still waits for it to reach a terminal
    /// state.
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
    #[cfg(test)]
    admission_pause: Mutex<Option<Arc<TestPause>>>,
    #[cfg(test)]
    last_completion_pause: Mutex<Option<Arc<TestPause>>>,
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
        // This token begins at admission, not at the first poll. It is moved
        // into the task immediately so construction unwind, cancellation,
        // forced shutdown, and normal completion all retire the admission
        // exactly once.
        let completion = CompletionGuard {
            // Tasks are owned by the scheduler inside this state. Keeping only a weak
            // reference here is essential: a queued task must not keep the scheduler
            // (and therefore itself) alive during shutdown_now or Runtime::drop.
            state: Arc::downgrade(self),
        };
        drop(gate);
        #[cfg(test)]
        self.pause_after_admission();
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
        let previous = self.accepted_tasks.fetch_sub(1, Ordering::AcqRel);
        assert!(previous > 0, "runtime accepted task count underflow");
        if previous == 1 {
            #[cfg(test)]
            self.pause_before_last_completion_notify();
            // The waiter checks the zero predicate while holding this same
            // mutex. Locking before notify closes both interleavings: either
            // the waiter observes zero, or it has atomically begun waiting.
            let _guard = self.drain_lock.lock().expect("runtime drain lock poisoned");
            self.drained.notify_all();
        }
    }

    #[cfg(test)]
    fn pause_after_admission(&self) {
        let pause = self
            .admission_pause
            .lock()
            .expect("runtime test hook poisoned")
            .clone();
        if let Some(pause) = pause {
            pause.pause();
        }
    }

    #[cfg(test)]
    fn pause_before_last_completion_notify(&self) {
        let pause = self
            .last_completion_pause
            .lock()
            .expect("runtime test hook poisoned")
            .clone();
        if let Some(pause) = pause {
            pause.pause();
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

#[cfg(test)]
struct TestPause {
    reached: std::sync::Barrier,
    released: std::sync::Barrier,
}

#[cfg(test)]
impl TestPause {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            reached: std::sync::Barrier::new(2),
            released: std::sync::Barrier::new(2),
        })
    }

    fn pause(&self) {
        self.reached.wait();
        self.released.wait();
    }

    fn wait_until_reached(&self) {
        self.reached.wait();
    }

    fn release(&self) {
        self.released.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::{Gate, RuntimeBuilder, ShutdownOutcome, TestPause};
    use crate::Priority;
    use futures_lite::future;
    use std::num::NonZeroUsize;
    use std::sync::atomic::Ordering;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    fn runtime() -> super::Runtime {
        RuntimeBuilder::new(NonZeroUsize::new(1).expect("non-zero worker count"))
            .build()
            .expect("runtime builds")
    }

    fn wait_until_not_running(state: &super::RuntimeState) {
        for _ in 0..10_000 {
            if *state.gate.lock().expect("runtime lifecycle gate poisoned") != Gate::Running {
                return;
            }
            std::thread::yield_now();
        }
        panic!("shutdown did not close the admission gate");
    }

    #[test]
    fn graceful_shutdown_waits_for_admitted_task_before_initial_schedule() {
        let runtime = runtime();
        let state = Arc::clone(&runtime.state);
        let pause = TestPause::new();
        *state.admission_pause.lock().expect("test hook poisoned") = Some(Arc::clone(&pause));
        let spawner = runtime.spawner();
        let (ran_tx, ran_rx) = mpsc::channel();
        let producer = std::thread::spawn(move || {
            spawner
                .spawn(Priority::Normal, async move {
                    ran_tx.send(()).expect("test receiver remains alive");
                })
                .expect("admitted spawn succeeds")
                .detach();
        });
        pause.wait_until_reached();

        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        let shutdown = std::thread::spawn(move || {
            shutdown_tx
                .send(runtime.shutdown_graceful())
                .expect("test receiver remains alive");
        });
        wait_until_not_running(&state);
        assert!(shutdown_rx.try_recv().is_err());

        pause.release();
        producer.join().expect("producer does not panic");
        shutdown_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("graceful shutdown finishes after scheduling")
            .expect("graceful shutdown succeeds");
        shutdown.join().expect("shutdown thread does not panic");
        ran_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("admitted task runs before graceful shutdown returns");
        assert_eq!(state.accepted_tasks.load(Ordering::Acquire), 0);
    }

    #[test]
    fn forced_shutdown_cancels_admitted_task_before_initial_schedule() {
        let runtime = runtime();
        let state = Arc::clone(&runtime.state);
        let pause = TestPause::new();
        *state.admission_pause.lock().expect("test hook poisoned") = Some(Arc::clone(&pause));
        let spawner = runtime.spawner();
        let (task_tx, task_rx) = mpsc::channel();
        let producer = std::thread::spawn(move || {
            let task = spawner
                .spawn(Priority::Normal, async { 7_u8 })
                .expect("spawn was admitted before shutdown");
            task_tx.send(task).expect("test receiver remains alive");
        });
        pause.wait_until_reached();

        runtime.shutdown_now().expect("forced shutdown succeeds");
        pause.release();
        producer.join().expect("producer does not panic");
        let task = task_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("spawn returns its cancelled task handle");
        assert_eq!(future::block_on(task.fallible()), None);
        assert_eq!(state.accepted_tasks.load(Ordering::Acquire), 0);
    }

    #[test]
    fn timed_shutdown_cancels_admitted_task_before_initial_schedule() {
        let runtime = runtime();
        let state = Arc::clone(&runtime.state);
        let pause = TestPause::new();
        *state.admission_pause.lock().expect("test hook poisoned") = Some(Arc::clone(&pause));
        let spawner = runtime.spawner();
        let (task_tx, task_rx) = mpsc::channel();
        let producer = std::thread::spawn(move || {
            let task = spawner
                .spawn(Priority::Normal, async { 9_u8 })
                .expect("spawn was admitted before shutdown");
            task_tx.send(task).expect("test receiver remains alive");
        });
        pause.wait_until_reached();

        assert!(matches!(
            runtime
                .shutdown_timeout(Duration::ZERO)
                .expect("timed shutdown succeeds"),
            ShutdownOutcome::TimedOut { remaining_tasks: 1 }
        ));
        pause.release();
        producer.join().expect("producer does not panic");
        let task = task_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("spawn returns its cancelled task handle");
        assert_eq!(future::block_on(task.fallible()), None);
        assert_eq!(state.accepted_tasks.load(Ordering::Acquire), 0);
    }

    #[test]
    fn waiter_observes_zero_when_last_completion_precedes_notification() {
        let runtime = runtime();
        let state = Arc::clone(&runtime.state);
        let pause = TestPause::new();
        *state
            .last_completion_pause
            .lock()
            .expect("test hook poisoned") = Some(Arc::clone(&pause));
        let task = runtime
            .spawn(Priority::Normal, async {})
            .expect("spawn succeeds");
        pause.wait_until_reached();
        assert_eq!(state.accepted_tasks.load(Ordering::Acquire), 0);

        let (wait_tx, wait_rx) = mpsc::channel();
        let wait_state = Arc::clone(&state);
        let waiter = std::thread::spawn(move || {
            wait_state.wait_for_drain();
            wait_tx.send(()).expect("test receiver remains alive");
        });
        wait_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("waiter sees zero without needing the pending notification");

        pause.release();
        future::block_on(task);
        waiter.join().expect("waiter does not panic");
        runtime.shutdown_graceful().expect("shutdown succeeds");
    }
}
