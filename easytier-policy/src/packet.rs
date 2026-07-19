use std::{net::IpAddr, sync::Arc};

use arc_swap::ArcSwap;
use cidr::{IpCidr, Ipv4Cidr, Ipv6Cidr};
use prefix_trie::PrefixSet;
use thiserror::Error;

const MIN_IPV4_HEADER: usize = 20;
const MIN_IPV6_HEADER: usize = 40;
const MAX_PACKET_SIZE: usize = 65_535;

#[derive(Debug, Error)]
pub enum PacketError {
    #[error("empty packet")]
    Empty,
    #[error("truncated IPv{version} packet: {length} bytes")]
    Truncated { version: u8, length: usize },
    #[error("unsupported IP version {0}")]
    UnsupportedVersion(u8),
    #[error("malformed IPv{version} packet")]
    Malformed { version: u8 },
    #[error("packet exceeds {MAX_PACKET_SIZE} bytes")]
    TooLarge,
    #[cfg(unix)]
    #[error("packet bridge I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketClass {
    Mesh,
    Policy,
}

#[derive(Debug, Clone, Default)]
pub struct MeshRouteSnapshot {
    routes: Arc<[IpCidr]>,
    ipv4: PrefixSet<Ipv4Cidr>,
    ipv6: PrefixSet<Ipv6Cidr>,
}

impl MeshRouteSnapshot {
    pub fn new(mut routes: Vec<IpCidr>) -> Self {
        routes.sort_unstable_by(|left, right| {
            right
                .network_length()
                .cmp(&left.network_length())
                .then_with(|| left.to_string().cmp(&right.to_string()))
        });
        routes.dedup();
        let mut ipv4 = PrefixSet::new();
        let mut ipv6 = PrefixSet::new();
        for route in &routes {
            match route {
                IpCidr::V4(route) => {
                    ipv4.insert(*route);
                }
                IpCidr::V6(route) => {
                    ipv6.insert(*route);
                }
            }
        }
        Self {
            routes: routes.into(),
            ipv4,
            ipv6,
        }
    }

    pub fn owns(&self, destination: IpAddr) -> bool {
        match destination {
            IpAddr::V4(address) => {
                address.is_broadcast()
                    || address.is_multicast()
                    || self
                        .ipv4
                        .get_lpm(&Ipv4Cidr::new(address, 32).expect("host prefix is valid"))
                        .is_some()
            }
            IpAddr::V6(address) => {
                address.is_multicast()
                    || self
                        .ipv6
                        .get_lpm(&Ipv6Cidr::new(address, 128).expect("host prefix is valid"))
                        .is_some()
            }
        }
    }

    pub fn routes(&self) -> &[IpCidr] {
        &self.routes
    }
}

pub struct PacketClassifier {
    routes: ArcSwap<MeshRouteSnapshot>,
}

impl PacketClassifier {
    pub fn new(routes: MeshRouteSnapshot) -> Self {
        Self {
            routes: ArcSwap::from_pointee(routes),
        }
    }

    pub fn replace_routes(&self, routes: MeshRouteSnapshot) {
        self.routes.store(Arc::new(routes));
    }

    pub fn classify(&self, packet: &[u8]) -> Result<PacketClass, PacketError> {
        let destination = destination_ip(packet)?;
        Ok(if self.routes.load().owns(destination) {
            PacketClass::Mesh
        } else {
            PacketClass::Policy
        })
    }
}

fn destination_ip(packet: &[u8]) -> Result<IpAddr, PacketError> {
    if packet.len() > MAX_PACKET_SIZE {
        return Err(PacketError::TooLarge);
    }
    let Some(first) = packet.first() else {
        return Err(PacketError::Empty);
    };
    match first >> 4 {
        4 => {
            if packet.len() < MIN_IPV4_HEADER {
                return Err(PacketError::Truncated {
                    version: 4,
                    length: packet.len(),
                });
            }
            let header_length = usize::from(packet[0] & 0x0f) * 4;
            let total_length = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
            if header_length < MIN_IPV4_HEADER
                || header_length > packet.len()
                || total_length < header_length
                || total_length > packet.len()
            {
                return Err(PacketError::Malformed { version: 4 });
            }
            Ok(IpAddr::V4(
                [packet[16], packet[17], packet[18], packet[19]].into(),
            ))
        }
        6 => {
            if packet.len() < MIN_IPV6_HEADER {
                return Err(PacketError::Truncated {
                    version: 6,
                    length: packet.len(),
                });
            }
            let payload_length = usize::from(u16::from_be_bytes([packet[4], packet[5]]));
            if MIN_IPV6_HEADER + payload_length > packet.len() {
                return Err(PacketError::Truncated {
                    version: 6,
                    length: packet.len(),
                });
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&packet[24..40]);
            Ok(IpAddr::V6(octets.into()))
        }
        version => Err(PacketError::UnsupportedVersion(version)),
    }
}

