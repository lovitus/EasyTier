#[cfg(not(target_os = "linux"))]
compile_error!("core-packet-path-probe models the Linux tun-rs GRO path and must run on Linux");

use std::{hint::black_box, time::Duration, time::Instant};

use tokio::{sync::mpsc, task::yield_now};
use tun_rs::{GROTable, VIRTIO_NET_HDR_LEN};

const CHANNEL_CAPACITY: usize = 32;
const IPV4_HEADER_LEN: usize = 20;
const TCP_HEADER_LEN: usize = 20;
const DEFAULT_PACKETS: usize = 300_000;
const DEFAULT_PAYLOAD_BYTES: usize = 1360;
const DEFAULT_ROUTE_WORK: usize = 64;
const DEFAULT_ROUNDS: usize = 3;
const PRODUCTION_FRAME_CAPACITY: usize = 4096;

#[derive(Clone, Copy, Debug)]
enum Mode {
    Current,
    WriterYield,
    OrderedBatch,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::WriterYield => "writer-yield",
            Self::OrderedBatch => "ordered-batch",
        }
    }
}

struct Packet {
    frame: Vec<u8>,
    digest: u64,
}

struct WriterStats {
    packets: usize,
    bytes: usize,
    gro_calls: usize,
    digest: u64,
    max_batch: usize,
    max_gro_frame_bytes: usize,
    expanded_gro_frames: usize,
}

struct RunStats {
    elapsed: Duration,
    writer: WriterStats,
}

fn parse_value<T: std::str::FromStr>(name: &str, default: T) -> T {
    let prefix = format!("--{name}=");
    std::env::args()
        .find_map(|arg| arg.strip_prefix(&prefix).map(str::to_owned))
        .map(|value| {
            value
                .parse()
                .unwrap_or_else(|_| panic!("invalid {name}: {value}"))
        })
        .unwrap_or(default)
}

fn internet_checksum(chunks: &[&[u8]]) -> u16 {
    let mut sum = 0_u32;
    let mut pending = None;
    for chunk in chunks {
        for &byte in *chunk {
            if let Some(high) = pending.take() {
                sum = sum.wrapping_add(u16::from_be_bytes([high, byte]) as u32);
            } else {
                pending = Some(byte);
            }
        }
    }
    if let Some(high) = pending {
        sum = sum.wrapping_add(u16::from_be_bytes([high, 0]) as u32);
    }
    while sum > u16::MAX as u32 {
        sum = (sum & u16::MAX as u32) + (sum >> 16);
    }
    !(sum as u16)
}

fn build_tcp_packet(sequence: u64, payload_bytes: usize, route_work: usize) -> Packet {
    let mut route_state = sequence ^ 0x9e37_79b9_7f4a_7c15;
    for round in 0..route_work {
        route_state ^= (round as u64).wrapping_mul(0x517c_c1b7_2722_0a95);
        route_state = route_state
            .rotate_left(13)
            .wrapping_mul(0xff51_afd7_ed55_8ccd);
    }
    black_box(route_state);

    let ip_packet_len = IPV4_HEADER_LEN + TCP_HEADER_LEN + payload_bytes;
    assert!(ip_packet_len <= u16::MAX as usize);
    let frame_len = VIRTIO_NET_HDR_LEN + ip_packet_len;
    let mut frame = Vec::with_capacity(PRODUCTION_FRAME_CAPACITY.max(frame_len));
    frame.resize(frame_len, 0);
    let ip_offset = VIRTIO_NET_HDR_LEN;
    let tcp_offset = ip_offset + IPV4_HEADER_LEN;
    let payload_offset = tcp_offset + TCP_HEADER_LEN;
    let source = [10, 44, 0, 80];
    let destination = [10, 44, 0, 8];

    {
        let ip = &mut frame[ip_offset..tcp_offset];
        ip[0] = 0x45;
        ip[2..4].copy_from_slice(&(ip_packet_len as u16).to_be_bytes());
        ip[4..6].copy_from_slice(&(sequence as u16).to_be_bytes());
        ip[6..8].copy_from_slice(&0x4000_u16.to_be_bytes());
        ip[8] = 64;
        ip[9] = 6;
        ip[12..16].copy_from_slice(&source);
        ip[16..20].copy_from_slice(&destination);
        let checksum = internet_checksum(&[ip]);
        ip[10..12].copy_from_slice(&checksum.to_be_bytes());
    }

    {
        let tcp = &mut frame[tcp_offset..payload_offset];
        tcp[0..2].copy_from_slice(&40000_u16.to_be_bytes());
        tcp[2..4].copy_from_slice(&5201_u16.to_be_bytes());
        let tcp_sequence = (sequence as u32).wrapping_mul(payload_bytes as u32);
        tcp[4..8].copy_from_slice(&tcp_sequence.to_be_bytes());
        tcp[8..12].copy_from_slice(&1_u32.to_be_bytes());
        tcp[12] = 5 << 4;
        tcp[13] = 0x10;
        tcp[14..16].copy_from_slice(&u16::MAX.to_be_bytes());
    }

    for (index, byte) in frame[payload_offset..].iter_mut().enumerate() {
        *byte = route_state.wrapping_add(index as u64) as u8;
    }

    let tcp_len = TCP_HEADER_LEN + payload_bytes;
    let pseudo_header = [
        source[0],
        source[1],
        source[2],
        source[3],
        destination[0],
        destination[1],
        destination[2],
        destination[3],
        0,
        6,
        (tcp_len >> 8) as u8,
        tcp_len as u8,
    ];
    let tcp_checksum = internet_checksum(&[&pseudo_header, &frame[tcp_offset..]]);
    frame[tcp_offset + 16..tcp_offset + 18].copy_from_slice(&tcp_checksum.to_be_bytes());

    let digest = route_state ^ sequence ^ tcp_checksum as u64;
    Packet { frame, digest }
}

