# macOS Interface Enumeration CPU Remediation

Status: `FUNCTIONAL_FIXES_AND_CONTRACT_TESTS_COMPLETE_AWAITING_EXACT_ARTIFACT_VALIDATION`
Implementation: `REMOTE_LOCKED_NO_RUN_AND_FOCUSED_TESTS_PASSED`
Release candidate observed: `86708133c5758902aad59fcccaeedc2cb4c548c1`
Date: `2026-07-28`

## User requirement

Eliminate periodic and burst CPU usage caused by interface enumeration on macOS
without weakening underlay loop prevention, bind-device correctness, network
change recovery, IPv4/IPv6 connectivity, hole punching, or multi-instance
isolation.

The implementation must remain useful on other supported desktop and Unix
platforms, but platform-specific behavior must not be forced onto Android,
OHOS, iOS Network Extension, or Linux network namespaces.

## Confirmed evidence

The installed macOS ARM64 candidate was observed after more than 35 minutes of
runtime:

- the user GUI process was at approximately `0%` CPU;
- WebKit was at approximately `0.1%` CPU;
- Mihomo was normally between approximately `1%` and `2.5%` CPU;
- the privileged EasyTier Core process remained bursty;
- fifty one-second Core samples had an average of `9.7%`, a maximum of
  `41.2%`, and five samples at or above `20%`;
- shorter Activity Monitor observations reached approximately `150%`;
- a ten-second Core sample accumulated `1,355` samples in `__sysctl`, with
  repeated `if_nametoindex -> getifaddrs -> sysctl` stacks;
- the sample contained multiple EasyTier call paths into `if_nametoindex`,
  rather than only the existing Darwin interface-index cache;
- the host had 39 interfaces;
- a bounded native benchmark measured 1,000 `if_nametoindex("en0")` calls in
  `60.135 ms`, or approximately `60.135 us` per call;
- the sample did not reproduce the old Quinn
  `recvmsg(EAGAIN)` busy-loop signature.

Three defunct validator/sidecar descendants were also found under Core. They do
not consume CPU, but they are a separate lifecycle defect that must be closed
without coupling it to the interface-cache implementation.

## Implementation evidence

The local implementation batch now contains:

- one `IPCollector`-owned, namespace-scoped underlay snapshot cache;
- a five-second on-demand TTL, event invalidation, generation-specific stale
  invalidation, asynchronous singleflight, and bounded publish retries;
- one raw interface collection deriving the existing filtered IPv4 and
  unfiltered IPv6 source sets;
- first-match address-to-interface identity mapping;
- preserved route-derived `local_ipv4()` and `local_ipv6()` fallback probes
  inside the collector's `NetNS`;
- one immutable snapshot reused by underlay validation, source-interface
  classification, connector bind selection, and Darwin interface-index
  selection;
- preflight UDP source binding that uses the same snapshot identity for
  explicit addresses, disables meaningless interface auto-resolution for
  wildcard addresses, and fails closed for missing identities or zero
  interface indexes;
- a compatibility-preserving `set_resolved_bind_addrs()` extension while
  retaining the existing `set_bind_addrs()` API;
- preservation of the last TCP, UDP, WebSocket, and WireGuard bind error when
  no socket can be created, with explicit private `LocalBindError` origin
  typing;
- generation invalidation restricted to explicitly typed stale local bind
  failures; ordinary connect errors cannot invalidate the snapshot;
- family-scoped unmapped fallback handling, so an IPv6 fallback cannot block
  a valid IPv4 attempt and vice versa;
- lazy invalidation of the legacy Darwin index cache instead of event-time
  eager `if_nametoindex()` refresh.

The standalone Rust probe is
`tools/interface_snapshot_cache_probe.rs`. On `192.168.2.160`, with 39
simulated interfaces and a 250 microsecond collection cost:

- baseline, 32 threads: 2,048 operations, 2,048 collector calls, maximum 32
  concurrent collectors, approximately 90,712 operations/second;
- fresh cache hit, 32 threads: 320,000 operations, zero collector calls,
  maximum one collector, approximately 3,408,470 operations/second;
- invalidated burst, 32 threads: 2,048 operations, one collector call,
  approximately 1,313,337 operations/second;
- invalidated burst, 128 threads: 4,096 operations, one collector call,
  approximately 619,553 operations/second.

The complete Rust snapshot passed the mandatory remote
`cargo test --locked --no-run --package easytier --lib` gate. Focused results:

- underlay guard and bounded preflight recovery: 11 passed;
- GlobalCtx event, breaker, and lifecycle regressions: 24 passed;
- underlay snapshot cache contracts: 4 passed;
- snapshot/connector contracts: 3 passed;
- resolved bind/error-origin contracts: 2 passed;
- existing connector regressions: 14 passed;
- existing interface-index cache regressions: 2 passed;
- selected TCP, UDP, WebSocket, and WireGuard bind regressions: 8 passed;
- total focused result: 68 passed, 0 failed.

These results prove the platform-independent cache, generation, source
selection, typed-error, retry, and existing transport contracts. They do not
yet prove macOS exact-artifact CPU/syscall reduction, real Wi-Fi/default-route
change recovery, or lifecycle/resource baselines.

These results do not yet authorize an exact macOS ARM64 candidate because the
new cache and generation contracts still need direct automated coverage. They
are also not evidence that the installed Core CPU target, real network-change
recovery, macOS syscall reduction, or cross-platform release matrix has
passed. No workflow was triggered by this implementation step.