#[cfg(unix)]
mod unix_bridge {
    use std::{
        collections::VecDeque,
        os::fd::{AsRawFd, IntoRawFd, RawFd},
    };

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{UnixDatagram, UnixStream, unix::OwnedReadHalf, unix::OwnedWriteHalf},
        sync::Mutex,
    };

    use super::{MAX_PACKET_SIZE, PacketError};

    const FRAME_MAGIC: [u8; 4] = *b"ETPB";
    const FRAME_VERSION: u8 = 1;
    const FRAME_HEADER_SIZE: usize = 12;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum PacketBridgeBackend {
        Legacy,
        MemoryBatch,
        StreamBatch,
    }

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub enum PacketBridgeMode {
        #[default]
        Legacy,
        PacketBatch,
    }

    impl PacketBridgeMode {
        pub fn for_request(requested: bool, supported: bool) -> Self {
            if requested && supported {
                Self::PacketBatch
            } else {
                Self::Legacy
            }
        }
    }

    pub const PACKET_BATCH_MAX_PACKETS: usize = 32;
    pub const PACKET_BATCH_MAX_BYTES: usize = 64 * 1024;
    #[cfg(feature = "leaf-runtime")]
    const PACKET_BATCH_CHANNEL_CAPACITY: usize = 8;
    pub const LEAF_PACKET_BATCH_EXPERIMENTAL_FEATURE: &str = "leaf-packet-batch";

    struct FramedPacketBatch {
        packets: Vec<Vec<u8>>,
        payload_bytes: usize,
    }

    impl FramedPacketBatch {
        fn try_new(packets: Vec<Vec<u8>>) -> Result<Self, PacketError> {
            if packets.is_empty()
                || packets.len() > PACKET_BATCH_MAX_PACKETS
                || packets
                    .iter()
                    .any(|packet| packet.is_empty() || packet.len() > MAX_PACKET_SIZE)
            {
                return Err(PacketError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "invalid packet batch count or packet length",
                )));
            }
            let payload_bytes = packets.iter().try_fold(0usize, |total, packet| {
                total.checked_add(packet.len()).ok_or_else(|| {
                    PacketError::Io(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "packet batch payload overflow",
                    ))
                })
            })?;
            if payload_bytes > PACKET_BATCH_MAX_BYTES {
                return Err(PacketError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "packet batch payload exceeds 64 KiB",
                )));
            }
            Ok(Self {
                packets,
                payload_bytes,
            })
        }

        fn into_packets(self) -> Vec<Vec<u8>> {
            self.packets
        }
    }

    /// Mux-owned packet endpoint. Datagram semantics preserve packet boundaries.
    pub struct LeafPacketBridge {
        kind: LeafPacketBridgeKind,
    }

    enum LeafPacketBridgeKind {
        Legacy(UnixDatagram),
        Batch(LeafPacketBatchBridge),
    }

    struct LeafPacketBatchBridge {
        backend: PacketBridgeBackend,
        sender: BatchSender,
        receiver: Mutex<BatchReceiver>,
    }

    enum BatchSender {
        #[cfg(feature = "leaf-runtime")]
        Memory(tokio::sync::mpsc::Sender<leaf::PacketBatch>),
        Stream(Mutex<OwnedWriteHalf>),
    }

    struct BatchReceiver {
        source: BatchReceiverSource,
        pending: VecDeque<Vec<u8>>,
    }

    enum BatchReceiverSource {
        #[cfg(feature = "leaf-runtime")]
        Memory(tokio::sync::mpsc::Receiver<leaf::PacketBatch>),
        Stream(OwnedReadHalf),
    }

    enum ReceivedPacketBatch {
        #[cfg(feature = "leaf-runtime")]
        Memory(leaf::PacketBatch),
        Stream(FramedPacketBatch),
    }

    /// Leaf-owned endpoint. Ownership of the FD is transferred exactly once.
    pub struct LeafPacketEndpoint {
        socket: Option<std::os::unix::net::UnixDatagram>,
    }

    pub struct LeafPacketStreamEndpoint {
        socket: Option<std::os::unix::net::UnixStream>,
    }

    impl LeafPacketBridge {
        pub fn pair() -> Result<(Self, LeafPacketEndpoint), PacketError> {
            let (mux, leaf) = std::os::unix::net::UnixDatagram::pair()?;
            mux.set_nonblocking(true)?;
            leaf.set_nonblocking(true)?;
            let socket = UnixDatagram::from_std(mux)?;
            Ok((
                Self {
                    kind: LeafPacketBridgeKind::Legacy(socket),
                },
                LeafPacketEndpoint { socket: Some(leaf) },
            ))
        }

        #[cfg(feature = "leaf-runtime")]
        pub fn memory_batch_pair() -> (Self, leaf::ExternalPacketEndpoint) {
            debug_assert_eq!(PACKET_BATCH_MAX_PACKETS, leaf::MAX_PACKET_BATCH_PACKETS);
            debug_assert_eq!(PACKET_BATCH_MAX_BYTES, leaf::MAX_PACKET_BATCH_BYTES);
            debug_assert_eq!(
                PACKET_BATCH_CHANNEL_CAPACITY,
                leaf::PACKET_BATCH_CHANNEL_CAPACITY
            );
            let capacity = PACKET_BATCH_CHANNEL_CAPACITY;
            let (to_leaf_tx, to_leaf_rx) = tokio::sync::mpsc::channel(capacity);
            let (from_leaf_tx, from_leaf_rx) = tokio::sync::mpsc::channel(capacity);
            (
                Self {
                    kind: LeafPacketBridgeKind::Batch(LeafPacketBatchBridge {
                        backend: PacketBridgeBackend::MemoryBatch,
                        sender: BatchSender::Memory(to_leaf_tx),
                        receiver: Mutex::new(BatchReceiver {
                            source: BatchReceiverSource::Memory(from_leaf_rx),
                            pending: VecDeque::new(),
                        }),
                    }),
                },
                leaf::ExternalPacketEndpoint::new(to_leaf_rx, from_leaf_tx),
            )
        }

        pub fn stream_batch_pair() -> Result<(Self, LeafPacketStreamEndpoint), PacketError> {
            let (mux, leaf) = std::os::unix::net::UnixStream::pair()?;
            mux.set_nonblocking(true)?;
            leaf.set_nonblocking(false)?;
            let (read, write) = UnixStream::from_std(mux)?.into_split();
            Ok((
                Self {
                    kind: LeafPacketBridgeKind::Batch(LeafPacketBatchBridge {
                        backend: PacketBridgeBackend::StreamBatch,
                        sender: BatchSender::Stream(Mutex::new(write)),
                        receiver: Mutex::new(BatchReceiver {
                            source: BatchReceiverSource::Stream(read),
                            pending: VecDeque::new(),
                        }),
                    }),
                },
                LeafPacketStreamEndpoint { socket: Some(leaf) },
            ))
        }

        pub fn backend(&self) -> PacketBridgeBackend {
            match &self.kind {
                LeafPacketBridgeKind::Legacy(_) => PacketBridgeBackend::Legacy,
                LeafPacketBridgeKind::Batch(bridge) => bridge.backend,
            }
        }

        pub fn is_batch(&self) -> bool {
            self.backend() != PacketBridgeBackend::Legacy
        }

        pub async fn send_to_leaf(&self, packet: &[u8]) -> Result<(), PacketError> {
            if packet.is_empty() {
                return Err(PacketError::Empty);
            }
            if packet.len() > MAX_PACKET_SIZE {
                return Err(PacketError::TooLarge);
            }
            match &self.kind {
                LeafPacketBridgeKind::Legacy(socket) => {
                    let sent = socket.send(packet).await?;
                    if sent != packet.len() {
                        return Err(PacketError::Io(std::io::Error::new(
                            std::io::ErrorKind::WriteZero,
                            "partial packet bridge write",
                        )));
                    }
                    Ok(())
                }
                LeafPacketBridgeKind::Batch(_) => {
                    self.send_batch_to_leaf(vec![packet.to_vec()]).await
                }
            }
        }

        pub async fn send_batch_to_leaf(&self, packets: Vec<Vec<u8>>) -> Result<(), PacketError> {
            let batch = FramedPacketBatch::try_new(packets)?;
            let LeafPacketBridgeKind::Batch(bridge) = &self.kind else {
                return Err(PacketError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "batch send requested for legacy packet bridge",
                )));
            };
            match &bridge.sender {
                #[cfg(feature = "leaf-runtime")]
                BatchSender::Memory(sender) => sender
                    .send(leaf::PacketBatch::try_new(batch.into_packets())?)
                    .await
                    .map_err(|_| {
                        PacketError::Io(std::io::Error::new(
                            std::io::ErrorKind::BrokenPipe,
                            "Leaf packet batch input closed",
                        ))
                    }),
                BatchSender::Stream(writer) => {
                    write_batch_async(&mut *writer.lock().await, &batch).await
                }
            }
        }

        pub fn try_send_to_leaf(&self, packet: &[u8]) -> Result<(), PacketError> {
            if packet.is_empty() {
                return Err(PacketError::Empty);
            }
            if packet.len() > MAX_PACKET_SIZE {
                return Err(PacketError::TooLarge);
            }
            match &self.kind {
                LeafPacketBridgeKind::Legacy(socket) => {
                    let sent = socket.try_send(packet)?;
                    if sent != packet.len() {
                        return Err(PacketError::Io(std::io::Error::new(
                            std::io::ErrorKind::WriteZero,
                            "partial packet bridge write",
                        )));
                    }
                    Ok(())
                }
                LeafPacketBridgeKind::Batch(bridge) => match &bridge.sender {
                    #[cfg(feature = "leaf-runtime")]
                    BatchSender::Memory(sender) => sender
                        .try_send(leaf::PacketBatch::try_new(vec![packet.to_vec()])?)
                        .map_err(|error| {
                            PacketError::Io(std::io::Error::new(
                                match error {
                                    tokio::sync::mpsc::error::TrySendError::Full(_) => {
                                        std::io::ErrorKind::WouldBlock
                                    }
                                    tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                                        std::io::ErrorKind::BrokenPipe
                                    }
                                },
                                "Leaf packet batch input unavailable",
                            ))
                        }),
                    BatchSender::Stream(_) => Err(PacketError::Io(std::io::Error::new(
                        std::io::ErrorKind::WouldBlock,
                        "stream batch bridge requires async send",
                    ))),
                },
            }
        }

        pub async fn recv_from_leaf(&self, packet: &mut [u8]) -> Result<usize, PacketError> {
            match &self.kind {
                LeafPacketBridgeKind::Legacy(socket) => Ok(socket.recv(packet).await?),
                LeafPacketBridgeKind::Batch(bridge) => {
                    let mut receiver = bridge.receiver.lock().await;
                    if receiver.pending.is_empty() {
                        let batch = match &mut receiver.source {
                            #[cfg(feature = "leaf-runtime")]
                            BatchReceiverSource::Memory(source) => ReceivedPacketBatch::Memory(
                                source.recv().await.ok_or_else(|| {
                                    PacketError::Io(std::io::Error::new(
                                        std::io::ErrorKind::BrokenPipe,
                                        "Leaf packet batch output closed",
                                    ))
                                })?,
                            ),
                            BatchReceiverSource::Stream(source) => {
                                ReceivedPacketBatch::Stream(read_batch_async(source).await?)
                            }
                        };
                        let packets = match batch {
                            #[cfg(feature = "leaf-runtime")]
                            ReceivedPacketBatch::Memory(batch) => batch.into_packets(),
                            ReceivedPacketBatch::Stream(batch) => batch.into_packets(),
                        };
                        receiver.pending.extend(packets);
                    }
                    let next = receiver.pending.pop_front().ok_or_else(|| {
                        PacketError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "Leaf returned an empty packet batch",
                        ))
                    })?;
                    if next.len() > packet.len() {
                        return Err(PacketError::Io(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "Leaf packet does not fit receive buffer",
                        )));
                    }
                    packet[..next.len()].copy_from_slice(&next);
                    Ok(next.len())
                }
            }
        }

        pub fn as_raw_fd(&self) -> RawFd {
            match &self.kind {
                LeafPacketBridgeKind::Legacy(socket) => socket.as_raw_fd(),
                LeafPacketBridgeKind::Batch(_) => {
                    panic!("batch packet bridge does not expose a datagram fd")
                }
            }
        }
    }

    impl LeafPacketEndpoint {
        pub fn into_raw_fd(mut self) -> RawFd {
            self.socket
                .take()
                .expect("Leaf endpoint already consumed")
                .into_raw_fd()
        }
    }

    impl LeafPacketStreamEndpoint {
        pub fn into_raw_fd(mut self) -> RawFd {
            self.socket
                .take()
                .expect("Leaf stream endpoint already consumed")
                .into_raw_fd()
        }
    }

    #[cfg(feature = "leaf-runtime")]
    impl LeafPacketStreamEndpoint {
        pub unsafe fn from_raw_fd(fd: RawFd) -> Self {
            Self {
                socket: Some(unsafe {
                    <std::os::unix::net::UnixStream as std::os::fd::FromRawFd>::from_raw_fd(fd)
                }),
            }
        }

        pub fn into_external_packet_endpoint(
            mut self,
        ) -> Result<leaf::ExternalPacketEndpoint, PacketError> {
            let socket = self
                .socket
                .take()
                .expect("Leaf stream endpoint already consumed");
            socket.set_nonblocking(true)?;
            let capacity = PACKET_BATCH_CHANNEL_CAPACITY;
            let (to_leaf_tx, to_leaf_rx) =
                tokio::sync::mpsc::channel::<leaf::PacketBatch>(capacity);
            let (from_leaf_tx, mut from_leaf_rx) =
                tokio::sync::mpsc::channel::<leaf::PacketBatch>(capacity);

            std::thread::Builder::new()
                .name("leaf-packet-batch-io".to_owned())
                .stack_size(512 * 1024)
                .spawn(move || {
                    let runtime = match tokio::runtime::Builder::new_current_thread()
                        .enable_io()
                        .build()
                    {
                        Ok(runtime) => runtime,
                        Err(_) => return,
                    };
                    let _ = runtime.block_on(async move {
                        let (mut reader, mut writer) = UnixStream::from_std(socket)?.into_split();
                        let read_loop = async move {
                            loop {
                                let batch = read_batch_async(&mut reader).await?;
                                let batch = leaf::PacketBatch::try_new(batch.into_packets())?;
                                if to_leaf_tx.send(batch).await.is_err() {
                                    return Ok::<(), PacketError>(());
                                }
                            }
                        };
                        let write_loop = async move {
                            while let Some(batch) = from_leaf_rx.recv().await {
                                let batch = FramedPacketBatch::try_new(batch.into_packets())?;
                                write_batch_async(&mut writer, &batch).await?;
                            }
                            Ok::<(), PacketError>(())
                        };
                        tokio::select! {
                            result = read_loop => result,
                            result = write_loop => result,
                        }
                    });
                })?;
            Ok(leaf::ExternalPacketEndpoint::new(to_leaf_rx, from_leaf_tx))
        }
    }

    fn encode_batch(batch: &FramedPacketBatch) -> Result<Vec<u8>, PacketError> {
        let count = u16::try_from(batch.packets.len()).map_err(|_| {
            PacketError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "packet batch count does not fit frame",
            ))
        })?;
        let payload_bytes = u32::try_from(batch.payload_bytes).map_err(|_| {
            PacketError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "packet batch payload does not fit frame",
            ))
        })?;
        let mut frame =
            Vec::with_capacity(FRAME_HEADER_SIZE + usize::from(count) * 2 + batch.payload_bytes);
        frame.extend_from_slice(&FRAME_MAGIC);
        frame.push(FRAME_VERSION);
        frame.push(0);
        frame.extend_from_slice(&count.to_be_bytes());
        frame.extend_from_slice(&payload_bytes.to_be_bytes());
        for packet in &batch.packets {
            let length = u16::try_from(packet.len()).map_err(|_| {
                PacketError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "packet length does not fit frame",
                ))
            })?;
            frame.extend_from_slice(&length.to_be_bytes());
            frame.extend_from_slice(packet);
        }
        Ok(frame)
    }

    fn decode_header(header: &[u8; FRAME_HEADER_SIZE]) -> Result<(usize, usize), PacketError> {
        if header[..4] != FRAME_MAGIC || header[4] != FRAME_VERSION || header[5] != 0 {
            return Err(PacketError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid packet batch frame header",
            )));
        }
        let count = usize::from(u16::from_be_bytes([header[6], header[7]]));
        let payload_bytes =
            u32::from_be_bytes([header[8], header[9], header[10], header[11]]) as usize;
        if count == 0 || count > PACKET_BATCH_MAX_PACKETS || payload_bytes > PACKET_BATCH_MAX_BYTES
        {
            return Err(PacketError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "packet batch frame exceeds configured bounds",
            )));
        }
        Ok((count, payload_bytes))
    }

    async fn write_batch_async(
        writer: &mut OwnedWriteHalf,
        batch: &FramedPacketBatch,
    ) -> Result<(), PacketError> {
        writer.write_all(&encode_batch(batch)?).await?;
        Ok(())
    }

    async fn read_batch_async(
        reader: &mut OwnedReadHalf,
    ) -> Result<FramedPacketBatch, PacketError> {
        let mut header = [0u8; FRAME_HEADER_SIZE];
        reader.read_exact(&mut header).await?;
        let (count, expected_payload) = decode_header(&header)?;
        let body_bytes = count
            .checked_mul(std::mem::size_of::<u16>())
            .and_then(|length_bytes| length_bytes.checked_add(expected_payload))
            .ok_or_else(|| {
                PacketError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "packet batch frame body length overflow",
                ))
            })?;
        let mut body = vec![0u8; body_bytes];
        reader.read_exact(&mut body).await?;
        decode_batch_body(count, expected_payload, &body)
    }

    fn decode_batch_body(
        count: usize,
        expected_payload: usize,
        body: &[u8],
    ) -> Result<FramedPacketBatch, PacketError> {
        let expected_body = count
            .checked_mul(std::mem::size_of::<u16>())
            .and_then(|length_bytes| length_bytes.checked_add(expected_payload))
            .ok_or_else(|| {
                PacketError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "packet batch frame body length overflow",
                ))
            })?;
        if body.len() != expected_body {
            return Err(PacketError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "packet batch frame body length mismatch",
            )));
        }
        let mut packets = Vec::with_capacity(count);
        let mut payload_bytes = 0usize;
        let mut offset = 0usize;
        for _ in 0..count {
            let length_end = offset.checked_add(2).ok_or_else(|| {
                PacketError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "packet batch length offset overflow",
                ))
            })?;
            let length_bytes = body.get(offset..length_end).ok_or_else(|| {
                PacketError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "truncated packet length in batch frame",
                ))
            })?;
            let length = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
            payload_bytes = payload_bytes.checked_add(length).ok_or_else(|| {
                PacketError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "packet batch payload overflow",
                ))
            })?;
            if length == 0 || payload_bytes > expected_payload {
                return Err(PacketError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid packet length in batch frame",
                )));
            }
            offset = length_end;
            let packet_end = offset.checked_add(length).ok_or_else(|| {
                PacketError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "packet payload offset overflow",
                ))
            })?;
            let packet = body.get(offset..packet_end).ok_or_else(|| {
                PacketError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "truncated packet payload in batch frame",
                ))
            })?;
            packets.push(packet.to_vec());
            offset = packet_end;
        }
        if payload_bytes != expected_payload || offset != body.len() {
            return Err(PacketError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "packet batch payload length mismatch",
            )));
        }
        FramedPacketBatch::try_new(packets)
    }

    #[cfg(test)]
    mod tests {
        use std::{
            io::{Read, Write},
            os::fd::FromRawFd,
        };

        use super::*;

        #[tokio::test]
        async fn preserves_boundaries_in_both_directions() {
            let (bridge, endpoint) = LeafPacketBridge::pair().unwrap();
            assert_eq!(bridge.backend(), PacketBridgeBackend::Legacy);
            let fd = endpoint.into_raw_fd();
            let mut leaf = unsafe { std::fs::File::from_raw_fd(fd) };

            bridge.send_to_leaf(&[1, 2, 3]).await.unwrap();
            bridge.send_to_leaf(&[4, 5]).await.unwrap();

            let mut packet = [0u8; 16];
            assert_eq!(leaf.read(&mut packet).unwrap(), 3);
            assert_eq!(&packet[..3], &[1, 2, 3]);
            assert_eq!(leaf.read(&mut packet).unwrap(), 2);
            assert_eq!(&packet[..2], &[4, 5]);

            leaf.write_all(&[6, 7, 8, 9]).unwrap();
            assert_eq!(bridge.recv_from_leaf(&mut packet).await.unwrap(), 4);
            assert_eq!(&packet[..4], &[6, 7, 8, 9]);
        }

        #[tokio::test]
        async fn rejects_empty_and_oversized_packets() {
            let (bridge, _endpoint) = LeafPacketBridge::pair().unwrap();
            assert!(matches!(
                bridge.send_to_leaf(&[]).await,
                Err(PacketError::Empty)
            ));
            assert!(matches!(
                bridge.try_send_to_leaf(&[]),
                Err(PacketError::Empty)
            ));
            let packet = vec![0; MAX_PACKET_SIZE + 1];
            assert!(matches!(
                bridge.send_to_leaf(&packet).await,
                Err(PacketError::TooLarge)
            ));
        }

        #[test]
        fn unsupported_packet_batch_request_keeps_legacy_backend() {
            assert_eq!(
                PacketBridgeMode::for_request(false, true),
                PacketBridgeMode::Legacy
            );
            assert_eq!(
                PacketBridgeMode::for_request(true, false),
                PacketBridgeMode::Legacy
            );
            assert_eq!(
                PacketBridgeMode::for_request(true, true),
                PacketBridgeMode::PacketBatch
            );
        }

        #[cfg(feature = "leaf-runtime")]
        #[tokio::test]
        async fn memory_batch_bridge_preserves_order_and_boundaries() {
            let (bridge, endpoint) = LeafPacketBridge::memory_batch_pair();
            assert_eq!(bridge.backend(), PacketBridgeBackend::MemoryBatch);
            bridge
                .send_batch_to_leaf(vec![vec![1, 2], vec![3], vec![4, 5, 6]])
                .await
                .unwrap();
            let (mut ingress, egress) = endpoint.into_parts();
            assert_eq!(
                ingress.recv().await.unwrap().into_packets(),
                vec![vec![1, 2], vec![3], vec![4, 5, 6]]
            );
            egress
                .send(leaf::PacketBatch::try_new(vec![vec![7], vec![8, 9]]).unwrap())
                .await
                .unwrap();
            let mut packet = [0u8; 16];
            assert_eq!(bridge.recv_from_leaf(&mut packet).await.unwrap(), 1);
            assert_eq!(&packet[..1], &[7]);
            assert_eq!(bridge.recv_from_leaf(&mut packet).await.unwrap(), 2);
            assert_eq!(&packet[..2], &[8, 9]);
        }

        #[tokio::test]
        async fn stream_batch_bridge_preserves_order_and_detects_close() {
            let (bridge, mut endpoint) = LeafPacketBridge::stream_batch_pair().unwrap();
            assert_eq!(bridge.backend(), PacketBridgeBackend::StreamBatch);
            let socket = endpoint.socket.take().unwrap();
            socket.set_nonblocking(true).unwrap();
            let (mut endpoint_reader, mut endpoint_writer) =
                UnixStream::from_std(socket).unwrap().into_split();
            bridge
                .send_batch_to_leaf(vec![vec![1], vec![2, 3]])
                .await
                .unwrap();
            assert_eq!(
                read_batch_async(&mut endpoint_reader)
                    .await
                    .unwrap()
                    .into_packets(),
                vec![vec![1], vec![2, 3]]
            );
            write_batch_async(
                &mut endpoint_writer,
                &FramedPacketBatch::try_new(vec![vec![4, 5]]).unwrap(),
            )
            .await
            .unwrap();
            let mut packet = [0u8; 16];
            assert_eq!(bridge.recv_from_leaf(&mut packet).await.unwrap(), 2);
            assert_eq!(&packet[..2], &[4, 5]);
            drop(endpoint_reader);
            drop(endpoint_writer);
            assert!(bridge.recv_from_leaf(&mut packet).await.is_err());
        }

        #[test]
        fn contiguous_batch_body_rejects_corrupt_lengths() {
            let body = [0, 1, 7, 0, 2, 8, 9];
            assert_eq!(
                decode_batch_body(2, 3, &body).unwrap().into_packets(),
                vec![vec![7], vec![8, 9]]
            );
            assert!(decode_batch_body(2, 4, &body).is_err());
            assert!(decode_batch_body(2, 3, &[0, 1, 7, 0, 3, 8, 9]).is_err());
            assert!(decode_batch_body(1, 0, &[0, 0]).is_err());
        }

        #[cfg(feature = "leaf-runtime")]
        #[tokio::test]
        async fn stream_endpoint_adapter_preserves_leaf_channel_ownership() {
            let (bridge, endpoint) = LeafPacketBridge::stream_batch_pair().unwrap();
            let endpoint = endpoint.into_external_packet_endpoint().unwrap();
            let (mut ingress, egress) = endpoint.into_parts();

            bridge
                .send_batch_to_leaf(vec![vec![1], vec![2, 3]])
                .await
                .unwrap();
            assert_eq!(
                ingress.recv().await.unwrap().into_packets(),
                vec![vec![1], vec![2, 3]]
            );

            egress
                .send(leaf::PacketBatch::try_new(vec![vec![4, 5]]).unwrap())
                .await
                .unwrap();
            let mut packet = [0u8; 16];
            assert_eq!(bridge.recv_from_leaf(&mut packet).await.unwrap(), 2);
            assert_eq!(&packet[..2], &[4, 5]);
        }
    }
}

