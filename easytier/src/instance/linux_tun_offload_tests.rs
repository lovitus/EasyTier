use super::*;
use futures::{FutureExt, SinkExt};
use std::time::Duration;
use tun_rs::{VirtioNetHdr, gso_split, handle_gro};

const PAYLOAD: usize = 1320;

fn checksum(chunks: &[&[u8]]) -> u16 {
    let mut sum = 0_u32;
    let mut high = None;
    for &byte in chunks.iter().flat_map(|chunk| chunk.iter()) {
        if let Some(first) = high.take() {
            sum += u16::from_be_bytes([first, byte]) as u32;
        } else {
            high = Some(byte);
        }
    }
    if let Some(first) = high {
        sum += (first as u32) << 8;
    }
    while sum > u16::MAX as u32 {
        sum = (sum & u16::MAX as u32) + (sum >> 16);
    }
    !(sum as u16)
}

fn tcp_frame(v6: bool, ordinal: u32, wide: bool) -> BytesMut {
    let ip_len = if v6 { 40 } else { 20 };
    let tcp_len = 20 + PAYLOAD;
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
        let sum = checksum(&[ip]);
        ip[10..12].copy_from_slice(&sum.to_be_bytes());
        let mut pseudo = ip[12..20].to_vec();
        pseudo.extend_from_slice(&[0, 6]);
        pseudo.extend_from_slice(&(tcp_len as u16).to_be_bytes());
        pseudo
    };
    let tcp = &mut bytes[VIRTIO_NET_HDR_LEN + ip_len..];
    tcp[0..2].copy_from_slice(&40000_u16.to_be_bytes());
    tcp[2..4].copy_from_slice(&5201_u16.to_be_bytes());
    tcp[4..8].copy_from_slice(&ordinal.wrapping_mul(PAYLOAD as u32).to_be_bytes());
    tcp[8..12].copy_from_slice(&1_u32.to_be_bytes());
    tcp[12] = 5 << 4;
    tcp[13] = 0x10;
    tcp[14..16].copy_from_slice(&u16::MAX.to_be_bytes());
    for (index, byte) in tcp[20..].iter_mut().enumerate() {
        *byte = ordinal.wrapping_mul(31).wrapping_add(index as u32) as u8;
    }
    let sum = checksum(&[&pseudo, tcp]);
    tcp[16..18].copy_from_slice(&sum.to_be_bytes());
    let mut packet = BytesMut::with_capacity(if wide {
        GRO_HEAD_CAPACITY
    } else {
        bytes.len() + 28 // Reserved AEAD tail, not spare GRO capacity.
    });
    packet.extend_from_slice(&bytes);
    packet
}

fn round_trip(mut packets: Vec<BytesMut>, head: &mut Option<BytesMut>) -> usize {
    let mut expected: Vec<_> = packets
        .iter()
        .map(|packet| packet[VIRTIO_NET_HDR_LEN..].to_vec())
        .collect();
    let identity = promote_gro_head(&mut packets, head);
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
    .unwrap();
    let mut after: Vec<_> = packets
        .iter()
        .map(|packet| (packet.as_ptr() as usize, packet.capacity()))
        .collect();
    after.sort_unstable();
    assert_eq!(after, allocations, "GRO must not grow/reallocate a frame");

    let mut output = Vec::new();
    for &index in &to_write {
        let header = VirtioNetHdr::decode(&packets[index][..VIRTIO_NET_HDR_LEN]).unwrap();
        let mut packet = packets[index][VIRTIO_NET_HDR_LEN..].to_vec();
        if header.gso_type == 0 {
            output.push(packet);
        } else {
            let mut split = vec![vec![0; 1500]; expected.len()];
            let mut sizes = vec![0; expected.len()];
            let v6 = packet[0] >> 4 == 6;
            let count = gso_split(&mut packet, header, &mut split, &mut sizes, 0, v6).unwrap();
            for (mut packet, size) in split.into_iter().zip(sizes).take(count) {
                packet.truncate(size);
                output.push(packet);
            }
        }
    }
    expected.sort();
    output.sort();
    assert_eq!(
        output, expected,
        "all packet bytes and multiplicities must survive"
    );
    if let Some(identity) = identity {
        *head = reclaim_gro_head(&mut packets, identity);
        let recovered = head.as_ref().expect("head must be retained");
        assert_eq!(recovered.as_ptr() as usize, identity);
        assert_eq!(recovered.capacity(), GRO_HEAD_CAPACITY);
        assert!(recovered.is_empty());
    }
    to_write.len()
}

