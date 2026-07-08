# Transport Priority And Underlay Breaker Validation - 2026-07-09

This note records the 2026-07-09 validation for direct transport priority,
WAN/LAN candidate classification, and the underlay breaker. It intentionally
uses sanitized host labels and avoids real public domains, public IP addresses,
network secrets, or operator-specific bootstrap endpoints.

## Scope

- Code line: `releases/v2.6.9`.
- Test binaries: Linux `x86_64-unknown-linux-musl` `easytier-core` and
  `easytier-cli` built on the remote builder.
- Test nodes: two Linux nodes with dual-stack public connectivity and EasyTier
  virtual IPs, referenced below as `node-a` and `node-b`.
- Common policy:

```text
stealth_mode=true
stealth_protocols=udp,tcp,faketcp,quic,wg,ws,wss
transport_priority=global:quic,faketcp,ws,wg,udp,tcp
underlay_candidate_guard=true
```

## Findings

### WAN/LAN Classification

A real classification bug was found and fixed: public IPv4/IPv6 addresses could
previously be classified as LAN when a remote public address happened to fall
inside a local interface prefix. This is common on VPS providers that assign
multiple hosts from the same public IPv6 aggregate. Treating such candidates as
LAN allowed lower-priority public TCP/WS candidates to suppress WAN QUIC or
FakeTCP, which violated the user's global transport preference.

The current rule is intentionally conservative:

- IPv4 private addresses are LAN only when they match one of the local interface
  networks.
- IPv6 ULA addresses are LAN only when they match one of the local interface
  networks.
- IPv4/IPv6 link-local addresses are LAN only when they match a local link-local
  network.
- Public IPv4/IPv6 addresses are WAN even if the kernel reports an on-link public
  prefix.

This can classify routed private networks, CGNAT, company VPN ranges, Docker
subnets, or Tailscale-like ranges as WAN rather than LAN. That is a safer
default: it may lose the special `lan:` priority bucket, but it should not break
connectivity. The opposite mistake, classifying public/VPN/fake/link-local
candidates as LAN, can block the preferred WAN protocol family.

If future behavior needs to treat routed private networks as LAN, the preferred
extension is a route lookup that only accepts directly attached `scope link`
paths. It should not blanket-classify all RFC1918, CGNAT, or ULA space as LAN.

### Transport Priority Runtime Behavior

After deploying the validated build to both nodes:

- `node-a -> node-b` had a direct P2P route with `quic,tcp`.
- `node-b -> node-a` had a direct P2P route with `tcp,quic`.
- Measured peer RTT was around sub-millisecond on both sides.
- Relay routes were still visible as backup route-table entries, but the route
  table also contained a lower-latency `DIRECT` path. A visible relay row is not
  by itself evidence that data is currently using relay.

The current data-plane selection still applies the RTT guard before protocol
preference. A preferred protocol is eligible when it is within the normal 125%
RTT window or within a small absolute slack for sub-millisecond links. This
prevents a preferred QUIC/FakeTCP connection from being rejected solely because a
near-zero-latency TCP connection is a few hundred microseconds faster.

### Underlay Breaker

No `underlay breaker` or `breaker gated` events were observed during the
two-node validation window. CPU and RSS remained stable on both test nodes.

The current breaker configuration is deliberately conservative for this release:

```text
hard strike threshold: 100
strike window: 10s
initial TTL: 30s
max TTL: 300s
soft TTL: 30s
half-open timeout: 30s
```

This greatly reduces the chance that the breaker blocks normal traffic. It also
means the breaker is now primarily a safety valve for clear loop storms, not an
aggressive first-line failover mechanism. The high-confidence signals remain
scoped by Endpoint or Peer plus protocol and scope; soft source-interface signals
do not gate traffic.

### KCP/QUIC Proxy To Local Virtual-IP Listeners

