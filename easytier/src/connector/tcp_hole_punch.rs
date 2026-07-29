use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Error};
use hotpath::instant::Instant;
use rand::Rng as _;
use tokio::task::JoinSet;

use crate::{
    common::{
        PeerId,
        global_ctx::{ProtocolLoopScope, UnderlayBreakerKey, UnderlayBreakerScope},
        join_joinset_background,
        stun::StunInfoCollectorTrait,
        transport_priority::TransportPathClass,
        underlay_guard,
    },
    connector::udp_hole_punch::BackOff,
    peers::{
        peer_manager::PeerManager,
        peer_task::{PeerTaskLauncher, PeerTaskManager},
    },
    proto::{
        common::NatType,
        peer_rpc::{
            TcpHolePunchRequest, TcpHolePunchResponse, TcpHolePunchRpc,
            TcpHolePunchRpcClientFactory, TcpHolePunchRpcServer,
        },
        rpc_types::{self, controller::BaseController},
    },
    tunnel::{
        IpScheme, TunnelConnector as _, TunnelListener as _,
        tcp::{TcpTunnelConnector, TcpTunnelListener},
    },
};

use crate::connector::{
    should_attempt_ranked_hole_punch, should_background_p2p_with_peer, should_try_p2p_with_peer,
};

pub const BLACKLIST_TIMEOUT_SEC: u64 = 3600;
pub const LOOP_BLACKLIST_TIMEOUT_SEC: u64 = 300;

fn handle_rpc_result<T>(
    ret: Result<T, rpc_types::error::Error>,
    dst_peer_id: PeerId,
    blacklist: &timedmap::TimedMap<PeerId, ()>,
) -> Result<T, rpc_types::error::Error> {
    match ret {
        Ok(ret) => Ok(ret),
        Err(e) => {
            if matches!(e, rpc_types::error::Error::InvalidServiceKey(_, _)) {
                blacklist.insert(dst_peer_id, (), Duration::from_secs(BLACKLIST_TIMEOUT_SEC));
            }
            Err(e)
        }
    }
}

fn is_symmetric_tcp_nat(nat_type: NatType) -> bool {
    matches!(
        nat_type,
        NatType::Symmetric | NatType::SymmetricEasyInc | NatType::SymmetricEasyDec
    )
}

fn bind_addr_for_port(port: u16, is_v6: bool) -> SocketAddr {
    if is_v6 {
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port)
    } else {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)
    }
}

async fn select_local_port(peer_mgr: &Arc<PeerManager>, is_v6: bool) -> Result<u16, Error> {
    let bind_addr = bind_addr_for_port(0, is_v6);
    tracing::trace!(?bind_addr, is_v6, "tcp hole punch select local port");
    let _g = peer_mgr.get_global_ctx().net_ns.guard();
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    let port = listener.local_addr()?.port();
    tracing::debug!(?bind_addr, port, "tcp hole punch selected local port");
    Ok(port)
}

