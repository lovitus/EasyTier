import type { NetworkTypes } from 'easytier-frontend-lib'
import { addPluginListener } from '@tauri-apps/api/core'
import { Utils } from 'easytier-frontend-lib'
import { setTunFd, updateMobileNetwork } from './backend'
import { get_vpn_status, prepare_vpn, start_vpn, stop_vpn } from 'tauri-plugin-vpnservice-api'

type Route = NetworkTypes.Route

interface vpnStatus {
  running: boolean
  ipv4Addr: string | null | undefined
  ipv4Cidr: number | null | undefined
  routes: string[]
  dns: string | null | undefined
}

let dhcpPollingTimer: NodeJS.Timeout | null = null
const DHCP_POLLING_INTERVAL = 2000 // 2秒后重试
let vpnOperationTail: Promise<void> = Promise.resolve()
let vpnOperationEpoch = 0
let vpnRevokedBySystem = false
let activeVpnInstanceId: string | undefined

const curVpnStatus: vpnStatus = {
  running: false,
  ipv4Addr: undefined,
  ipv4Cidr: undefined,
  routes: [],
  dns: undefined,
}

async function requestVpnPermission() {
  console.log('prepare vpn')
  const prepare_ret = await prepare_vpn()
  console.log('prepare vpn', JSON.stringify((prepare_ret)))
  if (prepare_ret?.errorMsg?.length) {
    throw new Error(prepare_ret.errorMsg)
  }

  const granted = prepare_ret?.granted === true
  if (!granted) {
    console.info('vpn permission request was denied or dismissed')
  }

  return granted
}

function resetVpnConfigStatus() {
  curVpnStatus.ipv4Addr = undefined
  curVpnStatus.ipv4Cidr = undefined
  curVpnStatus.routes = []
  curVpnStatus.dns = undefined
}

function syncVpnStatusFromNative(status: Awaited<ReturnType<typeof get_vpn_status>>) {
  curVpnStatus.running = status?.running ?? false
  if (!curVpnStatus.running) {
    resetVpnConfigStatus()
    return
  }

  const ipv4WithCidr = status?.ipv4Addr
  if (ipv4WithCidr?.length) {
    const [ipv4Addr, cidr] = ipv4WithCidr.split('/')
    curVpnStatus.ipv4Addr = ipv4Addr

    const parsedCidr = Number(cidr)
    curVpnStatus.ipv4Cidr = Number.isInteger(parsedCidr) ? parsedCidr : undefined
  }
  else {
    curVpnStatus.ipv4Addr = undefined
    curVpnStatus.ipv4Cidr = undefined
  }

  curVpnStatus.routes = [...(status?.routes ?? [])]
  curVpnStatus.dns = status?.dns ?? undefined
}

async function waitVpnStatus(target_status: boolean, timeout_sec: number) {
  const start_time = Date.now()
  while (curVpnStatus.running !== target_status) {
    syncVpnStatusFromNative(await get_vpn_status())
    if (curVpnStatus.running === target_status) {
      return
    }
    if (Date.now() - start_time > timeout_sec * 1000) {
      throw new Error('wait vpn status timeout')
    }
    await new Promise(r => setTimeout(r, 50))
  }
}

function runVpnOperation<T>(operation: () => Promise<T>): Promise<T> {
  const result = vpnOperationTail.then(operation, operation)
  vpnOperationTail = result.then(() => undefined, () => undefined)
  return result
}

async function doStopVpn(force = false) {
  const wasRunning = curVpnStatus.running
  if (!force && !wasRunning) {
    return
  }
  console.log('stop vpn')
  const stop_ret = await stop_vpn()
  console.log('stop vpn', JSON.stringify((stop_ret)))
  if (wasRunning) {
    await waitVpnStatus(false, 3)
  }

  resetVpnConfigStatus()
}

async function doStartVpn(instanceId: string, ipv4Addr: string, cidr: number, routes: string[], dns?: string) {
  if (vpnRevokedBySystem) {
    throw new Error('vpn_revoked')
  }
  if (curVpnStatus.running) {
    throw new Error('vpn service is still stopping')
  }

  console.log('start vpn service', ipv4Addr, cidr, routes, dns)
  const request = {
    instanceId,
    ipv4Addr: `${ipv4Addr}/${cidr}`,
    routes,
    dns,
    disallowedApplications: ['com.kkrainbow.easytier'],
    mtu: 1300,
  }

  activeVpnInstanceId = instanceId
  const start_ret = await start_vpn(request)
  if (vpnRevokedBySystem) {
    throw new Error('vpn_revoked')
  }
  console.log('start vpn response', JSON.stringify(start_ret))
  if (start_ret?.errorMsg === 'need_prepare') {
    // Background synchronization must never reclaim VPN ownership from another app.
    // Only prepareVpnService(), called by an explicit GUI action, may request consent.
    vpnRevokedBySystem = true
    activeVpnInstanceId = undefined
    throw new Error('vpn_revoked')
  }

  if (start_ret?.errorMsg?.length) {
    throw new Error(start_ret.errorMsg)
  }
  await waitVpnStatus(true, 3)

  curVpnStatus.ipv4Addr = ipv4Addr
  curVpnStatus.ipv4Cidr = cidr
  curVpnStatus.routes = routes
  curVpnStatus.dns = dns
}

