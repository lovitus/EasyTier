interface VpnPeerRoute {
  proxy_cidrs?: readonly string[]
}

interface VpnRouteConfig {
  routes?: readonly string[]
  enable_magic_dns?: boolean
  enable_policy_proxy?: boolean
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
