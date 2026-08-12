use std::{
    collections::BTreeSet,
    future::Future,
    sync::{Arc, Weak},
    time::Duration,
};

use dashmap::DashSet;
use hotpath::instant::Instant;
use tokio::{sync::mpsc, task::JoinSet, time::timeout};

use crate::{
    common::{PeerId, dns::socket_addrs, join_joinset_background},
    peers::peer_conn::PeerConnId,
    proto::{
        api::instance::{
            Connector, ConnectorManageRpc, ConnectorStatus, ListConnectorRequest,
            ListConnectorResponse,
        },
        rpc_types::{self, controller::BaseController},
    },
    tunnel::{IpVersion, TunnelConnector, TunnelScheme, matches_scheme},
    utils::weak_upgrade,
};

use crate::{
    common::{
        error::Error,
        global_ctx::{ArcGlobalCtx, GlobalCtxEvent},
        netns::NetNS,
    },
    peers::peer_manager::PeerManager,
    use_global_var,
};

use super::create_connector_by_url;

type ConnectorMap = Arc<DashSet<url::Url>>;

#[derive(Debug, Clone)]
struct ReconnResult {
    dead_url: String,
    peer_id: PeerId,
    conn_id: PeerConnId,
}

struct ConnectorManagerData {
    connectors: ConnectorMap,
    reconnecting: DashSet<url::Url>,
    runtime_bootstrap_helpers: DashSet<url::Url>,
    connector_state_lock: std::sync::Mutex<()>,
    peer_manager: Weak<PeerManager>,
    alive_conn_urls: Arc<DashSet<url::Url>>,
    // user removed connector urls
    removed_conn_urls: Arc<DashSet<url::Url>>,
    net_ns: NetNS,
    global_ctx: ArcGlobalCtx,
}

pub struct ManualConnectorManager {
    global_ctx: ArcGlobalCtx,
    data: Arc<ConnectorManagerData>,
    tasks: JoinSet<()>,
}

impl ManualConnectorManager {
    pub fn new(global_ctx: ArcGlobalCtx, peer_manager: Arc<PeerManager>) -> Self {
        let connectors = Arc::new(DashSet::new());
        let tasks = JoinSet::new();

        let mut ret = Self {
            global_ctx: global_ctx.clone(),
            data: Arc::new(ConnectorManagerData {
                connectors,
                reconnecting: DashSet::new(),
                runtime_bootstrap_helpers: DashSet::new(),
                connector_state_lock: std::sync::Mutex::new(()),
                peer_manager: Arc::downgrade(&peer_manager),
                alive_conn_urls: Arc::new(DashSet::new()),
                removed_conn_urls: Arc::new(DashSet::new()),
                net_ns: global_ctx.net_ns.clone(),
                global_ctx,
            }),
            tasks,
        };

        ret.tasks
            .spawn(Self::conn_mgr_reconn_routine(ret.data.clone()));

        ret
    }

    fn reconnect_timeout(dead_url: &url::Url, udp_stealth_enabled: bool) -> Duration {
        let use_long_timeout = matches_scheme!(
            dead_url,
            TunnelScheme::Http | TunnelScheme::Https | TunnelScheme::Txt | TunnelScheme::Srv
        ) || matches!(dead_url.scheme(), "ws" | "wss");

        let timeout_secs = if use_long_timeout {
            20
        } else if dead_url.scheme() == "udp" && udp_stealth_enabled {
            // Generic UDP compatibility uses a 1s stealth attempt followed by
            // a fresh 3s plain attempt. Leave time for resolve and PeerConn
            // handshake without slowing ordinary plain reconnects.
            6
        } else {
            2
        };
        Duration::from_secs(timeout_secs)
    }

    fn remaining_budget(started_at: Instant, total_timeout: Duration) -> Option<Duration> {
        let remaining = total_timeout.checked_sub(started_at.elapsed())?;
        (!remaining.is_zero()).then_some(remaining)
    }