// tcp support simultaneous connect, so initiator and server can both use connect.
async fn try_connect_to_remote(
    peer_mgr: Arc<PeerManager>,
    a_mapped_addr: SocketAddr,
    local_port: u16,
    is_client: bool,
    max_attempts: u32,
    loop_blacklist: Option<(PeerId, Arc<timedmap::TimedMap<PeerId, ()>>)>,
    loop_addr_blacklist: Option<Arc<timedmap::TimedMap<SocketAddr, ()>>>,
) -> Result<(), Error> {
    tracing::info!(
        ?a_mapped_addr,
        local_port,
        "tcp hole punch server start connect loop"
    );

    let global_ctx = peer_mgr.get_global_ctx();
    let expected_peer_id = loop_blacklist.as_ref().map(|(peer_id, _)| *peer_id);
    if is_client
        && expected_peer_id.is_some_and(|peer_id| {
            global_ctx.is_underlay_attempt_blocked(&[UnderlayBreakerKey::peer(
                peer_id,
                IpScheme::Tcp,
                UnderlayBreakerScope::HolePunch,
            )])
        })
    {
        anyhow::bail!("tcp hole punch peer is gated by underlay breaker");
    }
    let mut preflight = if is_client {
        Some(
            underlay_guard::prepare_underlay_attempt(
                &global_ctx,
                a_mapped_addr,
                IpScheme::Tcp,
                UnderlayBreakerScope::HolePunch,
                expected_peer_id,
            )
            .await?,
        )
    } else {
        None
    };

    let mut connector =
        TcpTunnelConnector::new(format!("tcp://{}", a_mapped_addr).parse().unwrap());
    connector.set_bind_addrs(vec![bind_addr_for_port(
        local_port,
        a_mapped_addr.is_ipv6(),
    )]);

    let start = tokio::time::Instant::now();
    let mut attempts: u32 = 0;
    while start.elapsed() < Duration::from_secs(10) && attempts < max_attempts {
        if let Some(blacklist) = loop_addr_blacklist.as_ref() {
            blacklist.cleanup();
            if blacklist.contains(&a_mapped_addr) {
                tracing::warn!(
                    ?a_mapped_addr,
                    "tcp hole punch connect loop skipped (addr blacklisted)"
                );
                return Ok(());
            }
        }
        // Legacy fallback: tcp hole-punch now installs tunnels through the
        // no-record PeerManager helpers, so GlobalCtx hole-punch suppression is
        // no longer written by the active runtime path. Keep this guard to
        // preserve a kill switch if another caller reuses the old semantics.
        if peer_mgr
            .get_global_ctx()
            .is_protocol_loop_suppressed(IpScheme::Tcp, ProtocolLoopScope::HolePunch)
        {
            return Ok(());
        }
        attempts = attempts.wrapping_add(1);
        let _g = peer_mgr.get_global_ctx().net_ns.guard();
        if let Some(mut guard) = preflight.take() {
            guard.commit();
        }
        if let Ok(Ok(tunnel)) =
            tokio::time::timeout(Duration::from_secs(3), connector.connect()).await
        {
            let add_tunnel_ret = if is_client {
                peer_mgr
                    .add_client_tunnel_with_peer_id_hint_without_runtime_loop_record(
                        tunnel,
                        false,
                        expected_peer_id,
                    )
                    .await
                    .map(|_| ())
            } else {
                peer_mgr
                    .add_tunnel_as_server_without_runtime_loop_record(tunnel, false)
                    .await
            };
            if let Err(e) = add_tunnel_ret {
                tracing::error!(
                    ?a_mapped_addr,
                    local_port,
                    attempts,
                    ?e,
                    "tcp hole punch server connected and added client tunnel failed"
                );
                if e.is_self_loop_signal()
                    && let Some((dst_peer_id, loop_blacklist)) = loop_blacklist.as_ref()
                {
                    loop_blacklist.insert(
                        *dst_peer_id,
                        (),
                        Duration::from_secs(LOOP_BLACKLIST_TIMEOUT_SEC),
                    );
                    return Err(e.into());
                }
                if e.is_self_loop_signal()
                    && let Some(loop_addr_blacklist) = loop_addr_blacklist.as_ref()
                {
                    loop_addr_blacklist.insert(
                        a_mapped_addr,
                        (),
                        Duration::from_secs(LOOP_BLACKLIST_TIMEOUT_SEC),
                    );
                    return Err(e.into());
                }
                if peer_mgr
                    .get_global_ctx()
                    .is_protocol_loop_suppressed(IpScheme::Tcp, ProtocolLoopScope::HolePunch)
                {
                    return Err(e.into());
                }
                continue;
            } else {
                tracing::info!(
                    ?a_mapped_addr,
                    local_port,
                    attempts,
                    is_client,
                    "tcp hole punch server connected and added tunnel"
                );
                return Ok(());
            }
        }
        tracing::trace!(
            ?a_mapped_addr,
            local_port,
            attempts,
            "tcp hole punch server connect attempt failed"
        );
        let sleep_ms = rand::thread_rng().gen_range(10..100);
        tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
    }

    tracing::warn!(
        ?a_mapped_addr,
        local_port,
        attempts,
        "tcp hole punch server connect loop timeout"
    );

    Err(anyhow::anyhow!(
        "tcp hole punch server connect loop timeout"
    ))
}

struct TcpHolePunchServer {
    peer_mgr: Arc<PeerManager>,
    tasks: Arc<std::sync::Mutex<JoinSet<()>>>,
    loop_addr_blacklist: Arc<timedmap::TimedMap<SocketAddr, ()>>,
}

