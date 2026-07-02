import { describe, expect, it } from 'vitest'
import { latencyMs, lossRate, numericValue, stableTunnelProtocols } from '../src/modules/statusDisplay'

describe('statusDisplay', () => {
  it('parses REST uint64 strings as numbers instead of concatenating them', () => {
    expect(numericValue('42000')).toBe(42000)
    expect(numericValue(42000n)).toBe(42000)
    expect(numericValue('')).toBeUndefined()
  })

  it('uses the default connection latency when available', () => {
    const info = {
      peer: {
        default_conn_id: 'default-conn',
        conns: [
          { conn_id: 'stale-conn', stats: { latency_us: '123000' }, loss_rate: undefined },
          { conn_id: 'default-conn', stats: { latency_us: '42000' }, loss_rate: 0.05 },
        ],
      },
    } as any

    expect(latencyMs(info)).toBe('42ms')
    expect(lossRate(info)).toBe('5%')
  })

  it('falls back to the smallest valid latency and skips missing loss values', () => {
    const info = {
      peer: {
        conns: [
          { conn_id: 'a', stats: { latency_us: '123000' }, loss_rate: undefined },
          { conn_id: 'b', stats: { latency_us: '42000' }, loss_rate: '0.07' },
        ],
      },
    } as any

    expect(latencyMs(info)).toBe('42ms')
    expect(lossRate(info)).toBe('7%')
  })

  it('keeps protocol display stable when the default connection changes', () => {
    const info = {
      route: {},
      peer: {
        default_conn_id: 'tcp-conn',
        conns: [
          { conn_id: 'udp-conn', tunnel: { tunnel_type: 'udp' } },
          { conn_id: 'tcp-conn', tunnel: { tunnel_type: 'tcp' } },
        ],
      },
    } as any

    const format = (tunnel?: { tunnel_type: string }) => tunnel?.tunnel_type ?? ''
    expect(stableTunnelProtocols(info, format)).toBe('tcp,udp')

    info.peer.default_conn_id = 'udp-conn'
    info.peer.conns.reverse()
    expect(stableTunnelProtocols(info, format)).toBe('tcp,udp')
  })
})
