use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    ffi::{CStr, CString},
    fs::File,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
    pin::Pin,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    task::{Context, Poll},
};

use anyhow::Context as _;
use async_trait::async_trait;
use cidr::{Ipv4Cidr, Ipv4Inet, Ipv6Inet};
use futures::{Sink, Stream, ready};
use netlink_packet_core::{
    NLM_F_ACK, NLM_F_CREATE, NLM_F_DUMP, NLM_F_EXCL, NLM_F_REQUEST, NetlinkHeader, NetlinkMessage,
    NetlinkPayload,
};
use netlink_packet_route::{
    AddressFamily, RouteNetlinkMessage,
    address::{AddressAttribute, AddressFlags, AddressMessage},
    link::{InfoData, InfoKind, InfoVeth, LinkAttribute, LinkFlags, LinkInfo, LinkMessage},
    neighbour::{NeighbourAddress, NeighbourAttribute, NeighbourMessage, NeighbourState},
    route::{
        RouteAddress, RouteAttribute, RouteFlags, RouteHeader, RouteMessage, RouteProtocol,
        RouteScope, RouteType,
    },
};
use netlink_sys::{Socket, SocketAddr, protocols::NETLINK_ROUTE};
use nix::{
    libc,
    sched::{CloneFlags, setns, unshare},
};
use tokio::io::unix::AsyncFd;

use crate::{
    common::{error::Error, global_ctx::ArcGlobalCtx, ifcfg::IfConfiguerTrait},
    tunnel::{SinkItem, StreamItem, TunnelError, packet_def::ZCPacket},
};

pub const INTERNAL_GATEWAY_V4: Ipv4Addr = Ipv4Addr::new(169, 254, 255, 254);
pub const INTERNAL_GATEWAY_V6: Ipv6Addr = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0x000e, 1);

const ETH_HEADER_LEN: usize = 14;
const RX_BATCH_SIZE: usize = 32;
const MAX_ADDRESSES: usize = 256;
const ETH_P_IP: u16 = 0x0800;
const ETH_P_IPV6: u16 = 0x86dd;
const ETH_P_ALL: u16 = 0x0003;
const TP_STATUS_COPY: u32 = 1 << 1;
const ETH_FLAG_LRO: u32 = 1 << 15;

fn has_directed_broadcast(prefix: u8) -> bool {
    matches!(prefix, 1..=30)
}

#[derive(Debug)]
struct BoundNetlink {
    socket: Mutex<Socket>,
    sequence: AtomicU32,
}

impl BoundNetlink {
    fn new() -> Result<Self, Error> {
        let mut socket = Socket::new(NETLINK_ROUTE)?;
        socket.bind_auto()?;
        socket.connect(&SocketAddr::new(0, 0))?;
        Ok(Self {
            socket: Mutex::new(socket),
            sequence: AtomicU32::new(1),
        })
    }

    fn request(&self, request: RouteNetlinkMessage, extra_flags: u16) -> Result<(), Error> {
        let mut request = NetlinkMessage::new(
            NetlinkHeader::default(),
            NetlinkPayload::InnerMessage(request),
        );
        request.header.flags = NLM_F_REQUEST | NLM_F_ACK | extra_flags;
        request.header.sequence_number = self.sequence.fetch_add(1, Ordering::Relaxed);
        request.finalize();

        let mut request_buf = vec![0; request.header.length as usize];
        request.serialize(&mut request_buf);
        let socket = self.socket.lock().unwrap();
        socket.send(&request_buf, 0)?;

        loop {
            let (response, _) = socket.recv_from_full()?;
            let mut offset = 0;
            while offset < response.len() {
                let message =
                    NetlinkMessage::<RouteNetlinkMessage>::deserialize(&response[offset..])
                        .context("failed to decode route netlink response")?;
                offset += message.buffer_len();
                if message.header.sequence_number != request.header.sequence_number {
                    continue;
                }
                match message.payload {
                    NetlinkPayload::Error(error) if error.code.is_none() => return Ok(()),
                    NetlinkPayload::Error(error) => return Err(error.to_io().into()),
                    NetlinkPayload::Done(_) => return Ok(()),
                    _ => {}
                }
            }
        }
    }

    fn dump(&self, request: RouteNetlinkMessage) -> Result<Vec<RouteNetlinkMessage>, Error> {
        let mut request = NetlinkMessage::new(
            NetlinkHeader::default(),
            NetlinkPayload::InnerMessage(request),
        );
        request.header.flags = NLM_F_REQUEST | NLM_F_DUMP;
        request.header.sequence_number = self.sequence.fetch_add(1, Ordering::Relaxed);
        request.finalize();

        let mut request_buf = vec![0; request.header.length as usize];
        request.serialize(&mut request_buf);
        let socket = self.socket.lock().unwrap();
        socket.send(&request_buf, 0)?;

        let mut messages = Vec::new();
        loop {
            let (response, _) = socket.recv_from_full()?;
            let mut offset = 0;
            while offset < response.len() {
                let message =
                    NetlinkMessage::<RouteNetlinkMessage>::deserialize(&response[offset..])
                        .context("failed to decode route netlink dump")?;
                offset += message.buffer_len();
                if message.header.sequence_number != request.header.sequence_number {
                    continue;
                }
                match message.payload {
                    NetlinkPayload::InnerMessage(message) => messages.push(message),
                    NetlinkPayload::Error(error) if error.code.is_none() => return Ok(messages),
                    NetlinkPayload::Error(error) => return Err(error.to_io().into()),
                    NetlinkPayload::Done(_) => return Ok(messages),
                    _ => {}
                }
            }
        }
    }
}

struct NamespaceRestore {
    original: File,
}

impl Drop for NamespaceRestore {
    fn drop(&mut self) {
        let _ = setns(self.original.as_fd(), CloneFlags::CLONE_NEWNET);
    }
}

fn current_thread_namespace() -> anyhow::Result<File> {
    let tid = unsafe { libc::syscall(libc::SYS_gettid) };
    anyhow::ensure!(tid > 0, "gettid returned an invalid thread id");
    let path = format!("/proc/self/task/{tid}/ns/net");
    File::open(&path).with_context(|| format!("open current thread network namespace {path}"))
}

fn enter_instance_namespace(name: Option<&str>) -> anyhow::Result<NamespaceRestore> {
    let original = current_thread_namespace()?;
    if let Some(name) = name {
        let path = if name == crate::common::netns::ROOT_NETNS_NAME {
            "/proc/1/ns/net".to_string()
        } else {
            format!("/var/run/netns/{name}")
        };
        let target = File::open(&path)
            .with_context(|| format!("open configured network namespace {path}"))?;
        setns(target.as_fd(), CloneFlags::CLONE_NEWNET)
            .with_context(|| format!("enter configured network namespace {path}"))?;
    }
    Ok(NamespaceRestore { original })
}

fn interface_index(name: &str) -> anyhow::Result<u32> {
    let name = CString::new(name)?;
    let index = unsafe { libc::if_nametoindex(name.as_ptr()) };
    if index == 0 {
        Err(io::Error::last_os_error()).context("resolve veth interface index")
    } else {
        Ok(index)
    }
}

fn interface_name(index: u32) -> anyhow::Result<String> {
    let mut buf = [0 as libc::c_char; libc::IF_NAMESIZE];
    let ptr = unsafe { libc::if_indextoname(index, buf.as_mut_ptr()) };
    if ptr.is_null() {
        return Err(io::Error::last_os_error()).context("resolve veth interface name");
    }
    Ok(unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned())
}

