import type { PeerRoutePair } from '../types/network'
import type { NetworkInstanceRunningInfo } from '../types/network'

type PeerInfoWithDefaultConn = NonNullable<PeerRoutePair['peer']> & {
  default_conn_id?: string | {
    part1?: number | string | bigint
    part2?: number | string | bigint
    part3?: number | string | bigint
    part4?: number | string | bigint
  }
}

const MAX_PROXY_FAILOVER_ENTRIES = 256

export function numericValue(value: unknown): number | undefined {
  if (typeof value === 'number')
    return Number.isFinite(value) ? value : undefined

  if (typeof value === 'bigint') {
    const parsed = Number(value)
    return Number.isFinite(parsed) ? parsed : undefined
  }

  if (typeof value !== 'string' || value.trim() === '')
    return undefined

  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : undefined
}

function firstDefined<T>(...values: T[]): T | undefined {
  return values.find((value) => value !== undefined && value !== null)
}

function numericOrDefault(value: unknown, fallback = 0): number {
  return numericValue(value) ?? fallback
}

function normalizeIpv4Addr(ip: any): any {
  if (!ip)
    return ip

  return {
    ...ip,
    addr: numericOrDefault(ip.addr),
  }
}

function normalizeIpv6Addr(ip: any): any {
  if (!ip)
    return ip

  return {
    ...ip,
    part1: numericOrDefault(ip.part1),
    part2: numericOrDefault(ip.part2),
    part3: numericOrDefault(ip.part3),
    part4: numericOrDefault(ip.part4),
  }
}

function normalizeIpv4Inet(ip: any): any {
  if (!ip || typeof ip === 'string')
    return ip

  return {
    ...ip,
    address: normalizeIpv4Addr(firstDefined(ip.address, ip.addr)),
    network_length: numericOrDefault(firstDefined(ip.network_length, ip.networkLength)),
  }
}

function normalizeUrl(url: any): any {
  if (!url)
    return url
  if (typeof url === 'string')
    return { url }
  return {
    ...url,
    url: url.url ?? '',
  }
}

function normalizeStunInfo(stun: any): any {
  if (!stun)
    return stun

  return {
    ...stun,
    udp_nat_type: numericOrDefault(firstDefined(stun.udp_nat_type, stun.udpNatType)),
    tcp_nat_type: numericOrDefault(firstDefined(stun.tcp_nat_type, stun.tcpNatType)),
    last_update_time: numericOrDefault(firstDefined(stun.last_update_time, stun.lastUpdateTime)),
  }
}

function normalizeFeatureFlag(flag: any): any {
  if (!flag)
    return flag

  return {
    ...flag,
    is_public_server: firstDefined(flag.is_public_server, flag.isPublicServer) === true,
    avoid_relay_data: firstDefined(flag.avoid_relay_data, flag.avoidRelayData) === true,
    kcp_input: firstDefined(flag.kcp_input, flag.kcpInput) === true,
    no_relay_kcp: firstDefined(flag.no_relay_kcp, flag.noRelayKcp) === true,
    support_conn_list_sync: firstDefined(flag.support_conn_list_sync, flag.supportConnListSync) === true,
    quic_input: firstDefined(flag.quic_input, flag.quicInput) === true,
    no_relay_quic: firstDefined(flag.no_relay_quic, flag.noRelayQuic) === true,
    is_credential_peer: firstDefined(flag.is_credential_peer, flag.isCredentialPeer) === true,
    need_p2p: firstDefined(flag.need_p2p, flag.needP2p) === true,
    disable_p2p: firstDefined(flag.disable_p2p, flag.disableP2p) === true,
    ipv6_public_addr_provider: firstDefined(flag.ipv6_public_addr_provider, flag.ipv6PublicAddrProvider) === true,
    stealth_supported: firstDefined(flag.stealth_supported, flag.stealthSupported) === true,
    stealth_capabilities: firstDefined(flag.stealth_capabilities, flag.stealthCapabilities, []),
    proxy_prepare_ack_version: numericOrDefault(firstDefined(flag.proxy_prepare_ack_version, flag.proxyPrepareAckVersion)),
  }
}

