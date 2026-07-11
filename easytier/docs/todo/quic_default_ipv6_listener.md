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

## Minimal Design

- Add a QUIC-private bind mode: `V4Only`, `V6Only`, and `DualStack`. Do not add a
  CLI, TOML, RPC, protobuf, GUI, capability, or wire field.
- Preserve the configured `0.0.0.0:port` QUIC listener as `V4Only`.
- When `enable_ipv6=true`, an unspecified IPv4 QUIC listener gets one automatic
  `[::]:port` `V6Only` companion with `IPV6_V6ONLY=true`.
- Do not add the companion when an explicit same-port IPv6 QUIC listener already
  exists.
- Preserve explicit `[::]:port` listeners as the current `DualStack` behavior.
- Keep bind-mode state local to the QUIC listener/endpoint. A local bind failure
  must not mutate process-global endpoint-pool behavior.
- Advertise the IPv6 QUIC candidate only after the companion has bound the exact
  requested address and entered the running-listener set.

## Safety Constraints

- A nonzero requested QUIC port must bind exactly or return an error. It must
  never silently continue on port zero or with an unbound socket.
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
  required on this path. Explicit `DualStack` listeners still require existing
  mapped-address behavior to remain stable.
- Enabling IPv6 increases the reachable UDP surface; existing firewall and
  strict-Stealth policy remains authoritative and must not be relaxed.

## Implementation Scope

- `instance/listeners.rs`: create and deduplicate the automatic QUIC IPv6
  companion under `enable_ipv6`.
- QUIC listener/endpoint code: accept the private bind mode and set
  `IPV6_V6ONLY` deterministically before bind.
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
- Occupied IPv4 and IPv6 ports fail explicitly without a port-zero listener.
- Multiple EasyTier instances do not share or corrupt endpoint bind mode.
- IPv4 and IPv6 candidates are advertised only for running listeners.
- QUIC reconnect, transport-priority selection, and failover work independently
  over IPv4 and IPv6.
- Correct/wrong Stealth secret and strict-listener no-response behavior are
  unchanged on both address families.
- Linux socket mark/netns behavior and Windows/macOS address reporting remain
  correct.
- Existing IPv4-only QUIC, Proxy `QUIC -> KCP -> Native`, and mixed-version
  interoperability tests remain green.

## Acceptance Boundary

This change is considered behavior-preserving only after the matrix above
passes. The intended user-visible difference is limited to an additional
working IPv6 QUIC listener/candidate when IPv6 is enabled; IPv4 availability,
fallback behavior, security properties, and configuration semantics must remain
unchanged.
