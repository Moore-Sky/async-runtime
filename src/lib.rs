//! A small, priority-aware async runtime built on the smol ecosystem.
//!
//! [`Runtime`] owns a general-purpose worker pool. [`LocalDomain`] is instead
//! driven by the host thread that created it and can run `!Send` futures.

mod error;
mod lifecycle;
mod local;
mod priority;
mod runtime;
mod task;
mod worker;

pub use error::{ShutdownError, ShutdownOutcome, SpawnError};
pub use local::{LocalDomain, LocalSpawner};
pub use priority::{Priority, PriorityWeights};
pub use runtime::{Runtime, RuntimeBuilder, Spawner};
pub use task::{FallibleTask, Task};
