use std::{
    collections::BTreeSet,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    pin::Pin,
    sync::{Arc, Weak},
    task::{Context, Poll},
};

#[cfg(all(feature = "leaf-policy-proxy", unix))]
use std::{collections::BTreeMap, net::SocketAddr};

#[cfg(all(feature = "leaf-policy-proxy", unix))]
type PolicyForwardingContext = (
    Arc<PacketClassifier>,
    Arc<ArcSwapOption<LeafPacketBridge>>,
    Arc<AtomicU64>,
);

use crate::{
    common::{
        config::NicBackend,
        error::Error,
        global_ctx::{ArcGlobalCtx, GlobalCtxEvent},
        ifcfg::{IfConfiger, IfConfiguerTrait},
        log,
    },
    instance::proxy_cidrs_monitor::ProxyCidrsMonitor,
    peers::{PacketRecvChanReceiver, peer_manager::PeerManager, recv_packet_from_chan},
    tunnel::{
        StreamItem, Tunnel, TunnelError, ZCPacketSink, ZCPacketStream,
        common::{FramedWriter, TunnelWrapper, ZCPacketToBytes, reserve_buf},
        packet_def::{TAIL_RESERVED_SIZE, ZCPacket, ZCPacketType},
    },
};

#[cfg(all(feature = "leaf-policy-proxy", unix))]
use arc_swap::ArcSwapOption;
use byteorder::WriteBytesExt as _;
use bytes::{BufMut, BytesMut};
use cidr::{Ipv4Inet, Ipv6Inet};
use futures::{SinkExt, Stream, StreamExt, lock::BiLock, ready};
use pin_project_lite::pin_project;
use pnet::packet::{ipv4::Ipv4Packet, ipv6::Ipv6Packet};
#[cfg(all(feature = "leaf-policy-proxy", unix))]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(all(feature = "leaf-policy-proxy", unix))]
use std::time::Duration;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{Mutex, Notify},
    task::JoinSet,
};
use tokio_util::bytes::Bytes;
#[cfg(target_os = "windows")]
use tokio_util::task::AbortOnDropHandle;
use tun::{AbstractDevice, AsyncDevice, Configuration, Layer};
use zerocopy::{NativeEndian, NetworkEndian};

#[cfg(target_os = "windows")]
use crate::common::ifcfg::RegistryManager;

#[cfg(all(feature = "leaf-policy-proxy", unix))]
use crate::gateway::socks5::Socks5Server;

#[cfg(all(feature = "leaf-policy-proxy", target_os = "linux"))]
use easytier_policy::LeafProcessRuntime;
#[cfg(all(feature = "leaf-policy-mobile", target_os = "android"))]
use easytier_policy::{InProcessLeafFactory, InProcessLeafRuntime};
#[cfg(all(feature = "leaf-policy-proxy", unix))]
use easytier_policy::{
    LeafPacketBridge, MeshRouteSnapshot, PacketClass, PacketClassifier, PolicyRuntime,
    RuntimeRestartBudget, RuntimeRestartDecision,
};

#[cfg(all(feature = "leaf-policy-proxy", unix))]
struct PolicyNicContext {
    revision: Arc<easytier_policy::PolicyRevision>,
    classifier: Arc<PacketClassifier>,
    bridge: Arc<ArcSwapOption<LeafPacketBridge>>,
    dropped_packets: Arc<AtomicU64>,
    bridge_updates: tokio::sync::watch::Sender<Option<Arc<LeafPacketBridge>>>,
    active: Arc<Mutex<Option<PolicyActiveRuntime>>>,
    #[cfg(target_os = "linux")]
    routing: Arc<Mutex<crate::policy_proxy::PolicyRoutingGuard>>,
    _lease: crate::policy_proxy::PolicyInstanceLease,
}

#[cfg(all(feature = "leaf-policy-proxy", unix))]
struct PolicyActiveRuntime {
    runtime: Arc<dyn PolicyRuntime>,
    bridge: Arc<LeafPacketBridge>,
    mesh_bridges: Arc<crate::policy_proxy::MeshProxyBridgeSet>,
}

#[cfg(all(feature = "leaf-policy-proxy", unix))]
struct PolicyLeafPacket {
    bridge: Arc<LeafPacketBridge>,
    packet: ZCPacket,
}

#[cfg(all(feature = "leaf-policy-proxy", unix))]
fn schedule_policy_runtime_restart(
    budget: &mut RuntimeRestartBudget,
    next_restart: &mut tokio::time::Instant,
) -> bool {
    match budget.record_failure() {
        RuntimeRestartDecision::RetryAfter(delay) => {
            *next_restart = tokio::time::Instant::now() + delay;
            false
        }
        RuntimeRestartDecision::Dormant => true,
    }
}

#[cfg(all(feature = "leaf-policy-proxy", unix))]
fn ensure_policy_mesh_credentials_confidential(
    global_ctx: &ArcGlobalCtx,
    revision: &easytier_policy::PolicyRevision,
) -> anyhow::Result<()> {
    let has_mesh_credentials =
        revision.document.proxies.values().any(|proxy| {
            proxy.via == easytier_policy::ProxyVia::Mesh && proxy.credentials().is_some()
        });
    if !has_mesh_credentials {
        return Ok(());
    }

    let identity = global_ctx.get_network_identity();
    let shared_secret_encryption = global_ctx.get_flags().enable_encryption
        && identity
            .network_secret
            .as_deref()
            .is_some_and(|secret| !secret.trim().is_empty());
    if shared_secret_encryption || global_ctx.is_explicit_secure_mode_enabled() {
        return Ok(());
    }

    anyhow::bail!(
        "authenticated mesh proxy actors require encrypted peer RPC; configure a non-empty network secret with encryption enabled or explicit secure_mode"
    )
}

#[cfg(all(feature = "leaf-policy-proxy", unix))]
async fn stop_policy_active_runtime(
    active: &Arc<Mutex<Option<PolicyActiveRuntime>>>,
    bridge: &Arc<ArcSwapOption<LeafPacketBridge>>,
    bridge_updates: &tokio::sync::watch::Sender<Option<Arc<LeafPacketBridge>>>,
) {
    let previous = active.lock().await.take();
    bridge.store(None);
    bridge_updates.send_replace(None);
    if let Some(previous) = previous {
        previous.mesh_bridges.disable_all();
        previous.runtime.shutdown().await;
    }
}

#[cfg(all(feature = "leaf-policy-proxy", unix))]
fn log_policy_drop(counter: &AtomicU64, reason: &'static str) {
    let dropped = counter.fetch_add(1, Ordering::Relaxed).saturating_add(1);
    if dropped.is_power_of_two() {
        tracing::warn!(
            dropped,
            reason,
            "policy packets are being dropped fail-closed"
        );
    }
}

pin_project! {
    pub struct TunStream {
        #[pin]
        l: BiLock<AsyncDevice>,
        cur_buf: BytesMut,
        has_packet_info: bool,
        payload_offset: usize,
    }
}

impl TunStream {
    pub fn new(l: BiLock<AsyncDevice>, has_packet_info: bool) -> Self {
        let mut payload_offset = ZCPacketType::NIC.get_packet_offsets().payload_offset;
        if has_packet_info {
            payload_offset -= 4;
        }
        Self {
            l,
            cur_buf: BytesMut::new(),
            has_packet_info,
            payload_offset,
        }
    }
}

