use std::{
    collections::VecDeque,
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use bytes::BytesMut;
use futures::{Sink, Stream, ready};
use tun_rs::{AsyncDevice, GROTable, IDEAL_BATCH_SIZE, VIRTIO_NET_HDR_LEN};

use crate::tunnel::{
    SinkError, StreamItem,
    packet_def::{TAIL_RESERVED_SIZE, ZCPacket, ZCPacketType},
};

const MAX_PACKET_SIZE: usize = 4096;
const MAX_GSO_FRAME_SIZE: usize = VIRTIO_NET_HDR_LEN + 65535;
const GRO_SCRATCH_CAPACITY: usize = MAX_GSO_FRAME_SIZE + VIRTIO_NET_HDR_LEN;

struct ReadBatch {
    original: Vec<u8>,
    packets: Vec<BytesMut>,
    sizes: Vec<usize>,
}

type ReadFuture = Pin<Box<dyn Future<Output = (io::Result<usize>, ReadBatch)> + Send>>;

pub(crate) struct LinuxTunOffloadStream {
    device: Arc<AsyncDevice>,
    payload_offset: usize,
    state: Option<ReadBatch>,
    read_future: Option<ReadFuture>,
    pending: VecDeque<ZCPacket>,
}

impl LinuxTunOffloadStream {
    pub(crate) fn new(device: Arc<AsyncDevice>) -> Self {
        let payload_offset = ZCPacketType::NIC.get_packet_offsets().payload_offset;
        let packets = (0..IDEAL_BATCH_SIZE)
            .map(|_| {
                let mut packet = BytesMut::with_capacity(payload_offset + MAX_PACKET_SIZE);
                packet.resize(payload_offset + MAX_PACKET_SIZE, 0);
                packet
            })
            .collect();
        Self {
            device,
            payload_offset,
            state: Some(ReadBatch {
                original: vec![0; MAX_GSO_FRAME_SIZE],
                packets,
                sizes: vec![0; IDEAL_BATCH_SIZE],
            }),
            read_future: None,
            pending: VecDeque::new(),
        }
    }
}

impl Stream for LinuxTunOffloadStream {
    type Item = StreamItem;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(packet) = self.pending.pop_front() {
            return Poll::Ready(Some(Ok(packet)));
        }

        if self.read_future.is_none() {
            let mut state = self.state.take().expect("offload read state missing");
            let device = self.device.clone();
            let payload_offset = self.payload_offset;
            self.read_future = Some(Box::pin(async move {
                let result = device
                    .recv_multiple(
                        &mut state.original,
                        &mut state.packets,
                        &mut state.sizes,
                        payload_offset,
                    )
                    .await;
                (result, state)
            }));
        }

        let (result, mut state) = ready!(
            self.read_future
                .as_mut()
                .expect("offload read future missing")
                .as_mut()
                .poll(cx)
        );
        self.read_future = None;

        match result {
            Ok(count) => {
                for index in 0..count {
                    let size = state.sizes[index];
                    if size == 0 || self.payload_offset + size > state.packets[index].len() {
                        self.state = Some(state);
                        return Poll::Ready(Some(Err(SinkError::InvalidPacket(
                            "invalid packet size returned by TUN GSO splitter".to_string(),
                        ))));
                    }

                    let mut replacement = BytesMut::with_capacity(
                        self.payload_offset + MAX_PACKET_SIZE + TAIL_RESERVED_SIZE,
                    );
                    replacement.resize(self.payload_offset + MAX_PACKET_SIZE, 0);
                    let mut packet = std::mem::replace(&mut state.packets[index], replacement);
                    packet.truncate(self.payload_offset + size);
                    self.pending
                        .push_back(ZCPacket::new_from_buf(packet, ZCPacketType::NIC));
                }
                self.state = Some(state);
                Poll::Ready(self.pending.pop_front().map(Ok))
            }
            Err(error) => {
                self.state = Some(state);
                Poll::Ready(Some(Err(error.into())))
            }
        }
    }
}

type FlushFuture = Pin<
    Box<dyn Future<Output = (io::Result<usize>, GROTable, Vec<BytesMut>, Option<BytesMut>)> + Send>,
>;

fn is_tcp_gro_candidate(frame: &BytesMut) -> bool {
    let Some(packet) = frame.get(VIRTIO_NET_HDR_LEN..) else {
        return false;
    };
    match packet.first().map(|byte| byte >> 4) {
        Some(4) => packet.len() >= 20 && packet[0] & 0x0f == 5 && packet.get(9).copied() == Some(6),
        Some(6) => packet.len() >= 40 && packet.get(6).copied() == Some(6),
        _ => false,
    }
}

fn prepare_gro_scratch(packets: &mut [BytesMut], scratch: &mut Option<BytesMut>) -> bool {
    // Existing 4 KiB packet buffers can already merge two MTU-sized segments.
    // Promote only a larger batch's first TCP flow head, avoiding a copy for
    // interactive/single-packet traffic and for protocols that cannot use TCP GRO.
    if packets.len() < 3 {
        return false;
    }
    let Some(index) = packets.iter().position(is_tcp_gro_candidate) else {
        return false;
    };
    let Some(mut reusable) = scratch.take() else {
        return false;
    };
    reusable.clear();
    reusable.extend_from_slice(&packets[index]);
    packets[index] = reusable;
    true
}

fn recover_gro_scratch(packets: &mut Vec<BytesMut>) -> Option<BytesMut> {
    let index = packets
        .iter()
        .position(|packet| packet.capacity() >= GRO_SCRATCH_CAPACITY)?;
    let mut scratch = packets.swap_remove(index);
    scratch.clear();
    Some(scratch)
}

