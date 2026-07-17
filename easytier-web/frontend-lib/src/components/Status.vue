<script setup lang="ts">
import { NetworkInstance, type TunnelInfo, type PeerRoutePair } from '../types/network'
import { useI18n } from 'vue-i18n';
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import { ipv4InetToString, ipv4ToString, ipv6ToString } from '../modules/utils';
import { latencyMs, lossRate, normalizeNatTypeValue, numericValue, peerConns } from '../modules/statusDisplay';
import { Badge, DataTable, Column, Tag, Chip, Button, Dialog, ScrollPanel, Timeline, Divider, Card, } from 'primevue';
import NetworkChart from './NetworkChart.vue';

const props = defineProps<{
  curNetworkInst: NetworkInstance | null,
}>()

const { t } = useI18n()
const MAX_PROXY_FAILOVER_ROWS = 256

const peerRouteInfos = computed(() => {
  if (props.curNetworkInst) {
    const my_node_info = props.curNetworkInst.detail?.my_node_info
    return [{
      route: {
        peer_id: my_node_info?.peer_id ?? 0,
        ipv4_addr: my_node_info?.virtual_ipv4 ?? null,
        next_hop_peer_id: 0,
        cost: 0,
        proxy_cidrs: [],
        hostname: my_node_info?.hostname ?? '',
        version: my_node_info?.version ?? '',
        stun_info: my_node_info?.stun_info,
        inst_id: props.curNetworkInst.instance_id,
      },
    }, ...(props.curNetworkInst.detail?.peer_route_pairs || [])]
  }

  return []
})

interface PeerRow {
  row_key: string
  peer_id: number
  virtual_ipv4: string
  hostname: string
  hostname_tooltip: string
  route_cost: string
  tunnel_proto: string
  latency: string
  tx_bytes: string
  rx_bytes: string
  tx_total_bytes: number
  rx_total_bytes: number
  loss_rate: string
  nat_type: string
  version: string
  is_public_server: boolean
  avoid_relay_data: boolean
}

function resolveObjPath(path: string, obj: any = globalThis, separator = '.') {
  const properties = path.split(separator)
  return properties.reduce((prev, curr) => prev?.[curr], obj)
}

function statsCommon(conns: any[], field: string): number | undefined {
  if (conns.length === 0)
    return undefined

  let sum = 0
  let hasValue = false
  for (const conn of conns) {
    const value = numericValue(resolveObjPath(field, conn))
    if (value === undefined)
      continue

    sum += value
    hasValue = true
  }
  return hasValue ? sum : undefined
}

function humanFileSize(bytes: number, si = false, dp = 1) {
  const thresh = si ? 1000 : 1024

  if (Math.abs(bytes) < thresh)
    return `${bytes} B`

  const units = si
    ? ['kB', 'MB', 'GB', 'TB', 'PB', 'EB', 'ZB', 'YB']
    : ['KiB', 'MiB', 'GiB', 'TiB', 'PiB', 'EiB', 'ZiB', 'YiB']
  let u = -1
  const r = 10 ** dp

  do {
    bytes /= thresh
    ++u
  } while (Math.round(Math.abs(bytes) * r) / r >= thresh && u < units.length - 1)

  return `${bytes.toFixed(dp)} ${units[u]}`
}

function formatByteTotal(bytes?: number) {
  return bytes ? humanFileSize(bytes) : ''
}

function version(info: PeerRoutePair) {
  return info.route.version === '' ? 'unknown' : info.route.version
}

function ipFormat(info: PeerRoutePair) {
  const ip = info.route.ipv4_addr
  if (typeof ip === 'string')
    return ip
  return ip ? ipv4InetToString(ip) : ''
}

function routeCost(info: PeerRoutePair) {
  const cost = info.route.cost
  return cost ? cost === 1 ? 'p2p' : `relay(${cost})` : t('status.local')
}