    fn emit_connect_error(
        data: &ConnectorManagerData,
        dead_url: &url::Url,
        ip_version: IpVersion,
        error: &Error,
    ) {
        data.global_ctx.issue_event(GlobalCtxEvent::ConnectError(
            dead_url.to_string(),
            format!("{:?}", ip_version),
            format!("{:#?}", error),
        ));
    }

    fn reconnect_timeout_error(stage: &str, duration: Duration) -> Error {
        Error::AnyhowError(anyhow::anyhow!("{} timeout after {:?}", stage, duration))
    }

    async fn with_reconnect_timeout<T, F>(
        stage: &'static str,
        started_at: Instant,
        total_timeout: Duration,
        fut: F,
    ) -> Result<T, Error>
    where
        F: Future<Output = Result<T, Error>>,
    {
        let remaining = Self::remaining_budget(started_at, total_timeout)
            .ok_or_else(|| Self::reconnect_timeout_error(stage, started_at.elapsed()))?;
        timeout(remaining, fut)
            .await
            .map_err(|_| Self::reconnect_timeout_error(stage, remaining))?
    }
}

impl ManualConnectorManager {
    fn add_normal_connector_url(&self, url: url::Url) {
        let _state = self.data.connector_state_lock.lock().unwrap();
        self.data.removed_conn_urls.remove(&url);
        self.data.runtime_bootstrap_helpers.remove(&url);
        if self.data.reconnecting.contains(&url) {
            return;
        }
        self.data.connectors.insert(url);
    }

    pub fn add_connector<T>(&self, connector: T)
    where
        T: TunnelConnector + 'static,
    {
        tracing::info!("add_connector: {}", connector.remote_url());
        self.add_normal_connector_url(connector.remote_url());
    }

    pub async fn add_connector_by_url(&self, url: url::Url) -> Result<(), Error> {
        self.add_normal_connector_url(url);
        Ok(())
    }

    pub(crate) async fn add_runtime_bootstrap_helper_by_url(
        &self,
        url: url::Url,
    ) -> Result<(), Error> {
        let _state = self.data.connector_state_lock.lock().unwrap();
        let revives_removed = self.data.removed_conn_urls.remove(&url).is_some();
        if self.data.connectors.contains(&url) || self.data.reconnecting.contains(&url) {
            if revives_removed {
                self.data.runtime_bootstrap_helpers.insert(url);
            }
            return Ok(());
        }
        self.data.runtime_bootstrap_helpers.insert(url.clone());
        self.data.connectors.insert(url);
        Ok(())
    }

    pub async fn remove_connector(&self, url: url::Url) -> Result<(), Error> {
        tracing::info!("remove_connector: {}", url);
        let proto_url = url.clone().into();
        if !self
            .list_connectors()
            .await
            .iter()
            .any(|x| x.url.as_ref() == Some(&proto_url))
        {
            return Err(Error::NotFound);
        }
        let _state = self.data.connector_state_lock.lock().unwrap();
        self.data.runtime_bootstrap_helpers.remove(&url);
        self.data.removed_conn_urls.insert(url.into());
        Ok(())
    }

    pub async fn clear_connectors(&self) {
        let _state = self.data.connector_state_lock.lock().unwrap();
        self.data.runtime_bootstrap_helpers.clear();
        for url in self
            .data
            .connectors
            .iter()
            .chain(self.data.reconnecting.iter())
        {
            self.data.removed_conn_urls.insert(url.key().clone());
        }
    }

