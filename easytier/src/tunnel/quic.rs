//! This example demonstrates how to make a QUIC connection that ignores the server certificate.
//!
//! Checkout the `README.md` for guidance.

use super::{FromUrl, IpVersion, Tunnel, TunnelConnector, TunnelError, TunnelListener};
use crate::common::global_ctx::ArcGlobalCtx;
use crate::tunnel::common::bind;
use crate::tunnel::{
    TunnelInfo,
    common::{FramedReader, FramedWriter, TunnelWrapper},
};
use anyhow::Context;
use derivative::Derivative;
use derive_more::{Deref, DerefMut};
use parking_lot::RwLock;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{
    AsyncUdpSocket, ClientConfig, ConnectError, Connection, Endpoint, EndpointConfig, ServerConfig,
    TransportConfig, UdpPoller, congestion::BbrConfig, default_runtime,
};
use std::collections::HashMap;
use std::io::{self, IoSliceMut};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::pin::Pin;
use std::sync::{Mutex, OnceLock};
use std::task::{Context as TaskContext, Poll};
use std::time::Instant;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::net::UdpSocket;

const QUIC_STEALTH_FALLBACK_TIMEOUT: Duration = Duration::from_secs(1);
const QUIC_STEALTH_OUTER_SEND_DELAY: Duration = Duration::from_secs(1);
const QUIC_STEALTH_GATE_RECV_GRACE: Duration = Duration::from_secs(5);
const QUIC_STEALTH_SESSION_TTL: Duration = Duration::from_secs(600);
const QUIC_STEALTH_MAX_SESSIONS: usize = 4096;

// region config
mod crypto {
    use crate::utils::BoxExt;
    use bytes::{Buf, BytesMut};
    use quinn_proto::crypto::{
        ClientConfig, ExportKeyingMaterialError, KeyPair, Keys, ServerConfig, Session,
        UnsupportedVersion,
    };
    use quinn_proto::transport_parameters::TransportParameters;
    use quinn_proto::{
        ConnectError, ConnectionId, Side, TransportError,
        crypto::{CryptoError, HeaderKey, PacketKey},
    };
    use seahash::SeaHasher;
    use std::any::Any;
    use std::{hash::Hasher, sync::Arc};
    use tracing::{error, instrument, trace};

    #[derive(Debug, Clone, Copy)]
    struct CryptoKey;

    impl CryptoKey {
        fn header(self) -> KeyPair<Box<dyn HeaderKey>> {
            KeyPair {
                local: Box::new(self),
                remote: Box::new(self),
            }
        }

        fn packet(self) -> KeyPair<Box<dyn PacketKey>> {
            KeyPair {
                local: Box::new(self),
                remote: Box::new(self),
            }
        }

        fn keys(self) -> Keys {
            Keys {
                header: self.header(),
                packet: self.packet(),
            }
        }
    }

    impl HeaderKey for CryptoKey {
        fn decrypt(&self, _: usize, _: &mut [u8]) {}
        fn encrypt(&self, _: usize, _: &mut [u8]) {}
        fn sample_size(&self) -> usize {
            0
        }
    }

    impl CryptoKey {
        fn checksum(slices: &[&[u8]]) -> u64 {
            let mut hasher = SeaHasher::default();
            for slice in slices {
                hasher.write(&(slice.len() as u64).to_le_bytes());
                hasher.write(slice);
            }
            hasher.finish()
        }
    }

    impl PacketKey for CryptoKey {
        #[instrument(level = "trace")]
        fn encrypt(&self, packet: u64, buf: &mut [u8], header_len: usize) {
            let (header, rest) = buf.split_at_mut(header_len);
            let (payload, tag) = rest.split_at_mut(rest.len() - self.tag_len());
            let checksum = Self::checksum(&[header, payload]);
            tag.copy_from_slice(&checksum.to_be_bytes());
            trace!(checksum, ?header, ?payload, ?tag);
        }

        #[instrument(level = "trace")]
        fn decrypt(
            &self,
            packet: u64,
            header: &[u8],
            payload: &mut BytesMut,
        ) -> Result<(), CryptoError> {
            let tag = payload.split_off(payload.len() - self.tag_len()).get_u64();
            trace!(tag, ?payload);
            let checksum = Self::checksum(&[header, payload]);
            if checksum != tag {
                error!(tag, checksum, "checksum mismatch");
                return Err(CryptoError);
            }
            Ok(())
        }

        fn tag_len(&self) -> usize {
            8
        }

        fn confidentiality_limit(&self) -> u64 {
            u64::MAX
        }

        fn integrity_limit(&self) -> u64 {
            1 << 36
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum HandshakeState {
        EmitInitial,
        EmitHandshake,
        Done,
    }

    #[derive(Debug)]
    struct QuicSession {
        side: Side,
        state: HandshakeState,
        local: TransportParameters,
        remote: Option<TransportParameters>,
    }

    impl QuicSession {
        fn new(side: Side, params: TransportParameters) -> Self {
            Self {
                side,
                state: HandshakeState::EmitInitial,
                local: params,
                remote: None,
            }
        }
    }

    impl Session for QuicSession {
        fn initial_keys(&self, _: &ConnectionId, _: Side) -> Keys {
            CryptoKey.keys()
        }

        fn handshake_data(&self) -> Option<Box<dyn Any>> {
            self.remote.map(|params| params.boxed() as _)
        }

        fn peer_identity(&self) -> Option<Box<dyn Any>> {
            None
        }

        fn early_crypto(&self) -> Option<(Box<dyn HeaderKey>, Box<dyn PacketKey>)> {
            None
        }

        fn early_data_accepted(&self) -> Option<bool> {
            Some(false)
        }

        #[instrument(level = "trace")]
        fn is_handshaking(&self) -> bool {
            self.remote.is_none() || self.state != HandshakeState::Done
        }

        #[instrument(level = "trace")]
        fn read_handshake(&mut self, mut buf: &[u8]) -> Result<bool, TransportError> {
            if self.remote.is_none() {
                self.remote = Some(
                    TransportParameters::read(self.side, &mut buf)
                        .expect("failed to read transport parameters"),
                );
            }
            Ok(true)
        }

        #[instrument(level = "trace")]
        fn transport_parameters(&self) -> Result<Option<TransportParameters>, TransportError> {
            Ok(self.remote)
        }

        #[instrument(level = "trace")]
        fn write_handshake(&mut self, buf: &mut Vec<u8>) -> Option<Keys> {
            match self.state {
                HandshakeState::EmitInitial => {
                    if self.side.is_client() {
                        self.local.write(buf);
                    }
                    self.state = HandshakeState::EmitHandshake;
                    Some(CryptoKey.keys())
                }
                HandshakeState::EmitHandshake => {
                    if self.side.is_server() {
                        self.local.write(buf);
                    }
                    self.state = HandshakeState::Done;
                    Some(CryptoKey.keys())
                }
                HandshakeState::Done => None,
            }
        }

        fn next_1rtt_keys(&mut self) -> Option<KeyPair<Box<dyn PacketKey>>> {
            Some(CryptoKey.packet())
        }

        fn is_valid_retry(&self, _: &ConnectionId, _: &[u8], _: &[u8]) -> bool {
            true
        }

        fn export_keying_material(
            &self,
            _: &mut [u8],
            _: &[u8],
            _: &[u8],
        ) -> Result<(), ExportKeyingMaterialError> {
            Ok(())
        }
    }

    #[derive(Debug)]
    pub struct CryptoConfig;

    impl ClientConfig for CryptoConfig {
        #[instrument(level = "trace")]
        fn start_session(
            self: Arc<Self>,
            version: u32,
            server_name: &str,
            params: &TransportParameters,
        ) -> Result<Box<dyn Session>, ConnectError> {
            Ok(Box::new(QuicSession::new(Side::Client, *params)))
        }
    }

    impl ServerConfig for CryptoConfig {
        fn initial_keys(&self, _: u32, _: &ConnectionId) -> Result<Keys, UnsupportedVersion> {
            Ok(CryptoKey.keys())
        }

        fn retry_tag(&self, _: u32, _: &ConnectionId, _: &[u8]) -> [u8; 16] {
            [0u8; 16]
        }

        #[instrument(level = "trace")]
        fn start_session(
            self: Arc<Self>,
            version: u32,
            params: &TransportParameters,
        ) -> Box<dyn Session> {
            Box::new(QuicSession::new(Side::Server, *params))
        }
    }
}

pub fn transport_config() -> Arc<TransportConfig> {
    let mut config = TransportConfig::default();

    config
        .max_concurrent_bidi_streams(u8::MAX.into())
        .max_concurrent_uni_streams(0u8.into())
        .keep_alive_interval(Some(Duration::from_secs(5)))
        .initial_mtu(1200)
        .min_mtu(1200)
        .enable_segmentation_offload(true)
        .stream_receive_window(quinn::VarInt::from_u32(8_388_608))
        .congestion_controller_factory(Arc::new(BbrConfig::default()));

    Arc::new(config)
}

pub fn server_config() -> ServerConfig {
    let mut config = ServerConfig::with_crypto(Arc::new(crypto::CryptoConfig));
    config.transport_config(transport_config());
    config
}

fn stealth_server_config() -> ServerConfig {
    let mut config = server_config();
    // The outer session is keyed by the authenticated source address. QUIC
    // migration would move packets to an address that cannot yet authenticate
    // with the connection-level key.
    config.migration(false);
    config
}

pub fn client_config() -> ClientConfig {
    let mut config = ClientConfig::new(Arc::new(crypto::CryptoConfig));
    config.transport_config(transport_config());
    config
}

pub fn endpoint_config() -> EndpointConfig {
    let mut config = EndpointConfig::default();
    config.max_udp_payload_size(1200).unwrap();
    config
}

fn stealth_endpoint_config() -> EndpointConfig {
    let mut config = EndpointConfig::default();
    // A stealth socket receives the outer nonce/tag before opening the packet.
    // The QUIC transport itself still uses a 1200-byte initial MTU.
    config
        .max_udp_payload_size(1200 + crate::tunnel::stealth::OUTER_OVERHEAD as u16)
        .unwrap();
    config
}
//endregion

// region stealth socket
#[derive(Debug)]
struct QuicStealthSession {
    state: Arc<crate::tunnel::stealth::OuterSessionState>,
}

impl QuicStealthSession {
    fn new(state: Arc<crate::tunnel::stealth::OuterSessionState>) -> Self {
        Self { state }
    }

