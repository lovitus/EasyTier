# EasyTier And Mihomo TUN Interoperability Risk

This note documents a known interoperability boundary when EasyTier runs
alongside Mihomo/Clash/sing-box system TUN mode. It is a diagnostic and
reproduction guide, not a completed fix list.

## Symptoms

- Mihomo keeps one or more CPU cores busy.
- EasyTier core also shows sustained CPU, while the EasyTier virtual NIC itself
  carries little traffic.
- Mihomo's connection list shows many `easytier-gui` / `easytier-core`
  connections targeting EasyTier listener ports such as `11010`, `11011`,
  `11012`, or `11013`.
- EasyTier Proxy Failover may show a few selector entries, but the failover
  table is usually not the unbounded state that causes the CPU load.
- Restarting EasyTier or Mihomo releases the current CPU load; repeated
  KCP/QUIC/direct-connect tests can trigger it again.

## Root Cause

When Mihomo TUN captures the system route, EasyTier underlay sockets may enter
Mihomo first and then be forwarded by Mihomo. Two things then interact badly:

1. EasyTier can collect proxy TUN/fake-IP addresses, its own virtual NIC
   address, or other virtual-interface addresses as ordinary local interface
   addresses and advertise them to peers.
2. Direct-connect expands remote `0.0.0.0` listeners into those addresses and
   creates direct candidates that should never be attempted. KCP, QUIC, WS, and
   FakeTCP candidates can then be captured by Mihomo TUN again.

This happens below QUIC/KCP Proxy failover. The failover table is short-lived TCP
SYN selector state; Mihomo TUN captures EasyTier's underlay sockets.

## Reproduction

1. Enable Mihomo TUN with route capture and fake-IP mode:

   ```yaml
   tun:
     enable: true
     auto-route: true
     stack: mixed
   dns:
     fake-ip-range: 198.18.0.1/16
   ```

2. Run EasyTier with a virtual NIC and multiple listeners:

   ```bash
   easytier-core \
     --network-name demo \
     --network-secret demo-secret \
     -i 10.44.0.3/16 \
     -l tcp://0.0.0.0:11010 \
     -l udp://0.0.0.0:11010 \
     -l ws://0.0.0.0:11011 \
     -l quic://0.0.0.0:11012 \
     -l faketcp://0.0.0.0:11013 \
     --enable-quic-proxy \
     --enable-kcp-proxy
   ```

3. Generate KCP/QUIC/direct-connect traffic, for example by opening TCP flows to
   remote virtual IPs or running many short connections.

4. Confirm that peer public/LAN candidate routes point at the proxy TUN instead
   of a physical interface:

   ```bash
   route -n get <peer-public-ip>      # macOS
   ip route get <peer-public-ip>      # Linux
   Get-NetRoute -AddressFamily IPv4   # Windows PowerShell
   ```

5. Check EasyTier's advertised addresses:

   ```bash
   easytier-cli -p 127.0.0.1:<rpc-port> node
   ```

   Risk signals include `198.18.0.0/15` addresses, the local EasyTier virtual
   IP, or proxy TUN ULA addresses in the interface list.

6. Check Mihomo connections:

   ```bash
   curl -H "Authorization: Bearer <secret>" \
     http://127.0.0.1:9090/connections
   ```

   Look for EasyTier processes with `metadata.type = Tun` and destination ports
   equal to EasyTier listener ports.

## Temporary Mitigation

- Exclude the EasyTier process or EasyTier underlay destinations at the TUN
  bypass/route-exclude layer. A plain `DIRECT` rule may still pass packets
  through the TUN first, so it may not be enough.
- For Mihomo/Clash-style rule lists, put EasyTier and commonly co-installed
  Tailscale process bypass rules before generic proxy rules:

  ```yaml
  - PROCESS-NAME,io.tailscale.ipn.macsys.network-extension,DIRECT
  - PROCESS-NAME,tailscaled,DIRECT
  - PROCESS-NAME,tailscaled.exe,DIRECT
  - PROCESS-NAME,tailscale,DIRECT
  - PROCESS-NAME,tailscale.exe,DIRECT
  - PROCESS-NAME,easytier-gui,DIRECT
  - PROCESS-NAME,easytier-gui.exe,DIRECT
  - PROCESS-NAME,easytier-core,DIRECT
  - PROCESS-NAME,easytier-core.exe,DIRECT
  - PROCESS-NAME-REGEX,(?i)^easytier(?:[-_.].*)?$,DIRECT
  - PROCESS-NAME,easytier-*,DIRECT
  - PROCESS-NAME,easytier-cli,DIRECT
  - PROCESS-NAME,easytier-cli.exe,DIRECT
  ```

  `PROCESS-NAME,easytier-*` depends on client wildcard support; keep the exact
  names and regex rule. Before the final `MATCH`/fallback rule, also direct the
  Tailscale CGNAT range and the EasyTier virtual CIDR:

  ```yaml
  - IP-CIDR,100.64.0.0/10,DIRECT,no-resolve
  - IP-CIDR,10.44.0.0/16,DIRECT,no-resolve
  ```

  Replace `10.44.0.0/16` if your EasyTier virtual network is different. For
  sing-box, NekoBox, Throne, or other clients, use equivalent process bypass and
  route-exclude settings.
