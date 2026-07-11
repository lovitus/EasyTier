use std::{
    fmt::Debug,
    net::IpAddr,
    str::FromStr,
    sync::{Arc, Weak},
};

#[cfg(feature = "quic")]
use std::collections::HashSet;

#[cfg(feature = "quic")]
use crate::tunnel::quic::QuicBindMode;

use anyhow::Context;
use async_trait::async_trait;
use tokio::task::JoinSet;

use crate::{
    common::{
        error::Error,
        global_ctx::{ArcGlobalCtx, GlobalCtxEvent},
        netns::NetNS,
    },
    peers::peer_manager::PeerManager,
    tunnel::{
        self, IpScheme, Tunnel, TunnelListener, TunnelScheme, ring::RingTunnelListener,
        tcp::TcpTunnelListener, udp::UdpTunnelListener,
    },
    utils::BoxExt,
};

pub fn create_listener_by_url(
    l: &url::Url,
    global_ctx: ArcGlobalCtx,
) -> Result<Box<dyn TunnelListener>, Error> {
    use crate::common::config::ConfigLoader;
    let socket_mark = global_ctx.config.get_flags().socket_mark;
    Ok(match l.try_into()? {
        TunnelScheme::Ip(scheme) => match scheme {
            IpScheme::Tcp => {
                let mut l = TcpTunnelListener::new(l.clone());
                l.set_socket_mark(socket_mark);
                let flags = global_ctx.config.get_flags();
                let tcp_stealth = crate::common::stealth_registry::protocol_enabled(
                    &flags,
                    crate::common::stealth_registry::StealthProtocol::Tcp,
                );
                l.set_stealth(crate::tunnel::stealth::build_outer_session(
                    global_ctx.get_network_identity().network_secret.as_deref(),
                    tcp_stealth,
                    global_ctx.is_secure_mode_enabled(),
                    flags.stealth_window_secs,
                ));
                l.boxed()
            }
            IpScheme::Udp => {
                let mut l = UdpTunnelListener::new(l.clone());
                l.set_socket_mark(socket_mark);
                let flags = global_ctx.config.get_flags();
                let secure_mode = global_ctx.is_secure_mode_enabled();
                let udp_stealth = crate::common::stealth_registry::protocol_enabled(
                    &flags,
                    crate::common::stealth_registry::StealthProtocol::Udp,
                );
                l.set_stealth(crate::tunnel::stealth::build_outer_session(
                    global_ctx.get_network_identity().network_secret.as_deref(),
                    udp_stealth,
                    secure_mode,
                    flags.stealth_window_secs,
                ));
                l.boxed()
            }
            #[cfg(feature = "wireguard")]
            IpScheme::Wg => {
                use crate::tunnel::wireguard::{WgConfig, WgTunnelListener};
                let nid = global_ctx.get_network_identity();
                let wg_config = WgConfig::new_from_network_identity(
                    &nid.network_name,
                    &nid.network_secret.unwrap_or_default(),
                );
                let mut l = WgTunnelListener::new(l.clone(), wg_config);
                l.set_socket_mark(socket_mark);
                let flags = global_ctx.get_flags();
                let enabled = crate::common::stealth_registry::protocol_enabled(
                    &flags,
                    crate::common::stealth_registry::StealthProtocol::Wg,
                );
                l.set_stealth(crate::tunnel::stealth::build_outer_session(
                    global_ctx.get_network_identity().network_secret.as_deref(),
                    enabled,
                    global_ctx.is_secure_mode_enabled(),
                    flags.stealth_window_secs,
                ));
                l.boxed()
            }
            #[cfg(feature = "quic")]
            IpScheme::Quic => create_quic_listener(l, global_ctx, None),
            #[cfg(feature = "websocket")]
            IpScheme::Ws | IpScheme::Wss => {
                let mut l = tunnel::websocket::WsTunnelListener::new(l.clone());
                l.set_socket_mark(socket_mark);
                let flags = global_ctx.get_flags();
                let protocol = if matches!(scheme, IpScheme::Wss) {
                    crate::common::stealth_registry::StealthProtocol::Wss
                } else {
                    crate::common::stealth_registry::StealthProtocol::Ws
                };
                let enabled = crate::common::stealth_registry::protocol_enabled(&flags, protocol);
                l.set_stealth(crate::tunnel::stealth::build_outer_session(
                    global_ctx.get_network_identity().network_secret.as_deref(),
                    enabled,
                    global_ctx.is_secure_mode_enabled(),
                    flags.stealth_window_secs,
                ));
                l.boxed()
            }
            #[cfg(feature = "faketcp")]
            IpScheme::FakeTcp => {
                let mut listener = tunnel::fake_tcp::FakeTcpTunnelListener::new(l.clone());
                let flags = global_ctx.get_flags();
                let enabled = crate::common::stealth_registry::protocol_enabled(
                    &flags,
                    crate::common::stealth_registry::StealthProtocol::FakeTcp,
                );
                listener.set_stealth(crate::tunnel::stealth::build_outer_session(
                    global_ctx.get_network_identity().network_secret.as_deref(),
                    enabled,
                    global_ctx.is_secure_mode_enabled(),
                    flags.stealth_window_secs,
                ));
                listener.boxed()
            }
        },
        #[cfg(unix)]
        TunnelScheme::Unix => tunnel::unix::UnixSocketTunnelListener::new(l.clone()).boxed(),
        _ => return Err(Error::InvalidUrl(l.to_string())),
    })
}