async function startVpnWithRetry(instanceId: string, ipv4Addr: string, cidr: number, routes: string[], dns?: string) {
  let lastError: unknown
  for (let attempt = 0; attempt < 3; attempt++) {
    try {
      return await doStartVpn(instanceId, ipv4Addr, cidr, routes, dns)
    }
    catch (error) {
      lastError = error
      if (vpnRevokedBySystem) {
        throw new Error('vpn_revoked')
      }
      const message = error instanceof Error ? error.message.toLowerCase() : String(error).toLowerCase()
      const transient = message.includes('wait vpn status timeout')
        || message.includes('already')
        || message.includes('stopping')
      if (!transient || attempt === 2) {
        throw error
      }
      syncVpnStatusFromNative(await get_vpn_status())
      await new Promise(resolve => setTimeout(resolve, 150 * (attempt + 1)))
    }
  }
  throw lastError
}

async function onVpnServiceStart(payload: any) {
  console.log('vpn service start', JSON.stringify(payload))
  if (vpnRevokedBySystem) {
    console.info('ignoring a stale VPN start after Android revoked ownership')
    await doStopVpn(true)
    return
  }
  const instanceId = typeof payload?.instanceId === 'string' && payload.instanceId.length
    ? payload.instanceId
    : activeVpnInstanceId
  if (!instanceId) {
    console.error('vpn service start did not identify its EasyTier instance')
    await doStopVpn(true)
    return
  }
  activeVpnInstanceId = instanceId
  curVpnStatus.running = true
  if (Number.isInteger(payload?.fd) && payload.fd >= 0) {
    await setTunFd(instanceId, payload.fd, payload.dnsServers ?? [], payload.networkKey ?? '').catch((e) => {
      console.error('set tun fd failed', e)
      void doStopVpn(true).catch(stopError => console.error('stop vpn after tun setup failure', stopError))
    })
  }
}

async function onVpnNetworkChanged(payload: any) {
  const dnsServers = payload?.dnsServers ?? []
  const networkKey = payload?.networkKey ?? ''
  if (vpnRevokedBySystem || !curVpnStatus.running || !networkKey) {
    return
  }
  await updateMobileNetwork(dnsServers, networkKey).catch((error) => {
    console.error('update mobile network failed', error)
  })
}

async function onVpnServiceStop(payload: any) {
  console.log('vpn service stop', JSON.stringify(payload))
  if (payload?.reason === 'revoked') {
    vpnRevokedBySystem = true
    vpnOperationEpoch += 1
    console.info('Android revoked EasyTier VPN ownership; automatic restart is suppressed')
  }
  curVpnStatus.running = false
  activeVpnInstanceId = undefined
  resetVpnConfigStatus()
}

async function registerVpnServiceListener() {
  console.log('register vpn service listener')
  await addPluginListener(
    'vpnservice',
    'vpn_service_start',
    onVpnServiceStart,
  )

  await addPluginListener(
    'vpnservice',
    'vpn_service_stop',
    onVpnServiceStop,
  )

  await addPluginListener(
    'vpnservice',
    'vpn_network_changed',
    onVpnNetworkChanged,
  )
}

function getRoutesForVpn(routes: Route[], node_config: NetworkTypes.NetworkConfig): string[] {
  if (!routes) {
    return []
  }

  const ret = []
  for (const r of routes) {
    for (let cidr of r.proxy_cidrs) {
      if (!cidr.includes('/')) {
        cidr += '/32'
      }
      ret.push(cidr)
    }
  }

  node_config.routes.forEach(r => {
    ret.push(r)
  })

  if (node_config.enable_magic_dns) {
    ret.push('100.100.100.101/32')
  }

  if (node_config.enable_policy_proxy) {
    ret.push('0.0.0.0/0')
    ret.push('::/0')
  }

  // sort and dedup
  return Array.from(new Set(ret)).sort()
}

