//! Host-driven, thread-affine local execution domains.

use std::cell::RefCell;
use std::future::Future;
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::panic::AssertUnwindSafe;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use async_channel::{Receiver, Sender};
use async_executor::LocalExecutor;
use futures_lite::future::{self, FutureExt};

use crate::error::{ShutdownOutcome, SpawnError};
use crate::lifecycle::{Lifecycle, CLOSED, RUNNING};
use crate::task::{BridgeCompletionGuard, BridgeDriver, Completion, Task};

/// A thread-affine executor driven explicitly by the thread that creates it.
///
/// The `Rc` marker deliberately makes this type `!Send + !Sync`. In particular,
/// a `LocalExecutor` runnable is never sent through the cross-thread inbox.
pub struct LocalDomain {
    executor: LocalExecutor<'static>,
    inbox: Receiver<InboxCommand>,
    sender: Sender<InboxCommand>,
    shared: Arc<Shared>,
    _not_send_or_sync: PhantomData<Rc<()>>,
}

/// A capability for submitting `Send` work to a particular [`LocalDomain`].
#[derive(Clone)]
pub struct LocalSpawner {
    sender: Sender<InboxCommand>,
    shared: Weak<Shared>,
}

struct Shared {
    lifecycle: Lifecycle,
    /// Serializes the running check, accepted-task increment, and inbox submission.
    gate: Mutex<()>,
    accepted_tasks: AtomicUsize,
}

impl Shared {
    fn complete_one(&self) {
        let previous = self.accepted_tasks.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "local accepted task count underflow");
    }
}

struct AcceptedGuard(Option<Arc<Shared>>);

type RunCommand = Box<dyn FnOnce(&LocalExecutor<'static>) + Send + 'static>;

impl AcceptedGuard {
    fn new(shared: Arc<Shared>) -> Self {
        Self(Some(shared))
    }
}

impl Drop for AcceptedGuard {
    fn drop(&mut self) {
        if let Some(shared) = self.0.take() {
            shared.complete_one();
        }
    }
}

/// A command is `Send` by construction. It contains a remote `Send` future and
/// bridge state only; it never contains a local runnable or `!Send` payload.
struct InboxCommand {
    run: Option<RunCommand>,
    cancel: Option<Box<dyn FnOnce() + Send + 'static>>,
}

impl InboxCommand {
    fn run(mut self, executor: &LocalExecutor<'static>, running: bool) {
        if running {
            if let Some(run) = self.run.take() {
                run(executor);
            }
        } else if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }

    fn cancel(mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }
}

impl LocalDomain {
    /// Creates a new domain bound to the current host thread.
    pub fn new() -> Self {
        let (sender, inbox) = async_channel::unbounded();
        Self {
            executor: LocalExecutor::new(),
            inbox,
            sender,
            shared: Arc::new(Shared {
                lifecycle: Lifecycle::new(),
                gate: Mutex::new(()),
                accepted_tasks: AtomicUsize::new(0),
            }),
            _not_send_or_sync: PhantomData,
        }
    }

    /// Returns a cross-thread capability that accepts `Send` futures only.
    pub fn spawner(&self) -> LocalSpawner {
        LocalSpawner {
            sender: self.sender.clone(),
            shared: Arc::downgrade(&self.shared),
        }
    }

    /// Spawns a future that is allowed to borrow only this local thread's affinity.
    ///
    /// # Errors
    ///
    /// Returns [`SpawnError::Closed`] after this domain begins shutting down.
    ///
    /// # Panics
    ///
    /// Panics if the internal lifecycle mutex was poisoned by an earlier panic.
    pub fn spawn_local<F, T>(&self, future: F) -> Result<Task<T>, SpawnError>
    where
        F: Future<Output = T> + 'static,
        T: 'static,
    {
        let _gate = self
            .shared
            .gate
            .lock()
            .expect("local lifecycle mutex poisoned");
        if self.shared.lifecycle.load() != RUNNING {
            return Err(SpawnError::Closed);
        }
        self.shared.accepted_tasks.fetch_add(1, Ordering::AcqRel);
        let guard = AcceptedGuard::new(Arc::clone(&self.shared));
        let task = self.executor.spawn(async move {
            let _guard = guard;
            future.await
        });
        Ok(Task::direct(task))
    }

    /// Returns whether the domain currently has no accepted, queued, or runnable work.
    pub fn is_empty(&self) -> bool {
        self.shared.accepted_tasks.load(Ordering::Acquire) == 0
            && self.inbox.is_empty()
            && self.executor.is_empty()
    }

    /// Processes a pending remote command or one scheduled local runnable.
    pub fn try_tick(&self) -> bool {
        if let Ok(command) = self.inbox.try_recv() {
            command.run(&self.executor, self.shared.lifecycle.load() != CLOSED);
            // A continuously supplied inbox must not starve already-materialized
            // local runnables. Give the executor one opportunity per command.
            let _ = self.executor.try_tick();
            true
        } else {
            self.executor.try_tick()
        }
    }

    /// Waits until a remote command or local runnable can make progress.
    pub async fn tick(&self) {
        if self.try_tick() {
            return;
        }
        future::race(async { self.executor.tick().await }, async {
            if let Ok(command) = self.inbox.recv().await {
                command.run(&self.executor, self.shared.lifecycle.load() != CLOSED);
            }
        })
        .await;
    }

    /// Drives this domain until `future` completes.
    pub async fn run<F: Future>(&self, future: F) -> F::Output {
        future::race(future, async {
            loop {
                self.tick().await;
            }
        })
        .await
    }

    /// Rejects new work and drives accepted tasks to completion.
    pub async fn shutdown_graceful(mut self) {
        self.begin_close();
        while self.shared.accepted_tasks.load(Ordering::Acquire) != 0 {
            self.tick().await;
        }
        self.shared.lifecycle.finish_close();
        self.cancel_inbox();
    }

    /// Gracefully drains until `deadline` resolves, then cancels remaining work.
    ///
    /// The deadline future is supplied by the host, so this executor does not
    /// require or drive a particular timer or I/O reactor.
    pub async fn shutdown_until<D>(mut self, deadline: D) -> ShutdownOutcome
    where
        D: Future,
    {
        self.begin_close();
        let drained = async {
            while self.shared.accepted_tasks.load(Ordering::Acquire) != 0 {
                self.tick().await;
            }
        };
        let completed = future::race(
            async {
                drained.await;
                true
            },
            async {
                deadline.await;
                false
            },
        )
        .await;
        if completed {
            self.shared.lifecycle.finish_close();
            self.cancel_inbox();
            ShutdownOutcome::Completed
        } else {
            let remaining = self.shared.accepted_tasks.load(Ordering::Acquire);
            self.shutdown_now_inner();
            ShutdownOutcome::TimedOut {
                remaining_tasks: remaining,
            }
        }
    }

    /// Rejects new work and drops the executor's remaining local tasks.
    pub fn shutdown_now(mut self) {
        self.shutdown_now_inner();
    }

    fn begin_close(&self) {
        let _gate = self
            .shared
            .gate
            .lock()
            .expect("local lifecycle mutex poisoned");
        self.shared.lifecycle.begin_close();
    }

    fn cancel_inbox(&mut self) {
        while let Ok(command) = self.inbox.try_recv() {
            command.cancel();
        }
    }

    fn shutdown_now_inner(&mut self) {
        self.begin_close();
        self.cancel_inbox();
        self.shared.lifecycle.finish_close();
    }
}

impl Drop for LocalDomain {
    fn drop(&mut self) {
        self.shutdown_now_inner();
    }
}

impl LocalSpawner {
    /// Submits `Send` work for execution on the local domain's owner thread.
    ///
    /// # Errors
    ///
    /// Returns [`SpawnError::Closed`] if the domain no longer exists or has
    /// begun shutting down.
    ///
    /// # Panics
    ///
    /// Panics if the internal lifecycle mutex was poisoned by an earlier panic.
    pub fn spawn<F, T>(&self, future: F) -> Result<Task<T>, SpawnError>
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let Some(shared) = self.shared.upgrade() else {
            return Err(SpawnError::Closed);
        };
        let _gate = shared.gate.lock().expect("local lifecycle mutex poisoned");
        if shared.lifecycle.load() != RUNNING {
            return Err(SpawnError::Closed);
        }

        shared.accepted_tasks.fetch_add(1, Ordering::AcqRel);
        let completed_shared = Arc::clone(&shared);
        let (task, driver) = Task::bridge(move || completed_shared.complete_one());
        let command = remote_command(future, driver);
        match self.sender.try_send(command) {
            Ok(()) => Ok(task),
            Err(error) => {
                error.into_inner().cancel();
                Err(SpawnError::Closed)
            }
        }
    }
}

