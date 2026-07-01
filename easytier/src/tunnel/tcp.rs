use std::{net::SocketAddr, sync::Arc};

use super::{FromUrl, TunnelInfo};
use crate::tunnel::common::{apply_socket_mark, bind};
use async_trait::async_trait;
use futures::{StreamExt as _, stream::FuturesUnordered};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpSocket, TcpStream},
};

use super::common::{StealthFramedReader, StealthTcpZCPacketToBytes};
use super::{
    IpVersion, Tunnel, TunnelError, TunnelListener,
    common::{FramedReader, FramedWriter, TunnelWrapper, wait_for_connect_futures},
};

const TCP_MTU_BYTES: usize = 2000;
const TCP_STEALTH_PREFACE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
const MAX_PENDING_STEALTH_ACCEPTS: usize = 256;

#[derive(Debug)]
pub struct TcpTunnelListener {
    addr: url::Url,
    listener: Option<TcpListener>,
    socket_mark: Option<u32>,
    stealth: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
    replay_guard: std::sync::Arc<crate::tunnel::stealth::GateReplayGuard>,
}

impl TcpTunnelListener {
    pub fn new(addr: url::Url) -> Self {
        TcpTunnelListener {
            addr,
            listener: None,
            socket_mark: None,
            stealth: std::sync::Arc::new(crate::tunnel::stealth::OuterSessionState::disabled()),
            replay_guard: std::sync::Arc::new(crate::tunnel::stealth::GateReplayGuard::default()),
        }
    }

    pub fn set_socket_mark(&mut self, socket_mark: Option<u32>) {
        self.socket_mark = socket_mark;
    }

    pub fn set_stealth(
        &mut self,
        stealth: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
    ) {
        self.stealth = stealth;
    }

    async fn do_accept(&self) -> Result<Box<dyn Tunnel>, std::io::Error> {
        let listener = self.listener.as_ref().unwrap();
        let (stream, _) = listener.accept().await?;

        if let Err(e) = stream.set_nodelay(true) {
            tracing::warn!(?e, "set_nodelay fail in accept");
        }

        let info = TunnelInfo {
            tunnel_type: "tcp".to_owned(),
            local_addr: Some(self.local_url().into()),
            remote_addr: Some(
                super::build_url_from_socket_addr(&stream.peer_addr()?.to_string(), "tcp").into(),
            ),
            resolved_remote_addr: Some(
                super::build_url_from_socket_addr(&stream.peer_addr()?.to_string(), "tcp").into(),
            ),
        };

        let (r, w) = stream.into_split();
        if self.stealth.is_enabled() {
            let stealth = self.stealth.fork_for_connection();
            return Ok(Box::new(TunnelWrapper::new_with_associate_data(
                StealthFramedReader::new(r, TCP_MTU_BYTES, stealth.clone()),
                FramedWriter::new_with_converter(
                    w,
                    StealthTcpZCPacketToBytes::new(stealth.clone()),
                ),
                Some(info),
                Some(Box::new(stealth)),
            )));
        }
        Ok(Box::new(TunnelWrapper::new(
            FramedReader::new(r, TCP_MTU_BYTES),
            FramedWriter::new(w),
            Some(info),
        )))
    }