function oneTunnelProto(tunnel?: TunnelInfo): string {
  if (!tunnel)
    return ''

  const local_addr = tunnel.local_addr
  let isIPv6 = false;
  if (local_addr?.url) {
    try {
      const urlObj = new URL(local_addr.url, 'http://dummy');
      // IPv6 addresses in URLs are enclosed in brackets and contain ':'
      isIPv6 = /^\[.*:.*\]$/.test(urlObj.hostname);
    } catch (e) {
      // fallback to original check if URL parsing fails
      isIPv6 = local_addr.url.indexOf('[') >= 0;
    }
  }
  if (isIPv6)
    return `${tunnel.tunnel_type}6`
  else
    return tunnel.tunnel_type
}

function tunnelProto(conns: any[]) {
  return [...new Set(conns.map(c => oneTunnelProto(c.tunnel)))].join(',')
}

const myNodeInfo = computed(() => {
  return props.curNetworkInst?.detail?.my_node_info
})

interface Chip {
  label: string
  icon: string
}

const udpNatTypeStrMap: Record<number, string> = {
  0: 'Unknown',
  1: 'Open Internet',
  2: 'No PAT',
  3: 'Full Cone',
  4: 'Restricted',
  5: 'Port Restricted',
  6: 'Symmetric',
  7: 'Symmetric UDP Firewall',
  8: 'Symmetric Easy Inc',
  9: 'Symmetric Easy Dec',
}

function natTypeLabel(value: unknown): string {
  return udpNatTypeStrMap[normalizeNatTypeValue(value)] ?? udpNatTypeStrMap[0]
}

const myNodeInfoChips = computed(() => {
  if (!props.curNetworkInst)
    return []

  const chips: Array<Chip> = []
  const my_node_info = props.curNetworkInst.detail?.my_node_info
  if (!my_node_info)
    return chips

  // peer id
  chips.push({
    label: `Peer ID: ${my_node_info.peer_id}`,
    icon: '',
  } as Chip)

  // TUN Device Name
  const dev_name = props.curNetworkInst.detail?.dev_name
  if (dev_name) {
    chips.push({
      label: `TUN Device Name: ${dev_name}`,
      icon: '',
    } as Chip)
  }

  // virtual ipv4
  chips.push({
    label: `Virtual IPv4: ${ipv4InetToString(my_node_info.virtual_ipv4)}`,
    icon: '',
  } as Chip)

  // local ipv4s
  const local_ipv4s = my_node_info.ips?.interface_ipv4s
  for (const [idx, ip] of local_ipv4s?.entries() ?? []) {
    chips.push({
      label: `Local IPv4 ${idx}: ${ipv4ToString(ip)}`,
      icon: '',
    } as Chip)
  }

  // local ipv6s
  const local_ipv6s = my_node_info.ips?.interface_ipv6s
  for (const [idx, ip] of local_ipv6s?.entries() ?? []) {
    chips.push({
      label: `Local IPv6 ${idx}: ${ipv6ToString(ip)}`,
      icon: '',
    } as Chip)
  }

  // public ip
  const public_ip = my_node_info.ips?.public_ipv4
  if (public_ip) {
    chips.push({
      label: `Public IP: ${ipv4ToString(public_ip)}`,
      icon: '',
    } as Chip)
  }

  const public_ipv6 = my_node_info.ips?.public_ipv6
  if (public_ipv6) {
    chips.push({
      label: `Public IPv6: ${ipv6ToString(public_ipv6)}`,
      icon: '',
    } as Chip)
  }

  // listeners:
  const listeners = my_node_info.listeners
  for (const [idx, listener] of listeners?.entries() ?? []) {
    chips.push({
      label: `Listener ${idx}: ${listener.url}`,
      icon: '',
    } as Chip)
  }

  const udpNatType = my_node_info.stun_info?.udp_nat_type
  if (udpNatType !== undefined) {
    chips.push({
      label: `UDP NAT Type: ${natTypeLabel(udpNatType)}`,
      icon: '',
    } as Chip)
  }

  return chips
})

