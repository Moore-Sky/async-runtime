#![forbid(unsafe_code)]
#![warn(clippy::all, clippy::pedantic)]
#![allow(clippy::module_name_repetitions, clippy::must_use_candidate)]

//! A priority-aware native async runtime for the smol ecosystem.
//!
//! It is built on `async-executor`, `async-task`, `async-channel`, and
//! `futures-lite`. It does not own or drive an I/O reactor. [`Runtime`] owns a
//! general-purpose worker pool. [`LocalDomain`] is driven by its host thread
//! and can run `!Send` futures.

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
