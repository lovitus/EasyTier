# EasyTier

[![Github release](https://img.shields.io/github/v/tag/EasyTier/EasyTier)](https://github.com/EasyTier/EasyTier/releases)
[![GitHub](https://img.shields.io/github/license/EasyTier/EasyTier)](https://github.com/EasyTier/EasyTier/blob/main/LICENSE)
[![GitHub last commit](https://img.shields.io/github/last-commit/EasyTier/EasyTier)](https://github.com/EasyTier/EasyTier/commits/main)
[![GitHub issues](https://img.shields.io/github/issues/EasyTier/EasyTier)](https://github.com/EasyTier/EasyTier/issues)
[![GitHub Core Actions](https://github.com/EasyTier/EasyTier/actions/workflows/core.yml/badge.svg)](https://github.com/EasyTier/EasyTier/actions/workflows/core.yml)
[![GitHub GUI Actions](https://github.com/EasyTier/EasyTier/actions/workflows/gui.yml/badge.svg)](https://github.com/EasyTier/EasyTier/actions/workflows/gui.yml)
[![GitHub Test Actions](https://github.com/EasyTier/EasyTier/actions/workflows/test.yml/badge.svg)](https://github.com/EasyTier/EasyTier/actions/workflows/test.yml)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/EasyTier/EasyTier)

[简体中文](/README_CN.md) | [English](/README.md)

> ✨ A simple, secure, decentralized virtual private network solution powered by Rust and Tokio

<p align="center">
<img src="assets/config-page.png" width="300" alt="config page">
<img src="assets/running-page.png" width="300" alt="running page">
</p>

📚 **[Full Documentation](https://easytier.cn/en/)** | 🖥️ **[Web Console](https://easytier.cn/web)** | 📝 **[Download Releases](https://github.com/EasyTier/EasyTier/releases)** | 🧩 **[Third Party Tools](https://easytier.cn/en/guide/installation_gui.html#third-party-graphical-interfaces)** | ❤️ **[Sponsor](#sponsor)**

## Features

### Core Features

- 🔒 **Decentralized**: Nodes are equal and independent, no centralized services required  
- 🚀 **Easy to Use**: Multiple operation methods via web, client, and command line  
- 🌍 **Cross-Platform**: Supports Win/MacOS/Linux/FreeBSD/Android and X86/ARM/MIPS architectures  
- 🔐 **Secure**: AES-GCM or WireGuard encryption, prevents man-in-the-middle attacks  

### Advanced Capabilities

- 🔌 **Efficient NAT Traversal**: Supports UDP and IPv6 traversal, works with NAT4-NAT4 networks  
- 🌐 **Subnet Proxy**: Nodes can share subnets for other nodes to access  
- 🔄 **Intelligent Routing**: Latency priority and automatic route selection for best network experience  
- ⚡ **High Performance**: Zero-copy throughout the entire link, supports TCP/UDP/WSS/WG protocols  

### Network Optimization

- 📊 **UDP Loss Resistance**: KCP/QUIC proxy optimizes latency and bandwidth in high packet loss environments  
- 🔧 **Web Management**: Easy configuration and monitoring through web interface  
- 🛠️ **Zero Config**: Simple deployment with statically linked executables  

## Quick Start

### 📥 Installation

Choose the installation method that best suits your needs:

Linux (Recommended):
```bash
curl -fsSL "https://github.com/EasyTier/EasyTier/blob/main/script/install.sh?raw=true" | sudo bash -s install
```

Homebrew (MacOS/Linux):
```bash
brew tap brewforge/chinese
brew install --cask easytier-gui
```

Windows (Recommended, run with administrator privileges):
```powershell
irm "https://github.com/EasyTier/EasyTier/blob/main/script/install.ps1?raw=true" | iex
```

Install via cargo (Latest development version): 
```bash
cargo install --git https://github.com/EasyTier/EasyTier.git easytier
```

[Install pre-built binary](https://github.com/EasyTier/EasyTier/releases) (Recommended, All platforms supported)

[Install via Docker](https://easytier.cn/en/guide/installation.html#installation-methods)

[Install OpenWrt ipk package](https://github.com/EasyTier/luci-app-easytier)

Additional steps:

[One-Click Register Service](https://easytier.cn/en/guide/network/oneclick-install-as-service.html) (Automatically start when the system boots and run in the background)

### Fork Pre-release Validation and Release Order

This fork separates in-development device validation from formal release builds so that each fix does not consume the complete cross-platform build matrix:

1. Before pushing a fix commit to `releases/**`, include `[skip ci]` in the commit message to prevent that push from starting the full workflow set.
2. After pushing, manually run only `EasyTier GUI macOS ARM64 Test` on the same ref.
3. Validate the generated macOS ARM64 GUI on a real device. During this stage, automation must not trigger Core, the full GUI matrix, Mobile, OHOS, the full Test workflow, or Release.
4. Only after the maintainer explicitly confirms device validation should the required `EasyTier Core`, `EasyTier GUI`, `EasyTier Mobile`, `EasyTier Test`, and `EasyTier OHOS` workflows be run for the same commit.
5. After all required workflows succeed, run `EasyTier Release` manually and provide the release version; the release workflow creates the tag and GitHub Release.

Notes:

- Do not create a tag or trigger `EasyTier Release` before device validation.
- `EasyTier Release` resolves required workflow run IDs from the selected ref's commit SHA, validates the requested version against Cargo metadata, and refuses to overwrite an existing tag.
- If `EasyTier Release` reports a missing successful workflow run, complete the corresponding formal release build for that commit first.
- OHOS artifacts are included in the GitHub Release. Docker is not part of this fork's release workflow.

## Fork-Specific Changes

This repository is no longer a drop-in mirror of upstream EasyTier. The summary
below reflects the fork-only delta over upstream. If you are comparing behavior
with upstream, upgrading an existing deployment, or deciding which parameters
to enable, read this first:

- Fixes and hardening in this fork: multi-transport stealth rollout and
  compatibility; target-scoped self-loop backoff for direct / hole-punch paths
  instead of broad scheme suppression; QUIC/KCP proxy readiness ACK and
  classified failover; UDP stealth fallback-budget and datagram phase-transition
  fixes; deterministic QUIC/KCP proxy TCP capture source selection and
  exact-local SYN recursion prevention; native TCP proxy NAT-entry lookup /
  handoff fixes; KCP close-path tail-data cleanup; and
  accurate proxy capability advertisement for feature-gated builds.
- Added features in this fork: structured stealth capabilities for `udp`,
  `tcp`, `faketcp`, `quic`, `wg`, `ws`, and `wss`; direct-connect
  `transport_priority`; strict legacy UDP hole-punch rejection control; and
  readiness ACK plus per-transport health / fallback reporting on top of the
  existing QUIC/KCP proxy path. Linux builds with the `tun` feature also add a
  native veth NIC fallback for environments where TUN is blocked by the device
  cgroup or device node policy. Underlay candidate sanitization is enabled by
  default to avoid advertising or dialing common system-TUN/fake-IP ranges and
  EasyTier's own virtual addresses as direct underlays.
- Behavior differences from upstream: strict stealth UDP listeners silently drop
  legacy probes, self-loop mitigation is hardened backoff rather than a promise
  that all residual loop traffic disappears, proxy failover order is fixed to
  `QUIC -> KCP -> Native`, `transport_priority` only affects direct-connect,
  `disable_quic_input` and `disable_kcp_input` do not disable underlying
  listeners, and IPv4 exact-match transport rules win for dual-stack peers.
  When EasyTier runs together with Mihomo/Clash/sing-box TUN, the built-in
  underlay guard reduces polluted direct candidates but is not a hard
  cross-platform promise that every generic underlay socket bypasses the system
  TUN; see [Mihomo TUN interoperability risk](easytier/docs/mihomo_tun_interop.md).
- Fork-added flags in this code line: `--stealth-mode`,
  `--stealth-window-secs`, `--stealth-protocols`,
  `--disable-legacy-udp-hole-punch`, `--transport-priority`,
  `--underlay-candidate-guard`, `--underlay-exclude-cidrs`, and the Linux-only
  `--nic-backend`.
- Existing upstream proxy flags with fork-specific behavior:
  `--enable-kcp-proxy`, `--enable-quic-proxy`, `--disable-kcp-input`, and
  `--disable-quic-input` are not new, but this fork changes failover,
  readiness, health tracking, and capability behavior around them.

Start with [fork differences and configuration notes](easytier/docs/fork_differences.md)
for the full change list, examples, and compatibility boundaries. Stealth,
proxy, and rollout details remain in
[the compatibility notes](easytier/docs/udp_stealth_compatibility.md).

### Common Configuration Pitfalls

- `--transport-priority` must use scoped rules such as
  `global:quic,faketcp,ws,wg,udp,tcp`; a bare list like
  `quic,faketcp,ws,wg,udp,tcp` is invalid and will fail validation.
- `--transport-priority` overrides `default_protocol` for direct-connect.
- `--transport-priority` is latency-bounded: among live connections, a preferred
  transport is selected only when its RTT is at most 125% of the lowest RTT
  connection. This prevents a configured preference from forcing a much slower
  underlay.
- `--stealth-mode` only becomes effective with a non-empty `network_secret`.
  If no explicit `secure_mode` exists, the authentication keypair is derived at
  runtime and is not written back to TOML/RPC. An empty secret warns and stays
  plain.
- Existing configs that already contain an explicit `[secure_mode]` section keep
  legacy plain behavior unless `stealth_mode=true` is also set explicitly.
- `--disable-legacy-udp-hole-punch` still rejects legacy UDP hole-punch
  requests without a stealth preference even when UDP stealth is inactive.
- `--nic-backend tun|veth|auto` is CLI-only and defaults to `tun`. `auto`
  falls back only when the TUN device cannot be created; later MTU, address, or
  route errors remain fatal. `veth`/`auto` conflict with `--no-tun`.
- When EasyTier coexists with Mihomo/Clash/sing-box TUN, verify that EasyTier
  underlay destinations are not captured by the system TUN. A proxy `DIRECT`
  rule may still pass packets through the TUN first; see
  [Mihomo TUN interoperability risk](easytier/docs/mihomo_tun_interop.md).
- `--underlay-candidate-guard` sanitizes advertised/dialed underlay candidates
  plus related bind-source and direct-UDP route-source checks. Guarded public
  IPv4 UDP direct candidates are skipped fail-closed instead of retrying
  through the generic direct fallback path.

### Linux veth NIC fallback

The Linux-only `--nic-backend veth` mode provides the same EasyTier L3 NIC
boundary through an isolated veth peer and AF_PACKET. It requires
`CAP_SYS_ADMIN`, `CAP_NET_ADMIN`, and `CAP_NET_RAW`; it is intended for
containers where network administration is allowed but `/dev/net/tun` or its
device cgroup permission is unavailable, not for ordinary unprivileged
containers. `--nic-backend auto` tries TUN first and preserves TUN as the
default path.

The veth backend reserves `169.254.255.254` and `fe80::e:1` as internal
gateways. Configured addresses and non-default dynamic routes must not contain
these addresses; conflicting routes are rejected instead of being installed.
This option is not stored in TOML, protobuf, or GUI configuration.

Legacy-kernel link-local cleanup failures happen before the device becomes
ready, so startup fails and removes the interface instead of exposing a
partially initialized data path. Prompt veth deletion during instance shutdown
or DHCP rebuild is also intentional; it does not need to wait for every
internal forwarding-task reference to expire naturally. The backend suppresses
link-control protocols on the veth path by design while continuing to forward
ordinary IPv4/IPv6 unicast, broadcast, and multicast data. EasyTier's normal
multicast forwarding does not use IGMP membership for routing, so these
implementation details do not require compatibility changes.

### Stealth and Transport Policy

Stealth defaults to enabled for `udp`, `tcp`, `faketcp`, `quic`, `wg`, `ws`, and `wss`.
It requires a non-empty network secret. Without an explicit `secure_mode`
section, authenticated handshake keys are derived at runtime and are not
serialized. The derived keys are used only for Stealth-protected PeerConn
handshakes; they are not advertised in RoutePeerInfo and do not enable global
relay/session secure mode. Explicit `secure_mode` remains the advanced
credential/Noise configuration. Explicitly clearing `stealth_protocols` restores the
rollout-compatible UDP-only stealth behavior. The effective
`stealth_window_secs` value is network-wide and must match on every stealth node.
`transport_priority` only reorders direct-connect underlays; QUIC/KCP proxy failover keeps
the fixed `QUIC -> KCP -> Native` order. The `transport_priority` syntax is
`scope:proto,...;scope:proto,...`, for example
`global:quic,faketcp,ws,wg,udp,tcp`. It is applied after the 125% RTT
eligibility check for live connections. See
[the compatibility notes](easytier/docs/udp_stealth_compatibility.md) for rollout details.

For operators, the security modes differ as follows:

| Setup | What it protects | What it does not enable |
| --- | --- | --- |
| GUI/new default: `network_secret` + Stealth | Stealth outer handshakes and Stealth-protected PeerConn payloads for configured transports. | RoutePeerInfo public key advertisement, global RelayPeerMap/PeerManager secure relay/session mode, credential identity. |
| Explicit `secure_mode.enabled=true` | Full explicit Noise identity, RoutePeerInfo public key advertisement, secure relay/session semantics, credential-compatible identity. | It is not required just to make default Stealth work. |

In plain terms: GUI Stealth hides and authenticates connection entrances so
`udp`, `tcp`, `faketcp`, `quic`, `wg`, `ws`, and `wss` do not expose plain
EasyTier handshakes to random probes. Explicit `secure_mode=true` is a separate
advanced identity mode: it lets the node publish a Noise public key, be pinned by
other nodes, participate in secure relay/session semantics, and use credential
workflows where temporary nodes can join without knowing the network secret.
The current GUI edits the Stealth preference only; explicit `secure_mode` remains
an advanced CLI/TOML/RPC setting. A GUI plan for that advanced identity setting
is tracked in
[gui_global_secure_identity.md](easytier/docs/todo/gui_global_secure_identity.md).

### 🚀 Basic Usage

#### Quick Networking with Shared Nodes

EasyTier supports quick networking using shared public nodes. When you don't have a public IP, you can use the free shared nodes provided by the EasyTier community. Nodes will automatically attempt NAT traversal and establish P2P connections. When P2P fails, data will be relayed through shared nodes.

When using shared nodes, each node entering the network needs to provide the same `--network-name` and `--network-secret` parameters as the unique identifier of the network.

Taking two nodes as an example (Please use more complex network name to avoid conflicts):

1. Run on Node A:

```bash
# Run with administrator privileges
sudo easytier-core -d --network-name abc --network-secret abc -p tcp://<SharedNodeIP>:11010
```

2. Run on Node B:

```bash
# Run with administrator privileges
sudo easytier-core -d --network-name abc --network-secret abc -p tcp://<SharedNodeIP>:11010
```

After successful execution, you can check the network status using `easytier-cli`:

```text
| ipv4         | hostname       | cost  | lat_ms | loss_rate | rx_bytes | tx_bytes | tunnel_proto | nat_type | id         | version         |
| ------------ | -------------- | ----- | ------ | --------- | -------- | -------- | ------------ | -------- | ---------- | --------------- |
| 10.126.126.1 | abc-1          | Local | *      | *         | *        | *        | udp          | FullCone | 439804259  | 2.6.2-70e69a38~ |
| 10.126.126.2 | abc-2          | p2p   | 3.452  | 0         | 17.33 kB | 20.42 kB | udp          | FullCone | 390879727  | 2.6.2-70e69a38~ |
|              | PublicServer_a | p2p   | 27.796 | 0.000     | 50.01 kB | 67.46 kB | tcp          | Unknown  | 3771642457 | 2.6.2-70e69a38~ |
```

You can test connectivity between nodes:

```bash
# Test connectivity
ping 10.126.126.1
ping 10.126.126.2
```

Note: If you cannot ping through, it may be that the firewall is blocking incoming traffic. Please turn off the firewall or add allow rules.

To improve availability, you can connect to multiple shared nodes simultaneously:

```bash
# Connect to multiple shared nodes
sudo easytier-core -d --network-name abc --network-secret abc -p tcp://<SharedNodeIP1>:11010 -p udp://<SharedNodeIP2>:11010
```

Once your network is set up successfully, you can easily configure it to start automatically on system boot. Refer to the [One-Click Register Service guide](https://easytier.cn/en/guide/network/oneclick-install-as-service.html) for step-by-step instructions on registering EasyTier as a system service.

#### Decentralized Networking

EasyTier is fundamentally decentralized, with no distinction between server and client. As long as one device can communicate with any node in the virtual network, it can join the virtual network. Here's how to set up a decentralized network:

1. Start First Node (Node A):

```bash
# Start the first node
sudo easytier-core -i 10.144.144.1
```

After startup, this node will listen on the following ports by default:
- TCP: 11010
- UDP: 11010
- WebSocket: 11011
- WebSocket SSL: 11012
- WireGuard: 11013

2. Connect Second Node (Node B):

```bash
# Connect to the first node using its public IP
sudo easytier-core -i 10.144.144.2 -p udp://FIRST_NODE_PUBLIC_IP:11010
```

Note: when `--stealth-mode` is enabled, a fixed `udp://` listener no longer accepts
plain SYN probes. A new node dialing a legacy endpoint can retry plain on a fresh
attempt, but a legacy node dialing a strict stealth listener is still silently
dropped. The default `--stealth-protocols` lists all supported transports; an
explicitly empty value is a compatibility override that protects UDP only. See
[stealth compatibility notes](easytier/docs/udp_stealth_compatibility.md).

3. Verify Connection:

```bash
# Test connectivity
ping 10.144.144.2

# View connected peers
easytier-cli peer

# View routing information
easytier-cli route

# View local node information
easytier-cli node
```

For more nodes to join the network, they can connect to any existing node in the network using the `-p` parameter:

```bash
# Connect to any existing node using its public IP
sudo easytier-core -i 10.144.144.3 -p udp://ANY_EXISTING_NODE_PUBLIC_IP:11010
```

### 🔍 Advanced Features

#### Subnet Proxy

Assuming the network topology is as follows, Node B wants to share its accessible subnet 10.1.1.0/24 with other nodes:

```mermaid
flowchart LR

subgraph Node A Public IP 22.1.1.1
nodea[EasyTier<br/>10.144.144.1]
end

subgraph Node B
nodeb[EasyTier<br/>10.144.144.2]
end

id1[[10.1.1.0/24]]

nodea <--> nodeb <-.-> id1
```

To share a subnet, add the `-n` parameter when starting EasyTier:

```bash
# Share subnet 10.1.1.0/24 with other nodes
sudo easytier-core -i 10.144.144.2 -n 10.1.1.0/24
```

Subnet proxy information will automatically sync to each node in the virtual network, and each node will automatically configure the corresponding route. You can verify the subnet proxy setup:

1. Check if the routing information has been synchronized (the proxy_cidrs column shows the proxied subnets):

```bash
# View routing information
easytier-cli route
```

![Routing Information](/assets/image-3.png)

2. Test if you can access nodes in the proxied subnet:

```bash
# Test connectivity to proxied subnet
ping 10.1.1.2
```

#### WireGuard Integration

EasyTier can act as a WireGuard server, allowing any device with a WireGuard client (including iOS and Android) to access the EasyTier network. Here's an example setup:

```mermaid
flowchart LR

ios[[iPhone<br/>WireGuard Installed]]

subgraph Node A Public IP 22.1.1.1
nodea[EasyTier<br/>10.144.144.1]
end

subgraph Node B
nodeb[EasyTier<br/>10.144.144.2]
end

id1[[10.1.1.0/24]]

ios <-.-> nodea <--> nodeb <-.-> id1
```

1. Start EasyTier with WireGuard portal enabled:

```bash
# Listen on 0.0.0.0:11013 and use 10.14.14.0/24 subnet for WireGuard clients
sudo easytier-core -i 10.144.144.1 --vpn-portal wg://0.0.0.0:11013/10.14.14.0/24
```

2. Get WireGuard client configuration:

```bash
# Get WireGuard client configuration
easytier-cli vpn-portal
```

3. In the output configuration:
   - Set `Interface.Address` to an available IP from the WireGuard subnet
   - Set `Peer.Endpoint` to the public IP/domain of your EasyTier node
   - Import the modified configuration into your WireGuard client

#### Self-Hosted Public Shared Node

You can run your own public shared node to help other nodes discover each other. A public shared node is just a regular EasyTier network (with same network name and secret) that other networks can connect to.

To run a public shared node:

```bash
# No need to specify IPv4 address for public shared nodes
sudo easytier-core --network-name mysharednode --network-secret mysharednode
```

## Related Projects

- [ZeroTier](https://www.zerotier.com/): A global virtual network for connecting devices.
- [TailScale](https://tailscale.com/): A VPN solution aimed at simplifying network configuration.

### Contact Us

- 💬 **[Telegram Group](https://t.me/easytier)**
- 👥 **[QQ Group]**
  - No.1 [949700262](https://qm.qq.com/q/wFoTUChqZW)
  - No.2 [837676408](https://qm.qq.com/q/4V33DrfgHe)
  - No.3 [957189589](https://qm.qq.com/q/YNyTQjwlai)

## License

EasyTier is released under the [LGPL-3.0](https://github.com/EasyTier/EasyTier/blob/main/LICENSE).

## Sponsor

CDN acceleration and security protection for this project are sponsored by Tencent EdgeOne.

<p align="center">
  <a href="https://edgeone.ai/?from=github" target="_blank">
    <img src="assets/edgeone.png" width="200" alt="EdgeOne Logo">
  </a>
</p>

Special thanks to [Langlang Cloud](https://langlangy.cn/?i26c5a5)  and [RainCloud](https://www.rainyun.com/NjM0NzQ1_) for sponsoring our public servers.

<p align="center">
<a href="https://langlangy.cn/?i26c5a5" target="_blank">
<img src="assets/langlang.png" width="200">
</a>
<a href="https://langlangy.cn/?i26c5a5" target="_blank">
<img src="assets/raincloud.png" width="200">
</a>
</p>


If you find EasyTier helpful, please consider sponsoring us. Software development and maintenance require a lot of time and effort, and your sponsorship will help us better maintain and improve EasyTier.

<p align="center">
<img src="assets/wechat.png" width="200">
<img src="assets/alipay.png" width="200">
</p>
