//! Optional runtime scheduling statistics.

/// A point-in-time snapshot of runtime scheduler counters.
///
/// The values are approximate under concurrent activity. Queue counts describe
/// runnable tasks that have not yet been taken by a worker; they are not a
/// task-liveness or completion count.
#[cfg(feature = "stats")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimeStats {
    /// Runnable high-priority tasks currently queued.
    pub queued_high: usize,
    /// Runnable normal-priority tasks currently queued.
    pub queued_normal: usize,
    /// Runnable background-priority tasks currently queued.
    pub queued_background: usize,
    /// Number of worker threads owned by the runtime.
    pub workers: usize,
    /// Workers currently parked waiting for more work.
    pub sleeping_workers: usize,
    /// Runnables taken by workers for execution.
    pub executed: u64,
    /// Successful steals from another worker's local queue.
    ///
    /// This counts successful steal operations, which may transfer a batch of
    /// runnable tasks into the stealing worker's local queue.
    pub stolen: u64,
    /// Attempts to steal from another worker's local queue.
    pub steal_attempts: u64,
    /// Tasks submitted from outside a runtime worker.
    pub external_spawned: u64,
    /// Tasks submitted from the owning runtime's worker threads.
    pub local_spawned: u64,
    /// Times workers entered the parked state.
    pub parks: u64,
    /// Worker wake notifications delivered for new work or shutdown.
    pub wakes: u64,
}
