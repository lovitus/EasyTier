# Bootstrap Peer Exit Snapshot

Status: PERSISTENCE AND RUNTIME HELPER PAUSE IMPLEMENTED; MANAGER-WIDE COOLDOWN REJECTED; CANDIDATE VALIDATION PENDING

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

Use a manager-owned bootstrap state directory. An explicit
`NetworkInstanceManager::config_dir` remains the preferred location when it is available:

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

If no persistent `config_dir` was supplied, a host entry point that owns a stable application-data
directory must supply that directory for bootstrap state. Do not silently assign this fallback to
the existing `config_dir`: that field also controls configuration files, Policy/Geo resources and
Mihomo runtime data, so changing it would alter unrelated Core behavior. Keep the bootstrap state
directory as a narrow manager input that defaults to an explicit `config_dir` when present.

If neither an explicit config directory nor a stable host-owned application-data directory can be
resolved, disable this auxiliary feature with one warning. Never fall back to the current working
directory or a temporary directory, and do not add a new CLI option solely for this cache.

Core, GUI and mobile entry points reuse the same manager helper. GUI/Tauri supplies its
application data directory. Core uses `--config-dir` when present and otherwise resolves the
platform application-data directory at the Core host boundary. A Windows service must resolve a
stable service-owned directory rather than an interactive user's transient profile. Android JNI
initializes the manager from the application's private files directory; OHOS reuses the root
already supplied to `init_config_store()`. The generic FFI exposes the same one-time path
initializer for other embedded/mobile hosts. No platform-specific peer or networking logic is
introduced.

The directory is already application-managed. No `fsync`, rotating backup generations, database
or strict transaction protocol is required. A malformed or partially written file is ignored line
by line on the next start or causes fallback to the fixed backup when no valid primary URL remains.

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
- Exclude every URL whose `TunnelScheme` is unavailable in the current feature/platform build.
  A URL written by one artifact must not become a permanent failing connector after upgrade,
  downgrade or migration to an artifact with a narrower transport set.
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

The baseline empty-snapshot rule prevents an offline restart followed by Stop from erasing the
last useful bootstrap snapshot. The follow-up hardening keeps the same narrow lifecycle boundary
and adds three write-elision rules before touching the file:

- Normalize, sort and deduplicate the new and previously persisted URL sets.
- Do not write when the new snapshot is empty.
- Do not write when the new snapshot is identical to the persisted snapshot.
- Do not write when the new snapshot is a strict subset of the persisted snapshot. This covers a
  rapid Start/Stop cycle in which only part of the previously reachable mesh had time to reconnect.

A non-empty snapshot that contains a new URL, or otherwise is not a strict subset of the old set,
remains eligible to replace the primary snapshot. This permits topology and endpoint changes to
be learned without accumulating every stale URL forever.

### 6.1 Fixed single backup

Keep exactly two possible files for each stable network key:

```text
<stable-network-key>.txt
<stable-network-key>.txt.bak
```

Only an actual primary-file update creates or replaces the fixed backup:

1. If the primary file exists and contains at least one valid URL, copy its current contents to
   the fixed `.bak` path, replacing the previous backup.
2. If that backup operation fails, leave the primary file untouched, report one warning and skip
   the update.
3. After the backup succeeds, write the new non-empty primary snapshot.
4. If writing the primary fails after truncation or partial output, the completed backup remains
   available for the next start.

Do not rotate backups, retain generations, add timestamps, call `fsync`, or introduce a database.
Empty, equal and strict-subset snapshots perform no backup and no primary-file write. This keeps
rapid lifecycle operations from repeatedly touching the filesystem.

The helper belongs in the existing normal removal path used by Stop Network and application Exit.
It must run before `retain_network_instance()` drops instances. It must not be triggered by peer
events or periodic tasks.

Expose one explicit manager shutdown helper that removes every active instance through this same
snapshot-before-drop path. All normal host shutdown paths call it; `Drop` remains only a defensive
fallback:

- GUI application Exit must call the helper before Tauri terminates. The GUI manager is stored in
  static process state, whose destructor is not a reliable normal-exit hook.
- Windows Service `Stop` must signal `run_main` to execute the helper and finish cleanup before the
  service thread calls `process::exit`. A direct `process::exit` from the service-control branch
  skips Rust destructors and cannot be accepted as snapshot evidence.
- Unix SIGINT/SIGTERM and ordinary Core return paths use the same helper so behavior is explicit
  and testable rather than depending on the final `Arc` drop order.