    async fn authenticate_stealth_stream(
        mut stream: TcpStream,
        local_url: url::Url,
        stealth: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
        replay_guard: std::sync::Arc<crate::tunnel::stealth::GateReplayGuard>,
    ) -> Result<Box<dyn Tunnel>, std::io::Error> {
        let mut preface = [0u8; crate::tunnel::stealth::STREAM_GATE_PREFACE_LEN];
        tokio::time::timeout(TCP_STEALTH_PREFACE_TIMEOUT, stream.read_exact(&mut preface))
            .await
            .map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "TCP stealth preface timed out",
                )
            })??;
        let Some(verified_preface) = crate::tunnel::stealth::verify_stream_gate_preface(
            stealth.as_ref(),
            replay_guard.as_ref(),
            &preface,
        ) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "TCP stealth preface authentication failed",
            ));
        };
        tracing::trace!("accepted TCP stealth preface");
        let ack =
            crate::tunnel::stealth::build_stream_gate_ack(stealth.as_ref(), &verified_preface);
        stream.write_all(&ack).await?;

        if let Err(error) = stream.set_nodelay(true) {
            tracing::warn!(?error, "set_nodelay fail in stealth accept");
        }
        let info = TunnelInfo {
            tunnel_type: "tcp".to_owned(),
            local_addr: Some(local_url.into()),
            remote_addr: Some(
                super::build_url_from_socket_addr(&stream.peer_addr()?.to_string(), "tcp").into(),
            ),
            resolved_remote_addr: Some(
                super::build_url_from_socket_addr(&stream.peer_addr()?.to_string(), "tcp").into(),
            ),
        };
        let (reader, writer) = stream.into_split();
        let stealth = stealth.fork_for_connection();
        Ok(Box::new(TunnelWrapper::new_with_associate_data(
            StealthFramedReader::new(reader, TCP_MTU_BYTES, stealth.clone()),
            FramedWriter::new_with_converter(
                writer,
                StealthTcpZCPacketToBytes::new(stealth.clone()),
            ),
            Some(info),
            Some(Box::new(stealth)),
        )))
    }

    async fn accept_stealth(&self) -> Result<Box<dyn Tunnel>, super::TunnelError> {
        let listener = self.listener.as_ref().unwrap();
        let mut pending = FuturesUnordered::new();

        loop {
            if pending.len() >= MAX_PENDING_STEALTH_ACCEPTS {
                if let Some(result) = pending.next().await {
                    match result {
                        Ok(tunnel) => return Ok(tunnel),
                        Err(error) => {
                            tracing::trace!(?error, "rejected TCP stealth connection");
                            continue;
                        }
                    }
                }
            }

            tokio::select! {
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    pending.push(Self::authenticate_stealth_stream(
                        stream,
                        self.local_url(),
                        self.stealth.clone(),
                        self.replay_guard.clone(),
                    ));
                }
                result = pending.next(), if !pending.is_empty() => {
                    match result.expect("pending stealth accept disappeared") {
                        Ok(tunnel) => return Ok(tunnel),
                        Err(error) => tracing::trace!(?error, "rejected TCP stealth connection"),
                    }
                }
            }
        }
    }
}

#[async_trait]
impl TunnelListener for TcpTunnelListener {
    async fn listen(&mut self) -> Result<(), TunnelError> {
        self.listener = None;

        let addr = SocketAddr::from_url(self.addr.clone(), IpVersion::Both).await?;
        let listener = bind::<TcpListener>()
            .addr(addr)
            .only_v6(true)
            .maybe_socket_mark(self.socket_mark)
            .call()?;

        self.addr
            .set_port(Some(listener.local_addr()?.port()))
            .unwrap();
        self.listener = Some(listener);

        Ok(())
    }

    async fn accept(&mut self) -> Result<Box<dyn Tunnel>, super::TunnelError> {
        if self.stealth.is_enabled() {
            return self.accept_stealth().await;
        }
        loop {
            match self.do_accept().await {
                Ok(ret) => return Ok(ret),
                Err(e) => {
                    use std::io::ErrorKind::*;
                    if matches!(
                        e.kind(),
                        NotConnected | ConnectionAborted | ConnectionRefused | ConnectionReset
                    ) {
                        tracing::warn!(?e, "accept fail with retryable error: {:?}", e);
                        continue;
                    }
                    tracing::warn!(?e, "accept fail");
                    return Err(e.into());
                }
            }
        }
    }

    fn local_url(&self) -> url::Url {
        self.addr.clone()
    }
}

