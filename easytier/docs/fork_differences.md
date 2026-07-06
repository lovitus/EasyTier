# Fork Differences From Upstream EasyTier

This document is the fork-level summary that the main README links to. It was
cross-checked against the current local code line and `upstream/main`. Use it to
answer four practical questions quickly:

- What problems were fixed in this fork?
- What features were added here but are not part of upstream behavior?
- What compatibility or behavior differences should operators watch for?
- Which fork-added parameters exist, and which upstream parameters behave
  differently here?

Detailed stealth rollout notes still live in
[udp_stealth_compatibility.md](udp_stealth_compatibility.md).

## 1. Problems Fixed In This Fork

### Stealth and underlay compatibility

- Added structured multi-transport stealth capability advertisement and
  compatibility handling instead of treating stealth as UDP-only.
- Fixed direct-connect and hole-punch behavior so new nodes can distinguish
  strict stealth listeners, rollout-compatible fallback paths, and explicit
  plain requests.
- Documented and enforced the network-wide meaning of `stealth_window_secs`.
- Preserved the manual UDP stealth fallback budget so a failed first stealth
  attempt does not consume the entire outer timeout budget.
- Fixed datagram stealth phase-transition timing so gate-key and connection
  outer-key switching happens at the intended handshake boundary.

### Loop backoff and connection stability

- Hardened self-loop handling so only post-underlay `peer id conflict` signals
  trigger loop backoff; generic timeouts, refusals, and unrelated network
  errors are no longer treated as self-loop evidence.
- Moved new direct / hole-punch backoff behavior to target-scoped TTL
  blacklists instead of escalating single-target loops into broad scheme-wide
  suppression.
- Kept the old scheme/scope suppression state as a compatibility safety rail,
  but stopped using it as the main gate for the newer direct / hole-punch
  paths.
- Preserved native TCP proxy NAT-entry lookup and handoff so real local entries
  win over fake-local fallback and first ACK/Data does not fall into a lookup
  gap during connection activation.
- This is loop/backoff hardening, not a claim that all loop traffic has been
  eliminated. Residual loop traffic may still be observed.

### QUIC/KCP Proxy reliability

- Fixed QUIC/KCP proxy prepare so source-side fallback decisions can wait for
  remote destination readiness instead of assuming that a transport stream alone
  means success.
- Added explicit readiness ACK semantics and failure classification for proxy
  fallback decisions.
- Fixed the local TCP capture path shared by QUIC/KCP proxy, which previously
  could pick an unusable pseudo-source address from the virtual subnet.
- Fixed KCP close-path behavior so tail data is drained correctly and broken
  connections do not remain in live state forever.

### Capability advertisement and selection

- Fixed proxy input advertisement so feature flags match compiled capabilities
  rather than only runtime `disable_*_input` switches.
- Documented the effective direct-connect precedence for dual-stack exact-IP
  transport rules.

## 2. Features Added In This Fork

- Multi-transport stealth for `udp`, `tcp`, `faketcp`, `quic`, `wg`, `ws`, and
  `wss`.
- `transport_priority` for direct-connect underlay ordering with `global`,
  `wan`, `lan`, and exact virtual-IP scopes.
- `disable_legacy_udp_hole_punch` to reject old UDP hole-punch RPCs that do not
  carry a stealth preference.
- Readiness ACK, classified fallback reasons, and per-transport health tracking
  built on top of the pre-existing QUIC/KCP proxy path.
- A Linux native veth NIC backend for containers that have network-management
  capabilities but cannot open or create TUN devices.
- Fork-specific GitHub Actions release flow that requires successful build/test
  runs before the release workflow is triggered.

## 3. Behavior Differences And Compatibility Boundaries

These are intentional differences from upstream behavior and need to be visible
to operators:

- A fixed stealth `udp://` listener does not accept legacy plain SYN probes.
  Legacy nodes dialing a strict stealth listener are silently dropped by design.
- Empty `stealth_protocols` keeps the rollout-compatible UDP-only stealth
  behavior; explicitly listing protocols opts those transports into stealth.
- `stealth_window_secs` is a network-wide value. `0` means 60 seconds, and all
  stealth-enabled nodes in the same network must use the same effective value.
- `transport_priority` only reorders direct-connect attempts. It does not
  reorder manual/bootstrap URLs and does not affect proxy failover.
