use std::{
    fmt::Debug,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    sync::{Arc, Weak},
    time::Duration,
};

use anyhow::Context;
use async_trait::async_trait;
use bytes::BytesMut;
use dashmap::DashMap;
use futures::{StreamExt, stream::FuturesUnordered};
use rand::{Rng, SeedableRng};
use zerocopy::{AsBytes, FromBytes};

use tokio::{
    net::UdpSocket,
    sync::mpsc::{Receiver, Sender, UnboundedReceiver, UnboundedSender},
    task::JoinSet,
};
use tokio_util::task::AbortOnDropHandle;
use tracing::{Instrument, instrument};

use super::{
    FromUrl, IpVersion, Tunnel, TunnelConnCounter, TunnelError, TunnelInfo, TunnelListener,
    TunnelUrl,
    common::wait_for_connect_futures,
    packet_def::{UDP_TUNNEL_HEADER_SIZE, UDPTunnelHeader, V4HolePunchPacket, V6HolePunchPacket},
    ring::{RingSink, RingStream},
};
use crate::tunnel::common::bind;
use crate::{
    common::{join_joinset_background, shrink_dashmap},
    tunnel::{
        build_url_from_socket_addr,
        common::{TunnelWrapper, reserve_buf},
        packet_def::{UdpPacketType, ZCPacket, ZCPacketType},
        ring::RingTunnel,
        udp_src,
    },
};

pub const UDP_DATA_MTU: usize = 2000;

type UdpCloseEventSender = UnboundedSender<(SocketAddr, Option<TunnelError>)>;
type UdpCloseEventReceiver = UnboundedReceiver<(SocketAddr, Option<TunnelError>)>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreferredIpv6Source {
    pub ip: Ipv6Addr,
    pub ifindex: u32,
}

fn new_udp_packet<F>(f: F, udp_body: Option<&[u8]>) -> ZCPacket
where
    F: FnOnce(&mut UDPTunnelHeader),
{
    let mut buf = BytesMut::new();
    buf.resize(
        UDP_TUNNEL_HEADER_SIZE + udp_body.as_ref().map(|v| v.len()).unwrap_or(0),
        0,
    );
    buf[UDP_TUNNEL_HEADER_SIZE..].copy_from_slice(udp_body.unwrap());

    let mut ret = ZCPacket::new_from_buf(buf, ZCPacketType::UDP);
    let header = ret.mut_udp_tunnel_header().unwrap();
    f(header);
    ret
}

fn new_syn_packet(conn_id: u32, magic: u64) -> ZCPacket {
    new_udp_packet(
        |header| {
            header.msg_type = UdpPacketType::Syn as u8;
            header.conn_id.set(conn_id);
            header.len.set(8);
        },
        Some(&magic.to_le_bytes()),
    )
}

fn new_sack_packet(conn_id: u32, magic: u64) -> ZCPacket {
    new_udp_packet(
        |header| {
            header.msg_type = UdpPacketType::Sack as u8;
            header.conn_id.set(conn_id);
            header.len.set(8);
        },
        Some(&magic.to_le_bytes()),
    )
}

fn new_syn_packet_token(conn_id: u32, token: &[u8]) -> ZCPacket {
    new_udp_packet(
        |header| {
            header.msg_type = UdpPacketType::Syn as u8;
            header.conn_id.set(conn_id);
            header.len.set(token.len() as u16);
        },
        Some(token),
    )
}

fn new_sack_packet_token(conn_id: u32, token: &[u8]) -> ZCPacket {
    new_udp_packet(
        |header| {
            header.msg_type = UdpPacketType::Sack as u8;
            header.conn_id.set(conn_id);
            header.len.set(token.len() as u16);
        },
        Some(token),
    )
}

pub fn new_hole_punch_packet(tid: u32, buf_len: u16) -> ZCPacket {
    // generate a 128 bytes vec with random data
    let mut rng = rand::rngs::StdRng::from_entropy();
    let mut buf = vec![0u8; buf_len as usize];
    rng.fill(&mut buf[..]);
    new_udp_packet(
        |header| {
            header.msg_type = UdpPacketType::HolePunch as u8;
            header.conn_id.set(tid);
            header.len.set(buf_len);
        },
        Some(&buf),
    )
}

pub fn new_v6_hole_punch_packet(
    dst: &SocketAddrV6,
    preferred_src: Option<PreferredIpv6Source>,
) -> ZCPacket {
    // generate a 128 bytes vec with random data
    let mut body = V6HolePunchPacket::default();
    body.dst_ipv6.copy_from_slice(&dst.ip().octets());
    body.dst_port.set(dst.port());
    if let Some(src) = preferred_src {
        body.preferred_src_ipv6.copy_from_slice(&src.ip.octets());
        body.preferred_src_ifindex.set(src.ifindex);
    }
    new_udp_packet(
        |header| {
            header.msg_type = UdpPacketType::V6HolePunch as u8;
            header.conn_id.set(dst.port() as u32);
            header
                .len
                .set(std::mem::size_of::<V6HolePunchPacket>() as u16);
        },
        Some(body.as_bytes()),
    )
}

pub fn new_v4_hole_punch_packet(dst: &SocketAddrV4) -> ZCPacket {
    let mut body = V4HolePunchPacket::default();
    body.dst_ipv4.copy_from_slice(&dst.ip().octets());
    body.dst_port.set(dst.port());
    new_udp_packet(
        |header| {
            header.msg_type = UdpPacketType::V4HolePunch as u8;
            header.conn_id.set(dst.port() as u32);
            header
                .len
                .set(std::mem::size_of::<V4HolePunchPacket>() as u16);
        },
        Some(body.as_bytes()),
    )
}

fn extract_dst_addr_from_v4_hole_punch_packet(buf: &[u8]) -> Option<SocketAddrV4> {
    let body = V4HolePunchPacket::ref_from_prefix(buf)?;
    let ip = Ipv4Addr::from(body.dst_ipv4);
    Some(SocketAddrV4::new(ip, body.dst_port.get()))
}

fn extract_v6_hole_punch_packet(buf: &[u8]) -> Option<(SocketAddrV6, Option<PreferredIpv6Source>)> {
    let body = V6HolePunchPacket::ref_from_prefix(buf)?;
    let ip = Ipv6Addr::from(body.dst_ipv6);
    let preferred_src_ipv6 = Ipv6Addr::from(body.preferred_src_ipv6);
    let preferred_src = (!preferred_src_ipv6.is_unspecified()).then_some(PreferredIpv6Source {
        ip: preferred_src_ipv6,
        ifindex: body.preferred_src_ifindex.get(),
    });
    Some((
        SocketAddrV6::new(ip, body.dst_port.get(), 0, 0),
        preferred_src,
    ))
}

fn is_stun_packet(b: &[u8]) -> bool {
    // stun has following pattern:
    // 1. first two bits are 0b00
    // 2. magic cookie between 32-64 bits: 0x2112A442
    b[4..8] == [0x21, 0x12, 0xA4, 0x42] && b[0] & 0xC0 == 0
}

pub async fn send_v6_hole_punch_packet(
    listener_port: u16,
    dst_addr: SocketAddrV6,
    preferred_src: Option<PreferredIpv6Source>,
) -> Result<(), TunnelError> {
    let local_socket = UdpSocket::bind("[::1]:0").await?;
    let udp_packet = new_v6_hole_punch_packet(&dst_addr, preferred_src);
    let remote_addr = format!("[::1]:{}", listener_port)
        .parse::<SocketAddr>()
        .unwrap();
    local_socket
        .send_to(&udp_packet.into_bytes(), remote_addr)
        .await?;
    Ok(())
}

pub async fn send_v4_hole_punch_packet(
    listener_port: u16,
    dst_addr: SocketAddrV4,
) -> Result<(), TunnelError> {
    let local_socket = UdpSocket::bind("127.0.0.1:0").await?;
    let udp_packet = new_v4_hole_punch_packet(&dst_addr);
    let remote_addr = format!("127.0.0.1:{}", listener_port)
        .parse::<SocketAddr>()
        .unwrap();
    local_socket
        .send_to(&udp_packet.into_bytes(), remote_addr)
        .await?;
    Ok(())
}

async fn respond_stun_packet(
    socket: Arc<UdpSocket>,
    addr: SocketAddr,
    req_buf: Vec<u8>,
) -> Result<(), anyhow::Error> {
    use crate::common::stun_codec_ext::*;
    use bytecodec::{DecodeExt as _, EncodeExt as _};
    use stun_codec::{
        Message, MessageClass, MessageDecoder, MessageEncoder,
        rfc5389::{attributes::XorMappedAddress, methods::BINDING},
    };

    let mut decoder = MessageDecoder::<Attribute>::new();
    let req_msg = decoder
        .decode_from_bytes(&req_buf)
        .map_err(|e| anyhow::anyhow!("stun decode error: {:?}", e))?
        .map_err(|e| anyhow::anyhow!("stun decode broken message error: {:?}", e))?;

    let tid = req_msg.transaction_id();
    // we only respond easytier stun req, whose tid has 0xdeadbeef prefix
    if tid.as_bytes()[0..4] != [0xde, 0xad, 0xbe, 0xef] {
        anyhow::bail!("stun req tid not from easytier");
    }

    let mut resp_msg = Message::<Attribute>::new(
        MessageClass::SuccessResponse,
        BINDING,
        // we discard the prefix, make sure our implementation is not compatible with other stun client
        u32_to_tid(tid_to_u32(&tid)),
    );
    resp_msg.add_attribute(Attribute::XorMappedAddress(XorMappedAddress::new(addr)));

    let mut encoder = MessageEncoder::new();
    let rsp_buf = encoder
        .encode_into_bytes(resp_msg.clone())
        .map_err(|e| anyhow::anyhow!("stun encode error: {:?}", e))?;

    let change_req = req_msg
        .get_attribute::<ChangeRequest>()
        .map(|r| r.ip() || r.port())
        .unwrap_or(false);

    if !change_req {
        socket
            .send_to(&rsp_buf, addr)
            .await
            .with_context(|| "send stun response error")?;
    } else {
        // send from a new udp socket
        let socket = if addr.is_ipv4() {
            UdpSocket::bind("0.0.0.0:0").await?
        } else {
            UdpSocket::bind("[::]:0").await?
        };
        socket.send_to(&rsp_buf, addr).await?;
    }

    tracing::debug!(?addr, ?req_msg, ?change_req, "udp respond stun packet done");
    Ok(())
}

