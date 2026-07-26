use std::{
    cell::Cell,
    hint::black_box,
    sync::Arc,
    time::{Duration, Instant as StdInstant},
};

use quanta::Instant;

const ITERATIONS: usize = 2_000_000;
const REPEATS: usize = 5;

#[derive(Clone)]
struct Label {
    key: String,
    value: String,
}

#[derive(Clone)]
struct MetricKey {
    name: u8,
    labels: Vec<Label>,
}

struct MetricData {
    value: Cell<u64>,
    last_updated: Cell<Instant>,
}

impl MetricData {
    fn new() -> Self {
        Self {
            value: Cell::new(0),
            last_updated: Cell::new(Instant::now()),
        }
    }
}

#[derive(Clone)]
struct CounterHandle {
    metric_data: Arc<MetricData>,
    _key: MetricKey,
}

impl CounterHandle {
    fn new(name: u8) -> Self {
        Self {
            metric_data: Arc::new(MetricData::new()),
            _key: MetricKey {
                name,
                labels: vec![
                    Label {
                        key: "network_name".to_string(),
                        value: "core-metrics-probe".to_string(),
                    },
                    Label {
                        key: "to_instance_id".to_string(),
                        value: "87ede5a2-9c3d-492d-9bbe-989b9d07e742".to_string(),
                    },
                ],
            },
        }
    }

    fn add(&self, delta: u64) {
        self.metric_data
            .value
            .set(self.metric_data.value.get().saturating_add(delta));
        self.metric_data.last_updated.set(Instant::now());
    }

    fn add_at(&self, delta: u64, now: Instant) {
        self.metric_data
            .value
            .set(self.metric_data.value.get().saturating_add(delta));
        self.metric_data.last_updated.set(now);
    }
}

#[derive(Clone)]
struct TrafficCounters {
    bytes: CounterHandle,
    packets: CounterHandle,
}

impl TrafficCounters {
    fn new() -> Self {
        Self {
            bytes: CounterHandle::new(1),
            packets: CounterHandle::new(2),
        }
    }

    fn add_sample(&self, bytes: u64) {
        self.bytes.add(bytes);
        self.packets.add(1);
    }

    fn add_sample_shared_timestamp(&self, bytes: u64) {
        let now = Instant::now();
        self.bytes.add_at(bytes, now);
        self.packets.add_at(1, now);
    }

    fn checksum(&self) -> u64 {
        let labels = &self.bytes._key.labels;
        self.bytes.metric_data.value.get()
            ^ self.packets.metric_data.value.get()
            ^ u64::from(self.bytes._key.name)
            ^ labels
                .iter()
                .map(|label| (label.key.len() + label.value.len()) as u64)
                .sum::<u64>()
    }
}

enum CachedPeerTrafficCounters {
    Resolved(TrafficCounters),
}

impl CachedPeerTrafficCounters {
    fn cloned_counters(&self) -> TrafficCounters {
        match self {
            Self::Resolved(counters) => counters.clone(),
        }
    }

    fn counters(&self) -> &TrafficCounters {
        match self {
            Self::Resolved(counters) => counters,
        }
    }
}

fn measure(mut operation: impl FnMut()) -> Duration {
    let start = StdInstant::now();
    for _ in 0..ITERATIONS {
        operation();
    }
    start.elapsed()
}

fn median_ns_per_operation(mut samples: Vec<Duration>) -> f64 {
    samples.sort_unstable();
    samples[samples.len() / 2].as_nanos() as f64 / ITERATIONS as f64
}

fn run_case(name: &str, mut case: impl FnMut() -> u64) {
    let mut samples = Vec::with_capacity(REPEATS);
    let mut checksum = 0;
    for _ in 0..REPEATS {
        samples.push(measure(|| checksum ^= black_box(case())));
    }
    println!(
        "{name}: median_ns_per_sample={:.2} checksum={}",
        median_ns_per_operation(samples),
        black_box(checksum)
    );
}

fn main() {
    let legacy = CachedPeerTrafficCounters::Resolved(TrafficCounters::new());
    run_case("clone_handles_and_touch_twice", || {
        legacy.cloned_counters().add_sample(black_box(1360));
        legacy.counters().checksum()
    });

    let borrowed = CachedPeerTrafficCounters::Resolved(TrafficCounters::new());
    run_case("borrow_handles_and_touch_twice", || {
        borrowed.counters().add_sample(black_box(1360));
        borrowed.counters().checksum()
    });

    let shared_timestamp = CachedPeerTrafficCounters::Resolved(TrafficCounters::new());
    run_case("borrow_handles_and_touch_once", || {
        shared_timestamp
            .counters()
            .add_sample_shared_timestamp(black_box(1360));
        shared_timestamp.counters().checksum()
    });
}