    pub async fn list_connectors(&self) -> Vec<Connector> {
        let dead_urls: BTreeSet<url::Url> = Self::collect_dead_conns(self.data.clone())
            .await
            .into_iter()
            .collect();
        let (connector_urls, reconnecting_urls) = {
            let _state = self.data.connector_state_lock.lock().unwrap();
            (
                self.data
                    .connectors
                    .iter()
                    .map(|url| url.key().clone())
                    .collect::<Vec<_>>(),
                self.data
                    .reconnecting
                    .iter()
                    .map(|url| url.key().clone())
                    .collect::<BTreeSet<_>>(),
            )
        };

        let mut ret = Vec::new();

        for conn_url in connector_urls {
            let mut status = ConnectorStatus::Connected;
            if dead_urls.contains(&conn_url) {
                status = ConnectorStatus::Disconnected;
            }
            ret.insert(
                0,
                Connector {
                    url: Some(conn_url.into()),
                    status: status.into(),
                },
            );
        }

        for conn_url in reconnecting_urls {
            let conn_url = conn_url.into();
            ret.insert(
                0,
                Connector {
                    url: Some(conn_url),
                    status: ConnectorStatus::Connecting.into(),
                },
            );
        }

        ret
    }

    async fn conn_mgr_reconn_routine(data: Arc<ConnectorManagerData>) {
        tracing::warn!("conn_mgr_routine started");
        let mut reconn_interval = tokio::time::interval(std::time::Duration::from_millis(
            use_global_var!(MANUAL_CONNECTOR_RECONNECT_INTERVAL_MS),
        ));
        let (reconn_result_send, mut reconn_result_recv) = mpsc::channel(100);
        let tasks = Arc::new(std::sync::Mutex::new(JoinSet::new()));
        join_joinset_background(tasks.clone(), "connector_reconnect_tasks".to_string());

        loop {
            tokio::select! {
                _ = reconn_interval.tick() => {
                    let dead_urls = Self::collect_dead_conns(data.clone()).await;
                    if dead_urls.is_empty() {
                        continue;
                    }
                    let pause_runtime_helpers = Self::pause_runtime_bootstrap_helpers(
                        &data,
                        &dead_urls,
                    );
                    for dead_url in dead_urls {
                        if pause_runtime_helpers
                            && data.runtime_bootstrap_helpers.contains(&dead_url)
                        {
                            continue;
                        }
                        if !Self::claim_dead_connector(&data, &dead_url) {
                            continue;
                        }
                        let data_clone = data.clone();
                        let sender = reconn_result_send.clone();

                        tasks.lock().unwrap().spawn(async move {
                            let reconn_ret = Self::conn_reconnect(data_clone.clone(), dead_url.clone() ).await;
                            let _ = sender.send(reconn_ret).await;

                            let _state = data_clone.connector_state_lock.lock().unwrap();
                            if data_clone.removed_conn_urls.remove(&dead_url).is_none() {
                                data_clone.connectors.insert(dead_url.clone());
                            }
                            data_clone.reconnecting.remove(&dead_url).unwrap();
                        });
                    }
                    tracing::info!("reconn_interval tick, done");
                }

                ret = reconn_result_recv.recv() => {
                    tracing::warn!("reconn_tasks done, reconn result: {:?}", ret);
                }
            }
        }
    }

    fn pause_runtime_bootstrap_helpers(
        data: &ConnectorManagerData,
        dead_urls: &BTreeSet<url::Url>,
    ) -> bool {
        dead_urls
            .iter()
            .any(|url| data.runtime_bootstrap_helpers.contains(url))
            && data
                .peer_manager
                .upgrade()
                .is_some_and(|peer_manager| peer_manager.get_peer_map().has_live_peer_conn())
    }

    fn claim_dead_connector(data: &ConnectorManagerData, url: &url::Url) -> bool {
        let _state = data.connector_state_lock.lock().unwrap();
        if !data.reconnecting.insert(url.clone()) {
            return false;
        }
        if data.connectors.remove(url).is_some() {
            return true;
        }
        data.reconnecting.remove(url);
        false
    }

