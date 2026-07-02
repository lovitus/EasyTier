import type { PeerRoutePair, TunnelInfo } from '../types/network'

type PeerInfoWithDefaultConn = NonNullable<PeerRoutePair['peer']> & {
  default_conn_id?: string | {
    part1?: number | string | bigint
    part2?: number | string | bigint
    part3?: number | string | bigint
    part4?: number | string | bigint
  }
}

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

export function peerConns(info: PeerRoutePair) {
  const conns = info.peer?.conns || []
  const connId = defaultConnId(info)
  return [...conns].sort((a, b) => {
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
}

export function stableTunnelProtocols(
  info: PeerRoutePair,
  format: (tunnel?: TunnelInfo) => string,
) {
  return [...new Set((info.peer?.conns ?? []).map(conn => format(conn.tunnel)).filter(Boolean))]
    .sort((a, b) => a.localeCompare(b))
    .join(',')
}

function defaultConnId(info: PeerRoutePair) {
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

function normalizeStunInfo(stun: any): any {
    if (!stun) return stun;
    return {
        ...stun,
        udp_nat_type: stun.udp_nat_type ?? stun.udpNatType,
        tcp_nat_type: stun.tcp_nat_type ?? stun.tcpNatType,
        last_update_time: stun.last_update_time ?? stun.lastUpdateTime,
    }
}

function normalizeIpv4Inet(ip: any): any {
    if (!ip || typeof ip === 'string') return ip;
    return {
        ...ip,
        network_length: ip.network_length ?? ip.networkLength,
    }
}

function normalizeNodeInfo(node: any): any {
    if (!node) return node;
    const ips = node.ips;
    const normalizedIps = ips ? {
        ...ips,
        public_ipv4: ips.public_ipv4 ?? ips.publicIpv4,
        interface_ipv4s: ips.interface_ipv4s ?? ips.interfaceIpv4s ?? [],
        public_ipv6: ips.public_ipv6 ?? ips.publicIpv6,
        interface_ipv6s: ips.interface_ipv6s ?? ips.interfaceIpv6s ?? [],
        listeners: ips.listeners ?? [],
    } : undefined;

    return {
        ...node,
        virtual_ipv4: normalizeIpv4Inet(node.virtual_ipv4 ?? node.virtualIpv4),
        stun_info: normalizeStunInfo(node.stun_info ?? node.stunInfo),
        vpn_portal_cfg: node.vpn_portal_cfg ?? node.vpnPortalCfg,
        peer_id: node.peer_id ?? node.peerId,
        ips: normalizedIps,
    }
}

function normalizeFeatureFlag(flag: any): any {
    if (!flag) return flag;
    return {
        ...flag,
        is_public_server: flag.is_public_server ?? flag.isPublicServer,
        avoid_relay_data: flag.avoid_relay_data ?? flag.avoidRelayData,
        kcp_input: flag.kcp_input ?? flag.kcpInput,
        no_relay_kcp: flag.no_relay_kcp ?? flag.noRelayKcp,
        support_conn_list_sync: flag.support_conn_list_sync ?? flag.supportConnListSync,
        quic_input: flag.quic_input ?? flag.quicInput,
        no_relay_quic: flag.no_relay_quic ?? flag.noRelayQuic,
        is_credential_peer: flag.is_credential_peer ?? flag.isCredentialPeer,
        need_p2p: flag.need_p2p ?? flag.needP2p,
        disable_p2p: flag.disable_p2p ?? flag.disableP2p,
        ipv6_public_addr_provider: flag.ipv6_public_addr_provider ?? flag.ipv6PublicAddrProvider,
        stealth_supported: flag.stealth_supported ?? flag.stealthSupported,
        stealth_capabilities: flag.stealth_capabilities ?? flag.stealthCapabilities,
        proxy_prepare_ack_version: flag.proxy_prepare_ack_version ?? flag.proxyPrepareAckVersion,
    }
}

function normalizeRoute(route: any): any {
    if (!route) return route;
    return {
        ...route,
        peer_id: route.peer_id ?? route.peerId,
        ipv4_addr: normalizeIpv4Inet(route.ipv4_addr ?? route.ipv4Addr),
        next_hop_peer_id: route.next_hop_peer_id ?? route.nextHopPeerId,
        proxy_cidrs: route.proxy_cidrs ?? route.proxyCidrs,
        inst_id: route.inst_id ?? route.instId,
        feature_flag: normalizeFeatureFlag(route.feature_flag ?? route.featureFlag),
        stun_info: normalizeStunInfo(route.stun_info ?? route.stunInfo),
    }
}

function normalizeTunnel(tunnel: any): any {
    if (!tunnel) return tunnel;
    return {
        ...tunnel,
        tunnel_type: tunnel.tunnel_type ?? tunnel.tunnelType,
        local_addr: tunnel.local_addr ?? tunnel.localAddr,
        remote_addr: tunnel.remote_addr ?? tunnel.remoteAddr,
        resolved_remote_addr: tunnel.resolved_remote_addr ?? tunnel.resolvedRemoteAddr,
    }
}

function normalizePeerConnStats(stats: any): any {
    if (!stats) return stats;
    return {
        ...stats,
        tx_bytes: stats.tx_bytes ?? stats.txBytes,
        rx_bytes: stats.rx_bytes ?? stats.rxBytes,
        tx_packets: stats.tx_packets ?? stats.txPackets,
        rx_packets: stats.rx_packets ?? stats.rxPackets,
        latency_us: stats.latency_us ?? stats.latencyUs,
    }
}

function normalizePeerConn(conn: any): any {
    if (!conn) return conn;
    return {
        ...conn,
        conn_id: conn.conn_id ?? conn.connId,
        my_peer_id: conn.my_peer_id ?? conn.myPeerId,
        is_client: conn.is_client ?? conn.isClient,
        peer_id: conn.peer_id ?? conn.peerId,
        loss_rate: conn.loss_rate ?? conn.lossRate,
        tunnel: normalizeTunnel(conn.tunnel),
        stats: normalizePeerConnStats(conn.stats),
    }
}

function normalizePeer(peer: any): any {
    if (!peer) return peer;
    return {
        ...peer,
        peer_id: peer.peer_id ?? peer.peerId,
        default_conn_id: peer.default_conn_id ?? peer.defaultConnId,
        conns: (peer.conns ?? []).map(normalizePeerConn),
    }
}

function normalizePeerRoutePair(pair: any): any {
    if (!pair) return pair;
    return {
        ...pair,
        route: normalizeRoute(pair.route),
        peer: normalizePeer(pair.peer),
    }
}

function normalizeProxyFailoverEntry(entry: any): any {
    if (!entry) return entry;
    return {
        ...entry,
        start_time: entry.start_time ?? entry.startTime,
        requested_transport: entry.requested_transport ?? entry.requestedTransport,
        selected_transport: entry.selected_transport ?? entry.selectedTransport,
        fallback_reason: entry.fallback_reason ?? entry.fallbackReason,
        dst_peer_id: entry.dst_peer_id ?? entry.dstPeerId,
        consecutive_failures: entry.consecutive_failures ?? entry.consecutiveFailures,
        consecutive_successes: entry.consecutive_successes ?? entry.consecutiveSuccesses,
        ambiguous_timeout_strikes: entry.ambiguous_timeout_strikes ?? entry.ambiguousTimeoutStrikes,
        transport_degraded: entry.transport_degraded ?? entry.transportDegraded,
    }
}

export function normalizeRunningInfo(detail: any): any {
    if (!detail) return detail;
    return {
        ...detail,
        dev_name: detail.dev_name ?? detail.devName,
        my_node_info: normalizeNodeInfo(detail.my_node_info ?? detail.myNodeInfo),
        events: detail.events ?? detail.eventLogs ?? [],
        routes: (detail.routes ?? []).map(normalizeRoute),
        peers: (detail.peers ?? []).map(normalizePeer),
        peer_route_pairs: (detail.peer_route_pairs ?? detail.peerRoutePairs ?? []).map(normalizePeerRoutePair),
        running: detail.running ?? false,
        error_msg: detail.error_msg ?? detail.errorMsg ?? '',
        proxy_failover_entries: (detail.proxy_failover_entries ?? detail.proxyFailoverEntries ?? []).map(normalizeProxyFailoverEntry),
    }
}
