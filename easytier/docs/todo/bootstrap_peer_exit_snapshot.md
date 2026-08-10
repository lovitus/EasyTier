# Bootstrap Peer Exit Snapshot

Status: IMPLEMENTED, VALIDATION PENDING

Date: 2026-08-10

## 1. User requirement

EasyTier currently needs a configured initial peer again after every process restart. If that
initial peer is unavailable, a node that was previously part of a healthy mesh has no local
bootstrap address with which to rejoin the remaining peers.

The requested behavior is deliberately small:

- A configured initial peer remains the first-join bootstrap mechanism.
- After at least one normal Stop Network or application Exit, save a small auxiliary snapshot of
  currently usable peer connection URLs.
- On a later start, add those URLs to the configured initial peers as best-effort bootstrap inputs.
- The snapshot may be stale. It is not authoritative configuration and does not need continuous
  maintenance.
- Do not add connection events, periodic refresh, TTL, scoring, health persistence, failure
  counters, background writers, or a new user-facing setting.

This feature does not promise recovery after a crash, forced kill, power loss, or a first run that
never completed a peer connection. A previous snapshot remains useful even if it is not current.

## 2. Current behavior and confirmed gap

The current startup path calls `Instance::add_initial_peers()` and adds only
`global_ctx.config.get_peers()` to `ManualConnectorManager`.

`ManualConnectorManager` keeps connector URLs only in memory. Dynamic P2P connections and their
successful remote URLs disappear when the instance is dropped.

The runtime `PeerConnInfo` already exposes all data needed by this narrow feature:

- `is_client`, which identifies the side that successfully dialed the URL;
- `is_closed`, which prevents saving a closed connection;
- `tunnel.remote_addr`, which preserves the dialable transport URL;
- `tunnel.resolved_remote_addr`, which can be a fallback when the original remote URL is absent.

Runtime `PeerId` is randomly generated in `PeerManager::new()` and must not be persisted. Route,
session, latency, STUN and NAT state are also unnecessary and become stale across restart.

## 3. Decision

Implement an auxiliary exit snapshot containing only URL strings.

The cache is not a peer database. It is equivalent to a small, automatically generated secondary
initial-peer list. Existing connector creation, network authentication, reconnect behavior,
transport selection and P2P logic remain authoritative.

The implementation must remain a three-step feature: snapshot on normal stop, write one small
file, and append it to the next run's in-memory connector inputs. Do not turn it into a persistent
peer-management framework.

## 4. Persisted format and location

Use the existing persistent `NetworkInstanceManager::config_dir` when it is available:

```text
<config-dir>/peer-bootstrap/<stable-network-key>.txt
```

The file key must not depend on runtime `PeerId` or a possibly generated, non-persisted CLI
instance UUID. Derive it from the existing `NetworkIdentity` equality boundary:

- Shared-secret networks use the existing digest derived from `network_name` and
  `network_secret`; never place the plaintext secret in a path or file.
- Credential networks use a digest of `network_name`, matching the current
  `NetworkIdentity::new_credential()` identity boundary.

Use a filesystem-safe hexadecimal digest as `<stable-network-key>`. Different configured networks
therefore use different files, while multiple local profiles that intentionally join the same
network may safely reuse the same auxiliary bootstrap URL list.

The file format is UTF-8 text with one complete URL per line:

```text
udp://203.0.113.10:11010
tcp://192.168.1.20:11010
```

No JSON schema, peer ID, timestamp, network secret, route or status field is needed.

If no persistent `config_dir` was supplied, disable this auxiliary feature for that manager. Do
not use a temporary directory and do not add a new CLI option solely for this cache.

Core, GUI and mobile entry points reuse the same manager helper. GUI/Tauri already supplies its
application data directory. Core uses `--config-dir` when present and otherwise skips the cache.
Android JNI initializes the manager from the application's private files directory; OHOS reuses
the root already supplied to `init_config_store()`. The generic FFI exposes the same one-time path
initializer for other embedded/mobile hosts. No platform-specific peer or networking logic is
introduced.

