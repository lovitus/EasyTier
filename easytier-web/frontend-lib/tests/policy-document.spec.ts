import { describe, expect, it } from 'vitest'
import {
  DEFAULT_FAKE_DNS_IPV4_RANGE,
  DEFAULT_FAKE_DNS_IPV6_RANGE,
  DEFAULT_POLICY_TEMPLATE,
  emptyPolicyDocument,
  parsePolicyDocument,
  policyRuleSupportsNoResolve,
  serializePolicyDocument,
} from '../src/components/policy/policyDocument'

describe('policy visual document codec', () => {
  it('provides Mihomo-informed GeoX presets with safe DIRECT-only groups and actor examples', () => {
    const document = emptyPolicyDocument()
    const template = parsePolicyDocument(DEFAULT_POLICY_TEMPLATE)

    expect(template).toEqual(document)
    expect(document.groups.map(group => group.name)).toEqual([
      'default-exit',
      'google-exit',
      'social-exit',
      'telegram-exit',
      'media-exit',
      'github-exit',
      'domestic-exit',
      'other-exit',
    ])
    expect(document.groups.every(group =>
      group.type === 'fallback' && group.members.join(',') === 'DIRECT')).toBe(true)
    expect(document.rules.map(rule => `${rule.type},${rule.operand},${rule.target}`)).toEqual(expect.arrayContaining([
      'GEOSITE,github,github-exit',
      'GEOSITE,geolocation-!cn,other-exit',
      'GEOIP,google,google-exit',
      'GEOIP,CN,domestic-exit',
      'MATCH,,default-exit',
    ]))
    expect(DEFAULT_POLICY_TEMPLATE).toContain('virtual-ip: 10.144.144.2')
    expect(DEFAULT_POLICY_TEMPLATE).toContain('server: 127.0.0.1')
    expect(DEFAULT_POLICY_TEMPLATE).toContain('port: 7890')
    expect(DEFAULT_POLICY_TEMPLATE).toContain('type: shadowsocks')
    expect(DEFAULT_POLICY_TEMPLATE).toContain('cipher: aes-256-gcm')
    expect(DEFAULT_POLICY_TEMPLATE).toContain('udp: uot-v2')
    expect(DEFAULT_POLICY_TEMPLATE).toContain('type: chain')
    expect(DEFAULT_POLICY_TEMPLATE).toContain('members: [mesh-exit, native-socks, DIRECT]')
    expect(document.rules.filter(rule => rule.type === 'GEOIP').every(rule => rule.noResolve)).toBe(true)
    expect(DEFAULT_POLICY_TEMPLATE).toContain('GEOIP,CN,domestic-exit,no-resolve')
  })

  it('defaults no-resolve capability for IP and CIDR rule kinds', () => {
    expect(policyRuleSupportsNoResolve('GEOIP')).toBe(true)
    expect(policyRuleSupportsNoResolve('COUNTRY')).toBe(true)
    expect(policyRuleSupportsNoResolve('IP-CIDR')).toBe(true)
    expect(policyRuleSupportsNoResolve('DOMAIN-SUFFIX')).toBe(false)
    expect(policyRuleSupportsNoResolve('EXTERNAL', 'geoip:google')).toBe(true)
    expect(policyRuleSupportsNoResolve('EXTERNAL', 'site:google')).toBe(false)
  })

  it('rejects HTTP nodes because pinned Leaf has no HTTP CONNECT outbound', () => {
    expect(() => parsePolicyDocument(`
version: 1
proxies:
  http-exit:
    type: http
    server: 192.0.2.10
    port: 8080
rules: ["MATCH,http-exit"]
`)).toThrow('proxies.http-exit.type is unsupported')
  })

  it('round-trips mesh/native nodes, ordered groups, geo data and ordered rules', () => {
    const source = `
version: 1
dns:
  direct: [223.5.5.5]
  proxy: ["doh:cloudflare-dns.com@1.1.1.1"]
  fake-ip-range: 198.19.64.0/22
  fake-ip-range6: fd12:3456:789a::/112
rule-sets:
  site:
    type: geosite
    path: rules/geosite.dat
    update: manual
    sha256: abcdef
    source-url: https://mirror.example/geosite.dat
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
    expect(document.dns).toEqual({
      direct: ['223.5.5.5'],
      proxy: ['doh:cloudflare-dns.com@1.1.1.1'],
      fakeIpRange: '198.19.64.0/22',
      fakeIpRange6: 'fd12:3456:789a::/112',
    })
    expect(document.rules.map(rule => rule.type)).toEqual(['GEOSITE', 'GEOIP', 'NETWORK', 'MATCH'])
    expect(document.rules[1].noResolve).toBe(true)
    expect(document.ruleSets[0].sourceUrl).toBe('https://mirror.example/geosite.dat')

    const reparsed = parsePolicyDocument(serializePolicyDocument(document))
    expect(reparsed).toEqual(document)
  })

  it('adds safe split-DNS defaults to legacy policy documents', () => {
    const document = parsePolicyDocument('version: 1\nrules: ["MATCH,DIRECT"]\n')
    expect(document.dns).toEqual({
      direct: ['system', '223.5.5.5', '119.29.29.29', '114.114.114.114'],
      proxy: [
        'doh:cloudflare-dns.com@1.1.1.1',
        'doh:dns.google@8.8.8.8',
        'doh:dns.quad9.net@9.9.9.9',
      ],
      fakeIpRange: DEFAULT_FAKE_DNS_IPV4_RANGE,
      fakeIpRange6: DEFAULT_FAKE_DNS_IPV6_RANGE,
    })
    const serialized = serializePolicyDocument(document)
    expect(serialized).toContain('doh:cloudflare-dns.com@1.1.1.1')
    expect(serialized).toContain(`fake-ip-range: ${DEFAULT_FAKE_DNS_IPV4_RANGE}`)
    expect(serialized).toContain(`fake-ip-range6: ${DEFAULT_FAKE_DNS_IPV6_RANGE}`)
  })

  it('preserves an explicitly empty proxy DNS set for backend validation', () => {
    const document = parsePolicyDocument(`
version: 1
dns:
  proxy: []
rules: ["MATCH,DIRECT"]
`)
    expect(document.dns.proxy).toEqual([])
    expect(serializePolicyDocument(document)).toContain('proxy: []')
  })

  it('keeps first-match rule order when rows are reordered', () => {
    const document = emptyPolicyDocument()
    document.rules.unshift({ type: 'GEOIP', operand: 'CN', target: 'DIRECT', noResolve: true })
    document.rules.unshift({ type: 'GEOSITE', operand: 'CN', target: 'preferred', noResolve: false })

    const serialized = serializePolicyDocument(document)
    expect(serialized.indexOf('GEOSITE,CN,preferred')).toBeLessThan(serialized.indexOf('GEOIP,CN,DIRECT'))
    expect(serialized.indexOf('GEOIP,CN,DIRECT,no-resolve')).toBeLessThan(serialized.indexOf('MATCH,default-exit'))
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
      cipher: '',
    })

    const serialized = serializePolicyDocument(document)
    expect(serialized).not.toContain('rule-sets:')
    expect(serialized).not.toContain('username:')
    expect(serialized).not.toContain('password:')
    expect(serialized).not.toContain('udp:')
  })

  it('omits the built-in mesh egress port while preserving explicit ports', () => {
    const document = parsePolicyDocument(`
version: 1
proxies:
  automatic:
    type: socks5
    server: { virtual-ip: 10.44.0.8 }
    via: mesh
    udp: true
  explicit:
    type: socks5
    server: { virtual-ip: 10.44.0.9 }
    port: 1080
    via: mesh
rules: ["MATCH,automatic"]
`)

    expect(document.proxies[0].port).toBeNull()
    expect(document.proxies[1].port).toBe(1080)
    const serialized = serializePolicyDocument(document)
    expect(serialized.match(/^    port:/gm)).toHaveLength(1)
    expect(serialized).toContain('port: 1080')
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
      cipher: '',
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
      cipher: '',
    })

    expect(() => serializePolicyDocument(document)).toThrow('proxy name __proto__ is reserved')
  })

  it('round-trips Shadowsocks cipher, password, native UDP, and UoT v2', () => {
    for (const udp of ['off', 'native', 'uot-v2']) {
      const source = `
version: 1
proxies:
  ss:
    type: shadowsocks
    server: 203.0.113.10
    port: 8388
    via: native
    cipher: aes-256-gcm
    password: secret
    udp: ${udp}
rules: ["MATCH,ss"]
`
      const document = parsePolicyDocument(source)
      expect(document.proxies[0]).toMatchObject({
        type: 'shadowsocks',
        address: '203.0.113.10',
        port: 8388,
        via: 'native',
        cipher: 'aes-256-gcm',
        password: 'secret',
        udp,
      })
      expect(parsePolicyDocument(serializePolicyDocument(document))).toEqual(document)
    }
  })

  it('normalizes legacy Shadowsocks UDP booleans and rejects invalid SS-only fields', () => {
    const legacy = parsePolicyDocument(`
version: 1
proxies:
  ss:
    type: shadowsocks
    server: 203.0.113.10
    port: 8388
    cipher: chacha20-ietf-poly1305
    password: secret
    udp: true
rules: ["MATCH,ss"]
`)
    expect(legacy.proxies[0].udp).toBe('native')
    expect(serializePolicyDocument(legacy)).toContain('udp: native')

    expect(() => parsePolicyDocument(`
version: 1
proxies:
  ss:
    type: shadowsocks
    server: { virtual-ip: 10.44.0.8 }
    via: mesh
    cipher: aes-256-gcm
    password: secret
rules: ["MATCH,ss"]
`)).toThrow('via must be native')
  })

  it('round-trips Trojan, VMess WS, and VLESS WSS without exposing internal Leaf actors', () => {
    const source = `
version: 1
proxies:
  trojan:
    type: trojan
    server: edge.example
    port: 443
    password: secret
    tls: { server-name: trojan.example }
    udp: true
  vmess:
    type: vmess
    server: edge.example
    port: 80
    uuid: 00000000-0000-0000-0000-000000000001
    alter-id: 0
    cipher: auto
    transport: { type: websocket, path: /vmess, headers: { Host: vmess.example } }
  vless:
    type: vless
    server: edge.example
    port: 443
    uuid: 00000000-0000-0000-0000-000000000002
    transport: { type: websocket, path: /vless, headers: { Host: vless.example } }
    tls: { server-name: vless.example }
groups:
  through-mesh:
    type: chain
    members: [mesh-exit, vless]
rules: ["MATCH,through-mesh"]
`
    const document = parsePolicyDocument(source)
    expect(document.proxies.map(proxy => proxy.type)).toEqual(['trojan', 'vmess', 'vless'])
    expect(document.proxies[1]).toMatchObject({
      cipher: 'auto',
      alterId: 0,
      transport: 'websocket',
      wsPath: '/vmess',
      wsHeaders: { Host: 'vmess.example' },
    })
    expect(document.proxies[2]).toMatchObject({
      tlsEnabled: true,
      tlsServerName: 'vless.example',
    })
    expect(parsePolicyDocument(serializePolicyDocument(document))).toEqual(document)
  })
})