    fn outer_elapsed(&self) -> Option<Duration> {
        self.state.outer_key_elapsed()
    }

    fn seal(&self, plaintext: &[u8]) -> Option<Vec<u8>> {
        match self.outer_elapsed() {
            Some(elapsed) if elapsed >= QUIC_STEALTH_OUTER_SEND_DELAY => {
                self.state.seal_datagram(plaintext)
            }
            _ => self.state.seal_gate_datagram(plaintext),
        }
    }

    fn open(&self, sealed: &[u8]) -> Option<Vec<u8>> {
        match self.outer_elapsed() {
            Some(elapsed) => self.state.open_datagram(sealed).or_else(|| {
                (elapsed <= QUIC_STEALTH_GATE_RECV_GRACE)
                    .then(|| self.state.open_gate_datagram(sealed))
                    .flatten()
            }),
            None => self.state.open_gate_datagram(sealed),
        }
    }

    /// Open a sealed datagram in-place. On success returns plaintext length.
    /// Buffer contains `nonce || ciphertext || tag`; on success plaintext is
    /// moved to buffer start.
    ///
    /// Grace period safety: only uses in-place when `elapsed >= OUTER_SEND_DELAY`
    /// (definite outer phase). During grace period, uses allocating path to
    /// avoid buffer corruption from failed AEAD open_in_place.
    fn open_in_place(&self, buf: &mut [u8]) -> Option<usize> {
        match self.outer_elapsed() {
            Some(elapsed) if elapsed >= QUIC_STEALTH_OUTER_SEND_DELAY => {
                self.state.open_datagram_in_place(buf)
            }
            Some(elapsed) => self
                .state
                .open_datagram(buf)
                .or_else(|| {
                    (elapsed <= QUIC_STEALTH_GATE_RECV_GRACE)
                        .then(|| self.state.open_gate_datagram(buf))
                        .flatten()
                })
                .map(|pt| {
                    buf[..pt.len()].copy_from_slice(&pt);
                    pt.len()
                }),
            None => self.state.open_datagram_in_place(buf),
        }
    }
}

#[derive(Debug)]
struct QuicStealthSessionEntry {
    session: Arc<QuicStealthSession>,
    last_seen: Instant,
}

#[derive(Debug)]
struct QuicStealthSocket {
    inner: Arc<dyn AsyncUdpSocket>,
    template: Arc<crate::tunnel::stealth::OuterSessionState>,
    sessions: Mutex<HashMap<SocketAddr, QuicStealthSessionEntry>>,
    initial_nonces: Mutex<HashMap<[u8; crate::tunnel::stealth::OUTER_NONCE_LEN], Instant>>,
}

impl QuicStealthSocket {
    fn new(
        inner: Arc<dyn AsyncUdpSocket>,
        template: Arc<crate::tunnel::stealth::OuterSessionState>,
    ) -> Self {
        debug_assert!(template.is_enabled());
        Self {
            inner,
            template,
            sessions: Mutex::new(HashMap::new()),
            initial_nonces: Mutex::new(HashMap::new()),
        }
    }

    fn is_quic_initial(plaintext: &[u8]) -> bool {
        plaintext.len() >= 5
            && plaintext[0] & 0xc0 == 0xc0
            && plaintext[0] & 0x30 == 0
            && plaintext[1..5] != [0, 0, 0, 0]
    }

    fn accept_initial_nonce(&self, sealed: &[u8]) -> bool {
        let Ok(nonce) = sealed
            .get(..crate::tunnel::stealth::OUTER_NONCE_LEN)
            .unwrap_or_default()
            .try_into()
        else {
            return false;
        };
        let now = Instant::now();
        let ttl = Duration::from_secs(self.template.window_secs().saturating_mul(2).max(1));
        let mut nonces = self.initial_nonces.lock().unwrap();
        nonces.retain(|_, seen| now.saturating_duration_since(*seen) <= ttl);
        if nonces.contains_key(&nonce) {
            return false;
        }
        while nonces.len() >= QUIC_STEALTH_MAX_SESSIONS {
            let Some(oldest) = nonces
                .iter()
                .min_by_key(|(_, seen)| **seen)
                .map(|(nonce, _)| *nonce)
            else {
                break;
            };
            nonces.remove(&oldest);
        }
        nonces.insert(nonce, now);
        true
    }

    fn cleanup_locked(sessions: &mut HashMap<SocketAddr, QuicStealthSessionEntry>, now: Instant) {
        sessions.retain(|_, entry| {
            now.saturating_duration_since(entry.last_seen) <= QUIC_STEALTH_SESSION_TTL
        });
    }

    fn make_room_locked(sessions: &mut HashMap<SocketAddr, QuicStealthSessionEntry>) {
        while sessions.len() >= QUIC_STEALTH_MAX_SESSIONS {
            let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, entry)| entry.last_seen)
                .map(|(addr, _)| *addr)
            else {
                break;
            };
            sessions.remove(&oldest);
        }
    }

    fn register_outbound(&self, addr: SocketAddr) -> Arc<QuicStealthSession> {
        let now = Instant::now();
        let mut sessions = self.sessions.lock().unwrap();
        Self::cleanup_locked(&mut sessions, now);
        if !sessions.contains_key(&addr) {
            Self::make_room_locked(&mut sessions);
        }
        let entry = sessions
            .entry(addr)
            .or_insert_with(|| QuicStealthSessionEntry {
                session: Arc::new(QuicStealthSession::new(
                    self.template.fork_for_transport_delayed_transition(),
                )),
                last_seen: now,
            });
        entry.last_seen = now;
        entry.session.clone()
    }

    fn session(&self, addr: SocketAddr) -> Option<Arc<QuicStealthSession>> {
        let now = Instant::now();
        let mut sessions = self.sessions.lock().unwrap();
        Self::cleanup_locked(&mut sessions, now);
        let entry = sessions.get_mut(&addr)?;
        entry.last_seen = now;
        Some(entry.session.clone())
    }