async fn produce_current(
    tx: mpsc::Sender<Packet>,
    packets: usize,
    payload_bytes: usize,
    route_work: usize,
) -> u64 {
    let mut digest = 0_u64;
    for sequence in 0..packets as u64 {
        let packet = build_tcp_packet(sequence, payload_bytes, route_work);
        digest ^= packet.digest;
        yield_now().await;
        tx.send(packet).await.expect("writer closed unexpectedly");
    }
    digest
}

async fn produce_ordered_batches(
    tx: mpsc::Sender<Packet>,
    packets: usize,
    payload_bytes: usize,
    route_work: usize,
) -> u64 {
    let mut digest = 0_u64;
    let mut sequence = 0_u64;
    while sequence < packets as u64 {
        let count = CHANNEL_CAPACITY.min(packets - sequence as usize);
        let mut batch = Vec::with_capacity(count);
        for _ in 0..count {
            let packet = build_tcp_packet(sequence, payload_bytes, route_work);
            digest ^= packet.digest;
            batch.push(packet);
            sequence += 1;
        }
        yield_now().await;
        let permits = tx
            .reserve_many(batch.len())
            .await
            .expect("writer closed unexpectedly");
        for (permit, packet) in permits.zip(batch) {
            permit.send(packet);
        }
    }
    digest
}

async fn process_gro(
    mut rx: mpsc::Receiver<Packet>,
    yield_before_drain: bool,
    scratch_capacity: usize,
) -> WriterStats {
    let mut gro = GROTable::new();
    let mut scratch = Vec::with_capacity(scratch_capacity);
    let original_frame_bytes = VIRTIO_NET_HDR_LEN + IPV4_HEADER_LEN + TCP_HEADER_LEN;
    let mut stats = WriterStats {
        packets: 0,
        bytes: 0,
        gro_calls: 0,
        digest: 0,
        max_batch: 0,
        max_gro_frame_bytes: 0,
        expanded_gro_frames: 0,
    };

    while let Some(first) = rx.recv().await {
        let mut packets = Vec::with_capacity(CHANNEL_CAPACITY);
        packets.push(first);
        if yield_before_drain {
            yield_now().await;
        }
        while packets.len() < CHANNEL_CAPACITY {
            match rx.try_recv() {
                Ok(packet) => packets.push(packet),
                Err(_) => break,
            }
        }

        let count = packets.len();
        let payload_total: usize = packets
            .iter()
            .map(|packet| packet.frame.len() - original_frame_bytes)
            .sum();
        for packet in &packets {
            stats.digest ^= packet.digest;
        }
        let mut frames: Vec<Vec<u8>> = packets.into_iter().map(|packet| packet.frame).collect();
        if scratch_capacity > 0 {
            scratch.clear();
            scratch.extend_from_slice(&frames[0]);
            std::mem::swap(&mut scratch, &mut frames[0]);
        }
        gro.apply_gro(&mut frames, VIRTIO_NET_HDR_LEN, false)
            .expect("tun-rs GRO rejected generated TCP packets");

        stats.gro_calls += 1;
        stats.packets += count;
        stats.bytes += payload_total;
        stats.max_batch = stats.max_batch.max(count);
        stats.max_gro_frame_bytes = stats
            .max_gro_frame_bytes
            .max(frames.iter().map(Vec::len).max().unwrap_or_default());
        stats.expanded_gro_frames += frames
            .iter()
            .filter(|frame| frame.len() > original_frame_bytes + payload_total / count)
            .count();
        if scratch_capacity > 0 {
            let scratch_index = frames
                .iter()
                .position(|frame| frame.capacity() >= scratch_capacity)
                .expect("reusable GRO scratch buffer was lost");
            std::mem::swap(&mut scratch, &mut frames[scratch_index]);
            scratch.clear();
        }
        black_box(&frames);
    }
    stats
}