function normalizeNodeInfo(node: any): any {
  if (!node)
    return node

  const ips = node.ips
  const normalizedIps = ips
    ? {
        ...ips,
        public_ipv4: normalizeIpv4Addr(firstDefined(ips.public_ipv4, ips.publicIpv4)),
        interface_ipv4s: (firstDefined(ips.interface_ipv4s, ips.interfaceIpv4s, []) as any[]).map(normalizeIpv4Addr),
        public_ipv6: normalizeIpv6Addr(firstDefined(ips.public_ipv6, ips.publicIpv6)),
        interface_ipv6s: (firstDefined(ips.interface_ipv6s, ips.interfaceIpv6s, []) as any[]).map(normalizeIpv6Addr),
        listeners: firstDefined(ips.listeners, []),
      }
    : undefined

  return {
    ...node,
    virtual_ipv4: normalizeIpv4Inet(firstDefined(node.virtual_ipv4, node.virtualIpv4)),
    hostname: node.hostname ?? '',
    version: node.version ?? '',
    ips: normalizedIps,
    stun_info: normalizeStunInfo(firstDefined(node.stun_info, node.stunInfo)),
    listeners: (firstDefined(node.listeners, []) as any[]).map(normalizeUrl),
    vpn_portal_cfg: firstDefined(node.vpn_portal_cfg, node.vpnPortalCfg),
    peer_id: numericOrDefault(firstDefined(node.peer_id, node.peerId)),
  }
}

function normalizeRoute(route: any): any {
  if (!route)
    return route

  return {
    ...route,
    peer_id: numericOrDefault(firstDefined(route.peer_id, route.peerId)),
    ipv4_addr: normalizeIpv4Inet(firstDefined(route.ipv4_addr, route.ipv4Addr)),
    next_hop_peer_id: numericOrDefault(firstDefined(route.next_hop_peer_id, route.nextHopPeerId)),
    cost: numericOrDefault(route.cost),
    path_latency: numericOrDefault(firstDefined(route.path_latency, route.pathLatency)),
    proxy_cidrs: firstDefined(route.proxy_cidrs, route.proxyCidrs, []),
    hostname: route.hostname ?? '',
    stun_info: normalizeStunInfo(firstDefined(route.stun_info, route.stunInfo)),
    inst_id: firstDefined(route.inst_id, route.instId, ''),
    version: route.version ?? '',
    feature_flag: normalizeFeatureFlag(firstDefined(route.feature_flag, route.featureFlag)),
    next_hop_peer_id_latency_first: firstDefined(route.next_hop_peer_id_latency_first, route.nextHopPeerIdLatencyFirst),
    cost_latency_first: firstDefined(route.cost_latency_first, route.costLatencyFirst),
  }
}

function normalizeTunnel(tunnel: any): any {
  if (!tunnel)
    return tunnel

  return {
    ...tunnel,
    tunnel_type: firstDefined(tunnel.tunnel_type, tunnel.tunnelType, ''),
    local_addr: normalizeUrl(firstDefined(tunnel.local_addr, tunnel.localAddr)),
    remote_addr: normalizeUrl(firstDefined(tunnel.remote_addr, tunnel.remoteAddr)),
    resolved_remote_addr: normalizeUrl(firstDefined(tunnel.resolved_remote_addr, tunnel.resolvedRemoteAddr)),
  }
}

function normalizePeerConnStats(stats: any): any {
  if (!stats)
    return stats

  return {
    ...stats,
    rx_bytes: numericOrDefault(firstDefined(stats.rx_bytes, stats.rxBytes)),
    tx_bytes: numericOrDefault(firstDefined(stats.tx_bytes, stats.txBytes)),
    rx_packets: numericOrDefault(firstDefined(stats.rx_packets, stats.rxPackets)),
    tx_packets: numericOrDefault(firstDefined(stats.tx_packets, stats.txPackets)),
    latency_us: numericOrDefault(firstDefined(stats.latency_us, stats.latencyUs)),
  }
}

function normalizePeerConn(conn: any): any {
  if (!conn)
    return conn

  return {
    ...conn,
    conn_id: firstDefined(conn.conn_id, conn.connId, ''),
    my_peer_id: numericOrDefault(firstDefined(conn.my_peer_id, conn.myPeerId)),
    is_client: firstDefined(conn.is_client, conn.isClient) === true,
    peer_id: numericOrDefault(firstDefined(conn.peer_id, conn.peerId)),
    features: firstDefined(conn.features, []),
    tunnel: normalizeTunnel(conn.tunnel),
    stats: normalizePeerConnStats(conn.stats),
    loss_rate: numericOrDefault(firstDefined(conn.loss_rate, conn.lossRate)),
  }
}

