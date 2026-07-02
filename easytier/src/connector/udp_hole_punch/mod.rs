use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use anyhow::{Context, Error};
use both_easy_sym::{PunchBothEasySymHoleClient, PunchBothEasySymHoleServer};
use common::{PunchHoleServerCommon, UdpNatType, UdpPunchClientMethod};
use cone::{PunchConeHoleClient, PunchConeHoleServer};
use dashmap::DashMap;
use hotpath::instant::Instant;
use once_cell::sync::Lazy;
use sym_to_cone::{PunchSymToConeHoleClient, PunchSymToConeHoleServer};
use tokio::{sync::Mutex, task::JoinHandle};

use crate::{
    common::{
        PeerId, global_ctx::ProtocolLoopScope, stun::StunInfoCollectorTrait,
        transport_priority::TransportPathClass,
    },
    peers::{
        peer_manager::PeerManager,
        peer_task::{PeerTaskLauncher, PeerTaskManager},
    },
    proto::{
        common::{NatType, Void},
        peer_rpc::{
            SelectPunchListenerRequest, SelectPunchListenerResponse,
            SendPunchPacketBothEasySymRequest, SendPunchPacketBothEasySymResponse,
            SendPunchPacketConeRequest, SendPunchPacketEasySymRequest,
            SendPunchPacketHardSymRequest, SendPunchPacketHardSymResponse, UdpHolePunchRpc,
            UdpHolePunchRpcServer,
        },
        rpc_types::{self, controller::BaseController},
    },
    tunnel::{IpScheme, Tunnel},
};

use crate::connector::{
    should_attempt_ranked_hole_punch, should_background_p2p_with_peer,
    should_downgrade_udp_stealth, should_try_p2p_with_peer,
};

pub(crate) mod both_easy_sym;
pub(crate) mod common;
pub(crate) mod cone;
pub(crate) mod sym_to_cone;

// sym punch should be serialized
static SYM_PUNCH_LOCK: Lazy<DashMap<PeerId, Arc<Mutex<()>>>> = Lazy::new(DashMap::new);
pub static RUN_TESTING: Lazy<AtomicBool> = Lazy::new(|| AtomicBool::new(false));

// Blacklist timeout in seconds
pub const BLACKLIST_TIMEOUT_SEC: u64 = 3600;
pub const LOOP_BLACKLIST_TIMEOUT_SEC: u64 = 300;

fn get_sym_punch_lock(peer_id: PeerId) -> Arc<Mutex<()>> {
    SYM_PUNCH_LOCK
        .entry(peer_id)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .value()
        .clone()
}

struct UdpHolePunchServer {
    common: Arc<PunchHoleServerCommon>,
    cone_server: PunchConeHoleServer,
    sym_to_cone_server: PunchSymToConeHoleServer,
    both_easy_sym_server: PunchBothEasySymHoleServer,
}

impl UdpHolePunchServer {
    pub fn new(peer_mgr: Arc<PeerManager>) -> Arc<Self> {
        let common = Arc::new(PunchHoleServerCommon::new(peer_mgr));
        let cone_server = PunchConeHoleServer::new(common.clone());
        let sym_to_cone_server = PunchSymToConeHoleServer::new(common.clone());
        let both_easy_sym_server = PunchBothEasySymHoleServer::new(common.clone());

        Arc::new(Self {
            common,
            cone_server,
            sym_to_cone_server,
            both_easy_sym_server,
        })
    }
}

#[async_trait::async_trait]
impl UdpHolePunchRpc for UdpHolePunchServer {
    type Controller = BaseController;