fn set_link(
    netlink: &BoundNetlink,
    index: u32,
    up: Option<bool>,
    netns_fd: Option<i32>,
) -> Result<(), Error> {
    let mut message = LinkMessage::default();
    message.header.index = index;
    if let Some(up) = up {
        message.header.change_mask = LinkFlags::Up;
        message.header.flags.set(LinkFlags::Up, up);
    }
    if let Some(fd) = netns_fd {
        message.attributes.push(LinkAttribute::NetNsFd(fd));
    }
    netlink.request(RouteNetlinkMessage::SetLink(message), 0)
}

fn create_veth(
    netlink: &BoundNetlink,
    main_name: &str,
    peer_name: &str,
    main_mac: [u8; 6],
    peer_mac: [u8; 6],
    mtu: u32,
) -> Result<(), Error> {
    let mut peer = LinkMessage::default();
    peer.attributes = vec![
        LinkAttribute::IfName(peer_name.to_string()),
        LinkAttribute::Address(peer_mac.to_vec()),
        LinkAttribute::Mtu(mtu),
    ];
    let mut message = LinkMessage::default();
    message.attributes = vec![
        LinkAttribute::IfName(main_name.to_string()),
        LinkAttribute::Address(main_mac.to_vec()),
        LinkAttribute::Mtu(mtu),
        LinkAttribute::LinkInfo(vec![
            LinkInfo::Kind(InfoKind::Veth),
            LinkInfo::Data(InfoData::Veth(InfoVeth::Peer(peer))),
        ]),
    ];
    netlink.request(
        RouteNetlinkMessage::NewLink(message),
        NLM_F_CREATE | NLM_F_EXCL,
    )
}

fn delete_link(netlink: &BoundNetlink, index: u32) {
    let mut message = LinkMessage::default();
    message.header.index = index;
    let _ = netlink.request(RouteNetlinkMessage::DelLink(message), 0);
}

fn write_sysctl(interface: &str, family: &str, setting: &str, value: &str) -> anyhow::Result<()> {
    let path = format!("/proc/sys/net/{family}/conf/{interface}/{setting}");
    std::fs::write(&path, value).with_context(|| format!("write {path}={value}"))
}

#[repr(C)]
struct EthtoolValue {
    command: u32,
    data: u32,
}

fn ethtool_value(interface: &str, command: u32, data: u32) -> io::Result<u32> {
    let socket = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if socket < 0 {
        return Err(io::Error::last_os_error());
    }
    let socket = unsafe { OwnedFd::from_raw_fd(socket) };
    let mut value = EthtoolValue { command, data };
    let mut request = unsafe { std::mem::zeroed::<libc::ifreq>() };
    let bytes = interface.as_bytes();
    if bytes.len() >= libc::IFNAMSIZ {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "interface name is too long",
        ));
    }
    for (slot, byte) in request.ifr_name.iter_mut().zip(bytes) {
        *slot = *byte as libc::c_char;
    }
    request.ifr_ifru.ifru_data = (&mut value as *mut EthtoolValue).cast();
    let result = unsafe { libc::ioctl(socket.as_raw_fd(), libc::SIOCETHTOOL as _, &mut request) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(value.data)
    }
}

fn feature_not_applicable(error: &io::Error) -> bool {
    error.raw_os_error().is_some_and(|code| {
        code == libc::EOPNOTSUPP || code == libc::EINVAL || code == libc::ENOTTY
    })
}

fn disable_offloads(interface: &str) -> anyhow::Result<()> {
    const FEATURES: [(&str, u32, u32); 6] = [
        ("tx-checksum", 0x16, 0x17),
        ("scatter-gather", 0x18, 0x19),
        ("tso", 0x1e, 0x1f),
        ("ufo", 0x21, 0x22),
        ("gso", 0x23, 0x24),
        ("gro", 0x2b, 0x2c),
    ];
    for (name, get, set) in FEATURES {
        match ethtool_value(interface, set, 0) {
            Ok(_) => {
                let active = ethtool_value(interface, get, 0)
                    .with_context(|| format!("read back {name} on {interface}"))?;
                anyhow::ensure!(active == 0, "{name} remains active on {interface}");
            }
            Err(error) if feature_not_applicable(&error) => {
                tracing::debug!(interface, feature = name, "offload is not applicable");
            }
            Err(error) => {
                return Err(error).with_context(|| format!("disable {name} on {interface}"));
            }
        }
    }

    match ethtool_value(interface, 0x25, 0) {
        Ok(flags) if flags & ETH_FLAG_LRO != 0 => {
            ethtool_value(interface, 0x26, flags & !ETH_FLAG_LRO)
                .with_context(|| format!("disable lro on {interface}"))?;
            let current = ethtool_value(interface, 0x25, 0)
                .with_context(|| format!("read back lro on {interface}"))?;
            anyhow::ensure!(
                current & ETH_FLAG_LRO == 0,
                "lro remains active on {interface}"
            );
        }
        Ok(_) => {}
        Err(error) if feature_not_applicable(&error) => {}
        Err(error) => return Err(error).with_context(|| format!("query lro on {interface}")),
    }
    Ok(())
}

fn attach_ip_filter(fd: i32) -> io::Result<()> {
    const BPF_LD_H_ABS: u16 = 0x28;
    const BPF_JMP_JEQ_K: u16 = 0x15;
    const BPF_RET_K: u16 = 0x06;
    let mut filters = [
        libc::sock_filter {
            code: BPF_LD_H_ABS,
            jt: 0,
            jf: 0,
            k: 12,
        },
        libc::sock_filter {
            code: BPF_JMP_JEQ_K,
            jt: 2,
            jf: 0,
            k: ETH_P_IP as u32,
        },
        libc::sock_filter {
            code: BPF_JMP_JEQ_K,
            jt: 1,
            jf: 0,
            k: ETH_P_IPV6 as u32,
        },
        libc::sock_filter {
            code: BPF_RET_K,
            jt: 0,
            jf: 0,
            k: 0,
        },
        libc::sock_filter {
            code: BPF_RET_K,
            jt: 0,
            jf: 0,
            k: u32::MAX,
        },
    ];
    let program = libc::sock_fprog {
        len: filters.len() as u16,
        filter: filters.as_mut_ptr(),
    };
    let result = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ATTACH_FILTER,
            (&program as *const libc::sock_fprog).cast(),
            std::mem::size_of::<libc::sock_fprog>() as libc::socklen_t,
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn create_packet_socket(interface_index: u32) -> anyhow::Result<(OwnedFd, bool)> {
    let fd = unsafe {
        libc::socket(
            libc::AF_PACKET,
            libc::SOCK_RAW | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            ETH_P_ALL.to_be() as i32,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error()).context("create AF_PACKET socket");
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let address = libc::sockaddr_ll {
        sll_family: libc::AF_PACKET as u16,
        sll_protocol: ETH_P_ALL.to_be(),
        sll_ifindex: interface_index as i32,
        sll_hatype: 0,
        sll_pkttype: 0,
        sll_halen: 0,
        sll_addr: [0; 8],
    };
    let result = unsafe {
        libc::bind(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_ll).cast(),
            std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error()).context("bind AF_PACKET socket");
    }

    let one: libc::c_int = 1;
    let result = unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_PACKET,
            libc::PACKET_AUXDATA,
            (&one as *const libc::c_int).cast(),
            std::mem::size_of_val(&one) as libc::socklen_t,
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error()).context("enable PACKET_AUXDATA");
    }

    let result = unsafe {
        libc::setsockopt(
            fd.as_raw_fd(),
            libc::SOL_PACKET,
            libc::PACKET_IGNORE_OUTGOING,
            (&one as *const libc::c_int).cast(),
            std::mem::size_of_val(&one) as libc::socklen_t,
        )
    };
    let ignore_outgoing = if result == 0 {
        true
    } else {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOPROTOOPT) {
            false
        } else {
            return Err(error).context("enable PACKET_IGNORE_OUTGOING");
        }
    };
    attach_ip_filter(fd.as_raw_fd()).context("attach AF_PACKET IP filter")?;
    Ok((fd, ignore_outgoing))
}