fn get_tunnel_with_tcp_stream(
    stream: TcpStream,
    remote_url: url::Url,
    stealth: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
) -> Result<Box<dyn Tunnel>, super::TunnelError> {
    if let Err(e) = stream.set_nodelay(true) {
        tracing::warn!(?e, "set_nodelay fail in get_tunnel_with_tcp_stream");
    }

    let info = TunnelInfo {
        tunnel_type: "tcp".to_owned(),
        local_addr: Some(
            super::build_url_from_socket_addr(&stream.local_addr()?.to_string(), "tcp").into(),
        ),
        remote_addr: Some(remote_url.into()),
        resolved_remote_addr: Some(
            super::build_url_from_socket_addr(&stream.peer_addr()?.to_string(), "tcp").into(),
        ),
    };

    let (r, w) = stream.into_split();
    if stealth.is_enabled() {
        let stealth = stealth.fork_for_connection();
        return Ok(Box::new(TunnelWrapper::new_with_associate_data(
            StealthFramedReader::new(r, TCP_MTU_BYTES, stealth.clone()),
            FramedWriter::new_with_converter(w, StealthTcpZCPacketToBytes::new(stealth.clone())),
            Some(info),
            Some(Box::new(stealth)),
        )));
    }
    Ok(Box::new(TunnelWrapper::new(
        FramedReader::new(r, TCP_MTU_BYTES),
        FramedWriter::new(w),
        Some(info),
    )))
}

async fn authenticate_stealth_stream(
    stream: &mut TcpStream,
    stealth: &crate::tunnel::stealth::OuterSessionState,
) -> Result<(), super::TunnelError> {
    if !stealth.is_enabled() {
        return Ok(());
    }
    let preface = crate::tunnel::stealth::build_stream_gate_preface(stealth);
    let exchange = async {
        stream.write_all(&preface).await?;
        let mut ack = [0u8; crate::tunnel::stealth::STREAM_GATE_PREFACE_LEN];
        stream.read_exact(&mut ack).await?;
        if !crate::tunnel::stealth::verify_stream_gate_ack(stealth, &preface, &ack) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "TCP stealth ACK authentication failed",
            ));
        }
        Ok::<(), std::io::Error>(())
    };
    tokio::time::timeout(TCP_STEALTH_PREFACE_TIMEOUT, exchange)
        .await
        .map_err(|_| {
            TunnelError::IOError(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "TCP stealth preface exchange timed out",
            ))
        })??;
    Ok(())
}

#[derive(Debug)]
pub struct TcpTunnelConnector {
    addr: url::Url,

    bind_addrs: Vec<SocketAddr>,
    ip_version: IpVersion,
    resolved_addr: Option<SocketAddr>,
    socket_mark: Option<u32>,
    stealth: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
    stealth_candidate: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
    stealth_mode: TcpStealthMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TcpStealthMode {
    Disabled,
    Required,
    PreferLegacyFallback,
}

impl TcpTunnelConnector {
    pub fn new(addr: url::Url) -> Self {
        TcpTunnelConnector {
            addr,
            bind_addrs: vec![],
            ip_version: IpVersion::Both,
            resolved_addr: None,
            socket_mark: None,
            stealth: std::sync::Arc::new(crate::tunnel::stealth::OuterSessionState::disabled()),
            stealth_candidate: std::sync::Arc::new(
                crate::tunnel::stealth::OuterSessionState::disabled(),
            ),
            stealth_mode: TcpStealthMode::Disabled,
        }
    }

    pub fn set_stealth_candidate(
        &mut self,
        stealth: std::sync::Arc<crate::tunnel::stealth::OuterSessionState>,
    ) {
        self.stealth_mode = if stealth.is_enabled() {
            TcpStealthMode::PreferLegacyFallback
        } else {
            TcpStealthMode::Disabled
        };
        self.stealth_candidate = stealth;
    }

