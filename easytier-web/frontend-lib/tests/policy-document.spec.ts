import { describe, expect, it } from 'vitest'
import {
  emptyPolicyDocument,
  parsePolicyDocument,
  serializePolicyDocument,
} from '../src/components/policy/policyDocument'

describe('policy visual document codec', () => {
  it('round-trips mesh/native nodes, ordered groups, geo data and ordered rules', () => {
    const source = `
version: 1
rule-sets:
  site:
    type: geosite
    path: rules/geosite.dat
    update: manual
    sha256: abcdef
  country:
    type: mmdb
    path: rules/country.mmdb
  geoip:
    type: geoip
    path: rules/geoip.dat
proxies:
  mesh-exit:
    type: socks5
    server:
      instance-id: 00000000-0000-0000-0000-000000000001
      virtual-ip: 10.44.0.8
    port: 1080
    via: mesh
    udp: true
  firewall:
    type: socks5
    server: 192.0.2.10
    port: 1081
    via: native
groups:
  preferred:
    type: fallback
    members: [mesh-exit, firewall, DIRECT]
rules:
  - GEOSITE,CN,DIRECT
  - GEOIP,CN,DIRECT,no-resolve
  - NETWORK,udp,preferred
  - MATCH,preferred
`
    const document = parsePolicyDocument(source)
    expect(document.proxies[0]).toMatchObject({
      name: 'mesh-exit',
      via: 'mesh',
      instanceId: '00000000-0000-0000-0000-000000000001',
      virtualIp: '10.44.0.8',
      udp: true,
    })
    expect(document.groups[0].members).toEqual(['mesh-exit', 'firewall', 'DIRECT'])
    expect(document.rules.map(rule => rule.type)).toEqual(['GEOSITE', 'GEOIP', 'NETWORK', 'MATCH'])
    expect(document.rules[1].noResolve).toBe(true)

    const reparsed = parsePolicyDocument(serializePolicyDocument(document))
    expect(reparsed).toEqual(document)
  })

  it('keeps first-match rule order when rows are reordered', () => {
    const document = emptyPolicyDocument()
    document.rules.unshift({ type: 'GEOIP', operand: 'CN', target: 'DIRECT', noResolve: true })
    document.rules.unshift({ type: 'GEOSITE', operand: 'CN', target: 'preferred', noResolve: false })

    const serialized = serializePolicyDocument(document)
    expect(serialized.indexOf('GEOSITE,CN,preferred')).toBeLessThan(serialized.indexOf('GEOIP,CN,DIRECT'))
    expect(serialized.indexOf('GEOIP,CN,DIRECT,no-resolve')).toBeLessThan(serialized.indexOf('MATCH,DIRECT'))
  })

  it('does not emit empty optional credentials or geo sections', () => {
    const document = emptyPolicyDocument()
    document.proxies.push({
      name: 'native',
      type: 'socks5',
      via: 'native',
      address: '127.0.0.1',
      instanceId: '',
      virtualIp: '',
      port: 1080,
      udp: false,
      username: '',
      password: '',
    })

    const serialized = serializePolicyDocument(document)
    expect(serialized).not.toContain('rule-sets:')
    expect(serialized).not.toContain('username:')
    expect(serialized).not.toContain('password:')
    expect(serialized).not.toContain('udp:')
  })

  it('preserves advanced root fields instead of silently discarding them', () => {
    const document = parsePolicyDocument(`
version: 1
future-option:
  enabled: true
rules: ["MATCH,DIRECT"]
`)
    document.rules.unshift({ type: 'NETWORK', operand: 'udp', target: 'DIRECT', noResolve: false })

    const serialized = serializePolicyDocument(document)
    expect(serialized).toContain('future-option:')
    expect(serialized).toContain('enabled: true')
  })

  it('rejects wrong visual field shapes instead of normalizing them', () => {
    expect(() => parsePolicyDocument(`
version: 1
proxies:
  broken:
    type: socks5
    server: 127.0.0.1
    port: "1080"
rules: ["MATCH,broken"]
`)).toThrow('proxies.broken.port must be an integer')
  })

  it('refuses duplicate visual actor names instead of dropping one row', () => {
    const document = emptyPolicyDocument()
    const proxy = {
      name: 'duplicate',
      type: 'socks5' as const,
      via: 'native' as const,
      address: '127.0.0.1',
      instanceId: '',
      virtualIp: '',
      port: 1080,
      udp: false,
      username: '',
      password: '',
    }
    document.proxies.push(proxy, { ...proxy, port: 1081 })

    expect(() => serializePolicyDocument(document)).toThrow('proxy name duplicate is duplicated')
  })

  it('refuses object prototype actor names', () => {
    const document = emptyPolicyDocument()
    document.proxies.push({
      name: '__proto__',
      type: 'socks5',
      via: 'native',
      address: '127.0.0.1',
      instanceId: '',
      virtualIp: '',
      port: 1080,
      udp: false,
      username: '',
      password: '',
    })

    expect(() => serializePolicyDocument(document)).toThrow('proxy name __proto__ is reserved')
  })
})
