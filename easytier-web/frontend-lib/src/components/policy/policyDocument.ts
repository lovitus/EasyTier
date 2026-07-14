import { parse, stringify } from 'yaml'

export type PolicyRuleSetKind = 'geosite' | 'geoip' | 'mmdb'
export type PolicyProxyKind = 'socks5' | 'http'
export type PolicyProxyVia = 'mesh' | 'native'
export type PolicyGroupKind = 'chain' | 'fallback'

export interface PolicyRuleSetRow {
  name: string
  type: PolicyRuleSetKind
  path: string
  update: string
  sha256: string
}

export interface PolicyProxyRow {
  name: string
  type: PolicyProxyKind
  via: PolicyProxyVia
  address: string
  instanceId: string
  virtualIp: string
  port: number
  udp: boolean
  username: string
  password: string
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
    ruleSets: [],
    proxies: [],
    groups: [],
    rules: [{ type: 'MATCH', operand: '', target: 'DIRECT', noResolve: false }],
  }
}

export function parsePolicyDocument(source: string): PolicyEditorDocument {
  if (source.trim().length === 0) return emptyPolicyDocument()

  const root = requireMap(parse(source), 'policy')
  const knownRootFields = new Set(['version', 'rule-sets', 'proxies', 'groups', 'rules'])
  const extra = Object.fromEntries(
    Object.entries(root).filter(([key]) =>
      !knownRootFields.has(key) && !['__proto__', 'constructor', 'prototype'].includes(key)),
  )
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
    if (!['socks5', 'http'].includes(kind)) throw new Error(`proxies.${name}.type is unsupported`)
    const via = optionalString(value.via, `proxies.${name}.via`) || 'native'
    if (!['mesh', 'native'].includes(via)) throw new Error(`proxies.${name}.via is unsupported`)
    return {
      name,
      type: kind as PolicyProxyKind,
      via: via as PolicyProxyVia,
      address: typeof server === 'string' ? server : '',
      instanceId: optionalString(selector['instance-id'], `proxies.${name}.server.instance-id`),
      virtualIp: optionalString(selector['virtual-ip'], `proxies.${name}.server.virtual-ip`),
      port: requiredPort(value.port, `proxies.${name}.port`),
      udp: optionalBoolean(value.udp, `proxies.${name}.udp`),
      username: optionalString(value.username, `proxies.${name}.username`),
      password: optionalString(value.password, `proxies.${name}.password`),
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
      port: row.port,
      via: row.via,
      udp: row.udp || undefined,
      username: row.username,
      password: row.password,
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
    'rule-sets': Object.keys(ruleSets).length ? ruleSets : undefined,
    proxies: Object.keys(proxies).length ? proxies : undefined,
    groups: Object.keys(groups).length ? groups : undefined,
    rules,
  })
  return stringify(root, { lineWidth: 0 })
}