function globalSumCommon(field: string) {
  let sum = 0
  for (const row of peerRows.value) {
    const value = field === 'stats.tx_bytes' ? row.tx_total_bytes : row.rx_total_bytes
    if (value)
      sum += value
  }
  return sum
}

function txGlobalSum() {
  return globalSumCommon('stats.tx_bytes')
}

function rxGlobalSum() {
  return globalSumCommon('stats.rx_bytes')
}

function natType(info: PeerRoutePair): string {
  const udpNatType = info.route?.stun_info?.udp_nat_type;
  if (udpNatType !== undefined)
    return natTypeLabel(udpNatType)

  return ''
}

function routeIsPublicServer(info: PeerRoutePair): boolean {
  return info.route?.feature_flag?.is_public_server === true
}

function routeAvoidRelayData(info: PeerRoutePair): boolean {
  return info.route?.feature_flag?.avoid_relay_data === true
}

function peerRowKey(info: PeerRoutePair, index: number): string {
  if (info.route?.peer_id !== undefined)
    return `peer:${info.route.peer_id}`
  if (typeof info.route?.ipv4_addr === 'string' && info.route.ipv4_addr)
    return `ipv4:${info.route.ipv4_addr}`
  return `row:${index}`
}

const peerRows = computed<PeerRow[]>(() => {
  return peerRouteInfos.value.map((info, index) => {
    const conns = peerConns(info)
    const txTotal = statsCommon(conns, 'stats.tx_bytes') ?? 0
    const rxTotal = statsCommon(conns, 'stats.rx_bytes') ?? 0

    return {
      row_key: peerRowKey(info, index),
      peer_id: info.route?.peer_id ?? 0,
      virtual_ipv4: ipFormat(info),
      hostname: info.route?.hostname ?? '',
      hostname_tooltip: info.route?.hostname ?? '',
      route_cost: routeCost(info),
      tunnel_proto: tunnelProto(conns),
      latency: latencyMs(info),
      tx_bytes: formatByteTotal(txTotal),
      rx_bytes: formatByteTotal(rxTotal),
      tx_total_bytes: txTotal,
      rx_total_bytes: rxTotal,
      loss_rate: lossRate(info),
      nat_type: natType(info),
      version: version(info),
      is_public_server: routeIsPublicServer(info),
      avoid_relay_data: routeAvoidRelayData(info),
    }
  })
})

const peerCount = computed(() => peerRows.value.length)

function entryValue(entry: any, snakeKey: string, camelKey: string) {
  return entry?.[snakeKey] ?? entry?.[camelKey]
}

function entryNumber(entry: any, snakeKey: string, camelKey: string): number {
  const value = entryValue(entry, snakeKey, camelKey)
  if (typeof value === 'number')
    return Number.isFinite(value) ? value : 0
  if (typeof value === 'bigint')
    return Number(value)
  if (typeof value === 'string' && value.trim() !== '') {
    const parsed = Number(value)
    return Number.isFinite(parsed) ? parsed : 0
  }
  return 0
}

function entryBool(entry: any, snakeKey: string, camelKey: string): boolean {
  const value = entryValue(entry, snakeKey, camelKey)
  return value === true
}