fn deterministic_macs(id: uuid::Uuid) -> ([u8; 6], [u8; 6]) {
    let bytes = id.as_bytes();
    (
        [0x02, bytes[0], bytes[1], bytes[2], bytes[3], 0x01],
        [0x02, bytes[0], bytes[1], bytes[2], bytes[3], 0x02],
    )
}

fn make_interface_names(id: uuid::Uuid, configured: &str) -> anyhow::Result<(String, String)> {
    let suffix = &id.simple().to_string()[..8];
    let main = if configured.is_empty() {
        format!("etv{suffix}")
    } else {
        anyhow::ensure!(
            configured.len() < libc::IF_NAMESIZE,
            "veth interface name exceeds Linux IFNAMSIZ"
        );
        configured.to_string()
    };
    Ok((main, format!("etp{suffix}")))
}

#[derive(Default)]
struct AddressRegistry {
    ipv4_normal: BTreeSet<Ipv4Inet>,
    ipv6_normal: BTreeSet<Ipv6Inet>,
    ipv4_orphans: BTreeSet<Ipv4Inet>,
    ipv6_orphans: BTreeSet<Ipv6Inet>,
    pending: usize,
}

impl AddressRegistry {
    fn used(&self) -> usize {
        self.ipv4_normal.len()
            + self.ipv6_normal.len()
            + self.ipv4_orphans.len()
            + self.ipv6_orphans.len()
            + self.pending
    }

    fn reserve(&mut self) -> Result<(), Error> {
        if self.used() >= MAX_ADDRESSES {
            return Err(
                anyhow::anyhow!("veth address capacity {MAX_ADDRESSES} is exhausted").into(),
            );
        }
        self.pending += 1;
        Ok(())
    }

    fn release_reservation(&mut self) {
        debug_assert!(self.pending > 0);
        self.pending = self.pending.saturating_sub(1);
    }

    fn commit_ipv4(&mut self, address: Ipv4Inet) {
        self.release_reservation();
        self.ipv4_normal.insert(address);
    }

    fn commit_ipv6(&mut self, address: Ipv6Inet) {
        self.release_reservation();
        self.ipv6_normal.insert(address);
    }

    fn orphan_ipv4(&mut self, address: Ipv4Inet) {
        self.release_reservation();
        self.ipv4_orphans.insert(address);
    }

    fn orphan_ipv6(&mut self, address: Ipv6Inet) {
        self.release_reservation();
        self.ipv6_orphans.insert(address);
    }
}

struct VethState {
    netlink: Arc<BoundNetlink>,
    main_index: u32,
    main_name: String,
    peer_index: u32,
    main_mac: [u8; 6],
    peer_mac: [u8; 6],
    mtu: AtomicU32,
    private_namespace: File,
    addresses: Mutex<AddressRegistry>,
    ipv4_routes: Mutex<BTreeMap<(Ipv4Addr, u8), RouteMessage>>,
    ipv6_routes: Mutex<BTreeMap<(Ipv6Addr, u8), RouteMessage>>,
    directed_broadcasts: RwLock<BTreeSet<Ipv4Cidr>>,
    legacy_link_local_cleanup: bool,
    link_up: AtomicBool,
    cleaned: AtomicBool,
}

impl Drop for VethState {
    fn drop(&mut self) {
        self.cleanup();
    }
}

impl VethState {
    fn cleanup(&self) {
        if !self.cleaned.swap(true, Ordering::AcqRel) {
            delete_link(&self.netlink, self.main_index);
        }
    }
}

#[derive(Clone)]
pub struct VethIfConfiguer {
    state: Arc<VethState>,
}

impl VethIfConfiguer {
    fn route_is_absent(error: &Error) -> bool {
        matches!(
            error,
            Error::IOError(error)
                if matches!(error.raw_os_error(), Some(libc::ESRCH | libc::ENOENT))
        )
    }