    fn remove_session_if_same(&self, addr: SocketAddr, session: &Arc<QuicStealthSession>) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        let Some(entry) = sessions.get(&addr) else {
            return false;
        };
        if !Arc::ptr_eq(&entry.session, session) {
            return false;
        }
        sessions.remove(&addr);
        true
    }

    fn open_from(
        &self,
        addr: SocketAddr,
        sealed: &[u8],
    ) -> Option<(Vec<u8>, Arc<QuicStealthSession>)> {
        if let Some(session) = self.session(addr) {
            let plaintext = session.open(sealed)?;
            if session.state.outer_key().is_some() && Self::is_quic_initial(&plaintext) {
                return None;
            }
            return Some((plaintext, session));
        }

        let candidate = Arc::new(QuicStealthSession::new(
            self.template.fork_for_transport_delayed_transition(),
        ));
        let plaintext = candidate.open(sealed)?;
        if !Self::is_quic_initial(&plaintext) || !self.accept_initial_nonce(sealed) {
            return None;
        }

        let now = Instant::now();
        let mut sessions = self.sessions.lock().unwrap();
        Self::cleanup_locked(&mut sessions, now);
        if !sessions.contains_key(&addr) {
            Self::make_room_locked(&mut sessions);
        }
        let entry = sessions
            .entry(addr)
            .or_insert_with(|| QuicStealthSessionEntry {
                session: candidate,
                last_seen: now,
            });
        entry.last_seen = now;
        Some((plaintext, entry.session.clone()))
    }

    /// In-place version of `open_from`. Decrypts directly in `buf` and returns
    /// plaintext length + session. Candidate path (new connection) uses
    /// allocating `open()` because in-place would corrupt buffer on failure.
    fn open_in_place_from(
        &self,
        addr: SocketAddr,
        buf: &mut [u8],
    ) -> Option<(usize, Arc<QuicStealthSession>)> {
        if let Some(session) = self.session(addr) {
            let pt_len = session.open_in_place(buf)?;
            if session.state.outer_key().is_some()
                && session
                    .outer_elapsed()
                    .is_none_or(|e| e > QUIC_STEALTH_GATE_RECV_GRACE)
                && Self::is_quic_initial(&buf[..pt_len])
            {
                return None;
            }
            return Some((pt_len, session));
        }

        // Candidate path: rare (only first packet from new addr).
        // Use allocating path to avoid buffer corruption on failure.
        let candidate = Arc::new(QuicStealthSession::new(
            self.template.fork_for_transport_delayed_transition(),
        ));
        let plaintext = candidate.open(buf)?;
        if !Self::is_quic_initial(&plaintext) || !self.accept_initial_nonce(buf) {
            return None;
        }
        // Copy plaintext back into buf
        buf[..plaintext.len()].copy_from_slice(&plaintext);

        let now = Instant::now();
        let mut sessions = self.sessions.lock().unwrap();
        Self::cleanup_locked(&mut sessions, now);
        if !sessions.contains_key(&addr) {
            Self::make_room_locked(&mut sessions);
        }
        let entry = sessions
            .entry(addr)
            .or_insert_with(|| QuicStealthSessionEntry {
                session: candidate,
                last_seen: now,
            });
        entry.last_seen = now;
        Some((plaintext.len(), entry.session.clone()))
    }
}

impl AsyncUdpSocket for QuicStealthSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        self.inner.clone().create_io_poller()
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        let session = self.session(transmit.destination).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotConnected,
                "missing QUIC stealth session for destination",
            )
        })?;

        if let Some(seg_size) = transmit.segment_size {
            // GSO: seal each segment independently, then batch via inner with adjusted segment size.
            let contents = transmit.contents;
            let sealed_seg_size = seg_size + crate::tunnel::stealth::OUTER_OVERHEAD;
            let num_segments = contents.len() / seg_size;
            let mut sealed = Vec::with_capacity(num_segments * sealed_seg_size);
            for i in 0..num_segments {
                let segment = &contents[i * seg_size..(i + 1) * seg_size];
                let sealed_segment = session.seal(segment).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "QUIC stealth sealing failed")
                })?;
                sealed.extend_from_slice(&sealed_segment);
            }
            self.inner.try_send(&Transmit {
                destination: transmit.destination,
                ecn: transmit.ecn,
                contents: &sealed,
                segment_size: Some(sealed_seg_size),
                src_ip: transmit.src_ip,
            })
        } else {
            let sealed = session.seal(transmit.contents).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "QUIC stealth sealing failed")
            })?;
            self.inner.try_send(&Transmit {
                destination: transmit.destination,
                ecn: transmit.ecn,
                contents: &sealed,
                segment_size: None,
                src_ip: transmit.src_ip,
            })
        }
    }

    fn poll_recv(
        &self,
        cx: &mut TaskContext<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        if bufs.is_empty() || meta.is_empty() {
            return Poll::Ready(Ok(0));
        }

        for _ in 0..4 {
            let count = match self.inner.poll_recv(cx, bufs, meta) {
                Poll::Ready(Ok(count)) => count,
                other => return other,
            };
            let mut opened_count = 0;
            for index in 0..count {
                let received = meta[index];
                let stride = received.stride.max(1);
                let mut offset = 0;
                while offset < received.len && opened_count < bufs.len().min(meta.len()) {
                    let end = (offset + stride).min(received.len);
                    if let Some((pt_len, _)) =
                        self.open_in_place_from(received.addr, &mut bufs[index][offset..end])
                    {
                        // Move plaintext to destination buffer if needed.
                        // Single stride (most common): opened_count == index && offset == 0,
                        // no copy needed.
                        if opened_count == index && offset != 0 {
                            bufs[index].copy_within(offset..offset + pt_len, 0);
                        } else if opened_count != index {
                            // src = bufs[index] (has plaintext at [offset..offset+pt_len])
                            // dst = bufs[opened_count] (destination)
                            let (left, right) = bufs.split_at_mut(opened_count.max(index));
                            let (src, dst) = if index < opened_count {
                                (&left[index], &mut right[0])
                            } else {
                                (&right[0], &mut left[opened_count])
                            };
                            dst[..pt_len].copy_from_slice(&src[offset..offset + pt_len]);
                        }
                        meta[opened_count] = RecvMeta {
                            len: pt_len,
                            stride: pt_len,
                            ..received
                        };
                        opened_count += 1;
                    }
                    offset = end;
                }
            }

            if opened_count == 0 {
                continue;
            }
            return Poll::Ready(Ok(opened_count));
        }

        cx.waker().wake_by_ref();
        Poll::Pending
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.inner.max_transmit_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.inner.max_receive_segments()
    }

    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}
// endregion

//region rw pool
#[derive(Derivative)]
#[derivative(Default(bound = ""))]
#[derive(Debug, Deref, DerefMut)]
struct RwPoolInner<Item> {
    #[deref]
    #[deref_mut]
    pool: Vec<Item>,
    enabled: bool,
}

#[derive(Debug)]
struct RwPool<Item> {
    ephemeral: RwLock<RwPoolInner<Item>>,
    persistent: RwLock<RwPoolInner<Item>>,
    capacity: usize,
}

impl<Item> RwPool<Item> {
    fn new(capacity: usize) -> Self {
        Self {
            ephemeral: RwLock::new(RwPoolInner::default()),
            persistent: RwLock::new(RwPoolInner::default()),
            capacity,
        }
    }

    /// return the capacity of the ephemeral pool;
    /// if `ephemeral` or `persistent` is None, read lock `self`'s pool
    fn capacity(
        &self,
        ephemeral: Option<&RwPoolInner<Item>>,
        persistent: Option<&RwPoolInner<Item>>,
    ) -> usize {
        let guard;
        let ephemeral = if let Some(ephemeral) = ephemeral {
            ephemeral
        } else {
            guard = self.ephemeral.read();
            &guard
        };

        let guard;
        let persistent = if let Some(persistent) = persistent {
            persistent
        } else {
            guard = self.persistent.read();
            &guard
        };

        (self.capacity * ephemeral.enabled as usize).saturating_sub(persistent.len())
    }

    fn is_full(&self) -> bool {
        let pool = self.ephemeral.read();
        pool.len() >= self.capacity(Some(&pool), None)
    }

