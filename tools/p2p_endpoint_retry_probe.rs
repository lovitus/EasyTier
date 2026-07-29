//! Standalone microbenchmark for the bounded, sharded P2P endpoint retry table.
//!
//! Build and run on the remote builder:
//!   rustc --edition 2024 -C opt-level=1 tools/p2p_endpoint_retry_probe.rs \
//!       -o /tmp/p2p_endpoint_retry_probe
//!   /tmp/p2p_endpoint_retry_probe

use std::{
    alloc::{GlobalAlloc, Layout, System},
    collections::HashMap,
    hash::{Hash, Hasher},
    hint::black_box,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Instant,
};

const SHARD_COUNT: usize = 64;
const SHARD_CAPACITY: usize = 1024;
const TOTAL_CAPACITY: usize = SHARD_COUNT * SHARD_CAPACITY;
const BACKOFF_SECS: [u64; 5] = [60, 120, 240, 480, 600];

struct CountingAllocator;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static DEALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        // SAFETY: Delegates the unchanged allocation request to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: `ptr` and `layout` are the pair returned by the system allocator.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct EndpointKey {
    peer_id: u32,
    scheme: u8,
    remote_addr: SocketAddr,
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    stage: Option<u8>,
    blocked_until_secs: u64,
    revision: u64,
    last_touched_secs: u64,
}

#[derive(Debug)]
struct Lease {
    key: EndpointKey,
    observed_revision: u64,
}

struct RetryTable {
    shards: Vec<Mutex<HashMap<EndpointKey, Entry>>>,
    next_revision: AtomicU64,
}

impl RetryTable {
    fn new() -> Self {
        let shards = (0..SHARD_COUNT)
            .map(|_| Mutex::new(HashMap::new()))
            .collect();
        Self {
            shards,
            next_revision: AtomicU64::new(0),
        }
    }

    fn shard_index(key: &EndpointKey) -> usize {
        let mut hasher = std::hash::DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish() as usize & (SHARD_COUNT - 1)
    }

    fn begin(&self, key: EndpointKey, now_secs: u64) -> Option<Lease> {
        let mut shard = self.shards[Self::shard_index(&key)].lock().unwrap();
        let revision = match shard.get_mut(&key) {
            Some(entry) => {
                entry.last_touched_secs = now_secs;
                if now_secs < entry.blocked_until_secs {
                    return None;
                }
                entry.revision
            }
            None => {
                if shard.len() >= SHARD_CAPACITY {
                    let oldest = shard
                        .iter()
                        .min_by_key(|(_, entry)| entry.last_touched_secs)
                        .map(|(key, _)| *key)
                        .expect("a full shard cannot be empty");
                    shard.remove(&oldest);
                }
                let revision = self.next_revision.fetch_add(1, Ordering::Relaxed) + 1;
                shard.insert(
                    key,
                    Entry {
                        stage: None,
                        blocked_until_secs: 0,
                        revision,
                        last_touched_secs: now_secs,
                    },
                );
                revision
            }
        };
        Some(Lease {
            key,
            observed_revision: revision,
        })
    }

    fn failed(&self, lease: Lease, now_secs: u64) {
        let shard_index = Self::shard_index(&lease.key);
        let mut shard = self.shards[shard_index].lock().unwrap();

        let Some(entry) = shard.get_mut(&lease.key) else {
            return;
        };
        if entry.revision != lease.observed_revision {
            return;
        }
        let stage = entry
            .stage
            .map_or(0, |stage| stage.saturating_add(1) as usize)
            .min(BACKOFF_SECS.len() - 1);
        entry.stage = Some(stage as u8);
        entry.revision = self.next_revision.fetch_add(1, Ordering::Relaxed) + 1;
        entry.blocked_until_secs = now_secs.saturating_add(BACKOFF_SECS[stage]);
        entry.last_touched_secs = now_secs;
    }

    fn succeeded(&self, lease: Lease) {
        self.shards[Self::shard_index(&lease.key)]
            .lock()
            .unwrap()
            .remove(&lease.key);
    }

    fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.lock().unwrap().len())
            .sum()
    }
}

fn key_for(index: u64) -> EndpointKey {
    EndpointKey {
        peer_id: index as u32,
        scheme: (index % 8) as u8,
        remote_addr: SocketAddr::new(
            IpAddr::V4(Ipv4Addr::new(
                100,
                ((index >> 16) & 0xff) as u8,
                ((index >> 8) & 0xff) as u8,
                (index & 0xff) as u8,
            )),
            10_000 + (index % 50_000) as u16,
        ),
    }
}

fn allocation_snapshot() -> (u64, u64, u64) {
    (
        ALLOCATIONS.load(Ordering::Relaxed),
        DEALLOCATIONS.load(Ordering::Relaxed),
        ALLOCATED_BYTES.load(Ordering::Relaxed),
    )
}