fn get_zcpacket_from_buf(buf: BytesMut, allow_stun: bool) -> Result<ZCPacket, TunnelError> {
    let dg_size = buf.len();
    if dg_size < UDP_TUNNEL_HEADER_SIZE {
        return Err(TunnelError::InvalidPacket(format!(
            "udp packet size too small: {:?}, packet: {:?}",
            dg_size, buf
        )));
    }

    if allow_stun && is_stun_packet(&buf[..UDP_TUNNEL_HEADER_SIZE]) {
        return Ok(ZCPacket::new_from_buf(buf, ZCPacketType::UDP));
    }

    let zc_packet = ZCPacket::new_from_buf(buf, ZCPacketType::UDP);
    let header = zc_packet.udp_tunnel_header().unwrap();
    let payload_len = header.len.get() as usize;
    if payload_len != dg_size - UDP_TUNNEL_HEADER_SIZE {
        return Err(TunnelError::InvalidPacket(format!(
            "udp packet payload len not match: header len: {:?}, real len: {:?}",
            payload_len, dg_size
        )));
    }

    Ok(zc_packet)
}

#[instrument]
async fn forward_from_ring_to_udp(
    mut ring_recv: RingStream,
    socket: &Arc<UdpSocket>,
    addr: &SocketAddr,
    conn_id: u32,
    stealth: &std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
) -> Option<TunnelError> {
    tracing::debug!("udp forward from ring to udp");
    loop {
        let buf = ring_recv.next().await?;
        let packet = match buf {
            Ok(v) => v,
            Err(e) => {
                return Some(e);
            }
        };

        let mut packet = packet.convert_type(ZCPacketType::UDP);
        let udp_payload_len = packet.udp_payload().len();
        let header = packet.mut_udp_tunnel_header().unwrap();
        header.conn_id.set(conn_id);
        header.len.set(udp_payload_len as u16);
        header.msg_type = UdpPacketType::Data as u8;

        let buf = packet.into_bytes();
        tracing::trace!(?udp_payload_len, ?buf, "udp forward from ring to udp");
        // Stealth: seal the whole data datagram (tunnel + peer-manager headers
        // included) under the per-connection outer key so the underlay exposes
        // no fixed protocol fingerprint. No-op when stealth is disabled.
        let ret = match stealth.seal_datagram(&buf) {
            Some(sealed) => socket.send_to(&sealed, &addr).await,
            None => socket.send_to(&buf, &addr).await,
        };
        if ret.is_err() {
            return Some(TunnelError::IOError(ret.unwrap_err()));
        } else if ret.unwrap() == 0 {
            return None;
        }
    }
}

struct UdpConnection {
    socket: Arc<UdpSocket>,
    conn_id: u32,
    dst_addr: SocketAddr,
    stealth: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,

    ring_sender: RingSink,
    forward_task: AbortOnDropHandle<()>,
}

impl UdpConnection {
    pub fn new(
        socket: Arc<UdpSocket>,
        conn_id: u32,
        dst_addr: SocketAddr,
        stealth: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
        ring_sender: RingSink,
        ring_recv: RingStream,
        close_event_sender: UdpCloseEventSender,
    ) -> Self {
        let s = socket.clone();
        let send_stealth = stealth.clone();
        let forward_task = AbortOnDropHandle::new(tokio::spawn(async move {
            let close_event_sender = close_event_sender;
            let err =
                forward_from_ring_to_udp(ring_recv, &s, &dst_addr, conn_id, &send_stealth).await;
            if let Err(e) = close_event_sender.send((dst_addr, err)) {
                tracing::error!(?e, "udp send close event error");
            }
        }));
        Self {
            socket,
            conn_id,
            dst_addr,
            stealth,
            ring_sender,
            forward_task,
        }
    }

    pub fn handle_packet_from_remote(&mut self, zc_packet: ZCPacket) -> Result<(), TunnelError> {
        let header = zc_packet.udp_tunnel_header().unwrap();
        let conn_id = header.conn_id.get();

        if header.msg_type != UdpPacketType::Data as u8 {
            return Err(TunnelError::InvalidPacket("not data packet".to_owned()));
        }

        if self.conn_id != conn_id {
            return Err(TunnelError::ConnIdNotMatch(self.conn_id, conn_id));
        }

        if zc_packet.is_lossy() {
            if let Err(e) = self.ring_sender.try_send(zc_packet) {
                tracing::trace!(?e, "ring sender full, drop lossy packet");
            }
        } else if self.ring_sender.force_send(zc_packet).is_err() {
            tracing::trace!("ring sender full, reject non-lossy packet");
            return Err(TunnelError::BufferFull);
        }

        Ok(())
    }
}

#[derive(Clone)]
struct UdpTunnelListenerData {
    local_url: url::Url,
    socket: Option<Arc<UdpSocket>>,
    sock_map: Arc<DashMap<SocketAddr, UdpConnection>>,
    conn_send: Sender<Box<dyn Tunnel>>,
    close_event_sender: UdpCloseEventSender,
    stealth: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
    gate_replay: std::sync::Arc<crate::tunnel::stealth::GateReplayGuard>,
}

impl UdpTunnelListenerData {
    pub fn new(
        local_url: url::Url,
        conn_send: Sender<Box<dyn Tunnel>>,
        close_event_sender: UdpCloseEventSender,
    ) -> Self {
        Self {
            local_url,
            socket: None,
            sock_map: Arc::new(DashMap::new()),
            conn_send,
            close_event_sender,
            stealth: std::sync::Arc::new(crate::tunnel::stealth::OuterSessionState::disabled()),
            gate_replay: std::sync::Arc::new(crate::tunnel::stealth::GateReplayGuard::default()),
        }
    }

    async fn handle_new_connect(self, remote_addr: SocketAddr, zc_packet: ZCPacket) {
        let conn_id = zc_packet.udp_tunnel_header().unwrap().conn_id.get();
        let udp_payload = zc_packet.udp_payload();

        let sack_buf = if self.stealth.is_enabled() {
            let Some(token) = crate::tunnel::stealth::GateToken::from_bytes(udp_payload) else {
                tracing::trace!(?remote_addr, "stealth: drop syn with bad token len");
                return;
            };
            let Some(window) = self.stealth.verify_gate_token(&token, conn_id) else {
                tracing::trace!(?remote_addr, "stealth: drop unauthenticated syn");
                return;
            };
            if !self.gate_replay.accept(window, &token.nonce) {
                tracing::trace!(?remote_addr, "stealth: drop replayed syn token");
                return;
            }
            let resp = self.stealth.build_gate_token(conn_id).to_bytes();
            new_sack_packet_token(conn_id, &resp).into_bytes()
        } else {
            if udp_payload.len() != 8 {
                tracing::warn!(
                    "udp syn packet payload len not match: {:?}, packet: {:?}",
                    udp_payload.len(),
                    zc_packet,
                );
                return;
            }
            let magic = u64::from_le_bytes(udp_payload[..8].try_into().unwrap());
            new_sack_packet(conn_id, magic).into_bytes()
        };

        tracing::info!(?conn_id, ?remote_addr, "udp connection accept handling",);
        let socket = self.socket.as_ref().unwrap().clone();
        if self
            .sock_map
            .get(&remote_addr)
            .is_some_and(|conn| conn.conn_id == conn_id)
        {
            if let Err(e) = socket.send_to(&sack_buf, remote_addr).await {
                tracing::error!(?e, "udp resend sack packet error");
            }
            tracing::debug!(?conn_id, ?remote_addr, "udp duplicate syn, resent sack");
            return;
        }

        let ring_for_send_udp = Arc::new(RingTunnel::new(128));
        let ring_for_recv_udp = Arc::new(RingTunnel::new(128));
        tracing::debug!(
            ?ring_for_send_udp,
            ?ring_for_recv_udp,
            "udp build tunnel for listener"
        );
        let conn_stealth = self.stealth.fork_for_connection();

        let new_internal_conn = || {
            UdpConnection::new(
                socket.clone(),
                conn_id,
                remote_addr,
                conn_stealth.clone(),
                RingSink::new(ring_for_recv_udp.clone()),
                RingStream::new(ring_for_send_udp.clone()),
                self.close_event_sender.clone(),
            )
        };
        let duplicate_syn = match self.sock_map.entry(remote_addr) {
            dashmap::mapref::entry::Entry::Occupied(entry) if entry.get().conn_id == conn_id => {
                true
            }
            dashmap::mapref::entry::Entry::Occupied(mut entry) => {
                entry.insert(new_internal_conn());
                false
            }
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                entry.insert(new_internal_conn());
                false
            }
        };
        if duplicate_syn {
            if let Err(e) = socket.send_to(&sack_buf, remote_addr).await {
                tracing::error!(?e, "udp resend sack packet error");
            }
            tracing::debug!(?conn_id, ?remote_addr, "udp duplicate syn, resent sack");
            return;
        }

        if let Err(e) = socket.send_to(&sack_buf, remote_addr).await {
            self.sock_map
                .remove_if(&remote_addr, |_, conn| conn.conn_id == conn_id);
            tracing::error!(?e, "udp send sack packet error");
            return;
        }