The helper remains synchronous at the manager boundary because the existing instance removal
already joins the launcher before reading its final live URL snapshot. Repeated shutdown calls are
idempotent: the first call removes the instance, and later calls find nothing to persist.

Explicit profile deletion may leave the tiny auxiliary file behind. Cleanup can remove it when
convenient, but orphan cleanup is not a release requirement because the file is harmless and keyed
by stable network identity.

## 7. Startup behavior

Before `NetworkInstance::new()` consumes the configuration:

1. Derive the same stable network key and read its `.txt` file when `config_dir` exists.
2. Parse each non-empty line as a URL.
3. Ignore malformed lines, `ring://` lines and schemes unavailable in the current build, then
   continue reading the remaining entries.
4. If the primary file yields at least one valid URL, use only the primary snapshot.
5. Only when the primary file is missing, unreadable, empty or yields no valid URL, try the fixed
   `.bak` file.
6. Never merge the primary and backup snapshots. Every loaded URL becomes an independently
   maintained `ManualConnectorManager` connector; combining two generations would preserve stale
   endpoints and multiply permanent reconnect work.
7. Keep configured URLs and cached URLs as separate runtime sources. Register configured URLs with
   the established manual-connector semantics and register persisted-only URLs as runtime bootstrap
   helpers.
8. Deduplicate exact URLs with configured precedence so a configured initial peer is never demoted
   to helper behavior.
9. Once the primary peer map has any registered, non-closed connection, stop scheduling new helper
   reconnect tasks. Resume helpers on the existing manager tick when the primary map becomes empty.

This merge is strictly runtime-only and in-memory. Never call a configuration persistence,
serialization or source-file write-back path for cached URLs. Never rewrite the user's TOML, GUI
configuration, imported file or initial-peer list. The cached URLs exist only in the temporary
connector input for that process run.

A missing, unreadable, empty or malformed cache must never block startup. Cached helpers reuse the
existing connector implementation and scheduler; their only distinct behavior is the scheduling
pause after primary-mesh recovery. No timer, network event subscription or persistent retry state is
added. An attempt already in flight may complete once.

## 7.1 Rejected attempt: manager-wide initial connector cooldown

Status: REJECTED for v3.0.16-3.

The design below was implemented experimentally, but formal Test evidence showed that one shared
cooldown changed recovery semantics for every manual connector rather than only reducing noise
from cold bootstrap URLs. It was removed from the release candidate. This section is retained as
failed-attempt evidence, not as an implementation requirement.

The baseline `ManualConnectorManager` scans once per second and maintains every configured URL as
an independent persistent connector. Connecting through one initial URL does not stop retries for
the other unreachable URLs. A large automatically restored list can therefore produce a linear
DNS, socket, timeout, event and log retry fan-out even after the mesh has already bootstrapped.

Replace the fixed one-second retry loop with one manager-wide batch schedule. Do not create a
per-URL timer or reuse the endpoint-keyed P2P cooldown table.

Required batch semantics:

- Attempt the initial batch immediately at startup.
- Attempt all currently failed URLs concurrently once per batch.
- After a batch completes, remove successful URLs from the failed set and place all remaining
  failures into one next retry period.
- Use the delay sequence `1, 10, 30, 60, 120, 360, 720` seconds. Stay at 720 seconds after the last
  stage.
- Apply the existing direct-connector jitter policy so different EasyTier nodes do not synchronize
  their retry bursts.
- A partial success does not reset the schedule for the remaining failed URLs.
- Reset the schedule only when no failed URL remains, a connector is added or changed, or an
  existing network-change signal requires immediate recovery.
- Connector additions and network-change resets wake a sleeping batch immediately instead of
  waiting for the current cooldown deadline.
- Shutdown cancels the batch and in-flight retry tasks through the existing manager lifecycle.

Reuse rather than duplicate the established primitives:

- Move the existing `connector::udp_hole_punch::BackOff` sequence cursor to a neutral
  `common` retry module and re-export it from the old path so UDP/TCP hole-punch callers keep their
  existing API and behavior.
- Add only the neutral `reset` and jittered-delay capabilities needed by batch consumers.
- Reuse the jitter calculation already used by the direct connector.
- Reuse the existing `GlobalCtx` event subscription and the same DHCP IPv4, IPv4 conflict, public
  IPv6 and configuration-change signals already used to invalidate underlay state. Do not add
  platform-specific interface monitors for this feature.

