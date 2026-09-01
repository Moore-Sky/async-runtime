//! Internal work-stealing scheduler used by the general runtime.
//!
//! This module deliberately owns no task lifecycle state. It only routes
//! [`async_task::Runnable`] values between local worker queues and global
//! priority injectors.

use crate::priority::Priority;
#[cfg(feature = "stats")]
use crate::stats::RuntimeStats;
use async_task::Runnable;
use crossbeam_deque::{Injector, Steal, Stealer, Worker};
use std::array;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};

static NEXT_SCHEDULER_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static WORKER_CONTEXT: RefCell<Option<WorkerContext>> = const { RefCell::new(None) };
}

const PRIORITY_COUNT: usize = 3;
const LOCAL_BURST_LIMIT: u8 = 64;

fn priority_index(priority: Priority) -> usize {
    match priority {
        Priority::High => 0,
        Priority::Normal => 1,
        Priority::Background => 2,
    }
}

/// Owner-only local queues assigned to one worker thread.
pub(crate) struct WorkerQueues {
    queues: [Worker<Runnable>; PRIORITY_COUNT],
}

impl WorkerQueues {
    fn new() -> Self {
        Self {
            queues: array::from_fn(|_| Worker::new_fifo()),
        }
    }

    fn stealers(&self) -> [Stealer<Runnable>; PRIORITY_COUNT] {
        array::from_fn(|priority| self.queues[priority].stealer())
    }
}

struct WorkerContext {
    scheduler_id: u64,
    index: usize,
    victim_cursor: usize,
    local_burst: [u8; PRIORITY_COUNT],
    queues: WorkerQueues,
}

struct ParkState {
    sleeping: bool,
}

struct Park {
    state: Mutex<ParkState>,
    cv: Condvar,
}

impl Park {
    fn new() -> Self {
        Self {
            state: Mutex::new(ParkState { sleeping: false }),
            cv: Condvar::new(),
        }
    }
}

#[cfg(feature = "stats")]
struct Counters {
    queued: [AtomicUsize; PRIORITY_COUNT],
    executed: AtomicU64,
    stolen: AtomicU64,
    steal_attempts: AtomicU64,
    external_spawned: AtomicU64,
    local_spawned: AtomicU64,
    parks: AtomicU64,
    wakes: AtomicU64,
}

#[cfg(feature = "stats")]
impl Counters {
    fn new() -> Self {
        Self {
            queued: array::from_fn(|_| AtomicUsize::new(0)),
            executed: AtomicU64::new(0),
            stolen: AtomicU64::new(0),
            steal_attempts: AtomicU64::new(0),
            external_spawned: AtomicU64::new(0),
            local_spawned: AtomicU64::new(0),
            parks: AtomicU64::new(0),
            wakes: AtomicU64::new(0),
        }
    }
}

/// Shared global state for a general runtime's worker pool.
pub(crate) struct Scheduler {
    id: u64,
    injectors: [Injector<Runnable>; PRIORITY_COUNT],
    stealers: Vec<[Stealer<Runnable>; PRIORITY_COUNT]>,
    parks: Vec<Park>,
    sleeping: AtomicUsize,
    wake_cursor: AtomicUsize,
    stopping: AtomicBool,
    #[cfg(feature = "stats")]
    counters: Counters,
}

impl Scheduler {
    /// Creates shared scheduling state and one owner-only queue set per worker.
    pub(crate) fn new(worker_count: usize) -> (Arc<Self>, Vec<WorkerQueues>) {
        let queues: Vec<WorkerQueues> = (0..worker_count).map(|_| WorkerQueues::new()).collect();
        let stealers = queues.iter().map(WorkerQueues::stealers).collect();
        let scheduler = Self {
            id: NEXT_SCHEDULER_ID.fetch_add(1, Ordering::Relaxed),
            injectors: array::from_fn(|_| Injector::new()),
            stealers,
            parks: (0..worker_count).map(|_| Park::new()).collect(),
            sleeping: AtomicUsize::new(0),
            wake_cursor: AtomicUsize::new(0),
            stopping: AtomicBool::new(false),
            #[cfg(feature = "stats")]
            counters: Counters::new(),
        };
        (Arc::new(scheduler), queues)
    }

