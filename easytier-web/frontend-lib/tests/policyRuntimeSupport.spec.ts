import { describe, expect, it } from 'vitest'

import {
  canEnablePolicyProxy,
  policyRuntimeNotice,
} from '../src/components/policy/policyRuntimeSupport'

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