    fn ensure_name(&self, name: &str) -> Result<(), Error> {
        if name == self.state.main_name {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "veth backend is bound to {}, not {name}",
                self.state.main_name
            )
            .into())
        }
    }

    fn normalized_v4_cidr(address: Ipv4Addr, prefix: u8) -> Result<Ipv4Cidr, Error> {
        Ok(Ipv4Inet::new(address, prefix)
            .map_err(anyhow::Error::msg)?
            .network())
    }

    fn route_conflicts_v4(address: Ipv4Addr, prefix: u8) -> Result<bool, Error> {
        Ok(
            prefix != 0
                && Self::normalized_v4_cidr(address, prefix)?.contains(&INTERNAL_GATEWAY_V4),
        )
    }

    fn normalized_v6_cidr(address: Ipv6Addr, prefix: u8) -> Result<cidr::Ipv6Cidr, Error> {
        Ok(Ipv6Inet::new(address, prefix)
            .map_err(anyhow::Error::msg)?
            .network())
    }

    fn route_conflicts_v6(address: Ipv6Addr, prefix: u8) -> Result<bool, Error> {
        let cidr = Self::normalized_v6_cidr(address, prefix)?;
        Ok(prefix != 0 && cidr.contains(&INTERNAL_GATEWAY_V6))
    }

    fn delete_address(&self, address: IpAddr, prefix: u8) -> Result<(), Error> {
        self.state.netlink.request(
            RouteNetlinkMessage::DelAddress(self.address_message(address, prefix)),
            0,
        )
    }

    fn retry_orphan_addresses(&self) {
        let (ipv4, ipv6) = {
            let registry = self.state.addresses.lock().unwrap();
            (
                registry.ipv4_orphans.iter().copied().collect::<Vec<_>>(),
                registry.ipv6_orphans.iter().copied().collect::<Vec<_>>(),
            )
        };
        for address in ipv4 {
            if self
                .delete_address(IpAddr::V4(address.address()), address.network_length())
                .is_ok()
            {
                self.state
                    .addresses
                    .lock()
                    .unwrap()
                    .ipv4_orphans
                    .remove(&address);
            }
        }
        for address in ipv6 {
            if self
                .delete_address(IpAddr::V6(address.address()), address.network_length())
                .is_ok()
            {
                self.state
                    .addresses
                    .lock()
                    .unwrap()
                    .ipv6_orphans
                    .remove(&address);
            }
        }
    }

    fn list_ipv6_addresses(&self) -> Result<Vec<Ipv6Inet>, Error> {
        let mut request = AddressMessage::default();
        request.header.family = AddressFamily::Inet6;
        let messages = self
            .state
            .netlink
            .dump(RouteNetlinkMessage::GetAddress(request))?;
        let mut addresses = Vec::new();
        for message in messages {
            let RouteNetlinkMessage::NewAddress(message) = message else {
                continue;
            };
            if message.header.index != self.state.main_index {
                continue;
            }
            let address = message
                .attributes
                .iter()
                .find_map(|attribute| match attribute {
                    AddressAttribute::Address(IpAddr::V6(address))
                    | AddressAttribute::Local(IpAddr::V6(address)) => Some(*address),
                    _ => None,
                });
            if let Some(address) = address {
                addresses.push(
                    Ipv6Inet::new(address, message.header.prefix_len)
                        .map_err(anyhow::Error::msg)?,
                );
            }
        }
        Ok(addresses)
    }

    async fn remove_automatic_link_local(&self) -> Result<(), Error> {
        if !self.state.legacy_link_local_cleanup {
            return Ok(());
        }
        let mut clean_checks = 0usize;
        for _ in 0..40 {
            let protected = {
                let registry = self.state.addresses.lock().unwrap();
                registry
                    .ipv6_normal
                    .iter()
                    .chain(&registry.ipv6_orphans)
                    .copied()
                    .collect::<BTreeSet<_>>()
            };
            let generated = self
                .list_ipv6_addresses()?
                .into_iter()
                .filter(|address| {
                    is_unspecified_or_link_local(address.address())
                        && !address.address().is_unspecified()
                        && address.address() != INTERNAL_GATEWAY_V6
                        && !protected.contains(address)
                })
                .collect::<Vec<_>>();
            if generated.is_empty() {
                clean_checks += 1;
                if clean_checks >= 3 {
                    return Ok(());
                }
            } else {
                clean_checks = 0;
                for address in generated {
                    self.delete_address(IpAddr::V6(address.address()), address.network_length())?;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        Err(
            anyhow::anyhow!("automatic IPv6 link-local address did not quiesce within 1 second")
                .into(),
        )
    }

    fn route_message(&self, address: IpAddr, prefix: u8, cost: Option<i32>) -> RouteMessage {
        let mut message = RouteMessage::default();
        message.header.address_family = match address {
            IpAddr::V4(_) => AddressFamily::Inet,
            IpAddr::V6(_) => AddressFamily::Inet6,
        };
        message.header.destination_prefix_length = prefix;
        message.header.table = RouteHeader::RT_TABLE_MAIN;
        message.header.protocol = RouteProtocol::Static;
        message.header.scope = RouteScope::Universe;
        message.header.kind = RouteType::Unicast;
        message.header.flags = RouteFlags::Onlink;
        message
            .attributes
            .push(RouteAttribute::Priority(cost.unwrap_or(65535) as u32));
        message
            .attributes
            .push(RouteAttribute::Oif(self.state.main_index));
        if prefix != 0 {
            message
                .attributes
                .push(RouteAttribute::Destination(match address {
                    IpAddr::V4(address) => RouteAddress::Inet(address),
                    IpAddr::V6(address) => RouteAddress::Inet6(address),
                }));
        }
        message
            .attributes
            .push(RouteAttribute::Gateway(match address {
                IpAddr::V4(_) => RouteAddress::Inet(INTERNAL_GATEWAY_V4),
                IpAddr::V6(_) => RouteAddress::Inet6(INTERNAL_GATEWAY_V6),
            }));
        message
    }

    fn address_message(&self, address: IpAddr, prefix: u8) -> AddressMessage {
        let mut message = AddressMessage::default();
        message.header.index = self.state.main_index;
        message.header.prefix_len = prefix;
        message.header.family = match address {
            IpAddr::V4(_) => AddressFamily::Inet,
            IpAddr::V6(_) => AddressFamily::Inet6,
        };
        message.attributes.push(AddressAttribute::Address(address));
        if address.is_ipv4() {
            message.attributes.push(AddressAttribute::Local(address));
            let IpAddr::V4(address) = address else {
                unreachable!();
            };
            let broadcast = if prefix == 32 {
                address
            } else {
                Ipv4Addr::from(u32::from(address) | (u32::MAX >> u32::from(prefix)))
            };
            message
                .attributes
                .push(AddressAttribute::Broadcast(broadcast));
        }
        message
            .attributes
            .push(AddressAttribute::Flags(AddressFlags::Noprefixroute));
        message
    }
}

#[async_trait]
impl IfConfiguerTrait for VethIfConfiguer {
    fn cleanup(&self) {
        self.retry_orphan_addresses();
        self.state.cleanup();
    }

    fn requires_runtime_netns_guard(&self) -> bool {
        false
    }

    async fn add_ipv4_route(
        &self,
        name: &str,
        address: Ipv4Addr,
        prefix: u8,
        cost: Option<i32>,
    ) -> Result<(), Error> {
        self.ensure_name(name)?;
        let cidr = Self::normalized_v4_cidr(address, prefix)?;
        if Self::route_conflicts_v4(address, prefix)? {
            tracing::warn!(%address, prefix, gateway = %INTERNAL_GATEWAY_V4, "veth route overlaps the reserved internal gateway");
            return Err(anyhow::anyhow!(
                "route {address}/{prefix} contains reserved veth gateway {INTERNAL_GATEWAY_V4}"
            )
            .into());
        }
        let route = self.route_message(IpAddr::V4(cidr.first_address()), prefix, cost);
        self.state.netlink.request(
            RouteNetlinkMessage::NewRoute(route.clone()),
            NLM_F_CREATE | NLM_F_EXCL,
        )?;
        self.state
            .ipv4_routes
            .lock()
            .unwrap()
            .insert((cidr.first_address(), prefix), route);
        if has_directed_broadcast(prefix) {
            self.state.directed_broadcasts.write().unwrap().insert(cidr);
        }
        Ok(())
    }

    async fn remove_ipv4_route(
        &self,
        name: &str,
        address: Ipv4Addr,
        prefix: u8,
    ) -> Result<(), Error> {
        self.ensure_name(name)?;
        let cidr = Self::normalized_v4_cidr(address, prefix)?;
        let key = (cidr.first_address(), prefix);
        let route = self
            .state
            .ipv4_routes
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .unwrap_or_else(|| self.route_message(IpAddr::V4(cidr.first_address()), prefix, None));
        if let Err(error) = self
            .state
            .netlink
            .request(RouteNetlinkMessage::DelRoute(route), 0)
            && !Self::route_is_absent(&error)
        {
            return Err(error);
        }
        self.state.ipv4_routes.lock().unwrap().remove(&key);
        self.state
            .directed_broadcasts
            .write()
            .unwrap()
            .remove(&cidr);
        Ok(())
    }

    async fn add_ipv4_ip(&self, name: &str, address: Ipv4Addr, prefix: u8) -> Result<(), Error> {
        self.ensure_name(name)?;
        self.retry_orphan_addresses();
        if address == INTERNAL_GATEWAY_V4 {
            return Err(anyhow::anyhow!("{INTERNAL_GATEWAY_V4} is reserved by veth").into());
        }
        let inet = Ipv4Inet::new(address, prefix).map_err(anyhow::Error::msg)?;
        self.state.addresses.lock().unwrap().reserve()?;
        if let Err(error) = self.state.netlink.request(
            RouteNetlinkMessage::NewAddress(self.address_message(IpAddr::V4(address), prefix)),
            NLM_F_CREATE | NLM_F_EXCL,
        ) {
            self.state.addresses.lock().unwrap().release_reservation();
            return Err(error);
        }
        match self
            .add_ipv4_route(name, inet.first_address(), prefix, None)
            .await
        {
            Ok(()) => {
                self.state.addresses.lock().unwrap().commit_ipv4(inet);
                Ok(())
            }
            Err(route_error) => match self.delete_address(IpAddr::V4(address), prefix) {
                Ok(()) => {
                    self.state.addresses.lock().unwrap().release_reservation();
                    Err(route_error)
                }
                Err(rollback_error) => {
                    self.state.addresses.lock().unwrap().orphan_ipv4(inet);
                    Err(anyhow::anyhow!(
                        "failed to add route after adding IPv4 address: {route_error}; address rollback failed: {rollback_error}"
                    )
                    .into())
                }
            },
        }
    }

    async fn add_ipv6_route(
        &self,
        name: &str,
        address: Ipv6Addr,
        prefix: u8,
        cost: Option<i32>,
    ) -> Result<(), Error> {
        self.ensure_name(name)?;
        let cidr = Self::normalized_v6_cidr(address, prefix)?;
        if Self::route_conflicts_v6(address, prefix)? {
            tracing::warn!(%address, prefix, gateway = %INTERNAL_GATEWAY_V6, "veth route overlaps the reserved internal gateway");
            return Err(anyhow::anyhow!(
                "route {address}/{prefix} contains reserved veth gateway {INTERNAL_GATEWAY_V6}"
            )
            .into());
        }
        let route = self.route_message(IpAddr::V6(cidr.first_address()), prefix, cost);
        self.state.netlink.request(
            RouteNetlinkMessage::NewRoute(route.clone()),
            NLM_F_CREATE | NLM_F_EXCL,
        )?;
        self.state
            .ipv6_routes
            .lock()
            .unwrap()
            .insert((cidr.first_address(), prefix), route);
        Ok(())
    }

    async fn remove_ipv6_route(
        &self,
        name: &str,
        address: Ipv6Addr,
        prefix: u8,
    ) -> Result<(), Error> {
        self.ensure_name(name)?;
        let cidr = Self::normalized_v6_cidr(address, prefix)?;
        let key = (cidr.first_address(), prefix);
        let route = self
            .state
            .ipv6_routes
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .unwrap_or_else(|| self.route_message(IpAddr::V6(cidr.first_address()), prefix, None));
        if let Err(error) = self
            .state
            .netlink
            .request(RouteNetlinkMessage::DelRoute(route), 0)
            && !Self::route_is_absent(&error)
        {
            return Err(error);
        }
        self.state.ipv6_routes.lock().unwrap().remove(&key);
        Ok(())
    }

    async fn add_ipv6_ip(&self, name: &str, address: Ipv6Addr, prefix: u8) -> Result<(), Error> {
        self.ensure_name(name)?;
        self.retry_orphan_addresses();
        if address == INTERNAL_GATEWAY_V6 {
            return Err(anyhow::anyhow!("{INTERNAL_GATEWAY_V6} is reserved by veth").into());
        }
        let inet = Ipv6Inet::new(address, prefix).map_err(anyhow::Error::msg)?;
        self.state.addresses.lock().unwrap().reserve()?;
        if let Err(error) = self.state.netlink.request(
            RouteNetlinkMessage::NewAddress(self.address_message(IpAddr::V6(address), prefix)),
            NLM_F_CREATE | NLM_F_EXCL,
        ) {
            self.state.addresses.lock().unwrap().release_reservation();
            return Err(error);
        }
        match self
            .add_ipv6_route(name, inet.first_address(), prefix, None)
            .await
        {
            Ok(()) => {
                self.state.addresses.lock().unwrap().commit_ipv6(inet);
                Ok(())
            }
            Err(route_error) => match self.delete_address(IpAddr::V6(address), prefix) {
                Ok(()) => {
                    self.state.addresses.lock().unwrap().release_reservation();
                    Err(route_error)
                }
                Err(rollback_error) => {
                    self.state.addresses.lock().unwrap().orphan_ipv6(inet);
                    Err(anyhow::anyhow!(
                        "failed to add route after adding IPv6 address: {route_error}; address rollback failed: {rollback_error}"
                    )
                    .into())
                }
            },
        }
    }

    async fn set_link_status(&self, name: &str, up: bool) -> Result<(), Error> {
        self.ensure_name(name)?;
        set_link(&self.state.netlink, self.state.main_index, Some(up), None)?;
        let was_up = self.state.link_up.swap(up, Ordering::AcqRel);
        if up && !was_up {
            if let Err(error) = self.remove_automatic_link_local().await {
                self.state.link_up.store(false, Ordering::Release);
                return Err(error);
            }
        }
        Ok(())
    }

    async fn remove_ip(&self, name: &str, ip: Option<Ipv4Inet>) -> Result<(), Error> {
        self.ensure_name(name)?;
        self.retry_orphan_addresses();
        let addresses = if let Some(ip) = ip {
            vec![ip]
        } else {
            self.state
                .addresses
                .lock()
                .unwrap()
                .ipv4_normal
                .iter()
                .copied()
                .collect()
        };
        for address in addresses {
            let message =
                self.address_message(IpAddr::V4(address.address()), address.network_length());
            self.state
                .netlink
                .request(RouteNetlinkMessage::DelAddress(message), 0)?;
            self.state
                .addresses
                .lock()
                .unwrap()
                .ipv4_normal
                .remove(&address);
            let _ = self
                .remove_ipv4_route(name, address.first_address(), address.network_length())
                .await;
        }
        Ok(())
    }

    async fn remove_ipv6(&self, name: &str, ip: Option<Ipv6Inet>) -> Result<(), Error> {
        self.ensure_name(name)?;
        self.retry_orphan_addresses();
        let addresses = if let Some(ip) = ip {
            vec![ip]
        } else {
            self.state
                .addresses
                .lock()
                .unwrap()
                .ipv6_normal
                .iter()
                .copied()
                .collect()
        };
        for address in addresses {
            let message =
                self.address_message(IpAddr::V6(address.address()), address.network_length());
            self.state
                .netlink
                .request(RouteNetlinkMessage::DelAddress(message), 0)?;
            self.state
                .addresses
                .lock()
                .unwrap()
                .ipv6_normal
                .remove(&address);
            let _ = self
                .remove_ipv6_route(name, address.first_address(), address.network_length())
                .await;
        }
        Ok(())
    }

    async fn set_mtu(&self, name: &str, mtu: u32) -> Result<(), Error> {
        self.ensure_name(name)?;
        let mut message = LinkMessage::default();
        message.header.index = self.state.main_index;
        message.attributes.push(LinkAttribute::Mtu(mtu));
        self.state
            .netlink
            .request(RouteNetlinkMessage::SetLink(message), 0)?;
        self.state.mtu.store(mtu, Ordering::Release);
        Ok(())
    }
}

fn add_permanent_neighbour(
    netlink: &BoundNetlink,
    index: u32,
    address: IpAddr,
    mac: [u8; 6],
) -> Result<(), Error> {
    let mut message = NeighbourMessage::default();
    message.header.family = match address {
        IpAddr::V4(_) => AddressFamily::Inet,
        IpAddr::V6(_) => AddressFamily::Inet6,
    };
    message.header.ifindex = index;
    message.header.state = NeighbourState::Permanent;
    message.header.kind = RouteType::Unicast;
    message
        .attributes
        .push(NeighbourAttribute::Destination(match address {
            IpAddr::V4(address) => NeighbourAddress::Inet(address),
            IpAddr::V6(address) => NeighbourAddress::Inet6(address),
        }));
    message
        .attributes
        .push(NeighbourAttribute::LinkLocalAddress(mac.to_vec()));
    netlink.request(
        RouteNetlinkMessage::NewNeighbour(message),
        NLM_F_CREATE | NLM_F_EXCL,
    )
}

fn configure_main_control_plane(name: &str) -> anyhow::Result<bool> {
    for (family, setting, value) in [
        ("ipv6", "autoconf", "0"),
        ("ipv6", "accept_ra", "0"),
        ("ipv6", "dad_transmits", "0"),
        ("ipv6", "accept_redirects", "0"),
        ("ipv4", "accept_redirects", "0"),
        ("ipv4", "send_redirects", "0"),
    ] {
        write_sysctl(name, family, setting, value)?;
    }
    let path = format!("/proc/sys/net/ipv6/conf/{name}/addr_gen_mode");
    match std::fs::write(&path, "1") {
        Ok(()) => Ok(false),
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {
            tracing::warn!(
                interface = name,
                "addr_gen_mode is unavailable; enabling bounded link-local cleanup"
            );
            Ok(true)
        }
        Err(error) => Err(error).with_context(|| format!("write {path}=1")),
    }
}

fn configure_peer_control_plane(name: &str) -> anyhow::Result<()> {
    write_sysctl(name, "ipv4", "forwarding", "0")?;
    write_sysctl(name, "ipv6", "forwarding", "0")?;
    write_sysctl(name, "ipv6", "disable_ipv6", "1")?;
    Ok(())
}

struct SetupResult {
    state: Arc<VethState>,
    packet_socket: OwnedFd,
    ignore_outgoing: bool,
}

fn setup(global_ctx: &ArcGlobalCtx, mtu: u32) -> anyhow::Result<SetupResult> {
    let _restore = enter_instance_namespace(global_ctx.net_ns.name().as_deref())?;
    let instance_namespace =
        current_thread_namespace().context("pin instance network namespace")?;

    unshare(CloneFlags::CLONE_NEWNET).context(
        "create private veth namespace (CAP_SYS_ADMIN, CAP_NET_ADMIN and CAP_NET_RAW are required)",
    )?;
    let private_namespace = current_thread_namespace().context("pin private veth namespace")?;
    setns(instance_namespace.as_fd(), CloneFlags::CLONE_NEWNET)
        .context("return to instance network namespace")?;

    let netlink = Arc::new(BoundNetlink::new().context("create namespace-bound netlink socket")?);
    let (main_name, peer_name) =
        make_interface_names(global_ctx.id, &global_ctx.get_flags().dev_name)?;
    let (main_mac, peer_mac) = deterministic_macs(global_ctx.id);
    create_veth(&netlink, &main_name, &peer_name, main_mac, peer_mac, mtu)
        .context("create veth pair")?;
    let main_index = interface_index(&main_name)?;
    let peer_index = match interface_index(&peer_name) {
        Ok(index) => index,
        Err(error) => {
            delete_link(&netlink, main_index);
            return Err(error);
        }
    };

    let setup_result = (|| -> anyhow::Result<SetupResult> {
        disable_offloads(&main_name)?;
        let legacy_link_local_cleanup = configure_main_control_plane(&main_name)?;
        set_link(
            &netlink,
            peer_index,
            None,
            Some(private_namespace.as_raw_fd()),
        )
        .context("move veth peer to private namespace")?;

        setns(private_namespace.as_fd(), CloneFlags::CLONE_NEWNET)
            .context("enter private veth namespace")?;
        let moved_peer_index = interface_index(&peer_name)?;
        anyhow::ensure!(
            interface_name(moved_peer_index)? == peer_name,
            "moved veth peer identity changed"
        );
        disable_offloads(&peer_name)?;
        configure_peer_control_plane(&peer_name)?;
        let (packet_socket, ignore_outgoing) = create_packet_socket(moved_peer_index)?;
        let private_netlink = BoundNetlink::new()?;
        set_link(&private_netlink, moved_peer_index, Some(true), None)?;

        setns(instance_namespace.as_fd(), CloneFlags::CLONE_NEWNET)
            .context("return to instance namespace after veth setup")?;
        add_permanent_neighbour(
            &netlink,
            main_index,
            IpAddr::V4(INTERNAL_GATEWAY_V4),
            peer_mac,
        )?;
        add_permanent_neighbour(
            &netlink,
            main_index,
            IpAddr::V6(INTERNAL_GATEWAY_V6),
            peer_mac,
        )?;

        let state = Arc::new(VethState {
            netlink: netlink.clone(),
            main_index,
            main_name,
            peer_index: moved_peer_index,
            main_mac,
            peer_mac,
            mtu: AtomicU32::new(mtu),
            private_namespace,
            addresses: Mutex::new(AddressRegistry::default()),
            ipv4_routes: Mutex::new(BTreeMap::new()),
            ipv6_routes: Mutex::new(BTreeMap::new()),
            directed_broadcasts: RwLock::new(BTreeSet::new()),
            legacy_link_local_cleanup,
            link_up: AtomicBool::new(false),
            cleaned: AtomicBool::new(false),
        });
        Ok(SetupResult {
            state,
            packet_socket,
            ignore_outgoing,
        })
    })();

    if setup_result.is_err() {
        let _ = setns(instance_namespace.as_fd(), CloneFlags::CLONE_NEWNET);
        delete_link(&netlink, main_index);
    }
    setup_result
}

fn gateway_packet(packet: &[u8]) -> bool {
    match packet.first().map(|byte| byte >> 4) {
        Some(4) if packet.len() >= 20 => {
            let source = Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]);
            let destination = Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);
            source == INTERNAL_GATEWAY_V4 || destination == INTERNAL_GATEWAY_V4
        }
        Some(6) if packet.len() >= 40 => {
            let source = Ipv6Addr::from(<[u8; 16]>::try_from(&packet[8..24]).unwrap());
            let destination = Ipv6Addr::from(<[u8; 16]>::try_from(&packet[24..40]).unwrap());
            source == INTERNAL_GATEWAY_V6 || destination == INTERNAL_GATEWAY_V6
        }
        _ => true,
    }
}