function normalizeUuid(uuid: any): any {
  if (!uuid || typeof uuid === 'string')
    return uuid

  return {
    ...uuid,
    part1: numericOrDefault(uuid.part1),
    part2: numericOrDefault(uuid.part2),
    part3: numericOrDefault(uuid.part3),
    part4: numericOrDefault(uuid.part4),
  }
}

function normalizePeer(peer: any): any {
  if (!peer)
    return peer

  return {
    ...peer,
    peer_id: numericOrDefault(firstDefined(peer.peer_id, peer.peerId)),
    conns: (firstDefined(peer.conns, []) as any[]).map(normalizePeerConn),
    default_conn_id: normalizeUuid(firstDefined(peer.default_conn_id, peer.defaultConnId)),
    directly_connected_conns: (firstDefined(peer.directly_connected_conns, peer.directlyConnectedConns, []) as any[]).map(normalizeUuid),
  }
}

function normalizePeerRoutePair(pair: any): any {
  if (!pair)
    return pair

  return {
    ...pair,
    route: normalizeRoute(pair.route),
    peer: normalizePeer(pair.peer),
  }
}

function normalizeSocketAddr(addr: any): any {
  if (!addr)
    return addr

  const port = numericOrDefault(addr.port)
  const ip = addr.ip
  if (ip?.oneofKind === 'ipv4')
    return { ...addr, ip: { oneofKind: 'ipv4', ipv4: normalizeIpv4Addr(ip.ipv4) }, port }
  if (ip?.oneofKind === 'ipv6')
    return { ...addr, ip: { oneofKind: 'ipv6', ipv6: normalizeIpv6Addr(ip.ipv6) }, port }

  const directIpv4 = firstDefined(addr.ipv4, addr.Ipv4)
  if (directIpv4)
    return { ...addr, ip: { oneofKind: 'ipv4', ipv4: normalizeIpv4Addr(directIpv4) }, port }

  const directIpv6 = firstDefined(addr.ipv6, addr.Ipv6)
  if (directIpv6)
    return { ...addr, ip: { oneofKind: 'ipv6', ipv6: normalizeIpv6Addr(directIpv6) }, port }

  const nestedIpv4 = firstDefined(ip?.ipv4, ip?.Ipv4)
  if (nestedIpv4)
    return { ...addr, ip: { oneofKind: 'ipv4', ipv4: normalizeIpv4Addr(nestedIpv4) }, port }

  const nestedIpv6 = firstDefined(ip?.ipv6, ip?.Ipv6)
  if (nestedIpv6)
    return { ...addr, ip: { oneofKind: 'ipv6', ipv6: normalizeIpv6Addr(nestedIpv6) }, port }

  return { ...addr, ip: { oneofKind: undefined }, port }
}

function normalizeProxyFailoverEntry(entry: any): any {
  if (!entry)
    return entry

  return {
    ...entry,
    src: normalizeSocketAddr(entry.src),
    dst: normalizeSocketAddr(entry.dst),
    start_time: numericOrDefault(firstDefined(entry.start_time, entry.startTime)),
    requested_transport: firstDefined(entry.requested_transport, entry.requestedTransport, ''),
    selected_transport: firstDefined(entry.selected_transport, entry.selectedTransport, ''),
    fallback_reason: firstDefined(entry.fallback_reason, entry.fallbackReason, ''),
    dst_peer_id: numericOrDefault(firstDefined(entry.dst_peer_id, entry.dstPeerId)),
    transport_degraded: firstDefined(entry.transport_degraded, entry.transportDegraded) === true,
    consecutive_failures: numericOrDefault(firstDefined(entry.consecutive_failures, entry.consecutiveFailures)),
    consecutive_successes: numericOrDefault(firstDefined(entry.consecutive_successes, entry.consecutiveSuccesses)),
    generation: numericOrDefault(entry.generation),
    ambiguous_timeout_strikes: numericOrDefault(firstDefined(entry.ambiguous_timeout_strikes, entry.ambiguousTimeoutStrikes)),
  }
}