The existing focused tests protect the old underlay filters, connector factory,
basic custom bind success/failure, and Darwin index-cache primitive. They do
not directly prove the new snapshot derivation, singleflight/epoch, resolved
bind, or stale-generation contracts. The following minimal automated contract
suite is therefore required before producing the exact macOS artifact:

The second implementation review found two real defects that must be corrected
before these tests or any artifact workflow:

- final transport `IOError` is not proof that local socket bind failed;
  generation invalidation must be driven only by an explicitly typed local
  bind failure;
- an unmapped IPv4 fallback must not block IPv6 and an unmapped IPv6 fallback
  must not block IPv4; fallback failure is scoped to the current remote/bind
  address family.

The stale classifier must not treat generic `ErrorKind::NotFound` or Unix
`ENOENT` as an interface transition. Retain only explicitly typed local-bind
errors whose underlying errno is a supported stale-interface signal.

1. `snapshot_derivation_preserves_sources_and_first_match`
   - combine IPv4, IPv6, route-derived fallback, STUN exclusion, and duplicate
     address first-match assertions in one table-driven derivation test;
2. `unmapped_fallback_obeys_bind_device_mode`
   - parameterize `bind_device=true/false`; require fail-closed only for the
     resolved bind-device path and preserve address-only behavior otherwise;
3. `concurrent_consumers_publish_once_per_stable_epoch`
   - inject only the collector closure and count calls; 32 and 128 consumers
     must publish once for the same stable epoch; also assert a cache hit at
     `t + 4.999s` and refresh at `t + 5s`;
4. `inflight_invalidation_discards_old_epoch`
   - pause one injected collection, invalidate, release it, and prove that the
     old result is not published and the latest epoch has one publisher;
     separately cancel the refresh owner and prove another waiter completes;
5. `resolved_bind_target_skips_auto_resolution`
   - inject or count the existing address-to-device resolver at the narrow
     `BindDev` boundary; `Resolved` must consume supplied metadata without
     calling `NetworkInterface::show()` or `if_nametoindex()` when an index is
     supplied;
6. `multi_bind_preserves_success_and_typed_failure`
   - combine partial failure plus success and all-failure cases; partial
     failure must preserve the existing successful race, while all failures
     return a tunnel error carrying the private `LocalBindError` marker rather
     than `Shutdown`;
7. `stale_bind_invalidates_generation_without_retry`
   - use a fake inner connector and generation invalidator to prove one
     transport call, generation-specific invalidation, and no same-attempt
     reconnect; ordinary connect `IOError(EADDRNOTAVAIL)` must not invalidate;
8. `next_attempt_recollects_and_rebuilds_targets`
   - construct a new prepared connector after invalidation and prove it uses a
     newer snapshot and rebuilds resolved targets; existing underlay tests
     continue to prove that every factory attempt enters candidate validation;
     stale preflight permits only one refresh and one revalidation.

Do not add a general interface-enumerator, resolver, socket-binder,
generation-invalidator, and clock trait hierarchy. Use private generic
closures, small fake connectors, or `cfg(test)` counters only at the exact
boundaries needed by these tests. Existing public behavior must remain
unchanged; any additive trait-object extension must be explicitly documented.

The following proposed tests are intentionally not separate requirements:

- `snapshot_excludes_stun_public_addresses` and
  `duplicate_address_preserves_first_match` are covered by the combined
  snapshot derivation test;
- separate mapped/unmapped tests for both bind modes are covered by one
  parameterized test;
- `stale_prepared_connector_cannot_reuse_old_targets` is only needed if code
  inspection finds that a failed `PreparedUnderlayConnector` is reused rather
  than discarded; otherwise the new-attempt test is the observable contract;
- partial bind failure does not need to invalidate the generation when another
  candidate succeeds; the five-second TTL bounds the stale candidate and
  preserving the successful race avoids adding a callback from every
  transport into the cache.

Cross-platform automation boundary:

- Linux `.160` runs the complete platform-independent contract suite,
  namespace tests, `--locked` no-run, and connector/underlay tests;
- existing target builds remain the compile-time `cfg` gate for Android,
  OHOS, iOS NE, macOS NE, Windows, and FreeBSD;
- add a Windows-native resolved-bind test only when a maintained native runner
  is available; Windows must use the correct IPv4/IPv6 interface-index
  contract rather than assuming the snapshot index is interchangeable;
- a FreeBSD-native runtime test is useful but is not a release blocker for this
  macOS CPU remediation when the existing cross-build passes and FreeBSD bind
  semantics remain address-only;
- exact macOS ARM64 artifact validation remains responsible for syscall/CPU
  reduction and real Wi-Fi/default-route transitions.

## Automated contract test implementation design

This section defines the implementation logic for review. No test-only public
API, production-wide trait hierarchy, global mutable mock, real interface
shutdown, or external network dependency is allowed.

### Private seams

Add only these narrow private seams:

1. `build_underlay_snapshot(...)`

   Move the in-memory derivation portion out of
   `do_collect_underlay_snapshot()`. Inputs are the already collected complete
   interface vector, the already filtered IPv4 interface vector, optional
   route-derived IPv4/IPv6 fallbacks, and the generation placeholder. The
   function performs no syscall and returns `UnderlayInterfaceSnapshot`.
   Production still performs exactly one raw enumeration and the same
   filtering/probes before calling it.