    /// Installs the owner-only queues for the current worker thread.
    pub(crate) fn enter_worker(&self, index: usize, queues: WorkerQueues) {
        assert!(
            index < self.parks.len(),
            "worker index belongs to scheduler"
        );
        WORKER_CONTEXT.with(|slot| {
            let previous = slot.replace(Some(WorkerContext {
                scheduler_id: self.id,
                index,
                victim_cursor: index.wrapping_add(1),
                local_burst: [0; PRIORITY_COUNT],
                queues,
            }));
            assert!(previous.is_none(), "worker context already installed");
        });
    }

    /// Removes the current worker's owner-only queues.
    pub(crate) fn leave_worker(&self) {
        WORKER_CONTEXT.with(|slot| {
            let previous = slot.replace(None);
            if let Some(context) = previous {
                assert_eq!(context.scheduler_id, self.id, "worker belongs to scheduler");
            }
        });
    }

    /// Whether the current thread is a worker for this scheduler.
    pub(crate) fn is_current_worker(&self) -> bool {
        WORKER_CONTEXT.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|context| context.scheduler_id == self.id)
        })
    }

    /// Routes a runnable locally when called by this runtime's worker, or to a
    /// global priority injector otherwise.
    pub(crate) fn schedule(&self, priority: Priority, runnable: Runnable) {
        if self.is_stopping() {
            return;
        }
        let index = priority_index(priority);
        let mut runnable = Some(runnable);
        let routed_locally = WORKER_CONTEXT.with(|slot| {
            let mut context = slot.borrow_mut();
            if let Some(context) = context
                .as_mut()
                .filter(|context| context.scheduler_id == self.id)
            {
                context.queues.queues[index].push(runnable.take().expect("runnable routed once"));
                true
            } else {
                false
            }
        });
        if !routed_locally {
            self.injectors[index].push(runnable.expect("runnable routed once"));
        }
        self.queued_inc(index);
        self.wake_one();
    }

    /// Takes one runnable, preferring the current worker's local queue.
    pub(crate) fn take(&self, priority: Priority) -> Option<Runnable> {
        if self.is_stopping() {
            return None;
        }
        let priority_index = priority_index(priority);
        let local = WORKER_CONTEXT.with(|slot| {
            let mut context = slot.borrow_mut();
            let context = context
                .as_mut()
                .filter(|context| context.scheduler_id == self.id)?;
            if context.local_burst[priority_index] >= LOCAL_BURST_LIMIT {
                context.local_burst[priority_index] = 0;
                if let Some(runnable) =
                    self.steal_global(priority_index, &context.queues.queues[priority_index])
                {
                    return Some(runnable);
                }
            }
            if let Some(runnable) = context.queues.queues[priority_index].pop() {
                context.local_burst[priority_index] += 1;
                return Some(runnable);
            }
            if let Some(runnable) =
                self.steal_global(priority_index, &context.queues.queues[priority_index])
            {
                context.local_burst[priority_index] = 0;
                return Some(runnable);
            }
            self.steal_victim(priority_index, context)
        });
        if local.is_some() {
            self.queued_dec(priority_index);
            return local;
        }
        loop {
            match self.injectors[priority_index].steal() {
                Steal::Success(runnable) => {
                    self.queued_dec(priority_index);
                    return Some(runnable);
                }
                Steal::Empty => return None,
                Steal::Retry => std::hint::spin_loop(),
            }
        }
    }

    fn steal_global(&self, priority: usize, local: &Worker<Runnable>) -> Option<Runnable> {
        loop {
            match self.injectors[priority].steal_batch_and_pop(local) {
                Steal::Success(runnable) => return Some(runnable),
                Steal::Empty => return None,
                Steal::Retry => std::hint::spin_loop(),
            }
        }
    }

    fn steal_victim(&self, priority: usize, context: &mut WorkerContext) -> Option<Runnable> {
        let worker_count = self.stealers.len();
        if worker_count < 2 {
            return None;
        }
        for _ in 0..worker_count.saturating_sub(1) {
            let victim = context.victim_cursor % worker_count;
            context.victim_cursor = context.victim_cursor.wrapping_add(1);
            if victim == context.index {
                continue;
            }
            loop {
                self.steal_attempt();
                match self.stealers[victim][priority]
                    .steal_batch_and_pop(&context.queues.queues[priority])
                {
                    Steal::Success(runnable) => {
                        self.stolen();
                        return Some(runnable);
                    }
                    Steal::Empty => break,
                    Steal::Retry => std::hint::spin_loop(),
                }
            }
        }
        None
    }

    /// Parks a worker after it has exhausted all priorities.
    pub(crate) fn park(&self, worker_index: usize) {
        if self.is_stopping() || self.has_any_work() {
            return;
        }
        let park = &self.parks[worker_index];
        let mut state = park.state.lock().expect("scheduler park state poisoned");
        if self.is_stopping() {
            return;
        }
        state.sleeping = true;
        self.sleeping_inc();
        // Register as sleeping before the second queue check. A producer that
        // raced before registration is observed by this check; a producer that
        // races after registration observes `sleeping` and takes the park lock
        // before notifying.
        if self.is_stopping() || self.has_any_work() {
            state.sleeping = false;
            self.sleeping_dec();
            return;
        }
        self.parked();
        while !self.is_stopping() && !self.has_any_work() {
            state = park.cv.wait(state).expect("scheduler park state poisoned");
        }
        if state.sleeping {
            state.sleeping = false;
            self.sleeping_dec();
        }
    }

    /// Stops scheduling and wakes every parked worker.
    pub(crate) fn stop(&self) {
        if self.stopping.swap(true, Ordering::AcqRel) {
            return;
        }
        for park in &self.parks {
            let state = park.state.lock().expect("scheduler park state poisoned");
            park.cv.notify_all();
            drop(state);
            self.woke();
        }
    }

    /// Whether shutdown has started.
    pub(crate) fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    /// Records an accepted spawn, split by submission origin.
    pub(crate) fn record_spawn(&self, local: bool) {
        #[cfg(feature = "stats")]
        {
            let counter = if local {
                &self.counters.local_spawned
            } else {
                &self.counters.external_spawned
            };
            counter.fetch_add(1, Ordering::Relaxed);
        }
        #[cfg(not(feature = "stats"))]
        let _ = local;
    }

    /// Records that a worker executed a runnable.
    pub(crate) fn record_executed(&self) {
        #[cfg(feature = "stats")]
        self.counters.executed.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns a point-in-time scheduling statistics snapshot.
    #[cfg(feature = "stats")]
    pub(crate) fn stats(&self) -> RuntimeStats {
        RuntimeStats {
            queued_high: self.counters.queued[0].load(Ordering::Relaxed),
            queued_normal: self.counters.queued[1].load(Ordering::Relaxed),
            queued_background: self.counters.queued[2].load(Ordering::Relaxed),
            workers: self.parks.len(),
            sleeping_workers: self.sleeping.load(Ordering::Relaxed),
            executed: self.counters.executed.load(Ordering::Relaxed),
            stolen: self.counters.stolen.load(Ordering::Relaxed),
            steal_attempts: self.counters.steal_attempts.load(Ordering::Relaxed),
            external_spawned: self.counters.external_spawned.load(Ordering::Relaxed),
            local_spawned: self.counters.local_spawned.load(Ordering::Relaxed),
            parks: self.counters.parks.load(Ordering::Relaxed),
            wakes: self.counters.wakes.load(Ordering::Relaxed),
        }
    }

    fn has_any_work(&self) -> bool {
        self.injectors.iter().any(|injector| !injector.is_empty())
            || self
                .stealers
                .iter()
                .flat_map(|queues| queues.iter())
                .any(|stealer| !stealer.is_empty())
    }

    fn wake_one(&self) {
        // This participates in the missed-wake protocol: either the producer
        // observes the worker's registration here, or the worker's second
        // queue check observes the producer's earlier push.
        if self.sleeping.load(Ordering::SeqCst) == 0 {
            return;
        }
        let worker_count = self.parks.len();
        if worker_count == 0 {
            return;
        }
        let start = self.wake_cursor.fetch_add(1, Ordering::Relaxed) % worker_count;
        for offset in 0..worker_count {
            let park = &self.parks[(start + offset) % worker_count];
            let state = park.state.lock().expect("scheduler park state poisoned");
            if state.sleeping {
                park.cv.notify_one();
                drop(state);
                self.woke();
                return;
            }
        }
    }

    #[cfg(feature = "stats")]
    fn queued_inc(&self, priority: usize) {
        self.counters.queued[priority].fetch_add(1, Ordering::Relaxed);
    }
    #[cfg(not(feature = "stats"))]
    fn queued_inc(&self, _: usize) {}

    #[cfg(feature = "stats")]
    fn queued_dec(&self, priority: usize) {
        let counter = &self.counters.queued[priority];
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_sub(1)
        });
    }
    #[cfg(not(feature = "stats"))]
    fn queued_dec(&self, _: usize) {}

    fn sleeping_inc(&self) {
        self.sleeping.fetch_add(1, Ordering::SeqCst);
    }
    fn sleeping_dec(&self) {
        let _ = self
            .sleeping
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |value| {
                value.checked_sub(1)
            });
    }
    #[cfg(feature = "stats")]
    fn stolen(&self) {
        self.counters.stolen.fetch_add(1, Ordering::Relaxed);
    }
    #[cfg(not(feature = "stats"))]
    fn stolen(&self) {}
    #[cfg(feature = "stats")]
    fn steal_attempt(&self) {
        self.counters.steal_attempts.fetch_add(1, Ordering::Relaxed);
    }
    #[cfg(not(feature = "stats"))]
    fn steal_attempt(&self) {}
    #[cfg(feature = "stats")]
    fn parked(&self) {
        self.counters.parks.fetch_add(1, Ordering::Relaxed);
    }
    #[cfg(not(feature = "stats"))]
    fn parked(&self) {}
    #[cfg(feature = "stats")]
    fn woke(&self) {
        self.counters.wakes.fetch_add(1, Ordering::Relaxed);
    }
    #[cfg(not(feature = "stats"))]
    fn woke(&self) {}
}