`P2pEndpointRetryTable` is deliberately not reused. It is keyed by peer ID, transport scheme and
resolved socket address, uses a different 60-to-600-second policy and cannot represent unresolved
domain, TXT, SRV or WebSocket initial URLs. Applying it here would preserve per-endpoint retry state
instead of the required single batch state.

## 8. Deliberate non-goals

- No write on every `PeerConnAdded` event.
- No timer or periodic cache refresh.
- No write on unexpected instance failure, abort or process crash.
- No reliance on static/global destructor execution as a normal GUI or service shutdown path.
- No route-table or topology persistence.
- No runtime Peer ID persistence.
- No dependency on a generated CLI instance UUID.
- No STUN result, NAT mapping or hole-punch session persistence.
- No peer ranking, TTL, last-success time or failure count.
- No unbounded append-only URL history and no merge of primary and backup generations.
- No independent retry timer, backoff state or cooldown table per initial URL.
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

Implemented production surface for the accepted persistence scope:

- `peers/peer_map.rs`: reuse the existing live outbound URL index as a synchronous snapshot.
- `instance/instance.rs` and `launcher.rs`: carry cached URLs separately from configuration and
  capture the live snapshot at the existing normal-stop boundary.
- `instance_manager.rs`: derive the stable key, read/write the line file, append runtime-only URLs,
  maintain one fixed backup, and save before normal removal or manager exit.
- `connector/manual.rs`: retain the established configured/RPC connector behavior, mark only
  persisted-only URLs as runtime helpers, and suppress new helper claims while the primary peer map
  is live. Add/Remove/Clear and reconnect completion are linearized per instance.
- `core.rs` and GUI/Tauri shutdown handling: invoke the shared manager shutdown path explicitly;
  Windows Service Stop joins the same Core shutdown path before reporting the service stopped.
- Android JNI/FFI and OHOS entry points: initialize the same manager with their existing
  app-private persistent directory.

The manual connector change is limited to helper classification and retry scheduling. It does not
alter connector creation, DNS, transport selection, handshake, authentication or P2P behavior. Do not make
`retain_network_instance()` async merely to collect the snapshot.

## 11. Automated tests

Add focused tests for the new contract:

- A live client connection contributes its full remote URL.
- Inbound, closed and missing-address connections are excluded.
- `resolved_remote_addr` is used only when `remote_addr` is absent.
- `ring://` is excluded during both snapshot generation and cache loading.
- A syntactically valid URL using a scheme unavailable in the current build is excluded during
  loading and never registered as a connector.
- Duplicate URLs produce one deterministic line.
- The same network identity produces the same file key across process restarts and random CLI
  instance UUIDs.
- Different shared-secret networks produce different file keys.
- Multiple configured Network instances load only the cache selected by their own network
  identity.
- A non-empty snapshot replaces the auxiliary file.
- An empty snapshot preserves the previous file.
- An identical snapshot performs no primary or backup write.
- A strict-subset snapshot preserves the previous primary and backup files.
- A changed eligible snapshot copies the previous primary to the fixed backup before replacing it.
- A backup failure leaves the primary unchanged and does not prevent normal shutdown.
- A failed or partial primary write leaves a previously completed backup available.
- Missing and malformed files do not block startup.
- A valid primary snapshot is used without loading or merging its backup.
- A missing, unreadable, empty or wholly malformed primary falls back to the backup.
- Each stable network key creates at most one primary and one fixed backup file.
- Cached URLs are appended after configured initial peers and deduplicated.
- Configured URLs take precedence over identical cached helpers and keep their original retry
  semantics.
- A live primary peer suppresses only new cached-helper reconnect attempts; configured and RPC
  connectors continue unchanged.
- Removing the last primary peer resumes cached helpers without a synthetic network event.
- Add, Remove and Clear cannot race an in-flight reconnect into resurrecting a removed connector or
  changing a newer source classification.
- A normal manager removal writes the snapshot before dropping the instance.
- GUI Exit, Windows Service Stop and normal Core signal shutdown explicitly invoke the shared
  manager shutdown helper exactly once.
- Repeated explicit shutdown followed by manager `Drop` does not rewrite either snapshot file.
- Default Core operation without `--config-dir` resolves an isolated bootstrap state directory
  without changing the manager's unrelated configuration/resource directory semantics.
- The user's serialized configuration remains byte-for-byte unchanged.