async fn run_once(
    mode: Mode,
    packets: usize,
    payload_bytes: usize,
    route_work: usize,
    scratch_capacity: usize,
) -> RunStats {
    let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
    let writer = tokio::spawn(process_gro(
        rx,
        matches!(mode, Mode::WriterYield),
        scratch_capacity,
    ));
    let started = Instant::now();
    let producer_digest = match mode {
        Mode::Current | Mode::WriterYield => {
            produce_current(tx, packets, payload_bytes, route_work).await
        }
        Mode::OrderedBatch => produce_ordered_batches(tx, packets, payload_bytes, route_work).await,
    };
    let writer = writer.await.expect("GRO task panicked");
    let elapsed = started.elapsed();
    assert_eq!(writer.packets, packets);
    assert_eq!(writer.bytes, packets * payload_bytes);
    assert_eq!(writer.digest, producer_digest, "packet corruption");
    RunStats { elapsed, writer }
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let packets = parse_value("packets", DEFAULT_PACKETS);
    let payload_bytes = parse_value("payload-bytes", DEFAULT_PAYLOAD_BYTES);
    let route_work = parse_value("route-work", DEFAULT_ROUTE_WORK);
    let rounds = parse_value("rounds", DEFAULT_ROUNDS);
    let scratch_capacity = parse_value("scratch-capacity", 0_usize);
    assert!(packets > 0 && payload_bytes > 0 && rounds > 0);

    println!(
        "# model=linux-tun-rs-gro packets={packets} payload_bytes={payload_bytes} route_work={route_work} rounds={rounds} channel_capacity={CHANNEL_CAPACITY} production_frame_capacity={PRODUCTION_FRAME_CAPACITY} scratch_capacity={scratch_capacity}"
    );
    for mode in [Mode::Current, Mode::WriterYield, Mode::OrderedBatch] {
        let mut packet_rates = Vec::with_capacity(rounds);
        let mut gbps_rates = Vec::with_capacity(rounds);
        let mut gro_calls = Vec::with_capacity(rounds);
        let mut mean_batches = Vec::with_capacity(rounds);
        let mut max_batch = 0;
        let mut max_gro_frame_bytes = 0;
        let mut expanded_gro_frames = 0;

        for _ in 0..rounds {
            let result = run_once(mode, packets, payload_bytes, route_work, scratch_capacity).await;
            let seconds = result.elapsed.as_secs_f64();
            packet_rates.push(result.writer.packets as f64 / seconds);
            gbps_rates.push(result.writer.bytes as f64 * 8.0 / seconds / 1_000_000_000.0);
            gro_calls.push(result.writer.gro_calls as f64);
            mean_batches.push(result.writer.packets as f64 / result.writer.gro_calls as f64);
            max_batch = max_batch.max(result.writer.max_batch);
            max_gro_frame_bytes = max_gro_frame_bytes.max(result.writer.max_gro_frame_bytes);
            expanded_gro_frames = expanded_gro_frames.max(result.writer.expanded_gro_frames);
        }

        println!(
            "{{\"mode\":\"{}\",\"median_packets_per_second\":{:.0},\"median_gbps\":{:.3},\"median_gro_calls\":{:.0},\"median_packets_per_gro_call\":{:.2},\"max_batch\":{},\"max_gro_frame_bytes\":{},\"expanded_gro_frames\":{}}}",
            mode.name(),
            median(packet_rates),
            median(gbps_rates),
            median(gro_calls),
            median(mean_batches),
            max_batch,
            max_gro_frame_bytes,
            expanded_gro_frames,
        );
    }
}
