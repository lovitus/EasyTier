import { parse, stringify } from 'yaml'

export type PolicyRuleSetKind = 'geosite' | 'geoip' | 'mmdb'
export type PolicyProxyKind = 'socks5' | 'shadowsocks' | 'trojan' | 'vmess' | 'vless'
export type PolicyProxyVia = 'mesh' | 'native'
export type PolicyProxyUdp = boolean | 'off' | 'native' | 'uot-v2'
export type PolicyGroupKind = 'chain' | 'fallback'
export const DEFAULT_POLICY_PROXY_DNS = 'doh:cloudflare-dns.com@1.1.1.1'
export const DEFAULT_FAKE_DNS_IPV6_RANGE = 'fd65:6173:7974::/64'
export const DEFAULT_FAKE_DNS_IPV4_RANGE = '198.19.0.0/16'
export const POLICY_SHADOWSOCKS_CIPHERS = [
  'aes-128-gcm',
  'aes-256-gcm',
  'chacha20-poly1305',
  'chacha20-ietf-poly1305',
] as const
export const POLICY_VMESS_CIPHERS = [
  'auto',
  'aes-128-gcm',
  'chacha20-poly1305',
  'chacha20-ietf-poly1305',
] as const
const DEFAULT_POLICY_DIRECT_DNS = ['system', '223.5.5.5', '119.29.29.29', '114.114.114.114']
const DEFAULT_POLICY_PROXY_DNS_SERVERS = [
  DEFAULT_POLICY_PROXY_DNS,
  'doh:dns.google@8.8.8.8',
  'doh:dns.quad9.net@9.9.9.9',
]

export function policyRuleSupportsNoResolve(type: string, operand = ''): boolean {
  const normalizedType = type.trim().toUpperCase()
  if (['IP-CIDR', 'GEOIP', 'COUNTRY'].includes(normalizedType)) return true
  if (normalizedType !== 'EXTERNAL') return false
  const source = operand.split(':', 1)[0]?.trim().toLowerCase()
  return ['mmdb', 'geoip', 'geoip-dat'].includes(source)
}

