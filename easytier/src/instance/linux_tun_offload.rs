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

// One allocation per Linux offload sink, not per packet/flow. Small decrypted
// slices otherwise prevent tun-rs from coalescing even an already queued pair.
// Keep the measured 8 KiB bound; do not grow queues or wait to form a cohort.
const GRO_HEAD_CAPACITY: usize = 8192;

fn promote_gro_head(packets: &mut [BytesMut], head: &mut Option<BytesMut>) -> Option<usize> {
    if packets.len() < 2 || head.is_none() {
        return None;
    }
    let index = packets.iter().position(|frame| {
        if frame.len() >= GRO_HEAD_CAPACITY || frame.capacity() >= GRO_HEAD_CAPACITY {
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
    let mut buffer = head.take()?;
    buffer.clear();
    buffer.extend_from_slice(&packets[index]);
    let identity = buffer.as_ptr() as usize;
    packets[index] = buffer;
    Some(identity)
}

fn reclaim_gro_head(packets: &mut Vec<BytesMut>, identity: usize) -> Option<BytesMut> {
    // tun-rs may swap slots when prepending. Equal capacities are not identities.
    let index = packets
        .iter()
        .position(|packet| packet.as_ptr() as usize == identity)?;
    let mut head = packets.swap_remove(index);
    head.clear();
    Some(head)
}

#[cfg(test)]
#[path = "linux_tun_offload_tests.rs"]
mod tests;

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

type FlushFuture =
    Pin<Box<dyn Future<Output = (io::Result<usize>, GROTable, Vec<BytesMut>)> + Send>>;

pub(crate) struct LinuxTunOffloadSink {
    device: Arc<AsyncDevice>,
    pending: Vec<BytesMut>,
    gro: Option<GROTable>,
    flush_future: Option<FlushFuture>,
    gro_head: Option<BytesMut>,
    gro_head_identity: Option<usize>,
}

impl LinuxTunOffloadSink {
    pub(crate) fn new(device: Arc<AsyncDevice>) -> Self {
        Self {
            device,
            pending: Vec::with_capacity(IDEAL_BATCH_SIZE),
            gro: Some(GROTable::new()),
            flush_future: None,
            gro_head: Some(BytesMut::with_capacity(GRO_HEAD_CAPACITY)),
            gro_head_identity: None,
        }
    }

    fn poll_flush_inner(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), SinkError>> {
        if self.flush_future.is_none() {
            if self.pending.is_empty() {
                return Poll::Ready(Ok(()));
            }
            let device = self.device.clone();
            let mut packets = std::mem::take(&mut self.pending);
            self.gro_head_identity = promote_gro_head(&mut packets, &mut self.gro_head);
            let mut gro = self.gro.take().expect("offload GRO state missing");
            self.flush_future = Some(Box::pin(async move {
                let result = device
                    .send_multiple(&mut gro, &mut packets, VIRTIO_NET_HDR_LEN)
                    .await;
                (result, gro, packets)
            }));
        }

        let (result, gro, mut packets) = ready!(
            self.flush_future
                .as_mut()
                .expect("offload flush future missing")
                .as_mut()
                .poll(cx)
        );
        self.flush_future = None;
        // The stored future owns the head until all attempted writes complete,
        // even if a caller cancels its flush. A partial write followed by an error
        // must not replay the cohort; preserve send_multiple's original result.
        if let Some(identity) = self.gro_head_identity.take() {
            self.gro_head = reclaim_gro_head(&mut packets, identity);
            if self.gro_head.is_none() {
                // Fail back to ordinary packet buffers, without repeated allocations.
                tracing::warn!("TUN GRO head was not retained; disabling head promotion");
            }
        }
        packets.clear();
        self.pending = packets;
        self.gro = Some(gro);
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
