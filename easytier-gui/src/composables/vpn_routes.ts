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

const MAGIC_DNS_SERVER = '100.100.100.101'
const POLICY_FAKE_DNS_SERVER = '198.19.0.1'

export interface StaticVpnBootstrap {
  ipv4Addr: string
  networkLength: number
  routes: string[]
}

/**
 * Selects the DNS address published by Android VpnService.
 *
 * The pinned Leaf TUN inbound intercepts UDP/53 before dispatch and restores
 * domains through FakeDNS. 198.19.0.1 is inside the reserved 198.18.0.0/15
 * benchmark range but outside Leaf's 198.18.0.0/16 allocation pool, so it is
 * only a stable packet destination and can never alias an allocated FakeIP.
 * Magic DNS keeps priority because its exact address remains mesh-owned; using
 * both mechanisms requires the separately tracked split-DNS adapter.
 */
export function getDnsForVpn(config: VpnRouteConfig): string | undefined {
  if (config.enable_magic_dns) {
    return MAGIC_DNS_SERVER
  }
  if (config.enable_policy_proxy) {
    return POLICY_FAKE_DNS_SERVER
  }
  return undefined
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
    ret.push(`${MAGIC_DNS_SERVER}/32`)
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
