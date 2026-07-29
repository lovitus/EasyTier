use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    net::{IpAddr, SocketAddr},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

const FAILURE_LIMIT: u8 = 10;
const WINDOW: Duration = Duration::from_secs(10);
const SILENCE: Duration = Duration::from_secs(60);
const ITERATIONS: u64 = 5_000_000;
const TASKS: usize = 1_024;

struct CountingAllocator;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

// SAFETY: every operation delegates directly to the process System allocator.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: delegated with the caller-provided valid layout.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: delegated with the pointer and layout supplied by the caller.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: delegated with the pointer and layout supplied by the caller.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Clone, Copy, Debug)]
struct PunchStormGuard {
    window_started_at: Instant,
    block_until: Option<Instant>,
    last_target_key: Option<PunchTargetKey>,
    failed_attempts: u8,
}

impl PunchStormGuard {
    fn new(now: Instant) -> Self {
        Self {
            window_started_at: now,
            block_until: None,
            last_target_key: None,
            failed_attempts: 0,
        }
    }

    #[inline]
    fn allow(&mut self, now: Instant, has_live_peer: bool, target: SocketAddr) -> bool {
        if !has_live_peer {
            self.reset(now);
            return true;
        }
        let target_key = PunchTargetKey::from(target);
        if self.last_target_key != Some(target_key) {
            self.reset(now);
            return true;
        }
        let Some(block_until) = self.block_until else {
            return true;
        };
        if now < block_until {
            return false;
        }
        self.reset(now);
        true
    }

    #[inline]
    fn record_success(&mut self, now: Instant) {
        self.reset(now);
    }

    #[inline]
    fn record_failed_attempts(&mut self, now: Instant, target: SocketAddr, attempts: u32) {
        let target_key = PunchTargetKey::from(target);
        if self.last_target_key != Some(target_key)
            || now.saturating_duration_since(self.window_started_at) >= WINDOW
        {
            self.window_started_at = now;
            self.failed_attempts = 0;
        }
        self.last_target_key = Some(target_key);
        let attempts = attempts.min(u32::from(FAILURE_LIMIT) + 1) as u8;
        self.failed_attempts = self
            .failed_attempts
            .saturating_add(attempts)
            .min(FAILURE_LIMIT + 1);
        if self.failed_attempts > FAILURE_LIMIT {
            self.block_until = now.checked_add(SILENCE);
        }
    }

    #[inline]
    fn reset(&mut self, now: Instant) {
        self.window_started_at = now;
        self.block_until = None;
        self.last_target_key = None;
        self.failed_attempts = 0;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PunchTargetKey {
    ip: [u8; 16],
    port: u16,
    is_ipv6: bool,
}

impl From<SocketAddr> for PunchTargetKey {
    fn from(target: SocketAddr) -> Self {
        let (ip, is_ipv6) = match target.ip() {
            IpAddr::V4(ip) => {
                let mut bytes = [0; 16];
                bytes[..4].copy_from_slice(&ip.octets());
                (bytes, false)
            }
            IpAddr::V6(ip) => (ip.octets(), true),
        };
        Self {
            ip,
            port: target.port(),
            is_ipv6,
        }
    }
}

fn allocation_count() -> u64 {
    ALLOCATIONS.load(Ordering::Relaxed)
}

fn run_baseline(now: Instant) -> (Duration, u64, u64) {
    let targets = (0..TASKS)
        .map(|index| SocketAddr::from(([192, 0, 2, 1], 10_000 + index as u16)))
        .collect::<Vec<_>>();
    let allocations_before = allocation_count();
    let started = Instant::now();
    let mut checksum = 0_u64;
    for index in 0..ITERATIONS {
        let target = targets[index as usize % TASKS];
        let allowed = black_box(index & 0x3f != 0);
        checksum = checksum.wrapping_add(u64::from(allowed) ^ target.port() as u64);
        black_box(now);
    }
    (
        started.elapsed(),
        allocation_count().saturating_sub(allocations_before),
        checksum,
    )
}

fn run_candidate(now: Instant) -> (Duration, u64, u64) {
    let mut guards = vec![PunchStormGuard::new(now); TASKS];
    let targets = (0..TASKS)
        .map(|index| SocketAddr::from(([192, 0, 2, 1], 10_000 + index as u16)))
        .collect::<Vec<_>>();
    let allocations_before = allocation_count();
    let started = Instant::now();
    let mut checksum = 0_u64;
    for index in 0..ITERATIONS {
        let task_index = index as usize % TASKS;
        let guard = &mut guards[task_index];
        let target = targets[task_index];
        let tick = now + Duration::from_micros(index);
        if guard.allow(tick, true, target) {
            if index % 257 == 0 {
                guard.record_success(tick);
            } else {
                guard.record_failed_attempts(tick, target, 1);
            }
            checksum =
                checksum.wrapping_add(u64::from(guard.failed_attempts) ^ target.port() as u64);
        }
    }
    (
        started.elapsed(),
        allocation_count().saturating_sub(allocations_before),
        checksum,
    )
}

fn ns_per_operation(elapsed: Duration) -> f64 {
    elapsed.as_nanos() as f64 / ITERATIONS as f64
}

fn main() {
    let now = Instant::now();

    // Warm up code paths and allocator bookkeeping before the measured runs.
    black_box(run_baseline(now));
    black_box(run_candidate(now));

    let (baseline_elapsed, baseline_allocations, baseline_checksum) = run_baseline(now);
    let (candidate_elapsed, candidate_allocations, candidate_checksum) = run_candidate(now);
    let baseline_ns = ns_per_operation(baseline_elapsed);
    let candidate_ns = ns_per_operation(candidate_elapsed);

    println!("iterations={ITERATIONS}");
    println!("tasks={TASKS}");
    println!("guard_size_bytes={}", size_of::<PunchStormGuard>());
    println!("baseline_ns_per_op={baseline_ns:.3}");
    println!("candidate_ns_per_op={candidate_ns:.3}");
    println!("delta_ns_per_op={:.3}", candidate_ns - baseline_ns);
    println!("baseline_allocations={baseline_allocations}");
    println!("candidate_allocations={candidate_allocations}");
    println!("baseline_checksum={baseline_checksum}");
    println!("candidate_checksum={candidate_checksum}");

    assert_eq!(
        candidate_allocations, 0,
        "task-local guard loop must not allocate"
    );
    assert!(
        size_of::<PunchStormGuard>() <= 64,
        "guard unexpectedly grew beyond its fixed small-state budget"
    );
    assert!(
        candidate_ns <= baseline_ns + 250.0,
        "guard added more than 250ns at a burst boundary"
    );
}
