import { describe, expect, it, vi } from 'vitest'
import type * as Api from '../src/modules/api'
import {
  ensureManagedRuleDataRows,
  updateManagedRuleData,
} from '../src/components/policy/managedRuleData'
import { emptyPolicyDocument } from '../src/components/policy/policyDocument'

describe('managed Geo rule data', () => {
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

    expect(update).toHaveBeenCalledWith('instance-id', 'geoip')
    expect(row.path).toBe('/managed/geoip-lite.dat')
    expect(row.sha256).toBe('a'.repeat(64))
    expect(row.update).toBe('manual')
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