fn remote_command<F, T>(future: F, driver: BridgeDriver<T>) -> InboxCommand
where
    F: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let state = Arc::new(Mutex::new(Some((future, driver))));
    let run_state = Arc::clone(&state);
    let cancel_state = Arc::clone(&state);
    InboxCommand {
        run: Some(Box::new(move |executor| {
            let Some((future, driver)) = run_state
                .lock()
                .expect("remote command mutex poisoned")
                .take()
            else {
                return;
            };
            if driver.is_cancel_requested() {
                driver.complete(Completion::Cancelled);
                return;
            }
            executor
                .spawn(async move {
                    let guard = BridgeCompletionGuard::new(driver.clone());
                    let user = async move {
                        match AssertUnwindSafe(future).catch_unwind().await {
                            Ok(value) => Completion::Completed(value),
                            Err(payload) => Completion::Panicked(payload),
                        }
                    };
                    let cancelled = async move {
                        driver.clone().cancelled().await;
                        Completion::Cancelled
                    };
                    guard.finish(user.race(cancelled).await);
                })
                .detach();
        })),
        cancel: Some(Box::new(move || {
            if let Some((_future, driver)) = cancel_state
                .lock()
                .expect("remote command mutex poisoned")
                .take()
            {
                driver.complete(Completion::Cancelled);
            }
        })),
    }
}

impl Default for LocalDomain {
    fn default() -> Self {
        Self::new()
    }
}

// Keep this compile-time-only import local to document the intended auto traits.
#[allow(dead_code)]
fn _local_domain_is_not_send_or_sync(_: &RefCell<LocalDomain>, _: NonZeroUsize) {}
