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
applies only to underlay candidate sanitization:

- `--underlay-candidate-guard` defaults to `true`.
- `--underlay-exclude-cidrs` defaults to
  `198.18.0.0/15,fdfe:dcba:9876::/48,192.19.0.0/24`.
- GUI exposes the same setting as **Underlay Candidate Sanitization** plus a
  single editable CIDR list.
- The guard filters local IP advertisement, direct candidate expansion, IPv6
  hole-punch candidates, direct UDP route-source validation before hole-punch
  RPCs, and connector bind-source lists.
- A guarded public IPv4 UDP direct candidate is skipped fail-closed. It is not
  retried through the generic direct UDP fallback path.
- Clearing `underlay_exclude_cidrs` disables configured CIDR filtering only.
  Runtime EasyTier virtual-address filtering remains active while the guard is
  enabled.
- Setting `underlay_candidate_guard=false` bypasses these new sanitization hooks
  and keeps only the older EasyTier-managed IPv6 filtering.

Listeners may still bind to `0.0.0.0`; the guard filters what EasyTier
advertises or dials as underlay candidates. It does not change PeerManager,
Proxy, Stealth, SOCKS, wire format, or the fixed QUIC/KCP proxy failover order.
A hard guarantee that every generic underlay socket bypasses a system TUN still
requires process/route bypass from the TUN implementation or a future
platform-specific socket protect/bind layer.