#[cfg(feature = "quic")]
fn create_quic_listener(
    l: &url::Url,
    global_ctx: ArcGlobalCtx,
    bind_mode: Option<QuicBindMode>,
) -> Box<dyn TunnelListener> {
    // QUIC reads socket_mark from global_ctx in QuicEndpointManager.
    let mut listener = if let Some(bind_mode) = bind_mode {
        tunnel::quic::QuicTunnelListener::new_with_bind_mode(
            l.clone(),
            global_ctx.clone(),
            bind_mode,
        )
    } else {
        tunnel::quic::QuicTunnelListener::new(l.clone(), global_ctx.clone())
    };
    let flags = global_ctx.get_flags();
    let enabled = crate::common::stealth_registry::protocol_enabled(
        &flags,
        crate::common::stealth_registry::StealthProtocol::Quic,
    );
    listener.set_stealth(crate::tunnel::stealth::build_outer_session(
        global_ctx.get_network_identity().network_secret.as_deref(),
        enabled,
        global_ctx.is_secure_mode_enabled(),
        flags.stealth_window_secs,
    ));
    listener.boxed()
}

pub fn is_url_host_ipv6(l: &url::Url) -> bool {
    l.host_str().is_some_and(|h| h.contains(':'))
}

pub fn is_url_host_unspecified(l: &url::Url) -> bool {
    url_ip_literal(l).is_some_and(|ip| ip.is_unspecified())
}

fn url_ip_literal(l: &url::Url) -> Option<IpAddr> {
    match l.host()? {
        url::Host::Ipv4(ip) => Some(IpAddr::V4(ip)),
        url::Host::Ipv6(ip) => Some(IpAddr::V6(ip)),
        url::Host::Domain(host) => IpAddr::from_str(host).ok(),
    }
}

#[cfg(feature = "quic")]
#[derive(Default)]
struct QuicListenerIndex {
    ipv4_ports: HashSet<u16>,
    ipv4_unspecified_ports: HashSet<u16>,
    ipv6_ports: HashSet<u16>,
}

#[cfg(feature = "quic")]
impl QuicListenerIndex {
    fn from_listeners(listeners: &[url::Url]) -> Self {
        let mut index = Self::default();
        for listener in listeners
            .iter()
            .filter(|listener| listener.scheme() == "quic")
        {
            let Some(port) = listener.port() else {
                continue;
            };
            match url_ip_literal(listener) {
                Some(IpAddr::V4(ip)) => {
                    index.ipv4_ports.insert(port);
                    if ip.is_unspecified() {
                        index.ipv4_unspecified_ports.insert(port);
                    }
                }
                Some(IpAddr::V6(_)) => {
                    index.ipv6_ports.insert(port);
                }
                None => {}
            }
        }
        index
    }

    fn bind_mode(&self, listener: &url::Url) -> Option<QuicBindMode> {
        let port = listener.port()?;
        match url_ip_literal(listener)? {
            IpAddr::V4(_) => Some(QuicBindMode::V4Only),
            IpAddr::V6(ip)
                if ip.is_unspecified() && port != 0 && self.ipv4_ports.contains(&port) =>
            {
                Some(QuicBindMode::V6Only)
            }
            IpAddr::V6(ip) if ip.is_unspecified() => Some(QuicBindMode::DualStack),
            IpAddr::V6(_) => Some(QuicBindMode::V6Only),
        }
    }