        let conn = Box::new(TunnelWrapper::new_with_associate_data(
            Box::new(RingStream::new(ring_for_recv_udp)),
            Box::new(RingSink::new(ring_for_send_udp)),
            Some(TunnelInfo {
                tunnel_type: "udp".to_owned(),
                local_addr: Some(self.local_url.clone().into()),
                remote_addr: Some(
                    build_url_from_socket_addr(&remote_addr.to_string(), "udp").into(),
                ),
                resolved_remote_addr: Some(
                    build_url_from_socket_addr(&remote_addr.to_string(), "udp").into(),
                ),
            }),
            Some(Box::new(conn_stealth)),
        ));

        tracing::info!(info = ?conn.info().unwrap().remote_addr, "udp connection accept done");

        if let Err(e) = self.conn_send.send(conn).await {
            tracing::warn!(?e, "udp send conn to accept channel error");
        }
    }

    fn do_forward_one_packet_to_conn(&self, zc_packet: ZCPacket, addr: SocketAddr) {
        let header = zc_packet.udp_tunnel_header().unwrap();
        if header.msg_type == UdpPacketType::Syn as u8 {
            tokio::spawn(Self::handle_new_connect(self.clone(), addr, zc_packet));
        } else if !self.stealth.is_enabled() && is_stun_packet(header.as_bytes()) {
            // ignore stun packet
            tracing::debug!("udp forward packet ignore stun packet");
            let socket = self.socket.as_ref().unwrap().clone();
            tokio::spawn(async move {
                let ret = respond_stun_packet(socket, addr, zc_packet.inner().to_vec()).await;
                if let Err(e) = ret {
                    tracing::error!(?e, "udp respond stun packet error");
                }
            });
        } else if header.msg_type == UdpPacketType::V4HolePunch as u8 {
            if !addr.ip().is_loopback() {
                tracing::warn!(?addr, "v4 hole punch packet should be from loopback");
                return;
            }
            if !addr.ip().is_ipv4() {
                tracing::warn!(?addr, "v4 hole punch packet should be sent from ipv4");
                return;
            }
            let Some(dst_addr) =
                extract_dst_addr_from_v4_hole_punch_packet(zc_packet.udp_payload())
            else {
                tracing::warn!("invalid v4 hole punch packet");
                return;
            };
            let socket = self.socket.as_ref().unwrap().clone();
            let udp_packet = new_hole_punch_packet(1, 32);
            if let Err(e) = socket.try_send_to(&udp_packet.into_bytes(), SocketAddr::V4(dst_addr)) {
                tracing::error!(?e, "udp send hole punch packet error");
            }
            tracing::debug!(?dst_addr, "udp forward packet send hole punch packet");
        } else if header.msg_type == UdpPacketType::V6HolePunch as u8 {
            if !addr.ip().is_loopback() {
                tracing::warn!(?addr, "v6 hole punch packet should be from loopback");
                return;
            }
            if !addr.ip().is_ipv6() {
                tracing::warn!(?addr, "v6 hole punch packet should be sent from ipv6");
                return;
            }
            let Some((dst_addr, preferred_src)) =
                extract_v6_hole_punch_packet(zc_packet.udp_payload())
            else {
                tracing::warn!("invalid v6 hole punch packet");
                return;
            };
            let socket = self.socket.as_ref().unwrap().clone();
            let udp_packet = new_hole_punch_packet(1, 32);
            let udp_packet = udp_packet.into_bytes();
            let sent_with_src = if let Some(src) = preferred_src {
                match udp_src::send_to_with_src_ipv6(
                    &socket,
                    src.ip,
                    src.ifindex,
                    dst_addr,
                    &udp_packet,
                ) {
                    Ok(ret) => {
                        tracing::debug!(
                            ?src,
                            ?dst_addr,
                            ?ret,
                            "udp forward packet send hole punch packet with preferred ipv6 source"
                        );
                        true
                    }
                    Err(e) => {
                        tracing::debug!(
                            ?src,
                            ?dst_addr,
                            ?e,
                            "udp forward packet preferred ipv6 source failed, falling back"
                        );
                        false
                    }
                }
            } else {
                false
            };
            if !sent_with_src
                && let Err(e) = socket.try_send_to(&udp_packet, SocketAddr::V6(dst_addr))
            {
                tracing::error!(?e, "udp send hole punch packet error");
            }
            tracing::debug!(
                ?dst_addr,
                ?preferred_src,
                "udp forward packet send hole punch packet"
            );
        } else if header.msg_type != UdpPacketType::HolePunch as u8 {
            let Some(mut conn) = self.sock_map.get_mut(&addr) else {
                tracing::trace!(?header, "udp forward packet error, connection not found");
                return;
            };
            if let Err(e) = conn.handle_packet_from_remote(zc_packet) {
                tracing::trace!(?e, "udp forward packet error");
            }
        } else {
            tracing::trace!(?header, "udp forward packet ignore hole punch packet");
        }
    }

    /// Open a stealth-sealed data datagram from an already-established
    /// connection and route it. Returns `true` only when the datagram was
    /// opened and handled here. A gate-key fallback is accepted only for SYN,
    /// allowing same-address reconnect without reopening the phase-2 data path.
    fn try_forward_sealed_data(&self, raw: &BytesMut, addr: SocketAddr) -> bool {
        if !self.stealth.is_enabled() {
            return false;
        }
        // Look up the per-connection state without holding the map guard across
        // the re-borrow below (avoids a DashMap shard self-deadlock).
        let opened = self
            .sock_map
            .get(&addr)
            .and_then(|conn| conn.stealth.open_datagram(raw));
        if let Some(plaintext) = opened {
            match get_zcpacket_from_buf(BytesMut::from(&plaintext[..]), false) {
                Ok(zc)
                    if zc
                        .udp_tunnel_header()
                        .is_some_and(|header| header.msg_type == UdpPacketType::Syn as u8) =>
                {
                    self.do_forward_one_packet_to_conn(zc, addr);
                }
                Ok(zc) => {
                    if let Some(mut conn) = self.sock_map.get_mut(&addr)
                        && let Err(e) = conn.handle_packet_from_remote(zc)
                    {
                        tracing::trace!(?e, "udp forward sealed packet error");
                    }
                }
                Err(e) => tracing::trace!(?e, "udp sealed data parse error"),
            }
            return true;
        }

        let Some(plaintext) = self.stealth.open_gate_datagram(raw) else {
            return false;
        };
        match get_zcpacket_from_buf(BytesMut::from(&plaintext[..]), false) {
            Ok(zc)
                if zc
                    .udp_tunnel_header()
                    .is_some_and(|header| header.msg_type == UdpPacketType::Syn as u8) =>
            {
                self.do_forward_one_packet_to_conn(zc, addr);
            }
            Ok(zc) => {
                tracing::trace!(?addr, ?zc, "udp drop gate-key non-SYN after phase2");
            }
            Err(e) => tracing::trace!(?e, "udp gate fallback parse error"),
        }
        true
    }

    /// Once a stealth UDP connection exists for `addr`, the only cleartext
    /// transport packet we still allow from that same 5-tuple is a duplicate
    /// transport `Syn`, so the listener can resend `Sack` while the connector's
    /// SYN retry loop is still winding down. Cleartext `Data` would be a
    /// downgrade around outer AEAD and must be dropped.
    fn allow_cleartext_fallback_for_established_addr(zc_packet: &ZCPacket) -> bool {
        zc_packet
            .udp_tunnel_header()
            .map(|header| header.msg_type == UdpPacketType::Syn as u8)
            .unwrap_or(false)
    }

    async fn do_forward_task(self) {
        let socket = self.socket.as_ref().unwrap().clone();
        let mut buf = BytesMut::new();
        loop {
            reserve_buf(&mut buf, UDP_DATA_MTU, UDP_DATA_MTU * 4);
            let addr = match socket.recv_buf_from(&mut buf).await {
                Ok((_dg_size, addr)) => addr,
                Err(e) => {
                    tracing::error!(?e, "udp recv packet error");
                    break;
                }
            };
            let raw = buf.split();
            let addr_has_conn = self.stealth.is_enabled() && self.sock_map.contains_key(&addr);
            if addr_has_conn {
                if self.try_forward_sealed_data(&raw, addr) {
                    continue;
                }
                match get_zcpacket_from_buf(raw, false) {
                    Ok(zc_packet) => {
                        if !Self::allow_cleartext_fallback_for_established_addr(&zc_packet) {
                            tracing::trace!(
                                ?addr,
                                "udp drop cleartext non-syn datagram on established stealth conn"
                            );
                            continue;
                        }
                        self.do_forward_one_packet_to_conn(zc_packet, addr);
                    }
                    Err(e) => tracing::trace!(?e, ?addr, "udp cleartext fallback parse error"),
                }
            } else {
                match get_zcpacket_from_buf(raw, !self.stealth.is_enabled()) {
                    Ok(zc_packet) => self.do_forward_one_packet_to_conn(zc_packet, addr),
                    Err(e) => {
                        if self.stealth.is_enabled() {
                            tracing::trace!(?e, ?addr, "udp drop unauthenticated datagram");
                        } else {
                            tracing::warn!(?e, "udp get zc packet from buf error");
                        }
                    }
                }
            }
        }
    }
}

pub struct UdpTunnelListener {
    addr: url::Url,
    socket: Option<Arc<UdpSocket>>,

    conn_recv: Receiver<Box<dyn Tunnel>>,
    data: UdpTunnelListenerData,
    forward_tasks: Arc<std::sync::Mutex<JoinSet<()>>>,
    close_event_recv: Option<UdpCloseEventReceiver>,
    socket_mark: Option<u32>,
}