    fn is_enabled(&self) -> bool {
        self.ephemeral.read().enabled
    }

    fn enable(&self) {
        self.ephemeral.write().enabled = true;
        self.resize();
    }

    fn disable(&self) {
        self.ephemeral.write().enabled = false;
        self.resize();
    }

    /// push an item to the persistent pool
    fn push(&self, item: Item) {
        self.persistent.write().push(item);
        self.resize();
    }

    fn len(&self) -> usize {
        let persistent_len = self.persistent.read().len();
        let ephemeral_len = self.ephemeral.read().len();
        persistent_len + ephemeral_len
    }

    /// try to push an item to the ephemeral pool, return the item if full
    fn try_push(&self, item: Item) -> Option<Item> {
        let mut pool = self.ephemeral.write();
        if pool.len() < self.capacity(Some(&pool), None) {
            pool.push(item);
            return None;
        }
        Some(item)
    }

    fn resize(&self) {
        let resize = {
            let pool = self.ephemeral.read();
            pool.capacity() != self.capacity(Some(&pool), None)
        };
        if resize {
            let mut pool = self.ephemeral.write();
            let capacity = self.capacity(Some(&pool), None);
            pool.reserve_exact(capacity);
            pool.truncate(capacity);
            pool.shrink_to(capacity);
        }
    }

    fn with_iter<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut dyn Iterator<Item = &Item>) -> R,
    {
        let ephemeral = self.ephemeral.read();
        let persistent = self.persistent.read();
        f(&mut persistent.iter().chain(ephemeral.iter()))
    }
}

impl RwPool<Endpoint> {
    fn retain_endpoints<F>(&self, mut keep: F) -> usize
    where
        F: FnMut(&Endpoint) -> bool,
    {
        let persistent_removed = {
            let mut persistent = self.persistent.write();
            let before = persistent.len();
            persistent.retain(|endpoint| keep(endpoint));
            before - persistent.len()
        };

        let ephemeral_removed = {
            let mut ephemeral = self.ephemeral.write();
            let before = ephemeral.len();
            ephemeral.retain(|endpoint| keep(endpoint));
            before - ephemeral.len()
        };

        let removed = persistent_removed + ephemeral_removed;
        if removed > 0 {
            self.resize();
        }
        removed
    }

    fn remove_by_local_addr(&self, local_addr: SocketAddr) -> usize {
        self.retain_endpoints(|endpoint| endpoint.local_addr().ok() != Some(local_addr))
    }

    fn contains_local_addr(&self, local_addr: SocketAddr) -> bool {
        self.persistent
            .read()
            .iter()
            .any(|endpoint| endpoint.local_addr().ok() == Some(local_addr))
            || self
                .ephemeral
                .read()
                .iter()
                .any(|endpoint| endpoint.local_addr().ok() == Some(local_addr))
    }
}
//endregion

//region endpoint manager
#[derive(Debug)]
pub struct QuicEndpointManager {
    ipv4: RwPool<Endpoint>,
    ipv6: RwPool<Endpoint>,
    both: RwPool<Endpoint>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QuicBindMode {
    V4Only,
    V6Only,
    DualStack,
}

impl QuicBindMode {
    fn dual_stack(self) -> bool {
        matches!(self, Self::DualStack)
    }

    fn validate_address_family(self, addr: SocketAddr) -> Result<(), TunnelError> {
        let valid = match self {
            Self::V4Only => addr.is_ipv4(),
            Self::V6Only => addr.is_ipv6(),
            Self::DualStack => addr.ip() == Ipv6Addr::UNSPECIFIED,
        };
        if valid {
            Ok(())
        } else {
            Err(TunnelError::InternalError(format!(
                "QUIC bind mode {self:?} does not match listen address {addr}"
            )))
        }
    }
}

static QUIC_ENDPOINT_MANAGER: OnceLock<QuicEndpointManager> = OnceLock::new();

type QuicStealthEndpoint = (
    Endpoint,
    Option<Arc<QuicStealthSocket>>,
    Option<Arc<QuicStealthSession>>,
);

impl QuicEndpointManager {
    fn try_create_with_stealth(
        addr: SocketAddr,
        dual_stack: bool,
        socket_mark: Option<u32>,
        stealth: Option<(
            Arc<crate::tunnel::stealth::OuterSessionState>,
            Option<SocketAddr>,
        )>,
    ) -> Result<QuicStealthEndpoint, TunnelError> {
        let socket = bind::<UdpSocket>()
            .addr(addr)
            .only_v6(addr.is_ipv6() && !dual_stack)
            .maybe_socket_mark(socket_mark)
            .call()?;
        let runtime = default_runtime().ok_or(TunnelError::InternalError(
            "no async runtime found".to_owned(),
        ))?;
        let socket = runtime.wrap_udp_socket(socket.into_std()?)?;
        let endpoint_config = if stealth.is_some() {
            stealth_endpoint_config()
        } else {
            endpoint_config()
        };
        let (socket, stealth_socket, connection_session) = if let Some((template, remote_addr)) =
            stealth
        {
            let stealth_socket = Arc::new(QuicStealthSocket::new(socket, template));
            let connection_session = remote_addr.map(|addr| stealth_socket.register_outbound(addr));
            let socket: Arc<dyn AsyncUdpSocket> = stealth_socket.clone();
            (socket, Some(stealth_socket), connection_session)
        } else {
            (socket, None, None)
        };
        let mut endpoint =
            Endpoint::new_with_abstract_socket(endpoint_config, None, socket, runtime)?;
        endpoint.set_default_client_config(client_config());
        Ok((endpoint, stealth_socket, connection_session))
    }

    fn try_create(
        addr: SocketAddr,
        dual_stack: bool,
        socket_mark: Option<u32>,
    ) -> Result<Endpoint, TunnelError> {
        Self::try_create_with_stealth(addr, dual_stack, socket_mark, None)
            .map(|(endpoint, _, _)| endpoint)
    }

    fn validate_server_bind(
        endpoint: &Endpoint,
        requested: SocketAddr,
        bind_mode: QuicBindMode,
    ) -> Result<(), TunnelError> {
        let actual = match endpoint.local_addr() {
            Ok(actual) => actual,
            Err(error) => {
                endpoint.close(0u32.into(), b"invalid server bind");
                return Err(error.into());
            }
        };
        let port_matches = if requested.port() == 0 {
            actual.port() != 0
        } else {
            actual.port() == requested.port()
        };
        let ip_matches = if requested.ip().is_unspecified() {
            match bind_mode {
                QuicBindMode::V4Only => actual.is_ipv4(),
                QuicBindMode::V6Only | QuicBindMode::DualStack => actual.is_ipv6(),
            }
        } else {
            actual.ip() == requested.ip()
        };

        if port_matches && ip_matches {
            return Ok(());
        }

        endpoint.close(0u32.into(), b"invalid server bind");
        Err(TunnelError::InternalError(format!(
            "QUIC server bind mismatch: requested={requested}, actual={actual}, mode={bind_mode:?}"
        )))
    }

    fn create<F>(
        &self,
        socket_mark: Option<u32>,
        mut selector: F,
    ) -> Result<(&RwPool<Endpoint>, Option<Endpoint>), TunnelError>
    where
        F: FnMut(&QuicEndpointManager) -> (&RwPool<Endpoint>, Option<(SocketAddr, bool)>),
    {
        loop {
            let (pool, r) = selector(self);
            let Some((addr, dual_stack)) = r else {
                return Ok((pool, None));
            };

            let endpoint = Self::try_create(addr, dual_stack, socket_mark);
            if let Err(error) = endpoint.as_ref()
                && dual_stack
            {
                tracing::warn!(?error, "create dual stack quic endpoint failed");
                self.both.disable();
                self.ipv4.enable();
                self.ipv6.enable();
                continue;
            }

            return Ok((pool, Some(endpoint?)));
        }
    }
}

impl QuicEndpointManager {
    fn new(capacity: usize) -> Self {
        let ipv4 = RwPool::new(capacity.div_ceil(2));
        let ipv6 = RwPool::new(capacity.div_ceil(2));
        let both = RwPool::new(capacity);
        both.enable();
        Self { ipv4, ipv6, both }
    }