const proxyFailoverEntries = computed(() => {
  return (props.curNetworkInst?.detail?.proxy_failover_entries ?? [])
    .map((entry: any) => {
      const generation = entryNumber(entry, 'generation', 'generation')
      const src = proxySocketAddr(entry.src)
      const dst = proxySocketAddr(entry.dst)
      return {
        ...entry,
        ui_key: `${src}->${dst}:${generation}:${entryValue(entry, 'selected_transport', 'selectedTransport') ?? ''}`,
        start_time: entryNumber(entry, 'start_time', 'startTime'),
        generation,
        requested_transport: entryValue(entry, 'requested_transport', 'requestedTransport') ?? '',
        selected_transport: entryValue(entry, 'selected_transport', 'selectedTransport') ?? '',
        fallback_reason: entryValue(entry, 'fallback_reason', 'fallbackReason') ?? '',
        dst_peer_id: entryNumber(entry, 'dst_peer_id', 'dstPeerId'),
        consecutive_failures: entryNumber(entry, 'consecutive_failures', 'consecutiveFailures'),
        consecutive_successes: entryNumber(entry, 'consecutive_successes', 'consecutiveSuccesses'),
        ambiguous_timeout_strikes: entryNumber(entry, 'ambiguous_timeout_strikes', 'ambiguousTimeoutStrikes'),
        transport_degraded: entryBool(entry, 'transport_degraded', 'transportDegraded'),
      }
    })
    .sort((a: any, b: any) => {
      const startDiff = b.start_time - a.start_time
      if (startDiff !== 0)
        return startDiff
      if (b.generation !== a.generation)
        return b.generation - a.generation
      return String(a.ui_key).localeCompare(String(b.ui_key))
    })
    .slice(0, MAX_PROXY_FAILOVER_ROWS)
})

function proxySocketAddr(addr: any): string {
  if (!addr)
    return ''
  if (addr.ip?.oneofKind === 'ipv4')
    return `${ipv4ToString(addr.ip.ipv4)}:${addr.port}`
  if (addr.ip?.oneofKind === 'ipv6')
    return `[${ipv6ToString(addr.ip.ipv6)}]:${addr.port}`
  return ''
}

function proxyHealth(entry: any): string {
  const state = entry.transport_degraded ? t('proxy_failover.degraded') : t('proxy_failover.healthy')
  return `${state} (${entry.consecutive_failures}/${entry.consecutive_successes}, ${t('proxy_failover.ambiguous')}: ${entry.ambiguous_timeout_strikes})`
}

// calculate tx/rx rate every 2 seconds
let rateIntervalId = 0
const rateInterval = 2000
let prevTxSum = 0
let prevRxSum = 0
let prevRateInstanceId: string | undefined
let ratePausedWhileHidden = false
const txRateBytes = ref(0)
const rxRateBytes = ref(0)
const dialogNowMs = ref(Date.now())
let dialogClockId = 0

// 控制节点详细信息chips的显示/隐藏
const showNodeDetails = ref(false)

onMounted(() => {
  prevTxSum = txGlobalSum()
  prevRxSum = rxGlobalSum()
  prevRateInstanceId = props.curNetworkInst?.instance_id

  rateIntervalId = window.setInterval(() => {
    if (document.hidden) {
      ratePausedWhileHidden = true
      return
    }

    const curTxSum = txGlobalSum()
    const curRxSum = rxGlobalSum()
    if (ratePausedWhileHidden) {
      prevTxSum = curTxSum
      prevRxSum = curRxSum
      txRateBytes.value = 0
      rxRateBytes.value = 0
      ratePausedWhileHidden = false
      return
    }

    txRateBytes.value = curTxSum >= prevTxSum ? (curTxSum - prevTxSum) / (rateInterval / 1000) : 0
    prevTxSum = curTxSum

    rxRateBytes.value = curRxSum >= prevRxSum ? (curRxSum - prevRxSum) / (rateInterval / 1000) : 0
    prevRxSum = curRxSum
  }, rateInterval)
})

watch(() => props.curNetworkInst?.instance_id, (instanceId) => {
  if (instanceId === prevRateInstanceId)
    return

  prevRateInstanceId = instanceId
  prevTxSum = txGlobalSum()
  prevRxSum = rxGlobalSum()
  txRateBytes.value = 0
  rxRateBytes.value = 0
})

onUnmounted(() => {
  clearInterval(rateIntervalId)
  if (dialogClockId) {
    clearInterval(dialogClockId)
    dialogClockId = 0
  }
})

const dialogVisible = ref(false)
const dialogContent = ref<any>('')
const dialogHeader = ref('event_log')