    fn needs_ipv6_companion(&self, listener: &url::Url, enable_ipv6: bool) -> bool {
        if !enable_ipv6 || listener.scheme() != "quic" {
            return false;
        }
        let Some(port) = listener.port() else {
            return false;
        };
        port != 0 && self.ipv4_unspecified_ports.contains(&port) && !self.ipv6_ports.contains(&port)
    }
}

#[async_trait]
pub trait TunnelHandlerForListener {
    async fn handle_tunnel(&self, tunnel: Box<dyn Tunnel>) -> Result<(), Error>;
}

#[async_trait]
impl TunnelHandlerForListener for PeerManager {
    #[tracing::instrument]
    async fn handle_tunnel(&self, tunnel: Box<dyn Tunnel>) -> Result<(), Error> {
        self.add_tunnel_as_server(tunnel, true).await
    }
}

pub trait ListenerCreatorTrait: Fn() -> Box<dyn TunnelListener> + Send + Sync {}
impl<T: Send + Sync> ListenerCreatorTrait for T where T: Fn() -> Box<dyn TunnelListener> + Send {}
pub type ListenerCreator = Box<dyn ListenerCreatorTrait>;

#[derive(Clone)]
struct ListenerFactory {
    creator_fn: Arc<ListenerCreator>,
    must_succ: bool,
}

pub struct ListenerManager<H> {
    global_ctx: ArcGlobalCtx,
    net_ns: NetNS,
    listeners: Vec<ListenerFactory>,
    peer_manager: Weak<H>,

    tasks: JoinSet<()>,
}

