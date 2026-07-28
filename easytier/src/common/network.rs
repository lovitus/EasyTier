use std::{
    collections::HashMap,
    future::Future,
    net::IpAddr,
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

#[cfg(target_os = "windows")]
use network_interface::{
    Addr as SystemAddr, NetworkInterface as SystemNetworkInterface, NetworkInterfaceConfig,
};
use pnet::datalink::NetworkInterface;
#[cfg(target_os = "windows")]
use pnet::{ipnetwork::IpNetwork, util::MacAddr};
use tokio::{
    sync::{Mutex, RwLock},
    task::JoinSet,
};

use crate::proto::peer_rpc::GetIpListResponse;

use super::{netns::NetNS, stun::StunInfoCollectorTrait};

pub const CACHED_IP_LIST_TIMEOUT_SEC: u64 = 60;
const UNDERLAY_SNAPSHOT_TTL: Duration = Duration::from_secs(5);
const UNDERLAY_SNAPSHOT_REFRESH_ATTEMPTS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnderlayInterfaceIdentity {
    pub(crate) name: String,
    pub(crate) index: u32,
    pub(crate) is_point_to_point: bool,
}

#[cfg(test)]
mod underlay_snapshot_contract_tests {
    use std::{
        net::{Ipv4Addr, Ipv6Addr},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use pnet::datalink::NetworkInterface;
    use tokio::sync::Notify;

    use super::*;

    fn interface(name: &str, index: u32, ips: &[&str]) -> NetworkInterface {
        NetworkInterface {
            name: name.to_owned(),
            description: String::new(),
            index,
            mac: None,
            ips: ips.iter().map(|ip| ip.parse().unwrap()).collect(),
            flags: 0,
        }
    }

    fn snapshot_with_interface(name: &str, index: u32, addr: &str) -> UnderlayInterfaceSnapshot {
        let iface = interface(name, index, &[addr]);
        IPCollector::build_underlay_snapshot(&[iface.clone()], &[iface], None, None)
    }

    #[test]
    fn snapshot_preserves_sources_first_match_and_fallback_families() {
        let primary = interface("en-test0", 4, &["192.0.2.10/24", "2001:db8::10/64"]);
        let duplicate = interface(
            "tun-test0",
            9,
            &["192.0.2.10/24", "2001:db8::10/64", "10.44.0.1/24"],
        );
        let all = vec![primary.clone(), duplicate];
        let snapshot = IPCollector::build_underlay_snapshot(
            &all,
            &[primary],
            Some(Ipv4Addr::new(192, 0, 2, 10)),
            Some("2001:db8::10".parse::<Ipv6Addr>().unwrap()),
        );

        assert!(
            snapshot
                .ip_list
                .interface_ipv4s
                .contains(&Ipv4Addr::new(192, 0, 2, 10).into())
        );
        assert!(
            !snapshot
                .ip_list
                .interface_ipv4s
                .contains(&Ipv4Addr::new(10, 44, 0, 1).into())
        );
        assert!(
            snapshot
                .ip_list
                .interface_ipv6s
                .contains(&"2001:db8::10".parse::<Ipv6Addr>().unwrap().into())
        );
        let ipv4: IpAddr = "192.0.2.10".parse().unwrap();
        let resolved = snapshot.interface_for(&ipv4).unwrap();
        assert_eq!(resolved.name, "en-test0");
        assert_eq!(resolved.index, 4);
        assert!(!snapshot.has_unmapped_fallback_for(true));
        assert!(!snapshot.has_unmapped_fallback_for(false));
    }

    #[test]
    fn unmapped_fallback_is_tracked_per_address_family() {
        let iface = interface("en-test0", 4, &["192.0.2.10/24"]);
        let snapshot = IPCollector::build_underlay_snapshot(
            &[iface.clone()],
            &[iface],
            Some(Ipv4Addr::new(192, 0, 2, 10)),
            Some("2001:db8::99".parse().unwrap()),
        );

        assert!(!snapshot.has_unmapped_fallback_for(true));
        assert!(snapshot.has_unmapped_fallback_for(false));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_consumers_publish_once_per_stable_epoch_and_ttl() {
        let cache = Arc::new(UnderlaySnapshotCache::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let now = Instant::now();
        let mut tasks = Vec::new();
        for _ in 0..32 {
            let cache = cache.clone();
            let calls = calls.clone();
            tasks.push(tokio::spawn(async move {
                cache
                    .get_or_refresh_at(now, move || {
                        let calls = calls.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            snapshot_with_interface("en-test0", 4, "192.0.2.10/24")
                        }
                    })
                    .await
            }));
        }
        let first = tasks.remove(0).await.unwrap().unwrap();
        for task in tasks {
            assert!(Arc::ptr_eq(&first, &task.await.unwrap().unwrap()));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let before_ttl = cache
            .get_or_refresh_at(now + Duration::from_millis(4_999), || async {
                panic!("fresh snapshot must not be recollected before the TTL")
            })
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&first, &before_ttl));

        let after_ttl = cache
            .get_or_refresh_at(now + Duration::from_secs(5), {
                let calls = calls.clone();
                move || {
                    let calls = calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        snapshot_with_interface("en-test1", 12, "198.51.100.10/24")
                    }
                }
            })
            .await
            .unwrap();
        assert!(!Arc::ptr_eq(&first, &after_ttl));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn inflight_invalidation_discards_old_epoch_and_owner_cancellation_recovers() {
        let cache = Arc::new(UnderlaySnapshotCache::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let now = Instant::now();

        let owner = {
            let cache = cache.clone();
            let calls = calls.clone();
            let started = started.clone();
            let release = release.clone();
            tokio::spawn(async move {
                cache
                    .get_or_refresh_at(now, move || {
                        let call = calls.fetch_add(1, Ordering::SeqCst);
                        let started = started.clone();
                        let release = release.clone();
                        async move {
                            if call == 0 {
                                started.notify_one();
                                release.notified().await;
                                snapshot_with_interface("stale", 1, "192.0.2.1/24")
                            } else {
                                snapshot_with_interface("current", 2, "192.0.2.2/24")
                            }
                        }
                    })
                    .await
            })
        };
        started.notified().await;
        cache.invalidate();
        release.notify_waiters();
        let current = tokio::time::timeout(Duration::from_secs(2), owner)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let current_ip: IpAddr = "192.0.2.2".parse().unwrap();
        assert_eq!(current.interface_for(&current_ip).unwrap().name, "current");
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let cancelled_cache = Arc::new(UnderlaySnapshotCache::new());
        let cancelled_started = Arc::new(Notify::new());
        let cancelled_owner = {
            let cache = cancelled_cache.clone();
            let started = cancelled_started.clone();
            tokio::spawn(async move {
                cache
                    .get_or_refresh_at(Instant::now(), move || {
                        let started = started.clone();
                        async move {
                            started.notify_one();
                            std::future::pending::<UnderlayInterfaceSnapshot>().await
                        }
                    })
                    .await
            })
        };
        cancelled_started.notified().await;
        // Cancellation-safety contract: `abort()` schedules the owner future
        // for cancellation, and dropping that future releases `_refresh`.
        // Start the contender immediately so this test exercises a waiter
        // recovering from an aborted refresh owner. Awaiting the aborted
        // `JoinHandle` here would serialize away the race this test protects;
        // the timeout below proves that the waiter is eventually released.
        cancelled_owner.abort();
        let recovered = tokio::time::timeout(
            Duration::from_secs(2),
            cancelled_cache.get_or_refresh_at(Instant::now(), || async {
                snapshot_with_interface("recovered", 3, "192.0.2.3/24")
            }),
        )
        .await
        .unwrap()
        .unwrap();
        let recovered_ip: IpAddr = "192.0.2.3".parse().unwrap();
        assert_eq!(
            recovered.interface_for(&recovered_ip).unwrap().name,
            "recovered"
        );
    }
}

#[derive(Debug)]
pub(crate) struct UnderlayInterfaceSnapshot {
    pub(crate) ip_list: GetIpListResponse,
    pub(crate) generation: u64,
    interfaces_by_addr: HashMap<IpAddr, UnderlayInterfaceIdentity>,
    unmapped_fallbacks: Vec<IpAddr>,
}

impl UnderlayInterfaceSnapshot {
    pub(crate) fn interface_for(&self, ip: &IpAddr) -> Option<&UnderlayInterfaceIdentity> {
        self.interfaces_by_addr.get(ip)
    }

    pub(crate) fn unmapped_fallbacks(&self) -> &[IpAddr] {
        &self.unmapped_fallbacks
    }

    pub(crate) fn has_unmapped_fallback_for(&self, is_ipv4: bool) -> bool {
        self.unmapped_fallbacks
            .iter()
            .any(|ip| ip.is_ipv4() == is_ipv4)
    }
}

struct CachedUnderlaySnapshot {
    snapshot: Arc<UnderlayInterfaceSnapshot>,
    epoch: u64,
    collected_at: Instant,
}

struct UnderlaySnapshotCache {
    state: RwLock<Option<CachedUnderlaySnapshot>>,
    refresh: Mutex<()>,
    epoch: AtomicU64,
    next_generation: AtomicU64,
    invalidated_through_generation: AtomicU64,
}

impl UnderlaySnapshotCache {
    fn new() -> Self {
        Self {
            state: RwLock::new(None),
            refresh: Mutex::new(()),
            epoch: AtomicU64::new(0),
            next_generation: AtomicU64::new(0),
            invalidated_through_generation: AtomicU64::new(0),
        }
    }

    fn invalidate(&self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
    }

    fn invalidate_generation(&self, generation: u64) {
        self.invalidated_through_generation
            .fetch_max(generation, Ordering::AcqRel);
    }

    fn is_fresh(&self, cached: &CachedUnderlaySnapshot, now: Instant, epoch: u64) -> bool {
        cached.epoch == epoch
            && cached.snapshot.generation
                > self.invalidated_through_generation.load(Ordering::Acquire)
            && now.saturating_duration_since(cached.collected_at) < UNDERLAY_SNAPSHOT_TTL
    }

    async fn get_or_refresh_at<F, Fut>(
        &self,
        now: Instant,
        mut collect: F,
    ) -> anyhow::Result<Arc<UnderlayInterfaceSnapshot>>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = UnderlayInterfaceSnapshot>,
    {
        for _ in 0..UNDERLAY_SNAPSHOT_REFRESH_ATTEMPTS {
            let epoch = self.epoch.load(Ordering::Acquire);
            if let Some(cached) = self.state.read().await.as_ref()
                && self.is_fresh(cached, now, epoch)
            {
                return Ok(cached.snapshot.clone());
            }

            let _refresh = self.refresh.lock().await;
            let epoch = self.epoch.load(Ordering::Acquire);
            if let Some(cached) = self.state.read().await.as_ref()
                && self.is_fresh(cached, now, epoch)
            {
                return Ok(cached.snapshot.clone());
            }

            let mut snapshot = collect().await;
            // Concurrency contract: this epoch observation is the successful
            // refresh's linearization point. An invalidation observed here
            // discards the collection. An invalidation immediately after this
            // point is ordered after this refresh: this caller may receive the
            // snapshot, while the cached entry retains the old epoch and is
            // rejected by every later lookup. That is the same unavoidable
            // boundary as a network event arriving immediately after a caller
            // receives any valid snapshot; it is not an old refresh
            // overwriting a newer event.
            if self.epoch.load(Ordering::Acquire) != epoch {
                continue;
            }

            snapshot.generation = self
                .next_generation
                .fetch_add(1, Ordering::AcqRel)
                .wrapping_add(1);
            let snapshot = Arc::new(snapshot);
            *self.state.write().await = Some(CachedUnderlaySnapshot {
                snapshot: snapshot.clone(),
                epoch,
                collected_at: now,
            });
            return Ok(snapshot);
        }

        anyhow::bail!("underlay interface snapshot changed repeatedly during collection")
    }
}

#[derive(Clone)]
pub(crate) struct UnderlaySnapshotGenerationLease {
    cache: Arc<UnderlaySnapshotCache>,
    generation: u64,
}

impl UnderlaySnapshotGenerationLease {
    pub(crate) fn invalidate(&self) {
        self.cache.invalidate_generation(self.generation);
    }

    #[cfg(test)]
    pub(crate) fn is_invalidated(&self) -> bool {
        self.cache
            .invalidated_through_generation
            .load(Ordering::Acquire)
            >= self.generation
    }
}

struct InterfaceFilter {
    iface: NetworkInterface,
}

#[cfg(any(
    target_os = "android",
    target_os = "ios",
    all(target_os = "macos", feature = "macos-ne"),
    target_env = "ohos"
))]
impl InterfaceFilter {
    async fn filter_iface(&self) -> bool {
        true
    }
}

