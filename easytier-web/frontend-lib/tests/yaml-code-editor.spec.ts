import { describe, expect, it } from 'vitest'
import { yamlSyntaxDiagnostics } from '../src/components/policy/yamlValidation'

describe('YAML editor diagnostics', () => {
  it('accepts a normal Mihomo or Leaf mapping', () => {
    expect(yamlSyntaxDiagnostics('proxies: []\nrules:\n  - MATCH,DIRECT\n')).toEqual([])
  })

  it('rejects smart quotes before the source file can be saved', () => {
    const diagnostics = yamlSyntaxDiagnostics(
      'proxies:\n  - {"name": "peer", "type”: "socks5"}\n',
    )
    expect(diagnostics).toHaveLength(1)
    expect(diagnostics[0].message).toMatch(/expected|mapping|missing|end/i)
  })

  it('rejects duplicate mapping keys', () => {
    expect(yamlSyntaxDiagnostics('rules: []\nrules: []\n')).toHaveLength(1)
  })
})
