import { describe, expect, it } from 'vitest'
import { latencyMs, lossRate, numericValue } from '../src/modules/statusDisplay'

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
})