impl TcpHolePunchServer {
    fn new(peer_mgr: Arc<PeerManager>) -> Arc<Self> {
        let tasks = Arc::new(std::sync::Mutex::new(JoinSet::new()));
        join_joinset_background(tasks.clone(), "tcp hole punch server".to_string());
        Arc::new(Self {
            peer_mgr,
            tasks,
            loop_addr_blacklist: Arc::new(timedmap::TimedMap::new()),
        })
    }
}

#[async_trait::async_trait]
impl TcpHolePunchRpc for TcpHolePunchServer {
    type Controller = BaseController;

    #[tracing::instrument(skip(self), fields(a_mapped_addr = ?input.connector_mapped_addr), err)]
    async fn exchange_mapped_addr(
        &self,
        _ctrl: Self::Controller,
        input: TcpHolePunchRequest,
    ) -> rpc_types::error::Result<TcpHolePunchResponse> {
        let my_tcp_nat_type = NatType::try_from(
            self.peer_mgr
                .get_global_ctx()
                .get_stun_info_collector()
                .get_stun_info()
                .tcp_nat_type,
        )
        .unwrap_or(NatType::Unknown);
        tracing::debug!(?my_tcp_nat_type, "tcp hole punch rpc received");
        if matches!(my_tcp_nat_type, NatType::Unknown) {
            tracing::warn!(?my_tcp_nat_type, "tcp hole punch rpc rejected (unknown)");
            return Err(anyhow::anyhow!("tcp nat type unknown not supported").into());
        }

        let a_mapped_addr = input
            .connector_mapped_addr
            .ok_or(anyhow::anyhow!("connector_mapped_addr is required"))?;
        let a_mapped_addr: SocketAddr = a_mapped_addr.into();
        let a_ip = a_mapped_addr.ip();
        if a_ip.is_unspecified() || a_ip.is_multicast() {
            tracing::warn!(?a_mapped_addr, "tcp hole punch rpc invalid connector addr");
            return Err(anyhow::anyhow!("connector_mapped_addr is malformed").into());
        }

        let is_v6 = a_mapped_addr.is_ipv6();
        let local_port = select_local_port(&self.peer_mgr, is_v6).await?;
        let mapped_addr = self
            .peer_mgr
            .get_global_ctx()
            .get_stun_info_collector()
            .get_tcp_port_mapping(local_port)
            .await
            .with_context(|| "failed to get tcp port mapping")?;

        self.loop_addr_blacklist.cleanup();

        tracing::info!(
            ?a_mapped_addr,
            local_port,
            ?mapped_addr,
            "tcp hole punch rpc responding with listener mapped addr and start connecting"
        );

        let peer_mgr = self.peer_mgr.clone();
        let loop_addr_blacklist = self.loop_addr_blacklist.clone();
        self.tasks.lock().unwrap().spawn(async move {
            let _ = try_connect_to_remote(
                peer_mgr,
                a_mapped_addr,
                local_port,
                true,
                5,
                None,
                Some(loop_addr_blacklist),
            )
            .await;
        });

        Ok(TcpHolePunchResponse {
            listener_mapped_addr: Some(mapped_addr.into()),
        })
    }
}

struct TcpHolePunchConnectorData {
    peer_mgr: Arc<PeerManager>,
    blacklist: Arc<timedmap::TimedMap<PeerId, ()>>,
    loop_blacklist: Arc<timedmap::TimedMap<PeerId, ()>>,
}

impl TcpHolePunchConnectorData {
    fn new(peer_mgr: Arc<PeerManager>) -> Arc<Self> {
        Arc::new(Self {
            peer_mgr,
            blacklist: Arc::new(timedmap::TimedMap::new()),
            loop_blacklist: Arc::new(timedmap::TimedMap::new()),
        })
    }

    fn blacklist_loop_peer(&self, dst_peer_id: PeerId) {
        self.loop_blacklist.insert(
            dst_peer_id,
            (),
            Duration::from_secs(LOOP_BLACKLIST_TIMEOUT_SEC),
        );
    }