    async fn select_punch_listener(
        &self,
        _ctrl: Self::Controller,
        input: SelectPunchListenerRequest,
    ) -> rpc_types::error::Result<SelectPunchListenerResponse> {
        if common::legacy_udp_hole_punch_is_rejected(
            input.use_stealth,
            self.common
                .get_global_ctx()
                .get_flags()
                .disable_legacy_udp_hole_punch,
        ) {
            return Err(anyhow::anyhow!("legacy UDP hole punch is disabled").into());
        }
        let stealth_enabled = common::negotiate_udp_listener_stealth(
            input.use_stealth,
            self.common
                .get_global_ctx()
                .get_feature_flags()
                .stealth_supported,
        );
        let (_, addr) = self
            .common
            .select_listener(input.force_new, input.prefer_port_mapping, stealth_enabled)
            .await
            .ok_or(anyhow::anyhow!("no listener available"))?;

        Ok(SelectPunchListenerResponse {
            listener_mapped_addr: Some(addr.into()),
            stealth_enabled: Some(stealth_enabled),
        })
    }

    /// send packet to one remote_addr, used by nat1-3 to nat1-3
    async fn send_punch_packet_cone(
        &self,
        ctrl: Self::Controller,
        input: SendPunchPacketConeRequest,
    ) -> rpc_types::error::Result<Void> {
        self.cone_server.send_punch_packet_cone(ctrl, input).await
    }

    /// send packet to multiple remote_addr (birthday attack), used by nat4 to nat1-3
    async fn send_punch_packet_hard_sym(
        &self,
        _ctrl: Self::Controller,
        input: SendPunchPacketHardSymRequest,
    ) -> rpc_types::error::Result<SendPunchPacketHardSymResponse> {
        let _locked = get_sym_punch_lock(self.common.get_peer_mgr().my_peer_id())
            .try_lock_owned()
            .with_context(|| "sym punch lock is busy")?;
        self.sym_to_cone_server
            .send_punch_packet_hard_sym(input)
            .await
    }

    async fn send_punch_packet_easy_sym(
        &self,
        _ctrl: Self::Controller,
        input: SendPunchPacketEasySymRequest,
    ) -> rpc_types::error::Result<Void> {
        let _locked = get_sym_punch_lock(self.common.get_peer_mgr().my_peer_id())
            .try_lock_owned()
            .with_context(|| "sym punch lock is busy")?;
        self.sym_to_cone_server
            .send_punch_packet_easy_sym(input)
            .await
            .map(|_| Void {})
    }

    /// nat4 to nat4 (both predictably)
    async fn send_punch_packet_both_easy_sym(
        &self,
        _ctrl: Self::Controller,
        input: SendPunchPacketBothEasySymRequest,
    ) -> rpc_types::error::Result<SendPunchPacketBothEasySymResponse> {
        let _locked = get_sym_punch_lock(self.common.get_peer_mgr().my_peer_id())
            .try_lock_owned()
            .with_context(|| "sym punch lock is busy")?;
        self.both_easy_sym_server
            .send_punch_packet_both_easy_sym(input)
            .await
    }
}

#[derive(Debug)]
pub struct BackOff {
    backoffs_ms: Vec<u64>,
    current_idx: usize,
}

impl BackOff {
    pub fn new(backoffs_ms: Vec<u64>) -> Self {
        Self {
            backoffs_ms,
            current_idx: 0,
        }
    }

    pub fn next_backoff(&mut self) -> u64 {
        let backoff = self.backoffs_ms[self.current_idx];
        self.current_idx = (self.current_idx + 1).min(self.backoffs_ms.len() - 1);
        backoff
    }

    pub fn rollback(&mut self) {
        self.current_idx = self.current_idx.saturating_sub(1);
    }

    pub async fn sleep_for_next_backoff(&mut self) {
        let backoff = self.next_backoff();
        if backoff > 0 {
            tokio::time::sleep(tokio::time::Duration::from_millis(backoff)).await;
        }
    }
}