#[cfg(test)]
mod tests {
    use super::{priority_index, Scheduler};
    use crate::Priority;
    use async_task::Runnable;

    fn runnable() -> Runnable {
        let (runnable, _task) = async_task::spawn(async {}, |_| {});
        runnable
    }

    #[test]
    fn current_worker_routes_to_local_queue_before_global_fallback() {
        let (scheduler, mut queues) = Scheduler::new(1);
        scheduler.schedule(Priority::Normal, runnable());
        let queues = queues.pop().expect("one worker queue set");
        scheduler.enter_worker(0, queues);

        scheduler.schedule(Priority::High, runnable());
        assert!(scheduler.take(Priority::High).is_some());
        assert!(scheduler.take(Priority::Normal).is_some());
        scheduler.leave_worker();
    }

    #[test]
    fn stopped_scheduler_drops_new_runnables_and_returns_no_work() {
        let (scheduler, mut queues) = Scheduler::new(1);
        scheduler.stop();
        scheduler.schedule(Priority::High, runnable());
        scheduler.enter_worker(0, queues.pop().expect("one worker queue set"));
        assert!(scheduler.take(Priority::High).is_none());
        scheduler.leave_worker();
    }

    #[cfg(feature = "stats")]
    #[test]
    fn global_injector_pulls_are_not_reported_as_worker_steals() {
        let (scheduler, mut queues) = Scheduler::new(1);
        scheduler.schedule(Priority::Normal, runnable());
        scheduler.enter_worker(0, queues.pop().expect("one worker queue set"));

        assert!(scheduler.take(Priority::Normal).is_some());
        let stats = scheduler.stats();
        assert_eq!(stats.stolen, 0);
        assert_eq!(stats.steal_attempts, 0);
        scheduler.leave_worker();
    }

    #[cfg(feature = "stats")]
    #[test]
    fn taking_from_another_worker_reports_a_victim_steal() {
        let (scheduler, mut queues) = Scheduler::new(2);
        queues[0].queues[priority_index(Priority::Normal)].push(runnable());
        let thief = queues.pop().expect("thief queue set");
        let _victim = queues.pop().expect("victim queue set remains alive");
        scheduler.enter_worker(1, thief);

        assert!(scheduler.take(Priority::Normal).is_some());
        let stats = scheduler.stats();
        assert_eq!(stats.stolen, 1);
        assert!(stats.steal_attempts >= 1);
        scheduler.leave_worker();
    }
}