impl UdpTunnelListener {
    pub fn new(addr: url::Url) -> Self {
        let (close_event_send, close_event_recv) =
            hotpath::channel!(tokio::sync::mpsc::unbounded_channel());
        let (conn_send, conn_recv) = hotpath::channel!(tokio::sync::mpsc::channel(100));
        Self {
            addr: addr.clone(),
            socket: None,
            conn_recv,
            data: UdpTunnelListenerData::new(addr, conn_send, close_event_send),
            forward_tasks: Arc::new(std::sync::Mutex::new(JoinSet::new())),
            close_event_recv: Some(close_event_recv),
            socket_mark: None,
        }
    }

    pub fn set_socket_mark(&mut self, socket_mark: Option<u32>) {
        self.socket_mark = socket_mark;
    }

    pub fn set_stealth(&mut self, s: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>) {
        self.data.stealth = s;
    }

    pub fn new_with_socket(addr: url::Url, socket: Arc<UdpSocket>) -> Self {
        let mut listener = Self::new(addr);
        listener.socket = Some(socket);
        listener
    }

    pub fn get_socket(&self) -> Option<Arc<UdpSocket>> {
        self.socket.clone()
    }
}

#[async_trait]
impl TunnelListener for UdpTunnelListener {
    async fn listen(&mut self) -> Result<(), TunnelError> {
        if self.socket.is_none() {
            let addr = SocketAddr::from_url(self.addr.clone(), IpVersion::Both).await?;
            let tunnel_url: TunnelUrl = self.addr.clone().into();
            self.socket = Some(Arc::new(
                bind()
                    .addr(addr)
                    .only_v6(true)
                    .maybe_dev(tunnel_url.bind_dev())
                    .maybe_socket_mark(self.socket_mark)
                    .call()?,
            ));
        }
        self.data.socket = self.socket.clone();

        self.addr
            .set_port(Some(self.socket.as_ref().unwrap().local_addr()?.port()))
            .unwrap();

        self.forward_tasks
            .lock()
            .unwrap()
            .spawn(self.data.clone().do_forward_task());

        let sock_map = Arc::downgrade(&self.data.sock_map.clone());
        let mut close_recv = self.close_event_recv.take().unwrap();
        self.forward_tasks.lock().unwrap().spawn(async move {
            while let Some((dst_addr, err)) = close_recv.recv().await {
                if let Some(err) = err {
                    tracing::error!(?err, "udp close event error");
                }
                if let Some(sock_map) = sock_map.upgrade() {
                    sock_map.remove(&dst_addr);
                    shrink_dashmap(&sock_map, None);
                }
            }
        });

        join_joinset_background(self.forward_tasks.clone(), "UdpTunnelListener".to_owned());

        Ok(())
    }

    async fn accept(&mut self) -> Result<Box<dyn super::Tunnel>, super::TunnelError> {
        tracing::info!("start udp accept: {:?}", self.addr);
        if let Some(conn) = self.conn_recv.recv().await {
            return Ok(conn);
        }
        return Err(super::TunnelError::InternalError(
            "udp accept error".to_owned(),
        ));
    }

    fn local_url(&self) -> url::Url {
        self.addr.clone()
    }

    fn get_conn_counter(&self) -> Arc<Box<dyn TunnelConnCounter>> {
        struct UdpTunnelConnCounter {
            sock_map: Weak<DashMap<SocketAddr, UdpConnection>>,
        }

        impl TunnelConnCounter for UdpTunnelConnCounter {
            fn get(&self) -> Option<u32> {
                self.sock_map.upgrade().map(|x| x.len() as u32)
            }
        }

        impl Debug for UdpTunnelConnCounter {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct("UdpTunnelConnCounter")
                    .field("sock_map_len", &self.get())
                    .finish()
            }
        }

        Arc::new(Box::new(UdpTunnelConnCounter {
            sock_map: Arc::downgrade(&self.data.sock_map.clone()),
        }))
    }
}

#[derive(Debug)]
pub struct UdpTunnelConnector {
    addr: url::Url,
    bind_addrs: Vec<SocketAddr>,
    ip_version: IpVersion,
    stealth: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
    stealth_mode: UdpStealthMode,
    resolved_addr: Option<SocketAddr>,
    socket_mark: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UdpStealthMode {
    Disabled,
    Required,
    PreferLegacyFallback,
}

struct UdpConnectAttempt {
    conn_id: u32,
    magic: u64,
    timeout: Duration,
    stealth: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
}

impl UdpConnectAttempt {
    fn new(
        stealth: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
        timeout: Duration,
    ) -> Self {
        Self {
            conn_id: rand::random(),
            magic: rand::random(),
            timeout,
            stealth,
        }
    }

    async fn run(
        &self,
        socket: &Arc<UdpSocket>,
        addr: SocketAddr,
    ) -> Result<SocketAddr, super::TunnelError> {
        let udp_packet = if self.stealth.is_enabled() {
            let token = self.stealth.build_gate_token(self.conn_id).to_bytes();
            new_syn_packet_token(self.conn_id, &token).into_bytes()
        } else {
            new_syn_packet(self.conn_id, self.magic).into_bytes()
        };
        let ret = socket.send_to(&udp_packet, &addr).await?;
        tracing::warn!(conn_id = self.conn_id, ?ret, ?addr, "udp send syn");

        let resend_task = AbortOnDropHandle::new(tokio::spawn({
            let socket = socket.clone();
            let udp_packet = udp_packet.clone();
            let stealth = self.stealth.clone();
            let conn_id = self.conn_id;
            async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    let packet = if stealth.is_enabled() {
                        let token = stealth.build_gate_token(conn_id).to_bytes();
                        new_syn_packet_token(conn_id, &token).into_bytes()
                    } else {
                        udp_packet.clone()
                    };
                    if let Err(error) = socket.send_to(&packet, addr).await {
                        tracing::debug!(?error, ?addr, "udp resend syn failed");
                        break;
                    }
                }
            }
        }));

        let recv_addr = tokio::time::timeout(
            self.timeout,
            UdpTunnelConnector::wait_sack_loop(
                socket,
                addr,
                self.conn_id,
                self.magic,
                &self.stealth,
            ),
        )
        .await??;
        drop(resend_task);
        Ok(recv_addr)
    }
}

impl UdpTunnelConnector {
    pub fn new(addr: url::Url) -> Self {
        Self {
            addr,
            bind_addrs: vec![],
            ip_version: IpVersion::Both,
            resolved_addr: None,
            socket_mark: None,
            stealth: std::sync::Arc::new(crate::tunnel::stealth::OuterSessionState::disabled()),
            stealth_mode: UdpStealthMode::Disabled,
        }
    }

    pub fn set_stealth(&mut self, s: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>) {
        self.stealth_mode = if s.is_enabled() {
            UdpStealthMode::Required
        } else {
            UdpStealthMode::Disabled
        };
        self.stealth = s;
    }

    pub fn prefer_stealth_with_legacy_fallback(
        &mut self,
        s: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
    ) {
        self.stealth_mode = if s.is_enabled() {
            UdpStealthMode::PreferLegacyFallback
        } else {
            UdpStealthMode::Disabled
        };
        self.stealth = s;
    }

    pub fn require_stealth(&mut self) {
        if self.stealth.is_enabled() {
            self.stealth_mode = UdpStealthMode::Required;
        }
    }

    pub fn disable_stealth(&mut self) {
        self.stealth = std::sync::Arc::new(crate::tunnel::stealth::OuterSessionState::disabled());
        self.stealth_mode = UdpStealthMode::Disabled;
    }

    fn should_resend_syn_to_hole_punch_source(
        recv_addr: SocketAddr,
        expected_addr: SocketAddr,
    ) -> bool {
        recv_addr == expected_addr
    }

    async fn wait_sack(
        socket: &UdpSocket,
        addr: SocketAddr,
        conn_id: u32,
        magic: u64,
        stealth: &std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
    ) -> Result<SocketAddr, TunnelError> {
        let mut buf = BytesMut::new();
        buf.reserve(UDP_DATA_MTU);

        let (usize, recv_addr) = tokio::time::timeout(
            tokio::time::Duration::from_secs(3),
            socket.recv_buf_from(&mut buf),
        )
        .await??;
        let zc_packet = get_zcpacket_from_buf(buf.split(), false)?;
        let header = zc_packet.udp_tunnel_header().unwrap();
        if header.msg_type == UdpPacketType::HolePunch as u8 {
            tracing::debug!(?recv_addr, ?addr, "udp wait sack got hole punch packet");
            if Self::should_resend_syn_to_hole_punch_source(recv_addr, addr) {
                let udp_packet = if stealth.is_enabled() {
                    let token = stealth.build_gate_token(conn_id).to_bytes();
                    new_syn_packet_token(conn_id, &token).into_bytes()
                } else {
                    new_syn_packet(conn_id, magic).into_bytes()
                };
                match socket.send_to(&udp_packet, recv_addr).await {
                    Ok(ret) => {
                        tracing::debug!(?recv_addr, ?ret, "udp send syn to hole punch source")
                    }
                    Err(e) => {
                        tracing::debug!(?recv_addr, ?e, "udp send syn to hole punch source failed")
                    }
                }
            } else {
                tracing::debug!(
                    ?recv_addr,
                    ?addr,
                    "ignore hole punch packet from unexpected source"
                );
            }
            return Err(TunnelError::InvalidPacket(
                "got hole punch packet while waiting for sack".to_owned(),
            ));
        }
        if recv_addr != addr {
            tracing::warn!(?recv_addr, ?addr, ?usize, "udp wait sack addr not match");
        }

        if header.conn_id.get() != conn_id {
            return Err(super::TunnelError::ConnIdNotMatch(
                header.conn_id.get(),
                conn_id,
            ));
        }

        if header.msg_type != UdpPacketType::Sack as u8 {
            return Err(TunnelError::InvalidPacket("not sack packet".to_owned()));
        }

        let payload = zc_packet.udp_payload();
        if stealth.is_enabled() {
            let Some(token) = crate::tunnel::stealth::GateToken::from_bytes(payload) else {
                return Err(TunnelError::InvalidPacket(
                    "udp sack stealth token len not match".to_owned(),
                ));
            };
            if stealth.verify_gate_token(&token, conn_id).is_none() {
                return Err(TunnelError::InvalidPacket(
                    "udp sack stealth token not verified".to_owned(),
                ));
            }
            return Ok(recv_addr);
        }
        if payload.len() != 8 {
            return Err(TunnelError::InvalidPacket(
                "udp sack packet payload len not match".to_owned(),
            ));
        }

        let sack_magic = u64::from_le_bytes(payload[..8].try_into().unwrap());
        if sack_magic != magic {
            return Err(TunnelError::InvalidPacket(
                "udp sack magic not match".to_owned(),
            ));
        }

        Ok(recv_addr)
    }