function formatTimeAgo(time?: string) {
  if (!time)
    return ''

  const timestamp = Date.parse(time)
  if (!Number.isFinite(timestamp))
    return time

  const diffMs = Math.max(0, dialogNowMs.value - timestamp)
  const diffSec = Math.floor(diffMs / 1000)
  if (diffSec < 60)
    return `${diffSec}s ago`

  const diffMin = Math.floor(diffSec / 60)
  if (diffMin < 60)
    return `${diffMin}m ago`

  const diffHour = Math.floor(diffMin / 60)
  if (diffHour < 24)
    return `${diffHour}h ago`

  const diffDay = Math.floor(diffHour / 24)
  return `${diffDay}d ago`
}

watch(dialogVisible, (visible) => {
  if (visible) {
    dialogNowMs.value = Date.now()
    if (!dialogClockId) {
      dialogClockId = window.setInterval(() => {
        dialogNowMs.value = Date.now()
      }, 60_000)
    }
    return
  }

  if (dialogClockId) {
    clearInterval(dialogClockId)
    dialogClockId = 0
  }

  // Release dialog payloads immediately after close so WebContent does not
  // retain large event arrays or config strings.
  dialogContent.value = ''
  dialogHeader.value = 'event_log'
})

function showVpnPortalConfig() {
  const my_node_info = myNodeInfo.value
  if (!my_node_info)
    return

  const url = 'https://www.wireguardconfig.com/qrcode'
  dialogContent.value = `${my_node_info.vpn_portal_cfg}\n\n # can generate QR code: ${url}`
  dialogHeader.value = 'vpn_portal_config'
  dialogVisible.value = true
}

function showEventLogs() {
  const detail = props.curNetworkInst?.detail
  if (!detail)
    return

  dialogContent.value = detail.events?.map((event: string) => JSON.parse(event)) ?? []
  dialogHeader.value = 'event_log'
  dialogVisible.value = true
}
</script>