    fn load(global_ctx: &ArcGlobalCtx) -> &Self {
        let capacity = global_ctx
            .config
            .get_flags()
            .multi_thread
            .then(std::thread::available_parallelism)
            .and_then(|r| r.ok())
            .map(|n| n.get())
            .unwrap_or(1);

        let mgr = QUIC_ENDPOINT_MANAGER.get();
        match mgr {
            Some(mgr) => {
                for pool in [&mgr.ipv4, &mgr.ipv6, &mgr.both] {
                    pool.resize();
                }
            }
            None => {
                let _ = QUIC_ENDPOINT_MANAGER.set(Self::new(capacity));
            }
        }

        QUIC_ENDPOINT_MANAGER.get().unwrap()
    }

    fn client_pool(&self, ip_version: IpVersion) -> &RwPool<Endpoint> {
        let dual_stack = self.both.is_enabled();
        match ip_version {
            IpVersion::V4 if !dual_stack => &self.ipv4,
            _ => {
                if dual_stack {
                    &self.both
                } else {
                    &self.ipv6
                }
            }
        }
    }

    /// Get a QUIC endpoint to be used as a server
    ///
    /// # Arguments
    /// * `addr`: listen address
    fn server(
        global_ctx: &ArcGlobalCtx,
        addr: SocketAddr,
        bind_mode: QuicBindMode,
        stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
    ) -> Result<(Endpoint, Option<Arc<QuicStealthSocket>>), TunnelError> {
        let mgr = Self::load(global_ctx);
        let socket_mark = global_ctx.config.get_flags().socket_mark;
        bind_mode.validate_address_family(addr)?;
        let dual_stack = bind_mode.dual_stack();

        if stealth.is_enabled() {
            let (endpoint, stealth_socket, _) = Self::try_create_with_stealth(
                addr,
                dual_stack,
                socket_mark,
                Some((stealth, None)),
            )?;
            Self::validate_server_bind(&endpoint, addr, bind_mode)?;
            endpoint.set_server_config(Some(stealth_server_config()));
            return Ok((endpoint, stealth_socket));
        }

        let endpoint = Self::try_create(addr, dual_stack, socket_mark)?;
        Self::validate_server_bind(&endpoint, addr, bind_mode)?;
        endpoint.set_server_config(Some(server_config()));
        let pool = match bind_mode {
            QuicBindMode::V4Only => &mgr.ipv4,
            QuicBindMode::V6Only => &mgr.ipv6,
            QuicBindMode::DualStack => &mgr.both,
        };
        pool.push(endpoint.clone());

        Ok((endpoint, None))
    }

    fn client_endpoint(
        &self,
        ip_version: IpVersion,
        socket_mark: Option<u32>,
    ) -> Result<Endpoint, TunnelError> {
        let (pool, endpoint) = self.create(socket_mark, |mgr| {
            let dual_stack = mgr.both.is_enabled();
            let (pool, addr) = match ip_version {
                IpVersion::V4 if !dual_stack => (&mgr.ipv4, (Ipv4Addr::UNSPECIFIED, 0).into()),
                _ => {
                    let pool = if dual_stack { &mgr.both } else { &mgr.ipv6 };
                    (pool, (Ipv6Addr::UNSPECIFIED, 0).into())
                }
            };
            if pool.is_full() {
                (pool, None)
            } else {
                (pool, Some((addr, dual_stack)))
            }
        })?;

        if let Some(endpoint) = endpoint {
            pool.try_push(endpoint);
        }

        Ok(pool.with_iter(|iter| iter.min_by_key(|e| e.open_connections()).unwrap().clone()))
    }

    fn remove_endpoint(&self, endpoint: &Endpoint) -> usize {
        let Ok(local_addr) = endpoint.local_addr() else {
            return 0;
        };
        self.remove_endpoint_by_local_addr(local_addr)
    }

    fn remove_endpoint_by_local_addr(&self, local_addr: SocketAddr) -> usize {
        [&self.ipv4, &self.ipv6, &self.both]
            .into_iter()
            .map(|pool| pool.remove_by_local_addr(local_addr))
            .sum()
    }

    fn contains_local_addr(&self, local_addr: SocketAddr) -> bool {
        [&self.ipv4, &self.ipv6, &self.both]
            .into_iter()
            .any(|pool| pool.contains_local_addr(local_addr))
    }

    async fn connect(
        global_ctx: &ArcGlobalCtx,
        addr: SocketAddr,
    ) -> Result<(Endpoint, Connection), TunnelError> {
        let ip_version = if addr.ip().is_ipv4() {
            IpVersion::V4
        } else {
            IpVersion::V6
        };
        let socket_mark = global_ctx.config.get_flags().socket_mark;
        Self::load(global_ctx)
            .connect_with_ip_version(addr, ip_version, socket_mark)
            .await
    }

    async fn connect_stealth(
        global_ctx: &ArcGlobalCtx,
        addr: SocketAddr,
        stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
    ) -> Result<(Endpoint, Connection, Arc<QuicStealthSession>), TunnelError> {
        let bind_addr = if addr.is_ipv4() {
            SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))
        } else {
            SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0))
        };
        let socket_mark = global_ctx.config.get_flags().socket_mark;
        let (endpoint, _, session) = Self::try_create_with_stealth(
            bind_addr,
            false,
            socket_mark,
            Some((stealth, Some(addr))),
        )?;
        let connection = endpoint
            .connect(addr, "localhost")
            .map_err(|error| {
                anyhow::Error::new(error).context(format!("failed to connect to {}", addr))
            })?
            .await
            .with_context(|| format!("failed to connect to {}", addr))?;
        Ok((
            endpoint,
            connection,
            session.expect("stealth client endpoint must have a remote session"),
        ))
    }

    async fn connect_with_ip_version(
        &self,
        addr: SocketAddr,
        ip_version: IpVersion,
        socket_mark: Option<u32>,
    ) -> Result<(Endpoint, Connection), TunnelError> {
        let max_endpoint_stopping_retries = self.client_pool(ip_version).len().saturating_add(1);
        let mut endpoint_stopping_retries = 0;

        loop {
            let endpoint = self.client_endpoint(ip_version, socket_mark)?;
            let connecting = match endpoint.connect(addr, "localhost") {
                Ok(connecting) => connecting,
                Err(ConnectError::EndpointStopping) => {
                    let local_addr = endpoint.local_addr().ok();
                    let removed = self.remove_endpoint(&endpoint);
                    endpoint_stopping_retries += 1;
                    tracing::warn!(
                        ?addr,
                        ?local_addr,
                        removed,
                        "removed stopped quic endpoint and retry connect"
                    );
                    if endpoint_stopping_retries > max_endpoint_stopping_retries {
                        return Err(anyhow::Error::new(ConnectError::EndpointStopping)
                            .context(format!("failed to create connection to {}", addr))
                            .into());
                    }
                    continue;
                }
                Err(e) => {
                    return Err(anyhow::Error::new(e)
                        .context(format!("failed to create connection to {}", addr))
                        .into());
                }
            };
            let connection = connecting
                .await
                .with_context(|| format!("failed to connect to {}", addr))?;

            return Ok((endpoint, connection));
        }
    }
}
//endregion

struct ConnWrapper {
    conn: Connection,
    stealth_cleanup: Option<(Arc<QuicStealthSocket>, SocketAddr, Arc<QuicStealthSession>)>,
}

impl Drop for ConnWrapper {
    fn drop(&mut self) {
        self.conn.close(0u32.into(), b"done");
        if let Some((socket, remote_addr, session)) = &self.stealth_cleanup {
            socket.remove_session_if_same(*remote_addr, session);
        }
    }
}

pub struct QuicTunnelListener {
    addr: url::Url,
    global_ctx: ArcGlobalCtx,
    endpoint: Option<Endpoint>,
    stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
    stealth_socket: Option<Arc<QuicStealthSocket>>,
    bind_mode: Option<QuicBindMode>,
}