#[test]
fn bounded_gro_head_preserves_packets_and_allocation() {
    let mut head = Some(BytesMut::with_capacity(GRO_HEAD_CAPACITY));
    let identity = head.as_ref().unwrap().as_ptr() as usize;
    for v6 in [false, true] {
        let pair = || vec![tcp_frame(v6, 0, false), tcp_frame(v6, 1, false)];
        // Negative control removes only the optimization, preserving the real
        // library and packet data. The one-emission predicate is then false.
        assert_eq!(round_trip(pair(), &mut None), 2);
        assert_eq!(round_trip(pair(), &mut head), 1);
        for count in [0, 1, 4, 8, 32, 128] {
            round_trip(
                (0..count)
                    .map(|ordinal| tcp_frame(v6, ordinal, false))
                    .collect(),
                &mut head,
            );
        }
        let mut corrupt = tcp_frame(v6, 0, false);
        *corrupt.last_mut().unwrap() ^= 1;
        assert_eq!(
            round_trip(vec![corrupt, tcp_frame(v6, 1, false)], &mut head),
            2
        );
        let mut slab = BytesMut::new();
        let frames = pair();
        for packet in &frames {
            slab.extend_from_slice(packet);
            slab.extend_from_slice(&[0; 28]);
        }
        slab.extend_from_slice(b"unrelated live slab tail");
        let shared = frames
            .iter()
            .map(|packet| {
                let mut part = slab.split_to(packet.len() + 28);
                part.truncate(packet.len());
                part
            })
            .collect();
        assert_eq!(round_trip(shared, &mut head), 1);
        assert_eq!(&slab[..], b"unrelated live slab tail");
        for _ in 0..64 {
            assert_eq!(round_trip(pair(), &mut head), 1);
            assert_eq!(head.as_ref().unwrap().as_ptr() as usize, identity);
        }
    }
}

#[test]
fn gro_head_reclaims_by_identity_after_prepend_and_gro_error() {
    for v6 in [false, true] {
        let mut head = Some(BytesMut::with_capacity(GRO_HEAD_CAPACITY));
        let mut packets = vec![tcp_frame(v6, 1, true), tcp_frame(v6, 0, false)];
        let identity = promote_gro_head(&mut packets, &mut head).unwrap();
        assert_eq!(packets[1].as_ptr() as usize, identity);
        GROTable::new()
            .apply_gro(&mut packets, VIRTIO_NET_HDR_LEN, false)
            .unwrap();
        assert_eq!(packets[0].as_ptr() as usize, identity);
        // Fixed-index or capacity-only recovery would select the other head.
        assert_ne!(packets[1].as_ptr() as usize, identity);
        assert_eq!(packets[1].capacity(), GRO_HEAD_CAPACITY);
        head = reclaim_gro_head(&mut packets, identity);
        assert_eq!(head.as_ref().unwrap().as_ptr() as usize, identity);

        let mut truncated = BytesMut::new();
        truncated.resize(VIRTIO_NET_HDR_LEN, 0);
        let mut packets = vec![tcp_frame(v6, 0, false), truncated];
        let identity = promote_gro_head(&mut packets, &mut head).unwrap();
        let error = GROTable::new()
            .apply_gro(&mut packets, VIRTIO_NET_HDR_LEN, false)
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        let recovered = reclaim_gro_head(&mut packets, identity).unwrap();
        assert_eq!(recovered.as_ptr() as usize, identity);
        assert_eq!(recovered.capacity(), GRO_HEAD_CAPACITY);
        assert!(recovered.is_empty());
    }
}