- Do not bind local SOCKS/proxy-chain entry points to the EasyTier virtual IP.
  Prefer `127.0.0.1` and avoid sending overlay traffic back into a local proxy
  entry point.
- For pure QUIC/KCP/Proxy Failover tests, temporarily disable the system TUN or
  verify that EasyTier underlay routes use a physical interface.
- Restarting EasyTier and Mihomo releases the current CPU loop, but it is only
  a recovery step.

## EasyTier-Side Guard

Forcing EasyTier sockets to bypass another system TUN on macOS, Windows, and
Linux without configuration is not a small cross-platform switch. Linux can use
socket marks and policy routing; macOS and Windows require different interface
binding or system-extension semantics.

This fork ships a smaller fail-safe guard instead. It is enabled by default and
applies only to underlay candidate sanitization plus a local runtime breaker:

- `--underlay-candidate-guard` defaults to `true`.
- The built-in fake-IP base set is
  `198.18.0.0/15,fc00::/18,fdfe:dcba:9876::/48,192.19.0.0/24`, covering common
  Mihomo/sing-box/Clash/V2Ray/Xray/Surge fake-IP pools.
- `--underlay-exclude-cidrs` is an additive user list. Clearing it disables
  only user-added CIDRs, not the built-in base set, runtime EasyTier virtual IP
  filtering, or the older EasyTier-managed IPv6 filtering.
- GUI exposes the same setting as **Underlay Candidate Sanitization** plus a
  single editable CIDR list.
- The guard filters local IP advertisement, direct candidate expansion, IPv6
  hole-punch candidates, direct UDP route-source validation before hole-punch
  RPCs, inbound hole-punch RPC connector addresses, generic connector
  destination/source addresses, and connector bind-source lists.
- Generic connector and direct validation use a temporary connected UDP socket
  to discover the system-selected source IP. A target IP or source IP matching
  the built-in, user, or runtime guard is rejected for that candidate.
- If the source IP does not match a CIDR guard but maps to a suspicious
  `utun`/`tun`/`tap`/`wintun`/point-to-point interface, v1 records only a
  warning and a bounded soft strike. It does not hard reject and does not trip
  the breaker.
- The internal breaker stores at most 4096 keys, either
  `Endpoint(remote_addr, scheme, scope)` or `Peer(peer_id, scheme, scope)`.
  Three hard strikes in 30 seconds block the key for 300 seconds, with repeated
  triggers backing off up to 1800 seconds.
- Hard strikes are limited to high-confidence signals: guard hard hits,
  handshake peer mismatch for a known expected peer, and the existing self-loop
  detection. Prepare timeout, ACL/Policy denial, destination refusal, and normal
  fast failures only log diagnostics.
- Peer and Endpoint breaker keys are acquired atomically with one lease, so
  mismatched TTLs cannot consume each other's half-open attempt. A cancelled
  preflight rolls back only its own lease; the lease is committed immediately
  before the first real connection or hole-punch side effect.
- After TTL expiry, only one half-open attempt per key set is released. Direct,
  generic, and TCP/UDP hole-punch keys clear only after the authenticated
  PeerConn receives its first pong; a handshake alone never clears a breaker.
- A guarded public IPv4 UDP direct candidate is skipped fail-closed. It is not
  retried through the generic direct UDP fallback path.
- Setting `underlay_candidate_guard=false` bypasses these new sanitization hooks
  and breaker behavior: no breaker gate, no hard/soft strike, and no TTL. It
  keeps only the older EasyTier-managed IPv6 filtering.

Listeners may still bind to `0.0.0.0`; the guard filters what EasyTier
advertises or dials as underlay candidates. It does not change PeerManager,
Proxy, Stealth, SOCKS, wire format, or the fixed QUIC/KCP proxy failover order.
A hard guarantee that every generic underlay socket bypasses a system TUN still
requires process/route bypass from the TUN implementation or a future
platform-specific socket protect/bind layer.