#[cfg(all(target_os = "linux", not(target_env = "ohos")))]
impl InterfaceFilter {
    async fn is_tun_tap_device(&self) -> bool {
        let path = format!("/sys/class/net/{}/tun_flags", self.iface.name);
        tokio::fs::metadata(&path).await.is_ok()
    }

    async fn has_valid_ip(&self) -> bool {
        self.iface
            .ips
            .iter()
            .map(|ip| ip.ip())
            .any(|ip| !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast())
    }

    async fn filter_iface(&self) -> bool {
        tracing::trace!(
            "filter linux iface: {:?}, is_point_to_point: {}, is_loopback: {}, is_up: {}, is_lower_up: {}, is_tun: {}, has_valid_ip: {}",
            self.iface,
            self.iface.is_point_to_point(),
            self.iface.is_loopback(),
            self.iface.is_up(),
            self.iface.is_lower_up(),
            self.is_tun_tap_device().await,
            self.has_valid_ip().await
        );

        !self.iface.is_point_to_point()
            && !self.iface.is_loopback()
            && self.iface.is_up()
            && self.iface.is_lower_up()
            && !self.is_tun_tap_device().await
            && self.has_valid_ip().await
    }
}

// Cache for networksetup command output
#[cfg(all(target_os = "macos", not(feature = "macos-ne")))]
static NETWORKSETUP_CACHE: std::sync::OnceLock<Mutex<(String, std::time::Instant)>> =
    std::sync::OnceLock::new();