// Reference semantics:
// - Mihomo rules/parser.go::ParseRule, rules/common/geosite.go::GEOSITE.Match,
//   and rules/common/geoip.go::GEOIP.Match keep ordered, first-match GeoX rules.
// - Mihomo adapter/outboundgroup/parser.go::ParseProxyGroup and
//   adapter/outboundgroup/fallback.go::NewFallback define ordered fallback groups;
//   current Mihomo explicitly rejects the removed relay group.
// - Mihomo adapter/parser.go::ParseProxy and
//   adapter/outbound/shadowsocks.go::{ShadowSocksOption,NewShadowSocks} define
//   Shadowsocks server/port/cipher/password/UDP fields. EasyTier intentionally
//   spells the actor kind `shadowsocks` and extends Mihomo's UDP boolean to
//   off/native/uot-v2 while retaining boolean input compatibility.
// EasyTier intentionally exposes only its validated v1 actors here. Its `chain` is
// Leaf's sequential actor chain, not Mihomo's removed relay group, and provider,
// select, and url-test fields remain unsupported rather than looking effective.
export const DEFAULT_POLICY_TEMPLATE = `# EasyTier Leaf policy v1
# Every preset group contains only DIRECT, so this file works before any node is configured.
# Add a proxy name before DIRECT in a fallback group to prefer it. Remove DIRECT when
# proxy failure must be fail-closed instead of falling back to the native connection.
version: 1
dns:
  # The first four addresses are reserved; 198.19.0.1 remains the virtual DNS.
  fake-ip-range: 198.19.0.0/16
  # Used only when EasyTier IPv6 is enabled. Change it if this ULA overlaps your network.
  fake-ip-range6: fd65:6173:7974::/64
  direct:
    - system
    - 223.5.5.5
    - 119.29.29.29
    - 114.114.114.114
  proxy:
    - doh:cloudflare-dns.com@1.1.1.1
    - doh:dns.google@8.8.8.8
    - doh:dns.quad9.net@9.9.9.9

# Mesh SOCKS example: select an EasyTier peer by virtual IP or instance ID.
# proxies:
#   mesh-exit:
#     type: socks5
#     server:
#       virtual-ip: 10.144.144.2
#       # instance-id: 00000000-0000-0000-0000-000000000000
#     via: mesh
#     udp: true
#
#   native-socks:
#     type: socks5
#     server: 127.0.0.1
#     port: 7890
#     via: native
#     udp: true
#     # username: user
#     # password: password
#
#   native-ss-uot:
#     type: shadowsocks
#     server: 203.0.113.10
#     port: 8388
#     via: native
#     cipher: aes-256-gcm
#     password: change-me
#     # off, native, or uot-v2; true/false remain accepted for old files.
#     udp: uot-v2
#
#   native-trojan:
#     type: trojan
#     server: edge.example.com
#     port: 443
#     password: change-me
#     tls: { server-name: cdn.example.com }
#     udp: true
#
#   native-vmess-ws:
#     type: vmess
#     server: edge.example.com
#     port: 80
#     uuid: 00000000-0000-0000-0000-000000000000
#     alter-id: 0
#     cipher: auto
#     transport: { type: websocket, path: /vmess, headers: { Host: cdn.example.com } }
#     udp: true
#
#   native-vless-wss:
#     type: vless
#     server: edge.example.com
#     port: 443
#     uuid: 00000000-0000-0000-0000-000000000000
#     transport: { type: websocket, path: /vless, headers: { Host: cdn.example.com } }
#     tls: { server-name: cdn.example.com }
#     udp: true
#
# Compose any native protocol after a mesh SOCKS actor instead of setting via: mesh:
#   mesh-vless:
#     type: chain
#     members: [mesh-exit, native-vless-wss]

groups:
  default-exit:
    type: fallback
    members: [DIRECT]
  google-exit:
    type: fallback
    members: [DIRECT]
  social-exit:
    type: fallback
    members: [DIRECT]
  telegram-exit:
    type: fallback
    members: [DIRECT]
  media-exit:
    type: fallback
    members: [DIRECT]
  github-exit:
    type: fallback
    members: [DIRECT]
  domestic-exit:
    type: fallback
    members: [DIRECT]
  other-exit:
    type: fallback
    members: [DIRECT]

  # Chain sends TCP through every member in order. Pinned Leaf SOCKS UDP does not
  # reuse chain transport, so route UDP explicitly to a proven mesh actor instead.
  # Uncomment only after both proxy actors above exist.
  # chained-exit:
  #   type: chain
  #   members: [mesh-exit, native-socks]

  # Fallback prefers the first actor that can be established. It does not rescue
  # UDP after an association succeeds but payload is lost. This example keeps
  # DIRECT as the final bypass; remove it when a failed proxy must block traffic.
  # Multi-connection protocols may need a whole-transaction retry during the
  # first transition.
  # preferred-exit:
  #   type: fallback
  #   members: [mesh-exit, native-socks, DIRECT]

# Bundled GeoSite and GeoIP snapshots are selected automatically; no local paths
# are required. Rules are first-match. The preset groups currently resolve to
# DIRECT, so choosing an exit only requires editing the corresponding members.
rules:
  - GEOIP,LAN,DIRECT,no-resolve
  - GEOSITE,github,github-exit
  - GEOSITE,twitter,social-exit
  - GEOSITE,youtube,google-exit
  - GEOSITE,google,google-exit
  - GEOSITE,telegram,telegram-exit
  - GEOSITE,netflix,media-exit
  - GEOSITE,bilibili,media-exit
  - GEOSITE,bahamut,media-exit
  - GEOSITE,spotify,media-exit
  - GEOSITE,CN,domestic-exit
  - GEOSITE,geolocation-!cn,other-exit
  - GEOIP,google,google-exit,no-resolve
  - GEOIP,netflix,media-exit,no-resolve
  - GEOIP,telegram,telegram-exit,no-resolve
  - GEOIP,twitter,social-exit,no-resolve
  - GEOIP,CN,domestic-exit,no-resolve
  - MATCH,default-exit
`