    async fn wait_sack_loop(
        socket: &UdpSocket,
        addr: SocketAddr,
        conn_id: u32,
        magic: u64,
        stealth: &std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
    ) -> Result<SocketAddr, super::TunnelError> {
        loop {
            let ret = Self::wait_sack(socket, addr, conn_id, magic, stealth).await;
            if ret.is_err() {
                tracing::debug!(?ret, "udp wait sack error");
                continue;
            } else {
                return ret;
            }
        }
    }

    async fn build_tunnel(
        &self,
        socket: Arc<UdpSocket>,
        dst_addr: SocketAddr,
        conn_id: u32,
        stealth: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
    ) -> Result<Box<dyn super::Tunnel>, super::TunnelError> {
        let ring_for_send_udp = Arc::new(RingTunnel::new(128));
        let ring_for_recv_udp = Arc::new(RingTunnel::new(128));
        tracing::debug!(
            ?ring_for_send_udp,
            ?ring_for_recv_udp,
            "udp build tunnel for connector"
        );

        let (close_event_sender, mut close_event_recv) =
            hotpath::channel!(tokio::sync::mpsc::unbounded_channel());

        let conn_stealth = stealth.fork_for_connection();
        let ring_recv = RingStream::new(ring_for_send_udp.clone());
        let ring_sender = RingSink::new(ring_for_recv_udp.clone());
        let mut udp_conn = UdpConnection::new(
            socket.clone(),
            conn_id,
            dst_addr,
            conn_stealth.clone(),
            ring_sender,
            ring_recv,
            close_event_sender,
        );

        let socket_clone = socket.clone();
        let recv_stealth = conn_stealth.clone();

        let recv_loop = async move {
            let mut buf = BytesMut::new();
            loop {
                reserve_buf(&mut buf, UDP_DATA_MTU, UDP_DATA_MTU * 4);
                let addr = match socket_clone.recv_buf_from(&mut buf).await {
                    Ok((_dg_size, addr)) => addr,
                    Err(e) => {
                        tracing::trace!(?e, "udp forward task error");
                        break;
                    }
                };
                let raw = buf.split();
                // Stealth: data datagrams are sealed under the per-connection
                // outer key; open before parsing the tunnel header.
                let parsed = if recv_stealth.is_enabled() {
                    match recv_stealth.open_datagram(&raw) {
                        Some(pt) => get_zcpacket_from_buf(BytesMut::from(&pt[..]), false),
                        None => {
                            tracing::trace!(?addr, "udp connector drop unopenable datagram");
                            continue;
                        }
                    }
                } else {
                    get_zcpacket_from_buf(raw, false)
                };
                match parsed {
                    Ok(zc_packet) => {
                        tracing::trace!(?addr, "connector udp forward task done");
                        if let Err(e) = udp_conn.handle_packet_from_remote(zc_packet) {
                            tracing::trace!(?e, ?addr, "udp forward packet error");
                        }
                    }
                    Err(e) => tracing::trace!(?e, ?addr, "udp connector parse packet error"),
                }
            }
        };
        tokio::spawn(
            async move {
                tokio::select! {
                    _ = close_event_recv.recv() => {
                        tracing::debug!("connector udp close event");
                    }
                    _ = recv_loop => {
                        tracing::debug!("connector udp forward task done");
                    }
                }
            }
            .instrument(tracing::info_span!(
                "udp forward from udp to ring",
                ?conn_id,
                ?dst_addr,
            )),
        );

        Ok(Box::new(TunnelWrapper::new_with_associate_data(
            Box::new(RingStream::new(ring_for_recv_udp)),
            Box::new(RingSink::new(ring_for_send_udp)),
            Some(TunnelInfo {
                tunnel_type: "udp".to_owned(),
                local_addr: Some(
                    build_url_from_socket_addr(&socket.local_addr()?.to_string(), "udp").into(),
                ),
                remote_addr: Some(self.addr.clone().into()),
                resolved_remote_addr: Some(
                    build_url_from_socket_addr(&dst_addr.to_string(), "udp").into(),
                ),
            }),
            Some(Box::new(conn_stealth)),
        )))
    }

    pub async fn try_connect_with_socket(
        &self,
        socket: Arc<UdpSocket>,
        addr: SocketAddr,
    ) -> Result<Box<dyn super::Tunnel>, super::TunnelError> {
        tracing::warn!("udp connect: {:?}", self.addr);

        #[cfg(target_os = "windows")]
        crate::arch::windows::disable_connection_reset(socket.as_ref())?;

        let (attempt, recv_addr) = match self.stealth_mode {
            UdpStealthMode::PreferLegacyFallback => {
                let stealth_attempt = UdpConnectAttempt::new(
                    self.stealth.fork_for_connection(),
                    Duration::from_secs(1),
                );
                match stealth_attempt.run(&socket, addr).await {
                    Ok(recv_addr) => (stealth_attempt, recv_addr),
                    Err(error) => {
                        tracing::info!(
                            ?addr,
                            ?error,
                            "UDP stealth bootstrap failed, retrying legacy wire format"
                        );
                        let plain_attempt = UdpConnectAttempt::new(
                            std::sync::Arc::new(
                                crate::tunnel::stealth::OuterSessionState::disabled(),
                            ),
                            Duration::from_secs(3),
                        );
                        let recv_addr = plain_attempt.run(&socket, addr).await?;
                        (plain_attempt, recv_addr)
                    }
                }
            }
            UdpStealthMode::Required | UdpStealthMode::Disabled => {
                let attempt = UdpConnectAttempt::new(
                    self.stealth.fork_for_connection(),
                    Duration::from_secs(3),
                );
                let recv_addr = attempt.run(&socket, addr).await?;
                (attempt, recv_addr)
            }
        };

        if recv_addr != addr {
            tracing::debug!(?recv_addr, ?addr, "udp connect addr not match");
        }

        self.build_tunnel(socket, recv_addr, attempt.conn_id, attempt.stealth)
            .await
    }

    async fn connect_with_default_bind(
        &self,
        addr: SocketAddr,
    ) -> Result<Box<dyn Tunnel>, super::TunnelError> {
        // Route through bind() so socket_mark is applied consistently for
        // both the None (no-op) and Some(_) paths.
        let bind_addr: SocketAddr = if addr.is_ipv4() {
            "0.0.0.0:0".parse().unwrap()
        } else {
            "[::]:0".parse().unwrap()
        };
        let socket = bind::<UdpSocket>()
            .addr(bind_addr)
            .only_v6(true)
            .maybe_socket_mark(self.socket_mark)
            .call()?;

        return self.try_connect_with_socket(Arc::new(socket), addr).await;
    }

    async fn connect_with_custom_bind(
        &self,
        addr: SocketAddr,
    ) -> Result<Box<dyn Tunnel>, super::TunnelError> {
        let futures = FuturesUnordered::new();

        for bind_addr in self.bind_addrs.iter() {
            tracing::info!(?bind_addr, ?addr, "bind addr");
            match bind()
                .addr(*bind_addr)
                .only_v6(true)
                .maybe_socket_mark(self.socket_mark)
                .call()
            {
                Ok(socket) => futures.push(self.try_connect_with_socket(Arc::new(socket), addr)),
                Err(error) => {
                    tracing::error!(?error, ?bind_addr, ?addr, "bind addr fail");
                    continue;
                }
            }
        }
        wait_for_connect_futures(futures).await
    }
}

#[async_trait]
impl super::TunnelConnector for UdpTunnelConnector {
    async fn connect(&mut self) -> Result<Box<dyn Tunnel>, TunnelError> {
        let addr = match self.resolved_addr {
            Some(addr) => addr,
            None => SocketAddr::from_url(self.addr.clone(), self.ip_version).await?,
        };
        if self.bind_addrs.is_empty() || addr.is_ipv6() {
            self.connect_with_default_bind(addr).await
        } else {
            self.connect_with_custom_bind(addr).await
        }
    }

    fn remote_url(&self) -> url::Url {
        self.addr.clone()
    }

    fn set_bind_addrs(&mut self, addrs: Vec<SocketAddr>) {
        self.bind_addrs = addrs;
    }

    fn set_ip_version(&mut self, ip_version: IpVersion) {
        self.ip_version = ip_version;
    }

    fn set_resolved_addr(&mut self, addr: SocketAddr) {
        self.resolved_addr = Some(addr);
    }

    fn set_socket_mark(&mut self, socket_mark: Option<u32>) {
        self.socket_mark = socket_mark;
    }

    fn disable_stealth(&mut self) {
        Self::disable_stealth(self);
    }

    fn require_stealth(&mut self) {
        Self::require_stealth(self);
    }
}

#[cfg(test)]
mod tests {
    use std::{net::IpAddr, time::Duration};

    use bytecodec::EncodeExt as _;
    use futures::SinkExt;
    use stun_codec::{Message, MessageClass, MessageEncoder, rfc5389::methods::BINDING};
    use tokio::time::timeout;

