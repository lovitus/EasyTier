# USER UNDECIDED: Android Leaf dual-TUN direction

Status: design clarification only. The user has not decided whether this should be
implemented. Do not turn this note into an Android candidate without a separate
decision and validation manifest.

## Correct platform semantics

Android is not accurately described as allowing only one TUN file descriptor.
`VpnService.Builder.establish()` permits a short overlap between an old and a new
VPN interface for seamless handover. Both descriptors can remain valid while the
old interface is drained.

The important limitation is that Android routes outgoing packets to only one active
VPN interface. After a new interface is established successfully, the old interface
is deactivated. This is a handover mechanism, not Linux-style steady-state routing
between two simultaneously active capture TUNs with different route metrics.

Authoritative API semantics:
<https://developer.android.com/reference/android/net/VpnService.Builder#establish()>.

## Reference implementations inspected

### Mihomo Android

The local Mihomo Android fork uses one active system VPN interface:

- `/Users/fanli/Documents/clashmeta-android-rev/service/src/main/java/com/github/kr328/clash/service/TunService.kt::TunModule.open`
  configures one `VpnService.Builder`, calls `establish()`, and returns one detached
  TUN descriptor.
- `/Users/fanli/Documents/clashmeta-android-rev/service/src/main/java/com/github/kr328/clash/service/clash/module/TunModule.kt::attach`
  passes that descriptor to `Clash.startTun(...)`.
- `/Users/fanli/Documents/clashmeta-android-rev/core/src/main/golang/native/tun/tun.go::Start`
  starts the userspace TUN with the supplied descriptor and disables native
  auto-route ownership.

This provides no precedent for two steady-state active Android VPN TUNs.

### Current EasyTier Android host

- `tauri-plugin-vpnservice/android/src/main/java/TauriVpnService.kt` stores one
  `ParcelFileDescriptor?` in `vpnInterface` and creates it through one
  `Builder.establish()` call.
- `easytier-gui/src-tauri/src/lib.rs::set_tun_fd` forwards the descriptor and the
  current mobile DNS/network generation to `set_mobile_tun_and_wait(...)`.

The current lifecycle is therefore also single-active-TUN. It does not currently
exercise Android's temporary handover overlap.

## Why the Linux implementation cannot be copied directly

The Linux candidate intentionally keeps two independently active capture paths:

- a Leaf-owned `etp*` policy TUN as the preferred route;
- the existing EasyTier TUN as the lower-priority compatibility fallback.

Android's normal application API cannot preserve both as active VPN routes at the
same time. Establishing the second system VPN interface deactivates the first.
Normal applications also cannot assume `CAP_NET_ADMIN` to create a separate kernel
TUN outside `VpnService`.

The Linux GSO setup additionally depends on Linux TUN ioctls and VNET headers. A
`VpnService` descriptor does not expose an equivalent Android application contract,
so Android dual descriptors do not automatically provide the Linux GSO gain.

## Feasible Android directions

### A. Seamless TUN handover

Create a replacement `VpnService` interface before closing the previous one, drain
the old descriptor, and atomically move native ownership to the new descriptor.

This can reduce disruption during VPN restart, network-generation change, or DNS
rebuild. It does not provide steady-state dual-path performance or metric fallback.

### B. One system TUN plus two logical native backends

Keep one active `VpnService` TUN and place a bounded native dispatcher immediately
behind it. The dispatcher can route policy packets to Leaf and mesh packets to the
existing EasyTier data plane without creating a second Android system VPN.

This is the closest portable analogue to Linux dual-TUN, but it must not restore the
failed framed `PacketBatch` stream design. A future design needs bounded queues,
explicit ownership and shutdown, packet-boundary preservation, backpressure, and
same-SHA Android profiling before it can be accepted.

### C. Give Leaf the sole system TUN

Passing the only `VpnService` descriptor directly to Leaf removes one copy boundary,
but Leaf would then receive all captured traffic. Mesh bypass, Magic DNS, ownership,
and fail-closed behavior would need a broader redesign. This is more coupled and is
not the default recommendation.

### D. Root/system-only second kernel TUN

A privileged build could create another kernel TUN independently, but root or
system-app privileges are outside the normal EasyTier Android product boundary.
This must not become a hidden requirement.

## Recommended interpretation

Android can use multiple TUN descriptors for handover and can implement a logical
dual data path behind one active VPN interface. It cannot use the exact Linux
steady-state dual-active-route design through the normal `VpnService` contract.

If implementation is later approved, start with seamless handover as a lifecycle
improvement and independently prototype a bounded native dispatcher for performance.
Do not claim Linux GSO-equivalent gains until Android-specific measurements prove
them, and do not change the current Android path merely to make platform diagrams
look uniform.
