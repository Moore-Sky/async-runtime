use std::error::Error;
use std::fmt;

/// An error returned when a task cannot be accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnError {
    /// The target runtime or local domain has begun shutting down.
    Closed,
}

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => f.write_str("the target executor is closed"),
        }
    }
}

impl Error for SpawnError {}

/// The result of a shutdown operation with a deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
    /// Every accepted task completed before the deadline.
    Completed,
    /// The deadline elapsed and the remaining tasks were cancelled.
    TimedOut {
        /// Number of accepted tasks that had not completed at the deadline.
        remaining_tasks: usize,
    },
}

/// An error raised while shutting down a general runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownError {
    /// Shutdown was invoked by one of this runtime's own worker threads.
    CalledFromWorker,
    /// At least one worker thread panicked while being joined.
    WorkerPanicked,
}

impl fmt::Display for ShutdownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CalledFromWorker => {
                f.write_str("a runtime cannot synchronously join its current worker")
            }
            Self::WorkerPanicked => f.write_str("a runtime worker panicked"),
        }
    }
}

impl Error for ShutdownError {}