- QUIC/KCP proxy failover order is fixed to `QUIC -> KCP -> Native`.
- The `failover` table is short-lived TCP SYN selection state, not a list of
  wrapped connections or loop-suppression entries. Prepared-stream handoff
  deliberately uses a local-to-local PeerManager marker; its TTL-bound status
  does not affect remote QUIC/KCP health or handoff. An original SYN whose
  destination exactly matches the local virtual IP bypasses the selector before
  state is created, preventing a local proxy destination socket from being
  recaptured by TUN and recursively amplified. Status output shows only the
  latest 256 entries for diagnostic stability.
- `disable_quic_input` and `disable_kcp_input` only disable QUIC/KCP Proxy
  inbound capability advertisement and acceptance. They do not disable the
  underlying `quic://` or `kcp`-backed listener path.
- When a dual-stack peer matches both exact IPv4 and exact IPv6 transport rules,
  IPv4 wins deterministically because the chosen direct tunnel is peer-scoped.
- `default_protocol` is still accepted, but once `transport_priority` is set it
  becomes a compatibility fallback only and direct-connect follows
  `transport_priority`.
- Self-loop mitigation is now target-scoped backoff, not a guarantee that every
  remaining loop-flow pattern disappears.

## 4. Fork-Added Parameters

The table below only lists parameters that are not present in `upstream/main`
and are part of this fork's operator-facing surface.

| CLI flag | Env | Purpose | Notes |
| --- | --- | --- | --- |
| `--stealth-mode` | `ET_STEALTH_MODE` | Enable stealth for the configured transports. | Requires secure mode and a non-empty `network_secret`. |
| `--stealth-window-secs <n>` | `ET_STEALTH_WINDOW_SECS` | Set the gate-key rolling window. | `0` means 60 seconds; all stealth nodes in one network must match. |
| `--stealth-protocols <list>` | `ET_STEALTH_PROTOCOLS` | Comma-separated stealth transports. | Empty means UDP-only stealth for rollout compatibility. |
| `--disable-legacy-udp-hole-punch` | `ET_DISABLE_LEGACY_UDP_HOLE_PUNCH` | Reject legacy UDP hole-punch RPCs without stealth preference. | New peers that explicitly request plain remain allowed. |
| `--transport-priority <rules>` | `ET_TRANSPORT_PRIORITY` | Reorder direct-connect underlays. | Format is `scope:proto,...;scope:proto,...`, for example `global:quic,faketcp,ws,udp,tcp`. |
| `--nic-backend <tun|veth|auto>` | None | Select the Linux virtual NIC backend. | CLI-only in Linux `tun` builds; defaults to `tun` and is not serialized to TOML/protobuf. |

Upstream-style proxy flags such as `--enable-kcp-proxy`, `--enable-quic-proxy`,
`--disable-kcp-input`, and `--disable-quic-input` still exist in this fork.
What changed here is not the existence of those flags, but the behavior around
Proxy readiness ACK, failover classification, health tracking, and capability
advertisement.

## 5. Existing Parameters With Fork-Relevant Behavior

These flags are not new, but operators comparing this fork with upstream should
still pay attention to them because the fork changes behavior around their
execution path.

| CLI flag | Env | Existing purpose | Fork-relevant note |
| --- | --- | --- | --- |
| `--enable-kcp-proxy` | `ET_ENABLE_KCP_PROXY` | Allow TCP-to-KCP proxying on the source side. | Source-side prepare/fallback now uses readiness ACK and classified failure reasons. |
| `--disable-kcp-input` | `ET_DISABLE_KCP_INPUT` | Disable KCP Proxy inbound capability. | Capability advertisement is now expected to match both runtime config and compiled features. |
| `--enable-quic-proxy` | `ET_ENABLE_QUIC_PROXY` | Allow TCP-to-QUIC proxying on the source side. | Source-side QUIC proxy may still fall back to KCP or native TCP, but classification and health accounting are more precise. |
| `--disable-quic-input` | `ET_DISABLE_QUIC_INPUT` | Disable QUIC Proxy inbound capability. | It still does not disable `quic://` listeners; only QUIC Proxy inbound capability is affected. |

## 6. Configuration Conflicts And Common Mistakes

- `--transport-priority` must use scoped rules such as
  `global:quic,faketcp,ws,wg,udp,tcp`. A bare list like
  `quic,faketcp,ws,wg,udp,tcp` is rejected with
  `failed to parse transport_priority`.
- Once `--transport-priority` is set, `default_protocol` becomes a compatibility
  fallback only for direct-connect. Operators should not expect both settings to
  co-control the same path.