    async fn connect_with_default_bind(&self, addr: SocketAddr) -> Result<TcpStream, TunnelError> {
        tracing::info!(url = ?self.addr, ?addr, "connect tcp start, bind addrs: {:?}", self.bind_addrs);
        let stream = if self.socket_mark.is_some() {
            // SO_MARK requires applying the option on the socket before
            // connect, so go through TcpSocket rather than TcpStream::connect.
            let socket = if addr.is_ipv4() {
                TcpSocket::new_v4()?
            } else {
                TcpSocket::new_v6()?
            };
            apply_socket_mark(&socket2::SockRef::from(&socket), self.socket_mark)?;
            socket.connect(addr).await?
        } else {
            TcpStream::connect(addr).await?
        };
        tracing::info!(url = ?self.addr, ?addr, "connect tcp succ");
        Ok(stream)
    }

    async fn connect_with_custom_bind(
        &self,
        addr: SocketAddr,
    ) -> Result<TcpStream, super::TunnelError> {
        let futures = FuturesUnordered::new();

        for bind_addr in self.bind_addrs.iter() {
            tracing::info!(?bind_addr, ?addr, "bind addr");
            match bind::<TcpSocket>()
                .addr(*bind_addr)
                .only_v6(true)
                .maybe_socket_mark(self.socket_mark)
                .call()
            {
                Ok(socket) => futures.push(socket.connect(addr)),
                Err(error) => {
                    tracing::error!(?bind_addr, ?addr, ?error, "bind addr fail");
                    continue;
                }
            }
        }

        wait_for_connect_futures(futures).await
    }