A separate proxy regression was found during the same validation pass. When a
remote peer used KCP or QUIC proxy to reach a service bound to this node's
EasyTier virtual IP, the responder applied the local "occupied virtual-IP port"
guard before considering the request origin. That incorrectly returned a policy
denial for legitimate remote traffic to a local virtual-IP listener.

The fix keeps the original loopback protection for self-originated traffic, but
disables only the final occupied-port guard for remote-origin KCP/QUIC proxy
requests. All other proxy denial checks remain in place, including EasyTier
internal listener/RPC protection and local physical-address safeguards.

The full port-forward matrix passed after this change:

- KCP enabled and disabled on source/destination combinations.
- Relay KCP enabled and disabled combinations.
- Small-buffer forwarding cases.
- Local virtual-IP listener destinations.

This fix does not change SOCKS KCP-only behavior, Proxy failover ordering, wire
format, or listener configuration.

### Remaining Non-Blocking Observations

- QUIC and FakeTCP are currently advertised as IPv4 unspecified listeners in the
  tested default listener set. They are not automatically expanded to IPv6 in the
  same way as TCP/UDP/WG/WS/WSS. This limits candidate diversity on IPv6 and is
  separate from the WAN/LAN bug.
- A transient `decryption failed` / `session invalidated` error was observed on
  one node after restart. The direct P2P route recovered and stayed usable. This
  should be tracked separately from transport priority and breaker behavior.

## Verification Commands

The validation used these categories of commands, with hostnames and addresses
redacted here:

```bash
easytier-cli -p 127.0.0.1:<rpc-port> node
easytier-cli -p 127.0.0.1:<rpc-port> peer
easytier-cli -p 127.0.0.1:<rpc-port> route
journalctl -u <easytier-service> --since "3 minutes ago"
grep -aiE "underlay breaker|breaker gated|source interface|decryption failed"
ps -o pid,pcpu,pmem,rss,etime,cmd -p <easytier-core-pid>
```

Remote build and targeted tests were run on the remote builder, not on the
maintainer's local machine:

```bash
cargo test -p easytier direct_candidates_are_classified_after_address_resolution --lib -- --nocapture
cargo test -p easytier transport_preference --lib -- --nocapture
cargo test -p easytier underlay_breaker --lib -- --nocapture
cargo test -p easytier peer_conn_empty_public_key_does_not_block_later_authenticated_conn --lib -- --nocapture
cargo test -p easytier tests::three_node::port_forward_test --features full -- --nocapture --test-threads=1
cargo test -p easytier common::global_ctx::tests --features full -- --nocapture --test-threads=1
cargo test -p easytier tests::three_node::wg_stealth_three_node_carries_phase2_tcp --features full -- --nocapture --test-threads=1
cargo test -p easytier tests::three_node::ws_and_wss_stealth_three_node_carry_phase2_tcp --features full -- --nocapture --test-threads=1
cargo test -p easytier tests::three_node::quic_proxy --features full -- --nocapture --test-threads=1
cargo test -p easytier tests::three_node::quic_stealth --features full -- --nocapture --test-threads=1
cargo check -p easytier --lib
cargo build -p easytier --bin easytier-core --bin easytier-cli --release --target x86_64-unknown-linux-musl
```

## Conclusion

The transport-priority regression observed after the earlier v2.6.9 build was
not caused by user configuration syntax and was not reproduced as a breaker
false positive. The validated fixes address the confirmed classification and
selection issues:

- Public IPv4/IPv6 no longer become LAN solely because they are on-link.
- Link-local addresses no longer become LAN unless they match the local
  link-local network.
- Sub-millisecond RTT links no longer reject preferred protocols purely because
  the 125% relative window is too narrow.
- Plain or legacy connections with an empty secure public key no longer block a
  later authenticated connection from the same peer.
- Remote-origin KCP/QUIC proxy can reach services bound to the destination
  node's EasyTier virtual IP without disabling the self-loop occupied-port guard
  for local-origin traffic.

The remaining QUIC/FakeTCP IPv6 listener-advertisement limitation and transient
session invalidation should be treated as separate follow-up items.