export interface PolicyDnsSettings {
  direct: string[]
  proxy: string[]
  fakeIpRange: string
  fakeIpRange6: string
}

export interface PolicyRuleSetRow {
  name: string
  type: PolicyRuleSetKind
  path: string
  update: string
  sha256: string
  sourceUrl: string
}

export interface PolicyProxyRow {
  name: string
  type: PolicyProxyKind
  via: PolicyProxyVia
  address: string
  instanceId: string
  virtualIp: string
  port: number | null
  udp: PolicyProxyUdp
  username: string
  password: string
  cipher: string
  uuid?: string
  alterId?: number
  transport?: 'tcp' | 'websocket'
  wsPath?: string
  wsHeaders?: Record<string, string>
  tlsEnabled?: boolean
  tlsServerName?: string
  tlsInsecure?: boolean
}

export interface PolicyGroupRow {
  name: string
  type: PolicyGroupKind
  members: string[]
}

export interface PolicyRuleRow {
  type: string
  operand: string
  target: string
  noResolve: boolean
}

export interface PolicyEditorDocument {
  version: number
  extra: Record<string, unknown>
  dns: PolicyDnsSettings
  ruleSets: PolicyRuleSetRow[]
  proxies: PolicyProxyRow[]
  groups: PolicyGroupRow[]
  rules: PolicyRuleRow[]
}

type UnknownMap = Record<string, unknown>

function requireMap(value: unknown, path: string): UnknownMap {
  if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
    return value as UnknownMap
  }
  throw new Error(`${path} must be a mapping`)
}

function optionalMap(root: UnknownMap, key: string): UnknownMap {
  return root[key] === undefined ? {} : requireMap(root[key], key)
}

function optionalString(value: unknown, path: string): string {
  if (value === undefined) return ''
  if (typeof value === 'string') return value
  throw new Error(`${path} must be a string`)
}

function requiredString(value: unknown, path: string): string {
  if (typeof value === 'string') return value
  throw new Error(`${path} must be a string`)
}

function optionalBoolean(value: unknown, path: string): boolean {
  if (value === undefined) return false
  if (typeof value === 'boolean') return value
  throw new Error(`${path} must be a boolean`)
}

function proxyUdp(value: unknown, kind: PolicyProxyKind, path: string): PolicyProxyUdp {
  if (kind === 'shadowsocks') {
    if (value === undefined || value === false || value === 'off') return 'off'
    if (value === true || value === 'native') return 'native'
    if (value === 'uot-v2') return 'uot-v2'
    throw new Error(`${path} must be off, native, uot-v2, true, or false`)
  }
  if (value === 'off') return false
  if (value === 'native') return true
  if (value === 'uot-v2') throw new Error(`${path} uot-v2 is only valid for Shadowsocks`)
  return optionalBoolean(value, path)
}

function optionalStringList(value: unknown, path: string, fallback: string[] = []): string[] {
  if (value === undefined) return [...fallback]
  if (!Array.isArray(value)) throw new Error(`${path} must be a sequence`)
  return value.map((entry, index) => requiredString(entry, `${path}[${index}]`))
}

function optionalStringMap(value: unknown, path: string): Record<string, string> {
  if (value === undefined) return {}
  const source = requireMap(value, path)
  return Object.fromEntries(Object.entries(source).map(([key, entry]) => [
    key,
    requiredString(entry, `${path}.${key}`),
  ]))
}

function requiredPort(value: unknown, path: string): number {
  if (typeof value === 'number' && Number.isInteger(value)) return value
  throw new Error(`${path} must be an integer`)
}

function requiredInteger(value: unknown, path: string): number {
  if (typeof value === 'number' && Number.isInteger(value)) return value
  throw new Error(`${path} must be an integer`)
}

