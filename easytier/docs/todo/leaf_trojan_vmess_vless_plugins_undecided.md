# ACTIVE: Leaf Trojan, VMess, VLESS protocol plugins

Status: user approved implementation on 2026-07-18. The historical filename is retained to avoid breaking existing links; this document is no longer undecided.

## Goal

Reuse the existing Shadowsocks actor compiler boundary to add Trojan, VMess, and VLESS without changing EasyTier mesh routing, HEV, chain, fallback, DNS, or policy first-match semantics. Each protocol must work as a direct native actor and as the second or later member of an existing EasyTier chain whose first member is a mesh SOCKS actor.

## Reference semantics and locked dependency

- Locked Leaf source: `lovitus/leaf@742ad65c441f9d60279916b82628b810efbd48fb`.
- Mihomo source inspected at `0a87b94845ef908c15f8495871e4cd8e33116328`.
- Proxy parsing reference: `adapter/parser.go::ParseProxy`.
- Trojan reference: `adapter/outbound/trojan.go::{TrojanOption,NewTrojan,StreamConnContext,ListenPacketContext}` and `test/trojan_test.go`.
- VMess reference: `adapter/outbound/vmess.go::{VmessOption,NewVmess,StreamConnContext,ListenPacketContext}`, `transport/vmess/vmess.go`, and `test/vmess_test.go`.
- VLESS reference: `adapter/outbound/vless.go::{VlessOption,NewVless,StreamConnContext,ListenPacketContext}` and `test/vless_test.go`.
- Intentional difference: Mihomo composes transport/TLS in a dialer. Pinned Leaf composes explicit `tls`, `ws`, and protocol actors. EasyTier hides those actors behind one stable public chain tag so existing user chain/fallback members remain unchanged.

## Frozen v1 schema

```yaml
proxies:
  trojan:
    type: trojan
    server: edge.example.com
    port: 443
    password: change-me
    tls: { server-name: cdn.example.com, insecure: false }
    udp: true

  vmess-ws:
    type: vmess
    server: edge.example.com
    port: 80
    uuid: 00000000-0000-0000-0000-000000000000
    alter-id: 0
    cipher: auto
    transport:
      type: websocket
      path: /vmess
      headers: { Host: cdn.example.com }
    udp: true

  vless-wss:
    type: vless
    server: edge.example.com
    port: 443
    uuid: 00000000-0000-0000-0000-000000000000
    transport:
      type: websocket
      path: /vless
      headers: { Host: cdn.example.com }
    tls: { server-name: cdn.example.com, insecure: false }
    udp: true

groups:
  vless-through-mesh:
    type: chain
    members: [mesh-exit, vless-wss]
```

`via: mesh` remains the built-in mesh SOCKS actor only. Trojan, VMess, VLESS, and Shadowsocks remain `via: native`; routing one through mesh is ordinary chain composition. This preserves the existing mesh owner and avoids protocol-specific mesh branches.

## Supported boundary

- Trojan password authentication with mandatory TLS, optional WebSocket, TCP and Leaf reliable UDP-over-stream.
- VMess AEAD with `alter-id: 0`; `auto`, AES-128-GCM, and ChaCha20-Poly1305 security; optional TLS and WebSocket.
- VLESS UUID authentication with optional TLS and WebSocket; TCP and Leaf reliable UDP-over-stream.
- Arbitrary bounded WebSocket headers are preserved by YAML; the visual editor exposes Host and preserves other keys.
- Nested public protocol chains can be members of existing EasyTier chain/fallback groups.

## Explicitly unsupported

- VMess legacy alter-id, VLESS flow/XTLS/XUDP/XHTTP, Reality, fingerprints/uTLS, early-data, smux, and Brutal are rejected or absent rather than silently accepted.
- HTTP outbound remains unavailable in pinned Leaf.
- Shadowsocks 2022 is not implemented by the locked Leaf data path. Its source uses legacy AEAD EVP_BytesToKey plus HKDF-SHA1 and lacks 2022 session-key/replay semantics. Do not claim compatibility with `2022-blake3-*` methods.

## Acceptance matrix

- `.160`: one `--locked` no-run build plus focused config/compiler/frontend tests.
- Exact GitHub candidate: one Linux/Android workflow set after the complete batch passes `.160`.
- Direct native functional checks for Trojan, VMess WS, and VLESS WSS.
- Mesh-prefixed chain functional checks for all three protocols.
- TCP and UDP behavior, DNS domain preservation, fallback, stop/start, failure cleanup, and resource baseline.
- `lv1g2` and `lv1g3`: direct and mesh-prefixed throughput comparisons against sing-box under matched protocol, address family, stream count, and duration.
- Final report must separate implemented evidence, unsupported fields, node/environment failures, and measured performance. Secrets remain only in temporary validation files.