fn in_test_netns(body: impl Future<Output = ()> + Send + 'static) {
    std::thread::spawn(move || {
        nix::sched::unshare(nix::sched::CloneFlags::CLONE_NEWNET).unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(body);
    })
    .join()
    .unwrap();
}

fn device(enabled: bool) -> Arc<AsyncDevice> {
    let name = format!("etgh{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
    let device = tun_rs::DeviceBuilder::new()
        .name(&name)
        .mtu(1360)
        .enable(enabled)
        .offload(true)
        .packet_information(false)
        .build_async()
        .unwrap();
    assert!(device.tcp_gso());
    Arc::new(device)
}

fn rx_packets(name: &str) -> Option<u64> {
    // thread-self matters: this test thread owns a private network namespace.
    std::fs::read_to_string("/proc/thread-self/net/dev")
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(interface, _)| interface.trim() == name)
        .map(|(_, fields)| fields.split_whitespace().nth(1).unwrap().parse().unwrap())
}

#[test]
fn tun_partial_write_error_reclaims_head_without_replay() {
    in_test_netns(async {
        let device = device(true);
        let name = device.name().unwrap();
        for promoted in [false, true] {
            let before = rx_packets(&name).unwrap();
            let mut sink = LinuxTunOffloadSink::new(device.clone());
            if !promoted {
                sink.gro_head = None; // Real-kernel negative control: original capacity.
            }
            let identity = sink.gro_head.as_ref().map(|head| head.as_ptr() as usize);
            sink.pending
                .extend([tcp_frame(false, 0, false), tcp_frame(false, 1, false)]);
            let mut invalid = BytesMut::new();
            invalid.resize(VIRTIO_NET_HDR_LEN + 20, 0); // Invalid IP version, after valid writes.
            sink.pending.push(invalid);
            let error = tokio::time::timeout(Duration::from_secs(5), sink.flush())
                .await
                .unwrap()
                .unwrap_err();
            assert!(
                matches!(error, SinkError::IOError(ref error) if error.raw_os_error() == Some(nix::libc::EINVAL)),
                "{error:?}"
            );
            let after = rx_packets(&name).unwrap();
            assert_eq!(after - before, if promoted { 1 } else { 2 });
            assert!(sink.pending.is_empty());
            assert!(sink.flush_future.is_none());
            assert!(sink.gro_head_identity.is_none());
            assert_eq!(
                sink.gro_head.as_ref().map(|head| head.as_ptr() as usize),
                identity
            );
            sink.flush().await.unwrap();
            sink.close().await.unwrap();
            assert_eq!(
                rx_packets(&name),
                Some(after),
                "error must not replay successful writes"
            );
        }
        drop(device);
        assert_eq!(rx_packets(&name), None, "last owner must release its TUN");
    });
}

#[test]
fn tun_pending_flush_survives_cancellation_and_releases_on_drop() {
    in_test_netns(async {
        for resume in [true, false] {
            let device = device(false); // A down TUN does not report EPOLLOUT.
            let name = device.name().unwrap();
            let weak = Arc::downgrade(&device);
            let mut sink = LinuxTunOffloadSink::new(device.clone());
            let identity = sink.gro_head.as_ref().unwrap().as_ptr() as usize;
            sink.pending
                .extend([tcp_frame(false, 0, false), tcp_frame(false, 1, false)]);
            // Consume/drop only the caller's flush, not the sink's stored future.
            assert!(sink.flush().now_or_never().is_none());
            assert!(sink.flush_future.is_some());
            assert!(sink.gro_head.is_none());
            assert_eq!(sink.gro_head_identity, Some(identity));
            if resume {
                device.enabled(true).unwrap();
                tokio::time::timeout(Duration::from_secs(5), sink.flush())
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(rx_packets(&name), Some(1));
                assert_eq!(sink.gro_head.as_ref().unwrap().as_ptr() as usize, identity);
                sink.close().await.unwrap();
                assert_eq!(rx_packets(&name), Some(1));
            }
            drop(device);
            drop(sink);
            assert!(
                weak.upgrade().is_none(),
                "no detached write task may retain the device"
            );
            assert_eq!(rx_packets(&name), None);
        }
    });
}
