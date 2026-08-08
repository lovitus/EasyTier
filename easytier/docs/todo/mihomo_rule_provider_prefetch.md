# Mihomo Rule Provider Initial Prefetch

Status: implementation and remote preflight complete; exact artifact pending.

## User requirement

Extend the existing stopped-network wrench action so a user can optionally
prefetch HTTP rule-provider files through the operating-system proxy or a
custom mesh SOCKS5 endpoint. Ordinary Save, start, runtime updates, and users
who do not need this helper must remain unchanged.

## Locked reference semantics

Reference source: Mihomo
`e26714a181ac0e2fa803453c0a8e9a9ce94e31cb`.

- `rules/provider/parse.go::ParseRuleProvider` downloads only `type: http`;
  `file` and `inline` are not network resources.
- The same function resolves explicit relative `path` against `-d` and uses
  `constant/path.go::path.GetPathByHash("rules", url)` when `path` is omitted.
- `constant/path.go::path.GetPathByHash` names that default cache with the
  lowercase MD5 digest of the URL.
- `rules/provider/parse.go::ruleProviderSchema` defines `url`, `path`,
  `header`, `size-limit`, `proxy`, `format`, and `behavior`.
- `config/config.go::parseRuleProviders` initializes every declared provider;
  the helper therefore prefetches every declared HTTP rule-provider, not only
  providers referenced by a top-level `RULE-SET` rule.
- `component/resource/fetcher.go::Fetcher.Initial` prefers an existing valid
  cache before attempting an update, which is why prefetching the exact cache
  path prevents a slow initial direct download.

## Compatibility boundary

- The existing public RPC and its GeoX-named compatibility types remain
  unchanged. The returned `resource` label uses `rule-provider:<name>`.
- The selected system proxy or custom SOCKS5 endpoint applies only to this
  explicit helper invocation. The provider's YAML `proxy` field is neither
  interpreted nor modified and remains authoritative for Mihomo's later
  updates.
- HTTP/HTTPS, headers, explicit/default paths, and positive `size-limit` are
  supported. `file` and `inline` providers are skipped.
- EasyTier's managed-home model intentionally rejects paths escaping the
  instance-managed directory, even if a standalone Mihomo process could be
  given an additional `SAFE_PATHS` entry.
- GeoX and rule-provider downloads share one bounded transaction. Native
  `mihomo -t` remains authoritative; any download or validation failure rolls
  the entire invocation back.
- Normal Save, startup, controller, provider refresh intervals, ETag handling,
  YAML contents, mesh, GOST, Leaf, DNS, and TUN behavior are unchanged.

## Validation contract

- Planner parity for explicit path and `rules/<md5(url)>` default path.
- Provider header forwarding and positive size-limit enforcement.
- `file`/`inline` exclusion and managed-directory traversal rejection.
- Nested cache-directory creation without following symlink parents.
- Combined GeoX/provider transactional rollback and native `mihomo -t`.
- Existing frontend action still submits the current unsaved draft and never
  invokes Save.

## Preflight evidence

- `.160` final `cargo test --locked --package easytier --lib --no-run`: PASS.
- `.160` GeoX filter: 7/7 PASS; rule-provider filter: 7/7 PASS. The combined
  transaction test appears in both filters, for 13 unique resource tests.
- `.160` `cargo clippy --locked --package easytier --lib -- -D warnings`: PASS.
- `.160` RemoteManagement focused Vitest: 21/21 PASS.
- `.160` frontend-lib, Web frontend, VPN plugin, and GUI production builds:
  PASS in the required dependency order.
- Formal Mihomo `1.19.29` from the published `v3.0.12` artifact accepted an
  HTTP rule-provider preseeded at `rules/<md5(url)>`; `mihomo -t` completed
  successfully without a network fetch.