fn resident_set_kib() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn run_parallel(
    label: &str,
    threads: usize,
    operations_per_thread: usize,
    operation: impl Fn(usize, usize) + Send + Sync + 'static,
) {
    let operation = Arc::new(operation);
    let barrier = Arc::new(Barrier::new(threads + 1));
    let mut workers = Vec::with_capacity(threads);
    let allocation_before = allocation_snapshot();

    for thread_index in 0..threads {
        let operation = operation.clone();
        let barrier = barrier.clone();
        workers.push(thread::spawn(move || {
            barrier.wait();
            for operation_index in 0..operations_per_thread {
                operation(thread_index, operation_index);
            }
        }));
    }

    barrier.wait();
    let started = Instant::now();
    for worker in workers {
        worker.join().unwrap();
    }
    let elapsed = started.elapsed();
    let allocation_after = allocation_snapshot();
    let operations = threads * operations_per_thread;
    let throughput = operations as f64 / elapsed.as_secs_f64();

    println!(
        "{label}: threads={threads} operations={operations} elapsed_ms={:.3} ops_per_sec={throughput:.0} allocations={} deallocations={} allocated_bytes={}",
        elapsed.as_secs_f64() * 1000.0,
        allocation_after.0.saturating_sub(allocation_before.0),
        allocation_after.1.saturating_sub(allocation_before.1),
        allocation_after.2.saturating_sub(allocation_before.2),
    );
}

fn main() {
    let threads = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .clamp(1, 32);
    let operations_per_thread = 250_000;

    println!(
        "layout: key_bytes={} entry_bytes={} lease_bytes={} shards={} shard_capacity={} total_capacity={}",
        std::mem::size_of::<EndpointKey>(),
        std::mem::size_of::<Entry>(),
        std::mem::size_of::<Lease>(),
        SHARD_COUNT,
        SHARD_CAPACITY,
        TOTAL_CAPACITY,
    );

    run_parallel(
        "baseline_black_box",
        threads,
        operations_per_thread,
        move |thread_index, operation_index| {
            black_box(key_for(
                (thread_index * operations_per_thread + operation_index) as u64,
            ));
        },
    );

    let prefill_allocation_before = allocation_snapshot();
    let prefill_rss_before = resident_set_kib();
    let table = Arc::new(RetryTable::new());
    for index in 0..TOTAL_CAPACITY as u64 {
        let key = key_for(index);
        if let Some(lease) = table.begin(key, 1) {
            table.failed(lease, 1);
        }
    }
    assert!(table.len() <= TOTAL_CAPACITY);
    let prefill_allocation_after = allocation_snapshot();
    println!(
        "prefilled_entries={} allocations={} allocated_bytes={} rss_before_kib={:?} rss_after_kib={:?}",
        table.len(),
        prefill_allocation_after
            .0
            .saturating_sub(prefill_allocation_before.0),
        prefill_allocation_after
            .2
            .saturating_sub(prefill_allocation_before.2),
        prefill_rss_before,
        resident_set_kib(),
    );

    let blocked_table = table.clone();
    run_parallel(
        "blocked_lookup_full_table",
        threads,
        operations_per_thread,
        move |thread_index, operation_index| {
            let index = (thread_index * operations_per_thread + operation_index) % TOTAL_CAPACITY;
            black_box(blocked_table.begin(key_for(index as u64), 2).is_none());
        },
    );

    let admitted_table = Arc::new(RetryTable::new());
    let admitted_table_for_run = admitted_table.clone();
    run_parallel(
        "admit_fail_success",
        threads,
        operations_per_thread / 10,
        move |thread_index, operation_index| {
            let index = (thread_index * (operations_per_thread / 10) + operation_index) as u64;
            let key = key_for(index % (TOTAL_CAPACITY as u64 / 2));
            if let Some(lease) = admitted_table_for_run.begin(key, index + 1_000) {
                if operation_index % 16 == 0 {
                    admitted_table_for_run.succeeded(lease);
                } else {
                    admitted_table_for_run.failed(lease, index + 1_000);
                }
            }
        },
    );
    assert!(admitted_table.len() <= TOTAL_CAPACITY);

    let churn_table = table.clone();
    run_parallel(
        "bounded_churn_at_capacity",
        threads,
        2_000,
        move |thread_index, operation_index| {
            let index = TOTAL_CAPACITY as u64 + (thread_index * 2_000 + operation_index) as u64;
            let key = key_for(index);
            if let Some(lease) = churn_table.begin(key, index + 10_000) {
                churn_table.failed(lease, index + 10_000);
            }
        },
    );
    assert!(table.len() <= TOTAL_CAPACITY);
    println!("final_entries={} hard_cap={}", table.len(), TOTAL_CAPACITY);
}