2. `UnderlaySnapshotCache::get_or_refresh_at(...)`

   Move the existing cache state machine behind a private generic collector
   closure and an explicit `Instant`. Production
   `collect_underlay_snapshot()` passes `Instant::now()` and the real
   collection closure. Tests pass a closure containing atomics, barriers, and
   deterministic snapshots. This is not a clock trait and does not change the
   public `IPCollector` API.

3. `resolve_bind_dev_with(...)`

   Keep `resolve_bind_dev()` as the production wrapper. The private helper
   accepts the existing address-to-device resolver closure. `Auto` invokes the
   closure; an already resolved target and `Custom` must not. For the Darwin
   index branch, add an equivalent private helper that consumes the supplied
   non-zero index before invoking the fallback `if_nametoindex` closure.

4. `UnderlaySnapshotGenerationLease`

   Replace `PreparedUnderlayConnector`'s direct
   `Arc<IPCollector> + generation` tuple with a crate-private lease containing
   only the generation and an `Arc` to the snapshot cache invalidation state.
   It exposes one idempotent `invalidate()` method. This makes
   generation-specific invalidation independently testable without mocking a
   complete `IPCollector` or `GlobalCtx`.

Do not abstract socket creation behind a new binder trait. Multi-bind behavior
can be tested through a small common helper that receives the already-created
`FuturesUnordered` and the last bind error, which also removes the duplicated
empty-future handling from TCP, UDP, WebSocket, and WireGuard.

Add a crate-private `LocalBindError` source type used only when local socket
creation/bind fails, and wrap it through the existing
`TunnelError::Anyhow(anyhow::Error)` variant. `PreparedUnderlayConnector`
recognizes it with typed `downcast_ref`, never by message matching. Ordinary
connect, handshake, DNS, timeout, resource lookup, and remote connection errors
retain their existing variants. This avoids adding a variant to the public
`TunnelError` enum.

Do not retain a new public `BindDev::Resolved` enum variant because external
exhaustive matching would make it an avoidable compatibility change. Route
resolved metadata through a crate-private bind helper instead. The defaulted
`TunnelConnector::set_resolved_bind_addrs()` extension and its metadata type
are additive but must be doc-hidden and documented as an internal extension
needed by trait objects.

### Synthetic interface fixture

Use deterministic documentation-only addresses:

| Interface | Index | Flags | Addresses | IPv4 filter |
|---|---:|---|---|---|
| `en-test0` | 4 | up, physical | `192.0.2.10`, `2001:db8::10` | included |
| `tun-test0` | 9 | point-to-point | `192.0.2.10`, `10.44.0.90` | excluded |
| `en-test1` | 12 | up, physical | `198.51.100.20`, `2001:db8:1::20` | included |

The complete-interface order is always `en-test0`, `tun-test0`, `en-test1`.
This proves that duplicate `192.0.2.10` maps to `en-test0` through
first-match insertion. Tests must not use a real interface name, host address,
mesh credential, domain, or route.

### Test 1: snapshot derivation

`snapshot_derivation_preserves_sources_and_first_match`:

1. Call `build_underlay_snapshot()` with the synthetic complete and filtered
   vectors.
2. Pass `192.0.2.10` and `2001:db8::10` as route-derived fallbacks already
   present in the interface vectors.
3. Assert filtered IPv4 contains the two physical IPv4 addresses and excludes
   `10.44.0.90`.
4. Assert unfiltered IPv6 contains both global documentation IPv6 addresses.
5. Assert the duplicate IPv4 maps to `en-test0`, index 4, not `tun-test0`.
6. Assert neither `public_ipv4` nor `public_ipv6` is populated because STUN
   observations are not an input to this builder.
7. Assert no fallback is marked unmapped.

This one test replaces separate source-preservation, STUN-exclusion, and
duplicate-address tests.

### Test 2: fallback mode

`unmapped_fallback_obeys_bind_device_mode` is table-driven:

| `bind_device` | Fallback | Expected |
|---|---|---|
| `true` | `203.0.113.30`, absent from all interfaces | fail closed before socket creation |
| `false` | `203.0.113.30`, absent from all interfaces | address remains available to the existing address-only path |

Construct the snapshot through `build_underlay_snapshot()`. Exercise the same
private resolved-target builder used by
`set_bind_addr_for_peer_connector()`. The failing case must return the current
typed configuration/underlay error and the mock connector must receive no
resolved target. The non-bind-device case must not call the resolved-target
builder at all; it preserves the pre-existing connector behavior.

Add two cross-family assertions to the same table:

- an unmapped IPv6 fallback does not block an IPv4 resolved target;
- an unmapped IPv4 fallback does not block an IPv6 resolved target.

Both the one-time preflight recollection and final fail-closed check filter
`unmapped_fallbacks` by the current remote address family.

### Test 3: stable-epoch singleflight

`concurrent_consumers_publish_once_per_stable_epoch`:

1. Create an empty `UnderlaySnapshotCache`.
2. Use a collector closure that increments `collector_calls`, increments an
   `active_collectors` counter, records `max_active_collectors`, waits on a
   barrier, then returns generation-marker snapshot A.
3. Spawn 32 callers against the same epoch and release the collector barrier.
4. Assert all callers receive the same `Arc` and generation.
5. Assert `collector_calls == 1` and `max_active_collectors == 1`.
6. Repeat with 128 callers after one explicit invalidation and require exactly
   one additional collector call.
7. Use `tokio::time::timeout` around the joins so cancellation or waiter
   deadlock becomes a bounded failure.
