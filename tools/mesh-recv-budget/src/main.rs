//! Scheduling model, not Core throughput or a network-loss acceptance test.
use async_ringbuf::{AsyncHeapRb, traits::*};
use futures::StreamExt;
use std::{
    hint::black_box,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

struct Packet {
    seq: u64,
    sent: Instant,
    control: bool,
    payload: [u8; 128],
}

fn process(packet: Packet, last: &mut Option<u64>, count: &mut u64, sum: &mut u64) {
    assert!(!packet.control);
    assert!(last.is_none_or(|n| packet.seq > n));
    assert!(packet.payload.iter().all(|b| *b == packet.seq as u8));
    let mut work = packet.seq;
    for i in 0..128 {
        work = black_box(work.rotate_left(7).wrapping_mul(31) ^ i);
    }
    black_box(work);
    *last = Some(packet.seq);
    *count += 1;
    *sum = sum.wrapping_add(packet.seq);
}

fn percentile(values: &mut [u128], fraction: usize) -> u128 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    values[(values.len() - 1) * fraction / 100]
}

fn cpu_seconds(ticks: f64) -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    (fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap()) as f64 / ticks
}

async fn trial(runtime: &str, mode: &str, repeat: usize, ticks: f64) {
    let (mut producer, mut ring) = AsyncHeapRb::<Packet>::new(128).split();
    let (tx, mut rx) = mpsc::channel::<Packet>(128);
    let done = Arc::new(AtomicBool::new(false));
    let timer_done = done.clone();
    let timer = tokio::spawn(async move {
        let mut delays = Vec::new();
        while !timer_done.load(Ordering::Relaxed) {
            let deadline = tokio::time::Instant::now() + Duration::from_millis(1);
            tokio::time::sleep_until(deadline).await;
            delays.push(deadline.elapsed().as_micros());
        }
        delays
    });
    let bridge = tokio::spawn(async move {
        let mut control_delays = Vec::new();
        while let Some(packet) = ring.next().await {
            if packet.control {
                control_delays.push(packet.sent.elapsed().as_micros());
            } else {
                tx.send(packet).await.unwrap();
            }
        }
        control_delays
    });
    let start = Instant::now();
    let cpu_start = cpu_seconds(ticks);
    let sender = tokio::spawn(async move {
        let (mut accepted, mut sum, mut data_drops, mut control_drops, mut control_sent) =
            (0u64, 0u64, 0u64, 0u64, 0u64);
        for seq in 0..1_000_000u64 {
            // Model one cooperative I/O operation per received datagram.
            // No claim that this reproduces real socket/crypto processing cost.
            tokio::task::consume_budget().await;
            let control = seq % 4096 == 0;
            let packet = Packet {
                seq,
                sent: Instant::now(),
                control,
                payload: [seq as u8; 128],
            };
            if !control && producer.base().occupied_len() >= 124 {
                data_drops += 1;
                continue;
            }
            if producer.try_push(packet).is_err() {
                if control {
                    control_drops += 1;
                } else {
                    data_drops += 1;
                }
            } else if control {
                control_sent += 1;
            } else {
                accepted += 1;
                sum = sum.wrapping_add(seq);
            }
        }
        (accepted, sum, data_drops, control_drops, control_sent)
    });
    let limit = match mode {
        "recv" => 1,
        "batch8" => 8,
        _ => 32,
    };
    let mut batch = Vec::with_capacity(limit);
    let (mut last, mut count, mut sum, mut batches) = (None, 0, 0, 0u64);
    if mode == "recv" {
        while let Some(packet) = rx.recv().await {
            process(packet, &mut last, &mut count, &mut sum);
            batches += 1;
        }
    } else {
        while rx.recv_many(&mut batch, limit).await != 0 {
            batches += 1;
            for packet in batch.drain(..) {
                if mode == "batch32-budget" {
                    tokio::task::consume_budget().await;
                }
                process(packet, &mut last, &mut count, &mut sum);
            }
        }
    }
    let offered = sender.await.unwrap();
    let mut controls = bridge.await.unwrap();
    done.store(true, Ordering::Relaxed);
    let elapsed = start.elapsed().as_secs_f64();
    let cpu = cpu_seconds(ticks) - cpu_start;
    let mut timers = timer.await.unwrap();
    assert_eq!((count, sum), (offered.0, offered.1));
    assert_eq!(controls.len() as u64, offered.4);
    assert_eq!(count + offered.2 + offered.3 + offered.4, 1_000_000);
    let timer_max = timers.iter().copied().max().unwrap_or(0);
    let control_max = controls.iter().copied().max().unwrap_or(0);
    let timer_p99 = percentile(&mut timers, 99);
    let control_p99 = percentile(&mut controls, 99);
    println!(
        "{{\"runtime\":\"{runtime}\",\"mode\":\"{mode}\",\"repeat\":{repeat},\"delivered\":{count},\"data_drops\":{},\"control_drops\":{},\"goodput_pps\":{},\"cpu_s_per_million_delivered\":{},\"batches\":{batches},\"extra_local_slots\":{limit},\"packet_size\":{},\"timer_samples\":{},\"timer_p99_us\":{timer_p99},\"timer_max_us\":{timer_max},\"control_p99_us\":{control_p99},\"control_max_us\":{control_max}}}",
        offered.2,
        offered.3,
        count as f64 / elapsed,
        cpu * 1e6 / count as f64,
        std::mem::size_of::<Packet>(),
        timers.len()
    );
}

fn main() {
    let ticks = std::process::Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .unwrap();
    assert!(ticks.status.success());
    let ticks: f64 = String::from_utf8(ticks.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    for kind in ["current-thread", "two-workers"] {
        let runtime = if kind == "current-thread" {
            tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap()
        } else {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_time()
                .build()
                .unwrap()
        };
        for repeat in 0..3 {
            let modes = if repeat % 2 == 0 {
                ["recv", "batch8", "batch32", "batch32-budget"]
            } else {
                ["batch32-budget", "batch32", "batch8", "recv"]
            };
            for mode in modes {
                runtime.block_on(trial(kind, mode, repeat, ticks));
            }
        }
    }
}
