import { describe, expect, it } from 'vitest'

import { canEnablePolicyProxy } from '../src/components/policy/policyRuntimeSupport'

describe('canEnablePolicyProxy', () => {
  it('allows enabling before capability discovery and on supported builds', () => {
    expect(canEnablePolicyProxy(undefined)).toBe(true)
    expect(canEnablePolicyProxy({ supported: true })).toBe(true)
  })

  it('rejects enabling when the backend reports no policy runtime', () => {
    expect(canEnablePolicyProxy({ supported: false })).toBe(false)
  })
})