fn is_unspecified_or_link_local(address: Ipv6Addr) -> bool {
    address.is_unspecified() || (address.segments()[0] & 0xffc0) == 0xfe80
}

fn is_link_local_multicast(address: Ipv6Addr) -> bool {
    address.octets()[0] == 0xff && address.octets()[1] & 0x0f <= 2
}

fn ipv6_upper_layer(packet: &[u8]) -> Option<(u8, usize)> {
    if packet.len() < 40 {
        return None;
    }
    let mut next = packet[6];
    let mut offset = 40usize;
    for _ in 0..8 {
        match next {
            0 | 43 | 60 => {
                let header = packet.get(offset..offset + 2)?;
                next = header[0];
                offset = offset.checked_add((header[1] as usize + 1) * 8)?;
            }
            44 => {
                next = *packet.get(offset)?;
                offset = offset.checked_add(8)?;
            }
            51 => {
                let header = packet.get(offset..offset + 2)?;
                next = header[0];
                offset = offset.checked_add((header[1] as usize + 2) * 4)?;
            }
            _ => return Some((next, offset)),
        }
        if offset > packet.len() {
            return None;
        }
    }
    None
}

fn internal_icmpv6_control(packet: &[u8]) -> bool {
    if packet.len() < 40 {
        return false;
    }
    let source = Ipv6Addr::from(<[u8; 16]>::try_from(&packet[8..24]).unwrap());
    let destination = Ipv6Addr::from(<[u8; 16]>::try_from(&packet[24..40]).unwrap());
    let Some((58, offset)) = ipv6_upper_layer(packet) else {
        return false;
    };
    let Some(kind) = packet.get(offset).copied() else {
        return false;
    };
    let internal_source = is_unspecified_or_link_local(source);
    let internal_destination =
        is_link_local_multicast(destination) || destination == INTERNAL_GATEWAY_V6;
    match kind {
        133..=137 => packet[7] == 255 && internal_source && internal_destination,
        130..=132 | 143 => {
            packet[7] == 1 && internal_source && is_link_local_multicast(destination)
        }
        _ => false,
    }
}