impl Stream for TunStream {
    type Item = StreamItem;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<StreamItem>> {
        let self_mut = self.project();
        let mut g = ready!(self_mut.l.poll_lock(cx));
        reserve_buf(self_mut.cur_buf, 2500, 4 * 1024);
        if self_mut.cur_buf.is_empty() {
            unsafe {
                self_mut.cur_buf.set_len(*self_mut.payload_offset);
            }
        }
        let buf = self_mut.cur_buf.chunk_mut().as_mut_ptr();
        let buf = unsafe { std::slice::from_raw_parts_mut(buf, 2500) };
        let mut buf = ReadBuf::new(buf);

        let ret = ready!(g.as_pin_mut().poll_read(cx, &mut buf));
        let len = buf.filled().len();
        if len == 0 {
            return Poll::Ready(None);
        }
        unsafe { self_mut.cur_buf.advance_mut(len + TAIL_RESERVED_SIZE) };

        let mut ret_buf = self_mut.cur_buf.split();
        let cur_len = ret_buf.len();
        ret_buf.truncate(cur_len - TAIL_RESERVED_SIZE);

        match ret {
            Ok(_) => Poll::Ready(Some(Ok(ZCPacket::new_from_buf(ret_buf, ZCPacketType::NIC)))),
            Err(err) => {
                log::error!("tun stream error: {:?}", err);
                Poll::Ready(None)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
enum PacketProtocol {
    #[default]
    IPv4,
    IPv6,
    Other(u8),
}

// Note: the protocol in the packet information header is platform dependent.
impl PacketProtocol {
    #[cfg(any(target_os = "linux", target_os = "android", target_env = "ohos"))]
    fn into_pi_field(self) -> Result<u16, io::Error> {
        use nix::libc;
        match self {
            PacketProtocol::IPv4 => Ok(libc::ETH_P_IP as u16),
            PacketProtocol::IPv6 => Ok(libc::ETH_P_IPV6 as u16),
            PacketProtocol::Other(_) => Err(io::Error::other("neither an IPv4 nor IPv6 packet")),
        }
    }

    #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
    fn into_pi_field(self) -> Result<u16, io::Error> {
        use nix::libc;
        match self {
            PacketProtocol::IPv4 => Ok(libc::PF_INET as u16),
            PacketProtocol::IPv6 => Ok(libc::PF_INET6 as u16),
            PacketProtocol::Other(_) => Err(io::Error::other("neither an IPv4 nor IPv6 packet")),
        }
    }

    #[cfg(target_os = "windows")]
    fn into_pi_field(self) -> Result<u16, io::Error> {
        unimplemented!()
    }
}

/// Infer the protocol based on the first nibble in the packet buffer.
fn infer_proto(buf: &[u8]) -> PacketProtocol {
    match buf[0] >> 4 {
        4 => PacketProtocol::IPv4,
        6 => PacketProtocol::IPv6,
        p => PacketProtocol::Other(p),
    }
}

struct TunZCPacketToBytes {
    has_packet_info: bool,
}

impl TunZCPacketToBytes {
    pub fn new(has_packet_info: bool) -> Self {
        Self { has_packet_info }
    }

    pub fn fill_packet_info(
        &self,
        mut buf: &mut [u8],
        proto: PacketProtocol,
    ) -> Result<(), io::Error> {
        // flags is always 0
        buf.write_u16::<NativeEndian>(0)?;
        // write the protocol as network byte order
        buf.write_u16::<NetworkEndian>(proto.into_pi_field()?)?;
        Ok(())
    }
}

impl ZCPacketToBytes for TunZCPacketToBytes {
    fn zcpacket_into_bytes(&self, zc_packet: ZCPacket) -> Result<Bytes, TunnelError> {
        let payload_offset = zc_packet.payload_offset();
        let mut inner = zc_packet.inner();
        // we have peer manager header, so payload offset must larger than 4
        assert!(payload_offset >= 4);

        let ret = if self.has_packet_info {
            let mut inner = inner.split_off(payload_offset - 4);
            let proto = infer_proto(&inner[4..]);
            self.fill_packet_info(&mut inner[0..4], proto)?;
            inner
        } else {
            inner.split_off(payload_offset)
        };

        tracing::debug!(?ret, ?payload_offset, "convert zc packet to tun packet");

        Ok(ret.into())
    }
}

pin_project! {
    pub struct TunAsyncWrite {
        #[pin]
        l: BiLock<AsyncDevice>,
    }
}

impl AsyncWrite for TunAsyncWrite {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        let self_mut = self.project();
        let mut g = ready!(self_mut.l.poll_lock(cx));
        g.as_pin_mut().poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let self_mut = self.project();
        let mut g = ready!(self_mut.l.poll_lock(cx));
        g.as_pin_mut().poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        let self_mut = self.project();
        let mut g = ready!(self_mut.l.poll_lock(cx));
        g.as_pin_mut().poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<Result<usize, io::Error>> {
        let self_mut = self.project();
        let mut g = ready!(self_mut.l.poll_lock(cx));
        g.as_pin_mut().poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        true
    }
}

pub struct VirtualNic {
    global_ctx: ArcGlobalCtx,

    ifname: Option<String>,
    ifcfg: Arc<dyn IfConfiguerTrait + Send + Sync + 'static>,
    pending_backend: Option<NicBackend>,
    tun_offload_enabled: bool,
}

impl Drop for VirtualNic {
    fn drop(&mut self) {
        self.ifcfg.cleanup();

        #[cfg(target_os = "windows")]
        {
            if let Some(ref ifname) = self.ifname {
                // Try to clean up firewall rules, but don't panic in destructor
                if let Err(error) = crate::arch::windows::remove_interface_firewall_rules(ifname) {
                    log::warn!(
                        %error,
                        "failed to remove firewall rules for interface {}",
                        ifname
                    );
                }
            }
        }
    }
}

impl VirtualNic {
    fn runtime_netns_guard(&self) -> Option<Box<crate::common::netns::NetNSGuard>> {
        self.ifcfg
            .requires_runtime_netns_guard()
            .then(|| self.global_ctx.net_ns.guard())
    }

    pub fn new(global_ctx: ArcGlobalCtx) -> Self {
        Self {
            global_ctx,
            ifname: None,
            ifcfg: Arc::new(IfConfiger {}),
            pending_backend: None,
            tun_offload_enabled: false,
        }
    }

    /// Check and create TUN device node if necessary on Linux systems
    #[cfg(target_os = "linux")]
    async fn ensure_tun_device_node() {
        const TUN_DEV_PATH: &str = "/dev/net/tun";
        const TUN_DIR_PATH: &str = "/dev/net";

        // Check if /dev/net/tun already exists
        if tokio::fs::metadata(TUN_DEV_PATH).await.is_ok() {
            tracing::debug!("TUN device node {} already exists", TUN_DEV_PATH);
            return;
        }

        tracing::info!(
            "TUN device node {} not found, attempting to create",
            TUN_DEV_PATH
        );

        // Check if TUN kernel module is available
        let tun_module_available = tokio::fs::metadata("/proc/net/dev").await.is_ok()
            && (tokio::fs::read_to_string("/proc/modules").await)
                .map(|content| content.contains("tun"))
                .unwrap_or(false);

        if !tun_module_available {
            log::warn!("TUN kernel module may not be available.");
            log::warn!("\tYou may need to load it with: sudo modprobe tun.");
        }

        // Try to create /dev/net directory if it doesn't exist
        if tokio::fs::metadata(TUN_DIR_PATH).await.is_err() {
            if let Err(error) = tokio::fs::create_dir_all(TUN_DIR_PATH).await {
                log::warn!(
                    ?error,
                    "Failed to create directory {}. TUN device creation may fail. Continuing anyway.",
                    TUN_DIR_PATH
                );
                log::warn!(
                    "\tYou may need to run with root privileges or manually create the TUN device."
                );
                Self::print_troubleshooting_info();
                return;
            }
            tracing::info!("Created directory {}", TUN_DIR_PATH);
        }

        // Try to create the TUN device node
        // Major number 10, minor number 200 for /dev/net/tun
        let dev_node = nix::sys::stat::makedev(10, 200);

        match nix::sys::stat::mknod(
            TUN_DEV_PATH,
            nix::sys::stat::SFlag::S_IFCHR,
            nix::sys::stat::Mode::from_bits(0o600).unwrap(),
            dev_node,
        ) {
            Ok(_) => {
                log::info!("Successfully created TUN device node {}", TUN_DEV_PATH);
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    "Failed to create TUN device node {}. Continuing anyway.",
                    TUN_DEV_PATH,
                );
                Self::print_troubleshooting_info();
            }
        }
    }

    /// Print troubleshooting information for TUN device issues
    #[cfg(target_os = "linux")]
    fn print_troubleshooting_info() {
        log::info!(
            "Possible solutions:\
            \n\t1. Run with root privileges: sudo ./easytier-core [options]\
            \n\t2. Manually create TUN device: sudo mkdir -p /dev/net && sudo mknod /dev/net/tun c 10 200\
            \n\t3. Load TUN kernel module: sudo modprobe tun\
            \n\t4. Use --no-tun flag if TUN functionality is not needed\
            \n\t5. Check if your system/container supports TUN devices\
            \nNote: TUN functionality may still work if the kernel supports dynamic device creation."
        );
    }

    /// For non-Linux systems, this is a no-op
    #[cfg(not(target_os = "linux"))]
    async fn ensure_tun_device_node() -> Result<(), Error> {
        Ok(())
    }

    /// FreeBSD specific: Rename a TUN interface
    #[cfg(target_os = "freebsd")]
    async fn rename_tun_interface(old_name: &str, new_name: &str) -> Result<(), Error> {
        let output = tokio::process::Command::new("ifconfig")
            .arg(old_name)
            .arg("name")
            .arg(new_name)
            .output()
            .await?;

        if output.status.success() {
            tracing::info!(
                "Successfully renamed interface {} to {}",
                old_name,
                new_name
            );
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(
                "Failed to rename interface {} to {}: {}",
                old_name,
                new_name,
                stderr
            );
            // Return Ok even if rename fails, as it's not critical
            Ok(())
        }
    }

    /// FreeBSD specific: List all TUN interface names
    #[cfg(target_os = "freebsd")]
    async fn list_tun_names() -> Result<Vec<String>, Error> {
        let output = tokio::process::Command::new("ifconfig")
            .arg("-g")
            .arg("tun")
            .output()
            .await?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let tun_names: Vec<String> = stdout
                .trim()
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
            tracing::debug!("Found TUN interfaces: {:?}", tun_names);
            Ok(tun_names)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("Failed to list TUN interfaces: {}", stderr);
            Ok(Vec::new())
        }
    }

    /// FreeBSD specific: Get interface information
    #[cfg(target_os = "freebsd")]
    async fn get_interface_info(ifname: &str) -> Result<String, Error> {
        let output = tokio::process::Command::new("ifconfig")
            .arg("-v")
            .arg(ifname)
            .output()
            .await?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(
                anyhow::anyhow!("Failed to get interface details for {}: {}", ifname, stderr)
                    .into(),
            )
        }
    }

    /// FreeBSD specific: Extract original name from interface information
    #[cfg(target_os = "freebsd")]
    fn extract_original_name(ifinfo: &str) -> Option<String> {
        ifinfo
            .lines()
            .find(|line| line.trim().starts_with("drivername:"))
            .and_then(|line| line.trim().split_whitespace().nth(1))
            .map(|name| name.to_string())
    }

    /// FreeBSD specific: Check if interface is used by any process
    #[cfg(target_os = "freebsd")]
    fn is_interface_used(ifinfo: &str) -> bool {
        ifinfo.contains("Opened by PID")
    }

    /// FreeBSD specific: Restore TUN interface name to its original value
    #[cfg(target_os = "freebsd")]
    async fn restore_tun_name(dev_name: &str) -> Result<(), Error> {
        let tun_names = Self::list_tun_names().await?;

        // Check if desired dev_name is in use
        if tun_names.iter().any(|name| name == dev_name) {
            tracing::debug!(
                "Desired dev_name {} is in TUN interfaces list, checking if it can be renamed",
                dev_name
            );

            let ifinfo = Self::get_interface_info(dev_name).await?;

            // Check if interface is not occupied
            if !Self::is_interface_used(&ifinfo) {
                // Extract original name
                if let Some(orig_name) = Self::extract_original_name(&ifinfo) {
                    if orig_name != dev_name {
                        tracing::info!(
                            "Restoring dev_name {} to original name {}",
                            dev_name,
                            orig_name
                        );
                        // Rename interface
                        Self::rename_tun_interface(dev_name, &orig_name).await?;
                    }
                }
            } else {
                tracing::debug!(
                    "Interface {} is opened by a process, skipping rename",
                    dev_name
                );
            }
        }

        Ok(())
    }

    async fn create_tun(&self, configure_up: bool) -> Result<tun::platform::Device, Error> {
        let mut config = Configuration::default();
        config.layer(Layer::L3);

        // FreeBSD specific: Check and restore TUN interfaces before creating new one
        #[cfg(target_os = "freebsd")]
        {
            let dev_name = self.global_ctx.get_flags().dev_name;

            if !dev_name.is_empty() {
                // Restore TUN interface name if needed, ignoring errors as it's not critical
                let _ = Self::restore_tun_name(&dev_name).await;
            }
        }

        #[cfg(target_os = "linux")]
        {
            // Check and create TUN device node if necessary (Linux only)
            Self::ensure_tun_device_node().await;

            let dev_name = self.global_ctx.get_flags().dev_name;
            if !dev_name.is_empty() {
                config.tun_name(&dev_name);
            }
        }

        #[cfg(all(target_os = "macos", not(feature = "macos-ne")))]
        config.platform_config(|config| {
            // disable packet information so we can process the header by ourselves, see tun2 impl for more details
            config.packet_information(false);
        });

        #[cfg(target_os = "windows")]
        {
            let dev_name = self.global_ctx.get_flags().dev_name;

            match crate::arch::windows::add_self_to_firewall_allowlist() {
                Ok(_) => tracing::info!("add_self_to_firewall_allowlist successful!"),
                Err(error) => {
                    log::warn!(%error, "Failed to add Easytier to firewall allowlist, Subnet proxy and KCP proxy may not work properly.");
                    log::warn!(
                        "You can add firewall rules manually, or use --use-smoltcp to run with user-space TCP/IP stack."
                    );
                }
            }

            match RegistryManager::reg_delete_obsoleted_items(&dev_name) {
                Ok(_) => tracing::trace!("delete successful!"),
                Err(e) => tracing::error!("An error occurred: {}", e),
            }

            if !dev_name.is_empty() {
                config.tun_name(&dev_name);
            } else {
                use rand::distributions::Distribution as _;
                let c = crate::arch::windows::interface_count()?;
                let mut rng = rand::thread_rng();
                let s: String = rand::distributions::Alphanumeric
                    .sample_iter(&mut rng)
                    .take(4)
                    .map(char::from)
                    .collect::<String>()
                    .to_lowercase();

                let random_dev_name = format!("et_{}_{}", c, s);
                config.tun_name(random_dev_name.clone());

                let mut flags = self.global_ctx.get_flags();
                flags.dev_name = random_dev_name.clone();
                self.global_ctx.set_flags(flags);
            }

            config.platform_config(|config| {
                config.skip_config(true);
                config.ring_cap(Some(std::cmp::min(
                    config.min_ring_cap() * 32,
                    config.max_ring_cap(),
                )));
            });
        }

        if configure_up {
            config.up();
        }
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        if !configure_up {
            config.platform_config(|platform| {
                platform.ensure_root_privileges(false);
            });
        }

        let _g = self.global_ctx.net_ns.guard();
        Ok(tun::create(&config)?)
    }

    #[cfg(mobile)]
    pub async fn create_dev_for_mobile(
        &mut self,
        tun_fd: std::os::fd::RawFd,
    ) -> Result<Box<dyn Tunnel>, Error> {
        log::debug!(%tun_fd);
        let mut config = Configuration::default();
        config.layer(Layer::L3);

        #[cfg(any(target_os = "ios", all(target_os = "macos", feature = "macos-ne")))]
        config.platform_config(|config| {
            // disable packet information so we can process the header by ourselves, see tun2 impl for more details
            config.packet_information(false);
        });

        config.raw_fd(tun_fd);
        config.close_fd_on_drop(false);
        config.up();

        let has_packet_info = cfg!(any(
            target_os = "ios",
            all(target_os = "macos", feature = "macos-ne")
        ));
        let dev = tun::create(&config)?;
        let dev = AsyncDevice::new(dev)?;
        let (a, b) = BiLock::new(dev);
        let ft = TunnelWrapper::new(
            TunStream::new(a, has_packet_info),
            FramedWriter::new_with_converter(
                TunAsyncWrite { l: b },
                TunZCPacketToBytes::new(has_packet_info),
            ),
            None,
        );

        self.ifname = Some(format!("tunfd_{}", tun_fd));

        Ok(Box::new(ft))
    }

    async fn finish_tun_device(
        &mut self,
        dev: tun::platform::Device,
    ) -> Result<Box<dyn Tunnel>, Error> {
        #[cfg(not(target_os = "freebsd"))]
        let ifname = dev.tun_name()?;

        #[cfg(target_os = "freebsd")]
        let mut ifname = dev.tun_name()?;
        self.ifcfg.wait_interface_show(ifname.as_str()).await?;

        // FreeBSD TUN interface rename functionality
        #[cfg(target_os = "freebsd")]
        {
            let dev_name = self.global_ctx.get_flags().dev_name;

            if !dev_name.is_empty() && dev_name != ifname {
                // Use ifconfig to rename the TUN interface
                if Self::rename_tun_interface(&ifname, &dev_name).await.is_ok() {
                    ifname = dev_name;
                }
            }
        }

        #[cfg(target_os = "windows")]
        {
            if let Ok(guid) = RegistryManager::find_interface_guid(&ifname) {
                if let Err(e) = RegistryManager::disable_dynamic_updates(&guid) {
                    tracing::error!(
                        "Failed to disable dhcp for interface {} {}: {}",
                        ifname,
                        guid,
                        e
                    );
                }

                // Disable NetBIOS over TCP/IP
                if let Err(e) = RegistryManager::disable_netbios(&guid) {
                    tracing::error!(
                        "Failed to disable netbios for interface {} {}: {}",
                        ifname,
                        guid,
                        e
                    );
                }
            }
        }

        let dev = AsyncDevice::new(dev)?;

        let flags = self.global_ctx.config.get_flags();
        let mut mtu_in_config = flags.mtu;
        if flags.enable_encryption {
            mtu_in_config -= 20;
        }
        {
            // set mtu by ourselves, rust-tun does not handle it correctly on windows
            let _g = self.global_ctx.net_ns.guard();
            self.ifcfg.set_mtu(ifname.as_str(), mtu_in_config).await?;
        }

        let has_packet_info = cfg!(all(target_os = "macos", not(feature = "macos-ne")));
        let (a, b) = BiLock::new(dev);
        let ft = TunnelWrapper::new(
            TunStream::new(a, has_packet_info),
            FramedWriter::new_with_converter(
                TunAsyncWrite { l: b },
                TunZCPacketToBytes::new(has_packet_info),
            ),
            None,
        );

        self.ifname = Some(ifname.to_owned());

        #[cfg(target_os = "windows")]
        {
            // Add firewall rules for virtual NIC interface to allow all traffic
            match crate::arch::windows::add_interface_to_firewall_allowlist(&ifname) {
                Ok(_) => {
                    tracing::info!(
                        "Successfully configured Windows Firewall for interface: {}",
                        ifname
                    );
                    tracing::info!(
                        "All protocols (TCP/UDP/ICMP) are now allowed on interface: {}",
                        ifname
                    );
                }
                Err(error) => {
                    log::warn!(%error, "Failed to configure Windows Firewall for interface {}\
                    \n\tThis may cause connectivity issues with ping and other network functions.\
                    \n\tPlease run as Administrator or manually configure Windows Firewall.\
                    \n\tAlternatively, you can disable Windows Firewall for testing purposes.", ifname);
                }
            }
        }

        Ok(Box::new(ft))
    }

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    async fn create_preferred_tun(&mut self, configure_up: bool) -> Result<Box<dyn Tunnel>, Error> {
        let flags = self.global_ctx.config.get_flags();
        let mut mtu = flags.mtu;
        if flags.enable_encryption {
            mtu -= 20;
        }
        let name = (!flags.dev_name.is_empty()).then_some(flags.dev_name.as_str());
        let offload_result = {
            let _guard = self.global_ctx.net_ns.guard();
            super::linux_tun_offload::create(name, mtu, configure_up)
        };

        match offload_result {
            Ok((ifname, stream, sink)) => {
                self.ifcfg.wait_interface_show(&ifname).await?;
                self.ifname = Some(ifname);
                self.tun_offload_enabled = true;
                tracing::info!("Linux TUN GSO/GRO offload enabled");
                Ok(Box::new(TunnelWrapper::new(stream, sink, None)))
            }
            Err(error) => {
                tracing::warn!(
                    ?error,
                    "Linux TUN offload unavailable; falling back to legacy TUN"
                );
                self.tun_offload_enabled = false;
                let dev = self.create_tun(configure_up).await?;
                self.finish_tun_device(dev).await
            }
        }
    }

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    fn tun_create_allows_auto_fallback(error: &Error) -> bool {
        let Error::TunError(tun::Error::Io(error)) = error else {
            return false;
        };
        matches!(
            error.raw_os_error(),
            Some(
                nix::libc::EPERM
                    | nix::libc::EACCES
                    | nix::libc::ENOENT
                    | nix::libc::ENODEV
                    | nix::libc::ENXIO
                    | nix::libc::EOPNOTSUPP
            )
        )
    }

    #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
    async fn create_veth_dev(&mut self) -> Result<Box<dyn Tunnel>, Error> {
        let flags = self.global_ctx.config.get_flags();
        let mut mtu = flags.mtu;
        if flags.enable_encryption {
            mtu -= 20;
        }
        let created = super::linux_veth::create(self.global_ctx.clone(), mtu).await?;
        self.ifname = Some(created.ifname);
        self.ifcfg = created.ifcfg;
        Ok(Box::new(TunnelWrapper::new(
            created.stream,
            created.sink,
            None,
        )))
    }

    pub async fn create_dev(&mut self) -> Result<Box<dyn Tunnel>, Error> {
        #[cfg(all(target_os = "linux", not(target_env = "ohos")))]
        {
            let requested = self
                .global_ctx
                .resolved_nic_backend()
                .unwrap_or_else(|| self.global_ctx.requested_nic_backend());
            match requested {
                NicBackend::Tun => {
                    let tunnel = self.create_preferred_tun(true).await?;
                    self.pending_backend = Some(NicBackend::Tun);
                    Ok(tunnel)
                }
                NicBackend::Veth => {
                    let tunnel = self.create_veth_dev().await?;
                    self.pending_backend = Some(NicBackend::Veth);
                    Ok(tunnel)
                }
                NicBackend::Auto => match self.create_preferred_tun(false).await {
                    Ok(tunnel) => {
                        self.pending_backend = Some(NicBackend::Tun);
                        Ok(tunnel)
                    }
                    Err(tun_error) if Self::tun_create_allows_auto_fallback(&tun_error) => {
                        tracing::warn!(
                            error = %tun_error,
                            "TUN creation is unavailable; trying native veth backend"
                        );
                        match self.create_veth_dev().await {
                            Ok(tunnel) => {
                                self.pending_backend = Some(NicBackend::Veth);
                                Ok(tunnel)
                            }
                            Err(veth_error) => Err(anyhow::anyhow!(
                                "both NIC backends failed; tun: {tun_error}; veth: {veth_error}"
                            )
                            .into()),
                        }
                    }
                    Err(error) => Err(error),
                },
            }
        }

        #[cfg(not(all(target_os = "linux", not(target_env = "ohos"))))]
        {
            let dev = self.create_tun(true).await?;
            let tunnel = self.finish_tun_device(dev).await?;
            self.pending_backend = Some(NicBackend::Tun);
            Ok(tunnel)
        }
    }

    pub fn ifname(&self) -> &str {
        self.ifname.as_ref().unwrap().as_str()
    }

    pub async fn link_up(&self) -> Result<(), Error> {
        let _g = self.runtime_netns_guard();
        self.ifcfg.set_link_status(self.ifname(), true).await?;
        Ok(())
    }

    pub async fn add_route(&self, address: Ipv4Addr, cidr: u8) -> Result<(), Error> {
        let _g = self.runtime_netns_guard();
        self.ifcfg
            .add_ipv4_route(self.ifname(), address, cidr, None)
            .await?;
        Ok(())
    }

    pub async fn add_ipv6_route(&self, address: Ipv6Addr, cidr: u8) -> Result<(), Error> {
        self.add_ipv6_route_with_cost(address, cidr, None).await
    }

    pub async fn add_ipv6_route_with_cost(
        &self,
        address: Ipv6Addr,
        cidr: u8,
        cost: Option<i32>,
    ) -> Result<(), Error> {
        let _g = self.runtime_netns_guard();
        self.ifcfg
            .add_ipv6_route(self.ifname(), address, cidr, cost)
            .await?;
        Ok(())
    }

    pub async fn remove_ipv6_route(&self, address: Ipv6Addr, cidr: u8) -> Result<(), Error> {
        let _g = self.runtime_netns_guard();
        self.ifcfg
            .remove_ipv6_route(self.ifname(), address, cidr)
            .await?;
        Ok(())
    }

    pub async fn remove_ip(&self, ip: Option<Ipv4Inet>) -> Result<(), Error> {
        let _g = self.runtime_netns_guard();
        self.ifcfg.remove_ip(self.ifname(), ip).await?;
        Ok(())
    }

    pub async fn remove_ipv6(&self, ip: Option<Ipv6Inet>) -> Result<(), Error> {
        let _g = self.runtime_netns_guard();
        self.ifcfg.remove_ipv6(self.ifname(), ip).await?;
        Ok(())
    }

    pub async fn add_ip(&self, ip: Ipv4Addr, cidr: i32) -> Result<(), Error> {
        let _g = self.runtime_netns_guard();
        self.ifcfg
            .add_ipv4_ip(self.ifname(), ip, cidr as u8)
            .await?;
        Ok(())
    }

    pub async fn add_ipv6(&self, ip: Ipv6Addr, cidr: i32) -> Result<(), Error> {
        let _g = self.runtime_netns_guard();
        self.ifcfg
            .add_ipv6_ip(self.ifname(), ip, cidr as u8)
            .await?;
        Ok(())
    }

    pub fn get_ifcfg(&self) -> Arc<dyn IfConfiguerTrait + Send + Sync + 'static> {
        self.ifcfg.clone()
    }

