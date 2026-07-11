# QUIC Endpoint Exact Removal Investigation

Status: separate existing-risk investigation; not part of default IPv6 listener
delivery.

## Problem

`QuicEndpointManager` is process-global. Listener drop currently removes entries
from all endpoint pools by `local_addr`, so one listener can potentially remove
another endpoint that has the same bound address.

This risk predates the default IPv6 companion plan. Fixing it would change pool
element types and lifecycle behavior, so it must not be bundled into the
listener bind-mode change without a reproducible case.

## Investigation

- Determine whether the supported platforms permit two same-process QUIC UDP
  endpoints to bind the same address and port with the current socket options.
- Cover persistent server endpoints, ephemeral client endpoints, endpoint
  clones, port-zero reuse, and multiple EasyTier instances.
- Verify whether dropping one listener can remove or disrupt an endpoint owned
  by another listener or instance.

## Possible Fix

If reproducible, give each persistent server endpoint a manager-issued private
registration ID and remove only that registration. Do not rely on `local_addr`
as identity or assume Quinn exposes a stable public endpoint identity API.

## Acceptance

- Dropping one listener removes only its registered endpoint.
- Other server and client endpoints remain usable.
- Pool capacity, resize, dual-stack fallback, Stealth endpoint ownership, and
  client selection behavior remain unchanged.