    use super::*;
    use crate::{
        common::global_ctx::tests::get_mock_global_ctx,
        common::stun_codec_ext::{Attribute, ChangeRequest, u32_to_tid},
        tunnel::{
            TunnelConnector,
            common::{
                get_interface_name_by_ip,
                tests::{_tunnel_bench, _tunnel_echo_server, _tunnel_pingpong, wait_for_condition},
            },
            packet_def::PacketType,
        },
    };

    fn new_udp_data_packet(conn_id: u32, packet_type: PacketType) -> ZCPacket {
        let mut packet = ZCPacket::new_with_payload(b"udp-data").convert_type(ZCPacketType::UDP);
        packet.fill_peer_manager_hdr(1, 2, packet_type as u8);
        let udp_payload_len = packet.udp_payload().len();
        let header = packet.mut_udp_tunnel_header().unwrap();
        header.conn_id.set(conn_id);
        header.msg_type = UdpPacketType::Data as u8;
        header.len.set(udp_payload_len as u16);
        packet
    }

    fn assert_sync_packet_handler(_: fn(&mut UdpConnection, ZCPacket) -> Result<(), TunnelError>) {}

    #[test]
    fn hole_punch_source_must_match_connect_addr_before_syn_resend() {
        let expected_addr: SocketAddr = "198.51.100.10:11010".parse().unwrap();
        let same_port_different_ip: SocketAddr = "198.51.100.11:11010".parse().unwrap();
        let same_ip_different_port: SocketAddr = "198.51.100.10:11011".parse().unwrap();

        assert!(UdpTunnelConnector::should_resend_syn_to_hole_punch_source(
            expected_addr,
            expected_addr
        ));
        assert!(!UdpTunnelConnector::should_resend_syn_to_hole_punch_source(
            same_port_different_ip,
            expected_addr
        ));
        assert!(!UdpTunnelConnector::should_resend_syn_to_hole_punch_source(
            same_ip_different_port,
            expected_addr
        ));
    }

    #[tokio::test]
    async fn udp_pingpong() {
        let listener = UdpTunnelListener::new("udp://0.0.0.0:5556".parse().unwrap());
        let connector = UdpTunnelConnector::new("udp://127.0.0.1:5556".parse().unwrap());
        _tunnel_pingpong(listener, connector).await;
    }

    #[tokio::test]
    async fn udp_preferred_stealth_falls_back_with_fresh_plain_attempt() {
        let mut listener = UdpTunnelListener::new("udp://127.0.0.1:0".parse().unwrap());
        listener.listen().await.unwrap();

        let mut connector = UdpTunnelConnector::new(listener.local_url());
        connector.prefer_stealth_with_legacy_fallback(stealth_state("net-secret"));

        let (client, server) = timeout(Duration::from_secs(6), async {
            tokio::join!(connector.connect(), listener.accept())
        })
        .await
        .expect("legacy fallback timed out");
        let client = client.unwrap();
        let server = server.unwrap();

        assert!(!tunnel_stealth_state(client.as_ref()).is_enabled());
        assert!(!tunnel_stealth_state(server.as_ref()).is_enabled());
    }

    #[tokio::test]
    async fn udp_plain_fallback_ignores_late_stealth_sack() {
        let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let server_addr = server.local_addr().unwrap();
        let server_state = stealth_state("net-secret");
        let server_task = tokio::spawn(async move {
            let mut buf = [0u8; 512];
            let (len, client_addr) = server.recv_from(&mut buf).await.unwrap();
            let first = get_zcpacket_from_buf(BytesMut::from(&buf[..len]), false).unwrap();
            let first_conn_id = first.udp_tunnel_header().unwrap().conn_id.get();
            assert_eq!(
                first.udp_tunnel_header().unwrap().msg_type,
                UdpPacketType::Syn as u8
            );
            assert_eq!(
                first.udp_payload().len(),
                crate::tunnel::stealth::GATE_TOKEN_LEN
            );
            let started = tokio::time::Instant::now();

            let (plain_conn_id, plain_magic) = loop {
                let (len, addr) = server.recv_from(&mut buf).await.unwrap();
                assert_eq!(addr, client_addr);
                let packet = get_zcpacket_from_buf(BytesMut::from(&buf[..len]), false).unwrap();
                let conn_id = packet.udp_tunnel_header().unwrap().conn_id.get();
                if packet.udp_payload().len() == 8 && conn_id != first_conn_id {
                    break (
                        conn_id,
                        u64::from_le_bytes(packet.udp_payload().try_into().unwrap()),
                    );
                }
            };
            assert_ne!(first_conn_id, plain_conn_id);

            tokio::time::sleep_until(started + Duration::from_millis(1200)).await;
            let late_token = server_state.build_gate_token(first_conn_id).to_bytes();
            server
                .send_to(
                    &new_sack_packet_token(first_conn_id, &late_token).into_bytes(),
                    client_addr,
                )
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
            server
                .send_to(
                    &new_sack_packet(plain_conn_id, plain_magic).into_bytes(),
                    client_addr,
                )
                .await
                .unwrap();
            (first_conn_id, plain_conn_id)
        });

        let mut connector =
            UdpTunnelConnector::new(format!("udp://{server_addr}").parse().unwrap());
        connector.prefer_stealth_with_legacy_fallback(stealth_state("net-secret"));
        let tunnel = timeout(Duration::from_secs(5), connector.connect())
            .await
            .expect("fallback timed out after late SACK")
            .expect("plain fallback did not accept its own SACK");
        let (first_conn_id, plain_conn_id) = server_task.await.unwrap();

        assert_ne!(first_conn_id, plain_conn_id);
        assert!(!tunnel_stealth_state(tunnel.as_ref()).is_enabled());
    }

    fn stealth_state(secret: &str) -> std::sync::Arc<crate::tunnel::stealth::OuterSessionState> {
        crate::tunnel::stealth::build_outer_session(Some(secret), true, true, 0)
    }

    fn tunnel_stealth_state(
        tunnel: &dyn Tunnel,
    ) -> std::sync::Arc<crate::tunnel::stealth::OuterSessionState> {
        tunnel
            .data()
            .and_then(|data| {
                data.downcast_ref::<std::sync::Arc<crate::tunnel::stealth::OuterSessionState>>()
            })
            .cloned()
            .expect("udp tunnel should carry per-connection stealth state")
    }

    #[tokio::test]
    async fn udp_stealth_pingpong() {
        let mut listener = UdpTunnelListener::new("udp://0.0.0.0:5571".parse().unwrap());
        listener.set_stealth(stealth_state("net-secret"));
        let mut connector = UdpTunnelConnector::new("udp://127.0.0.1:5571".parse().unwrap());
        connector.set_stealth(stealth_state("net-secret"));
        _tunnel_pingpong(listener, connector).await;
    }

    #[tokio::test]
    async fn udp_stealth_listener_drops_unauthenticated() {
        let mut listener = UdpTunnelListener::new("udp://0.0.0.0:5572".parse().unwrap());
        listener.set_stealth(stealth_state("net-secret"));
        tokio::spawn(async move {
            listener.listen().await.unwrap();
            let _ = listener.accept().await;
        });
        tokio::time::sleep(Duration::from_millis(200)).await;

        // plain connector (no stealth token) must not receive a SACK -> connect fails
        let mut connector = UdpTunnelConnector::new("udp://127.0.0.1:5572".parse().unwrap());
        let ret = timeout(Duration::from_secs(5), connector.connect()).await;
        assert!(ret.is_err() || ret.unwrap().is_err());
    }

    #[tokio::test]
    async fn udp_stealth_listener_drops_wrong_secret() {
        let mut listener = UdpTunnelListener::new("udp://0.0.0.0:5573".parse().unwrap());
        listener.set_stealth(stealth_state("right-secret"));
        tokio::spawn(async move {
            listener.listen().await.unwrap();
            let _ = listener.accept().await;
        });
        tokio::time::sleep(Duration::from_millis(200)).await;

        let mut connector = UdpTunnelConnector::new("udp://127.0.0.1:5573".parse().unwrap());
        connector.set_stealth(stealth_state("wrong-secret"));
        let ret = timeout(Duration::from_secs(5), connector.connect()).await;
        assert!(ret.is_err() || ret.unwrap().is_err());
    }

    #[tokio::test]
    async fn udp_stealth_listener_drops_stun_probe() {
        let mut listener = UdpTunnelListener::new("udp://0.0.0.0:5576".parse().unwrap());
        listener.set_stealth(stealth_state("net-secret"));
        listener.listen().await.unwrap();

        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut message = Message::<Attribute>::new(MessageClass::Request, BINDING, u32_to_tid(7));
        message.add_attribute(ChangeRequest::new(false, false));
        let mut encoder = MessageEncoder::new();
        let req = encoder.encode_into_bytes(message).unwrap();

        socket.send_to(&req, "127.0.0.1:5576").await.unwrap();

        let mut buf = [0u8; 256];
        let ret = timeout(Duration::from_millis(500), socket.recv_from(&mut buf)).await;
        assert!(
            ret.is_err(),
            "stealth listener should not answer STUN probes"
        );
    }