<template>
  <div class="frontend-lib">
    <Dialog v-if="dialogVisible" v-model:visible="dialogVisible" modal :header="t(dialogHeader)" class="w-full h-auto max-h-full"
      :baseZIndex="2000">
      <ScrollPanel v-if="dialogHeader === 'vpn_portal_config'">
        <pre>{{ dialogContent }}</pre>
      </ScrollPanel>
      <Timeline v-else :value="dialogContent">
        <template #opposite="slotProps">
          <small class="text-surface-500 dark:text-surface-400">{{ formatTimeAgo(slotProps.item.time) }}</small>
        </template>
        <template #content="slotProps">
          <HumanEvent :event="slotProps.item.event" />
        </template>
      </Timeline>
    </Dialog>

    <Card v-if="curNetworkInst?.error_msg">
      <template #title>
        Run Network Error
      </template>
      <template #content>
        <div class="flex flex-col gap-y-5">
          <div class="text-red-500">
            {{ curNetworkInst.error_msg }}
          </div>
        </div>
      </template>
    </Card>

    <template v-else>
      <Card>
        <template #title>
          {{ t('my_node_info') }}
        </template>
        <template #content>
          <div class="flex w-full flex-col gap-y-5">
            <div class="gap-4">
              <!-- 网络流量图表 -->
              <div class="w-full">
                <NetworkChart :key="curNetworkInst?.instance_id ?? 'default'"
                  :upload-rate-bytes="txRateBytes" :download-rate-bytes="rxRateBytes" />
              </div>
            </div>

            <!-- 展开/收起节点详细信息的divider按钮 -->
            <div class="w-full">
              <Button @click="showNodeDetails = !showNodeDetails"
                :icon="showNodeDetails ? 'pi pi-chevron-up' : 'pi pi-chevron-down'"
                :label="showNodeDetails ? t('hide_node_details') : t('show_node_details')" severity="secondary" outlined
                class="w-full justify-center" size="small" />
            </div>

            <!-- 节点详细信息chips，根据showNodeDetails状态显示/隐藏 -->
            <div v-show="showNodeDetails" class="flex flex-row items-center flex-wrap w-full max-h-40 overflow-scroll">
              <Chip v-for="(chip, i) in myNodeInfoChips" :key="i" :label="chip.label" :icon="chip.icon"
                class="mr-2 mt-2 text-sm" />
            </div>

            <div v-if="myNodeInfo" class="m-0 flex flex-row justify-center gap-x-5 text-sm">
              <Button severity="info" :label="t('show_vpn_portal_config')" @click="showVpnPortalConfig" />
              <Button severity="info" :label="t('show_event_log')" @click="showEventLogs" />
            </div>
          </div>
        </template>
      </Card>

      <Divider />

      <Card>
        <template #title>
          <div class="flex items-center gap-3">
            <div class="flex items-center gap-2">
              <span>{{ t('peer_info') }}</span>
            </div>
            <div class="flex items-center gap-1">
              <Badge :value="peerCount" severity="info"
                class="text-lg font-semibold px-2 py-1 rounded-full bg-blue-100 text-blue-800 dark:bg-blue-900 dark:text-blue-200" />
            </div>
          </div>
        </template>
        <template #content>
          <DataTable :value="peerRows" data-key="row_key" column-resize-mode="fit" table-class="w-full">
            <Column field="virtual_ipv4" :header="t('virtual_ipv4')" />
            <Column :header="t('hostname')">
              <template #body="slotProps">
                <div v-if="!slotProps.data.route_cost || !slotProps.data.is_public_server"
                  v-tooltip="slotProps.data.hostname_tooltip">
                  {{
                    slotProps.data.hostname }}
                </div>
                <div v-else v-tooltip="slotProps.data.hostname_tooltip" class="space-x-1">
                  <Tag v-if="slotProps.data.is_public_server" severity="info" value="Info">
                    {{ t('status.server') }}
                  </Tag>
                  <Tag v-if="slotProps.data.avoid_relay_data" severity="warn" value="Warn">
                    {{ t('status.relay') }}
                  </Tag>
                </div>
              </template>
            </Column>
            <Column field="route_cost" :header="t('route_cost')" />
            <Column field="tunnel_proto" :header="t('tunnel_proto')" />
            <Column field="latency" :header="t('latency')" />
            <Column field="tx_bytes" :header="t('upload_bytes')" />
            <Column field="rx_bytes" :header="t('download_bytes')" />
            <Column field="loss_rate" :header="t('loss_rate')" />
            <Column field="nat_type" :header="t('nat_type')" />
            <Column field="version" :header="t('status.version')" />
          </DataTable>
        </template>
      </Card>

      <Divider />
      <Card>
        <template #title>
          {{ t('proxy_failover.title') }}
        </template>
        <template #content>
          <DataTable v-if="proxyFailoverEntries.length" :value="proxyFailoverEntries" data-key="ui_key"
            column-resize-mode="fit" table-class="w-full">
            <Column :field="(entry: any) => proxySocketAddr(entry.src)" :header="t('proxy_failover.source')" />
            <Column :field="(entry: any) => proxySocketAddr(entry.dst)" :header="t('proxy_failover.destination')" />
            <Column field="requested_transport" :header="t('proxy_failover.requested')" />
            <Column field="selected_transport" :header="t('proxy_failover.selected')" />
            <Column field="fallback_reason" :header="t('proxy_failover.reason')" />
            <Column field="dst_peer_id" :header="t('proxy_failover.peer')" />
            <Column :field="proxyHealth" :header="t('proxy_failover.health')" />
            <Column field="generation" :header="t('proxy_failover.generation')" />
          </DataTable>
          <div v-else class="py-4 text-center text-gray-400">
            {{ t('proxy_failover.empty') }}
          </div>
        </template>
      </Card>
    </template>
  </div>
</template>

<style lang="postcss" scoped>
.p-timeline :deep(.p-timeline-event-opposite) {
  @apply flex-none;
}

:deep(.p-datatable .p-datatable-column-title) {
  white-space: nowrap;
}
</style>