    async fn connect_stream(&self, addr: SocketAddr) -> Result<TcpStream, TunnelError> {
        if self.bind_addrs.is_empty() {
            self.connect_with_default_bind(addr).await
        } else {
            self.connect_with_custom_bind(addr).await
        }
    }
}

#[async_trait]
impl super::TunnelConnector for TcpTunnelConnector {
    async fn connect(&mut self) -> Result<Box<dyn Tunnel>, TunnelError> {
        let addr = match self.resolved_addr {
            Some(addr) => addr,
            None => SocketAddr::from_url(self.addr.clone(), self.ip_version).await?,
        };
        match self.stealth_mode {
            TcpStealthMode::Disabled => get_tunnel_with_tcp_stream(
                self.connect_stream(addr).await?,
                self.addr.clone(),
                Arc::new(crate::tunnel::stealth::OuterSessionState::disabled()),
            ),
            TcpStealthMode::Required => {
                let mut stream = self.connect_stream(addr).await?;
                authenticate_stealth_stream(&mut stream, self.stealth.as_ref()).await?;
                get_tunnel_with_tcp_stream(stream, self.addr.clone(), self.stealth.clone())
            }
            TcpStealthMode::PreferLegacyFallback => {
                let mut stream = self.connect_stream(addr).await?;
                match authenticate_stealth_stream(&mut stream, self.stealth_candidate.as_ref())
                    .await
                {
                    Ok(()) => get_tunnel_with_tcp_stream(
                        stream,
                        self.addr.clone(),
                        self.stealth_candidate.clone(),
                    ),
                    Err(error) => {
                        tracing::info!(
                            ?error,
                            ?addr,
                            "TCP stealth preface failed, retrying legacy wire format"
                        );
                        get_tunnel_with_tcp_stream(
                            self.connect_stream(addr).await?,
                            self.addr.clone(),
                            Arc::new(crate::tunnel::stealth::OuterSessionState::disabled()),
                        )
                    }
                }
            }
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
        self.stealth = std::sync::Arc::new(crate::tunnel::stealth::OuterSessionState::disabled());
        self.stealth_mode = TcpStealthMode::Disabled;
    }

    fn require_stealth(&mut self) {
        if self.stealth_candidate.is_enabled() {
            self.stealth = self.stealth_candidate.clone();
            self.stealth_mode = TcpStealthMode::Required;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::{SinkExt, StreamExt};
    use tokio::io::AsyncWriteExt;

    use crate::tunnel::{
        TunnelConnector,
        common::tests::{_tunnel_bench, _tunnel_echo_server, _tunnel_pingpong},
        packet_def::ZCPacket,
    };

    use super::*;

    #[tokio::test]
    async fn tcp_pingpong() {
        let listener = TcpTunnelListener::new("tcp://0.0.0.0:31011".parse().unwrap());
        let connector = TcpTunnelConnector::new("tcp://127.0.0.1:31011".parse().unwrap());
        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    async fn tcp_stealth_pingpong() {
        let mut listener = TcpTunnelListener::new("tcp://0.0.0.0:31015".parse().unwrap());
        listener.set_stealth(crate::tunnel::stealth::build_outer_session(
            Some("tcp-secret"),
            true,
            true,
            0,
        ));
        let mut connector = TcpTunnelConnector::new("tcp://127.0.0.1:31015".parse().unwrap());
        connector.set_stealth_candidate(crate::tunnel::stealth::build_outer_session(
            Some("tcp-secret"),
            true,
            true,
            0,
        ));
        TunnelConnector::require_stealth(&mut connector);

        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    async fn tcp_unknown_capability_prefers_stealth_when_listener_supports_it() {
        let mut listener = TcpTunnelListener::new("tcp://127.0.0.1:31016".parse().unwrap());
        listener.set_stealth(crate::tunnel::stealth::build_outer_session(
            Some("tcp-secret"),
            true,
            true,
            0,
        ));
        let mut connector = TcpTunnelConnector::new("tcp://127.0.0.1:31016".parse().unwrap());
        connector.set_stealth_candidate(crate::tunnel::stealth::build_outer_session(
            Some("tcp-secret"),
            true,
            true,
            0,
        ));

        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    async fn tcp_unknown_capability_falls_back_to_fresh_plain_connection() {
        let mut listener = TcpTunnelListener::new("tcp://127.0.0.1:0".parse().unwrap());
        listener.listen().await.unwrap();
        let listener_url = listener.local_url();
        let server = tokio::spawn(async move {
            let first = listener.accept().await.unwrap();
            let (mut first_recv, _first_send) = first.split();
            let _ = tokio::time::timeout(Duration::from_secs(2), first_recv.next()).await;

            let second = listener.accept().await.unwrap();
            _tunnel_echo_server(second, false).await;
        });

        let mut connector = TcpTunnelConnector::new(listener_url);
        connector.set_stealth_candidate(crate::tunnel::stealth::build_outer_session(
            Some("tcp-secret"),
            true,
            true,
            0,
        ));
        let tunnel = tokio::time::timeout(Duration::from_secs(4), connector.connect())
            .await
            .expect("TCP stealth-to-legacy fallback timed out")
            .unwrap();
        assert!(tunnel.data().is_none());
        let (mut recv, mut send) = tunnel.split();
        send.send(ZCPacket::new_with_payload(b"fallback"))
            .await
            .unwrap();
        let echoed = tokio::time::timeout(Duration::from_secs(1), recv.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(echoed.payload(), b"fallback");
        send.close().await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("plain fallback server did not exit")
            .unwrap();
    }

    async fn assert_strict_listener_rejects(mut connector: TcpTunnelConnector) {
        let mut listener = TcpTunnelListener::new("tcp://127.0.0.1:0".parse().unwrap());
        listener.set_stealth(crate::tunnel::stealth::build_outer_session(
            Some("listener-secret"),
            true,
            true,
            0,
        ));
        listener.listen().await.unwrap();
        connector.addr = listener.local_url();

        let accept_task = tokio::spawn(async move { listener.accept().await });
        if let Ok(tunnel) = connector.connect().await {
            let (_recv, mut send) = tunnel.split();
            send.send(ZCPacket::new_with_payload(b"probe"))
                .await
                .unwrap();
            send.close().await.unwrap();
        }

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), accept_task)
                .await
                .is_err(),
            "strict listener exposed an unauthenticated tunnel"
        );
    }

    #[tokio::test]
    async fn tcp_stealth_listener_rejects_plain_record() {
        let connector = TcpTunnelConnector::new("tcp://127.0.0.1:0".parse().unwrap());
        assert_strict_listener_rejects(connector).await;
    }

    #[tokio::test]
    async fn tcp_stealth_listener_rejects_wrong_secret() {
        let mut connector = TcpTunnelConnector::new("tcp://127.0.0.1:0".parse().unwrap());
        connector.set_stealth_candidate(crate::tunnel::stealth::build_outer_session(
            Some("connector-secret"),
            true,
            true,
            0,
        ));
        TunnelConnector::require_stealth(&mut connector);
        assert_strict_listener_rejects(connector).await;
    }

    #[tokio::test]
    async fn tcp_stealth_slow_client_does_not_block_valid_accept() {
        let mut listener = TcpTunnelListener::new("tcp://127.0.0.1:0".parse().unwrap());
        listener.set_stealth(crate::tunnel::stealth::build_outer_session(
            Some("tcp-secret"),
            true,
            true,
            0,
        ));
        listener.listen().await.unwrap();
        let addr = listener.local_url();

        let _slow_client = TcpStream::connect(addr.socket_addrs(|| None).unwrap()[0])
            .await
            .unwrap();
        let mut connector = TcpTunnelConnector::new(addr);
        connector.set_stealth_candidate(crate::tunnel::stealth::build_outer_session(
            Some("tcp-secret"),
            true,
            true,
            0,
        ));
        TunnelConnector::require_stealth(&mut connector);
        let connect_task = tokio::spawn(async move { connector.connect().await });

        let accepted =
            tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept())
                .await
                .expect("slow TCP client blocked a valid stealth connection")
                .unwrap();
        assert!(connect_task.await.unwrap().is_ok());
        drop(accepted);
    }

    #[tokio::test]
    async fn tcp_stealth_preface_replay_is_rejected() {
        let mut listener = TcpTunnelListener::new("tcp://127.0.0.1:0".parse().unwrap());
        let state = crate::tunnel::stealth::build_outer_session(Some("tcp-secret"), true, true, 0);
        listener.set_stealth(state.clone());
        listener.listen().await.unwrap();
        let addr = listener.local_url().socket_addrs(|| None).unwrap()[0];
        let preface = crate::tunnel::stealth::build_stream_gate_preface(state.as_ref());

        let mut first = TcpStream::connect(addr).await.unwrap();
        first.write_all(&preface).await.unwrap();
        let accepted =
            tokio::time::timeout(std::time::Duration::from_millis(500), listener.accept())
                .await
                .unwrap()
                .unwrap();
        drop(accepted);
        drop(first);

        let mut replay = TcpStream::connect(addr).await.unwrap();
        replay.write_all(&preface).await.unwrap();
        replay.shutdown().await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(200), listener.accept())
                .await
                .is_err(),
            "replayed TCP stealth preface was accepted"
        );
    }

    #[tokio::test]
    async fn tcp_bench() {
        let listener = TcpTunnelListener::new("tcp://0.0.0.0:31012".parse().unwrap());
        let connector = TcpTunnelConnector::new("tcp://127.0.0.1:31012".parse().unwrap());
        _tunnel_bench(listener, connector).await
    }

    #[tokio::test]
    async fn tcp_bench_with_bind() {
        let listener = TcpTunnelListener::new("tcp://127.0.0.1:11013".parse().unwrap());
        let mut connector = TcpTunnelConnector::new("tcp://127.0.0.1:11013".parse().unwrap());
        connector.set_bind_addrs(vec!["127.0.0.1:0".parse().unwrap()]);
        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    #[should_panic]
    async fn tcp_bench_with_bind_fail() {
        let listener = TcpTunnelListener::new("tcp://127.0.0.1:11014".parse().unwrap());
        let mut connector = TcpTunnelConnector::new("tcp://127.0.0.1:11014".parse().unwrap());
        connector.set_bind_addrs(vec!["10.0.0.1:0".parse().unwrap()]);
        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    async fn bind_same_port() {
        let mut listener = TcpTunnelListener::new("tcp://[::]:31014".parse().unwrap());
        let mut listener2 = TcpTunnelListener::new("tcp://0.0.0.0:31014".parse().unwrap());
        listener.listen().await.unwrap();
        listener2.listen().await.unwrap();
    }

    #[tokio::test]
    async fn ipv6_pingpong() {
        let listener = TcpTunnelListener::new("tcp://[::1]:31015".parse().unwrap());
        let connector = TcpTunnelConnector::new("tcp://[::1]:31015".parse().unwrap());
        _tunnel_pingpong(listener, connector).await
    }

    #[tokio::test]
    async fn ipv6_domain_pingpong() {
        let listener = TcpTunnelListener::new("tcp://[::1]:31015".parse().unwrap());
        let mut connector =
            TcpTunnelConnector::new("tcp://test.easytier.top:31015".parse().unwrap());
        connector.set_ip_version(IpVersion::V6);
        _tunnel_pingpong(listener, connector).await;

        let listener = TcpTunnelListener::new("tcp://127.0.0.1:31015".parse().unwrap());
        let mut connector =
            TcpTunnelConnector::new("tcp://test.easytier.top:31015".parse().unwrap());
        connector.set_ip_version(IpVersion::V4);
        _tunnel_pingpong(listener, connector).await;
    }

    #[tokio::test]
    async fn connector_keeps_source_addr_and_reports_resolved_addr() {
        let mut listener = TcpTunnelListener::new("tcp://127.0.0.1:0".parse().unwrap());
        listener.listen().await.unwrap();

        let port = listener.local_url().port().unwrap();
        let source_url: url::Url = format!("tcp://localhost:{port}").parse().unwrap();
        let mut connector = TcpTunnelConnector::new(source_url.clone());
        connector.set_ip_version(IpVersion::V4);

        let accept_task = tokio::spawn(async move { listener.accept().await.unwrap() });
        let tunnel = connector.connect().await.unwrap();
        let accepted_tunnel = accept_task.await.unwrap();

        let info = tunnel.info().unwrap();
        assert_eq!(info.remote_addr.unwrap().url, source_url.to_string());

        let resolved_remote_addr: url::Url = info.resolved_remote_addr.unwrap().into();
        assert_eq!(resolved_remote_addr.host_str(), Some("127.0.0.1"));
        assert_eq!(resolved_remote_addr.port(), Some(port));

        let accepted_info = accepted_tunnel.info().unwrap();
        assert_eq!(
            accepted_info.remote_addr,
            accepted_info.resolved_remote_addr,
        );
    }

    #[tokio::test]
    async fn connector_uses_pre_resolved_addr_without_resolving_url() {
        let mut listener = TcpTunnelListener::new("tcp://127.0.0.1:0".parse().unwrap());
        listener.listen().await.unwrap();

        let port = listener.local_url().port().unwrap();
        let source_url: url::Url = format!("tcp://unresolvable.invalid:{port}")
            .parse()
            .unwrap();
        let resolved_addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let mut connector = TcpTunnelConnector::new(source_url.clone());
        connector.set_resolved_addr(resolved_addr);

        let accept_task = tokio::spawn(async move { listener.accept().await.unwrap() });
        let tunnel = connector.connect().await.unwrap();
        let _accepted_tunnel = accept_task.await.unwrap();

        let info = tunnel.info().unwrap();
        assert_eq!(info.remote_addr.unwrap().url, source_url.to_string());

        let resolved_remote_addr: url::Url = info.resolved_remote_addr.unwrap().into();
        assert_eq!(resolved_remote_addr.host_str(), Some("127.0.0.1"));
        assert_eq!(resolved_remote_addr.port(), Some(port));
    }

    #[tokio::test]
    async fn test_alloc_port() {
        // v4
        let mut listener = TcpTunnelListener::new("tcp://0.0.0.0:0".parse().unwrap());
        listener.listen().await.unwrap();
        let port = listener.local_url().port().unwrap();
        assert!(port > 0);

        // v6
        let mut listener = TcpTunnelListener::new("tcp://[::]:0".parse().unwrap());
        listener.listen().await.unwrap();
        let port = listener.local_url().port().unwrap();
        assert!(port > 0);
    }
}