async function applyNetworkInstanceChange(instanceId: string, epoch: number) {
  console.error('vpn service network instance change id', instanceId)

  if (vpnRevokedBySystem || epoch !== vpnOperationEpoch) {
    return
  }

  if (dhcpPollingTimer) {
    clearTimeout(dhcpPollingTimer)
    dhcpPollingTimer = null
  }

  if (!instanceId) {
    console.warn('vpn service skipped because instance id is empty')
    if (curVpnStatus.running) {
      await doStopVpn()
    }
    return
  }
  const config = await getConfig(instanceId)
  if (vpnRevokedBySystem || epoch !== vpnOperationEpoch) {
    return
  }
  console.log('vpn service loaded config', instanceId, JSON.stringify({
    no_tun: config.no_tun,
    dhcp: config.dhcp,
    enable_magic_dns: config.enable_magic_dns,
  }))
  if (config.no_tun) {
    console.log('vpn service skipped because no_tun is enabled', instanceId)
    return
  }
  const curNetworkInfo = (await collectNetworkInfo(instanceId)).info?.map?.[instanceId]
  if (vpnRevokedBySystem || epoch !== vpnOperationEpoch) {
    return
  }
  if (!curNetworkInfo || curNetworkInfo?.error_msg?.length) {
    console.warn('vpn service skipped because network info is unavailable', instanceId, curNetworkInfo?.error_msg)
    await doStopVpn()
    return
  }

  const virtual_ip = Utils.ipv4ToString(curNetworkInfo?.my_node_info?.virtual_ipv4?.address)

  if (config.dhcp && (!virtual_ip || !virtual_ip.length)) {
    console.log('DHCP enabled but no IP yet, will retry in', DHCP_POLLING_INTERVAL, 'ms')
    dhcpPollingTimer = setTimeout(() => {
      void onNetworkInstanceChange(instanceId)
    }, DHCP_POLLING_INTERVAL)
    return
  }

  if (!virtual_ip || !virtual_ip.length) {
    await doStopVpn()
    return
  }

  let network_length = curNetworkInfo?.my_node_info?.virtual_ipv4.network_length
  if (!network_length) {
    network_length = 24
  }

  const routes = getRoutesForVpn(curNetworkInfo?.routes, config)

  const dns = config.enable_magic_dns ? '100.100.100.101' : undefined

  const ipChanged = virtual_ip !== curVpnStatus.ipv4Addr
  const cidrChanged = network_length !== curVpnStatus.ipv4Cidr
  const routesChanged = JSON.stringify(routes) !== JSON.stringify(curVpnStatus.routes)
  const dnsChanged = dns != curVpnStatus.dns
  const configChanged = ipChanged || cidrChanged || routesChanged || dnsChanged
  const shouldStartVpn = !curVpnStatus.running

  if (shouldStartVpn || configChanged) {
    console.info('vpn service virtual ip changed', JSON.stringify(curVpnStatus), virtual_ip)
    if (curVpnStatus.running) {
      try {
        await doStopVpn()
      }
      catch (e) {
        console.error(e)
      }
    }

    if (vpnRevokedBySystem || epoch !== vpnOperationEpoch) {
      return
    }

    try {
      await startVpnWithRetry(instanceId, virtual_ip, network_length, routes, dns)
    }
    catch (e) {
      if (e instanceof Error && e.message === 'vpn_revoked') {
        console.info('Android VPN ownership is unavailable; waiting for an explicit user start')
        return
      }
      console.error('start vpn service failed', e)
    }
  }
}

export function onNetworkInstanceChange(instanceId: string) {
  const epoch = ++vpnOperationEpoch
  return runVpnOperation(() => applyNetworkInstanceChange(instanceId, epoch))
}

async function isNoTunEnabled(instanceId: string | undefined) {
  if (!instanceId) {
    return false
  }
  return (await getConfig(instanceId)).no_tun ?? false
}

async function findRunningTunInstanceId() {
  const instanceIds = await listNetworkInstanceIds()
  const runningIds = instanceIds.running_inst_ids.map(Utils.UuidToStr)
  console.log('vpn service sync running instances', JSON.stringify(runningIds))

  for (const instanceId of runningIds) {
    if (await isNoTunEnabled(instanceId)) {
      continue
    }

    return instanceId
  }

  return undefined
}

export async function initMobileVpnService() {
  await registerVpnServiceListener()
}

export async function prepareVpnService(noTun: boolean) {
  if (noTun) {
    return
  }
  // Only an explicit user run with granted ownership may reclaim VpnService after Android
  // assigned it to another VPN.
  const granted = await requestVpnPermission()
  vpnRevokedBySystem = !granted
  if (!granted) {
    throw new Error('vpn_permission_denied')
  }
  vpnOperationEpoch += 1
}

async function applyMobileVpnServiceSync(epoch: number) {
  syncVpnStatusFromNative(await get_vpn_status())
  if (epoch !== vpnOperationEpoch) {
    return
  }
  if (vpnRevokedBySystem) {
    return
  }
  const instanceId = await findRunningTunInstanceId()
  if (epoch !== vpnOperationEpoch || vpnRevokedBySystem) {
    return
  }
  if (instanceId) {
    console.log('vpn service sync selected instance', instanceId)
    await applyNetworkInstanceChange(instanceId, epoch)
    return
  }

  if (dhcpPollingTimer) {
    clearTimeout(dhcpPollingTimer)
    dhcpPollingTimer = null
  }

  await doStopVpn(true)
}

export function syncMobileVpnService() {
  const epoch = ++vpnOperationEpoch
  return runVpnOperation(() => applyMobileVpnServiceSync(epoch))
}