#[cfg(any(
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "freebsd"
))]
impl InterfaceFilter {
    #[cfg(all(target_os = "macos", not(feature = "macos-ne")))]
    async fn get_networksetup_output() -> String {
        use anyhow::Context;
        use std::time::{Duration, Instant};
        let cache = NETWORKSETUP_CACHE.get_or_init(|| Mutex::new((String::new(), Instant::now())));
        let mut cache_guard = cache.lock().await;

        // Check if cache is still valid (less than 1 minute old)
        if cache_guard.1.elapsed() < Duration::from_secs(60) && !cache_guard.0.is_empty() {
            return cache_guard.0.clone();
        }

        // Cache is expired or empty, fetch new data
        let stdout = tokio::process::Command::new("networksetup")
            .args(["-listallhardwareports"])
            .output()
            .await
            .with_context(|| "Failed to execute networksetup command")
            .and_then(|output| {
                std::str::from_utf8(&output.stdout)
                    .map(|s| s.to_string())
                    .with_context(|| "Failed to convert networksetup output to string")
            })
            .unwrap_or_else(|e| {
                tracing::error!("Failed to execute networksetup command: {:?}", e);
                String::new()
            });

        // Update cache
        cache_guard.0 = stdout.clone();
        cache_guard.1 = Instant::now();

        stdout
    }

