#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::must_use_candidate)]

//! A priority-aware native async runtime for the smol ecosystem.
//!
//! [`Runtime`] uses an `async-task` based scheduler with per-worker queues,
//! global priority injectors, work stealing, and parking workers. [`LocalDomain`]
//! uses `async-executor` and is driven explicitly by its host thread, allowing
//! it to run `!Send` futures. Neither runtime owns or drives an I/O reactor.
//!
//! Enable the `stats` feature to inspect an approximate [`RuntimeStats`]
//! snapshot of routing, stealing, parking, wake, and queue counters.

mod error;
mod lifecycle;
mod local;
mod priority;
mod runtime;
mod scheduler;
#[cfg(feature = "stats")]
mod stats;
mod task;
mod worker;

pub use error::{ShutdownError, ShutdownOutcome, SpawnError};
pub use local::{LocalDomain, LocalSpawner, RunStats};
pub use priority::{Priority, PriorityWeights};
pub use runtime::{Runtime, RuntimeBuilder, Spawner};
#[cfg(feature = "stats")]
pub use stats::RuntimeStats;
pub use task::{FallibleTask, Task};