impl<H: TunnelHandlerForListener + Send + Sync + 'static + Debug> ListenerManager<H> {
    pub fn new(global_ctx: ArcGlobalCtx, peer_manager: Arc<H>) -> Self {
        Self {
            global_ctx: global_ctx.clone(),
            net_ns: global_ctx.net_ns.clone(),
            listeners: Vec::new(),
            peer_manager: Arc::downgrade(&peer_manager),
            tasks: JoinSet::new(),
        }
    }

    pub async fn prepare_listeners(&mut self) -> Result<(), Error> {
        let self_id = self.global_ctx.get_id();
        self.add_listener(
            move || {
                Box::new(RingTunnelListener::new(
                    format!("ring://{}", self_id).parse().unwrap(),
                ))
            },
            true,
        )
        .await?;

        let configured_listeners = self.global_ctx.config.get_listener_uris();
        #[cfg(feature = "quic")]
        let quic_index = QuicListenerIndex::from_listeners(&configured_listeners);
        let enable_ipv6 = self.global_ctx.config.get_flags().enable_ipv6;

        for l in configured_listeners.iter() {
            let l = l.clone();
            let Ok(_) = create_listener_by_url(&l, self.global_ctx.clone()) else {
                let msg = format!("failed to get listener by url: {}, maybe not supported", l);
                self.global_ctx
                    .issue_event(GlobalCtxEvent::ListenerAddFailed(l.clone(), msg));
                continue;
            };
            let ctx = self.global_ctx.clone();

            let listener = l.clone();
            #[cfg(feature = "quic")]
            let quic_bind_mode = quic_index.bind_mode(&listener);
            self.add_listener(
                move || {
                    #[cfg(feature = "quic")]
                    if listener.scheme() == "quic" {
                        return create_quic_listener(&listener, ctx.clone(), quic_bind_mode);
                    }
                    create_listener_by_url(&listener, ctx.clone()).unwrap()
                },
                true,
            )
            .await?;

            #[cfg(feature = "quic")]
            let add_quic_ipv6_companion = quic_index.needs_ipv6_companion(&l, enable_ipv6);
            #[cfg(not(feature = "quic"))]
            let add_quic_ipv6_companion = false;

            if l.scheme() == "quic" && enable_ipv6 && is_url_host_unspecified(&l) {
                if l.port() == Some(0) {
                    tracing::warn!(
                        listener = %l,
                        "automatic QUIC IPv6 companion requires a nonzero port"
                    );
                } else if add_quic_ipv6_companion {
                    let mut ipv6_listener = l.clone();
                    ipv6_listener
                        .set_host(Some("[::]"))
                        .with_context(|| format!("failed to set ipv6 host for listener: {}", l))?;
                    let ctx = self.global_ctx.clone();
                    self.add_listener(
                        move || {
                            #[cfg(feature = "quic")]
                            {
                                return create_quic_listener(
                                    &ipv6_listener,
                                    ctx.clone(),
                                    Some(QuicBindMode::V6Only),
                                );
                            }
                            #[allow(unreachable_code)]
                            create_listener_by_url(&ipv6_listener, ctx.clone()).unwrap()
                        },
                        false,
                    )
                    .await?;
                }
            } else if enable_ipv6
                && !is_url_host_ipv6(&l)
                && is_url_host_unspecified(&l)
                && l.scheme() != "faketcp"
            {
                let mut ipv6_listener = l.clone();
                ipv6_listener
                    .set_host(Some("[::]".to_string().as_str()))
                    .with_context(|| format!("failed to set ipv6 host for listener: {}", l))?;
                let ctx = self.global_ctx.clone();
                self.add_listener(
                    move || create_listener_by_url(&ipv6_listener, ctx.clone()).unwrap(),
                    false,
                )
                .await?;
            }
        }

        Ok(())
    }

    pub async fn add_listener<C: ListenerCreatorTrait + 'static>(
        &mut self,
        creator: C,
        must_succ: bool,
    ) -> Result<(), Error> {
        self.listeners.push(ListenerFactory {
            creator_fn: Arc::new(Box::new(creator)),
            must_succ,
        });
        Ok(())
    }

    #[tracing::instrument(skip(creator))]
    async fn run_listener(
        creator: Arc<ListenerCreator>,
        peer_manager: Weak<H>,
        global_ctx: ArcGlobalCtx,
    ) {
        let mut err_count = 0;
        loop {
            let mut l = (creator)();
            let _g = global_ctx.net_ns.guard();
            match l.listen().await {
                Ok(_) => {
                    err_count = 0;
                    global_ctx.add_running_listener(l.local_url());
                    global_ctx.issue_event(GlobalCtxEvent::ListenerAdded(l.local_url()));
                }
                Err(e) => {
                    tracing::error!(?e, ?l, "listener listen error");
                    global_ctx.issue_event(GlobalCtxEvent::ListenerAddFailed(
                        l.local_url(),
                        format!("error: {:?}, retry listen later...", e),
                    ));
                    err_count += 1;
                    if err_count > 5 {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            }
            loop {
                let ret = match l.accept().await {
                    Ok(ret) => ret,
                    Err(e) => {
                        global_ctx.issue_event(GlobalCtxEvent::ListenerAcceptFailed(
                            l.local_url(),
                            format!("error: {:?}, retry listen later...", e),
                        ));
                        tracing::error!(?e, ?l, "listener accept error");
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                        break;
                    }
                };

                let tunnel_info = ret.info().unwrap();
                global_ctx.issue_event(GlobalCtxEvent::ConnectionAccepted(
                    tunnel_info
                        .local_addr
                        .clone()
                        .unwrap_or_default()
                        .to_string(),
                    tunnel_info
                        .remote_addr
                        .clone()
                        .unwrap_or_default()
                        .to_string(),
                ));
                tracing::info!(ret = ?ret, "conn accepted");
                let peer_manager = peer_manager.clone();
                let global_ctx = global_ctx.clone();
                tokio::spawn(async move {
                    let Some(peer_manager) = peer_manager.upgrade() else {
                        tracing::error!("peer manager is gone, cannot handle tunnel");
                        return;
                    };
                    let server_ret = peer_manager.handle_tunnel(ret).await;
                    if let Err(e) = &server_ret {
                        global_ctx.issue_event(GlobalCtxEvent::ConnectionError(
                            tunnel_info.local_addr.unwrap_or_default().to_string(),
                            tunnel_info.remote_addr.unwrap_or_default().to_string(),
                            e.to_string(),
                        ));
                        tracing::error!(error = ?e, "handle conn error");
                    }
                });
            }
        }
    }

    pub async fn run(&mut self) -> Result<(), Error> {
        for listener in &self.listeners {
            if listener.must_succ {
                // try listen once
                let mut l = (listener.creator_fn)();
                let _g = self.net_ns.guard();
                l.listen()
                    .await
                    .with_context(|| format!("failed to listen on {}", l.local_url()))?;
            }

            self.tasks.spawn(Self::run_listener(
                listener.creator_fn.clone(),
                self.peer_manager.clone(),
                self.global_ctx.clone(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI32, Ordering};

    use futures::{SinkExt, StreamExt};
    use tokio::time::timeout;

    use crate::{
        common::global_ctx::tests::get_mock_global_ctx,
        tunnel::{TunnelConnector, TunnelError, packet_def::ZCPacket, ring::RingTunnelConnector},
    };

    use super::*;

    #[cfg(feature = "quic")]
    #[test]
    fn quic_listener_index_is_literal_only_and_port_aware() {
        let listeners = vec![
            "quic://0.0.0.0:21012".parse().unwrap(),
            "quic://[::]:21012".parse().unwrap(),
            "quic://0.0.0.0:21013".parse().unwrap(),
            "quic://0.0.0.0:0".parse().unwrap(),
            "quic://example.com:21014".parse().unwrap(),
        ];
        let index = QuicListenerIndex::from_listeners(&listeners);

        assert_eq!(index.bind_mode(&listeners[0]), Some(QuicBindMode::V4Only));
        assert_eq!(index.bind_mode(&listeners[1]), Some(QuicBindMode::V6Only));
        assert!(!index.needs_ipv6_companion(&listeners[0], true));
        assert!(index.needs_ipv6_companion(&listeners[2], true));
        assert!(!index.needs_ipv6_companion(&listeners[3], true));
        assert_eq!(index.bind_mode(&listeners[4]), None);
        assert!(!index.needs_ipv6_companion(&listeners[4], true));
    }

    #[cfg(feature = "quic")]
    #[test]
    fn standalone_ipv6_unspecified_quic_listener_remains_dual_stack() {
        let listener: url::Url = "quic://[::]:21015".parse().unwrap();
        let index = QuicListenerIndex::from_listeners(std::slice::from_ref(&listener));

        assert_eq!(index.bind_mode(&listener), Some(QuicBindMode::DualStack));
    }

    #[derive(Debug)]
    struct MockListenerHandler {}

    #[async_trait]
    impl TunnelHandlerForListener for MockListenerHandler {
        async fn handle_tunnel(&self, tunnel: Box<dyn Tunnel>) -> Result<(), Error> {
            let data = "abc";
            let (_recv, mut send) = tunnel.split();

            let zc_packet = ZCPacket::new_with_payload(data.as_bytes());
            send.send(zc_packet).await.unwrap();
            Err(Error::Unknown)
        }
    }

    #[tokio::test]
    async fn handle_error_in_accept() {
        let handler = Arc::new(MockListenerHandler {});
        let mut listener_mgr = ListenerManager::new(get_mock_global_ctx(), handler.clone());

        let ring_id = format!("ring://{}", uuid::Uuid::new_v4());

        let ring_id_clone = ring_id.clone();
        listener_mgr
            .add_listener(
                move || Box::new(RingTunnelListener::new(ring_id_clone.parse().unwrap())),
                true,
            )
            .await
            .unwrap();
        listener_mgr.run().await.unwrap();

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let connect_once = |ring_id| async move {
            let tunnel = RingTunnelConnector::new(ring_id).connect().await.unwrap();
            let (mut recv, _send) = tunnel.split();
            assert_eq!(
                recv.next().await.unwrap().unwrap().payload(),
                "abc".as_bytes()
            );
            tunnel
        };

        timeout(std::time::Duration::from_secs(1), async move {
            connect_once(ring_id.parse().unwrap()).await;
            // handle tunnel fail should not impact the second connect
            connect_once(ring_id.parse().unwrap()).await;
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn retry_listen() {
        let counter = Arc::new(AtomicI32::new(0));
        let drop_counter = Arc::new(AtomicI32::new(0));
        struct MockListener {
            counter: Arc<AtomicI32>,
            drop_counter: Arc<AtomicI32>,
        }

        #[async_trait::async_trait]
        impl TunnelListener for MockListener {
            async fn listen(&mut self) -> Result<(), TunnelError> {
                self.counter.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }

            async fn accept(&mut self) -> Result<Box<dyn Tunnel>, TunnelError> {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                Err(TunnelError::BufferFull)
            }

            fn local_url(&self) -> url::Url {
                "mock://".parse().unwrap()
            }
        }

        impl Drop for MockListener {
            fn drop(&mut self) {
                self.drop_counter.fetch_add(1, Ordering::Relaxed);
            }
        }

        let handler = Arc::new(MockListenerHandler {});
        let mut listener_mgr = ListenerManager::new(get_mock_global_ctx(), handler.clone());
        let counter_clone = counter.clone();
        let drop_counter_clone = drop_counter.clone();
        listener_mgr
            .add_listener(
                move || {
                    Box::new(MockListener {
                        counter: counter_clone.clone(),
                        drop_counter: drop_counter_clone.clone(),
                    })
                },
                true,
            )
            .await
            .unwrap();
        listener_mgr.run().await.unwrap();

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        assert!(counter.load(Ordering::Relaxed) >= 2);
        assert!(drop_counter.load(Ordering::Relaxed) >= 1);
    }
}
