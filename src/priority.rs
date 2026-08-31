use std::num::NonZeroUsize;

/// Scheduling priority for tasks in the general worker pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Priority {
    /// Latency-sensitive work.
    High,
    /// Ordinary work.
    Normal,
    /// Work that may make progress at a lower rate.
    Background,
}

/// Relative scheduling opportunities assigned to each priority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriorityWeights {
    high: NonZeroUsize,
    normal: NonZeroUsize,
    background: NonZeroUsize,
}

impl PriorityWeights {
    /// Creates a set of non-zero weights.
    pub const fn new(high: NonZeroUsize, normal: NonZeroUsize, background: NonZeroUsize) -> Self {
        Self {
            high,
            normal,
            background,
        }
    }

    /// Returns the weight for high-priority work.
    pub const fn high(self) -> NonZeroUsize {
        self.high
    }

    /// Returns the weight for normal-priority work.
    pub const fn normal(self) -> NonZeroUsize {
        self.normal
    }

    /// Returns the weight for background work.
    pub const fn background(self) -> NonZeroUsize {
        self.background
    }
}

impl Default for PriorityWeights {
    fn default() -> Self {
        Self::new(
            NonZeroUsize::new(8).expect("8 is non-zero"),
            NonZeroUsize::new(4).expect("4 is non-zero"),
            NonZeroUsize::new(1).expect("1 is non-zero"),
        )
    }
}