No performance benchmark is required. Snapshot work is proportional to the small number of live
peer connections and occurs only during normal stop or exit, outside the packet hot path.

The following tests belonged to the rejected manager-wide cooldown and are deferred with its
replacement design:

- Startup attempts one immediate batch.
- Complete failures advance through `1, 10, 30, 60, 120, 360, 720, 720` seconds.
- Every failed URL is attempted at most once in each batch.
- Partial success removes only successful URLs and does not reset the failed batch.
- Complete success resets the next failure to the first retry stage.
- Connector mutation and each supported network-change event wake cooldown immediately and reset
  the schedule exactly once.
- Jitter remains within the shared policy's bounds and does not synchronize deterministic test
  instances.
- Cancellation leaves no retry task, timer or event subscriber behind.

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
- Default Core CLI and Windows Service operation have a stable host-owned bootstrap state
  directory even when `--config-dir` is omitted.
- No `ring://` URL is persisted or restored.
- URLs unsupported by the current feature/platform build are neither restored nor retried.
- No event subscriber, refresh task, timer, TTL or health database is added.
- Cache absence, corruption and write failure never prevent start or stop.
- GUI Exit and Windows Service Stop save explicitly before process termination; static manager
  destruction is not part of the normal correctness proof.
- Empty, identical and strict-subset snapshots never replace a useful primary file.
- Exactly one fixed backup protects the previous primary contents during an eligible update.
- A valid primary never causes its backup generation to be registered as additional connectors.
- Configured manual connectors retain the established retry and eventless recovery behavior;
  persisted-only helpers pause after primary recovery and resume if the primary map becomes empty.
- Stop does not perform network RPC, DNS, interface enumeration or hole punching.
- Packet forwarding and established-connection hot paths are untouched.
- Cached endpoints still pass the ordinary EasyTier authentication and connector pipeline.
- The limitation for expired NAT-only endpoints is documented rather than hidden.

## 14. Implementation and preflight evidence

Rejected-candidate evidence collected on 2026-08-11:

- Local command-environment, Rust formatting, shell/JSON/workflow syntax and pre-commit checks:
  PASS.
- `192.168.2.160` `cargo test --locked --no-run --package easytier --lib`: PASS without warnings.
- The early cooldown tree passed focused unit/preflight checks, but those checks did not preserve
  all existing integration semantics.
- Formal Test run `31482539039` failed all four positive P2P-only variants plus TCP/WG eventless
  disconnect recovery. Commit `078ed0a9` then injected a synthetic DHCP event into the existing
  disconnect test, changing its validation semantics; formal Test run `31495812249` still failed
  all four positive P2P-only variants.
- Root-cause review also found that runtime ring-client liveness had incorrectly reused the
  persistence-only `ring://` filter. The replacement tree separates those responsibilities.
- The manager-wide cooldown and synthetic event are therefore removed. All earlier PASS evidence
  is diagnostic only and does not apply to the replacement candidate.
- A Linux-hosted `i686-pc-windows-msvc` check was attempted but is `BLOCKED` as platform evidence:
  the builder has the Rust target but no MSVC C toolchain, so `ring` was handed to Linux `cc` and
  failed before checking EasyTier's Windows code. This is neither a product PASS nor a product
  FAIL.

Still required before release acceptance:

- build immutable Linux and Android candidate artifacts from one committed SHA;
- run the three-node cached-bootstrap and disconnected-stop lifecycle against the exact Linux
  artifact;
- install and validate the exact Android candidate on the physical device;
- pass the five exact-SHA formal workflows before Release. Windows and macOS remain formal
  compile/package gates rather than manual runtime scope for this release.

Accepted replacement-tree pre-build evidence on 2026-08-11:

- `connector/manual.rs`, `connector/udp_hole_punch/mod.rs`, and `tests/three_node.rs` match released
  `v3.0.16-2` byte-for-byte. No synthetic network-change event, enlarged deadline, or weakened
  P2P assertion remains.
- The first complete preflight exposed only stale focused-test names after the helper split; it did
  not expose a product failure. The filter-only correction also added the existing backup and
  persistence-filter tests to the maintained suite.
- The corrected final tree passed `scripts/release-operator.sh prepare`, including locked Rust
  no-run, all maintained Leaf/HEV focused tests, quinn-udp 8/8, frontend Vitest 122/122, and all
  required frontend, VPN-plugin, and GUI production builds.
- repeat the normal-stop, empty-snapshot preservation and cached restart smoke on Android.