impl QuicTunnelListener {
    pub fn new(addr: url::Url, global_ctx: ArcGlobalCtx) -> Self {
        let bind_mode = match addr.host() {
            Some(url::Host::Ipv4(_)) => Some(QuicBindMode::V4Only),
            Some(url::Host::Ipv6(ip)) if ip.is_unspecified() => Some(QuicBindMode::DualStack),
            Some(url::Host::Ipv6(_)) => Some(QuicBindMode::V6Only),
            Some(url::Host::Domain(host)) => host.parse::<IpAddr>().ok().map(|ip| match ip {
                IpAddr::V4(_) => QuicBindMode::V4Only,
                IpAddr::V6(ip) if ip.is_unspecified() => QuicBindMode::DualStack,
                IpAddr::V6(_) => QuicBindMode::V6Only,
            }),
            None => None,
        };
        Self::new_inner(addr, global_ctx, bind_mode)
    }

    pub(crate) fn new_with_bind_mode(
        addr: url::Url,
        global_ctx: ArcGlobalCtx,
        bind_mode: QuicBindMode,
    ) -> Self {
        Self::new_inner(addr, global_ctx, Some(bind_mode))
    }

    fn new_inner(
        addr: url::Url,
        global_ctx: ArcGlobalCtx,
        bind_mode: Option<QuicBindMode>,
    ) -> Self {
        QuicTunnelListener {
            addr,
            global_ctx,
            endpoint: None,
            stealth: Arc::new(crate::tunnel::stealth::OuterSessionState::disabled()),
            stealth_socket: None,
            bind_mode,
        }
    }

    pub fn set_stealth(&mut self, stealth: Arc<crate::tunnel::stealth::OuterSessionState>) {
        self.stealth = stealth;
    }

    async fn do_accept(&self) -> Result<Box<dyn Tunnel>, super::TunnelError> {
        // accept a single connection
        let conn = self
            .endpoint
            .as_ref()
            .unwrap()
            .accept()
            .await
            .ok_or_else(|| anyhow::anyhow!("accept failed, no incoming"))?;
        let conn = conn.await.with_context(|| "accept connection failed")?;
        let remote_addr = conn.remote_address();
        let (w, r) = conn.accept_bi().await.with_context(|| "accept_bi failed")?;
        let connection_stealth_session = match &self.stealth_socket {
            Some(socket) => Some(
                socket
                    .session(remote_addr)
                    .ok_or_else(|| anyhow::anyhow!("missing accepted QUIC stealth session"))?,
            ),
            None => None,
        };
        let connection_stealth = connection_stealth_session
            .as_ref()
            .map(|session| session.state.clone());

        let stealth_cleanup = if let (Some(socket), Some(session)) =
            (&self.stealth_socket, &connection_stealth_session)
        {
            Some((socket.clone(), remote_addr, session.clone()))
        } else {
            None
        };
        let arc_conn = Arc::new(ConnWrapper {
            conn,
            stealth_cleanup,
        });

        let info = TunnelInfo {
            tunnel_type: "quic".to_owned(),
            local_addr: Some(self.local_url().into()),
            remote_addr: Some(
                super::build_url_from_socket_addr(&remote_addr.to_string(), "quic").into(),
            ),
            resolved_remote_addr: Some(
                super::build_url_from_socket_addr(&remote_addr.to_string(), "quic").into(),
            ),
        };

        Ok(Box::new(TunnelWrapper::new_with_associate_data(
            FramedReader::new_with_associate_data(r, 2000, Some(Box::new(arc_conn.clone()))),
            FramedWriter::new_with_associate_data(w, Some(Box::new(arc_conn))),
            Some(info),
            connection_stealth.map(|state| Box::new(state) as Box<dyn std::any::Any + Send>),
        )))
    }
}

impl Drop for QuicTunnelListener {
    fn drop(&mut self) {
        let Some(endpoint) = &self.endpoint else {
            return;
        };
        let Ok(local_addr) = endpoint.local_addr() else {
            return;
        };
        QuicEndpointManager::load(&self.global_ctx).remove_endpoint_by_local_addr(local_addr);
    }
}

#[async_trait::async_trait]
impl TunnelListener for QuicTunnelListener {
    async fn listen(&mut self) -> Result<(), TunnelError> {
        let addr = SocketAddr::from_url(self.addr.clone(), IpVersion::Both).await?;
        let bind_mode = self.bind_mode.unwrap_or_else(|| {
            if addr.is_ipv4() {
                QuicBindMode::V4Only
            } else {
                QuicBindMode::V6Only
            }
        });
        let (endpoint, stealth_socket) =
            QuicEndpointManager::server(&self.global_ctx, addr, bind_mode, self.stealth.clone())?;
        self.addr
            .set_port(Some(endpoint.local_addr()?.port()))
            .unwrap();
        self.endpoint = Some(endpoint);
        self.stealth_socket = stealth_socket;
        self.bind_mode = Some(bind_mode);

        Ok(())
    }