8. With the injected `Instant`, assert `t + 4.999s` returns the existing `Arc`
   without collection and `t + 5s` starts exactly one refresh.

### Test 4: invalidation during refresh

`inflight_invalidation_discards_old_epoch`:

1. Start collector A and pause it after capturing epoch 0.
2. Invalidate the cache to epoch 1 while A is paused.
3. Release A and assert snapshot A is never published.
4. Permit collector B to return snapshot B for epoch 1.
5. Assert every waiting caller receives B.
6. Assert calls are exactly A plus B, no two collectors run concurrently for
   epoch 1, and the published state reports epoch 1.
7. Cancel one waiting consumer before B completes and assert the remaining
   consumers still finish.
8. In a separate phase, abort the task that owns refresh while its collector
   future is paused; assert the async mutex guard is released, one waiter
   becomes the new owner, and all non-cancelled callers finish.

The discarded old collection is allowed and is not counted as a duplicate
latest-epoch publisher.

### Test 5: resolved bind bypass

`resolved_bind_target_skips_auto_resolution`:

1. Construct a resolved bind target with `en-test0` and non-zero index 4.
2. Pass a resolver closure that increments a counter and then panics.
3. Assert the crate-private resolved bind path returns the supplied name/index
   and the resolver counter remains zero.
4. Under `cfg(any(test, target_os = "ios", target_os = "macos"))`, pass the
   resolved device to the Darwin index-selection helper with a fallback
   closure that panics.
5. Assert index 4 is returned without invoking the fallback.
6. As a control, use `BindDev::Auto` with a non-loopback address and assert the
   resolver is invoked exactly once.

This is sufficient platform-independent proof that resolved metadata bypasses
auto enumeration. Exact macOS profiling remains responsible for proving the
real syscall reduction.

### Test 6: multi-bind result

Introduce one crate-private helper:

```rust
async fn wait_for_bound_connect_futures<Fut, Ret, E>(
    futures: FuturesUnordered<Fut>,
    last_bind_error: Option<TunnelError>,
) -> Result<Ret, TunnelError>;
```

It returns `last_bind_error` when `futures` is empty; otherwise it delegates to
the existing `wait_for_connect_futures()`. TCP, UDP, WebSocket, and WireGuard
use this helper without changing candidate order or racing behavior.

The helper converts an empty multi-bind result caused by local bind `IOError`
into the existing `TunnelError::Anyhow` variant carrying the private
`LocalBindError` source. It must not wrap errors returned by a connect or
handshake future.

`multi_bind_preserves_success_and_typed_failure` asserts:

1. Empty futures plus local bind `IOError(EADDRNOTAVAIL)` returns
   a typed private `LocalBindError` source, not plain `IOError` or `Shutdown`.
2. One ready successful future plus an earlier bind error returns success.
3. Two failed connect futures still return the existing last connect error.
4. The bind error does not invalidate a generation when another candidate
   succeeds.

The existing real TCP/UDP bind tests remain as integration coverage.

### Test 7: stale prepared connector

`stale_bind_invalidates_generation_without_retry`:

1. Build a fake `TunnelConnector` whose `connect()` increments a counter and
   returns `TunnelError::Anyhow` carrying `LocalBindError(EADDRNOTAVAIL)`.
2. Build `PreparedUnderlayConnector` with no breaker lease and a generation
   lease for generation 7.
3. Call `connect()` once.
4. Assert the fake connector count is exactly one.
5. Assert invalidation advances only through generation 7.
6. Publish a synthetic generation 8 before invalidating generation 7 and
   assert generation 8 remains fresh.
7. Assert no timer, retry task, or second fake connector call was created.
8. Repeat with a fake connector returning ordinary
   `IOError(EADDRNOTAVAIL)` and assert generation is not invalidated.
9. Add `ErrorKind::NotFound` and Unix `ENOENT` classifier controls and assert
   neither is considered stale without the typed `LocalBindError` source.

The test does not require a real socket or interface transition.

### Test 8: next attempt

`next_attempt_recollects_and_rebuilds_targets`:

1. Publish snapshot A at generation 7 with `192.0.2.10 -> en-test0`.
2. Invalidate generation 7 through the generation lease.
3. Request a snapshot as a new attempt; the injected collector returns
   snapshot B with `192.0.2.10 -> en-test1`, generation 8.
4. Run the production resolved-target builder with B.
5. Assert the target contains `en-test1`, index 12, and no target from A is
   returned.
6. Assert the collector ran once for the new attempt.
7. Retain the existing connector guarded-destination tests as proof that the
   factory still invokes underlay validation for every newly constructed
   connector.
8. Inject a stale local-bind result into preflight, assert one cache
   invalidation, one recollection, and one revalidation, then return another
   stale result and assert it is returned without a third attempt.

Do not reuse the failed `PreparedUnderlayConnector` in this test. If later code
inspection establishes a supported reuse contract, add a separate poisoned
connector state and test it in that change rather than silently expanding this
batch.

### Production cache benchmark

The standalone `std::Mutex/Condvar` probe is mechanism evidence only and must
not be cited as production-cache throughput. After
`get_or_refresh_at()` exists, add a bounded ignored benchmark-style test that
executes that exact Tokio `RwLock/Mutex` primitive with 32 and 128 consumers,
39 synthetic interfaces, the same collection cost, and the same fresh and
invalidated cases. Run it explicitly on `.160` before artifact dispatch and
record collector calls, maximum concurrent collectors, throughput, and
p50/p95/p99 latency. Keep the standalone probe only as historical comparison
or remove it before merge.

