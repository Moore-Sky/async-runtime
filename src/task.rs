use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use async_channel::{Receiver, Sender};
use futures_lite::Stream;

const ACTIVE: u8 = 0;
const FINISHED: u8 = 1;

/// An awaitable task handle. Dropping it cancels the task.
#[must_use = "tasks are cancelled when dropped; await or detach the handle"]
pub struct Task<T> {
    inner: Option<TaskInner<T>>,
}

enum TaskInner<T> {
    Direct(async_task::Task<T>),
    Bridge(BridgeTask<T>),
}

impl<T> Task<T> {
    pub(crate) fn direct(inner: async_task::Task<T>) -> Self {
        Self {
            inner: Some(TaskInner::Direct(inner)),
        }
    }

    pub(crate) fn bridge(on_finish: impl Fn() + Send + Sync + 'static) -> (Self, BridgeDriver<T>)
    where
        T: Send + 'static,
    {
        let (sender, receiver) = async_channel::bounded(1);
        let shared = Arc::new(BridgeShared {
            state: AtomicU8::new(ACTIVE),
            cancel_requested: AtomicBool::new(false),
            cancel_sender: Mutex::new(None),
            sender,
            on_finish: Box::new(on_finish),
        });
        let (cancel_sender, cancel_receiver) = async_channel::bounded(1);
        *shared.cancel_sender.lock().expect("bridge mutex poisoned") = Some(cancel_sender);
        let bridge = BridgeTask {
            shared: Arc::clone(&shared),
            receiver: Box::pin(receiver),
        };
        (
            Self {
                inner: Some(TaskInner::Bridge(bridge)),
            },
            BridgeDriver {
                shared,
                cancel_receiver,
            },
        )
    }

    /// Lets the task continue in the background.
    pub fn detach(mut self) {
        match self.inner.take() {
            Some(TaskInner::Direct(task)) => task.detach(),
            Some(TaskInner::Bridge(bridge)) => bridge.detach(),
            None => {}
        }
    }

    /// Requests cancellation and waits for cancellation to finish.
    pub async fn cancel(mut self) -> Option<T> {
        match self.inner.take() {
            Some(TaskInner::Direct(task)) => task.cancel().await,
            Some(TaskInner::Bridge(bridge)) => bridge.cancel().await,
            None => None,
        }
    }

    /// Converts cancellation from a panic into `None`.
    ///
    /// # Panics
    ///
    /// Panics only if the internal task handle has already been consumed. This
    /// cannot occur through the public API because this method consumes `self`.
    pub fn fallible(mut self) -> FallibleTask<T> {
        let inner = match self.inner.take() {
            Some(TaskInner::Direct(task)) => FallibleInner::Direct(task.fallible()),
            Some(TaskInner::Bridge(bridge)) => FallibleInner::Bridge(bridge),
            None => panic!("task handle was already consumed"),
        };
        FallibleTask { inner: Some(inner) }
    }

    /// Returns whether the task has finished.
    pub fn is_finished(&self) -> bool {
        match self.inner.as_ref() {
            Some(TaskInner::Direct(task)) => task.is_finished(),
            Some(TaskInner::Bridge(task)) => task.is_finished(),
            None => true,
        }
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        if let Some(TaskInner::Bridge(bridge)) = self.inner.as_ref() {
            bridge.request_cancel();
        }
    }
}

impl<T> Unpin for Task<T> {}

impl<T> Future for Task<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self
            .get_mut()
            .inner
            .as_mut()
            .expect("task handle was already consumed")
        {
            TaskInner::Direct(task) => Pin::new(task).poll(cx),
            TaskInner::Bridge(task) => Pin::new(task).poll(cx),
        }
    }
}

/// A task handle that resolves to `None` if the task is cancelled.
#[must_use = "tasks are cancelled when dropped; await the handle"]
pub struct FallibleTask<T> {
    inner: Option<FallibleInner<T>>,
}

enum FallibleInner<T> {
    Direct(async_task::FallibleTask<T>),
    Bridge(BridgeTask<T>),
}

impl<T> Drop for FallibleTask<T> {
    fn drop(&mut self) {
        if let Some(FallibleInner::Bridge(bridge)) = self.inner.as_ref() {
            bridge.request_cancel();
        }
    }
}