    fn handle_remove_connector(data: Arc<ConnectorManagerData>) {
        let _state = data.connector_state_lock.lock().unwrap();
        let remove_later = DashSet::new();
        for it in data.removed_conn_urls.iter() {
            let url = it.key();
            if data.connectors.remove(url).is_some() {
                tracing::warn!("connector: {}, removed", url);
                continue;
            } else if data.reconnecting.contains(url) {
                tracing::warn!("connector: {}, reconnecting, remove later.", url);
                remove_later.insert(url.clone());
                continue;
            } else {
                tracing::warn!("connector: {}, not found", url);
            }
        }
        data.removed_conn_urls.clear();
        for it in remove_later.iter() {
            data.removed_conn_urls.insert(it.key().clone());
        }
    }

    async fn collect_dead_conns(data: Arc<ConnectorManagerData>) -> BTreeSet<url::Url> {
        Self::handle_remove_connector(data.clone());
        let mut ret = BTreeSet::new();
        let Some(pm) = data.peer_manager.upgrade() else {
            tracing::warn!("peer manager is gone, exit");
            return ret;
        };
        for url in data.connectors.iter().map(|x| x.key().clone()) {
            if !pm.get_peer_map().is_client_url_alive(&url)
                && !pm
                    .get_foreign_network_client()
                    .get_peer_map()
                    .is_client_url_alive(&url)
            {
                ret.insert(url.clone());
            }
        }
        ret
    }

    async fn conn_reconnect_with_ip_version(
        data: Arc<ConnectorManagerData>,
        dead_url: url::Url,
        ip_version: IpVersion,
        started_at: Instant,
        total_timeout: Duration,
    ) -> Result<ReconnResult, Error> {
        let connector = Self::with_reconnect_timeout(
            "resolve",
            started_at,
            total_timeout,
            create_connector_by_url(dead_url.as_str(), &data.global_ctx, ip_version),
        )
        .await?;

        data.global_ctx
            .issue_event(GlobalCtxEvent::Connecting(connector.remote_url()));
        tracing::info!("reconnect try connect... conn: {:?}", connector);
        let Some(pm) = data.peer_manager.upgrade() else {
            return Err(Error::AnyhowError(anyhow::anyhow!(
                "peer manager is gone, cannot reconnect"
            )));
        };

        let tunnel = Self::with_reconnect_timeout(
            "connect",
            started_at,
            total_timeout,
            pm.connect_tunnel(connector),
        )
        .await?;

        let (peer_id, conn_id) = Self::with_reconnect_timeout(
            "handshake",
            started_at,
            total_timeout,
            pm.add_client_tunnel_with_peer_id_hint(tunnel, true, None),
        )
        .await?;

        tracing::info!("reconnect succ: {} {} {}", peer_id, conn_id, dead_url);
        Ok(ReconnResult {
            dead_url: dead_url.to_string(),
            peer_id,
            conn_id,
        })
    }

