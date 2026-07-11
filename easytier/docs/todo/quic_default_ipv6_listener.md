# QUIC Default IPv6 Listener Plan

Status: pending implementation and cross-platform validation.

## Goal

Enable IPv6 listening for the default unspecified IPv4 QUIC listener when
`enable_ipv6=true`, while preserving the existing IPv4 listener and all public
configuration, wire, Stealth, Proxy, and non-QUIC behavior.

The current IPv4-only default is a stale listener-integration limitation, not a
known Quinn restriction. The implementation must not simply remove the QUIC
exclusion in `instance/listeners.rs`, because the resulting dual-stack IPv6
socket could overlap the existing IPv4 socket.

### Verified current behavior

- The default QUIC listener is `quic://0.0.0.0:11012`; it creates an IPv4 UDP
  socket. `only_v6(false)` cannot make an IPv4 socket dual-stack. It only controls
  `IPV6_V6ONLY` when the socket itself is bound to an IPv6 address.
- `QuicEndpointManager::server()` enables `dual_stack` only when the configured
  listener address is IPv6 unspecified (`[::]`). It does not convert an
  unspecified IPv4 listener into an IPv6 listener.
- A displayed `quic6` tunnel does not prove that the local node has an IPv6 QUIC
  listener. It means that connection's effective endpoint is IPv6 and can be
  produced by this node actively connecting to a remote IPv6 QUIC listener.
- When auditing sockets, use the QUIC listener's actual port. In the default
  all-protocol layout QUIC uses UDP `11012`; UDP `11010` and WG UDP `11011` are
  separate listeners and must not be mistaken for QUIC.

## Minimal Design

- Add a QUIC-private bind mode: `V4Only`, `V6Only`, and `DualStack`. Store it on
  `QuicTunnelListener` and pass it explicitly to `QuicEndpointManager::server()`.
  `server()` must not infer listener bind mode from `[::]` or the global
  `both.is_enabled()` client-pool state. Do not add a CLI, TOML, RPC, protobuf,
  GUI, capability, or wire field.
- Preserve the configured `0.0.0.0:port` QUIC listener as `V4Only`.
- When `enable_ipv6=true` and the configured port is nonzero, an unspecified
  IPv4 QUIC listener gets one optional `[::]:port` `V6Only` companion with
  `IPV6_V6ONLY=true`.
- Do a read-only pre-scan of QUIC URIs only. Build indexes for unspecified IPv4,
  unspecified IPv6, and explicit IPv6 QUIC ports. Keep the original listener
  iteration order and `must_succ` behavior unchanged; use the indexes only to
  choose bind mode and decide whether a companion is needed.
- The pre-scan recognizes only IP-literal listener hosts. Hostnames do not
  participate in same-port deduplication or companion decisions based on DNS
  resolution, because changing DNS answers must not change listener topology.
- An explicit `[::]:port` remains `DualStack` when it is the only QUIC listener
  for that port. If an explicit or generated IPv4 QUIC listener exists on the
  same nonzero port, resolve `[::]:port` to `V6Only` so the sockets cannot
  overlap. Port zero is not a shared-port identity and is excluded from this
  pairing rule.
- Do not add an automatic companion when a same-port explicit IPv6 QUIC listener
  already supplies the IPv6 side.
- Keep bind-mode state local to the QUIC listener/endpoint. A local bind failure
  must not mutate process-global endpoint-pool behavior.
- Advertise the IPv6 QUIC candidate only after the companion has bound the exact
  requested address and entered the running-listener set.
- Do not create an automatic companion for `quic://0.0.0.0:0`. Two independent
  port-zero binds do not guarantee the same random port. Keep the IPv4 listener
  and emit one precise warning that automatic IPv6 requires a nonzero port.
  Explicit `quic://[::]:0` remains valid with its existing random-port meaning.
- Keep FakeTCP excluded from automatic IPv6 companion creation. FakeTCP uses a
  different socket/transport implementation and is outside this QUIC-only task.

## Endpoint Pool Contract

- Listener bind mode and endpoint-pool enable state are separate concepts.
  Listener creation must not use `both.is_enabled()` to derive `V4Only`,
  `V6Only`, or `DualStack`.
- Every server listener, including an explicit `DualStack` listener, bypasses
  `QuicEndpointManager::create()` and its process-global dual-stack fallback.
  Server creation calls the endpoint constructor directly with the listener's
  explicit bind mode.
- A server `DualStack` bind failure returns a listener bind error. It must not
  silently retry as `V6Only`, call `both.disable()`, or enable the IPv4/IPv6
  client pools. This intentionally replaces the previous ambiguous behavior in
  which an explicit `[::]` listener could appear to start with reduced coverage.
- Non-Stealth `V4Only` and `V6Only` server endpoints enter their corresponding
  persistent server pools. Non-Stealth `DualStack` enters `both`; Stealth server
  endpoints retain their existing non-pooled ownership.