    pub fn commit_backend(&mut self) -> Result<(), Error> {
        let Some(backend) = self.pending_backend.take() else {
            return Err(anyhow::anyhow!("NIC backend was not initialized").into());
        };
        if let Some(committed) = self.global_ctx.resolved_nic_backend() {
            if committed != backend {
                return Err(anyhow::anyhow!(
                    "NIC backend changed from {committed:?} to {backend:?}"
                )
                .into());
            }
            return Ok(());
        }
        self.global_ctx
            .commit_nic_backend(backend)
            .map_err(|_| anyhow::anyhow!("NIC backend was committed concurrently"))?;
        Ok(())
    }
}

pub struct NicCtx {
    global_ctx: ArcGlobalCtx,
    peer_mgr: Weak<PeerManager>,
    peer_packet_receiver: Arc<Mutex<PacketRecvChanReceiver>>,

    close_notifier: Arc<Notify>,

    nic: Arc<Mutex<VirtualNic>>,
    tasks: JoinSet<()>,

    #[cfg(all(feature = "leaf-policy-proxy", unix))]
    policy: Option<PolicyNicContext>,
    #[cfg(all(feature = "leaf-policy-proxy", unix))]
    policy_data_plane: Weak<Socks5Server>,
    #[cfg(all(feature = "leaf-policy-mobile", target_os = "android"))]
    mobile_network_updates: tokio::sync::watch::Receiver<crate::launcher::MobileNetworkState>,

    #[cfg(target_os = "windows")]
    windows_udp_broadcast_relay: Option<AbortOnDropHandle<()>>,
}

impl NicCtx {
    pub fn new(
        global_ctx: ArcGlobalCtx,
        peer_manager: &Arc<PeerManager>,
        peer_packet_receiver: Arc<Mutex<PacketRecvChanReceiver>>,
        close_notifier: Arc<Notify>,
        #[cfg(all(feature = "leaf-policy-proxy", unix))] policy_data_plane: Weak<Socks5Server>,
        #[cfg(all(feature = "leaf-policy-mobile", target_os = "android"))]
        mobile_network_updates: tokio::sync::watch::Receiver<
            crate::launcher::MobileNetworkState,
        >,
    ) -> Self {
        NicCtx {
            global_ctx: global_ctx.clone(),
            peer_mgr: Arc::downgrade(peer_manager),
            peer_packet_receiver,

            close_notifier,

            nic: Arc::new(Mutex::new(VirtualNic::new(global_ctx))),
            tasks: JoinSet::new(),

            #[cfg(all(feature = "leaf-policy-proxy", unix))]
            policy: None,
            #[cfg(all(feature = "leaf-policy-proxy", unix))]
            policy_data_plane,
            #[cfg(all(feature = "leaf-policy-mobile", target_os = "android"))]
            mobile_network_updates,

            #[cfg(target_os = "windows")]
            windows_udp_broadcast_relay: None,
        }
    }

    pub async fn ifname(&self) -> Option<String> {
        let nic = self.nic.lock().await;
        nic.ifname.as_ref().map(|s| s.to_owned())
    }