function parseRule(source: unknown, index: number): PolicyRuleRow {
  if (typeof source !== 'string') throw new Error(`rules[${index}] must be a string`)
  const parts = source.split(',').map(part => part.trim())
  const noResolve = parts[parts.length - 1]?.toLowerCase() === 'no-resolve'
  if (noResolve) parts.pop()
  if (parts.length <= 2) {
    return {
      type: (parts[0] || 'MATCH').toUpperCase(),
      operand: '',
      target: parts[1] || 'DIRECT',
      noResolve,
    }
  }
  return {
    type: (parts[0] || 'MATCH').toUpperCase(),
    operand: parts.slice(1, -1).join(','),
    target: parts[parts.length - 1] || 'DIRECT',
    noResolve,
  }
}

export function emptyPolicyDocument(): PolicyEditorDocument {
  return {
    version: 1,
    extra: {},
    dns: {
      direct: [...DEFAULT_POLICY_DIRECT_DNS],
      proxy: [...DEFAULT_POLICY_PROXY_DNS_SERVERS],
      fakeIpRange: DEFAULT_FAKE_DNS_IPV4_RANGE,
      fakeIpRange6: DEFAULT_FAKE_DNS_IPV6_RANGE,
    },
    ruleSets: [],
    proxies: [],
    groups: [
      { name: 'default-exit', type: 'fallback', members: ['DIRECT'] },
      { name: 'google-exit', type: 'fallback', members: ['DIRECT'] },
      { name: 'social-exit', type: 'fallback', members: ['DIRECT'] },
      { name: 'telegram-exit', type: 'fallback', members: ['DIRECT'] },
      { name: 'media-exit', type: 'fallback', members: ['DIRECT'] },
      { name: 'github-exit', type: 'fallback', members: ['DIRECT'] },
      { name: 'domestic-exit', type: 'fallback', members: ['DIRECT'] },
      { name: 'other-exit', type: 'fallback', members: ['DIRECT'] },
    ],
    rules: [
      { type: 'GEOIP', operand: 'LAN', target: 'DIRECT', noResolve: true },
      { type: 'GEOSITE', operand: 'github', target: 'github-exit', noResolve: false },
      { type: 'GEOSITE', operand: 'twitter', target: 'social-exit', noResolve: false },
      { type: 'GEOSITE', operand: 'youtube', target: 'google-exit', noResolve: false },
      { type: 'GEOSITE', operand: 'google', target: 'google-exit', noResolve: false },
      { type: 'GEOSITE', operand: 'telegram', target: 'telegram-exit', noResolve: false },
      { type: 'GEOSITE', operand: 'netflix', target: 'media-exit', noResolve: false },
      { type: 'GEOSITE', operand: 'bilibili', target: 'media-exit', noResolve: false },
      { type: 'GEOSITE', operand: 'bahamut', target: 'media-exit', noResolve: false },
      { type: 'GEOSITE', operand: 'spotify', target: 'media-exit', noResolve: false },
      { type: 'GEOSITE', operand: 'CN', target: 'domestic-exit', noResolve: false },
      { type: 'GEOSITE', operand: 'geolocation-!cn', target: 'other-exit', noResolve: false },
      { type: 'GEOIP', operand: 'google', target: 'google-exit', noResolve: true },
      { type: 'GEOIP', operand: 'netflix', target: 'media-exit', noResolve: true },
      { type: 'GEOIP', operand: 'telegram', target: 'telegram-exit', noResolve: true },
      { type: 'GEOIP', operand: 'twitter', target: 'social-exit', noResolve: true },
      { type: 'GEOIP', operand: 'CN', target: 'domestic-exit', noResolve: true },
      { type: 'MATCH', operand: '', target: 'default-exit', noResolve: false },
    ],
  }
}