#[cfg(unix)]
pub use unix_bridge::LeafPacketStreamEndpoint;
#[cfg(unix)]
pub use unix_bridge::{
    LEAF_PACKET_BATCH_EXPERIMENTAL_FEATURE, LeafPacketBridge, LeafPacketEndpoint,
    PacketBridgeBackend, PacketBridgeMode,
};
#[cfg(unix)]
pub use unix_bridge::{PACKET_BATCH_MAX_BYTES, PACKET_BATCH_MAX_PACKETS};

#[cfg(test)]
mod tests {
    use super::*;

    fn ipv4(destination: [u8; 4]) -> [u8; MIN_IPV4_HEADER] {
        let mut packet = [0u8; MIN_IPV4_HEADER];
        packet[0] = 0x45;
        packet[2..4].copy_from_slice(&(MIN_IPV4_HEADER as u16).to_be_bytes());
        packet[16..20].copy_from_slice(&destination);
        packet
    }

    fn ipv6(destination: [u8; 16]) -> [u8; MIN_IPV6_HEADER] {
        let mut packet = [0u8; MIN_IPV6_HEADER];
        packet[0] = 0x60;
        packet[24..40].copy_from_slice(&destination);
        packet
    }

    #[test]
    fn classifies_ipv4_and_ipv6_without_payload_copy() {
        let classifier = PacketClassifier::new(MeshRouteSnapshot::new(vec![
            "10.44.0.0/16".parse().unwrap(),
            "fd00:44::/48".parse().unwrap(),
        ]));
        assert_eq!(
            classifier.classify(&ipv4([10, 44, 2, 3])).unwrap(),
            PacketClass::Mesh
        );
        assert_eq!(
            classifier.classify(&ipv4([1, 1, 1, 1])).unwrap(),
            PacketClass::Policy
        );
        assert_eq!(
            classifier
                .classify(&ipv6(
                    "fd00:44::8".parse::<std::net::Ipv6Addr>().unwrap().octets()
                ))
                .unwrap(),
            PacketClass::Mesh
        );
    }

