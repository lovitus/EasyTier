# EasyTier Changes

## v3.0.9

Release candidate based on v3.0.8.

### Changes since v3.0.8

- Fix a long-standing WireGuard listener race where concurrent peer cleanup
  could remove a session between packet classification and dispatch, causing
  the process-wide panic handler to terminate EasyTier.
- Keep the already captured `Arc<WgPeer>` alive for the in-flight datagram
  instead of performing a second fallible peer-table lookup.
- Cover both ordinary `wg://` listeners and the WireGuard VPN Portal, which
  share the same listener implementation.
- Add a deterministic regression test that removes the peer after the listener
  captures it and before dispatch resumes.

### Compatibility

- WireGuard session expiry, stopped-session cleanup, handshake replacement,
  stealth framing, listener configuration, and subsequent reconnect behavior
  are unchanged.
- TCP, UDP, WS/WSS, QUIC/QUIC-Brutal, and FakeTCP are unaffected.

## v3.0.8

Release candidate based on v3.0.7.

### Changes since v3.0.7

- Add a bounded, per-network failure table for automatic P2P endpoints. Repeated
  complete failures of the same peer, protocol, IP, and port cool down for
  1, 2, 4, 8, then at most 10 minutes.
- Apply the shared guard to Direct candidates, priority upgrade/fallback/probe,
  UDP hole punching, and TCP simultaneous-open without changing their existing
  scheduling or internal retry algorithms.
- Preserve every first attempt, every new endpoint, all attempts within one
  Direct candidate, and the existing UDP/TCP hole-punch backoff rounds needed
  for broadcast, fanout, and port guessing.
- Bound the table to 65,536 entries across 64 shards and keep it outside packet
  send/receive hot paths.
- Add the opt-out flag `disable_p2p_storm_throttle` to TOML, CLI, environment,
  managed configuration, and GUI. It restores the former unrestricted retry
  behavior and clears existing cooldown state.

### Compatibility

- `lazy_p2p`, `need_p2p`, `disable_p2p`, transport priority, protocol
  upgrade/fallback/probe, relay, and disconnect-recovery decisions retain their
  existing meaning.
- Manual connectors, listeners, established tunnel traffic, DNS, STUN, policy
  proxy traffic, and routing are outside this guard.
- The opt-out flag defaults to false, so repeated-failure throttling is enabled
  unless an operator explicitly disables it.

## v3.0.7

Release candidate based on v3.0.6.

### Changes since v3.0.6

- Open Mihomo Zashboard in one EasyTier-managed webview window instead of the
  operating system's default browser. This avoids the Windows
  `ShellExecuteExW` failure that could surface as DLL initialization error and
  `os error 1223`.
- Repeated Zashboard requests reuse, navigate, show, unminimize, and focus the
  existing window rather than creating duplicate windows.
- Closing Zashboard destroys only that auxiliary window. The main GUI keeps
  its existing close-to-background behavior.
- An unfocused Zashboard window closes after 15 minutes. The timeout is driven
  by focus events and generation cancellation; it adds no polling loop and
  does not change the GUI's existing background throttling.
- The Zashboard window receives no Tauri plugin capability. Mihomo's controller
  remains loopback-only and the existing runtime URL/secret generation is
  unchanged.

### Compatibility

- Core networking, Mihomo lifecycle/configuration, GOST, Leaf, Android, and
  OHOS behavior are unchanged from v3.0.6.
- The release still carries the platform and policy-routing boundaries
  documented for v3.0.6.

### Release evidence

- Released on 2026-07-29 from
  `50cf3de5a6179a05fd574f802814c154212b4b56`.
- Core: `30426581269`
- GUI: `30426583307`
- Mobile: `30426585308`
- OHOS: `30426587089`
- Test: `30426588901`
- Release: `30431643939`
- Published assets: 46
- The published macOS ARM64 DMG SHA-256 matched the exact formal GUI artifact
  used for the real-device Zashboard lifecycle validation.

## v3.0.6

Released on 2026-07-29 from
`a8a11c076c75a109c1fd882a32b206a60b0bf60f`.

### Major changes since v2.6.10

- Added optional policy routing with three explicit backends: `off`, `mihomo`,
  and the retained `leaf` compatibility backend.
- Added the desktop and Unix Mihomo backend without parsing or rewriting user
  proxy nodes, groups, rules, subscriptions, or provider semantics. EasyTier
  validates a managed runtime copy while the user-selected YAML remains the
  authoritative source.
- Added a Core-owned GOST SOCKS5 mesh entry with TCP, UDP, bounded restart,
  readiness checks, loopback process listeners, and parent-owned cleanup.
  Mihomo can use standard SOCKS5 and `dialer-proxy` chains to enter the mesh.
- Extended the Leaf compatibility backend with Shadowsocks/UoT, Trojan, VMess,
  VLESS, WebSocket/TLS transport composition, bounded dual-stack FakeDNS,
  GeoIP/GeoSite category indexing, port-range rules, DNS defaults, and
  dual-stack native proxy endpoint handling.
- Added compact policy editing, separate Leaf and Mihomo YAML state, backend
  validation, local Zashboard access, runtime status, field examples, and
  cached Geo category selection in the GUI.
- Improved macOS policy DNS and interface-change recovery, scoped routing,
  QUIC truncated-datagram handling, and interface snapshot reuse.
- Improved Android Leaf/HEV ownership, DNS/network recovery, configuration
  retention, FakeDNS, VPN capture, and lifecycle handling. Android and OHOS do
  not enable the Mihomo backend.
- Hardened child-process ownership, three-node timing, package provenance,
  target architecture checks, workflow-pinned Mihomo/GOST payloads, and
  release-time permission restoration.
- Reused cached traffic metric handles on the Core hot path. Experimental GRO,
  dual-TUN, large-window, and Leaf packet-batching attempts that did not meet
  cross-host acceptance criteria are not included.

### Measured candidate results

- Linux nested Mihomo -> GOST -> mesh -> peer GOST TCP reached a five-run
  median of `200.471 Mbit/s` for 32 MiB transfers, versus about `55 Mbit/s` on
  the superseded userspace ingress.
- UDP echoes of `64`, `1200`, and `8192` bytes passed before and after forced
  GOST restart.
- Normal shutdown removed GOST, Mihomo, SOCKS listeners, and TUN state.

### Compatibility and known boundaries

- Existing non-policy EasyTier use remains available. Leaf is retained for
  compatibility but is deprecated for new desktop and Unix deployments.
- Mihomo is not packaged for current MIPS, MIPSel, ARM/ARMv7, or LoongArch
  targets whose mappings remain `needs_review`.
- The macOS DMG is code-signed but not notarized.
- A transient macOS `dscacheutil` zombie can be an upstream Mihomo grandchild;
  EasyTier still reaps its directly managed Mihomo process.
- The first Mihomo configuration validation may download Geo data. If the GUI
  request times out while those files are still growing, wait for the download
  to finish and save again.
- Formal Windows compilation, packaging, installation, and payload inspection
  passed. A clean end-to-end Windows policy-routing smoke was not completed
  before release because the available routing host had another active TUN.

### Release evidence

- Core: `30412988643`
- GUI: `30412990463`
- Mobile: `30412992139`
- OHOS: `30412993844`
- Test: `30412995447`
- Release: `30419459948`
- Published assets: 46