    #[tokio::test]
    async fn udp_stealth_allocates_state_per_connection() {
        let listener_template = stealth_state("net-secret");
        let connector_template = stealth_state("net-secret");

        let mut listener = UdpTunnelListener::new("udp://0.0.0.0:5574".parse().unwrap());
        listener.set_stealth(listener_template.clone());
        listener.listen().await.unwrap();

        let mut connector1 = UdpTunnelConnector::new("udp://127.0.0.1:5574".parse().unwrap());
        connector1.set_stealth(connector_template.clone());
        let conn1 = connector1.connect().await.unwrap();
        let accepted1 = listener.accept().await.unwrap();

        let mut connector2 = UdpTunnelConnector::new("udp://127.0.0.1:5574".parse().unwrap());
        connector2.set_stealth(connector_template.clone());
        let conn2 = connector2.connect().await.unwrap();
        let accepted2 = listener.accept().await.unwrap();

        let conn1_state = tunnel_stealth_state(conn1.as_ref());
        let accepted1_state = tunnel_stealth_state(accepted1.as_ref());
        let conn2_state = tunnel_stealth_state(conn2.as_ref());
        let accepted2_state = tunnel_stealth_state(accepted2.as_ref());

        assert!(!std::sync::Arc::ptr_eq(&conn1_state, &connector_template));
        assert!(!std::sync::Arc::ptr_eq(
            &accepted1_state,
            &listener_template
        ));
        assert!(!std::sync::Arc::ptr_eq(&conn1_state, &conn2_state));
        assert!(!std::sync::Arc::ptr_eq(&accepted1_state, &accepted2_state));

        accepted1_state.set_outer_key_from_handshake_hash(b"accepted-1");
        conn1_state.set_outer_key_from_handshake_hash(b"conn-1");

        assert!(listener_template.outer_key().is_none());
        assert!(connector_template.outer_key().is_none());
        assert!(accepted1_state.outer_key().is_some());
        assert!(conn1_state.outer_key().is_some());
        assert!(accepted2_state.outer_key().is_none());
        assert!(conn2_state.outer_key().is_none());
    }

