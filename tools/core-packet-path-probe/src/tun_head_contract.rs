//! Locked-API ownership/byte contract for the existing bounded TUN experiment.
//! This is not a scheduler, real-device write, cancellation, or throughput test.

use bytes::BytesMut;
use tun_rs::{VIRTIO_NET_HDR_LEN, VirtioNetHdr, gso_split, handle_gro};

const HEAD_CAPACITY: usize = 8192;
const PAYLOAD: usize = 1320;
const AEAD_TAIL: usize = 28;

// Same selection and allocation-identity recovery as tun_capacity.rs.in.
// The dependency, not this helper, decides whether packets can be coalesced.
fn promote(packets: &mut [BytesMut], scratch: &mut Option<BytesMut>) -> Option<(usize, usize)> {
    if packets.len() < 2 || scratch.is_none() {
        return None;
    }
    let index = packets.iter().position(|frame| {
        if frame.len() >= HEAD_CAPACITY || frame.capacity() >= HEAD_CAPACITY {
            return false;
        }
        let Some(packet) = frame.get(VIRTIO_NET_HDR_LEN..) else {
            return false;
        };
        match packet.first().map(|byte| byte >> 4) {
            Some(4) => packet.len() >= 40 && packet[0] & 15 == 5 && packet[9] == 6,
            Some(6) => packet.len() >= 60 && packet[6] == 6,
            _ => false,
        }
    })?;
    let mut head = scratch.take().expect("scratch checked above");
    head.clear();
    head.extend_from_slice(&packets[index]);
    let identity = head.as_ptr() as usize;
    packets[index] = head;
    Some((identity, index))
}

fn reclaim(packets: &mut Vec<BytesMut>, identity: usize) -> BytesMut {
    let index = packets
        .iter()
        .position(|packet| packet.as_ptr() as usize == identity)
        .expect("GRO lost or reallocated the reusable head");
    let mut head = packets.swap_remove(index);
    head.clear();
    assert_eq!(head.capacity(), HEAD_CAPACITY);
    head
}

