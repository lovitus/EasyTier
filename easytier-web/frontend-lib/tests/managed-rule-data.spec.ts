import { describe, expect, it, vi } from 'vitest'
import type * as Api from '../src/modules/api'
import {
  MANAGED_RULE_DATA,
  ensureManagedRuleDataRows,
  managedRuleDataRows,
  updateManagedRuleData,
} from '../src/components/policy/managedRuleData'
import { emptyPolicyDocument } from '../src/components/policy/policyDocument'

describe('managed Geo rule data', () => {
  it('shows fixed managed resources without serializing implicit builtins', () => {
    const document = emptyPolicyDocument()

    const rows = managedRuleDataRows(document)

    expect(rows.map(row => row.type)).toEqual(['geosite', 'geoip', 'mmdb'])
    expect(MANAGED_RULE_DATA.geosite.builtin).toBe(true)
    expect(MANAGED_RULE_DATA.geoip.builtin).toBe(true)
    expect(MANAGED_RULE_DATA.mmdb.builtin).toBe(false)
    expect(document.ruleSets).toEqual([])
  })

  it('adds exactly one row for every supported resource', () => {
    const document = emptyPolicyDocument()
    ensureManagedRuleDataRows(document)
    ensureManagedRuleDataRows(document)

    expect(document.ruleSets.map(row => row.type)).toEqual(['geosite', 'geoip', 'mmdb'])
    expect(document.ruleSets.map(row => row.name)).toEqual(['geosite', 'geoip', 'country'])
  })

  it('updates path and digest only after the remote update succeeds', async () => {
    const document = emptyPolicyDocument()
    ensureManagedRuleDataRows(document)
    const row = document.ruleSets[1]
    const update = vi.fn(async () => ({
      path: '/managed/geoip-lite.dat',
      sha256: 'a'.repeat(64),
      size: 2048,
      source_url: 'https://example.invalid/geoip-lite.dat',
    }))
    const api = { update_policy_rule_data: update } as unknown as Api.RemoteClient

    await updateManagedRuleData(api, 'instance-id', row)

    expect(update).toHaveBeenCalledWith('instance-id', 'geoip', undefined)
    expect(row.path).toBe('/managed/geoip-lite.dat')
    expect(row.sha256).toBe('a'.repeat(64))
    expect(row.update).toBe('manual')
    expect(row.sourceUrl).toBe('')
  })

  it('persists only an explicitly selected custom source', async () => {
    const document = emptyPolicyDocument()
    ensureManagedRuleDataRows(document)
    const row = document.ruleSets[0]
    row.sourceUrl = 'https://mirror.example/geosite.dat'
    const update = vi.fn(async () => ({
      path: '/managed/geosite.dat',
      sha256: 'c'.repeat(64),
      size: 4096,
      source_url: 'https://mirror.example/geosite.dat',
    }))
    const api = { update_policy_rule_data: update } as unknown as Api.RemoteClient

    await updateManagedRuleData(api, 'instance-id', row)

    expect(update).toHaveBeenCalledWith(
      'instance-id',
      'geosite',
      'https://mirror.example/geosite.dat',
    )
    expect(row.sourceUrl).toBe('https://mirror.example/geosite.dat')
  })

  it('preserves the prior file reference when the update fails', async () => {
    const document = emptyPolicyDocument()
    ensureManagedRuleDataRows(document)
    const row = document.ruleSets[0]
    row.path = '/existing/geosite.dat'
    row.sha256 = 'b'.repeat(64)
    const api = {
      update_policy_rule_data: vi.fn(async () => {
        throw new Error('invalid download')
      }),
    } as unknown as Api.RemoteClient

    await expect(updateManagedRuleData(api, 'instance-id', row)).rejects.toThrow('invalid download')
    expect(row.path).toBe('/existing/geosite.dat')
    expect(row.sha256).toBe('b'.repeat(64))
  })
})