    async fn punch_as_initiator(self: Arc<Self>, dst_peer_id: PeerId) -> Result<(), Error> {
        let mut backoff = BackOff::new(vec![1000, 1000, 4000, 8000]);

        loop {
            if self.loop_blacklist.contains(&dst_peer_id) {
                break;
            }
            // Legacy fallback: see comment in try_connect_to_remote. This is
            // retained as a cold-path brake, not as part of the primary loop
            // avoidance strategy anymore.
            if self
                .peer_mgr
                .get_global_ctx()
                .is_protocol_loop_suppressed(IpScheme::Tcp, ProtocolLoopScope::HolePunch)
            {
                break;
            }
            backoff.sleep_for_next_backoff().await;
            if self.do_punch_as_initiator(dst_peer_id).await.is_ok() {
                break;
            }

            if self.blacklist.contains(&dst_peer_id) {
                tracing::warn!(
                    dst_peer_id,
                    "tcp hole punch initiator skipped (blacklisted)"
                );
                break;
            }
        }

        Ok(())
    }

    #[tracing::instrument(skip(self), fields(dst_peer_id), err)]
    async fn do_punch_as_initiator(&self, dst_peer_id: PeerId) -> Result<(), Error> {
        let global_ctx = self.peer_mgr.get_global_ctx();
        if self.loop_blacklist.contains(&dst_peer_id) {
            tracing::warn!(
                dst_peer_id,
                "tcp hole punch initiator skipped (loop-blacklisted)"
            );
            return Ok(());
        }
        // Legacy fallback: current hole-punch callers should never reach this
        // via a freshly-recorded global suppression bit, but keep the check for
        // compatibility with tests and any future external trigger.
        if global_ctx.is_protocol_loop_suppressed(IpScheme::Tcp, ProtocolLoopScope::HolePunch) {
            tracing::warn!("tcp hole punch initiator skipped (protocol loop suppressed)");
            return Ok(());
        }
        let my_tcp_nat_type = NatType::try_from(
            global_ctx
                .get_stun_info_collector()
                .get_stun_info()
                .tcp_nat_type,
        )
        .unwrap_or(NatType::Unknown);
        tracing::debug!(?my_tcp_nat_type, "tcp hole punch initiator start");
        if is_symmetric_tcp_nat(my_tcp_nat_type) || my_tcp_nat_type == NatType::Unknown {
            tracing::debug!("tcp hole punch initiator skipped (symmetric)");
            return Ok(());
        }

        let local_port = select_local_port(&self.peer_mgr, false).await?;
        let mapped_addr = global_ctx
            .get_stun_info_collector()
            .get_tcp_port_mapping(local_port)
            .await
            .with_context(|| "failed to get tcp port mapping")?;

        tracing::info!(
            dst_peer_id,
            local_port,
            ?mapped_addr,
            "tcp hole punch initiator got mapped addr, start rpc exchange"
        );

        let rpc_stub = self
            .peer_mgr
            .get_peer_rpc_mgr()
            .rpc_client()
            .scoped_client::<TcpHolePunchRpcClientFactory<BaseController>>(
                self.peer_mgr.my_peer_id(),
                dst_peer_id,
                global_ctx.get_network_name(),
            );

        let resp = rpc_stub
            .exchange_mapped_addr(
                BaseController {
                    timeout_ms: 6000,
                    ..Default::default()
                },
                TcpHolePunchRequest {
                    connector_mapped_addr: Some(mapped_addr.into()),
                },
            )
            .await;
        let resp = handle_rpc_result(resp, dst_peer_id, &self.blacklist)?;
        let remote_mapped_addr = resp
            .listener_mapped_addr
            .ok_or(anyhow::anyhow!("listener_mapped_addr is required"))?;
        let remote_mapped_addr: SocketAddr = remote_mapped_addr.into();
        tracing::info!(
            dst_peer_id,
            ?remote_mapped_addr,
            "tcp hole punch initiator rpc returned"
        );

        if let Ok(()) = try_connect_to_remote(
            self.peer_mgr.clone(),
            remote_mapped_addr,
            local_port,
            false,
            1,
            Some((dst_peer_id, self.loop_blacklist.clone())),
            None,
        )
        .await
        {
            tracing::info!(
                dst_peer_id,
                local_port,
                ?remote_mapped_addr,
                "tcp hole punch initiator connected to remote mapped addr with simultaneous connection"
            );
            return Ok(());
        }
        if self.loop_blacklist.contains(&dst_peer_id) {
            tracing::warn!(
                dst_peer_id,
                local_port,
                ?remote_mapped_addr,
                "tcp hole punch initiator aborted after self-loop signal"
            );
            return Ok(());
        }

        tracing::debug!(
            dst_peer_id,
            local_port,
            ?remote_mapped_addr,
            "tcp hole punch initiator sent syn to remote mapped addr"
        );

        let mut listener =
            TcpTunnelListener::new(format!("tcp://0.0.0.0:{}", local_port).parse().unwrap());
        {
            let _g = self.peer_mgr.get_global_ctx().net_ns.guard();
            listener.listen().await?;
        }
        tracing::info!(
            dst_peer_id,
            local_port,
            url = %listener.local_url(),
            "tcp hole punch initiator listening"
        );

        tokio::time::timeout(
            Duration::from_secs(10),
            self.accept_loop(&mut listener, dst_peer_id),
        )
        .await??;

        tracing::info!(
            dst_peer_id,
            "tcp hole punch initiator accepted and added server tunnel"
        );

        Ok(())
    }