The directory is already application-managed. No `fsync`, backup generation, database or strict
transaction protocol is required. A malformed or partially written file is ignored line by line
on the next start.

## 5. Snapshot extraction

Add a synchronous, read-only snapshot primitive over the current peer map. It must not close
connections, await network work, mutate peer state, or scan interfaces.

For every live `PeerConnInfo`:

- Include it only when `is_client == true`.
- Exclude it when `is_closed == true`.
- Use `tunnel.remote_addr` when present.
- Otherwise use `tunnel.resolved_remote_addr` when present.
- Keep the complete URL, including transport scheme, port, path and query parameters.
- Exclude `ring://` because it is an in-process transport and cannot be dialed after restart.
- Deduplicate exact normalized URL strings and sort them for deterministic output.

Do not save inbound `remote_addr` values. For TCP and similar transports, an inbound remote port is
normally the caller's ephemeral source port and is not a peer listener.

Do not query every peer for fresh listener, STUN or interface information during stop. The
requirement is to preserve connections that actually worked, not to generate a new discovery
database while shutting down.

## 6. Stop and exit behavior

Use one shared helper before a normal instance is removed from `instance_map`:

1. Take the synchronous URL snapshot while the peer manager still exists.
2. If the snapshot contains at least one URL, write the text file.
3. If the snapshot is empty, leave any previous file unchanged.
4. If directory creation or writing fails, log one warning and continue stopping.
5. Drop the instance through the existing lifecycle without waiting for additional work.

The empty-snapshot rule is the only preservation rule required. It prevents an offline restart
followed by Stop from erasing the last useful bootstrap snapshot.

The helper belongs in the existing normal removal path used by Stop Network and application Exit.
It must run before `retain_network_instance()` drops instances. It must not be triggered by peer
events or periodic tasks.

Explicit profile deletion may leave the tiny auxiliary file behind. Cleanup can remove it when
convenient, but orphan cleanup is not a release requirement because the file is harmless and keyed
by stable network identity.

## 7. Startup behavior

Before `NetworkInstance::new()` consumes the configuration:

1. Derive the same stable network key and read its `.txt` file when `config_dir` exists.
2. Parse each non-empty line as a URL.
3. Ignore malformed lines and `ring://` lines, then continue reading the remaining entries.
4. Build a runtime-only connector list and append cached URLs after user-configured peers.
5. Deduplicate exact URLs so a configured initial peer is not added twice.
6. Pass the merged in-memory peer list through the existing `add_initial_peers()` path.

This merge is strictly runtime-only and in-memory. Never call a configuration persistence,
serialization or source-file write-back path for cached URLs. Never rewrite the user's TOML, GUI
configuration, imported file or initial-peer list. The cached URLs exist only in the temporary
connector input for that process run.

A missing, unreadable, empty or malformed cache must never block startup. A stale but valid URL is
handled by the existing `ManualConnectorManager` reconnect behavior exactly like a stale configured
initial peer.

## 8. Deliberate non-goals

- No write on every `PeerConnAdded` event.
- No timer or periodic cache refresh.
- No write on unexpected instance failure, abort or process crash.
- No route-table or topology persistence.
- No runtime Peer ID persistence.
- No dependency on a generated CLI instance UUID.
- No STUN result, NAT mapping or hole-punch session persistence.
- No peer ranking, TTL, last-success time or failure count.
- No cache management UI, RPC or configuration switch.
- No guarantee that every cached URL remains reachable.
- No change to relay, lazy P2P, QUIC, KCP, FakeTCP or transport-priority semantics.

## 9. Correctness and security boundary

The saved URL has already completed an outbound EasyTier connection in the current instance. On
reuse, the normal EasyTier handshake still validates the current network identity and credentials.
The cache therefore does not bypass authentication or authorize a peer.

