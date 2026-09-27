//! Isolated scheduling experiment, not a socket or Core throughput benchmark.
//! Keep this separate from the historical consumer-batching experiment.
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
use tokio::sync::{Barrier, mpsc};

const CAPACITY: usize = 128;
const RESERVED: usize = 4;
const SATURATED_PACKETS: u64 = 1_000_000;
const PACED_PACKETS: u64 = 16_384;

struct Packet {
    peer: usize,
    sequence: u64,
    offered_at: Instant,
    probe: bool,
    payload: [u8; 128],
}

#[derive(Default)]
struct Admission {
    offered: u64,
    accepted: u64,
    rejected: u64,
    accepted_sum: u64,
    probes_offered: u64,
    probes_accepted: u64,
    high_water: usize,
}

#[derive(Default)]
struct Delivered {
    count: u64,
    sum: u64,
    last: Option<u64>,
    probe_delays_us: Vec<u128>,
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

fn percentile(values: &mut [u128], percent: usize) -> String {
    if values.is_empty() {
        return "null".to_owned();
    }
    values.sort_unstable();
    values[(values.len() - 1) * percent / 100].to_string()
}

async fn trial(kind: &str, quantum: usize, repeat: u64, work: u64, paced: bool, ticks: f64) {
    let packet_count = if paced {
        PACED_PACKETS
    } else {
        SATURATED_PACKETS
    };
    let (first_producer, first_consumer) = AsyncHeapRb::<Packet>::new(CAPACITY).split();
    let (second_producer, second_consumer) = AsyncHeapRb::<Packet>::new(CAPACITY).split();
    let (tx, mut rx) = mpsc::channel::<Packet>(CAPACITY);
    // Two ring bridges, the producer, timer, and inline downstream consumer.
    let start_gate = Arc::new(Barrier::new(5));
    let done = Arc::new(AtomicBool::new(false));
    let mut bridges = Vec::new();
    for mut ring in [first_consumer, second_consumer] {
        let tx = tx.clone();
        let gate = start_gate.clone();
        bridges.push(tokio::spawn(async move {
            gate.wait().await;
            let mut forwarded = 0u64;
            while let Some(packet) = ring.next().await {
                // All packets, including probes, use the same bounded queue.
                tx.send(packet).await.unwrap();
                forwarded += 1;
            }
            forwarded
        }));
    }
    drop(tx);

    let gate = start_gate.clone();
    let timer_done = done.clone();
    let timer = tokio::spawn(async move {
        gate.wait().await;
        let mut delays = Vec::new();
        while !timer_done.load(Ordering::Relaxed) {
            let deadline = tokio::time::Instant::now() + Duration::from_millis(1);
            tokio::time::sleep_until(deadline).await;
            delays.push(deadline.elapsed().as_micros());
        }
        delays
    });

    let gate = start_gate.clone();
    let producer = tokio::spawn(async move {
        let mut rings = [first_producer, second_producer];
        let mut admission = [Admission::default(), Admission::default()];
        let mut quantum_used = 0usize;
        let mut explicit_yields = 0u64;
        gate.wait().await;
        let mut pulse_deadline = tokio::time::Instant::now();
        for sequence in 0..packet_count {
            if paced && sequence % 64 == 0 {
                tokio::time::sleep_until(pulse_deadline).await;
                // No catch-up burst. Compare measured rates, not the ceiling.
                pulse_deadline = tokio::time::Instant::now() + Duration::from_millis(2);
            }
            // Locked Tokio UDP async_io also consumes cooperative budget.
            // This models only that operation, not real socket readiness,
            // kernel buffering, crypto, TUN work, or GRO segment draining.
            tokio::task::consume_budget().await;
            // Coprime periods and repetition offsets avoid always sampling
            // the same position in a 32/64/128-packet scheduling quantum.
            let peer = usize::from(sequence % 509 == repeat * 97 % 509);
            let probe = peer == 1 || sequence % 251 == repeat * 73 % 251;
            let packet = Packet {
                peer,
                sequence,
                offered_at: Instant::now(),
                probe,
                payload: [sequence as u8; 128],
            };
            let stats = &mut admission[peer];
            stats.offered += 1;
            stats.probes_offered += u64::from(probe);
            let occupied = rings[peer].base().occupied_len();
            // Unchanged ordinary lossy admission: probes never consume the
            // four reserved transport-control slots and are never retried.
            if occupied >= CAPACITY - RESERVED || rings[peer].try_push(packet).is_err() {
                stats.rejected += 1;
            } else {
                stats.accepted += 1;
                stats.accepted_sum = stats.accepted_sum.wrapping_add(sequence);
                stats.probes_accepted += u64::from(probe);
                stats.high_water = stats.high_water.max(occupied + 1);
            }
            quantum_used += 1;
            if quantum != 0 && quantum_used == quantum {
                // A yield is only an opportunity, not a promise that the ring
                // consumer or timer will run before this task is polled again.
                tokio::task::yield_now().await;
                explicit_yields += 1;
                quantum_used = 0;
            }
        }
        (admission, explicit_yields)
    });

    let mut delivered = [Delivered::default(), Delivered::default()];
    let started = Instant::now();
    let cpu_before = cpu_seconds(ticks);
    start_gate.wait().await;
    while let Some(packet) = rx.recv().await {
        let stats = &mut delivered[packet.peer];
        assert!(stats.last.is_none_or(|last| packet.sequence > last));
        assert!(
            packet
                .payload
                .iter()
                .all(|byte| *byte == packet.sequence as u8)
        );
        let mut value = packet.sequence;
        for round in 0..work {
            value = black_box(value.rotate_left(7).wrapping_mul(31) ^ round);
        }
        black_box(value);
        if packet.probe {
            stats
                .probe_delays_us
                .push(packet.offered_at.elapsed().as_micros());
        }
        stats.count += 1;
        stats.sum = stats.sum.wrapping_add(packet.sequence);
        stats.last = Some(packet.sequence);
    }
    let elapsed = started.elapsed().as_secs_f64();
    let (admission, explicit_yields) = producer.await.unwrap();
    let mut forwarded = 0u64;
    for bridge in bridges {
        forwarded += bridge.await.unwrap();
    }
    done.store(true, Ordering::Relaxed);
    let mut timer_delays = timer.await.unwrap();
    let cpu = cpu_seconds(ticks) - cpu_before;
    let count: u64 = delivered.iter().map(|stats| stats.count).sum();
    let rejected: u64 = admission.iter().map(|stats| stats.rejected).sum();
    assert_eq!(count, forwarded);
    assert_eq!(count + rejected, packet_count);
    assert!(count > 0 && !timer_delays.is_empty());
    let mut peers = Vec::new();
    for (index, (input, output)) in admission.iter().zip(delivered.iter_mut()).enumerate() {
        assert_eq!(input.offered, input.accepted + input.rejected);
        assert_eq!(
            (output.count, output.sum),
            (input.accepted, input.accepted_sum)
        );
        assert_eq!(output.probe_delays_us.len() as u64, input.probes_accepted);
        assert!(input.high_water <= CAPACITY - RESERVED);
        let p99 = percentile(&mut output.probe_delays_us, 99);
        let max = percentile(&mut output.probe_delays_us, 100);
        peers.push(format!(
            "{{\"peer\":{index},\"offered\":{},\"accepted\":{},\"rejected\":{},\"high_water\":{},\"probes_offered\":{},\"probes_delivered\":{},\"probe_p99_us\":{p99},\"probe_max_us\":{max}}}",
            input.offered, input.accepted, input.rejected, input.high_water,
            input.probes_offered, input.probes_accepted,
        ));
    }
    let timer_samples = timer_delays.len();
    let timer_p99 = percentile(&mut timer_delays, 99);
    let timer_max = percentile(&mut timer_delays, 100);
    let load = if paced { "pulse64-2ms" } else { "saturated" };
    println!(
        "{{\"model\":\"producer-quantum\",\"runtime\":\"{kind}\",\"quantum\":{quantum},\"repeat\":{repeat},\"work_rounds\":{work},\"load\":\"{load}\",\"ring_capacity\":{CAPACITY},\"reserved\":{RESERVED},\"downstream_capacity\":{CAPACITY},\"offered\":{packet_count},\"delivered\":{count},\"rejected\":{rejected},\"elapsed_s\":{elapsed},\"cpu_s\":{cpu},\"actual_offered_pps\":{},\"delivered_pps\":{},\"cpu_s_per_million_delivered\":{},\"packet_size\":{},\"explicit_yields\":{explicit_yields},\"timer_samples\":{timer_samples},\"timer_p99_us\":{timer_p99},\"timer_max_us\":{timer_max},\"joined_tasks\":4,\"peers\":[{}]}}",
        packet_count as f64 / elapsed,
        count as f64 / elapsed,
        cpu * 1e6 / count as f64,
        std::mem::size_of::<Packet>(),
        peers.join(","),
    );
}

fn main() {
    let ticks = std::process::Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .unwrap();
    assert!(ticks.status.success());
    let ticks = String::from_utf8(ticks.stdout)
        .unwrap()
        .trim()
        .parse::<f64>()
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
            // Latin rotation: every arm occupies every order position once.
            let modes = [[0, 64, 32], [64, 32, 0], [32, 0, 64]][repeat as usize];
            for work in [128, 512] {
                for paced in [false, true] {
                    for quantum in modes {
                        runtime.block_on(async {
                            tokio::time::timeout(
                                Duration::from_secs(30),
                                trial(kind, quantum, repeat, work, paced, ticks),
                            )
                            .await
                            .expect("bounded scheduling trial did not complete");
                        });
                    }
                }
            }
        }
    }
}