pub(crate) struct LinuxTunOffloadSink {
    device: Arc<AsyncDevice>,
    pending: Vec<BytesMut>,
    gro: Option<GROTable>,
    gro_scratch: Option<BytesMut>,
    flush_future: Option<FlushFuture>,
}

impl LinuxTunOffloadSink {
    pub(crate) fn new(device: Arc<AsyncDevice>) -> Self {
        Self {
            device,
            pending: Vec::with_capacity(IDEAL_BATCH_SIZE),
            gro: Some(GROTable::new()),
            gro_scratch: Some(BytesMut::with_capacity(GRO_SCRATCH_CAPACITY)),
            flush_future: None,
        }
    }

    fn poll_flush_inner(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), SinkError>> {
        if self.flush_future.is_none() {
            if self.pending.is_empty() {
                return Poll::Ready(Ok(()));
            }
            let device = self.device.clone();
            let mut packets = std::mem::take(&mut self.pending);
            let scratch_used = prepare_gro_scratch(&mut packets, &mut self.gro_scratch);
            let mut gro = self.gro.take().expect("offload GRO state missing");
            self.flush_future = Some(Box::pin(async move {
                let result = device
                    .send_multiple(&mut gro, &mut packets, VIRTIO_NET_HDR_LEN)
                    .await;
                let scratch = scratch_used
                    .then(|| recover_gro_scratch(&mut packets))
                    .flatten();
                (result, gro, packets, scratch)
            }));
        }

        let (result, gro, mut packets, scratch) = ready!(
            self.flush_future
                .as_mut()
                .expect("offload flush future missing")
                .as_mut()
                .poll(cx)
        );
        self.flush_future = None;
        packets.clear();
        self.pending = packets;
        self.gro = Some(gro);
        if let Some(scratch) = scratch {
            self.gro_scratch = Some(scratch);
        }
        result.map(|_| ()).map_err(Into::into).into()
    }
}

impl Sink<ZCPacket> for LinuxTunOffloadSink {
    type Error = SinkError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.pending.len() < IDEAL_BATCH_SIZE {
            Poll::Ready(Ok(()))
        } else {
            self.poll_flush_inner(cx)
        }
    }

    fn start_send(mut self: Pin<&mut Self>, packet: ZCPacket) -> Result<(), Self::Error> {
        let payload_offset = packet.payload_offset();
        if payload_offset < VIRTIO_NET_HDR_LEN {
            return Err(SinkError::InvalidPacket(
                "insufficient packet headroom for virtio-net header".to_string(),
            ));
        }
        let mut inner = packet.inner();
        let mut frame = inner.split_off(payload_offset - VIRTIO_NET_HDR_LEN);
        frame[..VIRTIO_NET_HDR_LEN].fill(0);
        self.pending.push(frame);
        Ok(())
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_flush_inner(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_flush_inner(cx)
    }
}

pub(crate) fn create(
    name: Option<&str>,
    mtu: u32,
    configure_up: bool,
) -> io::Result<(String, LinuxTunOffloadStream, LinuxTunOffloadSink)> {
    let mtu = u16::try_from(mtu)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "TUN MTU exceeds u16"))?;
    let mut builder = tun_rs::DeviceBuilder::new()
        .mtu(mtu)
        .enable(configure_up)
        .offload(true)
        .packet_information(false);
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        builder = builder.name(name);
    }
    let device = Arc::new(builder.build_async()?);
    if !device.tcp_gso() {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Linux TUN TCP GSO was not enabled",
        ));
    }
    let name = device.name()?;
    Ok((
        name,
        LinuxTunOffloadStream::new(device.clone()),
        LinuxTunOffloadSink::new(device),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(version: u8, protocol: u8) -> BytesMut {
        let header_len = if version == 6 { 40 } else { 20 };
        let mut packet = BytesMut::with_capacity(4096);
        packet.resize(VIRTIO_NET_HDR_LEN + header_len + 64, 0);
        packet[VIRTIO_NET_HDR_LEN] = version << 4 | if version == 4 { 5 } else { 0 };
        packet[VIRTIO_NET_HDR_LEN + if version == 6 { 6 } else { 9 }] = protocol;
        packet
    }

    #[test]
    fn gro_scratch_only_promotes_multi_packet_tcp_batches_and_is_reusable() {
        let mut scratch = Some(BytesMut::with_capacity(GRO_SCRATCH_CAPACITY));
        let mut short_batch = vec![packet(4, 6), packet(4, 6)];
        assert!(!prepare_gro_scratch(&mut short_batch, &mut scratch));
        assert!(scratch.is_some());

        let mut batch = vec![packet(4, 17), packet(4, 6), packet(6, 6)];
        let expected = batch[1].clone();
        assert!(prepare_gro_scratch(&mut batch, &mut scratch));
        assert!(scratch.is_none());
        assert_eq!(batch[1], expected);
        assert!(batch[1].capacity() >= GRO_SCRATCH_CAPACITY);

        scratch = recover_gro_scratch(&mut batch);
        assert_eq!(scratch.as_ref().map(BytesMut::len), Some(0));
        assert_eq!(batch.len(), 2);
    }

    #[test]
    fn gro_scratch_ignores_batches_without_tcp_candidates() {
        let mut scratch = Some(BytesMut::with_capacity(GRO_SCRATCH_CAPACITY));
        let mut batch = vec![packet(4, 17), packet(6, 17), packet(4, 1)];
        assert!(!prepare_gro_scratch(&mut batch, &mut scratch));
        assert!(scratch.is_some());
        assert!(
            batch
                .iter()
                .all(|packet| packet.capacity() < GRO_SCRATCH_CAPACITY)
        );
    }
}