    async fn conn_reconnect(
        data: Arc<ConnectorManagerData>,
        dead_url: url::Url,
    ) -> Result<ReconnResult, Error> {
        tracing::info!("reconnect: {}", dead_url);
        let reconnect_timeout = Self::reconnect_timeout(
            &dead_url,
            data.global_ctx.get_feature_flags().stealth_supported,
        );

        let mut ip_versions = vec![];
        if matches_scheme!(
            dead_url,
            TunnelScheme::Ring | TunnelScheme::Txt | TunnelScheme::Srv
        ) {
            ip_versions.push(IpVersion::Both);
        } else {
            let converted_dead_url =
                match crate::common::idn::convert_idn_to_ascii(dead_url.clone()) {
                    Ok(url) => url,
                    Err(error) => {
                        let error: Error = error.into();
                        Self::emit_connect_error(&data, &dead_url, IpVersion::Both, &error);
                        return Err(error);
                    }
                };
            let addrs = match Self::with_reconnect_timeout(
                "resolve",
                Instant::now(),
                reconnect_timeout,
                socket_addrs(&converted_dead_url, || Some(1000)),
            )
            .await
            {
                Ok(addrs) => addrs,
                Err(error) => {
                    Self::emit_connect_error(&data, &dead_url, IpVersion::Both, &error);
                    return Err(error);
                }
            };
            tracing::info!(?addrs, ?dead_url, "get ip from url done");
            let mut has_ipv4 = false;
            let mut has_ipv6 = false;
            for addr in addrs {
                if addr.is_ipv4() {
                    if !has_ipv4 {
                        ip_versions.insert(0, IpVersion::V4);
                    }
                    has_ipv4 = true;
                } else if addr.is_ipv6() {
                    if !has_ipv6 {
                        ip_versions.push(IpVersion::V6);
                    }
                    has_ipv6 = true;
                }
            }
        }

        let mut reconn_ret = Err(Error::AnyhowError(anyhow::anyhow!(
            "cannot get ip from url"
        )));
        for ip_version in ip_versions {
            let started_at = Instant::now();
            let ret = Self::conn_reconnect_with_ip_version(
                data.clone(),
                dead_url.clone(),
                ip_version,
                started_at,
                reconnect_timeout,
            )
            .await;
            tracing::info!("reconnect: {} done, ret: {:?}", dead_url, ret);

            match ret {
                Ok(result) => return Ok(result),
                Err(error) => {
                    Self::emit_connect_error(&data, &dead_url, ip_version, &error);
                    reconn_ret = Err(error);
                }
            }
        }

        reconn_ret
    }
}

#[derive(Clone)]
pub struct ConnectorManagerRpcService(pub Weak<ManualConnectorManager>);

#[async_trait::async_trait]
impl ConnectorManageRpc for ConnectorManagerRpcService {
    type Controller = BaseController;

    async fn list_connector(
        &self,
        _: BaseController,
        _request: ListConnectorRequest,
    ) -> Result<ListConnectorResponse, rpc_types::error::Error> {
        let mut ret = ListConnectorResponse::default();
        let connectors = weak_upgrade(&self.0)?.list_connectors().await;
        ret.connectors = connectors;
        Ok(ret)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        peers::tests::{connect_peer_manager, create_mock_peer_manager},
        tunnel::{Tunnel, TunnelError},
    };

    use super::*;

