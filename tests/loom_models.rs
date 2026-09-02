//! Small Loom models for the runtime's synchronization contracts.
//!
//! These deliberately do not model `async-task`, `crossbeam-deque`, or the
//! full scheduler. They exercise only the state transitions whose correctness
//! relies on our own synchronization protocol.

use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use loom::sync::{Arc, Condvar, Mutex};
use loom::thread;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AdmissionGate {
    Running,
    Closing,
}

struct AdmissionState {
    gate: Mutex<AdmissionGate>,
    accepted: AtomicUsize,
}

#[test]
fn admission_is_linearized_with_close() {
    loom::model(|| {
        let state = Arc::new(AdmissionState {
            gate: Mutex::new(AdmissionGate::Running),
            accepted: AtomicUsize::new(0),
        });
        let accepted = Arc::new(AtomicBool::new(false));

        let submitter_state = Arc::clone(&state);
        let submitter_accepted = Arc::clone(&accepted);
        let submitter = thread::spawn(move || {
            let gate = submitter_state
                .gate
                .lock()
                .expect("admission gate poisoned");
            if *gate == AdmissionGate::Running {
                submitter_state.accepted.fetch_add(1, Ordering::AcqRel);
                submitter_accepted.store(true, Ordering::Release);
            }
        });

        let closer_state = Arc::clone(&state);
        let closer = thread::spawn(move || {
            let mut gate = closer_state.gate.lock().expect("admission gate poisoned");
            *gate = AdmissionGate::Closing;
        });

        submitter.join().expect("submitter panicked");
        closer.join().expect("closer panicked");

        assert_eq!(
            *state.gate.lock().expect("admission gate poisoned"),
            AdmissionGate::Closing
        );
        assert_eq!(
            state.accepted.load(Ordering::Acquire),
            usize::from(accepted.load(Ordering::Acquire)),
            "only a submission linearized before close may enter the drain count"
        );
    });
}

#[test]
fn final_completion_wakes_a_drain_waiter() {
    loom::model(|| {
        let count = Arc::new(AtomicUsize::new(1));
        let drain_lock = Arc::new(Mutex::new(()));
        let drained = Arc::new(Condvar::new());
        let waiter_done = Arc::new(AtomicBool::new(false));

        let waiter_count = Arc::clone(&count);
        let waiter_lock = Arc::clone(&drain_lock);
        let waiter_drained = Arc::clone(&drained);
        let waiter_done_flag = Arc::clone(&waiter_done);
        let waiter = thread::spawn(move || {
            let mut guard = waiter_lock.lock().expect("drain lock poisoned");
            while waiter_count.load(Ordering::Acquire) != 0 {
                guard = waiter_drained.wait(guard).expect("drain lock poisoned");
            }
            waiter_done_flag.store(true, Ordering::Release);
        });

        let completer_count = Arc::clone(&count);
        let completer_lock = Arc::clone(&drain_lock);
        let completer_drained = Arc::clone(&drained);
        let completer = thread::spawn(move || {
            let previous = completer_count.fetch_sub(1, Ordering::AcqRel);
            assert_eq!(previous, 1, "completion count must not underflow");
            if previous == 1 {
                let _guard = completer_lock.lock().expect("drain lock poisoned");
                completer_drained.notify_all();
            }
        });

        completer.join().expect("completer panicked");
        waiter.join().expect("waiter panicked");
        assert_eq!(count.load(Ordering::Acquire), 0);
        assert!(waiter_done.load(Ordering::Acquire));
    });
}

#[test]
fn work_submission_cannot_miss_park_registration() {
    loom::model(|| {
        let work_available = Arc::new(AtomicBool::new(false));
        let park_lock = Arc::new(Mutex::new(false));
        let parked = Arc::new(Condvar::new());
        let worker_done = Arc::new(AtomicBool::new(false));

        let worker_work = Arc::clone(&work_available);
        let worker_lock = Arc::clone(&park_lock);
        let worker_parked = Arc::clone(&parked);
        let worker_done_flag = Arc::clone(&worker_done);
        let worker = thread::spawn(move || {
            if !worker_work.load(Ordering::Acquire) {
                let mut sleeping = worker_lock.lock().expect("park lock poisoned");
                if !worker_work.load(Ordering::Acquire) {
                    *sleeping = true;
                    while !worker_work.load(Ordering::Acquire) {
                        sleeping = worker_parked.wait(sleeping).expect("park lock poisoned");
                    }
                    *sleeping = false;
                }
            }
            worker_done_flag.store(true, Ordering::Release);
        });

        let producer_work = Arc::clone(&work_available);
        let producer_lock = Arc::clone(&park_lock);
        let producer_parked = Arc::clone(&parked);
        let producer = thread::spawn(move || {
            producer_work.store(true, Ordering::Release);
            let sleeping = producer_lock.lock().expect("park lock poisoned");
            if *sleeping {
                producer_parked.notify_one();
            }
        });

        producer.join().expect("producer panicked");
        worker.join().expect("worker panicked");
        assert!(work_available.load(Ordering::Acquire));
        assert!(worker_done.load(Ordering::Acquire));
        assert!(!*park_lock.lock().expect("park lock poisoned"));
    });
}