- Do not disable the global `both` pool when adding companions. It remains
  available for port-zero client endpoints and explicit `DualStack` listeners.
- A `V6Only` companion failure must not call `both.disable()`, enable/disable any
  other pool, or change another instance. The existing dual-stack client
  fallback remains only in client endpoint creation and must not be triggered by
  any server listener. Optional companion failure is reported and contained by
  the listener manager without changing pool state.
- Do not change pool element types, listener removal semantics, or client
  ephemeral cleanup in this task. Address-based endpoint removal is tracked as a
  separate existing-risk investigation.

## Safety Constraints

- QUIC performs strict post-bind verification because the generic socket helper
  currently suppresses IPv6 bind failures. Endpoint creation alone is not proof
  of a successful bind.
- A nonzero requested port must equal the actual port. A requested port of zero
  must result in a nonzero kernel-assigned port.
- A requested explicit IP must equal the actual bound IP. For an unspecified
  address, the actual address family must match the requested bind mode.
- Verification failure closes the endpoint and returns an error before it is
  inserted into any pool or advertised. It must never become a silent port-zero
  or wrong-family listener.
- Preserve the current Stealth endpoint ownership rule: Stealth server endpoints
  are not inserted into the shared endpoint pool. Strict validation applies
  before publication but must not change that ownership behavior.
- Keep strict bind verification local to QUIC unless the generic socket bind
  contract is reviewed separately.
- Failure to create the automatic IPv6 companion must not remove or alter the
  existing IPv4 listener. It must produce one precise warning/error according to
  the listener manager's optional-companion policy.
- Do not use an intentional IPv4/IPv6 bind collision as feature detection or
  fallback.
- Do not change QUIC packet format, connection IDs, TLS/Noise/Stealth behavior,
  transport priority, Proxy failover, listener ports, or listener startup for
  TCP, UDP, FakeTCP, WG, WS, or WSS.
- The default companion is IPv6-only, so IPv4-mapped IPv6 normalization is not
  required on this path. A standalone explicit `DualStack` listener must retain
  existing mapped-address behavior.
- Enabling IPv6 increases the reachable UDP surface; existing firewall and
  strict-Stealth policy remains authoritative and must not be relaxed.

## Implementation Scope

- `instance/listeners.rs`: pre-scan QUIC URIs into read-only port indexes, then
  keep the existing listener creation loop and append optional companions where
  eligible.
- QUIC listener/endpoint code: store and explicitly pass the private bind mode,
  and set `IPV6_V6ONLY` deterministically before bind. Companions must continue
  through the normal `server()` path so server config and Stealth initialization
  are not duplicated.
- QUIC-local socket setup: return authoritative bind errors and verify the
  requested local address/port.
- Listener advertisement: publish only successfully running IPv4/IPv6
  endpoints.

No PeerManager, routing, Proxy, SOCKS, Stealth record layer, or wire-level
refactor is part of this task.

## Test Coverage

- Linux, macOS, and Windows bind IPv4 and IPv6 QUIC listeners on the same port.
- IPv4-only host keeps working when the IPv6 companion cannot be created.
- `enable_ipv6=false` remains exactly IPv4-only.
- Explicit IPv4, explicit IPv6, explicit `[::]` dual-stack, and duplicate
  same-port IPv6 configurations retain their intended behavior.
- Explicit same-port `0.0.0.0` plus `[::]` resolves to `V4Only + V6Only`, while a
  standalone `[::]` remains `DualStack`.
- Occupied IPv4 and IPv6 ports fail explicitly without a port-zero listener.
- `quic://0.0.0.0:0` remains IPv4-only, creates no companion, and emits one
  warning; explicit `quic://[::]:0` remains independently valid.
- Companion failure does not disable `both` or alter client endpoint selection.
- Explicit server `DualStack` bind failure is reported and is never silently
  downgraded to `V6Only`; client dual-stack fallback remains unchanged.
- Hostname listeners preserve their current creation behavior and do not affect
  IP-literal companion deduplication through DNS resolution.
- IPv4 and IPv6 candidates are advertised only for running listeners.
- QUIC reconnect, transport-priority selection, and failover work independently
  over IPv4 and IPv6.
- Correct/wrong Stealth secret and strict-listener no-response behavior are
  unchanged on both address families.
- Linux socket mark/netns behavior and Windows/macOS address reporting remain
  correct.
- Existing IPv4-only QUIC, Proxy `QUIC -> KCP -> Native`, and mixed-version
  interoperability tests remain green.
- FakeTCP listener behavior and its existing IPv6 exclusion remain unchanged.
- Original listener order and required/optional startup semantics remain
  unchanged.

## Acceptance Boundary

This change is considered behavior-preserving only after the matrix above
passes. The intended user-visible difference is limited to an additional
working IPv6 QUIC listener/candidate when IPv6 is enabled; IPv4 availability,
fallback behavior, security properties, and configuration semantics must remain
unchanged.