export function parsePolicyDocument(source: string): PolicyEditorDocument {
  if (source.trim().length === 0) return emptyPolicyDocument()

  const root = requireMap(parse(source), 'policy')
  const knownRootFields = new Set(['version', 'dns', 'rule-sets', 'proxies', 'groups', 'rules'])
  const extra = Object.fromEntries(
    Object.entries(root).filter(([key]) =>
      !knownRootFields.has(key) && !['__proto__', 'constructor', 'prototype'].includes(key)),
  )
  const dnsIsMissing = !Object.prototype.hasOwnProperty.call(root, 'dns')
  const dnsValue = optionalMap(root, 'dns')
  const dns = {
    direct: optionalStringList(
      dnsValue.direct,
      'dns.direct',
      dnsIsMissing ? [...DEFAULT_POLICY_DIRECT_DNS] : [],
    ),
    proxy: optionalStringList(
      dnsValue.proxy,
      'dns.proxy',
      dnsIsMissing ? [...DEFAULT_POLICY_PROXY_DNS_SERVERS] : [DEFAULT_POLICY_PROXY_DNS],
    ),
    fakeIpRange: optionalString(
      dnsValue['fake-ip-range'],
      'dns.fake-ip-range',
    ) || DEFAULT_FAKE_DNS_IPV4_RANGE,
    fakeIpRange6: optionalString(
      dnsValue['fake-ip-range6'],
      'dns.fake-ip-range6',
    ) || DEFAULT_FAKE_DNS_IPV6_RANGE,
  }
  const ruleSets = Object.entries(optionalMap(root, 'rule-sets')).map(([name, raw]) => {
    const value = requireMap(raw, `rule-sets.${name}`)
    const kind = requiredString(value.type, `rule-sets.${name}.type`)
    if (!['geosite', 'geoip', 'mmdb'].includes(kind)) throw new Error(`rule-sets.${name}.type is unsupported`)
    return {
      name,
      type: kind as PolicyRuleSetKind,
      path: requiredString(value.path, `rule-sets.${name}.path`),
      update: optionalString(value.update, `rule-sets.${name}.update`) || 'manual',
      sha256: optionalString(value.sha256, `rule-sets.${name}.sha256`),
      sourceUrl: optionalString(value['source-url'], `rule-sets.${name}.source-url`),
    }
  })
  const proxies = Object.entries(optionalMap(root, 'proxies')).map(([name, raw]) => {
    const value = requireMap(raw, `proxies.${name}`)
    const server = value.server
    if (typeof server !== 'string' && (server === null || typeof server !== 'object' || Array.isArray(server))) {
      throw new Error(`proxies.${name}.server must be an address or mesh selector`)
    }
    const selector = typeof server === 'string' ? {} : requireMap(server, `proxies.${name}.server`)
    const kind = requiredString(value.type, `proxies.${name}.type`)
    if (!['socks5', 'shadowsocks', 'trojan', 'vmess', 'vless'].includes(kind)) throw new Error(`proxies.${name}.type is unsupported`)
    const via = optionalString(value.via, `proxies.${name}.via`) || 'native'
    if (!['mesh', 'native'].includes(via)) throw new Error(`proxies.${name}.via is unsupported`)
    if (kind !== 'socks5' && via !== 'native') {
      throw new Error(`proxies.${name}.via must be native; compose a mesh SOCKS actor before the native protocol actor in a chain`)
    }
    if (kind !== 'socks5' && typeof server !== 'string') {
      throw new Error(`proxies.${name}.server must be an address for ${kind}`)
    }
    const cipher = optionalString(value.cipher, `proxies.${name}.cipher`)
    if (kind === 'shadowsocks' && !POLICY_SHADOWSOCKS_CIPHERS.includes(cipher as typeof POLICY_SHADOWSOCKS_CIPHERS[number])) {
      throw new Error(`proxies.${name}.cipher is unsupported`)
    }
    if (kind === 'vmess' && !POLICY_VMESS_CIPHERS.includes(cipher as typeof POLICY_VMESS_CIPHERS[number])) {
      throw new Error(`proxies.${name}.cipher is unsupported`)
    }
    if (!['shadowsocks', 'vmess'].includes(kind) && cipher) {
      throw new Error(`proxies.${name}.cipher is only valid for Shadowsocks or VMess`)
    }
    const username = optionalString(value.username, `proxies.${name}.username`)
    if (kind === 'shadowsocks' && username) {
      throw new Error(`proxies.${name}.username is not valid for Shadowsocks`)
    }
    const password = optionalString(value.password, `proxies.${name}.password`)
    if (kind === 'shadowsocks' && !password) {
      throw new Error(`proxies.${name}.password is required for Shadowsocks`)
    }
    if (kind === 'trojan' && !password) throw new Error(`proxies.${name}.password is required for Trojan`)
    if (['vmess', 'vless'].includes(kind) && password) {
      throw new Error(`proxies.${name}.password is not valid for ${kind}`)
    }
    const layered = ['trojan', 'vmess', 'vless'].includes(kind)
    const uuid = optionalString(value.uuid, `proxies.${name}.uuid`)
    if (['vmess', 'vless'].includes(kind) && !uuid) throw new Error(`proxies.${name}.uuid is required`)
    if (!['vmess', 'vless'].includes(kind) && uuid) throw new Error(`proxies.${name}.uuid is not valid for ${kind}`)
    const alterId = value['alter-id'] === undefined && value.alterId === undefined
      ? 0
      : requiredInteger(value['alter-id'] ?? value.alterId, `proxies.${name}.alter-id`)
    if (kind === 'vmess' && alterId !== 0) throw new Error(`proxies.${name}.alter-id must be 0`)
    if (kind !== 'vmess' && (value['alter-id'] !== undefined || value.alterId !== undefined)) {
      throw new Error(`proxies.${name}.alter-id is only valid for VMess`)
    }
    const transportValue = value.transport === undefined ? {} : requireMap(value.transport, `proxies.${name}.transport`)
    const transport = optionalString(transportValue.type, `proxies.${name}.transport.type`) || 'tcp'
    if (!['tcp', 'websocket'].includes(transport)) throw new Error(`proxies.${name}.transport.type is unsupported`)
    if (!layered && value.transport !== undefined) throw new Error(`proxies.${name}.transport is not valid for ${kind}`)
    const tlsValue = value.tls === undefined ? {} : requireMap(value.tls, `proxies.${name}.tls`)
    const tlsEnabled = value.tls !== undefined
    if (!layered && tlsEnabled) throw new Error(`proxies.${name}.tls is not valid for ${kind}`)
    if (kind === 'trojan' && !tlsEnabled) throw new Error(`proxies.${name}.tls is required for Trojan`)
    return {
      name,
      type: kind as PolicyProxyKind,
      via: via as PolicyProxyVia,
      address: typeof server === 'string' ? server : '',
      instanceId: optionalString(selector['instance-id'], `proxies.${name}.server.instance-id`),
      virtualIp: optionalString(selector['virtual-ip'], `proxies.${name}.server.virtual-ip`),
      port: via === 'mesh' && value.port == null
        ? null
        : requiredPort(value.port, `proxies.${name}.port`),
      udp: proxyUdp(value.udp, kind as PolicyProxyKind, `proxies.${name}.udp`),
      username,
      password,
      cipher,
      uuid,
      alterId,
      transport: transport as 'tcp' | 'websocket',
      wsPath: optionalString(transportValue.path, `proxies.${name}.transport.path`) || '/',
      wsHeaders: optionalStringMap(transportValue.headers, `proxies.${name}.transport.headers`),
      tlsEnabled,
      tlsServerName: optionalString(tlsValue['server-name'], `proxies.${name}.tls.server-name`),
      tlsInsecure: optionalBoolean(tlsValue.insecure, `proxies.${name}.tls.insecure`),
    }
  })
  const groups = Object.entries(optionalMap(root, 'groups')).map(([name, raw]) => {
    const value = requireMap(raw, `groups.${name}`)
    const kind = requiredString(value.type, `groups.${name}.type`)
    if (!['chain', 'fallback'].includes(kind)) throw new Error(`groups.${name}.type is unsupported`)
    if (!Array.isArray(value.members)) throw new Error(`groups.${name}.members must be a sequence`)
    return {
      name,
      type: kind as PolicyGroupKind,
      members: value.members.map((member, index) =>
        requiredString(member, `groups.${name}.members[${index}]`)),
    }
  })
  if (!Array.isArray(root.rules)) throw new Error('rules must be a sequence')
  const rules = root.rules.map(parseRule)

  return {
    version: requiredInteger(root.version, 'version'),
    extra,
    dns,
    ruleSets,
    proxies,
    groups,
    rules,
  }
}