fn internal_ipv4_control(packet: &[u8]) -> bool {
    if packet.len() < 20 || packet[0] >> 4 != 4 {
        return false;
    }
    let header_len = usize::from(packet[0] & 0x0f) * 4;
    if header_len < 20 || packet.len() < header_len {
        return false;
    }
    let destination = Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);
    packet[9] == 2 && packet[8] == 1 && destination.is_multicast()
}

fn valid_auxdata(header: &libc::msghdr) -> bool {
    let mut control = unsafe { libc::CMSG_FIRSTHDR(header) };
    while !control.is_null() {
        let cmsg = unsafe { &*control };
        if cmsg.cmsg_level == libc::SOL_PACKET && cmsg.cmsg_type == libc::PACKET_AUXDATA {
            if (cmsg.cmsg_len as usize)
                < unsafe { libc::CMSG_LEN(std::mem::size_of::<libc::tpacket_auxdata>() as u32) }
                    as usize
            {
                return false;
            }
            let aux = unsafe { &*(libc::CMSG_DATA(control).cast::<libc::tpacket_auxdata>()) };
            if aux.tp_status & (libc::TP_STATUS_CSUMNOTREADY | TP_STATUS_COPY) != 0
                || aux.tp_snaplen < aux.tp_len
            {
                return false;
            }
        }
        control = unsafe { libc::CMSG_NXTHDR(header, control) };
    }
    true
}