    async fn accept_loop(
        &self,
        listener: &mut TcpTunnelListener,
        dst_peer_id: PeerId,
    ) -> Result<(), Error> {
        loop {
            match listener.accept().await {
                Ok(tunnel) => {
                    if let Err(e) = self
                        .peer_mgr
                        .add_tunnel_as_server_without_runtime_loop_record(tunnel, false)
                        .await
                    {
                        tracing::error!("tcp hole punch add tunnel error: {}", e);
                        if e.is_self_loop_signal() {
                            self.blacklist_loop_peer(dst_peer_id);
                            return Err(e.into());
                        }
                        if self.peer_mgr.get_global_ctx().is_protocol_loop_suppressed(
                            IpScheme::Tcp,
                            ProtocolLoopScope::HolePunch,
                        ) {
                            return Err(e.into());
                        }
                        continue;
                    }

                    tracing::info!(
                        dst_peer_id,
                        "tcp hole punch initiator accepted and added server tunnel"
                    );
                }
                Err(e) => {
                    tracing::error!("tcp hole punch accept error: {}", e);
                }
            }
        }
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
struct TcpPunchTaskInfo {
    dst_peer_id: PeerId,
}

#[derive(Clone)]
struct TcpHolePunchPeerTaskLauncher {}

#[async_trait::async_trait]
impl PeerTaskLauncher for TcpHolePunchPeerTaskLauncher {
    type Data = Arc<TcpHolePunchConnectorData>;
    type CollectPeerItem = TcpPunchTaskInfo;
    type TaskRet = ();

    fn new_data(&self, peer_mgr: Arc<PeerManager>) -> Self::Data {
        TcpHolePunchConnectorData::new(peer_mgr)
    }

    #[tracing::instrument(skip(self, data))]
    async fn collect_peers_need_task(&self, data: &Self::Data) -> Vec<Self::CollectPeerItem> {
        let global_ctx = data.peer_mgr.get_global_ctx();
        // Legacy fallback: production hole-punch no longer records this bit, so
        // this collector gate is effectively dead code in the current design.
        // Leave it in place as a defensive stop if a future path writes the
        // suppression slot again.
        if global_ctx.is_protocol_loop_suppressed(IpScheme::Tcp, ProtocolLoopScope::HolePunch) {
            tracing::warn!("tcp hole punch task collect skipped (protocol loop suppressed)");
            return vec![];
        }
        let flags = global_ctx.get_flags();
        let lazy_p2p = flags.lazy_p2p;
        let my_tcp_nat_type = NatType::try_from(
            global_ctx
                .get_stun_info_collector()
                .get_stun_info()
                .tcp_nat_type,
        )
        .unwrap_or(NatType::Unknown);
        if is_symmetric_tcp_nat(my_tcp_nat_type) || my_tcp_nat_type == NatType::Unknown {
            tracing::trace!(
                ?my_tcp_nat_type,
                "tcp hole punch task collect skipped (symmetric)"
            );
            return vec![];
        }

        let my_peer_id = data.peer_mgr.my_peer_id();
        let now = Instant::now();

        data.blacklist.cleanup();
        data.loop_blacklist.cleanup();

        let mut peers_to_connect = Vec::new();
        for route in data.peer_mgr.list_routes().await.iter() {
            let static_allowed = should_background_p2p_with_peer(
                route.feature_flag.as_ref(),
                false,
                lazy_p2p,
                flags.disable_p2p,
                flags.need_p2p,
            );
            let dynamic_allowed = should_try_p2p_with_peer(
                route.feature_flag.as_ref(),
                false,
                flags.disable_p2p,
                flags.need_p2p,
            ) && data.peer_mgr.has_recent_traffic(route.peer_id, now);
            let priority_upgrade_allowed = !flags.transport_priority.is_empty()
                && data.peer_mgr.get_peer_map().has_peer(route.peer_id)
                && should_try_p2p_with_peer(
                    route.feature_flag.as_ref(),
                    false,
                    flags.disable_p2p,
                    flags.need_p2p,
                );
            if !static_allowed && !dynamic_allowed && !priority_upgrade_allowed {
                continue;
            }

            let peer_id: PeerId = route.peer_id;
            if peer_id == my_peer_id {
                tracing::trace!(peer_id, "tcp hole punch task collect skip self");
                continue;
            }

            if data.blacklist.contains(&peer_id) {
                tracing::debug!(peer_id, "tcp hole punch task collect skip blacklisted");
                continue;
            }
            if data.loop_blacklist.contains(&peer_id) {
                tracing::debug!(peer_id, "tcp hole punch task collect skip loop-blacklisted");
                continue;
            }

            let has_peer = data.peer_mgr.get_peer_map().has_peer(peer_id);
            if has_peer && !flags.transport_priority.is_empty() {
                data.peer_mgr
                    .update_peer_transport_virtual_ip_from_route(route);
            }
            if !should_attempt_ranked_hole_punch(
                has_peer,
                !flags.transport_priority.is_empty(),
                data.peer_mgr.has_live_transport(peer_id, "tcp"),
                data.peer_mgr
                    .transport_candidate_improves(peer_id, TransportPathClass::Wan, "tcp"),
            ) {
                tracing::trace!(peer_id, "tcp hole punch task collect skip existing peer");
                continue;
            }

            let peer_tcp_nat_type = route
                .stun_info
                .as_ref()
                .map(|x| x.tcp_nat_type)
                .unwrap_or(0);
            let peer_tcp_nat_type =
                NatType::try_from(peer_tcp_nat_type).unwrap_or(NatType::Unknown);
            if matches!(peer_tcp_nat_type, NatType::Unknown) {
                tracing::debug!(
                    peer_id,
                    ?peer_tcp_nat_type,
                    "tcp hole punch task collect skip peer unknown"
                );
                continue;
            }

            tracing::info!(
                peer_id,
                my_peer_id,
                ?my_tcp_nat_type,
                ?peer_tcp_nat_type,
                "tcp hole punch task collect add peer"
            );
            peers_to_connect.push(TcpPunchTaskInfo {
                dst_peer_id: peer_id,
            });
        }

        peers_to_connect
    }

    async fn launch_task(
        &self,
        data: &Self::Data,
        item: Self::CollectPeerItem,
    ) -> tokio::task::JoinHandle<Result<Self::TaskRet, anyhow::Error>> {
        let data = data.clone();
        tokio::spawn(async move { data.punch_as_initiator(item.dst_peer_id).await.map(|_| ()) })
    }

    async fn all_task_done(&self, _data: &Self::Data) {}

    fn loop_interval_ms(&self) -> u64 {
        5000
    }
}

pub struct TcpHolePunchConnector {
    server: Arc<TcpHolePunchServer>,
    client: PeerTaskManager<TcpHolePunchPeerTaskLauncher>,
    peer_mgr: Arc<PeerManager>,
}

impl TcpHolePunchConnector {
    pub fn new(peer_mgr: Arc<PeerManager>) -> Self {
        Self {
            server: TcpHolePunchServer::new(peer_mgr.clone()),
            client: PeerTaskManager::new_with_external_signal(
                TcpHolePunchPeerTaskLauncher {},
                peer_mgr.clone(),
                Some(peer_mgr.p2p_demand_notify()),
            ),
            peer_mgr,
        }
    }

    pub async fn run_as_client(&mut self) -> Result<(), Error> {
        tracing::info!("tcp hole punch client start");
        self.client.start();
        Ok(())
    }

    pub async fn run_as_server(&mut self) -> Result<(), Error> {
        tracing::info!("tcp hole punch server register rpc");
        self.peer_mgr
            .get_peer_rpc_mgr()
            .rpc_server()
            .registry()
            .register(
                TcpHolePunchRpcServer::new_arc(self.server.clone()),
                &self.peer_mgr.get_global_ctx().get_network_name(),
            );
        Ok(())
    }

    pub async fn run(&mut self) -> Result<(), Error> {
        let flags = self.peer_mgr.get_global_ctx().get_flags();
        if flags.disable_tcp_hole_punching {
            tracing::debug!(
                "tcp hole punch disabled by disable_tcp_hole_punching(={});",
                flags.disable_tcp_hole_punching
            );
            return Ok(());
        }

        self.run_as_client().await?;
        self.run_as_server().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, sync::Arc, time::Duration};

    use crate::{
        common::{error::Error, stun::StunInfoCollectorTrait},
        connector::tcp_hole_punch::TcpHolePunchConnector,
        peers::{
            peer_manager::PeerManager,
            peer_task::PeerTaskLauncher,
            tests::{connect_peer_manager, create_mock_peer_manager, wait_route_appear},
        },
        proto::common::{NatType, StunInfo},
        tunnel::common::tests::wait_for_condition,
    };

    use super::TcpHolePunchPeerTaskLauncher;

    struct MockStunInfoCollector {
        udp_nat_type: NatType,
        tcp_nat_type: NatType,
    }

    #[async_trait::async_trait]
    impl StunInfoCollectorTrait for MockStunInfoCollector {
        fn get_stun_info(&self) -> StunInfo {
            StunInfo {
                udp_nat_type: self.udp_nat_type as i32,
                tcp_nat_type: self.tcp_nat_type as i32,
                last_update_time: 0,
                public_ip: vec!["127.0.0.1".to_string(), "::1".to_string()],
                min_port: 100,
                max_port: 200,
            }
        }

        async fn get_udp_port_mapping(&self, mut port: u16) -> Result<SocketAddr, Error> {
            if port == 0 {
                port = 40144;
            }
            Ok(format!("127.0.0.1:{}", port).parse().unwrap())
        }

        async fn get_udp_port_mapping_with_socket(
            &self,
            udp: std::sync::Arc<tokio::net::UdpSocket>,
        ) -> Result<SocketAddr, Error> {
            self.get_udp_port_mapping(udp.local_addr()?.port()).await
        }

        async fn get_tcp_port_mapping(&self, mut port: u16) -> Result<SocketAddr, Error> {
            if port == 0 {
                port = 40144;
            }
            Ok(format!("127.0.0.1:{}", port).parse().unwrap())
        }
    }

    fn replace_stun_info_collector(peer_mgr: Arc<PeerManager>, tcp_nat_type: NatType) {
        let collector = Box::new(MockStunInfoCollector {
            udp_nat_type: NatType::Unknown,
            tcp_nat_type,
        });
        peer_mgr
            .get_global_ctx()
            .replace_stun_info_collector(collector);
    }

    async fn collect_lazy_punch_peers(peer_mgr: Arc<PeerManager>) -> Vec<u32> {
        let launcher = TcpHolePunchPeerTaskLauncher {};
        let data = launcher.new_data(peer_mgr);
        launcher
            .collect_peers_need_task(&data)
            .await
            .into_iter()
            .map(|task| task.dst_peer_id)
            .collect()
    }

    #[tokio::test]
    async fn tcp_hole_punch_connects() {
        let p_a = create_mock_peer_manager().await;
        let p_b = create_mock_peer_manager().await;
        let p_c = create_mock_peer_manager().await;

        replace_stun_info_collector(p_a.clone(), NatType::PortRestricted);
        replace_stun_info_collector(p_b.clone(), NatType::PortRestricted);
        replace_stun_info_collector(p_c.clone(), NatType::PortRestricted);

        connect_peer_manager(p_a.clone(), p_b.clone()).await;
        connect_peer_manager(p_b.clone(), p_c.clone()).await;
        wait_route_appear(p_a.clone(), p_c.clone()).await.unwrap();

        let mut hole_punching_a = TcpHolePunchConnector::new(p_a.clone());
        let mut hole_punching_c = TcpHolePunchConnector::new(p_c.clone());
        hole_punching_a.run().await.unwrap();
        hole_punching_c.run().await.unwrap();

        hole_punching_a.client.run_immediately().await;
        hole_punching_c.client.run_immediately().await;

        wait_for_condition(
            || {
                let p_a = p_a.clone();
                let p_c = p_c.clone();
                async move {
                    let a_has = p_a
                        .get_peer_map()
                        .list_peer_conns(p_c.my_peer_id())
                        .await
                        .is_some_and(|c| !c.is_empty());
                    let c_has = p_c
                        .get_peer_map()
                        .list_peer_conns(p_a.my_peer_id())
                        .await
                        .is_some_and(|c| !c.is_empty());
                    a_has || c_has
                }
            },
            Duration::from_secs(15),
        )
        .await;
    }

    #[tokio::test]
    async fn tcp_hole_punch_skip_symmetric_peer() {
        let p_a = create_mock_peer_manager().await;
        let p_b = create_mock_peer_manager().await;
        let p_c = create_mock_peer_manager().await;

        replace_stun_info_collector(p_a.clone(), NatType::Symmetric);
        replace_stun_info_collector(p_b.clone(), NatType::PortRestricted);
        replace_stun_info_collector(p_c.clone(), NatType::Symmetric);

        connect_peer_manager(p_a.clone(), p_b.clone()).await;
        connect_peer_manager(p_b.clone(), p_c.clone()).await;
        wait_route_appear(p_a.clone(), p_c.clone()).await.unwrap();

        let mut hole_punching_a = TcpHolePunchConnector::new(p_a.clone());
        let mut hole_punching_c = TcpHolePunchConnector::new(p_c.clone());
        hole_punching_a.run().await.unwrap();
        hole_punching_c.run().await.unwrap();

        hole_punching_a.client.run_immediately().await;
        hole_punching_c.client.run_immediately().await;

        tokio::time::sleep(Duration::from_secs(2)).await;

        assert!(
            p_a.get_peer_map()
                .list_peer_conns(p_c.my_peer_id())
                .await
                .map(|c| c.is_empty())
                .unwrap_or(true)
        );
        assert!(
            p_c.get_peer_map()
                .list_peer_conns(p_a.my_peer_id())
                .await
                .map(|c| c.is_empty())
                .unwrap_or(true)
        );
    }

    #[tokio::test]
    async fn lazy_p2p_collects_tcp_hole_punch_tasks_only_after_recent_traffic() {
        let p_a = create_mock_peer_manager().await;
        let p_b = create_mock_peer_manager().await;
        let p_c = create_mock_peer_manager().await;

        replace_stun_info_collector(p_a.clone(), NatType::PortRestricted);
        replace_stun_info_collector(p_b.clone(), NatType::PortRestricted);
        replace_stun_info_collector(p_c.clone(), NatType::PortRestricted);

        let mut flags = p_a.get_global_ctx().get_flags();
        flags.lazy_p2p = true;
        p_a.get_global_ctx().set_flags(flags);

        connect_peer_manager(p_a.clone(), p_b.clone()).await;
        connect_peer_manager(p_b.clone(), p_c.clone()).await;
        wait_route_appear(p_a.clone(), p_c.clone()).await.unwrap();

        assert!(
            !collect_lazy_punch_peers(p_a.clone())
                .await
                .contains(&p_c.my_peer_id())
        );

        p_a.mark_recent_traffic(p_c.my_peer_id());

        assert!(
            collect_lazy_punch_peers(p_a.clone())
                .await
                .contains(&p_c.my_peer_id())
        );
    }

    #[tokio::test]
    async fn tcp_hole_punch_collect_skips_loop_blacklisted_peer() {
        let p_a = create_mock_peer_manager().await;
        let p_b = create_mock_peer_manager().await;
        let p_c = create_mock_peer_manager().await;

        replace_stun_info_collector(p_a.clone(), NatType::PortRestricted);
        replace_stun_info_collector(p_b.clone(), NatType::PortRestricted);
        replace_stun_info_collector(p_c.clone(), NatType::PortRestricted);

        connect_peer_manager(p_a.clone(), p_b.clone()).await;
        connect_peer_manager(p_b.clone(), p_c.clone()).await;
        wait_route_appear(p_a.clone(), p_c.clone()).await.unwrap();

        let launcher = TcpHolePunchPeerTaskLauncher {};
        let data = launcher.new_data(p_a.clone());
        data.loop_blacklist.insert(
            p_c.my_peer_id(),
            (),
            Duration::from_secs(super::LOOP_BLACKLIST_TIMEOUT_SEC),
        );

        let tasks = launcher.collect_peers_need_task(&data).await;
        assert!(
            tasks
                .into_iter()
                .all(|task| task.dst_peer_id != p_c.my_peer_id())
        );
    }
}
