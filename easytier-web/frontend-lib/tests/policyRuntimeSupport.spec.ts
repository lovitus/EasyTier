import { describe, expect, it } from 'vitest'

import {
  canEnablePolicyProxy,
  policyRuntimeNotice,
} from '../src/components/policy/policyRuntimeSupport'
import {
  applyPolicyBackend,
  configuredPolicyBackend,
  policyBackendSupported,
} from '../src/types/networkCompat'
import { DEFAULT_NETWORK_CONFIG } from '../src/types/network'

describe('policy backend compatibility', () => {
  it('maps legacy enabled configurations to Leaf', () => {
    expect(configuredPolicyBackend({ enable_policy_proxy: true })).toBe('leaf')
    expect(configuredPolicyBackend({ enable_policy_proxy: false })).toBe('off')
  })

  it('prefers an explicit backend and clears Leaf-only fields for Mihomo', () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.policy_proxy_backend = 'leaf'
    config.enable_policy_proxy = true
    config.policy_config_inline = 'proxies: []'
    config.policy_outbound_interface = 'eth0'
    config.policy_leaf_executable = '/tmp/leaf'
    config.policy_leaf_tun_fast_path = true

    applyPolicyBackend(config, 'mihomo')

    expect(config).toMatchObject({
      enable_policy_proxy: false,
      policy_proxy_backend: 'mihomo',
      policy_config_inline: '',
      policy_outbound_interface: '',
      policy_leaf_executable: '',
      policy_leaf_tun_fast_path: false,
    })
  })

  it('rejects Mihomo on mobile while preserving desktop and Leaf capability semantics', () => {
    expect(policyBackendSupported('mihomo', 'android')).toBe(false)
    expect(policyBackendSupported('mihomo', 'linux')).toBe(true)
    expect(policyBackendSupported('leaf', 'linux', false)).toBe(false)
    expect(policyBackendSupported('off', 'android', false)).toBe(true)
  })
})

describe('canEnablePolicyProxy', () => {
  it('allows enabling before capability discovery and on supported builds', () => {
    expect(canEnablePolicyProxy(undefined)).toBe(true)
    expect(canEnablePolicyProxy({ supported: true })).toBe(true)
  })

  it('rejects enabling when the backend reports no policy runtime', () => {
    expect(canEnablePolicyProxy({ supported: false })).toBe(false)
  })
})

describe('policyRuntimeNotice', () => {
  it('distinguishes validated, experimental, partial, and unavailable platforms', () => {
    expect(policyRuntimeNotice({ platform: 'linux', supported: true })).toBe('linux-supported')
    expect(policyRuntimeNotice({ platform: 'android', supported: true })).toBe('android-experimental')
    expect(policyRuntimeNotice({ platform: 'darwin', supported: false })).toBe('macos-partial')
    expect(policyRuntimeNotice({ platform: 'windows', supported: true })).toBe('windows-supported')
    expect(policyRuntimeNotice({ platform: 'windows', supported: false })).toBe('windows-unsupported')
  })
})