    async fn wait_for_connecting_event(
        events: &mut crate::common::global_ctx::EventBusSubscriber,
        expected: &url::Url,
    ) {
        loop {
            match events.recv().await {
                Ok(GlobalCtxEvent::Connecting(url)) if &url == expected => return,
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    panic!("event bus closed before connector attempt")
                }
            }
        }
    }

    async fn collect_connecting_events(
        events: &mut crate::common::global_ctx::EventBusSubscriber,
        duration: Duration,
    ) -> BTreeSet<url::Url> {
        let deadline = tokio::time::Instant::now() + duration;
        let mut urls = BTreeSet::new();
        loop {
            match tokio::time::timeout_at(deadline, events.recv()).await {
                Ok(Ok(GlobalCtxEvent::Connecting(url))) => {
                    urls.insert(url);
                }
                Ok(Ok(_)) => {}
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(count))) => {
                    panic!("lost {count} events while checking connector suppression")
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) | Err(_) => return urls,
            }
        }
    }

    async fn start_blackhole_listener() -> (url::Url, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = url::Url::parse(&format!("tcp://{}", listener.local_addr().unwrap())).unwrap();
        let task = tokio::spawn(async move {
            let mut streams = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                streams.push(stream);
            }
        });
        (url, task)
    }

    async fn wait_for_live_peer(peer_manager: &PeerManager) {
        timeout(Duration::from_secs(5), async {
            while !peer_manager.get_peer_map().has_live_peer_conn() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn runtime_helper_lifecycle_is_instance_local() {
        let peer_manager_a = create_mock_peer_manager().await;
        let peer_manager_b = create_mock_peer_manager().await;
        let remote_peer_manager = create_mock_peer_manager().await;
        connect_peer_manager(peer_manager_a.clone(), remote_peer_manager.clone()).await;
        wait_for_live_peer(&peer_manager_a).await;
        let manager_a =
            ManualConnectorManager::new(peer_manager_a.get_global_ctx(), peer_manager_a);
        let manager_b =
            ManualConnectorManager::new(peer_manager_b.get_global_ctx(), peer_manager_b);
        let url = url::Url::parse("tcp://127.0.0.1:1").unwrap();

        manager_a
            .add_runtime_bootstrap_helper_by_url(url.clone())
            .await
            .unwrap();
        assert!(manager_a.data.runtime_bootstrap_helpers.contains(&url));
        assert!(!manager_b.data.runtime_bootstrap_helpers.contains(&url));

        manager_a.add_connector_by_url(url.clone()).await.unwrap();
        assert!(!manager_a.data.runtime_bootstrap_helpers.contains(&url));
        manager_a.remove_connector(url.clone()).await.unwrap();
        manager_a.add_connector_by_url(url.clone()).await.unwrap();
        assert!(!manager_a.data.removed_conn_urls.contains(&url));

        let helper_to_remove = url::Url::parse("tcp://127.0.0.1:2").unwrap();
        manager_a
            .add_runtime_bootstrap_helper_by_url(helper_to_remove.clone())
            .await
            .unwrap();
        manager_a
            .remove_connector(helper_to_remove.clone())
            .await
            .unwrap();
        assert!(
            !manager_a
                .data
                .runtime_bootstrap_helpers
                .contains(&helper_to_remove)
        );

        let helper_to_clear = url::Url::parse("tcp://127.0.0.1:3").unwrap();
        manager_a
            .add_runtime_bootstrap_helper_by_url(helper_to_clear)
            .await
            .unwrap();
        manager_a.clear_connectors().await;
        assert!(manager_a.data.runtime_bootstrap_helpers.is_empty());
    }

    #[tokio::test]
    async fn live_primary_peer_pauses_only_runtime_helpers_without_events() {
        let peer_manager = create_mock_peer_manager().await;
        let remote_peer_manager = create_mock_peer_manager().await;
        connect_peer_manager(peer_manager.clone(), remote_peer_manager.clone()).await;
        wait_for_live_peer(&peer_manager).await;

        let manager =
            ManualConnectorManager::new(peer_manager.get_global_ctx(), peer_manager.clone());
        let mut events = peer_manager.get_global_ctx().subscribe();
        let (helper, helper_blackhole) = start_blackhole_listener().await;
        let (configured, configured_blackhole) = start_blackhole_listener().await;
        manager
            .add_runtime_bootstrap_helper_by_url(helper.clone())
            .await
            .unwrap();
        manager
            .add_connector_by_url(configured.clone())
            .await
            .unwrap();
        let dead_urls = BTreeSet::from([helper.clone(), configured.clone()]);

        let connecting = collect_connecting_events(&mut events, Duration::from_millis(2200)).await;
        assert!(connecting.contains(&configured));
        assert!(!connecting.contains(&helper));
        assert!(ManualConnectorManager::pause_runtime_bootstrap_helpers(
            &manager.data,
            &dead_urls
        ));
        assert!(manager.data.runtime_bootstrap_helpers.contains(&helper));
        assert!(!manager.data.runtime_bootstrap_helpers.contains(&configured));
        let helper_proto = helper.clone().into();
        assert_eq!(
            manager
                .list_connectors()
                .await
                .into_iter()
                .find(|connector| connector.url.as_ref() == Some(&helper_proto))
                .unwrap()
                .status,
            ConnectorStatus::Disconnected as i32
        );

        for peer_id in peer_manager.get_peer_map().list_peers() {
            peer_manager
                .get_peer_map()
                .close_peer(peer_id)
                .await
                .unwrap();
        }
        assert!(!ManualConnectorManager::pause_runtime_bootstrap_helpers(
            &manager.data,
            &dead_urls
        ));

        timeout(
            Duration::from_secs(3),
            wait_for_connecting_event(&mut events, &helper),
        )
        .await
        .unwrap();
        assert!(manager.data.reconnecting.contains(&helper));
        manager.add_connector_by_url(helper.clone()).await.unwrap();
        assert!(!manager.data.runtime_bootstrap_helpers.contains(&helper));
        assert!(!manager.data.connectors.contains(&helper));

        timeout(
            Duration::from_secs(5),
            wait_for_connecting_event(&mut events, &helper),
        )
        .await
        .unwrap();
        assert!(!manager.data.runtime_bootstrap_helpers.contains(&helper));

        manager.remove_connector(helper.clone()).await.unwrap();
        tokio::time::sleep(Duration::from_secs(3)).await;
        let helper_proto = helper.into();
        assert!(
            manager
                .list_connectors()
                .await
                .iter()
                .all(|connector| connector.url.as_ref() != Some(&helper_proto))
        );

        let _ = collect_connecting_events(&mut events, Duration::from_millis(100)).await;
        timeout(
            Duration::from_secs(5),
            wait_for_connecting_event(&mut events, &configured),
        )
        .await
        .unwrap();
        assert!(manager.data.reconnecting.contains(&configured));
        manager.clear_connectors().await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(manager.list_connectors().await.is_empty());
        helper_blackhole.abort();
        configured_blackhole.abort();
    }

    #[test]
    fn reconnect_timeout_reserves_udp_stealth_fallback_budget() {
        let udp = url::Url::parse("udp://127.0.0.1:11010").unwrap();
        let tcp = url::Url::parse("tcp://127.0.0.1:11010").unwrap();

        assert_eq!(
            ManualConnectorManager::reconnect_timeout(&udp, true),
            Duration::from_secs(6)
        );
        assert_eq!(
            ManualConnectorManager::reconnect_timeout(&udp, false),
            Duration::from_secs(2)
        );
        assert_eq!(
            ManualConnectorManager::reconnect_timeout(&tcp, true),
            Duration::from_secs(2)
        );
    }

    #[tokio::test]
    async fn reconnect_timeout_reports_exhausted_budget_for_stage() {
        let started_at = Instant::now() - Duration::from_millis(50);
        let err = ManualConnectorManager::with_reconnect_timeout(
            "resolve",
            started_at,
            Duration::from_millis(1),
            async { Ok::<(), Error>(()) },
        )
        .await
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("resolve timeout after"));
    }

    #[tokio::test]
    async fn reconnect_timeout_reports_stage_timeout_with_remaining_budget() {
        let err = ManualConnectorManager::with_reconnect_timeout(
            "handshake",
            Instant::now(),
            Duration::from_millis(10),
            async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok::<(), Error>(())
            },
        )
        .await
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("handshake timeout after"));
    }

    #[tokio::test]
    async fn reconnect_timeout_preserves_success_within_budget() {
        let result = ManualConnectorManager::with_reconnect_timeout(
            "connect",
            Instant::now(),
            Duration::from_millis(50),
            async { Ok::<_, Error>(123_u32) },
        )
        .await
        .unwrap();

        assert_eq!(result, 123);
    }

    #[tokio::test]
    async fn test_reconnect_with_connecting_addr() {
        let peer_mgr = create_mock_peer_manager().await;
        let mgr = ManualConnectorManager::new(peer_mgr.get_global_ctx(), peer_mgr);

        struct MockConnector {}
        #[async_trait::async_trait]
        impl TunnelConnector for MockConnector {
            fn remote_url(&self) -> url::Url {
                url::Url::parse("tcp://aa.com").unwrap()
            }
            async fn connect(&mut self) -> Result<Box<dyn Tunnel>, TunnelError> {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                Err(TunnelError::InvalidPacket("fake error".into()))
            }
        }

        mgr.add_connector(MockConnector {});

        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}
