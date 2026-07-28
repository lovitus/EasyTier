//! Standalone pre-production probe for the underlay interface snapshot cache.
//!
//! Build with:
//!   rustc --edition 2024 -O tools/interface_snapshot_cache_probe.rs -o /tmp/interface-snapshot-probe

use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const INTERFACE_COUNT: usize = 39;
const ENUMERATION_COST: Duration = Duration::from_micros(250);

#[derive(Debug)]
struct Snapshot {
    generation: u64,
    interfaces: Vec<(String, u32, [u8; 16])>,
}

#[derive(Default)]
struct CollectorStats {
    calls: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
}

impl CollectorStats {
    fn collect(&self, generation: u64) -> Arc<Snapshot> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_active.fetch_max(active, Ordering::AcqRel);
        thread::sleep(ENUMERATION_COST);
        let interfaces = (0..INTERFACE_COUNT)
            .map(|index| {
                (
                    format!("interface-{index}"),
                    index as u32 + 1,
                    [index as u8; 16],
                )
            })
            .collect();
        self.active.fetch_sub(1, Ordering::AcqRel);
        Arc::new(Snapshot {
            generation,
            interfaces,
        })
    }
}

#[derive(Default)]
struct CacheState {
    epoch: u64,
    refreshing: bool,
    snapshot: Option<Arc<Snapshot>>,
}

#[derive(Default)]
struct SnapshotCache {
    state: Mutex<CacheState>,
    changed: Condvar,
    next_generation: AtomicU64,
    collector: CollectorStats,
}

impl SnapshotCache {
    fn invalidate(&self) {
        let mut state = self.state.lock().unwrap();
        state.epoch = state.epoch.wrapping_add(1);
        self.changed.notify_all();
    }

    fn get(&self) -> Arc<Snapshot> {
        loop {
            let epoch = {
                let mut state = self.state.lock().unwrap();
                loop {
                    if let Some(snapshot) = state.snapshot.as_ref()
                        && snapshot.generation == state.epoch
                    {
                        return snapshot.clone();
                    }
                    if !state.refreshing {
                        state.refreshing = true;
                        break state.epoch;
                    }
                    state = self.changed.wait(state).unwrap();
                }
            };

            let _generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
            let snapshot = self.collector.collect(epoch);
            debug_assert_eq!(snapshot.interfaces.len(), INTERFACE_COUNT);

            let mut state = self.state.lock().unwrap();
            state.refreshing = false;
            if state.epoch == epoch {
                let published = Arc::new(Snapshot {
                    generation: state.epoch,
                    interfaces: snapshot.interfaces.clone(),
                });
                state.snapshot = Some(published.clone());
                self.changed.notify_all();
                return published;
            }
            self.changed.notify_all();
        }
    }
}

fn percentile(samples: &mut [u128], numerator: usize, denominator: usize) -> u128 {
    samples.sort_unstable();
    let index = (samples.len().saturating_sub(1) * numerator) / denominator;
    samples[index]
}

fn run_baseline(threads: usize, operations_per_thread: usize) {
    let stats = Arc::new(CollectorStats::default());
    let started = Instant::now();
    let mut workers = Vec::new();
    for worker in 0..threads {
        let stats = stats.clone();
        workers.push(thread::spawn(move || {
            let mut latencies = Vec::with_capacity(operations_per_thread);
            for operation in 0..operations_per_thread {
                let call_started = Instant::now();
                let snapshot = stats.collect((worker * operations_per_thread + operation) as u64);
                std::hint::black_box(snapshot.interfaces.len());
                latencies.push(call_started.elapsed().as_micros());
            }
            latencies
        }));
    }
    let mut latencies: Vec<_> = workers
        .into_iter()
        .flat_map(|worker| worker.join().unwrap())
        .collect();
    let elapsed = started.elapsed();
    let operations = threads * operations_per_thread;
    println!(
        "baseline threads={threads} operations={operations} collector_calls={} max_concurrent={} ops_per_sec={:.0} p50_us={} p95_us={} p99_us={}",
        stats.calls.load(Ordering::Relaxed),
        stats.max_active.load(Ordering::Relaxed),
        operations as f64 / elapsed.as_secs_f64(),
        percentile(&mut latencies, 50, 100),
        percentile(&mut latencies, 95, 100),
        percentile(&mut latencies, 99, 100),
    );
}

fn run_cached(label: &str, threads: usize, operations_per_thread: usize, invalidate: bool) {
    let cache = Arc::new(SnapshotCache::default());
    let initial = cache.get();
    std::hint::black_box(initial.interfaces.len());
    if invalidate {
        cache.invalidate();
    }
    let calls_before = cache.collector.calls.load(Ordering::Relaxed);
    let started = Instant::now();
    let mut workers = Vec::new();
    for _ in 0..threads {
        let cache = cache.clone();
        workers.push(thread::spawn(move || {
            let mut latencies = Vec::with_capacity(operations_per_thread);
            for _ in 0..operations_per_thread {
                let call_started = Instant::now();
                let snapshot = cache.get();
                std::hint::black_box(snapshot.interfaces.len());
                latencies.push(call_started.elapsed().as_micros());
            }
            latencies
        }));
    }
    let mut latencies: Vec<_> = workers
        .into_iter()
        .flat_map(|worker| worker.join().unwrap())
        .collect();
    let elapsed = started.elapsed();
    let operations = threads * operations_per_thread;
    println!(
        "{label} threads={threads} operations={operations} collector_calls={} max_concurrent={} ops_per_sec={:.0} p50_us={} p95_us={} p99_us={}",
        cache.collector.calls.load(Ordering::Relaxed) - calls_before,
        cache.collector.max_active.load(Ordering::Relaxed),
        operations as f64 / elapsed.as_secs_f64(),
        percentile(&mut latencies, 50, 100),
        percentile(&mut latencies, 95, 100),
        percentile(&mut latencies, 99, 100),
    );
}

fn main() {
    run_baseline(32, 64);
    run_cached("fresh-hit", 32, 10_000, false);
    run_cached("invalidated-burst", 32, 64, true);
    run_cached("invalidated-burst", 128, 32, true);
}