### Platform and CI placement after approval

Before adding workflows, all eight tests must pass through the `.160`
`--locked` lib test binary with `--test-threads 1` for network-sensitive
filters. Pure cache tests may run concurrently internally because their state
is instance-local.

After maintainer review:

- add the eight exact filters to the existing remote preflight script;
- run the same pure tests in the normal Linux Test workflow;
- run the resolved-bind and cache tests in the slim macOS ARM64 candidate;
- let existing Windows, FreeBSD, Android, OHOS, iOS, and macOS NE target jobs
  remain compile gates for this batch;
- do not create a new workflow solely for these tests;
- do not require a Windows or FreeBSD manual checklist to prove cache
  correctness;
- do not start the exact artifact workflow until `.160` no-run and all eight
  tests pass from the same source snapshot.

Windows continues to receive the resolved interface name but uses its existing
native name-to-index resolution. This batch does not claim that Windows
eliminates all interface enumeration, and it must not consume the generic
snapshot index until IPv4/IPv6 interface-index semantics are proven on a
native Windows runner. FreeBSD retains its existing address-only bind behavior.

## Root cause

The current Darwin cache in
`easytier/src/tunnel/common.rs::{InterfaceIndexCache,cached_interface_index}`
only caches the final interface-name-to-index lookup. Its fallback TTL is five
seconds, and selected global events eagerly refresh every cached name.

The expensive full interface snapshot is not covered by that cache:

1. `connector/mod.rs::set_bind_addr_for_peer_connector()` calls
   `IPCollector::collect_local_ip_addrs_now()` for a connection attempt. That
   method currently performs one filtered IPv4 interface scan and a second
   unfiltered IPv6 interface scan.
2. `underlay_guard.rs::bind_device_sources()` calls the same uncached
   collection again during candidate validation, producing another two scans.
3. `underlay_guard.rs::source_interface_signal()` calls
   `IPCollector::collect_interfaces()` again after the connected-source probe.
4. TCP and UDP hole-punch preparation also enter the same underlay guard.
5. `tunnel/common.rs::resolve_bind_dev()` maps every non-loopback
   `BindDev::Auto` address with `get_interface_name_by_ip()`, which independently
   calls `network_interface::NetworkInterface::show()`.

An ordinary `bind_device` connector can therefore perform at least five full
`pnet::datalink::interfaces()` scans before the additional
`NetworkInterface::show()` work performed for socket binds. TCP and UDP
hole-punch preflight usually performs at least three scans, while
`bind_device=false` paths perform fewer. Reconnect or candidate retry bursts
multiply those scans across all interfaces. The five-second index cache cannot
prevent this work.

## Reference behavior

The implementation must follow the externally observable behavior of the local
Mihomo source:

- `/Users/fanli/Documents/mihomo-rev/component/iface/iface.go`
  - `getCache()`
  - `ResolveInterface()`
  - `ResolveInterfaceByAddr()`
  - `FlushCache()`
- `/Users/fanli/Documents/mihomo-rev/component/dialer/bind_darwin.go`
  - `bindIfaceToDialer()`
  - `bindIfaceToListenConfig()`
- `/Users/fanli/Documents/mihomo-rev/listener/sing_tun/iface.go`
  - `defaultInterfaceFinder.Update()`
- `/Users/fanli/Documents/mihomo-rev/listener/sing_tun/server.go`
  - default-interface monitor callback

Relevant Mihomo semantics:

- build one complete interface snapshot and index it by name and address;
- reuse the snapshot for dial and address-to-interface decisions;
- use a bounded 20-second fallback lifetime when its platform network monitor is
  available;
- flush the snapshot when the platform network monitor reports a change;
- rebuild lazily on the next consumer;
- do not eagerly enumerate interfaces for every connection or every event.

`ResolveInterface()` and `ResolveInterfaceByAddr()` do not themselves refresh
on a miss. The sing-tun `ByName()` and `ByIndex()` paths conditionally call
`Update()` after the operating system confirms that the requested interface
exists, while the default-interface monitor callback performs a lazy
`FlushCache()`.

EasyTier currently has no equivalent physical-network monitor that reliably
covers Wi-Fi changes, default-route changes, physical DHCP updates, address
migration, and interface recreation. The first EasyTier implementation must
therefore intentionally use a five-second on-demand fallback TTL. A future
platform monitor may authorize alignment with Mihomo's 20-second lifetime.

EasyTier may also differ where network namespaces or its explicit underlay
breaker require it, but the difference must preserve bounded,
event-invalidated snapshot behavior without weakening fail-closed checks.

## Scope

### In scope

- connector source-address selection;
- underlay candidate source validation;
- address-to-interface safety classification;
- `BindDev::Auto` address-to-device selection;
- Darwin interface-name-to-index lookup;
- TCP and UDP hole-punch preflight;
- DHCP, IPv6, configuration, and platform network-change invalidation;
- concurrent refresh singleflight;
- stale-cache retry;
- bounded CPU, allocations, and lock contention;
- validator/guardian child reaping as a separate lifecycle commit.

### Out of scope

- routing-policy changes;
- peer selection or transport preference changes;
- QUIC, KCP, UDP, TCP, or hole-punch retry interval changes;
- weakening managed-IP or suspicious-interface checks;
- Mihomo, GOST, Leaf, TUN, DNS, FakeDNS, or Geo behavior changes;
- replacing `pnet`;
- adding a new macOS physical-network monitor in the first cache patch;
- Android or OHOS network-observer redesign;
- broad connector refactoring.