- Data-plane transport preference is bounded by latency. A preferred live
  connection is selected only after the peer filters out connections whose RTT
  is greater than 125% of the lowest RTT connection.
- `--stealth-mode` without secure mode or a non-empty `network_secret` does not
  become a hard config error. Startup warns and the node continues in plain
  mode.
- A custom `--stealth-window-secs` value must match every stealth-enabled node
  in the same network.
- `--disable-legacy-udp-hole-punch` still rejects old requests without stealth
  preference even when UDP stealth is currently inactive.
- `stealth_protocols` entries that are not compiled into the current build are
  warned and skipped rather than silently becoming active.
- `--nic-backend veth` and `auto` conflict with `--no-tun`. The veth backend
  requires `CAP_SYS_ADMIN`, `CAP_NET_ADMIN`, and `CAP_NET_RAW`; it is not an
  unprivileged-container fallback.
- `auto` falls back only for a TUN open/create failure with an expected device
  availability or permission errno. MTU, address, and route failures do not
  trigger veth fallback.
- The veth backend reserves `169.254.255.254` and `fe80::e:1`. Non-default
  configured or dynamic routes containing either internal gateway are rejected.

### Reviewed veth lifecycle boundaries

The following behaviors have been checked against their production call paths
and Linux 3.10 runtime behavior. They are not outstanding functional defects:

- Kernels without `addr_gen_mode` use bounded link-local cleanup. A cleanup
  failure happens before `TunDeviceReady`; initialization propagates the error
  and destroys the veth instead of exposing a partially initialized data path.
- Immediate veth cleanup from `VirtualNic` destruction is intentional during
  instance shutdown and DHCP rebuild. Internal forwarding tasks are being
  cancelled at that point, so interface deletion does not need to wait for
  every task-held `Arc` to expire naturally.
- The veth stream suppresses NDP, MLD, IGMP, and similar link-control packets
  that match its internal-control rules. Ordinary IPv4/IPv6 unicast, broadcast,
  multicast UDP, and other user data continue to pass. EasyTier floods
  multicast destinations to the relevant peers rather than routing from IGMP
  membership, so normal multicast behavior is unaffected. Raw IGMP tunnelling
  for an IGMP proxy or multicast-routing daemon would be a separate future
  feature.
- Address rollback failures are retained in an orphan registry capped at 256
  entries and retried during later configuration and cleanup. This cannot
  become unbounded memory growth. A slot can remain occupied only in exceptional
  races such as an external process deleting the address concurrently, which
  is not a current functional blocker.
- Linux may remove an address-dependent explicit route when the address is
  deleted. A following `ESRCH` or `ENOENT` is treated as idempotent success, and
  the backend still clears its IPv4/IPv6 route cache and directed-broadcast
  state.

## 7. Configuration Examples

### CLI example

The last two flags below already exist upstream; they are included here because
this fork changes behavior around the QUIC/KCP Proxy path.

```bash
easytier-core \
  --network-name demo \
  --network-secret demo-secret \
  --secure-mode \
  --stealth-mode \
  --stealth-window-secs 60 \
  --stealth-protocols udp,tcp,faketcp,quic,wg,ws,wss \
  --transport-priority 'global:quic,faketcp,ws,wg,udp,tcp' \
  --enable-quic-proxy \
  --enable-kcp-proxy
```

### TOML example

```toml
[flags]
stealth_mode = true
stealth_window_secs = 60
stealth_protocols = "udp,tcp,faketcp,quic,wg,ws,wss"
disable_legacy_udp_hole_punch = false
transport_priority = "global:quic,faketcp,ws,wg,udp,tcp"
enable_quic_proxy = true
enable_kcp_proxy = true
disable_quic_input = false
disable_kcp_input = false
```

## 8. Recommended Upgrade Reading Order

If you are migrating from upstream or comparing two branches, read in this
order:

1. This document for the fork-level summary.
2. [udp_stealth_compatibility.md](udp_stealth_compatibility.md) for stealth,
   proxy, rollout, and transport-priority details.
3. [SOCKS5 performance investigation and maintenance boundaries](socks5_performance_investigation.md)
   when diagnosing SOCKS5, `no_tun`, or QUIC/KCP Proxy throughput. It prevents
   destination-side `no_tun` TCP ingress cost from being misattributed to the
   SOCKS5 source path.
4. The main README section "Fork-Specific Changes" for the short operator-facing
   summary shown on the project front page.