    #[cfg(all(target_os = "macos", not(feature = "macos-ne")))]
    async fn is_interface_physical(&self) -> bool {
        let interface_name = &self.iface.name;
        let stdout = Self::get_networksetup_output().await;

        let lines: Vec<&str> = stdout.lines().collect();

        for i in 0..lines.len() {
            let line = lines[i];

            if line.contains("Device:") && line.contains(interface_name) {
                let next_line = lines[i + 1];
                return !next_line.contains("Virtual Interface");
            }
        }

        false
    }

    #[cfg(target_os = "freebsd")]
    async fn is_interface_physical(&self) -> bool {
        // if mac addr is not zero, then it's physical interface
        self.iface.mac.map(|mac| !mac.is_zero()).unwrap_or(false)
    }

    async fn filter_iface(&self) -> bool {
        !self.iface.is_point_to_point()
            && !self.iface.is_loopback()
            && self.iface.is_up()
            && self.is_interface_physical().await
    }
}

#[cfg(target_os = "windows")]
impl InterfaceFilter {
    async fn filter_iface(&self) -> bool {
        tracing::debug!(
            "iface_name: {:?}, p2p: {:?}, is_up: {:?}, iface: {:?}",
            self.iface.name,
            self.iface.is_point_to_point(),
            self.iface.is_up(),
            self.iface
        );
        !self.iface.is_point_to_point()
            && !self.iface.is_loopback()
            && self
                .iface
                .ips
                .iter()
                .map(|ip| ip.ip())
                .any(|ip| !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast())
            && self.iface.mac.map(|mac| !mac.is_zero()).unwrap_or(false)
    }
}