## Proposed implementation

### 1. Add a connector interface snapshot cache

Add a cache owned by `IPCollector`, or by a narrowly scoped helper owned by the
same `GlobalCtx`, rather than a process-global cache.

The cached snapshot must contain enough data for all current consumers without
changing the current address-selection semantics:

- IPv4 source addresses derived from the currently filtered interface set;
- IPv6 source addresses derived from the currently unfiltered interface set,
  with the existing multicast, loopback, and link-local exclusions;
- the existing `local_ipv4()` fallback behavior;
- the existing `local_ipv6()` fallback behavior;
- interface name, index, flags, point-to-point state, and addresses;
- an address-to-interface lookup map;
- collection timestamp, invalidation epoch, and published generation.

The snapshot used for bind sources must not include STUN-observed
`public_ipv4` or `public_ipv6`. Those values are not necessarily configured on
a local interface and attempting to bind them can produce `EADDRNOTAVAIL`.

The implementation should perform one operating-system interface collection
and derive the existing IPv4 and IPv6 result sets in memory. It must not retain
the current two-scan implementation behind the new cache.

The address lookup must preserve the current first-match behavior in the
collector's interface order. If the same address appears on multiple
interfaces, construction must retain the first mapping, for example with
`entry.or_insert(...)`, rather than overwrite it with the last interface.

If an existing `local_ipv4()` or `local_ipv6()` fallback address cannot be
mapped to snapshot interface metadata, invalidate and recollect once. If it
remains unmapped and `bind_device=true`, fail closed instead of silently
changing to `BindDev::Disabled`. With `bind_device=false`, preserve the current
address-only behavior.

Scoping the cache to `IPCollector` preserves Linux network namespace isolation
and prevents one EasyTier instance from consuming another instance's snapshot.

### 2. Keep immediate and cached APIs distinct

Do not silently change the contract of
`IPCollector::collect_local_ip_addrs_now()`.

Add an explicit connector-facing API, for example:

```rust
pub async fn collect_underlay_snapshot(
    &self,
) -> Result<Arc<UnderlayInterfaceSnapshot>, Error>;
```

Only connection setup and underlay validation migrate to the cached API.
Callers that explicitly require an immediate uncached observation retain the
existing method.

### 3. Use lazy singleflight refresh

The cache state should provide:

- a fast read path returning an `Arc` snapshot;
- a five-second on-demand fallback freshness lifetime for the first
  implementation;
- one asynchronous refresh lock;
- double-check-after-lock so concurrent attempts share one enumeration;
- a monotonic invalidation epoch;
- no blocking mutex held while `pnet` or namespace work runs;
- no unbounded waiter or snapshot accumulation.

An expired or invalidated cache is rebuilt once. All concurrent connection
attempts await or reuse that result instead of each enumerating interfaces.

A refresh must capture the invalidation epoch before collection and may publish
its result only when the epoch is unchanged. If an event invalidates the cache
while collection is in flight, the old result must not overwrite that event.
The cache remains stale and the current or next consumer performs a new
singleflight collection.

### 4. Invalidate on events instead of eagerly refreshing

DHCP, public IPv6, configuration, and platform network-change events must mark
the relevant snapshot stale. They must not synchronously enumerate interfaces
inside `GlobalCtx::issue_event()`.

The event path must remain cheap and nonblocking. The next connector or
underlay consumer performs the singleflight refresh.

Repeated identical events while the cache is already stale may be coalesced,
but an invalidation concurrent with collection must advance the epoch so the
in-flight result cannot be published as fresh.

The current internal event set does not constitute a complete macOS physical
network monitor. The five-second on-demand fallback remains required until a
separate platform monitor is designed and validated.

### 5. Reuse one snapshot through an attempt

Acquire one snapshot at the beginning of a connector or hole-punch attempt and
pass it through:

- bind-address construction;
- managed-IP filtering;
- underlay source probing;
- source-address-to-interface classification;
- socket `BindDev::Auto` resolution.

Do not call the collector again inside
`validate_connected_udp_source()` or `source_interface_signal()` for the same
attempt. Do not call `NetworkInterface::show()` from a socket bind when the
attempt already carries resolved interface name/index metadata.

Safety decisions remain per candidate. Only the immutable interface data is
shared.

### 6. Preserve stale-data recovery

Stale recovery must distinguish two phases.

Preflight bind or source validation:

- recognize only preserved `TunnelError::IOError` values with an explicit
  stale-interface errno such as `ENXIO`, `ENODEV`, or `EADDRNOTAVAIL`;
- invalidate the snapshot;
- rebuild once;
- repeat preflight once;
- return the real error with context after the retry.

Actual transport socket bind in the first implementation:

- resolve interface metadata before protocol connection or handshake data is
  sent;
- represent that decision explicitly, for example as
  `ResolvedBindTarget { addr, interface_name, interface_index,
  snapshot_generation }` or `BindDev::Resolved`;
- Darwin consumes the resolved index and Linux consumes the resolved name;
- non-attempt callers may retain the existing `BindDev::Auto` fallback;
- preserve each original local socket creation/bind `IOError` while TCP and UDP
  build their connection futures;
- if no local socket was created, return a typed bind failure containing the
  original error instead of collapsing the outcome to `Shutdown`;