impl<T> Unpin for FallibleTask<T> {}

impl<T> Future for FallibleTask<T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self
            .get_mut()
            .inner
            .as_mut()
            .expect("task handle was already consumed")
        {
            FallibleInner::Direct(task) => Pin::new(task).poll(cx),
            FallibleInner::Bridge(task) => task.poll_fallible(cx),
        }
    }
}

pub(crate) enum Completion<T> {
    Completed(T),
    Cancelled,
    Panicked(Box<dyn Any + Send + 'static>),
}

struct BridgeShared<T> {
    state: AtomicU8,
    cancel_requested: AtomicBool,
    cancel_sender: Mutex<Option<Sender<()>>>,
    sender: Sender<Completion<T>>,
    on_finish: Box<dyn Fn() + Send + Sync>,
}

struct BridgeTask<T> {
    shared: Arc<BridgeShared<T>>,
    receiver: Pin<Box<Receiver<Completion<T>>>>,
}

impl<T> BridgeTask<T> {
    fn detach(self) {
        drop(self);
    }

    fn request_cancel(&self) {
        self.shared.cancel_requested.store(true, Ordering::Release);
        if let Some(sender) = self
            .shared
            .cancel_sender
            .lock()
            .expect("bridge mutex poisoned")
            .as_ref()
        {
            let _ = sender.try_send(());
        }
    }

    fn is_finished(&self) -> bool {
        self.shared.state.load(Ordering::Acquire) == FINISHED
    }

    async fn cancel(mut self) -> Option<T> {
        self.request_cancel();
        match self.receiver.as_mut().recv().await {
            Ok(Completion::Completed(value)) => Some(value),
            Ok(Completion::Cancelled) | Err(_) => None,
            Ok(Completion::Panicked(payload)) => std::panic::resume_unwind(payload),
        }
    }

    fn poll_fallible(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        match self.receiver.as_mut().poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Completion::Completed(value))) => Poll::Ready(Some(value)),
            Poll::Ready(Some(Completion::Cancelled) | None) => Poll::Ready(None),
            Poll::Ready(Some(Completion::Panicked(payload))) => std::panic::resume_unwind(payload),
        }
    }
}

impl<T> Future for BridgeTask<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.receiver.as_mut().poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Some(Completion::Completed(value))) => Poll::Ready(value),
            Poll::Ready(Some(Completion::Cancelled) | None) => {
                panic!("task was cancelled")
            }
            Poll::Ready(Some(Completion::Panicked(payload))) => std::panic::resume_unwind(payload),
        }
    }
}

/// Owner-thread half of a remote-local bridge. It contains only `Send` data.
pub(crate) struct BridgeDriver<T> {
    shared: Arc<BridgeShared<T>>,
    cancel_receiver: Receiver<()>,
}

impl<T> Clone for BridgeDriver<T> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            cancel_receiver: self.cancel_receiver.clone(),
        }
    }
}

impl<T> BridgeDriver<T> {
    pub(crate) fn is_cancel_requested(&self) -> bool {
        self.shared.cancel_requested.load(Ordering::Acquire)
    }

    pub(crate) async fn cancelled(self) {
        let _ = self.cancel_receiver.recv().await;
    }

    pub(crate) fn complete(&self, completion: Completion<T>) {
        if self
            .shared
            .state
            .compare_exchange(ACTIVE, FINISHED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            let _ = self.shared.sender.try_send(completion);
            (self.shared.on_finish)();
            *self
                .shared
                .cancel_sender
                .lock()
                .expect("bridge mutex poisoned") = None;
        }
    }
}

/// Ensures an accepted remote task becomes cancelled if its local wrapper is dropped.
pub(crate) struct BridgeCompletionGuard<T>(Option<BridgeDriver<T>>);

impl<T> BridgeCompletionGuard<T> {
    pub(crate) fn new(driver: BridgeDriver<T>) -> Self {
        Self(Some(driver))
    }

    pub(crate) fn finish(mut self, completion: Completion<T>) {
        if let Some(driver) = self.0.take() {
            driver.complete(completion);
        }
    }
}

impl<T> Drop for BridgeCompletionGuard<T> {
    fn drop(&mut self) {
        if let Some(driver) = self.0.take() {
            driver.complete(Completion::Cancelled);
        }
    }
}