The cache is keyed by the current stable `NetworkIdentity` boundary rather than runtime Peer ID or
instance UUID. Changing the network name or shared secret selects a different file automatically.
No identity metadata, migration table or cleanup policy is needed inside the file.

The design cannot recreate a changed NAT mapping. If all remaining peers are behind symmetric NAT,
all old mappings have expired, and no peer has a reachable listener, public IPv6 address or relay,
a rendezvous peer is still required. The snapshot improves restart bootstrap for previously
reachable P2P endpoints; it cannot remove the fundamental need for rendezvous in every topology.

## 10. Minimal implementation surface

Implemented production surface:

- `peers/peer_map.rs`: reuse the existing live outbound URL index as a synchronous snapshot.
- `instance/instance.rs` and `launcher.rs`: carry cached URLs separately from configuration and
  capture the live snapshot at the existing normal-stop boundary.
- `instance_manager.rs`: derive the stable key, read/write the line file, append runtime-only URLs,
  and save before normal removal or manager exit.
- Android JNI/FFI and OHOS entry points: initialize the same manager with their existing
  app-private persistent directory.

Do not change `ManualConnectorManager` reconnect logic. Do not make
`retain_network_instance()` async merely to collect the snapshot.

## 11. Automated tests

Add focused tests for the new contract:

- A live client connection contributes its full remote URL.
- Inbound, closed and missing-address connections are excluded.
- `resolved_remote_addr` is used only when `remote_addr` is absent.
- `ring://` is excluded during both snapshot generation and cache loading.
- Duplicate URLs produce one deterministic line.
- The same network identity produces the same file key across process restarts and random CLI
  instance UUIDs.
- Different shared-secret networks produce different file keys.
- Multiple configured Network instances load only the cache selected by their own network
  identity.
- A non-empty snapshot replaces the auxiliary file.
- An empty snapshot preserves the previous file.
- Missing and malformed files do not block startup.
- Cached URLs are appended after configured initial peers and deduplicated.
- A normal manager removal writes the snapshot before dropping the instance.
- The user's serialized configuration remains byte-for-byte unchanged.

No performance benchmark is required. Snapshot work is proportional to the small number of live
peer connections and occurs only during normal stop or exit, outside the packet hot path.

## 12. Functional validation

Use three nodes for exact-artifact validation:

1. Node A starts with only node B as its configured initial peer.
2. A reaches node C and establishes a real direct client connection to C.
3. Stop A normally and confirm its auxiliary file contains C's successful URL.
4. Stop or isolate B.
5. Restart A with its unchanged user configuration.
6. Confirm A attempts the configured B URL and the cached C URL.
7. Confirm A rejoins through C and rebuilds the normal mesh routes.
8. Stop A once while disconnected and confirm the previous non-empty snapshot remains available.
9. Corrupt one line in the file and confirm valid remaining lines still bootstrap normally.

Run the same lifecycle smoke on Linux, macOS and Windows. The implementation uses only existing
cross-platform URL parsing, peer snapshots and ordinary file I/O, so platform-specific networking
logic must not be introduced.

## 13. Acceptance criteria

- One successful bootstrap plus one normal Stop/Exit can provide alternative bootstrap URLs for a
  later start.
- The configured initial peer remains unchanged and retains priority.
- Cached URLs are appended only to an in-memory runtime connector list and never appear in any
  persisted user configuration.
- Cache selection is stable across CLI restarts and isolated by the existing Network identity.
- No `ring://` URL is persisted or restored.
- No event subscriber, refresh task, timer, TTL or health database is added.
- Cache absence, corruption and write failure never prevent start or stop.
- Stop does not perform network RPC, DNS, interface enumeration or hole punching.
- Packet forwarding and established-connection hot paths are untouched.
- Cached endpoints still pass the ordinary EasyTier authentication and connector pipeline.
- The limitation for expired NAT-only endpoints is documented rather than hidden.