- let `PreparedUnderlayConnector` retain only the lightweight snapshot
  generation/invalidation handle needed to mark that generation stale;
- on a recognized stale-interface errno, invalidate that generation and return
  the error; do not refresh or retry inside the same transport attempt;
- the next normal connector attempt acquires a fresh snapshot and reruns the
  complete underlay source validation before constructing new bind targets;
- if at least one local socket was created successfully, preserve the existing
  multi-address racing behavior;
- do not classify DNS, timeout, refusal, TLS, authentication, or general
  connect errors as stale-interface failures.

This deliberately avoids adding an async refresh coordinator inside the
synchronous bind API and avoids repeating a protocol dial or handshake. A
future same-attempt transport retry would require a separately reviewed
coordinator that refreshes the snapshot, reruns the complete fail-closed
underlay validation, and recreates all bind targets before any handshake
starts. It is not part of this first patch.

There must be no request-level infinite retry and no fallback that bypasses the
underlay guard.

### 7. Align the Darwin index cache

The first implementation should source interface name and index from the same
immutable underlay snapshot. `BindDev::Auto` must receive this resolved metadata
instead of starting another system enumeration.

Remove the eager event refresh behavior from the Darwin index path. Do not
retain independent five-second and 20-second caches or separate refresh
generations. If a small synchronous adapter cache remains necessary at the
socket API boundary, it must be populated from the current snapshot generation
and invalidated with that generation, not by another system lookup.

### 8. Reap validator and sidecar children separately

In a separate commit:

- every short-lived Mihomo validator child must be waited after success,
  rejection, timeout, and cancellation;
- every guardian must be waited after its child exits or attachment fails;
- dropping an ownership handle must not be the normal reaping mechanism;
- repeated validation and failure must return child count to baseline.

This lifecycle fix must not depend on or modify the interface snapshot cache.

## Performance prototype

Before changing the production hot path, add a focused Rust prototype under
`tools/` or beside the cache primitive.

The prototype must compare:

- current uncached enumeration model;
- cached snapshot with a fresh hit;
- expired snapshot with 32 concurrent attempts;
- invalidated snapshot with 32 concurrent attempts;
- 128 concurrent attempts with a simulated expensive collector;
- one refresh failure followed by recovery;
- 39 interfaces with mixed IPv4/IPv6 addresses;
- repeated address-to-interface lookups.

Report:

- operations per second;
- `pnet::datalink::interfaces()` invocation count;
- `network_interface::NetworkInterface::show()` invocation count;
- `if_nametoindex()` invocation count;
- p50/p95/p99 latency;
- lock wait time;
- allocations or retained snapshot count;
- maximum concurrent refresh count;
- CPU time.

Run the Rust prototype on `192.168.2.160`. Rust counters may instrument
EasyTier-owned wrappers, but they cannot prove how often dependency internals
invoke Darwin syscalls. Retain the macOS native benchmark and use Instruments
System Trace, DTrace when permitted, `sample`, or an equivalent system probe to
count or sample the actual `NetworkInterface::show()`,
`if_nametoindex/getifaddrs/sysctl` path. Record the chosen collection method and
its resolution. Do not treat platform syscall evidence as a replacement for
the Rust singleflight and contention test.

The production experiment is authorized only if:

- fresh-hit overhead is negligible relative to the current cached lookup;
- each stable latest epoch has at most one successful collection and publish
  under concurrency;
- an in-flight collection from an older epoch may complete and be discarded,
  after which the latest epoch receives exactly one singleflight collection;
- two collectors must never run concurrently for the same latest epoch;
- `BindDev::Auto` consumes snapshot metadata without a second collector;
- memory remains bounded;
- no task can wait forever after collector cancellation or failure.

## Unit and integration tests

### Cache primitive

- fresh snapshot is reused before expiry;
- expiry causes exactly one refresh;
- explicit invalidation causes exactly one published refresh for the latest
  stable epoch;
- repeated invalidation while stale does not create more generations;
- invalidation while refresh is in flight prevents publishing the old epoch;
- invalidation immediately before publish prevents publishing the old epoch;
- cancellation concurrent with invalidation wakes waiters and keeps the cache
  stale;
- an old epoch can never overwrite a newer published generation;
- 32 and 128 concurrent callers observe one collector invocation for the same
  latest epoch;
- refresh failure wakes all waiters and permits a later recovery;
- cancelled refresh ownership does not strand waiters;
- old snapshots are released after all attempt-local `Arc` owners drop;
- namespace/instance keys never share snapshots accidentally.

### Connector and underlay

- ordinary connector source selection and underlay validation share one
  snapshot;
- one system interface scan derives the current-compatible IPv4 and IPv6
  collections;
- existing `local_ipv4()` and `local_ipv6()` fallback behavior is preserved;
- an unmapped fallback address recollects once and then fails closed when
  `bind_device=true`;
- an unmapped fallback address preserves address-only behavior when
  `bind_device=false`;
- duplicate addresses on physical and TUN interfaces preserve the collector's
  current first-match interface choice;
- STUN-observed public addresses are not used as bind sources;
- resolved attempt bind targets use the snapshot and do not call
  `NetworkInterface::show()`;
- TCP hole punch shares one snapshot;
- UDP hole punch shares one snapshot;
- IPv4 and IPv6 candidate filtering remains unchanged;
- managed EasyTier addresses remain blocked;
- suspicious TUN/point-to-point interfaces still add the same breaker strike;
- no usable bind-device source remains fail-closed;
- stale preflight bind or validation retries once after invalidation;
- stale actual transport bind returns a typed original error, invalidates its
  generation, and does not retry within the same attempt;
