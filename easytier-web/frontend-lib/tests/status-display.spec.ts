import { describe, expect, it } from 'vitest'
import { latencyMs, lossRate, normalizeNatTypeValue, normalizeRunningInfo, numericValue } from '../src/modules/statusDisplay'

describe('statusDisplay', () => {
  it('parses REST uint64 strings as numbers instead of concatenating them', () => {
    expect(numericValue('42000')).toBe(42000)
    expect(numericValue(42000n)).toBe(42000)
    expect(numericValue('')).toBeUndefined()
  })

  it('normalizes protobuf JSON NAT enum strings', () => {
    expect(normalizeNatTypeValue('FullCone')).toBe(3)
    expect(normalizeNatTypeValue('PortRestricted')).toBe(5)
    expect(normalizeNatTypeValue('sym_udp_firewall')).toBe(7)
    expect(normalizeNatTypeValue('6')).toBe(6)
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

  it('shows cumulative route latency for relay peers without a direct connection', () => {
    const info = {
      route: {
        cost: 3,
        path_latency: 120,
        path_latency_latency_first: 87,
      },
    } as any

    expect(latencyMs(info)).toBe('87ms')
  })

  it('keeps direct connection latency ahead of route latency', () => {
    const info = {
      route: {
        cost: 2,
        path_latency_latency_first: 87,
      },
      peer: {
        default_conn_id: 'default-conn',
        conns: [
          { conn_id: 'default-conn', stats: { latency_us: 42000 } },
        ],
      },
    } as any

    expect(latencyMs(info)).toBe('42ms')
  })

  it('normalizes mixed camelCase running info at the API boundary', () => {
    const normalized = normalizeRunningInfo({
      devName: 'utun7',
      errorMsg: 'mixed-error',
      eventLogs: ['{"time":"2026-07-03T00:00:00Z","event":{"kind":"test"}}'],
      myNodeInfo: {
        virtualIpv4: { address: { addr: '123' }, networkLength: '24' },
        hostname: 'local',
        version: '2.6.7',
        peerId: '7',
        listeners: [{ url: 'udp://0.0.0.0:11010' }],
        ips: {
          publicIpv4: { addr: '1' },
          interfaceIpv4s: [{ addr: '2' }],
          publicIpv6: { part1: '1', part2: '2', part3: '3', part4: '4' },
          interfaceIpv6s: [],
        },
        stunInfo: { udpNatType: 'FullCone', tcpNatType: 'Restricted', lastUpdateTime: '5' },
      },
      peerRoutePairs: [{
        route: {
          peerId: 8,
          ipv4Addr: { address: { addr: '456' }, networkLength: '24' },
          nextHopPeerId: 8,
          cost: 1,
          pathLatencyLatencyFirst: 23,
          proxyCidrs: [],
          hostname: 'peer',
          stunInfo: { udpNatType: 'PortRestricted', tcpNatType: 'Symmetric', lastUpdateTime: '6' },
          instId: 'inst',
          version: '2.6.7',
          featureFlag: {
            isPublicServer: true,
            avoidRelayData: true,
            proxyPrepareAckVersion: '1',
          },
        },
        peer: {
          peerId: 8,
          defaultConnId: 'default-conn',
          conns: [{
            connId: 'default-conn',
            myPeerId: 7,
            isClient: true,
            peerId: 8,
            lossRate: '0.05',
            tunnel: {
              tunnelType: 'quic',
              localAddr: { url: 'quic://0.0.0.0:11010' },
              remoteAddr: { url: 'quic://1.2.3.4:11010' },
              resolvedRemoteAddr: { url: 'quic://1.2.3.4:11010' },
            },
            stats: {
              txBytes: '1000',
              rxBytes: 2048n,
              latencyUs: '15000',
            },
          }],
        },
      }],
      proxyFailoverEntries: [{
        src: { ip: { Ipv4: { addr: '1' } }, port: '1000' },
        dst: { ipv4: { addr: '2' }, port: '2000' },
        startTime: '3',
        requestedTransport: 'quic,kcp,native',
        selectedTransport: 'native',
        fallbackReason: 'quic_policy_denied',
        dstPeerId: '9',
        consecutiveFailures: '1',
        consecutiveSuccesses: '2',
        generation: '4',
        ambiguousTimeoutStrikes: '1',
      }],
      running: true,
    } as any)

    expect(normalized?.dev_name).toBe('utun7')
    expect(normalized?.error_msg).toBe('mixed-error')
    expect(normalized?.events).toHaveLength(1)
    expect(normalized?.my_node_info.peer_id).toBe(7)
    expect(normalized?.my_node_info.virtual_ipv4.network_length).toBe(24)
    expect(normalized?.my_node_info.stun_info.udp_nat_type).toBe(3)
    expect(normalized?.my_node_info.stun_info.tcp_nat_type).toBe(4)
    expect(normalized?.peer_route_pairs[0].route.stun_info?.udp_nat_type).toBe(5)
    expect(normalized?.peer_route_pairs[0].route.stun_info?.tcp_nat_type).toBe(6)
    expect(normalized?.peer_route_pairs[0].route.feature_flag?.is_public_server).toBe(true)
    expect(normalized?.peer_route_pairs[0].route.feature_flag?.proxy_prepare_ack_version).toBe(1)
    expect(normalized?.peer_route_pairs[0].route.path_latency_latency_first).toBe(23)
    expect(latencyMs(normalized?.peer_route_pairs[0] as any)).toBe('15ms')
    expect(normalized?.peer_route_pairs[0].peer?.conns[0].stats?.tx_bytes).toBe(1000)
    expect(normalized?.peer_route_pairs[0].peer?.conns[0].stats?.rx_bytes).toBe(2048)
    expect(normalized?.peer_route_pairs[0].peer?.conns[0].loss_rate).toBe(0.05)
    expect(normalized?.peer_route_pairs[0].peer?.conns[0].tunnel?.tunnel_type).toBe('quic')
    expect(normalized?.proxy_failover_entries?.[0].requested_transport).toBe('quic,kcp,native')
    expect(normalized?.proxy_failover_entries?.[0].src?.ip.oneofKind).toBe('ipv4')
    expect(normalized?.proxy_failover_entries?.[0].generation).toBe(4)
  })

  it('keeps only the newest failover entries returned by older cores', () => {
    const normalized = normalizeRunningInfo({
      proxyFailoverEntries: Array.from({ length: 300 }, (_, generation) => ({
        startTime: Math.floor(generation / 2),
        generation,
      })),
    })

    expect(normalized?.proxy_failover_entries).toHaveLength(256)
    expect(normalized?.proxy_failover_entries?.[0].generation).toBe(299)
    expect(normalized?.proxy_failover_entries?.[255].generation).toBe(44)
  })
})