    #[test]
    fn atomically_replaces_route_snapshot() {
        let classifier = PacketClassifier::new(MeshRouteSnapshot::default());
        let packet = ipv4([10, 44, 2, 3]);
        assert_eq!(classifier.classify(&packet).unwrap(), PacketClass::Policy);
        classifier.replace_routes(MeshRouteSnapshot::new(vec![
            "10.44.0.0/16".parse().unwrap(),
        ]));
        assert_eq!(classifier.classify(&packet).unwrap(), PacketClass::Mesh);
    }

    #[test]
    fn preserves_mesh_broadcast_and_multicast_paths() {
        let classifier = PacketClassifier::new(MeshRouteSnapshot::default());
        assert_eq!(
            classifier.classify(&ipv4([255, 255, 255, 255])).unwrap(),
            PacketClass::Mesh
        );
        assert_eq!(
            classifier.classify(&ipv4([224, 0, 0, 251])).unwrap(),
            PacketClass::Mesh
        );
        assert_eq!(
            classifier
                .classify(&ipv6(
                    "ff02::fb".parse::<std::net::Ipv6Addr>().unwrap().octets()
                ))
                .unwrap(),
            PacketClass::Mesh
        );
    }

    #[test]
    fn rejects_malformed_and_oversized_packets() {
        let classifier = PacketClassifier::new(MeshRouteSnapshot::default());
        let mut malformed = ipv4([1, 1, 1, 1]);
        malformed[0] = 0x44;
        assert!(matches!(
            classifier.classify(&malformed),
            Err(PacketError::Malformed { version: 4 })
        ));
        assert!(matches!(
            classifier.classify(&vec![0x60; MAX_PACKET_SIZE + 1]),
            Err(PacketError::TooLarge)
        ));

        let mut truncated_ipv6 = ipv6([0; 16]);
        truncated_ipv6[4..6].copy_from_slice(&8u16.to_be_bytes());
        assert!(matches!(
            classifier.classify(&truncated_ipv6),
            Err(PacketError::Truncated {
                version: 6,
                length: MIN_IPV6_HEADER
            })
        ));
    }
}