- the next normal connector attempt recollects and reruns complete underlay
  validation before binding;
- exhausted TCP/UDP local binds do not collapse a preserved bind error to
  `Shutdown`;
- disabling `bind_device` preserves existing behavior;
- Android, OHOS, iOS NE, and macOS NE excluded paths remain unchanged.

### Event behavior

- DHCP address change invalidates without eager enumeration;
- DHCP conflict invalidates without eager enumeration;
- public IPv6 change invalidates without eager enumeration;
- relevant configuration patch invalidates without eager enumeration;
- irrelevant or duplicate events do not enumerate;
- invalidation during collection cannot be overwritten by that collection;
- the next consumer observes the new address/interface generation.

### Child lifecycle

- successful Mihomo `-t` leaves no child or guardian;
- rejected Mihomo `-t` leaves no child or guardian;
- validator timeout leaves no child or guardian;
- guardian attachment failure leaves no child or guardian;
- cancellation during validation leaves no child or guardian;
- 20 repeated validation cycles return process and FD counts to baseline.

## Validation sequence

### 1. Local

- format changed Rust files with Rust 1.95 and edition 2024;
- run no local repository build;
- inspect the exact full diff, lockfile, platform `cfg`, and documentation.

### 2. Remote builder

On `192.168.2.160`:

- run the performance prototype first;
- reject the production mechanism if the prototype is not clearly beneficial;
- run the smallest-feature `--locked` no-run gate;
- run exact cache, connector, underlay, hole-punch, event, and child-lifecycle
  tests;
- do not produce a release build manually.

### 3. One macOS ARM64 candidate

After `.160` passes:

- commit one complete implementation batch;
- trigger one slim macOS ARM64 candidate workflow;
- verify exact SHA, sidecars, signatures, build information, and checksums;
- do not trigger the formal five-workflow release set before real-device
  acceptance.

### 4. macOS real-device matrix

Use the exact installed candidate and the same network/configuration baseline:

- five-minute idle Core CPU sampling with the GUI visible;
- five-minute idle Core CPU sampling with the GUI hidden;
- syscall/call counters for pnet collection, `NetworkInterface::show()`, and
  `if_nametoindex()`;
- Mihomo enabled and policy TUN running;
- Mihomo disabled while mesh remains running;
- IPv4-only connection attempt;
- IPv6-only connection attempt;
- dual-stack connection attempt;
- direct connector;
- TCP hole punch;
- UDP hole punch;
- QUIC and KCP enabled;
- physical-interface address renewal;
- Wi-Fi disable/enable;
- default-route/interface transition;
- three stop/start cycles;
- Mihomo validation success and rejection;
- sidecar crash and recovery;
- final process, thread, FD, route, TUN, and child cleanup.

Terminate profiling immediately if CPU remains above one full core for ten
seconds, process count grows continuously, or network availability is affected.

### 5. Linux regression

The implementation must also pass on Linux:

- namespace isolation;
- `bind_device` source selection;
- IPv4 and IPv6 connector setup;
- direct, TCP-hole-punch, and UDP-hole-punch paths;
- no-TUN, TUN, and veth paths;
- network-change invalidation;
- lifecycle and resource baseline.

Linux validation must not be used as evidence for Darwin syscall behavior.

## Acceptance criteria

All criteria apply to the same exact candidate:

- no idle interface-enumeration storm;
- `if_nametoindex/getifaddrs/sysctl` samples reduced by at least 95% under the
  same macOS workload;
- five-minute idle Core median CPU at or below `2%`;
- five-minute idle Core p95 CPU at or below `5%`;
- no one-second idle Core sample above `20%` after startup convergence;
- a consumer is exposed to stale cached interface data for no more than five
  seconds;
- end-to-end recovery after a real network change is measured separately and
  may additionally depend on the existing connector retry schedule;
- one concurrent reconnect burst with no new invalidation during the burst
  causes one interface snapshot refresh for the latest epoch;
- ordinary connector, TCP hole punch, UDP hole punch, and socket bind do not
  start independent interface scans for the same attempt;
- no degradation of IPv4, IPv6, dual-stack, direct, relay, TCP hole punch, UDP
  hole punch, QUIC, or KCP behavior;
- managed-address and suspicious-interface traffic remains fail-closed;
- no stable throughput regression beyond 5% against the parent candidate;
- no RSS, FD, thread, task, process, or retry-frequency growth;
- no defunct validator or guardian child after repeated validation;
- stop returns routes, TUNs, children, and temporary runtime directories to
  baseline.

If the CPU target cannot be met without weakening loop prevention or network
change correctness, reject the implementation and retain the evidence rather
than publishing a partial optimization.

## Commit boundaries

Use separate reviewable commits:

1. benchmark/prototype and tests;
2. interface snapshot cache and connector/underlay integration;
3. validator/guardian child reaping;
4. pre-build documentation and candidate manifest.

Do not mix GUI, Mihomo configuration, Leaf, DNS, Geo, routing, or transport
feature changes into this batch.

## Rollback

If exact-artifact validation fails:

- revert the interface snapshot implementation commit;
- retain the benchmark, failure evidence, and this TODO;
- independently retain or revert the child-reaping commit based on its own
  lifecycle evidence;
- do not reset or force-push;
- do not compensate by weakening the underlay guard or increasing retry
  frequency.