    async fn accept(&mut self) -> Result<Box<dyn Tunnel>, super::TunnelError> {
        loop {
            match self.do_accept().await {
                Ok(ret) => return Ok(ret),
                Err(e) => {
                    tracing::warn!(?e, "accept fail");
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
        }
    }

    fn local_url(&self) -> url::Url {
        self.addr.clone()
    }
}

pub struct QuicTunnelConnector {
    addr: url::Url,
    global_ctx: ArcGlobalCtx,
    ip_version: IpVersion,
    resolved_addr: Option<SocketAddr>,
    stealth_candidate: Arc<crate::tunnel::stealth::OuterSessionState>,
    stealth_mode: QuicStealthMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuicStealthMode {
    Disabled,
    Required,
    PreferLegacyFallback,
}

impl QuicTunnelConnector {
    pub fn new(addr: url::Url, global_ctx: ArcGlobalCtx) -> Self {
        QuicTunnelConnector {
            addr,
            global_ctx,
            ip_version: IpVersion::Both,
            resolved_addr: None,
            stealth_candidate: Arc::new(crate::tunnel::stealth::OuterSessionState::disabled()),
            stealth_mode: QuicStealthMode::Disabled,
        }
    }

    pub fn set_stealth_candidate(
        &mut self,
        stealth: Arc<crate::tunnel::stealth::OuterSessionState>,
    ) {
        self.stealth_mode = if stealth.is_enabled() {
            QuicStealthMode::PreferLegacyFallback
        } else {
            QuicStealthMode::Disabled
        };
        self.stealth_candidate = stealth;
    }
}

#[async_trait::async_trait]
impl TunnelConnector for QuicTunnelConnector {
    async fn connect(&mut self) -> Result<Box<dyn Tunnel>, TunnelError> {
        let addr = match self.resolved_addr {
            Some(addr) => addr,
            None => SocketAddr::from_url(self.addr.clone(), self.ip_version).await?,
        };
        let (endpoint, connection, connection_stealth) = match self.stealth_mode {
            QuicStealthMode::Disabled => {
                let (endpoint, connection) =
                    QuicEndpointManager::connect(&self.global_ctx, addr).await?;
                (endpoint, connection, None)
            }
            QuicStealthMode::Required => {
                let (endpoint, connection, session) = QuicEndpointManager::connect_stealth(
                    &self.global_ctx,
                    addr,
                    self.stealth_candidate.clone(),
                )
                .await?;
                (endpoint, connection, Some(session.state.clone()))
            }
            QuicStealthMode::PreferLegacyFallback => {
                match tokio::time::timeout(
                    QUIC_STEALTH_FALLBACK_TIMEOUT,
                    QuicEndpointManager::connect_stealth(
                        &self.global_ctx,
                        addr,
                        self.stealth_candidate.clone(),
                    ),
                )
                .await
                {
                    Ok(Ok((endpoint, connection, session))) => {
                        (endpoint, connection, Some(session.state.clone()))
                    }
                    result => {
                        tracing::info!(
                            ?addr,
                            ?result,
                            "QUIC stealth attempt failed, retrying legacy wire format"
                        );
                        let (endpoint, connection) =
                            QuicEndpointManager::connect(&self.global_ctx, addr).await?;
                        (endpoint, connection, None)
                    }
                }
            }
        };

        let local_addr = endpoint.local_addr()?;

        let (w, r) = connection
            .open_bi()
            .await
            .with_context(|| "open_bi failed")?;

        let info = TunnelInfo {
            tunnel_type: "quic".to_owned(),
            local_addr: Some(
                super::build_url_from_socket_addr(&local_addr.to_string(), "quic").into(),
            ),
            remote_addr: Some(self.addr.clone().into()),
            resolved_remote_addr: Some(
                super::build_url_from_socket_addr(&connection.remote_address().to_string(), "quic")
                    .into(),
            ),
        };

        let arc_conn = Arc::new(ConnWrapper {
            conn: connection,
            stealth_cleanup: None,
        });
        Ok(Box::new(TunnelWrapper::new_with_associate_data(
            FramedReader::new_with_associate_data(r, 4500, Some(Box::new(arc_conn.clone()))),
            FramedWriter::new_with_associate_data(w, Some(Box::new(arc_conn))),
            Some(info),
            connection_stealth.map(|state| Box::new(state) as Box<dyn std::any::Any + Send>),
        )))
    }

    fn remote_url(&self) -> url::Url {
        self.addr.clone()
    }

    fn set_ip_version(&mut self, ip_version: IpVersion) {
        self.ip_version = ip_version;
    }

    fn set_resolved_addr(&mut self, addr: SocketAddr) {
        self.resolved_addr = Some(addr);
    }

    fn disable_stealth(&mut self) {
        self.stealth_mode = QuicStealthMode::Disabled;
    }

    fn require_stealth(&mut self) {
        if self.stealth_candidate.is_enabled() {
            self.stealth_mode = QuicStealthMode::Required;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::common::global_ctx::tests::get_mock_global_ctx_with_network;
    use crate::tunnel::{
        TunnelConnector,
        common::tests::{_tunnel_bench, _tunnel_pingpong},
    };
    use std::sync::LazyLock;
    use tokio::runtime::{Builder, Runtime};

    use super::*;

    // Shared runtime for all tests to avoid endpoint invalidation across runtimes
    static RUNTIME: LazyLock<Runtime> =
        LazyLock::new(|| Builder::new_multi_thread().enable_all().build().unwrap());

    fn global_ctx() -> ArcGlobalCtx {
        let identity = crate::common::config::NetworkIdentity::default();
        get_mock_global_ctx_with_network(Some(identity))
    }

    fn stealth(secret: &str) -> Arc<crate::tunnel::stealth::OuterSessionState> {
        crate::tunnel::stealth::build_outer_session(Some(secret), true, true, 60)
    }

    fn stopped_client_endpoint() -> (Endpoint, SocketAddr) {
        let rt = Builder::new_current_thread().enable_all().build().unwrap();
        let endpoint = rt.block_on(async {
            QuicEndpointManager::try_create((Ipv4Addr::UNSPECIFIED, 0).into(), false, None).unwrap()
        });
        let local_addr = endpoint.local_addr().unwrap();
        drop(rt);
        assert!(matches!(
            endpoint.connect("127.0.0.1:1".parse().unwrap(), "localhost"),
            Err(ConnectError::EndpointStopping)
        ));
        (endpoint, local_addr)
    }

    #[test]
    fn quic_bind_mode_requires_matching_address_family() {
        assert!(
            QuicBindMode::V4Only
                .validate_address_family("0.0.0.0:11012".parse().unwrap())
                .is_ok()
        );
        assert!(
            QuicBindMode::V6Only
                .validate_address_family("[::1]:11012".parse().unwrap())
                .is_ok()
        );
        assert!(
            QuicBindMode::DualStack
                .validate_address_family("[::]:11012".parse().unwrap())
                .is_ok()
        );
        assert!(
            QuicBindMode::DualStack
                .validate_address_family("[::1]:11012".parse().unwrap())
                .is_err()
        );
        assert!(
            QuicBindMode::V6Only
                .validate_address_family("0.0.0.0:11012".parse().unwrap())
                .is_err()
        );
    }

    #[test]
    fn strict_server_bind_validation_rejects_wrong_family_and_port() {
        RUNTIME.block_on(async {
            let endpoint =
                QuicEndpointManager::try_create((Ipv4Addr::LOCALHOST, 0).into(), false, None)
                    .unwrap();
            let actual = endpoint.local_addr().unwrap();
            let wrong_family = SocketAddr::from((Ipv6Addr::LOCALHOST, actual.port()));
            assert!(
                QuicEndpointManager::validate_server_bind(
                    &endpoint,
                    wrong_family,
                    QuicBindMode::V6Only,
                )
                .is_err()
            );

            let endpoint =
                QuicEndpointManager::try_create((Ipv4Addr::LOCALHOST, 0).into(), false, None)
                    .unwrap();
            let actual = endpoint.local_addr().unwrap();
            let wrong_port_number = if actual.port() == u16::MAX {
                actual.port() - 1
            } else {
                actual.port() + 1
            };
            let wrong_port = SocketAddr::from((Ipv4Addr::LOCALHOST, wrong_port_number));
            assert!(
                QuicEndpointManager::validate_server_bind(
                    &endpoint,
                    wrong_port,
                    QuicBindMode::V4Only,
                )
                .is_err()
            );
        });
    }

    #[test]
    fn quic_pingpong() {
        RUNTIME.block_on(quic_pingpong_impl())
    }
    async fn quic_pingpong_impl() {
        let listener = QuicTunnelListener::new("quic://[::]:21011".parse().unwrap(), global_ctx());
        let connector =
            QuicTunnelConnector::new("quic://127.0.0.1:21011".parse().unwrap(), global_ctx());
        _tunnel_pingpong(listener, connector).await
    }

    #[test]
    fn quic_stealth_pingpong() {
        RUNTIME.block_on(async {
            let mut listener =
                QuicTunnelListener::new("quic://127.0.0.1:21013".parse().unwrap(), global_ctx());
            listener.set_stealth(stealth("quic-secret"));
            let mut connector =
                QuicTunnelConnector::new("quic://127.0.0.1:21013".parse().unwrap(), global_ctx());
            connector.set_stealth_candidate(stealth("quic-secret"));
            TunnelConnector::require_stealth(&mut connector);
            _tunnel_pingpong(listener, connector).await;
        })
    }

    #[test]
    fn quic_unknown_capability_falls_back_to_plain() {
        RUNTIME.block_on(async {
            let listener =
                QuicTunnelListener::new("quic://127.0.0.1:21014".parse().unwrap(), global_ctx());
            let mut connector =
                QuicTunnelConnector::new("quic://127.0.0.1:21014".parse().unwrap(), global_ctx());
            connector.set_stealth_candidate(stealth("quic-secret"));
            _tunnel_pingpong(listener, connector).await;
        })
    }

    async fn assert_strict_stealth_listener_rejects(mut connector: QuicTunnelConnector) {
        let mut listener =
            QuicTunnelListener::new("quic://127.0.0.1:0".parse().unwrap(), global_ctx());
        listener.set_stealth(stealth("listener-secret"));
        listener.listen().await.unwrap();
        connector.addr = listener.local_url();
        let accept_task = tokio::spawn(async move { listener.accept().await });

        let _ = tokio::time::timeout(Duration::from_millis(300), connector.connect()).await;
        assert!(
            !accept_task.is_finished(),
            "strict QUIC stealth listener exposed an unauthenticated connection"
        );
        accept_task.abort();
    }

    #[test]
    fn quic_stealth_listener_rejects_plain_and_wrong_secret() {
        RUNTIME.block_on(async {
            assert_strict_stealth_listener_rejects(QuicTunnelConnector::new(
                "quic://127.0.0.1:0".parse().unwrap(),
                global_ctx(),
            ))
            .await;

            let mut wrong_secret =
                QuicTunnelConnector::new("quic://127.0.0.1:0".parse().unwrap(), global_ctx());
            wrong_secret.set_stealth_candidate(stealth("connector-secret"));
            TunnelConnector::require_stealth(&mut wrong_secret);
            assert_strict_stealth_listener_rejects(wrong_secret).await;
        })
    }

    #[test]
    fn quic_stealth_session_transitions_from_gate_to_outer_key() {
        let sender = QuicStealthSession::new(stealth("quic-secret"));
        let receiver = QuicStealthSession::new(stealth("quic-secret"));

        let gate_packet = sender.seal(b"gate").unwrap();
        assert_eq!(receiver.open(&gate_packet).unwrap(), b"gate");

        sender
            .state
            .set_outer_key_from_handshake_hash(b"quic-handshake");
        receiver
            .state
            .set_outer_key_from_handshake_hash(b"quic-handshake");
        let transition_packet = sender.seal(b"transition").unwrap();
        assert_eq!(receiver.open(&transition_packet).unwrap(), b"transition");

        sender
            .state
            .set_outer_key_age_for_test(QUIC_STEALTH_OUTER_SEND_DELAY);
        receiver
            .state
            .set_outer_key_age_for_test(QUIC_STEALTH_OUTER_SEND_DELAY);
        let outer_packet = sender.seal(b"outer").unwrap();
        assert_eq!(receiver.open(&outer_packet).unwrap(), b"outer");

        receiver
            .state
            .set_outer_key_age_for_test(QUIC_STEALTH_GATE_RECV_GRACE + Duration::from_millis(1));
        let stale_gate = sender.state.seal_gate_datagram(b"stale").unwrap();
        assert!(receiver.open(&stale_gate).is_none());
    }

    #[test]
    fn quic_stealth_socket_does_not_replace_live_phase2_session() {
        RUNTIME.block_on(async {
            let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
            udp.set_nonblocking(true).unwrap();
            let runtime = default_runtime().unwrap();
            let inner = runtime.wrap_udp_socket(udp).unwrap();
            let socket = QuicStealthSocket::new(inner, stealth("quic-secret"));
            let remote = SocketAddr::from((Ipv4Addr::LOCALHOST, 12345));
            let sender = stealth("quic-secret");
            let initial = [0xc0, 0, 0, 0, 1, 8, 0, 0];

            let first = sender.seal_gate_datagram(&initial).unwrap();
            let (_, old_session) = socket.open_from(remote, &first).unwrap();
            old_session.state.set_outer_key_from_handshake_hash(b"old");

            let reconnect_during_grace = sender.seal_gate_datagram(&initial).unwrap();
            assert!(
                socket.open_from(remote, &reconnect_during_grace).is_none(),
                "gate-key Initial reused a live phase-2 QUIC session during grace"
            );
            assert!(Arc::ptr_eq(&socket.session(remote).unwrap(), &old_session));

            old_session.state.set_outer_key_age_for_test(
                QUIC_STEALTH_GATE_RECV_GRACE + Duration::from_millis(1),
            );

            let gate_data = sender
                .seal_gate_datagram(&[0x40, 0, 0, 0, 1, 8, 0, 0])
                .unwrap();
            assert!(
                socket.open_from(remote, &gate_data).is_none(),
                "gate-key non-Initial data reopened a phase-2 QUIC session"
            );

            let reconnect = sender.seal_gate_datagram(&initial).unwrap();
            assert!(
                socket.open_from(remote, &reconnect).is_none(),
                "gate-key Initial replaced a live phase-2 QUIC session"
            );
            assert!(Arc::ptr_eq(&socket.session(remote).unwrap(), &old_session));

            assert!(socket.remove_session_if_same(remote, &old_session));
            let (_, new_session) = socket.open_from(remote, &reconnect).unwrap();
            assert!(!Arc::ptr_eq(&old_session, &new_session));

            new_session.state.set_outer_key_from_handshake_hash(b"new");
            new_session.state.set_outer_key_age_for_test(
                QUIC_STEALTH_GATE_RECV_GRACE + Duration::from_millis(1),
            );
            assert!(
                socket.open_from(remote, &reconnect).is_none(),
                "replayed QUIC Initial replaced the live session twice"
            );
            assert!(Arc::ptr_eq(&socket.session(remote).unwrap(), &new_session));
        })
    }

    #[test]
    fn quic_bench() {
        RUNTIME.block_on(quic_bench_impl())
    }
    async fn quic_bench_impl() {
        let listener = QuicTunnelListener::new("quic://[::]:21012".parse().unwrap(), global_ctx());
        let connector =
            QuicTunnelConnector::new("quic://127.0.0.1:21012".parse().unwrap(), global_ctx());
        _tunnel_bench(listener, connector).await
    }

    #[test]
    fn ipv6_pingpong() {
        RUNTIME.block_on(ipv6_pingpong_impl())
    }
    async fn ipv6_pingpong_impl() {
        let listener = QuicTunnelListener::new("quic://[::1]:31015".parse().unwrap(), global_ctx());
        let connector =
            QuicTunnelConnector::new("quic://[::1]:31015".parse().unwrap(), global_ctx());
        _tunnel_pingpong(listener, connector).await
    }

    #[test]
    fn ipv6_domain_pingpong() {
        RUNTIME.block_on(ipv6_domain_pingpong_impl())
    }
    async fn ipv6_domain_pingpong_impl() {
        let listener = QuicTunnelListener::new("quic://[::1]:31016".parse().unwrap(), global_ctx());
        let mut connector = QuicTunnelConnector::new(
            "quic://test.easytier.top:31016".parse().unwrap(),
            global_ctx(),
        );
        connector.set_ip_version(IpVersion::V6);
        _tunnel_pingpong(listener, connector).await;

        let listener =
            QuicTunnelListener::new("quic://127.0.0.1:31016".parse().unwrap(), global_ctx());
        let mut connector = QuicTunnelConnector::new(
            "quic://test.easytier.top:31016".parse().unwrap(),
            global_ctx(),
        );
        connector.set_ip_version(IpVersion::V4);
        _tunnel_pingpong(listener, connector).await;
    }

    #[test]
    fn alloc_port() {
        RUNTIME.block_on(alloc_port_impl())
    }
    async fn alloc_port_impl() {
        // v4
        let mut listener =
            QuicTunnelListener::new("quic://0.0.0.0:0".parse().unwrap(), global_ctx());
        listener.listen().await.unwrap();
        let port = listener.local_url().port().unwrap();
        assert!(port > 0);

        // v6
        let mut listener = QuicTunnelListener::new("quic://[::]:0".parse().unwrap(), global_ctx());
        listener.listen().await.unwrap();
        let port = listener.local_url().port().unwrap();
        assert!(port > 0);
    }

    #[test]
    fn listener_drop_removes_persistent_endpoint() {
        RUNTIME.block_on(listener_drop_removes_persistent_endpoint_impl())
    }
    async fn listener_drop_removes_persistent_endpoint_impl() {
        let global_ctx = global_ctx();
        let endpoint_addr = {
            let mut listener =
                QuicTunnelListener::new("quic://127.0.0.1:0".parse().unwrap(), global_ctx.clone());
            listener.listen().await.unwrap();
            let endpoint_addr = listener.endpoint.as_ref().unwrap().local_addr().unwrap();
            assert!(QuicEndpointManager::load(&global_ctx).contains_local_addr(endpoint_addr));
            endpoint_addr
        };

        assert!(!QuicEndpointManager::load(&global_ctx).contains_local_addr(endpoint_addr));
    }

    #[test]
    fn connect_removes_stopped_endpoints_and_retries() {
        let (stopped_endpoint_a, stopped_addr_a) = stopped_client_endpoint();
        let (stopped_endpoint_b, stopped_addr_b) = stopped_client_endpoint();

        RUNTIME.block_on(async move {
            let mgr = QuicEndpointManager::new(2);
            mgr.both.push(stopped_endpoint_a);
            mgr.both.push(stopped_endpoint_b);
            assert!(mgr.contains_local_addr(stopped_addr_a));
            assert!(mgr.contains_local_addr(stopped_addr_b));

            let err = mgr
                .connect_with_ip_version("127.0.0.1:0".parse().unwrap(), IpVersion::V4, None)
                .await
                .unwrap_err();
            let err = format!("{:?}", err);
            assert!(
                err.contains("invalid remote address"),
                "unexpected error: {}",
                err
            );
            assert!(!mgr.contains_local_addr(stopped_addr_a));
            assert!(!mgr.contains_local_addr(stopped_addr_b));
        });
    }

    #[test]
    fn invalid_peer_addr() {
        RUNTIME.block_on(invalid_peer_addr_impl())
    }
    async fn invalid_peer_addr_impl() {
        let mut connector =
            QuicTunnelConnector::new("quic://127.0.0.1:0".parse().unwrap(), global_ctx());
        let err = format!("{:?}", connector.connect().await.unwrap_err());
        assert!(
            err.contains("invalid remote address"),
            "unexpected error: {}",
            err
        );
    }
}
