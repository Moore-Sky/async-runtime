use std::sync::atomic::{AtomicU8, Ordering};

pub(crate) const RUNNING: u8 = 0;
pub(crate) const CLOSING: u8 = 1;
pub(crate) const CLOSED: u8 = 2;

pub(crate) struct Lifecycle(AtomicU8);

impl Lifecycle {
    pub(crate) const fn new() -> Self {
        Self(AtomicU8::new(RUNNING))
    }

    pub(crate) fn load(&self) -> u8 {
        self.0.load(Ordering::Acquire)
    }

    pub(crate) fn begin_close(&self) {
        let _ = self
            .0
            .compare_exchange(RUNNING, CLOSING, Ordering::AcqRel, Ordering::Acquire);
    }

    pub(crate) fn finish_close(&self) {
        self.0.store(CLOSED, Ordering::Release);
    }
}