pub fn handle_rpc_result<T>(
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

struct UdpHoePunchConnectorData {
    cone_client: PunchConeHoleClient,
    sym_to_cone_client: PunchSymToConeHoleClient,
    both_easy_sym_client: PunchBothEasySymHoleClient,
    peer_mgr: Arc<PeerManager>,
    blacklist: Arc<timedmap::TimedMap<PeerId, ()>>,
    loop_blacklist: Arc<timedmap::TimedMap<PeerId, ()>>,
}

impl UdpHoePunchConnectorData {
    pub fn new(peer_mgr: Arc<PeerManager>) -> Arc<Self> {
        let blacklist = Arc::new(timedmap::TimedMap::new());
        let loop_blacklist = Arc::new(timedmap::TimedMap::new());
        let cone_client = PunchConeHoleClient::new(peer_mgr.clone(), blacklist.clone());
        let sym_to_cone_client = PunchSymToConeHoleClient::new(peer_mgr.clone(), blacklist.clone());
        let both_easy_sym_client =
            PunchBothEasySymHoleClient::new(peer_mgr.clone(), blacklist.clone());

        Arc::new(Self {
            cone_client,
            sym_to_cone_client,
            both_easy_sym_client,
            peer_mgr,
            blacklist,
            loop_blacklist,
        })
    }

    fn blacklist_loop_peer(&self, dst_peer_id: PeerId) {
        self.loop_blacklist.insert(
            dst_peer_id,
            (),
            Duration::from_secs(LOOP_BLACKLIST_TIMEOUT_SEC),
        );
    }

    #[tracing::instrument(skip(self))]
    async fn handle_punch_result(
        &self,
        dst_peer_id: PeerId,
        ret: Result<Option<Box<dyn Tunnel>>, Error>,
        backoff: Option<&mut BackOff>,
        round: Option<&mut u32>,
    ) -> bool {
        // Legacy fallback: production hole-punch paths no longer record
        // ProtocolLoopScope::HolePunch into GlobalCtx, so this branch is
        // effectively dead in normal runtime. Keep it as a safety rail for
        // tests or future callers that might still inject the global flag.
        if self
            .peer_mgr
            .get_global_ctx()
            .is_protocol_loop_suppressed(IpScheme::Udp, ProtocolLoopScope::HolePunch)
        {
            return true;
        }
        let op = |rollback: bool| {
            if rollback {
                if let Some(backoff) = backoff {
                    backoff.rollback();
                }
                if let Some(round) = round {
                    *round = round.saturating_sub(1);
                }
            } else if let Some(round) = round {
                *round += 1;
            }
        };

        match ret {
            Ok(Some(tunnel)) => {
                tracing::info!(?tunnel, "hole punching get tunnel success");

                if let Err(e) = self
                    .peer_mgr
                    .add_client_tunnel_without_runtime_loop_record(tunnel, false)
                    .await
                {
                    tracing::warn!("add client tunnel failed, err: {}", e);
                    if e.is_self_loop_signal() {
                        self.blacklist_loop_peer(dst_peer_id);
                        return true;
                    }
                    if self
                        .peer_mgr
                        .get_global_ctx()
                        .is_protocol_loop_suppressed(IpScheme::Udp, ProtocolLoopScope::HolePunch)
                    {
                        return true;
                    }
                    op(true);
                    false
                } else {
                    true
                }
            }
            Ok(None) => {
                tracing::info!("hole punching failed, no punch tunnel");
                op(false);
                false
            }
            Err(e) => {
                tracing::info!("hole punching failed, err: {}", e);
                op(true);
                false
            }
        }
    }

    #[tracing::instrument(skip(self))]
    async fn cone_to_cone(self: Arc<Self>, task_info: PunchTaskInfo) -> Result<(), Error> {
        let mut backoff = BackOff::new(vec![1000, 1000, 2000, 4000, 4000, 8000, 8000, 16000]);

        loop {
            if self.loop_blacklist.contains(&task_info.dst_peer_id) {
                break;
            }
            backoff.sleep_for_next_backoff().await;

            let ret = self
                .cone_client
                .do_hole_punching(task_info.dst_peer_id, task_info.disable_udp_stealth)
                .await;

            if self
                .handle_punch_result(task_info.dst_peer_id, ret, Some(&mut backoff), None)
                .await
            {
                break;
            }
        }

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn sym_to_cone(self: Arc<Self>, task_info: PunchTaskInfo) -> Result<(), Error> {
        let mut backoff =
            BackOff::new(vec![1000, 1000, 2000, 4000, 4000, 8000, 8000, 16000, 64000]);
        let mut round = 0;
        let mut port_idx = rand::random();

        loop {
            if self.loop_blacklist.contains(&task_info.dst_peer_id) {
                break;
            }
            backoff.sleep_for_next_backoff().await;

            // always try cone first
            if !RUN_TESTING.load(std::sync::atomic::Ordering::Relaxed) {
                let ret = self
                    .cone_client
                    .do_hole_punching(task_info.dst_peer_id, task_info.disable_udp_stealth)
                    .await;
                if self
                    .handle_punch_result(task_info.dst_peer_id, ret, None, None)
                    .await
                {
                    break;
                }
            }

            let ret = {
                let _lock = get_sym_punch_lock(self.peer_mgr.my_peer_id())
                    .lock_owned()
                    .await;
                self.sym_to_cone_client
                    .do_hole_punching(
                        task_info.dst_peer_id,
                        round,
                        &mut port_idx,
                        task_info.my_nat_type,
                        task_info.disable_udp_stealth,
                    )
                    .await
            };

            if self
                .handle_punch_result(
                    task_info.dst_peer_id,
                    ret,
                    Some(&mut backoff),
                    Some(&mut round),
                )
                .await
            {
                break;
            }
        }

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn both_easy_sym(self: Arc<Self>, task_info: PunchTaskInfo) -> Result<(), Error> {
        let mut backoff =
            BackOff::new(vec![1000, 1000, 2000, 4000, 4000, 8000, 8000, 16000, 64000]);

        loop {
            if self.loop_blacklist.contains(&task_info.dst_peer_id) {
                break;
            }
            backoff.sleep_for_next_backoff().await;

            // always try cone first
            if !RUN_TESTING.load(std::sync::atomic::Ordering::Relaxed) {
                let ret = self
                    .cone_client
                    .do_hole_punching(task_info.dst_peer_id, task_info.disable_udp_stealth)
                    .await;
                if self
                    .handle_punch_result(task_info.dst_peer_id, ret, None, None)
                    .await
                {
                    break;
                }
            }

            let mut is_busy = false;

            let ret = {
                let _lock = get_sym_punch_lock(self.peer_mgr.my_peer_id())
                    .lock_owned()
                    .await;
                self.both_easy_sym_client
                    .do_hole_punching(
                        task_info.dst_peer_id,
                        task_info.my_nat_type,
                        task_info.dst_nat_type,
                        task_info.disable_udp_stealth,
                        &mut is_busy,
                    )
                    .await
            };

            if is_busy {
                backoff.rollback();
            } else if self
                .handle_punch_result(task_info.dst_peer_id, ret, Some(&mut backoff), None)
                .await
            {
                break;
            }
        }

        Ok(())
    }
}

#[derive(Clone)]
struct UdpHolePunchPeerTaskLauncher {}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
struct PunchTaskInfo {
    dst_peer_id: PeerId,
    dst_nat_type: UdpNatType,
    my_nat_type: UdpNatType,
    disable_udp_stealth: bool,
}

#[async_trait::async_trait]
impl PeerTaskLauncher for UdpHolePunchPeerTaskLauncher {
    type Data = Arc<UdpHoePunchConnectorData>;
    type CollectPeerItem = PunchTaskInfo;
    type TaskRet = ();

    fn new_data(&self, peer_mgr: Arc<PeerManager>) -> Self::Data {
        UdpHoePunchConnectorData::new(peer_mgr)
    }

    async fn collect_peers_need_task(&self, data: &Self::Data) -> Vec<Self::CollectPeerItem> {
        // Legacy fallback: current hole-punch entry points use the no-record
        // PeerManager helpers, so this global guard should stay cold in
        // production. It is intentionally retained to preserve a hard stop if a
        // future path reintroduces GlobalCtx-level hole-punch suppression.
        if data
            .peer_mgr
            .get_global_ctx()
            .is_protocol_loop_suppressed(IpScheme::Udp, ProtocolLoopScope::HolePunch)
        {
            tracing::warn!("udp hole punch task collect skipped (protocol loop suppressed)");
            return Vec::new();
        }

        let my_nat_type = data
            .peer_mgr
            .get_global_ctx()
            .get_stun_info_collector()
            .get_stun_info()
            .udp_nat_type;
        let my_nat_type: UdpNatType = NatType::try_from(my_nat_type)
            .unwrap_or(NatType::Unknown)
            .into();
        if !my_nat_type.is_sym() {
            data.sym_to_cone_client.clear_udp_array().await;
        }

        let mut peers_to_connect: Vec<Self::CollectPeerItem> = Vec::new();
        // do not do anything if:
        // 1. our nat type is OpenInternet or NoPat, which means we can wait other peers to connect us
        // notice that if we are unknown, we treat ourselves as cone
        if my_nat_type.is_open() {
            return peers_to_connect;
        }

        let my_peer_id = data.peer_mgr.my_peer_id();
        let flags = data.peer_mgr.get_global_ctx().get_flags();
        let lazy_p2p = flags.lazy_p2p;
        let now = Instant::now();

        data.blacklist.cleanup();
        data.loop_blacklist.cleanup();

        // collect peer list from peer manager and do some filter:
        // 1. peers without direct conns;
        // 2. peers is full cone (any restricted type);
        // 3. peers not in blacklist;
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

            let peer_nat_type = route
                .stun_info
                .as_ref()
                .map(|x| x.udp_nat_type)
                .unwrap_or(0);
            let Ok(peer_nat_type) = NatType::try_from(peer_nat_type) else {
                continue;
            };
            let peer_nat_type = peer_nat_type.into();

            let peer_id: PeerId = route.peer_id;

            // Check if peer is blacklisted
            if data.blacklist.contains(&peer_id) {
                tracing::debug!(?peer_id, "peer is blacklisted, skipping");
                continue;
            }
            if data.loop_blacklist.contains(&peer_id) {
                tracing::debug!(?peer_id, "peer is loop-blacklisted, skipping");
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
                data.peer_mgr.has_live_transport(peer_id, "udp"),
                data.peer_mgr
                    .transport_candidate_improves(peer_id, TransportPathClass::Wan, "udp"),
            ) {
                continue;
            }

            let global_ctx = data.peer_mgr.get_global_ctx();
            if !my_nat_type.can_punch_hole_as_client(peer_nat_type, my_peer_id, peer_id, global_ctx)
            {
                continue;
            }

            tracing::info!(
                ?peer_id,
                ?peer_nat_type,
                ?my_nat_type,
                "found peer to do hole punching"
            );

            peers_to_connect.push(PunchTaskInfo {
                dst_peer_id: peer_id,
                dst_nat_type: peer_nat_type,
                my_nat_type,
                disable_udp_stealth: should_downgrade_udp_stealth(
                    crate::common::stealth_registry::protocol_enabled(
                        &flags,
                        crate::common::stealth_registry::StealthProtocol::Udp,
                    ),
                    route.feature_flag.as_ref(),
                ),
            });
        }

        peers_to_connect
    }

    async fn launch_task(
        &self,
        data: &Self::Data,
        item: Self::CollectPeerItem,
    ) -> JoinHandle<Result<Self::TaskRet, Error>> {
        let data = data.clone();
        let global_ctx = data.peer_mgr.get_global_ctx();
        let punch_method = item
            .my_nat_type
            .get_punch_hole_method(item.dst_nat_type, global_ctx);
        match punch_method {
            UdpPunchClientMethod::ConeToCone => tokio::spawn(data.cone_to_cone(item)),
            UdpPunchClientMethod::SymToCone => tokio::spawn(data.sym_to_cone(item)),
            UdpPunchClientMethod::EasySymToEasySym => tokio::spawn(data.both_easy_sym(item)),
            _ => unreachable!(),
        }
    }

    async fn all_task_done(&self, data: &Self::Data) {
        data.sym_to_cone_client.clear_udp_array().await;
    }

    fn loop_interval_ms(&self) -> u64 {
        5000
    }
}

pub struct UdpHolePunchConnector {
    server: Arc<UdpHolePunchServer>,
    client: PeerTaskManager<UdpHolePunchPeerTaskLauncher>,
    peer_mgr: Arc<PeerManager>,
}

// Currently support:
// Symmetric -> Full Cone
// Any Type of Full Cone -> Any Type of Full Cone

// if same level of full cone, node with smaller peer_id will be the initiator
// if different level of full cone, node with more strict level will be the initiator

impl UdpHolePunchConnector {
    pub fn new(peer_mgr: Arc<PeerManager>) -> Self {
        Self {
            server: UdpHolePunchServer::new(peer_mgr.clone()),
            client: PeerTaskManager::new_with_external_signal(
                UdpHolePunchPeerTaskLauncher {},
                peer_mgr.clone(),
                Some(peer_mgr.p2p_demand_notify()),
            ),
            peer_mgr,
        }
    }

    pub async fn run_as_client(&mut self) -> Result<(), Error> {
        self.client.start();
        Ok(())
    }

    pub async fn run_as_server(&mut self) -> Result<(), Error> {
        self.peer_mgr
            .get_peer_rpc_mgr()
            .rpc_server()
            .registry()
            .register(
                UdpHolePunchRpcServer::new(Arc::downgrade(&self.server)),
                &self.peer_mgr.get_global_ctx().get_network_name(),
            );

        Ok(())
    }

    pub async fn run(&mut self) -> Result<(), Error> {
        let global_ctx = self.peer_mgr.get_global_ctx();

        if global_ctx.get_flags().disable_udp_hole_punching {
            return Ok(());
        }

        self.run_as_client().await?;
        self.run_as_server().await?;

        Ok(())
    }

    #[cfg(test)]
    pub async fn run_immediately_for_test(&self) {
        self.client.run_immediately().await;
    }
}

#[cfg(test)]
pub mod tests {

    use std::sync::Arc;
    use std::time::Duration;

    use crate::common::stun::MockStunInfoCollector;
    use crate::peers::{
        peer_manager::PeerManager,
        peer_task::PeerTaskLauncher,
        tests::{connect_peer_manager, create_mock_peer_manager, wait_route_appear},
    };
    use crate::proto::common::NatType;
    use crate::tunnel::common::tests::wait_for_condition;

    use super::{RUN_TESTING, UdpHolePunchConnector, UdpHolePunchPeerTaskLauncher};

    pub fn replace_stun_info_collector(peer_mgr: Arc<PeerManager>, udp_nat_type: NatType) {
        let collector = Box::new(MockStunInfoCollector { udp_nat_type });
        peer_mgr
            .get_global_ctx()
            .replace_stun_info_collector(collector);
    }

    pub async fn create_mock_peer_manager_with_mock_stun(
        udp_nat_type: NatType,
    ) -> Arc<PeerManager> {
        let p_a = create_mock_peer_manager().await;
        let mut flags = p_a.get_global_ctx().get_flags();
        flags.disable_upnp = true;
        p_a.get_global_ctx().set_flags(flags);
        replace_stun_info_collector(p_a.clone(), udp_nat_type);
        p_a
    }

    async fn collect_lazy_punch_peers(peer_mgr: Arc<PeerManager>) -> Vec<u32> {
        let launcher = UdpHolePunchPeerTaskLauncher {};
        let data = launcher.new_data(peer_mgr);
        launcher
            .collect_peers_need_task(&data)
            .await
            .into_iter()
            .map(|task| task.dst_peer_id)
            .collect()
    }

    async fn collect_punch_tasks(peer_mgr: Arc<PeerManager>) -> Vec<super::PunchTaskInfo> {
        let launcher = UdpHolePunchPeerTaskLauncher {};
        let data = launcher.new_data(peer_mgr);
        launcher.collect_peers_need_task(&data).await
    }

    #[rstest::rstest]
    #[tokio::test]
    pub async fn test_hole_punching_blacklist(
        #[values(NatType::Symmetric, NatType::PortRestricted, NatType::Unknown)] nat_type: NatType,
    ) {
        RUN_TESTING.store(true, std::sync::atomic::Ordering::Relaxed);

        let p_a = create_mock_peer_manager_with_mock_stun(nat_type).await;
        let p_b = create_mock_peer_manager_with_mock_stun(NatType::PortRestricted).await;
        let p_c = create_mock_peer_manager_with_mock_stun(NatType::PortRestricted).await;
        connect_peer_manager(p_a.clone(), p_b.clone()).await;
        connect_peer_manager(p_b.clone(), p_c.clone()).await;
        wait_route_appear(p_a.clone(), p_c.clone()).await.unwrap();

        let mut hole_punching_a = UdpHolePunchConnector::new(p_a.clone());

        hole_punching_a.run().await.unwrap();

        hole_punching_a.client.run_immediately().await;

        wait_for_condition(
            || async {
                hole_punching_a
                    .client
                    .data()
                    .blacklist
                    .contains(&p_c.my_peer_id())
            },
            Duration::from_secs(10),
        )
        .await;
    }

    #[tokio::test]
    async fn lazy_p2p_collects_udp_hole_punch_tasks_only_after_recent_traffic() {
        let p_a = create_mock_peer_manager_with_mock_stun(NatType::PortRestricted).await;
        let p_b = create_mock_peer_manager_with_mock_stun(NatType::PortRestricted).await;
        let p_c = create_mock_peer_manager_with_mock_stun(NatType::PortRestricted).await;

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
    async fn udp_hole_punch_collect_marks_legacy_peer_for_stealth_downgrade() {
        let p_a = create_mock_peer_manager_with_mock_stun(NatType::PortRestricted).await;
        let p_b = create_mock_peer_manager_with_mock_stun(NatType::PortRestricted).await;
        let p_c = create_mock_peer_manager_with_mock_stun(NatType::PortRestricted).await;

        let mut flags = p_a.get_global_ctx().get_flags();
        flags.stealth_mode = true;
        p_a.get_global_ctx().set_flags(flags);

        connect_peer_manager(p_a.clone(), p_b.clone()).await;
        connect_peer_manager(p_b.clone(), p_c.clone()).await;
        wait_route_appear(p_a.clone(), p_c.clone()).await.unwrap();

        let tasks = collect_punch_tasks(p_a.clone()).await;
        let task = tasks
            .into_iter()
            .find(|task| task.dst_peer_id == p_c.my_peer_id())
            .expect("expected punch task for peer");
        assert!(task.disable_udp_stealth);
    }

    #[tokio::test]
    async fn udp_hole_punch_collect_skips_loop_blacklisted_peer() {
        let p_a = create_mock_peer_manager_with_mock_stun(NatType::PortRestricted).await;
        let p_b = create_mock_peer_manager_with_mock_stun(NatType::PortRestricted).await;
        let p_c = create_mock_peer_manager_with_mock_stun(NatType::PortRestricted).await;

        connect_peer_manager(p_a.clone(), p_b.clone()).await;
        connect_peer_manager(p_b.clone(), p_c.clone()).await;
        wait_route_appear(p_a.clone(), p_c.clone()).await.unwrap();

        let launcher = UdpHolePunchPeerTaskLauncher {};
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