export function normalizeRunningInfo(raw: any): NetworkInstanceRunningInfo | undefined {
  if (!raw)
    return undefined

  const peerRoutePairs = (firstDefined(raw.peer_route_pairs, raw.peerRoutePairs, []) as any[]).map(normalizePeerRoutePair)
  const proxyFailoverEntries = [
    ...(firstDefined(raw.proxy_failover_entries, raw.proxyFailoverEntries, []) as any[]),
  ]
    .sort((left, right) => {
      const startDiff = numericOrDefault(firstDefined(right.start_time, right.startTime))
        - numericOrDefault(firstDefined(left.start_time, left.startTime))
      if (startDiff !== 0)
        return startDiff
      return numericOrDefault(right.generation) - numericOrDefault(left.generation)
    })
    .slice(0, MAX_PROXY_FAILOVER_ENTRIES)
    .map(normalizeProxyFailoverEntry)
  return {
    ...raw,
    dev_name: firstDefined(raw.dev_name, raw.devName, ''),
    my_node_info: normalizeNodeInfo(firstDefined(raw.my_node_info, raw.myNodeInfo)),
    events: firstDefined(raw.events, raw.eventLogs, []),
    routes: (firstDefined(raw.routes, []) as any[]).map(normalizeRoute),
    peers: (firstDefined(raw.peers, []) as any[]).map(normalizePeer),
    peer_route_pairs: peerRoutePairs,
    running: firstDefined(raw.running, false),
    error_msg: firstDefined(raw.error_msg, raw.errorMsg),
    proxy_failover_entries: proxyFailoverEntries,
  } as NetworkInstanceRunningInfo
}

const peerConnCache = new WeakMap<PeerRoutePair, any[]>()

export function peerConns(info: PeerRoutePair) {
  const cached = peerConnCache.get(info)
  if (cached)
    return cached

  const conns = info.peer?.conns || []
  const connId = defaultConnId(info)
  const sorted = [...conns].sort((a, b) => {
    if (connId) {
      if (a.conn_id === connId)
        return -1
      if (b.conn_id === connId)
        return 1
    }

    const aTunnel = a.tunnel?.tunnel_type ?? ''
    const bTunnel = b.tunnel?.tunnel_type ?? ''
    if (aTunnel !== bTunnel)
      return aTunnel.localeCompare(bTunnel)

    return String(a.conn_id ?? '').localeCompare(String(b.conn_id ?? ''))
  })
  peerConnCache.set(info, sorted)
  return sorted
}

export function defaultConnId(info: PeerRoutePair) {
  const defaultConn = (info.peer as PeerInfoWithDefaultConn | undefined)?.default_conn_id
  if (!defaultConn)
    return undefined

  if (typeof defaultConn === 'string')
    return defaultConn

  const part1 = numericValue(defaultConn.part1) ?? 0
  const part2 = numericValue(defaultConn.part2) ?? 0
  const part3 = numericValue(defaultConn.part3) ?? 0
  const part4 = numericValue(defaultConn.part4) ?? 0
  if (part1 === 0 && part2 === 0 && part3 === 0 && part4 === 0)
    return undefined

  const toHex = (value: number) => value.toString(16).padStart(8, '0')
  const part1Hex = toHex(part1)
  const part2Hex = toHex(part2)
  const part3Hex = toHex(part3)
  const part4Hex = toHex(part4)
  return `${part1Hex}-${part2Hex.slice(0, 4)}-${part2Hex.slice(4, 8)}-${part3Hex.slice(0, 4)}-${part3Hex.slice(4, 8)}${part4Hex}`
}

function defaultConnFirst(info: PeerRoutePair) {
  return peerConns(info)
}

export function latencyMs(info: PeerRoutePair) {
  const connId = defaultConnId(info)
  let minLatencyUs: number | undefined

  for (const conn of peerConns(info)) {
    if (!conn.stats)
      continue

    const latencyUs = numericValue(conn.stats.latency_us)
    if (latencyUs === undefined)
      continue

    if (connId === conn.conn_id)
      return `${Math.ceil(latencyUs / 1000)}ms`

    minLatencyUs = Math.min(minLatencyUs ?? latencyUs, latencyUs)
  }

  if (minLatencyUs === undefined)
    return ''

  return `${Math.ceil(minLatencyUs / 1000)}ms`
}

export function lossRate(info: PeerRoutePair) {
  for (const conn of defaultConnFirst(info)) {
    const loss = numericValue(conn.loss_rate)
    if (loss === undefined)
      continue

    return `${Math.round(loss * 100)}%`
  }

  return ''
}
