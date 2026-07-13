import { describe, expect, it } from 'vitest'
import { getRoutesForVpn } from '../../../easytier-gui/src/composables/vpn_routes'

describe('getRoutesForVpn', () => {
  it('keeps policy capture routes when runtime peer routes are absent', () => {
    expect(getRoutesForVpn(undefined, {
      enable_policy_proxy: true,
    })).toEqual(['0.0.0.0/0', '::/0'])
  })

  it('keeps configured and Magic DNS routes when runtime peer routes are absent', () => {
    expect(getRoutesForVpn(undefined, {
      routes: ['10.44.0.0/16'],
      enable_magic_dns: true,
    })).toEqual(['10.44.0.0/16', '100.100.100.101/32'])
  })

  it('normalizes host routes and returns a sorted unique list', () => {
    expect(getRoutesForVpn([
      { proxy_cidrs: ['10.88.0.2', '10.88.0.0/24'] },
      { proxy_cidrs: ['10.88.0.2'] },
    ], {
      routes: ['10.88.0.0/24'],
    })).toEqual(['10.88.0.0/24', '10.88.0.2/32'])
  })
})
