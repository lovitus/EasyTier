import { describe, expect, it } from 'vitest'
import { formatMihomoListenPorts } from '../src/modules/mihomoRuntimeDisplay'

describe('Mihomo runtime listener display', () => {
  it('includes the exact controller-reported bind scope for every enabled port', () => {
    expect(formatMihomoListenPorts({
      bind_address: '*',
      mixed_port: 7890,
      http_port: 7891,
      socks_port: 7892,
    })).toBe('mixed *:7890 · http *:7891 · socks *:7892')

    expect(formatMihomoListenPorts({ bind_address: '127.0.0.1', socks_port: 1080 }))
      .toBe('socks 127.0.0.1:1080')
    expect(formatMihomoListenPorts({ bind_address: '0.0.0.0', mixed_port: 7890 }))
      .toBe('mixed 0.0.0.0:7890')
    expect(formatMihomoListenPorts({ bind_address: '::', http_port: 8080 }))
      .toBe('http [::]:8080')
  })

  it('keeps older Core responses readable without inventing a bind scope', () => {
    expect(formatMihomoListenPorts({ mixed_port: 7890 })).toBe('mixed 7890')
  })
})