pub async fn local_ipv4() -> std::io::Result<std::net::Ipv4Addr> {
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    socket.connect("8.8.8.8:80").await?;
    let addr = socket.local_addr()?;
    match addr.ip() {
        std::net::IpAddr::V4(ip) => Ok(ip),
        std::net::IpAddr::V6(_) => Err(std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "no ipv4 address",
        )),
    }
}

pub async fn local_ipv6() -> std::io::Result<std::net::Ipv6Addr> {
    let socket = tokio::net::UdpSocket::bind("[::]:0").await?;
    socket
        .connect("[2001:4860:4860:0000:0000:0000:0000:8888]:80")
        .await?;
    let addr = socket.local_addr()?;
    match addr.ip() {
        std::net::IpAddr::V6(ip) => Ok(ip),
        std::net::IpAddr::V4(_) => Err(std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "no ipv4 address",
        )),
    }
}

pub struct IPCollector {
    cached_ip_list: Arc<RwLock<GetIpListResponse>>,
    collect_ip_task: Mutex<JoinSet<()>>,
    underlay_snapshot: Arc<UnderlaySnapshotCache>,
    net_ns: NetNS,
    stun_info_collector: Arc<Box<dyn StunInfoCollectorTrait>>,
}

impl IPCollector {
    pub fn new<T: StunInfoCollectorTrait + 'static>(net_ns: NetNS, stun_info_collector: T) -> Self {
        Self {
            cached_ip_list: Arc::new(RwLock::new(GetIpListResponse::default())),
            collect_ip_task: Mutex::new(JoinSet::new()),
            underlay_snapshot: Arc::new(UnderlaySnapshotCache::new()),
            net_ns,
            stun_info_collector: Arc::new(Box::new(stun_info_collector)),
        }
    }

    pub async fn collect_ip_addrs(&self) -> GetIpListResponse {
        let mut task = self.collect_ip_task.lock().await;
        if task.is_empty() {
            let cached_ip_list = self.cached_ip_list.clone();
            *cached_ip_list.write().await =
                Self::do_collect_local_ip_addrs(self.net_ns.clone()).await;
            let net_ns = self.net_ns.clone();
            let stun_info_collector = self.stun_info_collector.clone();
            let cached_ip_list = self.cached_ip_list.clone();
            task.spawn(async move {
                let mut last_fetch_iface_time = std::time::Instant::now();
                loop {
                    if last_fetch_iface_time.elapsed().as_secs() > CACHED_IP_LIST_TIMEOUT_SEC {
                        let ifaces = Self::do_collect_local_ip_addrs(net_ns.clone()).await;
                        *cached_ip_list.write().await = ifaces;
                        last_fetch_iface_time = std::time::Instant::now();
                    }

                    let stun_info = stun_info_collector.get_stun_info();
                    for ip in stun_info.public_ip.iter() {
                        let Ok(ip_addr) = ip.parse::<IpAddr>() else {
                            continue;
                        };

                        match ip_addr {
                            IpAddr::V4(v) => {
                                cached_ip_list.write().await.public_ipv4.replace(v.into());
                            }
                            IpAddr::V6(v) => {
                                cached_ip_list.write().await.public_ipv6.replace(v.into());
                            }
                        }
                    }

                    tracing::debug!(
                        "got public ip: {:?}, {:?}",
                        cached_ip_list.read().await.public_ipv4,
                        cached_ip_list.read().await.public_ipv6
                    );

                    let sleep_sec = if cached_ip_list.read().await.public_ipv4.is_some() {
                        CACHED_IP_LIST_TIMEOUT_SEC
                    } else {
                        3
                    };
                    tokio::time::sleep(std::time::Duration::from_secs(sleep_sec)).await;
                }
            });
        }

        self.cached_ip_list.read().await.deref().clone()
    }

    /// Collect interface addresses without the advertisement cache.
    ///
    /// Connector bind addresses must reflect DHCP and interface changes
    /// immediately; using the one-minute peer-advertisement cache here can
    /// repeatedly bind a removed address during reconnect.
    pub async fn collect_local_ip_addrs_now(&self) -> GetIpListResponse {
        Self::do_collect_local_ip_addrs(self.net_ns.clone()).await
    }

    pub(crate) async fn collect_underlay_snapshot(
        &self,
    ) -> anyhow::Result<Arc<UnderlayInterfaceSnapshot>> {
        self.underlay_snapshot
            .get_or_refresh_at(Instant::now(), || {
                Self::do_collect_underlay_snapshot(self.net_ns.clone())
            })
            .await
    }

    pub(crate) fn invalidate_underlay_snapshot(&self) {
        self.underlay_snapshot.invalidate();
    }

    pub(crate) fn invalidate_underlay_snapshot_generation(&self, generation: u64) {
        self.underlay_snapshot.invalidate_generation(generation);
    }

    pub(crate) fn underlay_snapshot_generation_lease(
        &self,
        generation: u64,
    ) -> UnderlaySnapshotGenerationLease {
        UnderlaySnapshotGenerationLease {
            cache: self.underlay_snapshot.clone(),
            generation,
        }
    }

    #[cfg(test)]
    pub(crate) async fn collect_underlay_snapshot_with<F, Fut>(
        &self,
        now: Instant,
        collect: F,
    ) -> anyhow::Result<Arc<UnderlayInterfaceSnapshot>>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = UnderlayInterfaceSnapshot>,
    {
        self.underlay_snapshot.get_or_refresh_at(now, collect).await
    }

    async fn collect_interfaces_raw(net_ns: NetNS) -> Vec<NetworkInterface> {
        let _g = net_ns.guard();
        #[cfg(target_os = "windows")]
        let ifaces = Self::collect_interfaces_windows();
        #[cfg(not(target_os = "windows"))]
        let ifaces = pnet::datalink::interfaces();
        ifaces
    }

    async fn filter_collected_interfaces(
        net_ns: NetNS,
        ifaces: &[NetworkInterface],
        filter: bool,
    ) -> Vec<NetworkInterface> {
        let _g = net_ns.guard();
        let mut ret = Vec::with_capacity(ifaces.len());
        for iface in ifaces {
            let f = InterfaceFilter {
                iface: iface.clone(),
            };
            if filter && !f.filter_iface().await {
                continue;
            }
            ret.push(iface.clone());
        }
        ret
    }

    pub async fn collect_interfaces(net_ns: NetNS, filter: bool) -> Vec<NetworkInterface> {
        let ifaces = Self::collect_interfaces_raw(net_ns.clone()).await;
        Self::filter_collected_interfaces(net_ns, &ifaces, filter).await
    }

    #[cfg(target_os = "windows")]
    fn collect_interfaces_windows() -> Vec<NetworkInterface> {
        match SystemNetworkInterface::show() {
            Ok(ifaces) => ifaces
                .into_iter()
                .map(Self::convert_windows_interface)
                .collect(),
            Err(e) => {
                tracing::warn!(
                    ?e,
                    "failed to enumerate interfaces via network-interface, falling back to pnet"
                );
                match std::panic::catch_unwind(pnet::datalink::interfaces) {
                    Ok(ifaces) => ifaces,
                    Err(_) => {
                        tracing::error!(
                            "failed to enumerate interfaces via both network-interface and pnet"
                        );
                        Vec::new()
                    }
                }
            }
        }
    }

    #[cfg(target_os = "windows")]
    fn convert_windows_interface(iface: SystemNetworkInterface) -> NetworkInterface {
        let mac = iface.mac_addr.as_deref().and_then(|mac| {
            mac.parse::<MacAddr>()
                .map_err(|e| {
                    tracing::debug!(iface = %iface.name, mac, ?e, "failed to parse interface mac")
                })
                .ok()
        });

        let ips = iface
            .addr
            .into_iter()
            .filter_map(Self::convert_windows_interface_addr)
            .collect();

        NetworkInterface {
            name: iface.name,
            description: String::new(),
            index: iface.index,
            mac,
            ips,
            // pnet does not populate Windows flags either, so keep the existing semantics.
            flags: 0,
        }
    }

    #[cfg(target_os = "windows")]
    fn convert_windows_interface_addr(addr: SystemAddr) -> Option<IpNetwork> {
        match addr {
            SystemAddr::V4(addr) => {
                let netmask = addr
                    .netmask
                    .map(IpAddr::V4)
                    .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::new(255, 255, 255, 255)));
                IpNetwork::with_netmask(IpAddr::V4(addr.ip), netmask)
                    .map_err(|e| {
                        tracing::debug!(ip = %addr.ip, ?addr.netmask, ?e, "failed to convert ipv4")
                    })
                    .ok()
            }
            SystemAddr::V6(addr) => {
                let netmask = addr
                    .netmask
                    .map(IpAddr::V6)
                    .unwrap_or(IpAddr::V6(std::net::Ipv6Addr::from(u128::MAX)));
                IpNetwork::with_netmask(IpAddr::V6(addr.ip), netmask)
                    .map_err(|e| {
                        tracing::debug!(ip = %addr.ip, ?addr.netmask, ?e, "failed to convert ipv6")
                    })
                    .ok()
            }
        }
    }

    #[tracing::instrument(skip(net_ns))]
    async fn do_collect_local_ip_addrs(net_ns: NetNS) -> GetIpListResponse {
        let mut ret = GetIpListResponse::default();

        let ifaces = Self::collect_interfaces(net_ns.clone(), true).await;
        let _g = net_ns.guard();
        for iface in ifaces {
            for ip in iface.ips {
                let ip: std::net::IpAddr = ip.ip();
                if let std::net::IpAddr::V4(v4) = ip {
                    if ip.is_loopback() || ip.is_multicast() {
                        continue;
                    }
                    ret.interface_ipv4s.push(v4.into());
                }
            }
        }

        let ifaces = Self::collect_interfaces(net_ns.clone(), false).await;
        let _g = net_ns.guard();
        for iface in ifaces {
            for ip in iface.ips {
                let ip: std::net::IpAddr = ip.ip();
                if let std::net::IpAddr::V6(v6) = ip {
                    if v6.is_multicast() || v6.is_loopback() || v6.is_unicast_link_local() {
                        continue;
                    }
                    ret.interface_ipv6s.push(v6.into());
                }
            }
        }

        if let Ok(v4_addr) = local_ipv4().await {
            tracing::trace!("got local ipv4: {}", v4_addr);
            if !ret.interface_ipv4s.contains(&v4_addr.into()) {
                ret.interface_ipv4s.push(v4_addr.into());
            }
        }

        if let Ok(v6_addr) = local_ipv6().await {
            tracing::trace!("got local ipv6: {}", v6_addr);
            if !ret.interface_ipv6s.contains(&v6_addr.into()) {
                ret.interface_ipv6s.push(v6_addr.into());
            }
        }

        ret
    }

    pub(crate) fn build_underlay_snapshot(
        ifaces: &[NetworkInterface],
        filtered_ifaces: &[NetworkInterface],
        fallback_ipv4: Option<std::net::Ipv4Addr>,
        fallback_ipv6: Option<std::net::Ipv6Addr>,
    ) -> UnderlayInterfaceSnapshot {
        let mut ip_list = GetIpListResponse::default();
        let mut interfaces_by_addr = HashMap::new();

        for iface in ifaces {
            let identity = UnderlayInterfaceIdentity {
                name: iface.name.clone(),
                index: iface.index,
                is_point_to_point: iface.is_point_to_point(),
            };
            for network in &iface.ips {
                interfaces_by_addr
                    .entry(network.ip())
                    .or_insert_with(|| identity.clone());
            }
        }

        for iface in filtered_ifaces {
            for network in &iface.ips {
                let ip = network.ip();
                if let IpAddr::V4(ipv4) = ip
                    && !ip.is_loopback()
                    && !ip.is_multicast()
                {
                    ip_list.interface_ipv4s.push(ipv4.into());
                }
            }
        }

        for iface in ifaces {
            for network in &iface.ips {
                if let IpAddr::V6(ipv6) = network.ip()
                    && !ipv6.is_multicast()
                    && !ipv6.is_loopback()
                    && !ipv6.is_unicast_link_local()
                {
                    ip_list.interface_ipv6s.push(ipv6.into());
                }
            }
        }

        let mut unmapped_fallbacks = Vec::new();
        if let Some(ipv4) = fallback_ipv4 {
            let ip = IpAddr::V4(ipv4);
            if !ip_list.interface_ipv4s.contains(&ipv4.into()) {
                ip_list.interface_ipv4s.push(ipv4.into());
            }
            if !interfaces_by_addr.contains_key(&ip) {
                unmapped_fallbacks.push(ip);
            }
        }
        if let Some(ipv6) = fallback_ipv6 {
            let ip = IpAddr::V6(ipv6);
            if !ip_list.interface_ipv6s.contains(&ipv6.into()) {
                ip_list.interface_ipv6s.push(ipv6.into());
            }
            if !interfaces_by_addr.contains_key(&ip) {
                unmapped_fallbacks.push(ip);
            }
        }

        UnderlayInterfaceSnapshot {
            ip_list,
            generation: 0,
            interfaces_by_addr,
            unmapped_fallbacks,
        }
    }

    async fn do_collect_underlay_snapshot(net_ns: NetNS) -> UnderlayInterfaceSnapshot {
        let ifaces = Self::collect_interfaces_raw(net_ns.clone()).await;
        let filtered_ifaces =
            Self::filter_collected_interfaces(net_ns.clone(), &ifaces, true).await;
        // Preserve the existing namespace semantics of
        // `do_collect_local_ip_addrs`: route-derived fallback probes must run
        // inside the collector's NetNS, not the process default namespace.
        let (fallback_ipv4, fallback_ipv6) = {
            let _g = net_ns.guard();
            (local_ipv4().await.ok(), local_ipv6().await.ok())
        };
        Self::build_underlay_snapshot(&ifaces, &filtered_ifaces, fallback_ipv4, fallback_ipv6)
    }
}
