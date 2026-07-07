# GUI Global Secure Identity Plan

Status: pending discussion after the current macOS ARM64 GUI validation.

## Goal

Expose explicit `secure_mode.enabled=true` in the GUI without changing wire
format, protobuf, TOML semantics, or the default Stealth behavior.

The GUI should not present the internal name `secure_mode` to normal users.
Use a user-facing term such as **Global Secure Identity** instead.

## UX Model

Add one advanced setting with three states:

| UI state | Stored config | Meaning |
| --- | --- | --- |
| Auto (recommended) | omit `secure_mode` | Default GUI behavior. `network_secret + Stealth` may derive runtime-only keys for Stealth-protected PeerConn handshakes. |
| Enabled | `secure_mode = { enabled = true }` | Explicit Noise identity. Publishes the RoutePeerInfo public key and enables global secure relay/session semantics. |
| Explicitly disabled | `secure_mode = { enabled = false }` | Preserve imported legacy config intent. Must not be saved together with `stealth_mode=true`. |

## Rules

- New GUI networks stay on **Auto (recommended)**.
- Enabling Global Secure Identity writes `secure_mode.enabled=true`; existing
  backend logic may generate missing keys.
- Returning to Auto removes `secure_mode` from the saved GUI config. It must not
  write `enabled=false`.
- `stealth_mode=true` plus `secure_mode.enabled=false` is a save-time conflict.
- Empty `network_secret` still disables the Stealth switch visually, but must not
  rewrite explicit secure identity settings.
- Imported `secure_mode.enabled=true` is shown as Enabled.
- Imported `secure_mode.enabled=false` is shown as Explicitly disabled with a
  warning if Stealth is enabled.

## Non-Goals

- No key editor in the first GUI version.
- No credential management UI.
- No public-key pinning UI.
- No new protobuf, wire, or RPC field.
- No change to CLI/TOML semantics.

## Test Coverage

- Round-trip Auto, Enabled, and Explicitly disabled.
- Import existing explicit secure configs.
- Reject `stealth_mode=true` with explicit `secure_mode.enabled=false`.
- Verify toggling Stealth does not implicitly rewrite secure identity.
- Verify empty secret greys out Stealth but preserves secure identity state.
