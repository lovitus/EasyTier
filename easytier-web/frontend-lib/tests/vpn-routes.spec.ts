import { describe, expect, it } from 'vitest'
import {
  getRoutesForVpn,
  getStaticVpnBootstrap,
} from '../../../easytier-gui/src/composables/vpn_routes'

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

  it('bootstraps static Android TUN state without runtime routes', () => {
    expect(getStaticVpnBootstrap({
      dhcp: false,
      virtual_ipv4: '10.245.0.2',
      network_length: 24,
      enable_policy_proxy: true,
    })).toEqual({
      ipv4Addr: '10.245.0.2',
      networkLength: 24,
      routes: ['0.0.0.0/0', '::/0'],
    })
  })

  it('fails static bootstrap closed for DHCP and malformed addresses', () => {
    expect(getStaticVpnBootstrap({
      dhcp: true,
      virtual_ipv4: '10.245.0.2',
      network_length: 24,
    })).toBeUndefined()
    expect(getStaticVpnBootstrap({
      dhcp: false,
      virtual_ipv4: '10.245.0.999',
      network_length: 24,
    })).toBeUndefined()
  })
})