    pub async fn assign_ipv4_to_tun_device(&self, ipv4_addr: cidr::Ipv4Inet) -> Result<(), Error> {
        let nic = self.nic.lock().await;
        nic.link_up().await?;
        nic.remove_ip(None).await?;
        nic.add_ip(ipv4_addr.address(), ipv4_addr.network_length() as i32)
            .await?;
        #[cfg(any(
            all(target_os = "macos", not(feature = "macos-ne")),
            target_os = "freebsd"
        ))]
        {
            nic.add_route(ipv4_addr.first_address(), ipv4_addr.network_length())
                .await?;
        }
        Ok(())
    }

    pub async fn assign_ipv6_to_tun_device(&self, ipv6_addr: cidr::Ipv6Inet) -> Result<(), Error> {
        let nic = self.nic.lock().await;
        nic.link_up().await?;
        nic.remove_ipv6(None).await?;
        nic.add_ipv6(ipv6_addr.address(), ipv6_addr.network_length() as i32)
            .await?;
        #[cfg(any(
            all(target_os = "macos", not(feature = "macos-ne")),
            target_os = "freebsd"
        ))]
        {
            nic.add_ipv6_route(ipv6_addr.first_address(), ipv6_addr.network_length())
                .await?;
        }
        Ok(())
    }

    async fn do_forward_nic_to_peers_ipv4(ret: ZCPacket, mgr: &PeerManager) {
        if let Some(ipv4) = Ipv4Packet::new(ret.payload()) {
            if ipv4.get_version() != 4 {
                tracing::info!("[USER_PACKET] not ipv4 packet: {:?}", ipv4);
                return;
            }
            let dst_ipv4 = ipv4.get_destination();
            let src_ipv4 = ipv4.get_source();
            let my_ipv4 = mgr.get_global_ctx().get_ipv4().map(|x| x.address());
            tracing::trace!(
                ?ret,
                ?src_ipv4,
                ?dst_ipv4,
                "[USER_PACKET] recv new packet from tun device and forward to peers."
            );

            // Subnet A is proxied as 10.0.0.0/24, and Subnet B is also proxied as 10.0.0.0/24.
            //
            // Subnet A has received a route advertised by Subnet B. As a result, A can reach
            // the physical subnet 10.0.0.0/24 directly and has also added a virtual route for
            // the same subnet 10.0.0.0/24. However, the physical route has a higher priority
            // (lower metric) than the virtual one.
            //
            // When A sends a UDP packet to a non-existent IP within this subnet, the packet
            // cannot be delivered on the physical network and is instead routed to the virtual
            // network interface.
            //
            // The virtual interface receives the packet and forwards it to itself, which triggers
            // the subnet proxy logic. The subnet proxy then attempts to send another packet to
            // the same destination address, causing the same process to repeat and creating an
            // infinite loop. Therefore, we must avoid re-sending packets back to ourselves
            // when the subnet proxy itself is the originator of the packet.
            //
            // However, there is a special scenario to consider: when A acts as a gateway,
            // packets from devices behind A may be forwarded by the OS to the ET (e.g., an
            // eBPF or tunneling component), which happens to proxy the subnet. In this case,
            // the packet’s source IP is not A’s own IP, and we must allow such packets to be
            // sent to the virtual interface (i.e., "sent to ourselves") to maintain correct
            // forwarding behavior. Thus, loop prevention should only apply when the source IP
            // belongs to the local host.
            let send_ret = mgr
                .send_msg_by_ip(ret, IpAddr::V4(dst_ipv4), Some(src_ipv4) == my_ipv4)
                .await;
            if send_ret.is_err() {
                tracing::trace!(?send_ret, "[USER_PACKET] send_msg failed")
            }
        } else {
            tracing::warn!(?ret, "[USER_PACKET] not ipv4 packet");
        }
    }

    async fn do_forward_nic_to_peers_ipv6(ret: ZCPacket, mgr: &PeerManager) {
        if let Some(ipv6) = Ipv6Packet::new(ret.payload()) {
            if ipv6.get_version() != 6 {
                tracing::info!("[USER_PACKET] not ipv6 packet: {:?}", ipv6);
                return;
            }
            let src_ipv6 = ipv6.get_source();
            let dst_ipv6 = ipv6.get_destination();
            let is_local_src = mgr.get_global_ctx().is_ip_local_ipv6(&src_ipv6);
            tracing::trace!(
                ?ret,
                ?src_ipv6,
                ?dst_ipv6,
                "[USER_PACKET] recv new packet from tun device and forward to peers."
            );

            if src_ipv6.is_unicast_link_local() && !is_local_src {
                // do not route link local packet to other nodes unless the address is assigned by user
                return;
            }

            // TODO: use zero-copy
            let send_ret = mgr
                .send_msg_by_ip(ret, IpAddr::V6(dst_ipv6), is_local_src)
                .await;
            if send_ret.is_err() {
                tracing::trace!(?send_ret, "[USER_PACKET] send_msg failed")
            }
        } else {
            tracing::warn!(?ret, "[USER_PACKET] not ipv6 packet");
        }
    }

    async fn do_forward_nic_to_peers(ret: ZCPacket, mgr: &PeerManager) {
        let payload = ret.payload();
        if payload.is_empty() {
            return;
        }

        match payload[0] >> 4 {
            4 => Self::do_forward_nic_to_peers_ipv4(ret, mgr).await,
            6 => Self::do_forward_nic_to_peers_ipv6(ret, mgr).await,
            _ => {
                tracing::warn!(?ret, "[USER_PACKET] unknown IP version");
            }
        }
    }

    fn do_forward_nic_to_peers_task(
        &mut self,
        mut stream: Pin<Box<dyn ZCPacketStream>>,
        #[cfg(all(feature = "leaf-policy-proxy", unix))] policy: Option<PolicyForwardingContext>,
    ) -> Result<(), Error> {
        // read from nic and write to corresponding tunnel
        let Some(mgr) = self.peer_mgr.upgrade() else {
            return Err(anyhow::anyhow!("peer manager not available").into());
        };
        #[cfg(all(feature = "leaf-policy-proxy", unix))]
        let policy = policy.map(|(classifier, bridge_slot, dropped_packets)| {
            // Decouple TUN reads from Leaf's packet socket without making the queue
            // unbounded. A full queue remains fail-closed, while ordinary socket
            // backpressure no longer turns a short scheduling delay into packet loss.
            const POLICY_LEAF_WRITER_CAPACITY: usize = 4_096;
            let (writer_tx, mut writer_rx) =
                tokio::sync::mpsc::channel::<PolicyLeafPacket>(POLICY_LEAF_WRITER_CAPACITY);
            let writer_bridge_slot = bridge_slot.clone();
            let writer_dropped_packets = dropped_packets.clone();
            self.tasks.spawn(async move {
                while let Some(queued) = writer_rx.recv().await {
                    let Some(current_bridge) = writer_bridge_slot.load_full() else {
                        log_policy_drop(&writer_dropped_packets, "Leaf is unavailable");
                        continue;
                    };
                    if !Arc::ptr_eq(&current_bridge, &queued.bridge) {
                        log_policy_drop(&writer_dropped_packets, "Leaf runtime generation changed");
                        continue;
                    }
                    if queued
                        .bridge
                        .send_to_leaf(queued.packet.payload())
                        .await
                        .is_err()
                    {
                        log_policy_drop(&writer_dropped_packets, "Leaf input queue is unavailable");
                    }
                }
            });
            (classifier, bridge_slot, dropped_packets, writer_tx)
        });
        let close_notifier = self.close_notifier.clone();
        self.tasks.spawn(async move {
            while let Some(ret) = stream.next().await {
                if ret.is_err() {
                    tracing::error!("read from nic failed: {:?}", ret);
                    break;
                }
                let ret = ret.unwrap();
                #[cfg(all(feature = "leaf-policy-proxy", unix))]
                if let Some((classifier, bridge_slot, dropped_packets, writer_tx)) = &policy {
                    match classifier.classify(ret.payload()) {
                        Ok(PacketClass::Policy) => {
                            let Some(bridge) = bridge_slot.load_full() else {
                                log_policy_drop(dropped_packets, "Leaf is unavailable");
                                continue;
                            };
                            match writer_tx.try_send(PolicyLeafPacket {
                                bridge,
                                packet: ret,
                            }) {
                                Ok(()) => {}
                                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                    log_policy_drop(dropped_packets, "Leaf writer queue is full");
                                }
                                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                    log_policy_drop(dropped_packets, "Leaf writer is unavailable");
                                }
                            }
                            continue;
                        }
                        Ok(PacketClass::Mesh) => {}
                        Err(error) => {
                            tracing::warn!(?error, "dropping malformed policy TUN packet");
                            continue;
                        }
                    }
                }
                Self::do_forward_nic_to_peers(ret, mgr.as_ref()).await;
            }
            close_notifier.notify_one();
            tracing::error!("nic closed when recving from it");
        });

        Ok(())
    }

    fn do_forward_peers_to_nic(&mut self, sink: Pin<Box<dyn ZCPacketSink>>) {
        self.do_forward_peers_to_nic_with_mode(sink, false);
    }

    fn do_forward_peers_to_nic_with_mode(
        &mut self,
        mut sink: Pin<Box<dyn ZCPacketSink>>,
        offload: bool,
    ) {
        let channel = self.peer_packet_receiver.clone();
        let close_notifier = self.close_notifier.clone();
        self.tasks.spawn(async move {
            // unlock until coroutine finished
            let mut channel = channel.lock().await;
            while let Ok(packet) = recv_packet_from_chan(&mut channel).await {
                tracing::trace!(
                    "[USER_PACKET] forward packet from peers to nic. packet: {:?}",
                    packet
                );
                let ret = if offload {
                    let mut result = sink.feed(packet).await;
                    let mut count = 1usize;
                    while result.is_ok() && count < 64 {
                        match channel.try_recv() {
                            Ok(packet) => {
                                result = sink.feed(packet).await;
                                count += 1;
                            }
                            Err(_) => break,
                        }
                    }
                    if result.is_ok() {
                        result = sink.flush().await;
                    }
                    result
                } else {
                    sink.send(packet).await
                };
                if ret.is_err() {
                    tracing::error!(?ret, "do_forward_tunnel_to_nic sink error");
                }
            }
            close_notifier.notify_one();
            tracing::error!("nic closed when sending to it");
        });
    }

    #[cfg(all(feature = "leaf-policy-proxy", unix))]
    fn do_forward_peers_and_policy_to_nic(
        &mut self,
        mut sink: Pin<Box<dyn ZCPacketSink>>,
        mut bridge_updates: tokio::sync::watch::Receiver<Option<Arc<LeafPacketBridge>>>,
        offload: bool,
    ) {
        const PEER_WRITER_CAPACITY: usize = 1024;
        const POLICY_WRITER_CAPACITY: usize = 256;
        const MAX_BATCH: usize = 64;

        let (peer_tx, mut peer_rx) = tokio::sync::mpsc::channel(PEER_WRITER_CAPACITY);
        let (policy_tx, mut policy_rx) = tokio::sync::mpsc::channel(POLICY_WRITER_CAPACITY);
        let channel = self.peer_packet_receiver.clone();
        self.tasks.spawn(async move {
            let mut channel = channel.lock().await;
            while let Ok(packet) = recv_packet_from_chan(&mut channel).await {
                if peer_tx.send(packet).await.is_err() {
                    break;
                }
            }
        });

        self.tasks.spawn(async move {
            let mut packet = vec![0u8; u16::MAX as usize];
            let mut dropped_packets = 0u64;
            loop {
                let current = bridge_updates.borrow().clone();
                let Some(bridge) = current else {
                    if bridge_updates.changed().await.is_err() {
                        break;
                    }
                    continue;
                };
                tokio::select! {
                    changed = bridge_updates.changed() => {
                        if changed.is_err() {
                            break;
                        }
                    }
                    result = bridge.recv_from_leaf(&mut packet) => {
                        match result {
                            Ok(0) => {}
                            Ok(length) => {
                                let still_current = bridge_updates
                                    .borrow()
                                    .as_ref()
                                    .is_some_and(|active| Arc::ptr_eq(active, &bridge));
                                if !still_current {
                                    continue;
                                }
                                match policy_tx.try_send(ZCPacket::new_with_payload(&packet[..length])) {
                                    Ok(()) => {}
                                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                                        dropped_packets = dropped_packets.saturating_add(1);
                                        if dropped_packets.is_power_of_two() {
                                            tracing::warn!(dropped_packets, "dropping policy packets because TUN writer queue is full");
                                        }
                                    }
                                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => break,
                                }
                            }
                            Err(error) => {
                                tracing::warn!(?error, "Leaf packet bridge closed; waiting for replacement");
                                if bridge_updates.changed().await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        });

        let close_notifier = self.close_notifier.clone();
        self.tasks.spawn(async move {
            let mut peer_open = true;
            let mut policy_open = true;
            while peer_open || policy_open {
                let selected = tokio::select! {
                    packet = peer_rx.recv(), if peer_open => {
                        match packet {
                            Some(packet) => Some((packet, false)),
                            None => { peer_open = false; None }
                        }
                    }
                    packet = policy_rx.recv(), if policy_open => {
                        match packet {
                            Some(packet) => Some((packet, true)),
                            None => { policy_open = false; None }
                        }
                    }
                };
                let Some((packet, from_policy)) = selected else {
                    continue;
                };
                let mut result = sink.feed(packet).await;
                let mut count = 1usize;
                let mut prefer_policy = !from_policy;
                while result.is_ok() && count < MAX_BATCH {
                    let packet = if prefer_policy {
                        policy_rx.try_recv().or_else(|_| peer_rx.try_recv())
                    } else {
                        peer_rx.try_recv().or_else(|_| policy_rx.try_recv())
                    };
                    let Ok(packet) = packet else {
                        break;
                    };
                    result = sink.feed(packet).await;
                    count += 1;
                    prefer_policy = !prefer_policy;
                }
                if result.is_ok() {
                    result = sink.flush().await;
                }
                if let Err(error) = result {
                    tracing::error!(?error, "policy TUN writer failed");
                    break;
                }
                if !offload && count == 1 {
                    tokio::task::yield_now().await;
                }
            }
            close_notifier.notify_one();
        });
    }

    #[cfg(target_os = "windows")]
    fn start_windows_udp_broadcast_relay(&mut self, virtual_ipv4: Ipv4Inet) {
        if !self.global_ctx.get_flags().enable_udp_broadcast_relay {
            return;
        }

        let Some(peer_manager) = self.peer_mgr.upgrade() else {
            tracing::warn!("peer manager is dropped, skip Windows UDP broadcast relay");
            return;
        };

        match super::windows_udp_broadcast::start(peer_manager, virtual_ipv4) {
            Ok(handle) => {
                self.windows_udp_broadcast_relay = Some(handle);
                tracing::info!("Windows UDP broadcast relay started");
            }
            Err(err) => {
                tracing::warn!(
                    ?err,
                    "failed to start Windows UDP broadcast relay; administrator privileges are required"
                );
            }
        }
    }

    async fn apply_route_changes(
        ifcfg: &(impl IfConfiguerTrait + ?Sized),
        ifname: &str,
        net_ns: &crate::common::netns::NetNS,
        cur_proxy_cidrs: &mut BTreeSet<cidr::Ipv4Cidr>,
        added: Vec<cidr::Ipv4Cidr>,
        removed: Vec<cidr::Ipv4Cidr>,
    ) {
        tracing::debug!(?added, ?removed, "applying proxy_cidrs route changes");

        // Remove routes
        for cidr in removed {
            if !cur_proxy_cidrs.contains(&cidr) {
                continue;
            }
            let _g = ifcfg.requires_runtime_netns_guard().then(|| net_ns.guard());
            let ret = ifcfg
                .remove_ipv4_route(ifname, cidr.first_address(), cidr.network_length())
                .await;

            if ret.is_err() {
                tracing::trace!(
                    cidr = ?cidr,
                    err = ?ret,
                    "remove route failed.",
                );
            }
            cur_proxy_cidrs.remove(&cidr);
        }

        // Add routes
        for cidr in added {
            if cur_proxy_cidrs.contains(&cidr) {
                continue;
            }
            let _g = ifcfg.requires_runtime_netns_guard().then(|| net_ns.guard());
            let ret = ifcfg
                .add_ipv4_route(ifname, cidr.first_address(), cidr.network_length(), None)
                .await;

            if ret.is_err() {
                tracing::trace!(
                    cidr = ?cidr,
                    err = ?ret,
                    "add route failed.",
                );
            }
            cur_proxy_cidrs.insert(cidr);
        }
    }

    async fn apply_public_ipv6_route_changes(
        ifcfg: &(impl IfConfiguerTrait + ?Sized),
        ifname: &str,
        net_ns: &crate::common::netns::NetNS,
        cur_routes: &mut BTreeSet<cidr::Ipv6Inet>,
        added: Vec<cidr::Ipv6Inet>,
        removed: Vec<cidr::Ipv6Inet>,
    ) {
        for route in removed {
            if !cur_routes.contains(&route) {
                continue;
            }
            let _g = ifcfg.requires_runtime_netns_guard().then(|| net_ns.guard());
            let ret = ifcfg
                .remove_ipv6_route(ifname, route.address(), route.network_length())
                .await;
            if ret.is_err() {
                tracing::trace!(route = ?route, err = ?ret, "remove public ipv6 route failed");
            }
            cur_routes.remove(&route);
        }

        for route in added {
            if cur_routes.contains(&route) {
                continue;
            }
            let _g = ifcfg.requires_runtime_netns_guard().then(|| net_ns.guard());
            let ret = ifcfg
                .add_ipv6_route(ifname, route.address(), route.network_length(), None)
                .await;
            if ret.is_err() {
                tracing::trace!(route = ?route, err = ?ret, "add public ipv6 route failed");
            } else {
                cur_routes.insert(route);
            }
        }
    }

    async fn run_proxy_cidrs_route_updater(&mut self) -> Result<(), Error> {
        let Some(peer_mgr) = self.peer_mgr.upgrade() else {
            return Err(anyhow::anyhow!("peer manager not available").into());
        };
        let global_ctx = self.global_ctx.clone();
        let net_ns = self.global_ctx.net_ns.clone();
        let nic = self.nic.lock().await;
        let ifcfg = nic.get_ifcfg();
        let ifname = nic.ifname().to_owned();
        let mut event_receiver = global_ctx.subscribe();

        self.tasks.spawn(async move {
            let mut cur_proxy_cidrs = BTreeSet::<cidr::Ipv4Cidr>::new();

            // Initial sync: get current proxy_cidrs state and apply routes
            let (_, added, removed) = ProxyCidrsMonitor::diff_proxy_cidrs(
                peer_mgr.as_ref(),
                &global_ctx,
                &cur_proxy_cidrs,
            )
            .await;
            Self::apply_route_changes(
                ifcfg.as_ref(),
                &ifname,
                &net_ns,
                &mut cur_proxy_cidrs,
                added,
                removed,
            )
            .await;

            loop {
                let event = match event_receiver.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tracing::debug!("event bus closed, stopping proxy_cidrs route updater");
                        break;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        tracing::warn!(
                            "event bus lagged in proxy_cidrs route updater, doing full sync"
                        );
                        event_receiver = event_receiver.resubscribe();
                        // Full sync after lagged to recover consistent state
                        let (_, added, removed) = ProxyCidrsMonitor::diff_proxy_cidrs(
                            peer_mgr.as_ref(),
                            &global_ctx,
                            &cur_proxy_cidrs,
                        )
                        .await;
                        GlobalCtxEvent::ProxyCidrsUpdated(added, removed)
                    }
                };

                // Only handle ProxyCidrsUpdated events
                let (added, removed) = match event {
                    GlobalCtxEvent::ProxyCidrsUpdated(added, removed) => (added, removed),
                    _ => continue,
                };

                Self::apply_route_changes(
                    ifcfg.as_ref(),
                    &ifname,
                    &net_ns,
                    &mut cur_proxy_cidrs,
                    added,
                    removed,
                )
                .await;
            }
        });

        Ok(())
    }

    async fn run_public_ipv6_route_updater(&mut self) -> Result<(), Error> {
        let Some(peer_mgr) = self.peer_mgr.upgrade() else {
            return Err(anyhow::anyhow!("peer manager not available").into());
        };
        let global_ctx = self.global_ctx.clone();
        let net_ns = self.global_ctx.net_ns.clone();
        let nic = self.nic.lock().await;
        let ifcfg = nic.get_ifcfg();
        let ifname = nic.ifname().to_owned();
        let mut event_receiver = global_ctx.subscribe();

        self.tasks.spawn(async move {
            let mut cur_routes = BTreeSet::<cidr::Ipv6Inet>::new();
            let initial_routes = peer_mgr.list_public_ipv6_routes().await;
            let initial_added = initial_routes.iter().copied().collect::<Vec<_>>();
            Self::apply_public_ipv6_route_changes(
                ifcfg.as_ref(),
                &ifname,
                &net_ns,
                &mut cur_routes,
                initial_added,
                Vec::new(),
            )
            .await;

            loop {
                let event = match event_receiver.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        event_receiver = event_receiver.resubscribe();
                        let latest = peer_mgr.list_public_ipv6_routes().await;
                        let added = latest.difference(&cur_routes).copied().collect::<Vec<_>>();
                        let removed = cur_routes.difference(&latest).copied().collect::<Vec<_>>();
                        GlobalCtxEvent::PublicIpv6RoutesUpdated(added, removed)
                    }
                };

                let (added, removed) = match event {
                    GlobalCtxEvent::PublicIpv6RoutesUpdated(added, removed) => (added, removed),
                    _ => continue,
                };

                Self::apply_public_ipv6_route_changes(
                    ifcfg.as_ref(),
                    &ifname,
                    &net_ns,
                    &mut cur_routes,
                    added,
                    removed,
                )
                .await;
            }
        });

        Ok(())
    }

    async fn run_public_ipv6_addr_updater(
        &mut self,
        policy_owns_default_route: bool,
    ) -> Result<(), Error> {
        let Some(peer_mgr) = self.peer_mgr.upgrade() else {
            return Err(anyhow::anyhow!("peer manager not available").into());
        };
        let global_ctx = self.global_ctx.clone();
        let nic = self.nic.clone();
        let mut event_receiver = global_ctx.subscribe();
        self.tasks.spawn(async move {
            let mut current_addr = peer_mgr.get_my_public_ipv6_addr().await;
            if let Some(addr) = current_addr {
                let nic = nic.lock().await;
                if let Err(err) = nic.link_up().await {
                    tracing::warn!(?err, "failed to bring public ipv6 nic link up");
                }
                if let Err(err) = nic.add_ipv6(addr.address(), addr.network_length() as i32).await {
                    tracing::warn!(addr = ?addr, ?err, "failed to add public ipv6 address");
                }
                if !policy_owns_default_route
                    && let Err(err) = nic
                        .add_ipv6_route_with_cost(Ipv6Addr::UNSPECIFIED, 0, Some(5))
                        .await
                {
                    tracing::warn!(route = %Ipv6Addr::UNSPECIFIED, prefix = 0, ?err, "failed to add default public ipv6 route");
                }
            }

            loop {
                let event = match event_receiver.recv().await {
                    Ok(event) => event,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        event_receiver = event_receiver.resubscribe();
                        let latest = peer_mgr.get_my_public_ipv6_addr().await;
                        GlobalCtxEvent::PublicIpv6Changed(current_addr, latest)
                    }
                };

                let (old, new) = match event {
                    GlobalCtxEvent::PublicIpv6Changed(old, new) => (old, new),
                    _ => continue,
                };

                current_addr = new;
                let nic = nic.lock().await;
                if let Err(err) = nic.link_up().await {
                    tracing::warn!(?err, "failed to bring public ipv6 nic link up");
                }
                if let Some(old) = old {
                    if !policy_owns_default_route
                        && let Err(err) = nic.remove_ipv6_route(Ipv6Addr::UNSPECIFIED, 0).await
                    {
                        tracing::warn!(route = %Ipv6Addr::UNSPECIFIED, prefix = 0, ?err, "failed to remove default public ipv6 route");
                    }
                    if let Err(err) = nic.remove_ipv6(Some(old)).await {
                        tracing::warn!(addr = ?old, ?err, "failed to remove old public ipv6 address");
                    }
                }
                if let Some(new) = new {
                    if let Err(err) = nic.add_ipv6(new.address(), new.network_length() as i32).await
                    {
                        tracing::warn!(addr = ?new, ?err, "failed to add public ipv6 address");
                    }
                    if !policy_owns_default_route
                        && let Err(err) = nic
                            .add_ipv6_route_with_cost(Ipv6Addr::UNSPECIFIED, 0, Some(5))
                            .await
                    {
                        tracing::warn!(route = %Ipv6Addr::UNSPECIFIED, prefix = 0, ?err, "failed to add default public ipv6 route");
                    }
                }
            }
        });

        Ok(())
    }

    #[cfg(all(feature = "leaf-policy-proxy", unix))]
    async fn collect_policy_mesh_routes(
        &self,
        ipv4_addr: Option<cidr::Ipv4Inet>,
        ipv6_addr: Option<cidr::Ipv6Inet>,
    ) -> Result<MeshRouteSnapshot, Error> {
        let Some(peer_mgr) = self.peer_mgr.upgrade() else {
            return Err(anyhow::anyhow!("peer manager not available").into());
        };
        Self::collect_policy_mesh_routes_for(
            &self.global_ctx,
            peer_mgr.as_ref(),
            ipv4_addr,
            ipv6_addr,
        )
        .await
    }

    #[cfg(all(feature = "leaf-policy-proxy", unix))]
    async fn collect_policy_mesh_routes_for(
        global_ctx: &ArcGlobalCtx,
        peer_mgr: &PeerManager,
        ipv4_addr: Option<cidr::Ipv4Inet>,
        ipv6_addr: Option<cidr::Ipv6Inet>,
    ) -> Result<MeshRouteSnapshot, Error> {
        let mut routes = Vec::<cidr::IpCidr>::new();
        if let Some(ipv4) = ipv4_addr {
            routes.push(cidr::IpCidr::V4(
                cidr::Ipv4Cidr::new(ipv4.first_address(), ipv4.network_length())
                    .expect("Ipv4Inet always describes a valid network"),
            ));
        }
        if let Some(ipv6) = ipv6_addr {
            routes.push(cidr::IpCidr::V6(
                cidr::Ipv6Cidr::new(ipv6.first_address(), ipv6.network_length())
                    .expect("Ipv6Inet always describes a valid network"),
            ));
        }
        #[cfg(feature = "magic-dns")]
        if global_ctx.get_flags().accept_dns {
            routes.push(cidr::IpCidr::V4(
                cidr::Ipv4Cidr::new(
                    crate::instance::dns_server::MAGIC_DNS_FAKE_IP
                        .parse()
                        .expect("Magic DNS address is valid"),
                    32,
                )
                .expect("Magic DNS host prefix is valid"),
            ));
        }
        let (proxy_v4, _, _) =
            ProxyCidrsMonitor::diff_proxy_cidrs(peer_mgr, global_ctx, &BTreeSet::new()).await;
        routes.extend(proxy_v4.into_iter().map(cidr::IpCidr::V4));
        routes.extend(
            peer_mgr
                .list_proxy_cidrs_v6()
                .await
                .into_iter()
                .map(cidr::IpCidr::V6),
        );
        routes.extend(
            peer_mgr
                .list_public_ipv6_routes()
                .await
                .into_iter()
                .map(|route| {
                    cidr::IpCidr::V6(
                        cidr::Ipv6Cidr::new(route.first_address(), route.network_length())
                            .expect("Ipv6Inet always describes a valid network"),
                    )
                }),
        );
        Ok(MeshRouteSnapshot::new(routes))
    }

    #[cfg(all(feature = "leaf-policy-proxy", target_os = "linux"))]
    async fn start_policy_proxy(
        &mut self,
        ipv4_addr: Option<cidr::Ipv4Inet>,
        ipv6_addr: Option<cidr::Ipv6Inet>,
        config: crate::policy_proxy::PolicyProcessConfig,
        lease: crate::policy_proxy::PolicyInstanceLease,
    ) -> Result<(), Error> {
        if self.global_ctx.get_flags().accept_dns {
            tracing::warn!(
                "Magic DNS remains mesh-owned in policy mode; DOMAIN/GEOSITE rules cannot observe names resolved through Magic DNS"
            );
        }
        if !self.global_ctx.get_flags().bind_device {
            return Err(anyhow::anyhow!(
                "policy mode requires EasyTier bind_device=true to prevent underlay recursion"
            )
            .into());
        }
        if self.global_ctx.net_ns.name().is_some() {
            return Err(anyhow::anyhow!(
                "policy mode does not yet support an instance netns; worker namespace ownership must be explicit"
            )
            .into());
        }
        if self.nic.lock().await.ifname() == config.outbound_interface {
            return Err(anyhow::anyhow!(
                "policy outbound interface cannot be the EasyTier virtual NIC"
            )
            .into());
        }
        let revision = config.revision.clone();
        ensure_policy_mesh_credentials_confidential(&self.global_ctx, &revision)?;
        let data_plane = self
            .policy_data_plane
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("policy data plane is not available"))?;
        let peer_mgr = self
            .peer_mgr
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("peer manager not available"))?;
        let routes = peer_mgr.list_routes().await;
        let classifier = Arc::new(PacketClassifier::new(
            self.collect_policy_mesh_routes(ipv4_addr, ipv6_addr)
                .await?,
        ));

        let routing = Arc::new(Mutex::new({
            let nic = self.nic.lock().await;
            crate::policy_proxy::PolicyRoutingGuard::install(
                &config.outbound_interface,
                nic.ifname(),
                self.global_ctx.get_flags().enable_ipv6,
                self.global_ctx.get_flags().socket_mark,
            )?
        }));

        let bridge = Arc::new(ArcSwapOption::empty());
        let dropped_packets = Arc::new(AtomicU64::new(0));
        let (bridge_updates, _) = tokio::sync::watch::channel(None);
        let active = Arc::new(Mutex::new(None));
        match Self::build_policy_runtime(
            &config,
            revision.clone(),
            data_plane,
            peer_mgr.clone(),
            peer_mgr.my_peer_id(),
            &routes,
        )
        .await
        {
            Ok(candidate) => {
                let candidate_bridge = candidate.bridge.clone();
                bridge.store(Some(candidate_bridge.clone()));
                bridge_updates.send_replace(Some(candidate_bridge));
                tracing::info!(
                    revision = candidate.runtime.revision_id(),
                    policy_source = %config.source_label,
                    outbound_interface = %config.outbound_interface,
                    "transparent policy proxy is ready"
                );
                *active.lock().await = Some(candidate);
            }
            Err(error) => {
                tracing::error!(
                    ?error,
                    policy_source = %config.source_label,
                    "policy runtime is unavailable; mesh remains active and non-mesh traffic is blocked"
                );
            }
        }
        self.policy = Some(PolicyNicContext {
            revision,
            classifier,
            bridge,
            dropped_packets,
            bridge_updates,
            active,
            routing,
            _lease: lease,
        });
        Ok(())
    }

    #[cfg(all(feature = "leaf-policy-proxy", target_os = "linux"))]
    async fn build_policy_runtime(
        config: &crate::policy_proxy::PolicyProcessConfig,
        revision: Arc<easytier_policy::PolicyRevision>,
        data_plane: Arc<Socks5Server>,
        peer_mgr: Arc<PeerManager>,
        self_peer_id: u32,
        routes: &[crate::proto::api::instance::Route],
    ) -> anyhow::Result<PolicyActiveRuntime> {
        let mesh_endpoints = Self::resolve_policy_mesh_endpoints(&revision, self_peer_id, routes)?;
        let mesh_bridges = Arc::new(
            crate::policy_proxy::MeshProxyBridgeSet::start(
                data_plane,
                peer_mgr,
                &revision,
                &mesh_endpoints,
            )
            .await?,
        );
        let runtime = LeafProcessRuntime::start(
            &config.leaf_executable,
            &config.base_dir,
            Some(&config.outbound_interface),
            mesh_bridges.as_ref(),
            revision,
        )
        .await
        .map_err(|error| anyhow::anyhow!(error))?;
        let bridge = runtime.bridge();
        Ok(PolicyActiveRuntime {
            runtime: runtime as Arc<dyn PolicyRuntime>,
            bridge,
            mesh_bridges,
        })
    }

    #[cfg(all(feature = "leaf-policy-proxy", target_os = "linux"))]
    fn run_policy_route_updater(
        &mut self,
        ipv4_addr: Option<cidr::Ipv4Inet>,
        ipv6_addr: Option<cidr::Ipv6Inet>,
        config: crate::policy_proxy::PolicyProcessConfig,
    ) {
        let Some(policy) = self.policy.as_ref() else {
            return;
        };
        let classifier = policy.classifier.clone();
        let revision = policy.revision.clone();
        let active = policy.active.clone();
        // The context owns the routing guard. The supervisor must not keep policy
        // rules alive after the NIC context starts shutting down.
        let routing = Arc::downgrade(&policy.routing);
        let bridge = policy.bridge.clone();
        let bridge_updates = policy.bridge_updates.clone();
        let global_ctx = self.global_ctx.clone();
        let peer_mgr = self.peer_mgr.clone();
        let policy_data_plane = self.policy_data_plane.clone();
        let mut events = global_ctx.subscribe();
        self.tasks.spawn(async move {
            let mut monitor = tokio::time::interval(Duration::from_secs(1));
            monitor.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let initial_active = active.lock().await.is_some();
            let mut restart_budget = RuntimeRestartBudget::default();
            let mut next_restart = tokio::time::Instant::now();
            let mut dormant = !initial_active
                && schedule_policy_runtime_restart(&mut restart_budget, &mut next_restart);
            let mut last_route_refresh = tokio::time::Instant::now();
            let mut last_mesh_endpoints: Option<
                BTreeMap<String, crate::policy_proxy::MeshProxyTarget>,
            > = None;
            let mut mesh_generation_initialized = false;
            let mut active_since = initial_active.then(tokio::time::Instant::now);
            loop {
                let event = tokio::select! {
                    _ = monitor.tick() => None,
                    event = events.recv() => {
                        match event {
                            Ok(event) => Some(event),
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                events = events.resubscribe();
                                Some(GlobalCtxEvent::PeerAdded(0))
                            }
                        }
                    }
                };
                let meaningful_event = event.as_ref().is_some_and(|event| {
                    matches!(
                        event,
                        GlobalCtxEvent::PeerAdded(_)
                            | GlobalCtxEvent::PeerRemoved(_)
                            | GlobalCtxEvent::ProxyCidrsUpdated(_, _)
                            | GlobalCtxEvent::PublicIpv6RoutesUpdated(_, _)
                            | GlobalCtxEvent::DhcpIpv4Changed(_, _)
                            | GlobalCtxEvent::ConfigPatched(_)
                    )
                });
                let route_refresh_due =
                    meaningful_event || last_route_refresh.elapsed() >= Duration::from_secs(5);
                let Some(peer_mgr) = peer_mgr.upgrade() else {
                    break;
                };

                if route_refresh_due {
                    last_route_refresh = tokio::time::Instant::now();
                    let Some(routing) = routing.upgrade() else {
                        break;
                    };
                    match routing.lock().await.refresh() {
                        Ok(true) => tracing::info!(
                            "refreshed policy underlay routes after network change"
                        ),
                        Ok(false) => {}
                        Err(error) => tracing::warn!(
                            ?error,
                            "failed to refresh policy underlay routes; state is tracked and the update will be retried"
                        ),
                    }
                    match Self::collect_policy_mesh_routes_for(
                        &global_ctx,
                        peer_mgr.as_ref(),
                        global_ctx.get_ipv4().or(ipv4_addr),
                        global_ctx.get_ipv6().or(ipv6_addr),
                    )
                    .await
                    {
                        Ok(routes) => classifier.replace_routes(routes),
                        Err(error) => tracing::warn!(
                            ?error,
                            "failed to refresh policy mesh routes; retaining the previous snapshot"
                        ),
                    }
                }

                let stopped = {
                    let guard = active.lock().await;
                    guard
                        .as_ref()
                        .is_some_and(|active| !active.runtime.is_running())
                };
                if stopped {
                    stop_policy_active_runtime(&active, &bridge, &bridge_updates).await;
                    active_since = None;
                    dormant = schedule_policy_runtime_restart(
                        &mut restart_budget,
                        &mut next_restart,
                    );
                    tracing::warn!("Leaf policy worker exited; non-mesh traffic is fail-closed");
                }

                let has_active = active.lock().await.is_some();
                if has_active && !route_refresh_due {
                    if active_since.is_some_and(|since| since.elapsed() >= Duration::from_secs(60)) {
                        restart_budget.reset();
                    }
                    continue;
                }
                if !has_active
                    && !meaningful_event
                    && !route_refresh_due
                    && (dormant || tokio::time::Instant::now() < next_restart)
                {
                    continue;
                }
                let route_table = peer_mgr.list_routes().await;
                let resolved_mesh_endpoints =
                    Self::resolve_policy_mesh_endpoints(
                        &revision,
                        peer_mgr.my_peer_id(),
                        &route_table,
                    )
                    .ok();
                let endpoint_generation_changed = mesh_generation_initialized
                    && resolved_mesh_endpoints != last_mesh_endpoints;
                mesh_generation_initialized = true;
                last_mesh_endpoints = resolved_mesh_endpoints.clone();
                let config_patched = matches!(event, Some(GlobalCtxEvent::ConfigPatched(_)));
                if endpoint_generation_changed || config_patched {
                    dormant = false;
                    restart_budget.reset();
                    next_restart = tokio::time::Instant::now();
                }
                if endpoint_generation_changed {
                    stop_policy_active_runtime(&active, &bridge, &bridge_updates).await;
                    active_since = None;
                    tracing::info!(
                        "mesh proxy endpoint generation changed; rebuilding policy runtime"
                    );
                }
                if let Some(current) = active.lock().await.as_ref() {
                    if route_refresh_due {
                        match resolved_mesh_endpoints {
                            Some(endpoints) => {
                                for (name, endpoint) in &endpoints {
                                    if let Err(error) =
                                        current.mesh_bridges.update_remote(name, *endpoint)
                                    {
                                        tracing::warn!(proxy = %name, ?error, "failed to update mesh proxy endpoint");
                                    }
                                }
                            }
                            None => {
                                current.mesh_bridges.disable_all();
                                tracing::warn!("disabled mesh proxy bridges because a configured endpoint is unavailable");
                            }
                        }
                    }
                    continue;
                }

                if dormant || tokio::time::Instant::now() < next_restart {
                    continue;
                }
                let Some(data_plane) = policy_data_plane.upgrade() else {
                    tracing::warn!("policy data plane disappeared; stopping policy supervisor");
                    break;
                };
                match Self::build_policy_runtime(
                    &config,
                    revision.clone(),
                    data_plane,
                    peer_mgr.clone(),
                    peer_mgr.my_peer_id(),
                    &route_table,
                )
                .await
                {
                    Ok(candidate) => {
                        let candidate_bridge = candidate.bridge.clone();
                        bridge.store(Some(candidate_bridge.clone()));
                        bridge_updates.send_replace(Some(candidate_bridge));
                        *active.lock().await = Some(candidate);
                        active_since = Some(tokio::time::Instant::now());
                        tracing::info!(revision = %revision.id, "Leaf policy worker recovered");
                    }
                    Err(error) => {
                        dormant = schedule_policy_runtime_restart(
                            &mut restart_budget,
                            &mut next_restart,
                        );
                        if dormant {
                            tracing::error!(?error, "policy restart budget exhausted; waiting for route or configuration change");
                        } else {
                            tracing::warn!(
                                ?error,
                                failures = restart_budget.failures(),
                                "policy restart failed"
                            );
                        }
                    }
                }
            }
        });
    }

    #[cfg(all(feature = "leaf-policy-mobile", target_os = "android"))]
    async fn start_policy_proxy_mobile(
        &mut self,
        dns_servers: Vec<IpAddr>,
        network_key: String,
        lease: crate::policy_proxy::PolicyInstanceLease,
    ) -> Result<(), Error> {
        let config = self
            .global_ctx
            .config
            .get_policy_proxy_config()
            .ok_or_else(|| anyhow::anyhow!("policy_proxy configuration is missing"))?;
        if self.global_ctx.get_flags().accept_dns {
            tracing::warn!(
                "Magic DNS remains mesh-owned in Android policy mode; DOMAIN/GEOSITE rules cannot observe names resolved through Magic DNS"
            );
        }
        let document = crate::policy_proxy::resolve_document(&config)?;
        ensure_policy_mesh_credentials_confidential(&self.global_ctx, &document.revision)?;
        let data_plane = self
            .policy_data_plane
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("policy data plane is not available"))?;
        let peer_mgr = self
            .peer_mgr
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("peer manager not available"))?;
        let revision = document.revision.clone();
        let classifier = Arc::new(PacketClassifier::new(
            self.collect_policy_mesh_routes(self.global_ctx.get_ipv4(), self.global_ctx.get_ipv6())
                .await?,
        ));
        let bridge = Arc::new(ArcSwapOption::empty());
        let dropped_packets = Arc::new(AtomicU64::new(0));
        let (bridge_updates, _) = tokio::sync::watch::channel(None);
        let active = Arc::new(Mutex::new(None));
        let routes = peer_mgr.list_routes().await;
        let initial_endpoints =
            Self::resolve_policy_mesh_endpoints(&revision, peer_mgr.my_peer_id(), &routes).ok();
        let initial_active = if dns_servers.is_empty() {
            tracing::warn!(
                policy_source = %document.source_label,
                "Android has no usable underlying DNS yet; mesh remains active and policy traffic stays blocked until network recovery"
            );
            false
        } else {
            match Self::build_policy_runtime_mobile(
                &document,
                &dns_servers,
                data_plane,
                peer_mgr.clone(),
                peer_mgr.my_peer_id(),
                &routes,
            )
            .await
            {
                Ok(candidate) => {
                    bridge.store(Some(candidate.bridge.clone()));
                    bridge_updates.send_replace(Some(candidate.bridge.clone()));
                    *active.lock().await = Some(candidate);
                    tracing::info!(
                        revision = %revision.id,
                        policy_source = %document.source_label,
                        "Android transparent policy proxy is ready"
                    );
                    true
                }
                Err(error) => {
                    tracing::error!(
                        ?error,
                        policy_source = %document.source_label,
                        "Android policy runtime is unavailable; mesh remains active and non-mesh traffic is blocked"
                    );
                    false
                }
            }
        };
        self.policy = Some(PolicyNicContext {
            revision,
            classifier,
            bridge,
            dropped_packets,
            bridge_updates,
            active,
            _lease: lease,
        });
        self.run_mobile_policy_updater(
            document,
            dns_servers,
            network_key,
            initial_endpoints,
            initial_active,
            self.mobile_network_updates.clone(),
        );
        Ok(())
    }

    #[cfg(all(feature = "leaf-policy-mobile", target_os = "android"))]
    async fn build_policy_runtime_mobile(
        document: &crate::policy_proxy::ResolvedPolicyDocument,
        dns_servers: &[IpAddr],
        data_plane: Arc<Socks5Server>,
        peer_mgr: Arc<PeerManager>,
        self_peer_id: u32,
        routes: &[crate::proto::api::instance::Route],
    ) -> anyhow::Result<PolicyActiveRuntime> {
        let mesh_endpoints =
            Self::resolve_policy_mesh_endpoints(&document.revision, self_peer_id, routes)?;
        let mesh_bridges = Arc::new(
            crate::policy_proxy::MeshProxyBridgeSet::start(
                data_plane,
                peer_mgr,
                &document.revision,
                &mesh_endpoints,
            )
            .await?,
        );
        let worker_threads = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1)
            .min(2);
        let factory = InProcessLeafFactory::new(
            document.base_dir.clone(),
            mesh_bridges.clone(),
            dns_servers.to_vec(),
            worker_threads,
        )
        .map_err(anyhow::Error::msg)?;
        let runtime: Arc<InProcessLeafRuntime> = factory
            .start(document.revision.clone())
            .await
            .map_err(anyhow::Error::msg)?;
        let bridge = runtime.bridge();
        Ok(PolicyActiveRuntime {
            runtime: runtime as Arc<dyn PolicyRuntime>,
            bridge,
            mesh_bridges,
        })
    }

    #[cfg(all(feature = "leaf-policy-mobile", target_os = "android"))]
    fn run_mobile_policy_updater(
        &mut self,
        document: crate::policy_proxy::ResolvedPolicyDocument,
        dns_servers: Vec<IpAddr>,
        initial_network_key: String,
        initial_endpoints: Option<BTreeMap<String, crate::policy_proxy::MeshProxyTarget>>,
        initial_active: bool,
        mut network_updates: tokio::sync::watch::Receiver<crate::launcher::MobileNetworkState>,
    ) {
        let Some(policy) = self.policy.as_ref() else {
            return;
        };
        let revision = policy.revision.clone();
        let classifier = policy.classifier.clone();
        let active = policy.active.clone();
        let bridge = policy.bridge.clone();
        let bridge_updates = policy.bridge_updates.clone();
        let global_ctx = self.global_ctx.clone();
        let peer_mgr = self.peer_mgr.clone();
        let policy_data_plane = self.policy_data_plane.clone();
        self.tasks.spawn(async move {
            let mut dns_servers = dns_servers;
            let mut network_key = initial_network_key;
            let mut network_available = !dns_servers.is_empty();
            let mut events = global_ctx.subscribe();
            let mut last_endpoints = initial_endpoints;
            let mut restart_budget = RuntimeRestartBudget::default();
            let mut next_restart = tokio::time::Instant::now();
            let mut dormant = !initial_active
                && (dns_servers.is_empty()
                    || schedule_policy_runtime_restart(&mut restart_budget, &mut next_restart));
            let mut active_since = initial_active.then(tokio::time::Instant::now);
            let mut pending_network_state = Some(network_updates.borrow_and_update().clone());
            loop {
                let active_present = active.lock().await.is_some();
                let retry_pending = network_available && !dormant && !active_present;
                let route_poll = tokio::time::sleep(Duration::from_secs(5));
                let retry_timer = tokio::time::sleep_until(next_restart);
                tokio::pin!(route_poll);
                tokio::pin!(retry_timer);
                let network_state = if let Some(state) = pending_network_state.take() {
                    Some(state)
                } else {
                    tokio::select! {
                        changed = network_updates.changed() => {
                            if changed.is_err() {
                                break;
                            }
                            Some(network_updates.borrow_and_update().clone())
                        }
                        event = events.recv() => {
                            match event {
                                Ok(event) if matches!(
                                    event,
                                    GlobalCtxEvent::PeerAdded(_)
                                        | GlobalCtxEvent::PeerRemoved(_)
                                        | GlobalCtxEvent::ProxyCidrsUpdated(_, _)
                                        | GlobalCtxEvent::PublicIpv6RoutesUpdated(_, _)
                                        | GlobalCtxEvent::DhcpIpv4Changed(_, _)
                                        | GlobalCtxEvent::ConfigPatched(_)
                                ) => None,
                                Ok(_) => continue,
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                    events = events.resubscribe();
                                    None
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            }
                        }
                        _ = &mut route_poll, if active_present => None,
                        _ = &mut retry_timer, if retry_pending => None,
                    }
                };
                if let Some(state) = network_state {
                    if !state.key.is_empty() && state.dns_servers.is_empty() {
                        if state.key != network_key {
                            network_key = state.key;
                            dns_servers.clear();
                            network_available = false;
                            dormant = true;
                            active_since = None;
                            stop_policy_active_runtime(&active, &bridge, &bridge_updates).await;
                            tracing::info!(
                                %network_key,
                                "Android underlying network is unavailable; policy traffic is fail-closed without consuming restart budget"
                            );
                        }
                    } else if !state.key.is_empty()
                        && !state.dns_servers.is_empty()
                        && (state.key != network_key || state.dns_servers != dns_servers)
                    {
                        network_key = state.key;
                        dns_servers = state.dns_servers;
                        network_available = true;
                        restart_budget.reset();
                        dormant = false;
                        next_restart = tokio::time::Instant::now();
                        active_since = None;
                        stop_policy_active_runtime(&active, &bridge, &bridge_updates).await;
                        tracing::info!(%network_key, "Android underlying network changed; rebuilding policy runtime once");
                    }
                }
                let Some(peer_mgr) = peer_mgr.upgrade() else {
                    break;
                };
                if let Ok(routes) = Self::collect_policy_mesh_routes_for(
                    &global_ctx,
                    peer_mgr.as_ref(),
                    global_ctx.get_ipv4(),
                    global_ctx.get_ipv6(),
                )
                .await
                {
                    classifier.replace_routes(routes);
                }

                let routes = peer_mgr.list_routes().await;
                let endpoints =
                    Self::resolve_policy_mesh_endpoints(&revision, peer_mgr.my_peer_id(), &routes)
                        .ok();
                if endpoints != last_endpoints {
                    last_endpoints = endpoints.clone();
                    restart_budget.reset();
                    dormant = !network_available;
                    active_since = None;
                    next_restart = tokio::time::Instant::now();
                    stop_policy_active_runtime(&active, &bridge, &bridge_updates).await;
                }

                let runtime_failed = active
                    .lock()
                    .await
                    .as_ref()
                    .is_some_and(|active| !active.runtime.is_running());
                if runtime_failed {
                    stop_policy_active_runtime(&active, &bridge, &bridge_updates).await;
                    active_since = None;
                    dormant = schedule_policy_runtime_restart(
                        &mut restart_budget,
                        &mut next_restart,
                    );
                    if dormant {
                        tracing::error!(
                            "Android policy worker restart budget exhausted; waiting for route identity change"
                        );
                    }
                } else if active_since.is_some_and(|since| since.elapsed() >= Duration::from_secs(60))
                {
                    restart_budget.reset();
                    active_since = None;
                }

                if active.lock().await.is_some()
                    || !network_available
                    || dormant
                    || tokio::time::Instant::now() < next_restart
                {
                    continue;
                }
                let Some(endpoints) = endpoints else {
                    next_restart = tokio::time::Instant::now() + Duration::from_secs(5);
                    continue;
                };
                let Some(data_plane) = policy_data_plane.upgrade() else {
                    break;
                };
                match Self::build_policy_runtime_mobile(
                    &document,
                    &dns_servers,
                    data_plane,
                    peer_mgr.clone(),
                    peer_mgr.my_peer_id(),
                    &routes,
                )
                .await
                {
                    Ok(candidate) => {
                        debug_assert_eq!(
                            Self::resolve_policy_mesh_endpoints(
                                &revision,
                                peer_mgr.my_peer_id(),
                                &routes
                            )
                            .ok(),
                            Some(endpoints)
                        );
                        bridge.store(Some(candidate.bridge.clone()));
                        bridge_updates.send_replace(Some(candidate.bridge.clone()));
                        *active.lock().await = Some(candidate);
                        active_since = Some(tokio::time::Instant::now());
                        tracing::info!(revision = %revision.id, "Android policy runtime recovered");
                    }
                    Err(error) => {
                        dormant = schedule_policy_runtime_restart(
                            &mut restart_budget,
                            &mut next_restart,
                        );
                        if dormant {
                            tracing::error!(?error, "Android policy restart budget exhausted; waiting for route identity change");
                        } else {
                            tracing::warn!(
                                ?error,
                                failures = restart_budget.failures(),
                                "Android policy restart failed"
                            );
                        }
                    }
                }
            }
        });
    }

    #[cfg(all(feature = "leaf-policy-proxy", unix))]
    fn resolve_policy_mesh_endpoints(
        revision: &easytier_policy::PolicyRevision,
        self_peer_id: u32,
        routes: &[crate::proto::api::instance::Route],
    ) -> Result<BTreeMap<String, crate::policy_proxy::MeshProxyTarget>, Error> {
        let mut endpoints = BTreeMap::new();
        for (name, proxy) in &revision.document.proxies {
            if proxy.via != easytier_policy::ProxyVia::Mesh {
                continue;
            }
            let easytier_policy::ProxyServer::Mesh {
                instance_id,
                virtual_ip,
            } = &proxy.server
            else {
                unreachable!("validated mesh proxy must use a structured selector");
            };
            let route = if let Some(instance_id) = instance_id {
                routes
                    .iter()
                    .find(|route| route.inst_id == instance_id.to_string())
            } else if let Some(virtual_ip) = virtual_ip {
                routes.iter().find(|route| {
                    route.ipv4_addr.is_some_and(|address| {
                        IpAddr::V4(cidr::Ipv4Inet::from(address).address()) == *virtual_ip
                    }) || route.ipv6_addr.is_some_and(|address| {
                        IpAddr::V6(cidr::Ipv6Inet::from(address).address()) == *virtual_ip
                    })
                })
            } else {
                None
            };
            if route.is_none() {
                return Err(anyhow::anyhow!(
                    "mesh proxy {name} endpoint is not present in the route table"
                )
                .into());
            }
            if route.is_some_and(|route| route.peer_id == self_peer_id) {
                return Err(anyhow::anyhow!(
                    "mesh proxy {name} cannot target the current EasyTier instance"
                )
                .into());
            }
            let routed_ip = route.and_then(|route| {
                route
                    .ipv4_addr
                    .map(|address| IpAddr::V4(cidr::Ipv4Inet::from(address).address()))
                    .or_else(|| {
                        route
                            .ipv6_addr
                            .map(|address| IpAddr::V6(cidr::Ipv6Inet::from(address).address()))
                    })
            });
            if let (Some(expected), Some(actual)) = (*virtual_ip, routed_ip)
                && expected != actual
            {
                return Err(anyhow::anyhow!(
                    "mesh proxy {name} virtual-ip {expected} does not match instance route {actual}"
                )
                .into());
            }
            let address = (*virtual_ip).or(routed_ip).ok_or_else(|| {
                anyhow::anyhow!("mesh proxy {name} has no resolvable virtual address")
            })?;
            if address.is_ipv6() {
                return Err(anyhow::anyhow!(
                    "mesh proxy {name} requires a virtual IPv4 endpoint in v1"
                )
                .into());
            }
            endpoints.insert(
                name.clone(),
                crate::policy_proxy::MeshProxyTarget {
                    peer_id: route.expect("route was checked above").peer_id,
                    endpoint: SocketAddr::new(address, proxy.port),
                },
            );
        }
        Ok(endpoints)
    }

    pub async fn run(
        &mut self,
        ipv4_addr: Option<cidr::Ipv4Inet>,
        ipv6_addr: Option<cidr::Ipv6Inet>,
    ) -> Result<(), Error> {
        let tunnel = {
            let mut nic = self.nic.lock().await;
            match nic.create_dev().await {
                Ok(ret) => {
                    #[cfg(target_os = "windows")]
                    {
                        let dev_name = self.global_ctx.get_flags().dev_name;
                        let _ = RegistryManager::reg_change_catrgory_in_profile(&dev_name);
                    }

                    #[cfg(any(
                        all(target_os = "macos", not(feature = "macos-ne")),
                        target_os = "freebsd"
                    ))]
                    {
                        // remove the 10.0.0.0/24 route (which is added by rust-tun by default)
                        let _ = nic
                            .ifcfg
                            .remove_ipv4_route(nic.ifname(), "10.0.0.0".parse().unwrap(), 24)
                            .await;
                    }

                    ret
                }
                Err(err) => {
                    self.global_ctx
                        .issue_event(GlobalCtxEvent::TunDeviceError(err.to_string()));
                    return Err(err);
                }
            }
        };

        // Assign IPv4 address if provided
        if let Some(ipv4_addr) = ipv4_addr {
            self.assign_ipv4_to_tun_device(ipv4_addr).await?;
            #[cfg(target_os = "windows")]
            self.start_windows_udp_broadcast_relay(ipv4_addr);
        }

        // Assign IPv6 address if provided
        if let Some(ipv6_addr) = ipv6_addr {
            self.assign_ipv6_to_tun_device(ipv6_addr).await?;
        }
        if ipv4_addr.is_none() && ipv6_addr.is_none() {
            self.nic.lock().await.link_up().await?;
        }

        #[cfg(all(feature = "leaf-policy-proxy", target_os = "linux"))]
        let policy_config = crate::policy_proxy::configured_for(self.global_ctx.config.as_ref())?;
        #[cfg(all(feature = "leaf-policy-proxy", target_os = "linux"))]
        let policy_lease = if policy_config.is_some() {
            match crate::policy_proxy::acquire_instance() {
                Ok(lease) => Some(lease),
                Err(error) => {
                    tracing::warn!(
                        ?error,
                        "policy mode is already owned by another instance; using the ordinary NIC path"
                    );
                    None
                }
            }
        } else {
            None
        };
        #[cfg(all(feature = "leaf-policy-proxy", target_os = "linux"))]
        let policy_owns_default_route = policy_lease.is_some();
        #[cfg(not(all(feature = "leaf-policy-proxy", target_os = "linux")))]
        let policy_owns_default_route = false;

        self.run_proxy_cidrs_route_updater().await?;
        self.run_public_ipv6_route_updater().await?;
        // Keep the updater running so runtime config patches can enable auto mode
        // without recreating the NIC.
        self.run_public_ipv6_addr_updater(policy_owns_default_route)
            .await?;

        #[cfg(all(feature = "leaf-policy-proxy", target_os = "linux"))]
        if let (Some(policy_config), Some(policy_lease)) = (policy_config, policy_lease) {
            self.start_policy_proxy(ipv4_addr, ipv6_addr, policy_config.clone(), policy_lease)
                .await?;
            self.run_policy_route_updater(ipv4_addr, ipv6_addr, policy_config);
        }

        let mut nic = self.nic.lock().await;
        nic.commit_backend()?;
        let tun_offload_enabled = nic.tun_offload_enabled;
        self.global_ctx
            .issue_event(GlobalCtxEvent::TunDeviceReady(nic.ifname().to_string()));
        drop(nic);

        let (stream, sink) = tunnel.split();
        #[cfg(all(feature = "leaf-policy-proxy", target_os = "linux"))]
        let policy_io = self.policy.as_ref().map(|policy| {
            (
                policy.classifier.clone(),
                policy.bridge.clone(),
                policy.dropped_packets.clone(),
            )
        });
        self.do_forward_nic_to_peers_task(
            stream,
            #[cfg(all(feature = "leaf-policy-proxy", target_os = "linux"))]
            policy_io.clone(),
            #[cfg(all(feature = "leaf-policy-proxy", unix, not(target_os = "linux")))]
            None,
        )?;
        #[cfg(all(feature = "leaf-policy-proxy", target_os = "linux"))]
        let policy_bridge_updates = self
            .policy
            .as_ref()
            .map(|policy| policy.bridge_updates.subscribe());
        #[cfg(all(feature = "leaf-policy-proxy", target_os = "linux"))]
        if let Some(bridge_updates) = policy_bridge_updates {
            self.do_forward_peers_and_policy_to_nic(sink, bridge_updates, tun_offload_enabled);
        } else {
            self.do_forward_peers_to_nic_with_mode(sink, tun_offload_enabled);
        }
        #[cfg(not(all(feature = "leaf-policy-proxy", target_os = "linux")))]
        self.do_forward_peers_to_nic_with_mode(sink, tun_offload_enabled);

        Ok(())
    }

    #[cfg(mobile)]
    pub async fn run_for_mobile(
        &mut self,
        tun_fd: std::os::fd::RawFd,
        dns_servers: Vec<IpAddr>,
        network_key: String,
    ) -> Result<(), Error> {
        let tunnel = {
            let mut nic = self.nic.lock().await;
            match nic.create_dev_for_mobile(tun_fd).await {
                Ok(ret) => ret,
                Err(err) => {
                    self.global_ctx
                        .issue_event(GlobalCtxEvent::TunDeviceError(err.to_string()));
                    return Err(err);
                }
            }
        };

        #[cfg(all(feature = "leaf-policy-mobile", target_os = "android"))]
        if self
            .global_ctx
            .config
            .get_policy_proxy_config()
            .is_some_and(|config| config.enabled)
        {
            let lease = crate::policy_proxy::acquire_instance()?;
            self.start_policy_proxy_mobile(dns_servers, network_key, lease)
                .await?;
        }
        #[cfg(not(all(feature = "leaf-policy-mobile", target_os = "android")))]
        let _ = (dns_servers, network_key);

        self.global_ctx.issue_event(GlobalCtxEvent::TunDeviceReady(
            self.nic.lock().await.ifname().to_string(),
        ));

        let (stream, sink) = tunnel.split();

        #[cfg(all(feature = "leaf-policy-proxy", unix))]
        let policy_io = self.policy.as_ref().map(|policy| {
            (
                policy.classifier.clone(),
                policy.bridge.clone(),
                policy.dropped_packets.clone(),
            )
        });
        self.do_forward_nic_to_peers_task(
            stream,
            #[cfg(all(feature = "leaf-policy-proxy", unix))]
            policy_io,
        )?;
        #[cfg(all(feature = "leaf-policy-proxy", unix))]
        if let Some(policy) = self.policy.as_ref() {
            self.do_forward_peers_and_policy_to_nic(sink, policy.bridge_updates.subscribe(), false);
        } else {
            self.do_forward_peers_to_nic(sink);
        }
        #[cfg(not(all(feature = "leaf-policy-proxy", unix)))]
        self.do_forward_peers_to_nic(sink);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[cfg(all(feature = "leaf-policy-proxy", unix))]
    use crate::common::{
        config::NetworkIdentity, global_ctx::tests::get_mock_global_ctx_with_network,
    };
    use crate::common::{error::Error, global_ctx::tests::get_mock_global_ctx};

    use super::VirtualNic;
    #[cfg(all(feature = "leaf-policy-proxy", unix))]
    use super::{NicCtx, ensure_policy_mesh_credentials_confidential};

    async fn run_test_helper() -> Result<VirtualNic, Error> {
        let mut dev = VirtualNic::new(get_mock_global_ctx());
        let _tunnel = dev.create_dev().await?;

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

        dev.link_up().await?;
        dev.remove_ip(None).await?;
        dev.add_ip("10.144.111.1".parse().unwrap(), 24).await?;
        Ok(dev)
    }

    #[tokio::test]
    async fn tun_test() {
        let _dev = run_test_helper().await.unwrap();

        // let mut stream = nic.pin_recv_stream();
        // while let Some(item) = stream.next().await {
        //     println!("item: {:?}", item);
        // }

        // let framed = dev.into_framed();
        // let (mut s, mut b) = framed.split();
        // loop {
        //     let tmp = b.next().await.unwrap().unwrap();
        //     let tmp = EthernetPacket::new(tmp.get_bytes());
        //     println!("ret: {:?}", tmp.unwrap());
        // }
    }

    #[cfg(all(feature = "leaf-policy-proxy", unix))]
    #[test]
    fn policy_mesh_endpoint_rejects_current_peer_identity() {
        let instance_id = uuid::Uuid::new_v4();
        let revision = easytier_policy::PolicyRevision::parse(
            format!(
                r#"
version: 1
proxies:
  exit:
    type: socks5
    server: {{ instance-id: "{instance_id}", virtual-ip: 10.44.0.7 }}
    port: 1080
    via: mesh
rules: ["FINAL,exit"]
"#
            ),
            std::path::Path::new("."),
        )
        .unwrap();
        let route = crate::proto::api::instance::Route {
            peer_id: 7,
            inst_id: instance_id.to_string(),
            ipv4_addr: Some(
                cidr::Ipv4Inet::new("10.44.0.7".parse().unwrap(), 24)
                    .unwrap()
                    .into(),
            ),
            ..Default::default()
        };

        assert!(
            NicCtx::resolve_policy_mesh_endpoints(&revision, 7, std::slice::from_ref(&route))
                .is_err()
        );
        let resolved = NicCtx::resolve_policy_mesh_endpoints(&revision, 8, &[route]).unwrap();
        assert_eq!(resolved["exit"].peer_id, 7);
        assert_eq!(resolved["exit"].endpoint, "10.44.0.7:1080".parse().unwrap());
    }

    #[cfg(all(feature = "leaf-policy-proxy", unix))]
    #[tokio::test]
    async fn authenticated_mesh_proxy_requires_confidential_peer_rpc() {
        let revision = easytier_policy::PolicyRevision::parse(
            r#"
version: 1
proxies:
  exit:
    type: socks5
    server: { virtual-ip: 10.44.0.7 }
    port: 1080
    via: mesh
    username: alice
    password: secret
rules: ["FINAL,exit"]
"#,
            std::path::Path::new("."),
        )
        .unwrap();
        let plain = get_mock_global_ctx_with_network(Some(NetworkIdentity::new(
            "plain".to_owned(),
            String::new(),
        )));
        assert!(ensure_policy_mesh_credentials_confidential(&plain, &revision).is_err());

        let encrypted = get_mock_global_ctx_with_network(Some(NetworkIdentity::new(
            "encrypted".to_owned(),
            "network-secret".to_owned(),
        )));
        assert!(ensure_policy_mesh_credentials_confidential(&encrypted, &revision).is_ok());
    }
}
