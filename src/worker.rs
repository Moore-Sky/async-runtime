use crate::priority::{Priority, PriorityWeights};
use crate::runtime::RuntimeState;
use crate::scheduler::WorkerQueues;

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

pub(crate) fn run(
    state: &RuntimeState,
    worker_index: usize,
    queues: WorkerQueues,
    weights: PriorityWeights,
) {
    state.register_worker(std::thread::current().id());
    state.scheduler.enter_worker(worker_index, queues);
    let mut selector = WeightedSelector::new(weights);
    while !state.scheduler.is_stopping() {
        if let Some(runnable) = take_selected(state, &mut selector) {
            state.scheduler.record_executed();
            let _ = runnable.run();
        } else {
            state.scheduler.park(worker_index);
        }
    }
    state.scheduler.leave_worker();
}

fn take_selected(
    state: &RuntimeState,
    selector: &mut WeightedSelector,
) -> Option<async_task::Runnable> {
    let first = selector.next();
    for priority in [
        first,
        next_priority(first),
        next_priority(next_priority(first)),
    ] {
        if let Some(runnable) = state.scheduler.take(priority) {
            return Some(runnable);
        }
    }
    None
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