fn frame(v6: bool, ordinal: u32, flow: u16, payload: usize, flags: u8, wide: bool) -> BytesMut {
    let ip_len = if v6 { 40 } else { 20 };
    let tcp_len = 20 + payload;
    let mut bytes = vec![0_u8; VIRTIO_NET_HDR_LEN + ip_len + tcp_len];
    let ip = &mut bytes[VIRTIO_NET_HDR_LEN..VIRTIO_NET_HDR_LEN + ip_len];
    let pseudo = if v6 {
        ip[0] = 0x60;
        ip[4..6].copy_from_slice(&(tcp_len as u16).to_be_bytes());
        ip[6] = 6;
        ip[7] = 64;
        let src = [0x20, 1, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        let dst = [0x20, 1, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2];
        ip[8..24].copy_from_slice(&src);
        ip[24..40].copy_from_slice(&dst);
        let mut pseudo = src.to_vec();
        pseudo.extend_from_slice(&dst);
        pseudo.extend_from_slice(&(tcp_len as u32).to_be_bytes());
        pseudo.extend_from_slice(&[0, 0, 0, 6]);
        pseudo
    } else {
        ip[0] = 0x45;
        ip[2..4].copy_from_slice(&((ip_len + tcp_len) as u16).to_be_bytes());
        ip[4..6].copy_from_slice(&(ordinal as u16).to_be_bytes());
        ip[6..8].copy_from_slice(&0x4000_u16.to_be_bytes());
        ip[8] = 64;
        ip[9] = 6;
        ip[12..16].copy_from_slice(&[192, 0, 2, 1]);
        ip[16..20].copy_from_slice(&[192, 0, 2, 2]);
        let checksum = super::internet_checksum(&[ip]);
        ip[10..12].copy_from_slice(&checksum.to_be_bytes());
        let mut pseudo = ip[12..20].to_vec();
        pseudo.extend_from_slice(&[0, 6]);
        pseudo.extend_from_slice(&(tcp_len as u16).to_be_bytes());
        pseudo
    };
    let tcp = &mut bytes[VIRTIO_NET_HDR_LEN + ip_len..];
    tcp[0..2].copy_from_slice(&(40000 + flow).to_be_bytes());
    tcp[2..4].copy_from_slice(&5201_u16.to_be_bytes());
    tcp[4..8].copy_from_slice(&ordinal.wrapping_mul(PAYLOAD as u32).to_be_bytes());
    tcp[8..12].copy_from_slice(&1_u32.to_be_bytes());
    tcp[12] = 5 << 4;
    tcp[13] = flags;
    tcp[14..16].copy_from_slice(&u16::MAX.to_be_bytes());
    for (index, byte) in tcp[20..].iter_mut().enumerate() {
        *byte = ordinal.wrapping_mul(31).wrapping_add(index as u32) as u8;
    }
    let checksum = super::internet_checksum(&[&pseudo, tcp]);
    tcp[16..18].copy_from_slice(&checksum.to_be_bytes());
    let capacity = if wide {
        HEAD_CAPACITY
    } else {
        bytes.len() + AEAD_TAIL
    };
    let mut result = BytesMut::with_capacity(capacity);
    result.extend_from_slice(&bytes);
    assert_eq!(result.capacity(), capacity);
    result
}

struct Observation {
    writes: usize,
    promoted_index: Option<usize>,
    recovered_index: Option<usize>,
    original_slot_matches: bool,
}

fn exercise(name: &str, mut packets: Vec<BytesMut>, scratch: &mut Option<BytesMut>) -> Observation {
    let original: Vec<Vec<u8>> = packets
        .iter()
        .map(|packet| packet[VIRTIO_NET_HDR_LEN..].to_vec())
        .collect();
    let largest = original.iter().map(Vec::len).max().unwrap_or(1);
    let promoted = promote(&mut packets, scratch);
    // Record allocation identity/capacity before the real library mutates frames.
    let mut allocations: Vec<_> = packets
        .iter()
        .map(|packet| (packet.as_ptr() as usize, packet.capacity()))
        .collect();
    allocations.sort_unstable();
    let mut to_write = Vec::new();
    handle_gro(
        &mut packets,
        VIRTIO_NET_HDR_LEN,
        &mut Default::default(),
        &mut Default::default(),
        false,
        &mut to_write,
    )
    .unwrap_or_else(|error| panic!("{name}: GRO rejected fixture: {error}"));

    let mut observed_allocations: Vec<_> = packets
        .iter()
        .map(|packet| (packet.as_ptr() as usize, packet.capacity()))
        .collect();
    observed_allocations.sort_unstable();
    assert_eq!(
        observed_allocations, allocations,
        "{name}: GRO allocation changed"
    );

    // Use the actual emission indices, then reverse GRO through locked gso_split.
    // Comparing every resulting IP byte detects lost/duplicated payload, changed
    // ports, flags, sequence numbers, lengths, and IP/TCP checksums.
    let mut output = Vec::new();
    for &index in &to_write {
        let header = VirtioNetHdr::decode(&packets[index][..VIRTIO_NET_HDR_LEN]).unwrap();
        let mut packet = packets[index][VIRTIO_NET_HDR_LEN..].to_vec();
        if header.gso_type == 0 {
            output.push(packet);
        } else {
            let mut split = vec![vec![0_u8; largest]; original.len()];
            let mut sizes = vec![0; original.len()];
            let v6 = packet[0] >> 4 == 6;
            let count = gso_split(&mut packet, header, &mut split, &mut sizes, 0, v6).unwrap();
            for (mut segment, size) in split.into_iter().zip(sizes).take(count) {
                segment.truncate(size);
                output.push(segment);
            }
        }
    }
    let mut expected = original;
    expected.sort();
    output.sort();
    assert_eq!(
        output, expected,
        "{name}: GRO/GSO changed packet bytes or multiplicity"
    );

    let recovered_index = promoted.map(|(identity, _)| {
        packets
            .iter()
            .position(|packet| packet.as_ptr() as usize == identity)
            .unwrap()
    });
    let original_slot_matches =
        promoted.is_some_and(|(identity, index)| packets[index].as_ptr() as usize == identity);
    if let Some((identity, _)) = promoted {
        *scratch = Some(reclaim(&mut packets, identity));
    }
    if !name.is_empty() {
        println!(
            "{{\"contract\":\"tun-head\",\"case\":\"{name}\",\"input_packets\":{},\"writes\":{},\"byte_roundtrip\":true,\"allocation_capacity_unchanged\":true}}",
            expected.len(),
            to_write.len()
        );
    }
    Observation {
        writes: to_write.len(),
        promoted_index: promoted.map(|(_, index)| index),
        recovered_index,
        original_slot_matches,
    }
}

pub(super) fn run() {
    let mut scratch = Some(BytesMut::with_capacity(HEAD_CAPACITY));
    let identity = scratch.as_ref().unwrap().as_ptr() as usize;
    for v6 in [false, true] {
        let family = if v6 { 6 } else { 4 };
        let pair = || {
            vec![
                frame(v6, 0, 0, PAYLOAD, 0x10, false),
                frame(v6, 1, 0, PAYLOAD, 0x10, false),
            ]
        };
        let red = exercise(&format!("ipv{family}-no-head-control"), pair(), &mut None);
        assert_ne!(
            red.writes, 1,
            "negative control must fail the one-write contract"
        );
        let green = exercise(&format!("ipv{family}-head"), pair(), &mut scratch);
        assert_eq!(
            green.writes, 1,
            "bounded head must satisfy the same one-write contract"
        );
        println!(
            "{{\"control\":\"ipv{family}-one-write\",\"baseline_predicate\":false,\"bounded_head_predicate\":true}}"
        );

        for count in [0, 1, 4, 8, 32, 128] {
            let packets = (0..count)
                .map(|ordinal| frame(v6, ordinal, 0, PAYLOAD, 0x10, false))
                .collect();
            let result = exercise(
                &format!("ipv{family}-cohort-{count}"),
                packets,
                &mut scratch,
            );
            if count < 2 {
                assert!(result.promoted_index.is_none());
                assert_eq!(result.writes, count as usize);
            }
        }
        let prepend = exercise(
            &format!("ipv{family}-prepend-two-wide-allocations"),
            vec![
                frame(v6, 1, 0, PAYLOAD, 0x10, true),
                frame(v6, 0, 0, PAYLOAD, 0x10, false),
            ],
            &mut scratch,
        );
        assert_eq!(prepend.writes, 1);
        assert_eq!(prepend.promoted_index, Some(1));
        assert_eq!(prepend.recovered_index, Some(0));
        assert!(
            !prepend.original_slot_matches,
            "fixed-index recovery negative control must fail"
        );
        exercise(
            &format!("ipv{family}-reverse-narrow"),
            vec![
                frame(v6, 1, 0, PAYLOAD, 0x10, false),
                frame(v6, 0, 0, PAYLOAD, 0x10, false),
            ],
            &mut scratch,
        );
        exercise(
            &format!("ipv{family}-interleaved-flows"),
            (0..4)
                .flat_map(|ordinal| {
                    [
                        frame(v6, ordinal, 0, PAYLOAD, 0x10, false),
                        frame(v6, ordinal, 1, PAYLOAD, 0x10, false),
                    ]
                })
                .collect(),
            &mut scratch,
        );
        exercise(
            &format!("ipv{family}-short-psh-tail"),
            vec![
                frame(v6, 0, 0, PAYLOAD, 0x10, false),
                frame(v6, 1, 0, 111, 0x18, false),
            ],
            &mut scratch,
        );
        for flags in [0x02, 0x04, 0x11, 0x18] {
            exercise(
                &format!("ipv{family}-flags-{flags}"),
                vec![
                    frame(v6, 0, 0, PAYLOAD, flags, false),
                    frame(v6, 1, 0, PAYLOAD, 0x10, false),
                ],
                &mut scratch,
            );
        }
        exercise(
            &format!("ipv{family}-ack-only"),
            vec![
                frame(v6, 0, 0, 0, 0x10, false),
                frame(v6, 0, 0, PAYLOAD, 0x10, false),
            ],
            &mut scratch,
        );
        let mut corrupt = frame(v6, 0, 0, PAYLOAD, 0x10, false);
        *corrupt.last_mut().unwrap() ^= 1;
        let bad = exercise(
            &format!("ipv{family}-bad-checksum"),
            vec![corrupt, frame(v6, 1, 0, PAYLOAD, 0x10, false)],
            &mut scratch,
        );
        assert_eq!(
            bad.writes, 2,
            "invalid checksum must not be repaired or merged"
        );
        let oversized = exercise(
            &format!("ipv{family}-larger-than-head"),
            vec![
                frame(v6, 0, 0, HEAD_CAPACITY, 0x10, false),
                frame(v6, 0, 1, HEAD_CAPACITY, 0x10, false),
            ],
            &mut scratch,
        );
        assert!(oversized.promoted_index.is_none());
        assert_eq!(oversized.writes, 2);

        let pieces = pair();
        let mut slab = BytesMut::new();
        for packet in &pieces {
            slab.extend_from_slice(packet);
            slab.extend_from_slice(&[0; AEAD_TAIL]);
        }
        slab.extend_from_slice(b"live unrelated slab tail");
        let packets = pieces
            .iter()
            .map(|packet| {
                let mut part = slab.split_to(packet.len() + AEAD_TAIL);
                part.truncate(packet.len());
                part
            })
            .collect();
        exercise(
            &format!("ipv{family}-shared-receive-slab"),
            packets,
            &mut scratch,
        );
        assert_eq!(&slab[..], b"live unrelated slab tail");
    }

    let mut malformed = vec![
        frame(false, 0, 0, PAYLOAD, 0x10, false),
        BytesMut::from(&[0_u8; VIRTIO_NET_HDR_LEN][..]),
    ];
    let promoted = promote(&mut malformed, &mut scratch).unwrap();
    let error = handle_gro(
        &mut malformed,
        VIRTIO_NET_HDR_LEN,
        &mut Default::default(),
        &mut Default::default(),
        false,
        &mut Vec::new(),
    )
    .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    scratch = Some(reclaim(&mut malformed, promoted.0));
    println!(
        "{{\"contract\":\"tun-head\",\"case\":\"gro-error-recovery\",\"original_error\":\"InvalidInput\",\"reclaimed\":true}}"
    );

    for iteration in 0..1000 {
        let v6 = iteration % 2 == 0;
        exercise(
            "",
            vec![
                frame(v6, 0, 0, PAYLOAD, 0x10, false),
                frame(v6, 1, 0, PAYLOAD, 0x10, false),
            ],
            &mut scratch,
        );
        let head = scratch.as_ref().unwrap();
        assert_eq!(head.as_ptr() as usize, identity);
        assert_eq!(head.capacity(), HEAD_CAPACITY);
        assert!(head.is_empty());
    }
    println!(
        "{{\"contract\":\"tun-head\",\"case\":\"reuse\",\"iterations\":1000,\"same_allocation\":true,\"head_capacity\":8192}}"
    );
}