function compact<T extends UnknownMap>(value: T): T {
  for (const key of Object.keys(value)) {
    if (value[key] === '' || value[key] === undefined) delete value[key]
  }
  return value
}

function reserveName(target: UnknownMap, rawName: string, path: string): string {
  const name = rawName.trim()
  if (!name) throw new Error(`${path} name must not be empty`)
  if (['__proto__', 'constructor', 'prototype'].includes(name)) {
    throw new Error(`${path} name ${name} is reserved`)
  }
  if (Object.prototype.hasOwnProperty.call(target, name)) {
    throw new Error(`${path} name ${name} is duplicated`)
  }
  return name
}

export function serializePolicyDocument(document: PolicyEditorDocument): string {
  const ruleSets: UnknownMap = {}
  for (const row of document.ruleSets) {
    const name = reserveName(ruleSets, row.name, 'rule-set')
    ruleSets[name] = compact({
      type: row.type,
      path: row.path,
      update: row.update || 'manual',
      sha256: row.sha256,
      'source-url': row.sourceUrl,
    })
  }

  const proxies: UnknownMap = {}
  for (const row of document.proxies) {
    const name = reserveName(proxies, row.name, 'proxy')
    const server = row.via === 'mesh'
      ? compact({ 'instance-id': row.instanceId, 'virtual-ip': row.virtualIp })
      : row.address
    proxies[name] = compact({
      type: row.type,
      server,
      port: row.port ?? undefined,
      via: row.via,
      udp: row.type === 'shadowsocks'
        ? (typeof row.udp === 'boolean' ? (row.udp ? 'native' : 'off') : row.udp)
        : (typeof row.udp === 'string' ? row.udp === 'native' : row.udp) || undefined,
      username: row.username,
      password: row.password,
      cipher: row.cipher,
      uuid: row.uuid,
      'alter-id': row.type === 'vmess' ? row.alterId ?? 0 : undefined,
      transport: ['trojan', 'vmess', 'vless'].includes(row.type) && row.transport === 'websocket'
        ? compact({ type: 'websocket', path: row.wsPath || '/', headers: row.wsHeaders })
        : undefined,
      tls: ['trojan', 'vmess', 'vless'].includes(row.type) && row.tlsEnabled
        ? compact({ 'server-name': row.tlsServerName, insecure: row.tlsInsecure || undefined })
        : undefined,
    })
  }

  const groups: UnknownMap = {}
  for (const row of document.groups) {
    const name = reserveName(groups, row.name, 'group')
    groups[name] = { type: row.type, members: row.members.filter(Boolean) }
  }

  const rules = document.rules.map(row => {
    const type = row.type.trim().toUpperCase()
    const base = row.operand.trim()
      ? `${type},${row.operand.trim()},${row.target.trim()}`
      : `${type},${row.target.trim()}`
    return row.noResolve ? `${base},no-resolve` : base
  })
  const root = compact({
    ...document.extra,
    version: document.version || 1,
    dns: compact({
      direct: document.dns.direct.length ? document.dns.direct : undefined,
      proxy: document.dns.proxy,
      'fake-ip-range': document.dns.fakeIpRange,
      'fake-ip-range6': document.dns.fakeIpRange6,
    }),
    'rule-sets': Object.keys(ruleSets).length ? ruleSets : undefined,
    proxies: Object.keys(proxies).length ? proxies : undefined,
    groups: Object.keys(groups).length ? groups : undefined,
    rules,
  })
  return stringify(root, { lineWidth: 0 })
}
