use crate::priority::{Priority, PriorityWeights};
use crate::runtime::RuntimeState;
use async_channel::Receiver;
use futures_lite::future;
use std::sync::atomic::Ordering;

/// Per-worker weighted scheduling opportunities, not a completed-work ratio.
pub(crate) struct WeightedSelector {
    weights: PriorityWeights,
    current: Priority,
    remaining: usize,
}
impl WeightedSelector {
    pub(crate) fn new(weights: PriorityWeights) -> Self {
        Self {
            weights,
            current: Priority::High,
            remaining: weights.high().get(),
        }
    }
    pub(crate) fn next(&mut self) -> Priority {
        let selected = self.current;
        self.remaining -= 1;
        if self.remaining == 0 {
            self.current = next_priority(self.current);
            self.remaining = match self.current {
                Priority::High => self.weights.high().get(),
                Priority::Normal => self.weights.normal().get(),
                Priority::Background => self.weights.background().get(),
            };
        }
        selected
    }
}

pub(crate) fn run(state: &RuntimeState, shutdown: &Receiver<()>, weights: PriorityWeights) {
    state.register_worker(std::thread::current().id());
    let mut selector = WeightedSelector::new(weights);
    while !state.stopping.load(Ordering::Acquire) {
        if tick_selected(state, &mut selector) {
            continue;
        }
        // Cancellation of the losing tick futures unregisters their sleepers. async_io drives
        // the reactor too; the runtime intentionally owns no park/unpark mechanism.
        async_io::block_on(future::race(
            future::race(state.high.tick(), state.normal.tick()),
            future::race(state.background.tick(), async {
                let _ = shutdown.recv().await;
            }),
        ));
    }
}

fn tick_selected(state: &RuntimeState, selector: &mut WeightedSelector) -> bool {
    let first = selector.next();
    for priority in [
        first,
        next_priority(first),
        next_priority(next_priority(first)),
    ] {
        let ran = match priority {
            Priority::High => state.high.try_tick(),
            Priority::Normal => state.normal.try_tick(),
            Priority::Background => state.background.try_tick(),
        };
        if ran {
            return true;
        }
    }
    false
}
fn next_priority(priority: Priority) -> Priority {
    match priority {
        Priority::High => Priority::Normal,
        Priority::Normal => Priority::Background,
        Priority::Background => Priority::High,
    }
}

#[cfg(test)]
mod tests {
    use super::WeightedSelector;
    use crate::{Priority, PriorityWeights};

    #[test]
    fn default_selector_is_exactly_eight_four_one() {
        let mut selector = WeightedSelector::new(PriorityWeights::default());
        let selections: Vec<_> = (0..13).map(|_| selector.next()).collect();
        assert_eq!(
            selections,
            [
                Priority::High,
                Priority::High,
                Priority::High,
                Priority::High,
                Priority::High,
                Priority::High,
                Priority::High,
                Priority::High,
                Priority::Normal,
                Priority::Normal,
                Priority::Normal,
                Priority::Normal,
                Priority::Background,
            ]
        );
    }
}