#[repr(C, align(8))]
struct ControlBuffer([u8; 128]);

struct RecvBatchStorage {
    frame_capacity: usize,
    buffers: Vec<Vec<u8>>,
    addresses: Box<[libc::sockaddr_ll]>,
    controls: Box<[ControlBuffer]>,
    iovecs: Box<[libc::iovec]>,
    messages: Box<[libc::mmsghdr]>,
}

// Raw pointers only reference this storage's boxed slices and packet buffers. The stream
// rebuilds them before every syscall and accesses the storage exclusively through &mut self.
unsafe impl Send for RecvBatchStorage {}

impl RecvBatchStorage {
    fn new(frame_capacity: usize) -> Self {
        let mut storage = Self {
            frame_capacity: 0,
            buffers: (0..RX_BATCH_SIZE).map(|_| Vec::new()).collect(),
            addresses: (0..RX_BATCH_SIZE)
                .map(|_| unsafe { std::mem::zeroed() })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            controls: (0..RX_BATCH_SIZE)
                .map(|_| ControlBuffer([0; 128]))
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            iovecs: (0..RX_BATCH_SIZE)
                .map(|_| unsafe { std::mem::zeroed() })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            messages: (0..RX_BATCH_SIZE)
                .map(|_| unsafe { std::mem::zeroed() })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        };
        storage.prepare(frame_capacity);
        storage
    }

    fn prepare(&mut self, frame_capacity: usize) {
        self.frame_capacity = frame_capacity;
        for index in 0..RX_BATCH_SIZE {
            self.buffers[index].resize(frame_capacity, 0);
            self.addresses[index] = unsafe { std::mem::zeroed() };
            self.controls[index].0.fill(0);
            self.iovecs[index] = libc::iovec {
                iov_base: self.buffers[index].as_mut_ptr().cast(),
                iov_len: self.buffers[index].len(),
            };
            let mut header = unsafe { std::mem::zeroed::<libc::msghdr>() };
            header.msg_name = (&mut self.addresses[index] as *mut libc::sockaddr_ll).cast();
            header.msg_namelen = std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t;
            header.msg_iov = &mut self.iovecs[index];
            header.msg_iovlen = 1;
            header.msg_control = self.controls[index].0.as_mut_ptr().cast();
            header.msg_controllen = self.controls[index].0.len() as _;
            header.msg_flags = 0;

            let mut message = unsafe { std::mem::zeroed::<libc::mmsghdr>() };
            message.msg_hdr = header;
            message.msg_len = 0;
            self.messages[index] = message;
        }
    }
}

pub struct VethStream {
    socket: Arc<AsyncFd<OwnedFd>>,
    state: Arc<VethState>,
    queue: VecDeque<ZCPacket>,
    batch: RecvBatchStorage,
}

impl VethStream {
    fn receive_batch(&mut self) -> io::Result<()> {
        let frame_capacity = self.state.mtu.load(Ordering::Acquire) as usize + ETH_HEADER_LEN;
        self.batch.prepare(frame_capacity);
        let count = unsafe {
            libc::recvmmsg(
                self.socket.get_ref().as_raw_fd(),
                self.batch.messages.as_mut_ptr(),
                RX_BATCH_SIZE as u32,
                (libc::MSG_DONTWAIT | libc::MSG_TRUNC) as _,
                std::ptr::null_mut(),
            )
        };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENETDOWN) {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            return Err(error);
        }
        for index in 0..count as usize {
            let message = &self.batch.messages[index];
            let frame_len = message.msg_len as usize;
            if self.batch.addresses[index].sll_pkttype == libc::PACKET_OUTGOING as u8
                || message.msg_hdr.msg_flags & libc::MSG_TRUNC != 0
                || frame_len > frame_capacity
                || frame_len < ETH_HEADER_LEN
                || !valid_auxdata(&message.msg_hdr)
            {
                continue;
            }
            let frame = &self.batch.buffers[index][..frame_len];
            let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
            if !matches!(ethertype, ETH_P_IP | ETH_P_IPV6) {
                continue;
            }
            let packet = &frame[ETH_HEADER_LEN..];
            if gateway_packet(packet)
                || (ethertype == ETH_P_IP && internal_ipv4_control(packet))
                || (ethertype == ETH_P_IPV6 && internal_icmpv6_control(packet))
            {
                continue;
            }
            self.queue.push_back(ZCPacket::new_with_payload(packet));
        }
        Ok(())
    }
}

impl Stream for VethStream {
    type Item = StreamItem;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(packet) = self.queue.pop_front() {
            return Poll::Ready(Some(Ok(packet)));
        }
        loop {
            let socket = self.socket.clone();
            let mut readiness = ready!(socket.poll_read_ready(cx))?;
            match readiness.try_io(|_| self.receive_batch()) {
                Ok(Ok(())) => {
                    if let Some(packet) = self.queue.pop_front() {
                        return Poll::Ready(Some(Ok(packet)));
                    }
                }
                Ok(Err(error)) => {
                    return Poll::Ready(Some(Err(TunnelError::IOError(error))));
                }
                Err(_) => continue,
            }
        }
    }
}

pub struct VethSink {
    socket: Arc<AsyncFd<OwnedFd>>,
    state: Arc<VethState>,
    pending: Option<Vec<u8>>,
}

impl VethSink {
    fn destination_mac(&self, packet: &[u8]) -> Result<[u8; 6], TunnelError> {
        match packet.first().map(|byte| byte >> 4) {
            Some(4) if packet.len() >= 20 => {
                let destination = Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);
                if destination == Ipv4Addr::BROADCAST
                    || self
                        .state
                        .directed_broadcasts
                        .read()
                        .unwrap()
                        .iter()
                        .any(|cidr| cidr.last_address() == destination)
                {
                    Ok([0xff; 6])
                } else if destination.is_multicast() {
                    let value = u32::from(destination);
                    Ok([
                        0x01,
                        0x00,
                        0x5e,
                        ((value >> 16) & 0x7f) as u8,
                        ((value >> 8) & 0xff) as u8,
                        (value & 0xff) as u8,
                    ])
                } else {
                    Ok(self.state.main_mac)
                }
            }
            Some(6) if packet.len() >= 40 => {
                let destination = Ipv6Addr::from(<[u8; 16]>::try_from(&packet[24..40]).unwrap());
                if destination.is_multicast() {
                    let octets = destination.octets();
                    Ok([0x33, 0x33, octets[12], octets[13], octets[14], octets[15]])
                } else {
                    Ok(self.state.main_mac)
                }
            }
            _ => Err(TunnelError::InvalidPacket(
                "veth sink accepts only IPv4 or IPv6".to_string(),
            )),
        }
    }

    fn frame(&self, packet: ZCPacket) -> Result<Vec<u8>, TunnelError> {
        let payload = packet.payload();
        if gateway_packet(payload) {
            return Err(TunnelError::InvalidPacket(
                "reserved veth gateway packet rejected".to_string(),
            ));
        }
        let protocol = match payload.first().map(|byte| byte >> 4) {
            Some(4) => ETH_P_IP,
            Some(6) => ETH_P_IPV6,
            _ => {
                return Err(TunnelError::InvalidPacket(
                    "veth sink accepts only IPv4 or IPv6".to_string(),
                ));
            }
        };
        let mtu = self.state.mtu.load(Ordering::Acquire) as usize;
        if payload.len() > mtu {
            return Err(TunnelError::ExceedMaxPacketSize(mtu, payload.len()));
        }
        let destination = self.destination_mac(payload)?;
        let mut frame = Vec::with_capacity(ETH_HEADER_LEN + payload.len());
        frame.extend_from_slice(&destination);
        frame.extend_from_slice(&self.state.peer_mac);
        frame.extend_from_slice(&protocol.to_be_bytes());
        frame.extend_from_slice(payload);
        Ok(frame)
    }

    fn send_frame(&self, frame: &[u8]) -> io::Result<()> {
        let mut address = libc::sockaddr_ll {
            sll_family: libc::AF_PACKET as u16,
            sll_protocol: u16::from_be_bytes([frame[12], frame[13]]).to_be(),
            sll_ifindex: self.state.peer_index as i32,
            sll_hatype: 0,
            sll_pkttype: 0,
            sll_halen: 6,
            sll_addr: [0; 8],
        };
        address.sll_addr[..6].copy_from_slice(&frame[..6]);
        let sent = unsafe {
            libc::sendto(
                self.socket.get_ref().as_raw_fd(),
                frame.as_ptr().cast(),
                frame.len(),
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
                (&address as *const libc::sockaddr_ll).cast(),
                std::mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            )
        };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        if sent as usize != frame.len() {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "partial AF_PACKET datagram write",
            ));
        }
        Ok(())
    }

    fn poll_pending(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), TunnelError>> {
        if self.pending.is_none() {
            return Poll::Ready(Ok(()));
        }
        loop {
            let socket = self.socket.clone();
            let mut readiness = ready!(socket.poll_write_ready(cx))?;
            let frame = self.pending.take().expect("pending frame disappeared");
            match readiness.try_io(|_| self.send_frame(&frame)) {
                Ok(Ok(())) => return Poll::Ready(Ok(())),
                Ok(Err(error)) => return Poll::Ready(Err(error.into())),
                Err(_) => {
                    self.pending = Some(frame);
                }
            }
        }
    }
}

