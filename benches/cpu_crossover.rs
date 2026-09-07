//! Locates the CPU-task granularity where a 64-task batch benefits from the
//! general runtime. Runtime construction, warm-up, and shutdown are untimed.
//!
//! `ASYNC_RUNTIME_CPU_WORKLOADS` selects comma-separated workload labels.
//! `ASYNC_RUNTIME_CPU_CASE_ORDER` selects and orders comma-separated cases.
//! Set `ASYNC_RUNTIME_CPU_CALIBRATE=1` for a short release-mode rounds probe.

use async_runtime::{Priority, Runtime, RuntimeBuilder};
use criterion::{Criterion, SamplingMode, Throughput};
use futures_lite::future;
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::time::Instant;

const TASKS: usize = 64;
const SEED: u64 = 0x1234_5678_9abc_def0;

#[derive(Clone, Copy)]
struct Workload {
    label: &'static str,
    rounds: u64,
}

// Calibrated on the formal measurement machine. Labels are target costs, not
// portable duration guarantees. Run the calibration mode before formal data.
const WORKLOADS: [Workload; 8] = [
    Workload {
        label: "0us",
        rounds: 0,
    },
    Workload {
        label: "2us",
        rounds: 1_023,
    },
    Workload {
        label: "5us",
        rounds: 2_559,
    },
    Workload {
        label: "10us",
        rounds: 5_117,
    },
    Workload {
        label: "20us",
        rounds: 10_234,
    },
    Workload {
        label: "50us",
        rounds: 25_586,
    },
    Workload {
        label: "100us",
        rounds: 51_172,
    },
    Workload {
        label: "200us",
        rounds: 102_344,
    },
];

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Case {
    Inline,
    Workers(usize),
}

impl Case {
    fn label(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Workers(1) => "workers-1",
            Self::Workers(2) => "workers-2",
            Self::Workers(4) => "workers-4",
            Self::Workers(8) => "workers-8",
            Self::Workers(_) => unreachable!("validated worker count"),
        }
    }
}

#[inline(never)]
fn cpu_kernel(mut state: u64, rounds: u64) -> u64 {
    for _ in 0..rounds {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state = state.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
    std::hint::black_box(state)
}

fn task_seed(task: usize) -> u64 {
    SEED ^ (task as u64).wrapping_mul(0xd6e8_feb8_6659_fd93)
}

fn run_inline(rounds: u64) -> u64 {
    (0..TASKS).fold(0_u64, |checksum, task| {
        checksum.wrapping_add(cpu_kernel(task_seed(task), rounds))
    })
}

fn run_runtime(runtime: &Runtime, rounds: u64) -> u64 {
    let handles = (0..TASKS)
        .map(|task| {
            runtime
                .spawn(Priority::Normal, async move {
                    cpu_kernel(task_seed(task), rounds)
                })
                .expect("CPU task spawn")
        })
        .collect::<Vec<_>>();
    handles.into_iter().fold(0_u64, |checksum, handle| {
        checksum.wrapping_add(future::block_on(handle))
    })
}

fn build_runtime(workers: usize) -> Runtime {
    let runtime = RuntimeBuilder::new(NonZeroUsize::new(workers).expect("non-zero workers"))
        .build()
        .expect("runtime");
    let expected = run_inline(0);
    assert_eq!(run_runtime(&runtime, 0), expected, "warm-up checksum");
    runtime
}

fn selected_workloads() -> Vec<Workload> {
    let Ok(value) = std::env::var("ASYNC_RUNTIME_CPU_WORKLOADS") else {
        return WORKLOADS.to_vec();
    };
    let labels = value.split(',').map(str::trim).filter(|s| !s.is_empty());
    let mut seen = HashSet::new();
    let selected = labels
        .map(|label| {
            assert!(seen.insert(label.to_owned()), "duplicate workload: {label}");
            WORKLOADS
                .iter()
                .copied()
                .find(|workload| workload.label == label)
                .unwrap_or_else(|| panic!("unknown workload: {label}"))
        })
        .collect::<Vec<_>>();
    assert!(!selected.is_empty(), "workload selection must not be empty");
    selected
}

fn selected_cases() -> Vec<Case> {
    let Ok(value) = std::env::var("ASYNC_RUNTIME_CPU_CASE_ORDER") else {
        return vec![
            Case::Inline,
            Case::Workers(1),
            Case::Workers(2),
            Case::Workers(4),
            Case::Workers(8),
        ];
    };
    let mut seen = HashSet::new();
    let selected = value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|label| {
            let case = match label {
                "inline" => Case::Inline,
                "workers-1" => Case::Workers(1),
                "workers-2" => Case::Workers(2),
                "workers-4" => Case::Workers(4),
                "workers-8" => Case::Workers(8),
                _ => panic!("unknown CPU crossover case: {label}"),
            };
            assert!(seen.insert(case), "duplicate CPU crossover case: {label}");
            case
        })
        .collect::<Vec<_>>();
    assert!(!selected.is_empty(), "case selection must not be empty");
    selected
}

fn cpu_crossover(criterion: &mut Criterion) {
    let cases = selected_cases();
    let workloads = selected_workloads();
    eprintln!(
        "CPU crossover tasks={TASKS}; workloads={}; cases={}",
        workloads
            .iter()
            .map(|workload| format!("{}:{}", workload.label, workload.rounds))
            .collect::<Vec<_>>()
            .join(","),
        cases
            .iter()
            .map(|case| case.label())
            .collect::<Vec<_>>()
            .join(",")
    );
    for workload in workloads {
        let expected = run_inline(workload.rounds);
        let mut group = criterion.benchmark_group(format!("cpu_crossover/64/{}", workload.label));
        group.sampling_mode(SamplingMode::Flat);
        group.throughput(Throughput::Elements(TASKS as u64));

        for case in &cases {
            match *case {
                Case::Inline => {
                    group.bench_function(case.label(), |b| {
                        b.iter(|| {
                            let checksum = run_inline(workload.rounds);
                            assert_eq!(checksum, expected);
                            std::hint::black_box(checksum)
                        });
                    });
                }
                Case::Workers(workers) => {
                    let runtime = build_runtime(workers);
                    group.bench_function(case.label(), |b| {
                        b.iter(|| {
                            let checksum = run_runtime(&runtime, workload.rounds);
                            assert_eq!(checksum, expected);
                            std::hint::black_box(checksum)
                        });
                    });
                    runtime.shutdown_graceful().expect("runtime shutdown");
                }
            }
        }
        group.finish();
    }
}

fn calibrate() {
    const PROBE_ROUNDS: u64 = 1_000_000;
    const TARGET_US: [u64; 7] = [2, 5, 10, 20, 50, 100, 200];
    let mut samples = (0..9)
        .map(|sample| {
            let started = Instant::now();
            std::hint::black_box(cpu_kernel(SEED ^ sample, PROBE_ROUNDS));
            started.elapsed().as_nanos() as u64
        })
        .collect::<Vec<_>>();
    samples.sort_unstable();
    let median_ns = samples[samples.len() / 2];
    println!("probe_rounds={PROBE_ROUNDS},median_ns={median_ns}");
    for target_us in TARGET_US {
        let rounds = (target_us * 1_000 * PROBE_ROUNDS + median_ns / 2) / median_ns;
        println!("{target_us}us={rounds}");
    }
}

fn main() {
    if std::env::var("ASYNC_RUNTIME_CPU_CALIBRATE").as_deref() == Ok("1") {
        calibrate();
        return;
    }
    let mut criterion = Criterion::default().configure_from_args();
    cpu_crossover(&mut criterion);
    criterion.final_summary();
}