    #[tokio::test]
    async fn udp_stealth_data_flows_with_phase2_outer_key() {
        use futures::StreamExt as _;

        let mut listener = UdpTunnelListener::new("udp://0.0.0.0:5575".parse().unwrap());
        listener.set_stealth(stealth_state("net-secret"));
        listener.listen().await.unwrap();

        let mut connector = UdpTunnelConnector::new("udp://127.0.0.1:5575".parse().unwrap());
        connector.set_stealth(stealth_state("net-secret"));
        let conn = connector.connect().await.unwrap();
        let accepted = listener.accept().await.unwrap();

        // Simulate the phase-2 handoff: both endpoints install the same
        // connection-level outer key derived from the (shared) handshake hash.
        let conn_state = tunnel_stealth_state(conn.as_ref());
        let accepted_state = tunnel_stealth_state(accepted.as_ref());
        conn_state.set_outer_key_from_handshake_hash(b"shared-handshake-hash");
        accepted_state.set_outer_key_from_handshake_hash(b"shared-handshake-hash");
        assert_eq!(conn_state.outer_key(), accepted_state.outer_key());

        // Echo on the accepted side; data datagrams now travel sealed under the
        // phase-2 outer key in both directions.
        tokio::spawn(_tunnel_echo_server(accepted, false));

        let (mut recv, mut send) = conn.split();
        let payload = b"phase2-sealed-data";
        send.send(ZCPacket::new_with_payload(payload))
            .await
            .unwrap();

        let echoed = timeout(Duration::from_secs(3), recv.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(&echoed.payload()[..], payload);
    }

    #[tokio::test]
    async fn udp_stealth_duplicate_syn_resends_sack_without_second_accept() {
        let mut listener = UdpTunnelListener::new("udp://127.0.0.1:0".parse().unwrap());
        listener.set_stealth(stealth_state("net-secret"));
        listener.listen().await.unwrap();
        let listener_addr = SocketAddr::from_url(listener.local_url(), IpVersion::V4)
            .await
            .unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let client_state = stealth_state("net-secret");
        let conn_id = 71;

        let first_token = client_state.build_gate_token(conn_id).to_bytes();
        socket
            .send_to(
                &new_syn_packet_token(conn_id, &first_token).into_bytes(),
                listener_addr,
            )
            .await
            .unwrap();
        let mut sack = [0u8; 512];
        timeout(Duration::from_secs(1), socket.recv_from(&mut sack))
            .await
            .expect("initial SYN did not receive SACK")
            .unwrap();
        let accepted = timeout(Duration::from_secs(1), listener.accept())
            .await
            .expect("initial SYN was not accepted")
            .unwrap();

        let retry_token = client_state.build_gate_token(conn_id).to_bytes();
        assert_ne!(first_token, retry_token);
        socket
            .send_to(
                &new_syn_packet_token(conn_id, &retry_token).into_bytes(),
                listener_addr,
            )
            .await
            .unwrap();
        timeout(Duration::from_secs(1), socket.recv_from(&mut sack))
            .await
            .expect("duplicate SYN did not trigger SACK retransmission")
            .unwrap();
        assert!(
            timeout(Duration::from_millis(200), listener.accept())
                .await
                .is_err(),
            "duplicate SYN created a second accepted tunnel"
        );
        drop(accepted);
    }

    #[tokio::test]
    async fn udp_phase2_accepts_gate_syn_but_rejects_gate_data() {
        use futures::StreamExt as _;

        let mut listener = UdpTunnelListener::new("udp://127.0.0.1:0".parse().unwrap());
        listener.set_stealth(stealth_state("net-secret"));
        listener.listen().await.unwrap();
        let listener_addr = SocketAddr::from_url(listener.local_url(), IpVersion::V4)
            .await
            .unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        let first_conn_id = 41;
        let first_gate = stealth_state("net-secret");
        let first_token = first_gate.build_gate_token(first_conn_id).to_bytes();
        socket
            .send_to(
                &new_syn_packet_token(first_conn_id, &first_token).into_bytes(),
                listener_addr,
            )
            .await
            .unwrap();
        let mut sack = [0u8; 512];
        timeout(Duration::from_secs(2), socket.recv_from(&mut sack))
            .await
            .unwrap()
            .unwrap();
        let first = timeout(Duration::from_secs(2), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let first_listener_state = tunnel_stealth_state(first.as_ref());
        first_listener_state.set_outer_key_from_handshake_hash(b"phase2");
        first_gate.set_outer_key_from_handshake_hash(b"phase2");
        let (mut first_recv, _first_send) = first.split();

        let outer_data = first_gate
            .seal_datagram(&new_udp_data_packet(first_conn_id, PacketType::Data).into_bytes())
            .unwrap();
        socket.send_to(&outer_data, listener_addr).await.unwrap();
        timeout(Duration::from_secs(2), first_recv.next())
            .await
            .expect("outer-key data was not delivered")
            .expect("listener stream closed")
            .expect("outer-key data was rejected");

        let gate_only = stealth_state("net-secret");
        let gate_data = gate_only
            .seal_datagram(&new_udp_data_packet(first_conn_id, PacketType::Data).into_bytes())
            .unwrap();
        socket.send_to(&gate_data, listener_addr).await.unwrap();
        assert!(
            timeout(Duration::from_millis(300), first_recv.next())
                .await
                .is_err(),
            "gate-key data must not re-enter the phase-2 data path"
        );

        let second_conn_id = 42;
        let second_token = gate_only.build_gate_token(second_conn_id).to_bytes();
        let sealed_syn = gate_only
            .seal_datagram(&new_syn_packet_token(second_conn_id, &second_token).into_bytes())
            .unwrap();
        socket.send_to(&sealed_syn, listener_addr).await.unwrap();
        timeout(Duration::from_secs(2), socket.recv_from(&mut sack))
            .await
            .expect("gate-key reconnect SYN did not receive SACK")
            .unwrap();
        timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("gate-key reconnect SYN did not replace the old connection")
            .unwrap();
    }

    #[test]
    fn udp_stealth_established_addr_allows_only_cleartext_syn_fallback() {
        let syn = new_syn_packet_token(7, &[0u8; crate::tunnel::stealth::GATE_TOKEN_LEN]);
        let data = new_udp_data_packet(7, PacketType::Data);

        assert!(UdpTunnelListenerData::allow_cleartext_fallback_for_established_addr(&syn));
        assert!(!UdpTunnelListenerData::allow_cleartext_fallback_for_established_addr(&data));
    }

    #[tokio::test]
    async fn udp_connection_handler_uses_sync_nonblocking_ring_delivery() {
        assert_sync_packet_handler(UdpConnection::handle_packet_from_remote);

        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let dst_addr = "127.0.0.1:1".parse().unwrap();
        let ring_for_send_udp = Arc::new(RingTunnel::new(8));
        let ring_for_recv_udp = Arc::new(RingTunnel::new(8));
        let (close_event_sender, _close_event_recv) = tokio::sync::mpsc::unbounded_channel();
        let mut conn = UdpConnection::new(
            socket,
            7,
            dst_addr,
            std::sync::Arc::new(crate::tunnel::stealth::OuterSessionState::disabled()),
            RingSink::new(ring_for_recv_udp),
            RingStream::new(ring_for_send_udp),
            close_event_sender,
        );

        for _ in 0..16 {
            conn.handle_packet_from_remote(new_udp_data_packet(7, PacketType::Data))
                .unwrap();
        }

        let mut got_buffer_full = false;
        for _ in 0..16 {
            match conn.handle_packet_from_remote(new_udp_data_packet(7, PacketType::Ping)) {
                Ok(()) => {}
                Err(TunnelError::BufferFull) => {
                    got_buffer_full = true;
                    break;
                }
                Err(e) => panic!("unexpected error: {e:?}"),
            }
        }
        assert!(got_buffer_full);
    }

    #[tokio::test]
    async fn udp_bench() {
        let listener = UdpTunnelListener::new("udp://0.0.0.0:5555".parse().unwrap());
        let connector = UdpTunnelConnector::new("udp://127.0.0.1:5555".parse().unwrap());
        _tunnel_bench(listener, connector).await
    }

    #[tokio::test]
    async fn udp_bench_with_bind() {
        let listener = UdpTunnelListener::new("udp://127.0.0.1:5554".parse().unwrap());
        let mut connector = UdpTunnelConnector::new("udp://127.0.0.1:5554".parse().unwrap());
        connector.set_bind_addrs(vec!["127.0.0.1:0".parse().unwrap()]);
        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    #[should_panic]
    async fn udp_bench_with_bind_fail() {
        let listener = UdpTunnelListener::new("udp://127.0.0.1:5553".parse().unwrap());
        let mut connector = UdpTunnelConnector::new("udp://127.0.0.1:5553".parse().unwrap());
        connector.set_bind_addrs(vec!["10.0.0.1:0".parse().unwrap()]);
        _tunnel_pingpong(listener, connector).await
    }

    async fn send_random_data_to_socket(remote_url: url::Url) {
        let socket = UdpSocket::bind("0.0.0.0:0").await.unwrap();
        socket
            .connect(format!(
                "{}:{}",
                remote_url.host().unwrap(),
                remote_url.port().unwrap()
            ))
            .await
            .unwrap();

        // get a random 100-len buf
        loop {
            let mut buf = vec![0u8; 100];
            rand::thread_rng().fill(&mut buf[..]);
            socket.send(&buf).await.unwrap();
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn udp_multiple_conns() {
        let mut listener = UdpTunnelListener::new("udp://0.0.0.0:5557".parse().unwrap());
        listener.listen().await.unwrap();

        let _lis = tokio::spawn(async move {
            loop {
                let ret = listener.accept().await.unwrap();
                assert_eq!(
                    ret.info()
                        .unwrap()
                        .local_addr
                        .unwrap_or_default()
                        .to_string(),
                    listener.local_url().to_string()
                );
                tokio::spawn(async move { _tunnel_echo_server(ret, false).await });
            }
        });

        let mut connector1 = UdpTunnelConnector::new("udp://127.0.0.1:5557".parse().unwrap());
        let mut connector2 = UdpTunnelConnector::new("udp://127.0.0.1:5557".parse().unwrap());

        let t1 = connector1.connect().await.unwrap();
        let t2 = connector2.connect().await.unwrap();

        tokio::spawn(timeout(
            Duration::from_secs(2),
            send_random_data_to_socket(t1.info().unwrap().local_addr.unwrap().into()),
        ));
        tokio::spawn(timeout(
            Duration::from_secs(2),
            send_random_data_to_socket(t1.info().unwrap().remote_addr.unwrap().into()),
        ));
        tokio::spawn(timeout(
            Duration::from_secs(2),
            send_random_data_to_socket(t2.info().unwrap().remote_addr.unwrap().into()),
        ));

        let sender1 = tokio::spawn(async move {
            let (mut stream, mut sink) = t1.split();

            for i in 0..10 {
                sink.send(ZCPacket::new_with_payload("hello1".as_bytes()))
                    .await
                    .unwrap();
                let recv = stream.next().await.unwrap().unwrap();
                println!("t1 recv: {:?}, {:?}", recv, i);
                assert_eq!(recv.payload(), "hello1".as_bytes());
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        });

        let sender2 = tokio::spawn(async move {
            let (mut stream, mut sink) = t2.split();

            for i in 0..10 {
                sink.send(ZCPacket::new_with_payload("hello2".as_bytes()))
                    .await
                    .unwrap();
                let recv = stream.next().await.unwrap().unwrap();
                println!("t2 recv: {:?}, {:?}", recv, i);
                assert_eq!(recv.payload(), "hello2".as_bytes());
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        });

        let _ = tokio::join!(sender1, sender2);
    }

    #[tokio::test]
    async fn bind_multi_ip_to_same_dev() {
        let global_ctx = get_mock_global_ctx();
        let ips = global_ctx
            .get_ip_collector()
            .collect_ip_addrs()
            .await
            .interface_ipv4s;
        if ips.is_empty() {
            return;
        }
        let bind_dev = get_interface_name_by_ip(&IpAddr::V4(ips[0].into()));

        for ip in ips {
            println!("bind to ip: {}, {:?}", ip, bind_dev);
            let addr = SocketAddr::from_url(
                format!("udp://{}:11111", ip).parse().unwrap(),
                IpVersion::Both,
            )
            .await
            .unwrap();
            let _ = bind::<UdpSocket>()
                .addr(addr)
                .maybe_dev(bind_dev.clone())
                .only_v6(true)
                .call()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn bind_same_port() {
        println!("{}", "[::]:8888".parse::<SocketAddr>().unwrap());
        let mut listener = UdpTunnelListener::new("udp://[::]:31014".parse().unwrap());
        let mut listener2 = UdpTunnelListener::new("udp://0.0.0.0:31014".parse().unwrap());
        listener.listen().await.unwrap();
        listener2.listen().await.unwrap();
    }

    #[tokio::test]
    async fn ipv6_pingpong() {
        let listener = UdpTunnelListener::new("udp://[::1]:31015".parse().unwrap());
        let connector = UdpTunnelConnector::new("udp://[::1]:31015".parse().unwrap());
        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    async fn ipv6_domain_pingpong() {
        let listener = UdpTunnelListener::new("udp://[::1]:31016".parse().unwrap());
        let mut connector =
            UdpTunnelConnector::new("udp://test.easytier.top:31016".parse().unwrap());
        connector.set_ip_version(IpVersion::V6);
        _tunnel_pingpong(listener, connector).await;

        let listener = UdpTunnelListener::new("udp://127.0.0.1:31016".parse().unwrap());
        let mut connector =
            UdpTunnelConnector::new("udp://test.easytier.top:31016".parse().unwrap());
        connector.set_ip_version(IpVersion::V4);
        _tunnel_pingpong(listener, connector).await;
    }

    #[tokio::test]
    async fn test_alloc_port() {
        // v4
        let mut listener = UdpTunnelListener::new("udp://0.0.0.0:0".parse().unwrap());
        listener.listen().await.unwrap();
        let port = listener.local_url().port().unwrap();
        assert!(port > 0);

        // v6
        let mut listener = UdpTunnelListener::new("udp://[::]:0".parse().unwrap());
        listener.listen().await.unwrap();
        let port = listener.local_url().port().unwrap();
        assert!(port > 0);
    }

    #[tokio::test]
    async fn test_conn_counter() {
        let mut listener = UdpTunnelListener::new("udp://0.0.0.0:5556".parse().unwrap());
        let mut connector = UdpTunnelConnector::new("udp://127.0.0.1:5556".parse().unwrap());
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            let _c1 = connector.connect().await.unwrap();
            let _c2 = connector.connect().await.unwrap();
        });

        let conn_counter = listener.get_conn_counter();

        listener.listen().await.unwrap();
        let c1 = listener.accept().await.unwrap();
        assert_eq!(conn_counter.get(), Some(1));
        let c2 = listener.accept().await.unwrap();
        assert_eq!(conn_counter.get(), Some(2));

        drop(c2);
        wait_for_condition(
            || async { conn_counter.get() == Some(1) },
            Duration::from_secs(1),
        )
        .await;

        drop(c1);
        wait_for_condition(
            || async { conn_counter.get().unwrap_or(0) == 0 },
            Duration::from_secs(1),
        )
        .await;
    }

    #[test]
    fn v6_hole_punch_packet_preserves_preferred_source_ifindex() {
        let dst_addr = "[2001:db8::1]:10001".parse::<SocketAddrV6>().unwrap();
        let preferred_src = PreferredIpv6Source {
            ip: "2001:db8::2".parse().unwrap(),
            ifindex: 42,
        };

        let packet = new_v6_hole_punch_packet(&dst_addr, Some(preferred_src));
        let (parsed_dst_addr, parsed_preferred_src) =
            extract_v6_hole_punch_packet(packet.udp_payload()).unwrap();

        assert_eq!(parsed_dst_addr, dst_addr);
        assert_eq!(parsed_preferred_src, Some(preferred_src));
    }

    #[tokio::test]
    async fn test_v6_hole_punch_packet() {
        let mut lis = UdpTunnelListener::new("udp://[::]:0".parse().unwrap());
        lis.listen().await.unwrap();

        // a socket to receive forwarded hole punch packets
        let socket = Arc::new(UdpSocket::bind("[::]:0").await.unwrap());
        let socket_clone = socket.clone();
        let t = tokio::spawn(async move {
            let mut buf = BytesMut::new();
            buf.resize(128, 0);
            socket_clone.recv_from(&mut buf).await.unwrap();
        });

        tracing::info!("lis local addr: {:?}", lis.local_url());
        tracing::info!("socket local addr: {:?}", socket.local_addr().unwrap());

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // a socket to send v6 hole punch packets
        send_v6_hole_punch_packet(
            lis.local_url().port().unwrap(),
            match socket.local_addr().unwrap() {
                std::net::SocketAddr::V6(addr_v6) => addr_v6,
                _ => panic!("Expected an IPv6 address"),
            },
            None,
        )
        .await
        .unwrap();

        tokio::time::timeout(tokio::time::Duration::from_secs(2), t)
            .await
            .expect("Timeout waiting for v6 hole punch packet")
            .unwrap();
    }

    #[tokio::test]
    async fn test_v4_hole_punch_packet() {
        let mut lis = UdpTunnelListener::new("udp://0.0.0.0:0".parse().unwrap());
        lis.listen().await.unwrap();

        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let socket_clone = socket.clone();
        let t = tokio::spawn(async move {
            let mut buf = BytesMut::new();
            buf.resize(128, 0);
            socket_clone.recv_from(&mut buf).await.unwrap();
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        send_v4_hole_punch_packet(
            lis.local_url().port().unwrap(),
            match socket.local_addr().unwrap() {
                std::net::SocketAddr::V4(addr_v4) => addr_v4,
                _ => panic!("Expected an IPv4 address"),
            },
        )
        .await
        .unwrap();

        tokio::time::timeout(tokio::time::Duration::from_secs(2), t)
            .await
            .expect("Timeout waiting for v4 hole punch packet")
            .unwrap();
    }
}