impl Sink<SinkItem> for VethSink {
    type Error = TunnelError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_pending(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: SinkItem) -> Result<(), Self::Error> {
        let frame = self.frame(item)?;
        match self.send_frame(&frame) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                self.pending = Some(frame);
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_pending(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_pending(cx)
    }
}

pub struct CreatedVeth {
    pub ifname: String,
    pub ifcfg: Arc<dyn IfConfiguerTrait>,
    pub stream: VethStream,
    pub sink: VethSink,
}

pub async fn create(global_ctx: ArcGlobalCtx, mtu: u32) -> Result<CreatedVeth, Error> {
    let setup = tokio::task::spawn_blocking(move || setup(&global_ctx, mtu))
        .await
        .context("veth setup thread panicked")?
        .map_err(Error::from)?;
    let socket = Arc::new(AsyncFd::new(setup.packet_socket)?);
    let ifcfg = Arc::new(VethIfConfiguer {
        state: setup.state.clone(),
    });
    tracing::info!(
        interface = setup.state.main_name,
        ignore_outgoing = setup.ignore_outgoing,
        "created native veth NIC backend"
    );
    Ok(CreatedVeth {
        ifname: setup.state.main_name.clone(),
        ifcfg,
        stream: VethStream {
            socket: socket.clone(),
            state: setup.state.clone(),
            queue: VecDeque::new(),
            batch: RecvBatchStorage::new(
                setup.state.mtu.load(Ordering::Acquire) as usize + ETH_HEADER_LEN,
            ),
        },
        sink: VethSink {
            socket,
            state: setup.state,
            pending: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_registry_reserves_before_kernel_mutation() {
        let mut registry = AddressRegistry::default();
        for _ in 0..MAX_ADDRESSES {
            registry.reserve().unwrap();
        }
        assert!(registry.reserve().is_err());
        registry.release_reservation();
        assert!(registry.reserve().is_ok());
        assert_eq!(registry.used(), MAX_ADDRESSES);
    }

    #[test]
    fn directed_broadcast_excludes_default_and_point_to_point_prefixes() {
        assert!(!has_directed_broadcast(0));
        assert!(has_directed_broadcast(30));
        assert!(!has_directed_broadcast(31));
        assert!(!has_directed_broadcast(32));
    }

    #[test]
    fn recv_batch_storage_rebinds_without_reallocating_for_stable_mtu() {
        let mut storage = RecvBatchStorage::new(1514);
        let capacities = storage
            .buffers
            .iter()
            .map(Vec::capacity)
            .collect::<Vec<_>>();
        let pointers = storage
            .buffers
            .iter()
            .map(|buffer| buffer.as_ptr())
            .collect::<Vec<_>>();
        for _ in 0..10_000 {
            storage.prepare(1514);
        }
        assert_eq!(
            capacities,
            storage
                .buffers
                .iter()
                .map(Vec::capacity)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            pointers,
            storage
                .buffers
                .iter()
                .map(|buffer| buffer.as_ptr())
                .collect::<Vec<_>>()
        );
        for index in 0..RX_BATCH_SIZE {
            assert_eq!(
                storage.messages[index].msg_hdr.msg_iov,
                &mut storage.iovecs[index] as *mut libc::iovec
            );
            assert_eq!(
                storage.iovecs[index].iov_base,
                storage.buffers[index].as_mut_ptr().cast()
            );
        }
    }

    #[test]
    fn gateway_filter_is_bidirectional() {
        let mut v4 = vec![0u8; 20];
        v4[0] = 0x45;
        v4[12..16].copy_from_slice(&INTERNAL_GATEWAY_V4.octets());
        assert!(gateway_packet(&v4));
        v4[12..16].copy_from_slice(&Ipv4Addr::new(10, 0, 0, 1).octets());
        v4[16..20].copy_from_slice(&INTERNAL_GATEWAY_V4.octets());
        assert!(gateway_packet(&v4));
    }

    #[test]
    fn icmpv6_filter_requires_all_internal_characteristics() {
        let mut packet = vec![0u8; 48];
        packet[0] = 0x60;
        packet[6] = 58;
        packet[7] = 255;
        packet[8..24].copy_from_slice(&Ipv6Addr::LOCALHOST.octets());
        packet[24..40].copy_from_slice(&"ff02::1".parse::<Ipv6Addr>().unwrap().octets());
        packet[40] = 135;
        assert!(!internal_icmpv6_control(&packet));
        packet[8..24].copy_from_slice(&"fe80::2".parse::<Ipv6Addr>().unwrap().octets());
        assert!(internal_icmpv6_control(&packet));
        packet[7] = 64;
        assert!(!internal_icmpv6_control(&packet));
    }

    #[test]
    fn ipv4_filter_drops_igmp_but_keeps_multicast_udp() {
        let mut packet = vec![0u8; 20];
        packet[0] = 0x45;
        packet[8] = 1;
        packet[9] = 2;
        packet[16..20].copy_from_slice(&Ipv4Addr::new(224, 0, 0, 22).octets());
        assert!(internal_ipv4_control(&packet));
        packet[9] = 17;
        assert!(!internal_ipv4_control(&packet));
    }

    #[test]
    fn reserved_route_conflicts_exclude_default() {
        assert!(!VethIfConfiguer::route_conflicts_v4(Ipv4Addr::UNSPECIFIED, 0).unwrap());
        assert!(VethIfConfiguer::route_conflicts_v4(Ipv4Addr::new(169, 254, 0, 0), 16).unwrap());
        assert!(!VethIfConfiguer::route_conflicts_v6(Ipv6Addr::UNSPECIFIED, 0).unwrap());
        assert!(VethIfConfiguer::route_conflicts_v6("fe80::".parse().unwrap(), 64).unwrap());
    }

    #[test]
    fn missing_route_errors_are_idempotent() {
        assert!(VethIfConfiguer::route_is_absent(&Error::IOError(
            io::Error::from_raw_os_error(libc::ESRCH)
        )));
        assert!(VethIfConfiguer::route_is_absent(&Error::IOError(
            io::Error::from_raw_os_error(libc::ENOENT)
        )));
        assert!(!VethIfConfiguer::route_is_absent(&Error::IOError(
            io::Error::from_raw_os_error(libc::EPERM)
        )));
    }
}
