interface VpnPeerRoute {
  proxy_cidrs?: readonly string[]
}

interface VpnRouteConfig {
  routes?: readonly string[]
  enable_magic_dns?: boolean
  enable_policy_proxy?: boolean
  dhcp?: boolean
  virtual_ipv4?: string
  network_length?: number
}

export interface StaticVpnBootstrap {
  ipv4Addr: string
  networkLength: number
  routes: string[]
}

export function getRoutesForVpn(
  routes: readonly VpnPeerRoute[] | null | undefined,
  config: VpnRouteConfig,
): string[] {
  const ret: string[] = []
  for (const route of routes ?? []) {
    for (let cidr of route.proxy_cidrs ?? []) {
      if (!cidr.includes('/')) {
        cidr += '/32'
      }
      ret.push(cidr)
    }
  }

  ret.push(...(config.routes ?? []))

  if (config.enable_magic_dns) {
    ret.push('100.100.100.101/32')
  }

  if (config.enable_policy_proxy) {
    ret.push('0.0.0.0/0', '::/0')
  }

  return Array.from(new Set(ret)).sort()
}

function isValidIpv4Address(value: string): boolean {
  const octets = value.split('.')
  return (
    octets.length === 4
    && octets.every((octet) => {
      if (!/^\d{1,3}$/.test(octet)) return false
      const numeric = Number(octet)
      return numeric >= 0 && numeric <= 255
    })
  )
}

/**
 * Builds Android TUN startup state without waiting for aggregated runtime status.
 * DHCP remains fail-closed because its address exists only in runtime state.
 */
export function getStaticVpnBootstrap(config: VpnRouteConfig): StaticVpnBootstrap | undefined {
  const ipv4Addr = config.virtual_ipv4?.trim()
  const networkLength = config.network_length

  if (
    config.dhcp
    || !ipv4Addr
    || !isValidIpv4Address(ipv4Addr)
    || networkLength === undefined
    || !Number.isInteger(networkLength)
    || networkLength < 0
    || networkLength > 32
  ) {
    return undefined
  }

  return {
    ipv4Addr,
    networkLength,
    routes: getRoutesForVpn(undefined, config),
  }
}
