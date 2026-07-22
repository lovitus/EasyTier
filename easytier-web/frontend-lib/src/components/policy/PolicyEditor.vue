<script setup lang="ts">
import {
  Button,
  Checkbox,
  InputNumber,
  InputText,
  Message,
  Panel,
  Password,
  Select,
  SelectButton,
  Textarea,
} from 'primevue'
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import type * as Api from '../../modules/api'
import type { NetworkConfig } from '../../types/network'
import { canEnablePolicyProxy, policyRuntimeNotice } from './policyRuntimeSupport'
import {
  DEFAULT_POLICY_TEMPLATE,
  POLICY_SHADOWSOCKS_CIPHERS,
  POLICY_VMESS_CIPHERS,
  emptyPolicyDocument,
  parsePolicyDocument,
  policyRuleSupportsNoResolve,
  serializePolicyDocument,
  type PolicyEditorDocument,
  type PolicyGroupRow,
  type PolicyProxyRow,
  type PolicyProxyKind,
  type PolicyRuleRow,
  type PolicyRuleSetRow,
  type PolicyRuleSetKind,
} from './policyDocument'
import {
  MANAGED_RULE_DATA,
  managedRuleDataRows,
  updateManagedRuleData,
} from './managedRuleData'

const config = defineModel<NetworkConfig>({ required: true })
const props = defineProps<{ api?: Api.RemoteClient; yamlOnly?: boolean }>()
const { t } = useI18n()

const document = ref<PolicyEditorDocument>(emptyPolicyDocument())
const ruleDataRows = ref<PolicyRuleSetRow[]>(managedRuleDataRows(document.value))
const parseError = ref('')
const editError = ref('')
const ruleDataMessage = ref('')
const ruleDataError = ref('')
const updatingRuleData = ref<PolicyRuleSetKind | ''>('')
const ruleDataHashes = ref<Partial<Record<PolicyRuleSetKind, string>>>({})
const outboundInfo = ref<Api.ListPolicyOutboundInterfacesResponse>()
const outboundLoading = ref(false)
const outboundError = ref('')
const expandedRowKeys = ref<Set<number>>(new Set())
let lastSerialized = ''
let ruleDataCategoryGeneration = 0
const rowKeys = new WeakMap<object, number>()
let nextRowKey = 1

const sourceOptions = computed(() => [
  { label: t('policy.editor.inline'), value: 'inline' },
  { label: t('policy.editor.file'), value: 'file' },
])
const proxyViaOptions = ['mesh', 'native']
const proxyTypeOptions: PolicyProxyKind[] = ['socks5', 'shadowsocks', 'trojan', 'vmess', 'vless']
const shadowsocksUdpOptions = computed(() => [
  { label: t('policy.editor.udp_off'), value: 'off' },
  { label: t('policy.editor.udp_native'), value: 'native' },
  { label: t('policy.editor.udp_uot_v2'), value: 'uot-v2' },
])
const groupTypeOptions = ['fallback', 'chain']
const geositeCategoryOptions = ref<string[]>([])
const geoipCategoryOptions = ref<string[]>(['LAN'])
const geoipDataCategoryCount = computed(() => Math.max(0, geoipCategoryOptions.value.length - 1))
const outboundOptions = computed(() => (outboundInfo.value?.interfaces ?? []).map(item => ({
  label: item.addresses.length
    ? `${item.name} (${item.addresses.join(', ')})${item.recommended ? ` · ${t('policy.editor.recommended')}` : ''}`
    : `${item.name}${item.recommended ? ` · ${t('policy.editor.recommended')}` : ''}`,
  value: item.name,
})))
const ruleTypeOptions = [
  'GEOSITE', 'GEOIP', 'COUNTRY', 'DOMAIN', 'DOMAIN-SUFFIX', 'DOMAIN-KEYWORD', 'IP-CIDR',
  'NETWORK', 'PORT-RANGE', 'INBOUND-TAG', 'EXTERNAL', 'MATCH', 'FINAL',
]
const actorOptions = computed(() => [
  'DIRECT',
  'REJECT',
  ...document.value.proxies.map(proxy => proxy.name).filter(Boolean),
  ...document.value.groups.map(group => group.name).filter(Boolean),
])
const runtimeNotice = computed(() => outboundInfo.value
  ? policyRuntimeNotice(outboundInfo.value)
  : undefined)
const runtimeNoticeSeverity = computed(() => {
  if (runtimeNotice.value === 'linux-supported' || runtimeNotice.value === 'supported') return 'success'
  if (runtimeNotice.value === 'windows-unsupported' || runtimeNotice.value === 'unsupported') return 'error'
  return 'warn'
})
const runtimeNoticeKey = computed(() => runtimeNotice.value
  ? `policy.editor.runtime_${runtimeNotice.value.replace(/-/g, '_')}`
  : '')

const sourceMode = computed({
  get: () => config.value.policy_config_file?.trim() ? 'file' : 'inline',
  set: (mode: string) => {
    if (mode === 'file') {
      config.value.policy_config_inline = ''
    }
    else {
      config.value.policy_config_file = ''
      ensureInlineDocument()
    }
  },
})

function ensureInlineDocument() {
  if (!config.value.policy_config_inline?.trim()) {
    config.value.policy_config_inline = DEFAULT_POLICY_TEMPLATE
  }
}

function setEnabled(enabled: boolean) {
  if (enabled && !canEnablePolicyProxy(outboundInfo.value)) return
  config.value.enable_policy_proxy = enabled
  if (enabled && sourceMode.value === 'inline') ensureInlineDocument()
}

watch(
  () => config.value.policy_config_inline ?? '',
  source => {
    if (source === lastSerialized) return
    try {
      const parsed = parsePolicyDocument(source)
      document.value = parsed
      ruleDataRows.value = managedRuleDataRows(parsed)
      expandedRowKeys.value = new Set()
      lastSerialized = serializePolicyDocument(parsed)
      parseError.value = ''
      editError.value = ''
    }
    catch (error) {
      parseError.value = error instanceof Error ? error.message : String(error)
    }
  },
  { immediate: true },
)

watch(document, value => {
  if (sourceMode.value !== 'inline' || parseError.value) return
  try {
    const serialized = serializePolicyDocument(value)
    editError.value = ''
    if (serialized === lastSerialized) return
    lastSerialized = serialized
    config.value.policy_config_inline = serialized
  }
  catch (error) {
    editError.value = error instanceof Error ? error.message : String(error)
  }
}, { deep: true })

async function updateRuleData(row: PolicyRuleSetRow) {
  const api = props.api
  if (!api?.update_policy_rule_data || !config.value.instance_id || updatingRuleData.value) return
  updatingRuleData.value = row.type
  ruleDataMessage.value = ''
  ruleDataError.value = ''
  try {
    const result = await updateManagedRuleData(api, config.value.instance_id, row)
    setRuleDataCategories(row.type, result.categories, result.sha256)
    if (row.path.trim() && !document.value.ruleSets.includes(row)) document.value.ruleSets.push(row)
    if (result.updated === false) {
      ruleDataMessage.value = t('policy.editor.rule_data_unchanged', {
        type: row.type,
        size: formatRuleDataSize(result.size),
      })
      return
    }
    ruleDataMessage.value = t('policy.editor.rule_data_updated', {
      type: row.type,
      size: formatRuleDataSize(result.size),
    })
  }
  catch (error) {
    ruleDataError.value = error instanceof Error ? error.message : String(error)
  }
  finally {
    updatingRuleData.value = ''
  }
}

function removeRuleData(row: PolicyRuleSetRow) {
  const index = document.value.ruleSets.indexOf(row)
  if (index >= 0) document.value.ruleSets.splice(index, 1)
  ruleDataRows.value = managedRuleDataRows(document.value)
}

function addProxy() {
  const row: PolicyProxyRow = {
    name: `proxy${document.value.proxies.length + 1}`,
    type: 'socks5',
    via: 'mesh',
    address: '',
    instanceId: '',
    virtualIp: '',
    port: null,
    // A new row is the managed built-in HEV mesh endpoint, whose UDP support is
    // known and validated. Native SOCKS users can explicitly clear this visible
    // capability flag when their independently managed server is TCP-only.
    udp: true,
    username: '',
    password: '',
    cipher: '',
    uuid: '',
    alterId: 0,
    transport: 'tcp',
    wsPath: '/',
    wsHeaders: {},
    tlsEnabled: false,
    tlsServerName: '',
    tlsInsecure: false,
  }
  document.value.proxies.push(row)
  expandRow(row)
}

function updateProxyType(row: PolicyProxyRow, value: PolicyProxyKind) {
  row.type = value
  if (value === 'shadowsocks') {
    row.via = 'native'
    row.instanceId = ''
    row.virtualIp = ''
    row.username = ''
    row.cipher ||= 'aes-256-gcm'
    row.udp = typeof row.udp === 'boolean' ? (row.udp ? 'native' : 'off') : row.udp
    row.uuid = ''
    row.alterId = 0
    row.transport = 'tcp'
    row.wsPath = '/'
    row.wsHeaders = {}
    row.tlsEnabled = false
    row.tlsServerName = ''
    row.tlsInsecure = false
    return
  }
  if (['trojan', 'vmess', 'vless'].includes(value)) {
    row.via = 'native'
    row.instanceId = ''
    row.virtualIp = ''
    row.username = ''
    row.udp = typeof row.udp === 'string' ? row.udp === 'native' : row.udp
    row.transport ||= 'tcp'
    row.wsPath ||= '/'
    row.wsHeaders ||= {}
    row.tlsEnabled = value === 'trojan' || Boolean(row.tlsEnabled)
    row.alterId = 0
    row.cipher = value === 'vmess' ? (row.cipher || 'auto') : ''
    if (value === 'trojan') row.uuid = ''
    if (value !== 'trojan') row.password = ''
    return
  }
  row.cipher = ''
  row.uuid = ''
  row.transport = 'tcp'
  row.wsPath = '/'
  row.wsHeaders = {}
  row.tlsEnabled = false
  row.tlsServerName = ''
  row.tlsInsecure = false
  row.udp = typeof row.udp === 'string' ? row.udp === 'native' : row.udp
}

function proxyViaOptionsFor(row: PolicyProxyRow) {
  return row.type === 'socks5' ? proxyViaOptions : ['native']
}

function proxyCipherOptions(row: PolicyProxyRow) {
  return row.type === 'vmess' ? [...POLICY_VMESS_CIPHERS] : [...POLICY_SHADOWSOCKS_CIPHERS]
}

function updateWebSocketHost(row: PolicyProxyRow, value: string) {
  row.wsHeaders ||= {}
  if (value.trim()) row.wsHeaders.Host = value.trim()
  else delete row.wsHeaders.Host
}

function addGroup() {
  const row: PolicyGroupRow = {
    name: `fallback${document.value.groups.length + 1}`,
    type: 'fallback',
    members: [],
  }
  document.value.groups.push(row)
  expandRow(row)
}

function addRule() {
  const row: PolicyRuleRow = { type: 'GEOSITE', operand: 'CN', target: 'DIRECT', noResolve: false }
  const finalIndex = document.value.rules.findIndex(rule => ['MATCH', 'FINAL'].includes(rule.type))
  if (finalIndex < 0) document.value.rules.push(row)
  else document.value.rules.splice(finalIndex, 0, row)
  expandRow(row)
}

function preferredTarget() {
  return document.value.groups[0]?.name || document.value.proxies[0]?.name || 'DIRECT'
}

function applyPreset(preset: 'china-direct' | 'global' | 'direct') {
  const target = preferredTarget()
  if (preset === 'china-direct') {
    document.value.rules = [
      { type: 'GEOSITE', operand: 'CN', target: 'DIRECT', noResolve: false },
      { type: 'GEOIP', operand: 'CN', target: 'DIRECT', noResolve: true },
      { type: 'MATCH', operand: '', target, noResolve: false },
    ]
  }
  else if (preset === 'global') {
    document.value.rules = [{ type: 'MATCH', operand: '', target, noResolve: false }]
  }
  else {
    document.value.rules = [{ type: 'MATCH', operand: '', target: 'DIRECT', noResolve: false }]
  }
  expandedRowKeys.value = new Set()
}

function removeAt<T extends object>(rows: T[], index: number) {
  const [removed] = rows.splice(index, 1)
  if (!removed) return
  const next = new Set(expandedRowKeys.value)
  next.delete(rowKey(removed))
  expandedRowKeys.value = next
}

function rowKey(row: object) {
  const existing = rowKeys.get(row)
  if (existing !== undefined) return existing
  const key = nextRowKey++
  rowKeys.set(row, key)
  return key
}

function isRowExpanded(row: object) {
  return expandedRowKeys.value.has(rowKey(row))
}

function expandRow(row: object) {
  const next = new Set(expandedRowKeys.value)
  next.add(rowKey(row))
  expandedRowKeys.value = next
}

function toggleRow(row: object) {
  const key = rowKey(row)
  const next = new Set(expandedRowKeys.value)
  if (next.has(key)) next.delete(key)
  else next.add(key)
  expandedRowKeys.value = next
}

function moveRule(index: number, offset: number) {
  const destination = index + offset
  if (destination < 0 || destination >= document.value.rules.length) return
  const [row] = document.value.rules.splice(index, 1)
  document.value.rules.splice(destination, 0, row)
}

function updateMembers(row: PolicyGroupRow, value: string) {
  row.members = value.split(',').map(member => member.trim()).filter(Boolean)
}

function updateDnsServers(set: 'direct' | 'proxy', value: string) {
  document.value.dns[set] = value
    .split(/[\n,]/)
    .map(server => server.trim())
    .filter(Boolean)
}

function ruleNeedsOperand(type: string) {
  return !['MATCH', 'FINAL'].includes(type.toUpperCase())
}

function updateRuleType(row: PolicyRuleRow, value: string) {
  const previouslySupported = policyRuleSupportsNoResolve(row.type, row.operand)
  row.type = value
  const supported = policyRuleSupportsNoResolve(value, row.operand)
  if (!supported) row.noResolve = false
  else if (!previouslySupported) row.noResolve = true
}

function updateRuleOperand(row: PolicyRuleRow, value: string) {
  row.operand = value
  if (!policyRuleSupportsNoResolve(row.type, value)) row.noResolve = false
}

function fieldLabel(key: string, value: string) {
  return `${t(key)} · ${t('policy.editor.example', { value })}`
}

function proxyProtocolLabel(row: PolicyProxyRow) {
  const names: Record<PolicyProxyKind, string> = {
    socks5: 'SOCKS5',
    shadowsocks: 'Shadowsocks',
    trojan: 'Trojan',
    vmess: 'VMess',
    vless: 'VLESS',
  }
  return names[row.type]
}

function proxyEndpointSummary(row: PolicyProxyRow) {
  const server = row.via === 'mesh'
    ? row.virtualIp || row.instanceId || t('policy.editor.not_configured')
    : row.address || t('policy.editor.not_configured')
  const port = row.port ?? (row.via === 'mesh' ? t('policy.editor.automatic') : t('policy.editor.not_configured'))
  return `${server} · ${port}`
}

function proxyUdpSummary(row: PolicyProxyRow) {
  if (row.type === 'shadowsocks') {
    const mode = typeof row.udp === 'boolean' ? (row.udp ? 'native' : 'off') : row.udp
    return t(`policy.editor.udp_${mode.replace('-', '_')}`)
  }
  const enabled = typeof row.udp === 'boolean' ? row.udp : row.udp === 'native'
  return enabled ? t('policy.editor.enabled') : t('policy.editor.disabled')
}

function ruleOperandExample(type: string) {
  const examples: Record<string, string> = {
    GEOSITE: 'GITHUB',
    GEOIP: 'CN',
    COUNTRY: 'US',
    DOMAIN: 'www.example.com',
    'DOMAIN-SUFFIX': 'example.com',
    'DOMAIN-KEYWORD': 'google',
    'IP-CIDR': '192.0.2.0/24',
    NETWORK: 'udp',
    'PORT-RANGE': '443 / 10000-20000',
    'INBOUND-TAG': 'tun',
    EXTERNAL: 'geoip:google',
  }
  return examples[type.trim().toUpperCase()] ?? 'value'
}

function ruleCategoryOptions(type: string) {
  const normalized = type.trim().toUpperCase()
  if (normalized === 'GEOSITE') return geositeCategoryOptions.value
  if (normalized === 'GEOIP') return geoipCategoryOptions.value
  return []
}

function formatRuleDataSize(value: number | string) {
  const size = Number(value)
  return Number.isFinite(size) ? `${(size / 1024 / 1024).toFixed(1)} MiB` : String(value)
}

function setRuleDataCategories(resource: string, categories?: string[], sha256?: string) {
  const normalized = [...new Set((categories ?? []).map(category => category.trim().toUpperCase()).filter(Boolean))]
    .sort((left, right) => left.localeCompare(right))
  if (resource === 'geosite') geositeCategoryOptions.value = normalized
  if (resource === 'geoip') geoipCategoryOptions.value = ['LAN', ...normalized.filter(category => category !== 'LAN')]
  if (resource in MANAGED_RULE_DATA && sha256?.trim()) {
    ruleDataHashes.value[resource as PolicyRuleSetKind] = sha256.trim()
  }
}

async function loadRuleDataCategories() {
  const generation = ++ruleDataCategoryGeneration
  geositeCategoryOptions.value = []
  geoipCategoryOptions.value = ['LAN']
  const api = props.api
  const instanceId = config.value.instance_id
  const listRuleDataCategories = api?.list_policy_rule_data_categories?.bind(api)
  if (!listRuleDataCategories || !instanceId) return
  const rows = managedRuleDataRows(document.value)
  await Promise.all(['geosite', 'geoip'].map(async resource => {
    const row = rows.find(candidate => candidate.type === resource)
    try {
      const result = await listRuleDataCategories(
        instanceId,
        resource as Api.PolicyRuleDataResource,
        row?.sha256.trim() || undefined,
        row?.path.trim() || undefined,
      )
      if (generation !== ruleDataCategoryGeneration) return
      setRuleDataCategories(resource, result.categories, result.sha256)
    }
    catch {
      // Category discovery is an editor convenience. Typed rules remain usable
      // when an older core or an unreadable custom index cannot provide it.
    }
  }))
}

function ruleSummary(row: PolicyRuleRow) {
  const operand = row.operand.trim() ? ` · ${row.operand.trim()}` : ''
  const modifier = row.noResolve ? ' · no-resolve' : ''
  return `${row.type}${operand} -> ${row.target}${modifier}`
}

function managedRuleDataSource(type: PolicyRuleSetKind) {
  return MANAGED_RULE_DATA[type].source
}

function ruleDataSource(row: PolicyRuleSetRow) {
  return row.sourceUrl.trim() || managedRuleDataSource(row.type)
}

function setRuleDataSource(row: PolicyRuleSetRow, value: string) {
  const source = value.trim()
  row.sourceUrl = source === managedRuleDataSource(row.type) ? '' : source
}

function isManagedRuleDataInstalled(row: PolicyRuleSetRow) {
  return Boolean(row.path.trim())
}

function usesBundledRuleData(row: PolicyRuleSetRow) {
  return MANAGED_RULE_DATA[row.type].builtin && !isManagedRuleDataInstalled(row)
}

function managedRuleDataStatus(row: PolicyRuleSetRow) {
  if (usesBundledRuleData(row)) return t('policy.editor.builtin')
  if (!row.path.trim()) return t('policy.editor.not_installed')
  return row.sha256.trim()
    ? t('policy.editor.installed')
    : t('policy.editor.installed_unverified')
}

function managedRuleDataHash(row: PolicyRuleSetRow) {
  return row.sha256.trim() || ruleDataHashes.value[row.type] || ''
}

async function loadOutboundInterfaces() {
  if (!props.api?.list_policy_outbound_interfaces || outboundLoading.value) return
  outboundLoading.value = true
  outboundError.value = ''
  try {
    const result = await props.api.list_policy_outbound_interfaces()
    outboundInfo.value = result
    if (result.required && !config.value.policy_outbound_interface?.trim()) {
      const recommended = result.interfaces.find(item => item.recommended)
      if (recommended) config.value.policy_outbound_interface = recommended.name
    }
  }
  catch (error) {
    outboundError.value = error instanceof Error ? error.message : String(error)
  }
  finally {
    outboundLoading.value = false
  }
}

watch(() => config.value.enable_policy_proxy, enabled => {
  if (enabled && !props.yamlOnly) void loadOutboundInterfaces()
})

const ruleDataCategoryIdentity = computed(() => {
  const rows = managedRuleDataRows(document.value)
    return ['geosite', 'geoip'].map(resource => {
      const row = rows.find(candidate => candidate.type === resource)
      return [resource, row?.path.trim() ?? '', row?.sha256.trim() ?? ''].join('\u0000')
    }).join('\u0001') + `\u0001${config.value.instance_id ?? ''}`
  })

watch(ruleDataCategoryIdentity, () => {
  if (!props.yamlOnly) void loadRuleDataCategories()
}, { immediate: true })

onMounted(() => {
  if (!props.yamlOnly) void loadOutboundInterfaces()
})
</script>

<template>
  <div class="flex flex-col gap-4">
    <template v-if="props.yamlOnly">
      <div class="flex flex-wrap items-end gap-4">
        <div class="flex flex-col gap-2">
          <label class="font-semibold">{{ t('policy.editor.source') }}</label>
          <SelectButton v-model="sourceMode" :options="sourceOptions" option-label="label" option-value="value"
            :allow-empty="false" />
        </div>
      </div>
      <template v-if="sourceMode === 'file'">
        <div class="flex items-center">
          <label for="policy_config_file_quick">{{ fieldLabel('policy_config_file', '/etc/easytier/policy.yaml') }}</label>
          <span class="pi pi-question-circle ml-2" v-tooltip="t('policy_config_file_help')" />
        </div>
        <InputText id="policy_config_file_quick" v-model="config.policy_config_file"
          :placeholder="t('policy_config_file_placeholder')" />
        <Message severity="info" :closable="false">{{ t('policy.editor.file_notice') }}</Message>
      </template>
      <template v-else>
        <Message v-if="parseError" severity="error" :closable="false">
          {{ t('policy.editor.yaml_error') }}: {{ parseError }}
        </Message>
        <label for="policy_config_inline_quick" class="font-semibold">{{ t('policy.editor.advanced_yaml') }}</label>
        <Textarea id="policy_config_inline_quick" v-model="config.policy_config_inline" rows="20" auto-resize
          class="w-full font-mono" :placeholder="t('policy_config_inline_placeholder')" />
      </template>
    </template>
    <template v-else>
    <Message v-if="outboundInfo && runtimeNoticeKey" :severity="runtimeNoticeSeverity" :closable="false">
      {{ t(runtimeNoticeKey, { platform: outboundInfo.platform }) }}
    </Message>
    <Message v-else-if="outboundError" severity="warn" :closable="false">
      {{ t('policy.editor.outbound_load_failed') }}: {{ outboundError }}
    </Message>

    <div class="flex items-center gap-3">
      <Checkbox input-id="enable_policy_proxy" :model-value="config.enable_policy_proxy"
        :disabled="!config.enable_policy_proxy && outboundInfo?.supported === false" binary
        @update:model-value="setEnabled(Boolean($event))" />
      <label for="enable_policy_proxy" class="font-semibold">{{ t('enable_policy_proxy') }}</label>
      <span class="pi pi-question-circle" v-tooltip="t('enable_policy_proxy_help')" />
    </div>

    <template v-if="config.enable_policy_proxy">
      <div v-if="outboundInfo?.platform === 'linux'" class="flex items-center gap-3">
        <Checkbox input-id="policy_leaf_tun_fast_path" v-model="config.policy_leaf_tun_fast_path" binary />
        <label for="policy_leaf_tun_fast_path" class="font-semibold">{{ t('policy_leaf_tun_fast_path') }}</label>
        <span class="pi pi-question-circle" v-tooltip="t('policy_leaf_tun_fast_path_help')" />
      </div>
      <div class="flex flex-wrap items-end gap-4">
        <div class="flex flex-col gap-2">
          <label class="font-semibold">{{ t('policy.editor.source') }}</label>
          <SelectButton v-model="sourceMode" :options="sourceOptions" option-label="label" option-value="value"
            :allow-empty="false" />
        </div>
        <div v-if="outboundInfo?.required" class="flex flex-col gap-2 grow min-w-64">
          <label for="policy_outbound_interface">{{ t('policy_outbound_interface') }}</label>
          <div class="flex gap-2">
            <Select id="policy_outbound_interface" v-model="config.policy_outbound_interface"
              :options="outboundOptions" option-label="label" option-value="value"
              :placeholder="t('policy_outbound_interface_placeholder')" class="grow" />
            <Button icon="pi pi-refresh" severity="secondary" outlined :loading="outboundLoading"
              :aria-label="t('policy.editor.refresh_interfaces')" @click="loadOutboundInterfaces" />
          </div>
          <small class="text-surface-500">{{ t('policy.editor.outbound_required', { platform: outboundInfo.platform }) }}</small>
        </div>
        <Message v-else-if="outboundInfo?.supported" severity="info" :closable="false">
          {{ t('policy.editor.outbound_automatic', { platform: outboundInfo.platform }) }}
        </Message>
        <Message v-else-if="outboundInfo" severity="warn" :closable="false">
          {{ t('policy.editor.outbound_unavailable', { platform: outboundInfo.platform }) }}
        </Message>
        <div v-if="outboundInfo?.required" class="flex flex-col gap-2 grow min-w-64">
          <label for="policy_leaf_executable">{{ fieldLabel('policy_leaf_executable', 'easytier-leaf-worker') }}</label>
          <InputText id="policy_leaf_executable" v-model="config.policy_leaf_executable"
            placeholder="easytier-leaf-worker" />
        </div>
      </div>

      <template v-if="sourceMode === 'file'">
        <div class="flex items-center">
          <label for="policy_config_file">{{ fieldLabel('policy_config_file', '/etc/easytier/policy.yaml') }}</label>
          <span class="pi pi-question-circle ml-2" v-tooltip="t('policy_config_file_help')" />
        </div>
        <InputText id="policy_config_file" v-model="config.policy_config_file"
          :placeholder="t('policy_config_file_placeholder')" />
        <Message severity="info" :closable="false">{{ t('policy.editor.file_notice') }}</Message>
      </template>

      <template v-else>
        <Message v-if="parseError" severity="error" :closable="false">
          {{ t('policy.editor.yaml_error') }}: {{ parseError }}
        </Message>
        <Message v-if="editError" severity="error" :closable="false">
          {{ t('policy.editor.edit_error') }}: {{ editError }}
        </Message>

        <template v-if="!parseError">
          <Panel :header="t('policy.editor.dns')" toggleable>
            <div class="flex flex-col gap-4">
              <Message severity="info" :closable="false">{{ t('policy.editor.dns_isolation_help') }}</Message>
              <div class="grid gap-4 lg:grid-cols-2">
                <div class="flex flex-col gap-2 rounded-xl border border-surface-200 bg-surface-50 p-4 dark:border-surface-700 dark:bg-surface-900">
                  <span class="text-xs font-semibold uppercase tracking-wide text-surface-500">DIRECT</span>
                <label for="policy_dns_direct" class="font-semibold">{{ fieldLabel('policy.editor.dns_direct', 'system / 223.5.5.5') }}</label>
                <Textarea id="policy_dns_direct" :model-value="document.dns.direct.join('\n')" rows="4"
                  auto-resize class="w-full" :placeholder="t('policy.editor.dns_direct_placeholder')"
                  @update:model-value="updateDnsServers('direct', String($event))" />
                <small class="text-surface-500">{{ t('policy.editor.dns_direct_help') }}</small>
              </div>
                <div class="flex flex-col gap-2 rounded-xl border border-surface-200 bg-surface-50 p-4 dark:border-surface-700 dark:bg-surface-900">
                  <span class="text-xs font-semibold uppercase tracking-wide text-surface-500">PROXY</span>
                <label for="policy_dns_proxy" class="font-semibold">{{ fieldLabel('policy.editor.dns_proxy', 'doh:cloudflare-dns.com@1.1.1.1') }}</label>
                <Textarea id="policy_dns_proxy" :model-value="document.dns.proxy.join('\n')" rows="4"
                  auto-resize class="w-full" placeholder="doh:cloudflare-dns.com@1.1.1.1"
                  @update:model-value="updateDnsServers('proxy', String($event))" />
                <small class="text-surface-500">{{ t('policy.editor.dns_proxy_help') }}</small>
                </div>
              </div>
              <div class="grid gap-4 lg:grid-cols-2">
                <div class="flex flex-col gap-2">
                  <label for="policy_fake_ip_range" class="font-semibold">{{ fieldLabel('policy.editor.fake_ip_range', '198.19.0.0/16') }}</label>
                  <InputText id="policy_fake_ip_range" v-model="document.dns.fakeIpRange" class="w-full" />
                </div>
                <div class="flex flex-col gap-2">
                  <label for="policy_fake_ip_range6" class="font-semibold">{{ fieldLabel('policy.editor.fake_ip_range6', 'fd65:6173:7974::/64') }}</label>
                  <InputText id="policy_fake_ip_range6" v-model="document.dns.fakeIpRange6" class="w-full" />
                </div>
              </div>
            </div>
          </Panel>

          <Panel :header="t('policy.editor.nodes')" toggleable>
            <div class="flex flex-col gap-3">
              <Message v-if="document.proxies.length === 0" severity="secondary" :closable="false">
                {{ t('policy.editor.nodes_empty') }}
              </Message>
              <article v-for="(data, index) in document.proxies" :key="rowKey(data)"
                class="flex flex-col gap-4 rounded-xl border border-surface-200 p-4 dark:border-surface-700">
                <div class="flex items-center justify-between gap-3">
                  <div class="min-w-0">
                    <div class="flex items-center gap-2">
                      <span class="rounded-full bg-primary-100 px-2 py-1 text-xs font-semibold text-primary-700 dark:bg-primary-900 dark:text-primary-200">#{{ index + 1 }}</span>
                      <strong class="truncate">{{ data.name || t('policy.editor.unnamed_node') }}</strong>
                    </div>
                    <div class="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs text-surface-500">
                      <span>{{ proxyProtocolLabel(data) }}</span>
                      <span>{{ data.via }}</span>
                      <span class="break-all">{{ proxyEndpointSummary(data) }}</span>
                      <span>UDP: {{ proxyUdpSummary(data) }}</span>
                    </div>
                  </div>
                  <div class="flex shrink-0 gap-1">
                    <Button :icon="isRowExpanded(data) ? 'pi pi-check' : 'pi pi-pencil'" severity="secondary" text
                      :label="isRowExpanded(data) ? t('policy.editor.finish_edit') : t('policy.editor.edit')"
                      :aria-expanded="isRowExpanded(data)" :data-testid="`policy-proxy-edit-${index}`"
                      @click="toggleRow(data)" />
                    <Button icon="pi pi-trash" severity="danger" text :aria-label="t('policy.editor.remove_node')"
                      @click="removeAt(document.proxies, index)" />
                  </div>
                </div>
                <div v-if="isRowExpanded(data)" class="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
                  <div class="flex flex-col gap-2">
                    <label :for="`policy_proxy_name_${index}`" class="font-semibold">{{ fieldLabel('policy.editor.name', 'kr-exit') }}</label>
                    <InputText :id="`policy_proxy_name_${index}`" v-model="data.name" class="w-full"
                      :data-testid="`policy-proxy-name-${index}`" />
                  </div>
                  <div class="flex flex-col gap-2">
                    <label class="font-semibold">{{ t('policy.editor.proxy_type') }}</label>
                    <Select :model-value="data.type" :options="proxyTypeOptions" class="w-full"
                      :data-testid="`policy-proxy-type-${index}`"
                      @update:model-value="updateProxyType(data, $event as PolicyProxyKind)" />
                  </div>
                  <div class="flex flex-col gap-2">
                    <label class="font-semibold">{{ t('policy.editor.path') }}</label>
                    <Select v-model="data.via" :options="proxyViaOptionsFor(data)" class="w-full" />
                  </div>
                  <div class="flex flex-col gap-2 sm:col-span-2">
                    <label class="font-semibold">{{ fieldLabel('policy.editor.server', data.via === 'mesh' ? '10.44.0.8 / UUID' : 'proxy.example.com') }}</label>
                    <div v-if="data.via === 'mesh'" class="grid gap-2 sm:grid-cols-2">
                      <InputText v-model="data.instanceId" class="w-full" :placeholder="t('policy.editor.instance_id')" />
                      <InputText v-model="data.virtualIp" class="w-full" :placeholder="t('policy.editor.virtual_ip')" />
                    </div>
                    <InputText v-else v-model="data.address" placeholder="host / IP" class="w-full" />
                  </div>
                  <div class="flex flex-col gap-2">
                    <label class="font-semibold">{{ fieldLabel('policy.editor.port', data.via === 'mesh' ? 'auto / 1080' : '443') }}</label>
                    <InputNumber
                      v-model="data.port"
                      :min="1"
                      :max="65535"
                      :use-grouping="false"
                      :placeholder="data.via === 'mesh' ? 'auto' : undefined"
                      class="w-full"
                    />
                  </div>
                  <div class="flex flex-col gap-2">
                    <span class="font-semibold">{{ t('policy.editor.udp') }}</span>
                    <Select v-if="data.type === 'shadowsocks'" v-model="data.udp"
                      :options="shadowsocksUdpOptions" option-label="label" option-value="value" class="w-full"
                      :data-testid="`policy-proxy-udp-mode-${index}`" />
                    <div v-else class="flex min-h-10 items-center gap-2">
                      <Checkbox :input-id="`policy_proxy_udp_${index}`" v-model="data.udp" binary />
                      <label :for="`policy_proxy_udp_${index}`">{{ t('policy.editor.udp_capable', { protocol: proxyProtocolLabel(data) }) }}</label>
                    </div>
                  </div>
                  <div v-if="['shadowsocks', 'vmess'].includes(data.type)" class="flex flex-col gap-2">
                    <label class="font-semibold">{{ t('policy.editor.cipher') }}</label>
                    <Select v-model="data.cipher" :options="proxyCipherOptions(data)" class="w-full" />
                  </div>
                  <div v-if="['socks5', 'shadowsocks', 'trojan'].includes(data.type)"
                    class="flex flex-col gap-2" :class="['shadowsocks', 'trojan'].includes(data.type) ? '' : 'sm:col-span-2'">
                    <label class="font-semibold">{{ fieldLabel('policy.editor.credentials', data.type === 'socks5' ? 'alice / secret' : 'secret') }}</label>
                    <div v-if="data.type === 'socks5'" class="grid gap-2 sm:grid-cols-2">
                      <InputText v-model="data.username" class="w-full" :placeholder="t('username')" />
                      <Password v-model="data.password" class="w-full" :placeholder="t('password')" :feedback="false" toggle-mask />
                    </div>
                    <Password v-else v-model="data.password" class="w-full" :placeholder="t('password')" :feedback="false" toggle-mask />
                  </div>
                  <div v-if="['vmess', 'vless'].includes(data.type)" class="flex flex-col gap-2 sm:col-span-2">
                    <label class="font-semibold">{{ fieldLabel('policy.editor.uuid', '00000000-0000-0000-0000-000000000000') }}</label>
                    <InputText v-model="data.uuid" class="w-full" placeholder="00000000-0000-0000-0000-000000000000" />
                  </div>
                  <template v-if="['trojan', 'vmess', 'vless'].includes(data.type)">
                    <div class="flex flex-col gap-2">
                      <label class="font-semibold">{{ t('policy.editor.transport') }}</label>
                      <Select v-model="data.transport" :options="['tcp', 'websocket']" class="w-full" />
                    </div>
                    <div v-if="data.transport === 'websocket'" class="flex flex-col gap-2">
                      <label class="font-semibold">{{ fieldLabel('policy.editor.websocket_path', '/vless') }}</label>
                      <InputText v-model="data.wsPath" class="w-full" placeholder="/" />
                    </div>
                    <div v-if="data.transport === 'websocket'" class="flex flex-col gap-2 sm:col-span-2">
                      <label class="font-semibold">{{ fieldLabel('policy.editor.websocket_host', 'cdn.example.com') }}</label>
                      <InputText :model-value="data.wsHeaders?.Host ?? ''" class="w-full"
                        @update:model-value="updateWebSocketHost(data, String($event))" />
                    </div>
                    <div class="flex min-h-10 items-center gap-2">
                      <Checkbox :input-id="`policy_proxy_tls_${index}`" v-model="data.tlsEnabled"
                        :disabled="data.type === 'trojan'" binary />
                      <label :for="`policy_proxy_tls_${index}`">{{ t('policy.editor.tls') }}</label>
                    </div>
                    <div v-if="data.tlsEnabled" class="flex flex-col gap-2">
                      <label class="font-semibold">{{ fieldLabel('policy.editor.tls_server_name', 'cdn.example.com') }}</label>
                      <InputText v-model="data.tlsServerName" class="w-full" placeholder="cdn.example.com" />
                    </div>
                    <div v-if="data.tlsEnabled" class="flex min-h-10 items-center gap-2">
                      <Checkbox :input-id="`policy_proxy_tls_insecure_${index}`" v-model="data.tlsInsecure" binary />
                      <label :for="`policy_proxy_tls_insecure_${index}`">{{ t('policy.editor.tls_insecure') }}</label>
                    </div>
                  </template>
                </div>
              </article>
              <Button icon="pi pi-plus" :label="t('policy.editor.add_node')" class="self-start" @click="addProxy" />
            </div>
          </Panel>

          <Panel :header="t('policy.editor.groups')" toggleable>
            <div class="flex flex-col gap-3">
              <article v-for="(data, index) in document.groups" :key="rowKey(data)"
                class="flex flex-col gap-4 rounded-xl border border-surface-200 p-4 dark:border-surface-700">
                <div class="flex items-center justify-between gap-3">
                  <div class="min-w-0">
                    <strong class="truncate">{{ data.name || t('policy.editor.unnamed_group') }}</strong>
                    <div class="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs text-surface-500">
                      <span>{{ data.type }}</span>
                      <span class="break-all">{{ data.members.join(' -> ') || t('policy.editor.no_members') }}</span>
                    </div>
                  </div>
                  <div class="flex shrink-0 gap-1">
                    <Button :icon="isRowExpanded(data) ? 'pi pi-check' : 'pi pi-pencil'" severity="secondary" text
                      :label="isRowExpanded(data) ? t('policy.editor.finish_edit') : t('policy.editor.edit')"
                      :aria-expanded="isRowExpanded(data)" :data-testid="`policy-group-edit-${index}`"
                      @click="toggleRow(data)" />
                    <Button icon="pi pi-trash" severity="danger" text
                      :aria-label="t('policy.editor.remove_group')" @click="removeAt(document.groups, index)" />
                  </div>
                </div>
                <div v-if="isRowExpanded(data)"
                  class="grid gap-4 sm:grid-cols-2 lg:grid-cols-[minmax(10rem,1fr)_12rem_minmax(16rem,2fr)]">
                  <div class="flex flex-col gap-2">
                    <label class="font-semibold">{{ fieldLabel('policy.editor.name', 'overseas-exit') }}</label>
                    <InputText v-model="data.name" class="w-full" :data-testid="`policy-group-name-${index}`" />
                  </div>
                  <div class="flex flex-col gap-2">
                    <label class="font-semibold">{{ t('policy.editor.group_type') }}</label>
                    <Select v-model="data.type" :options="groupTypeOptions" class="w-full" />
                  </div>
                  <div class="flex flex-col gap-2 sm:col-span-2 lg:col-span-1">
                    <label class="font-semibold">{{ fieldLabel('policy.editor.members', 'mesh-hop,native-vless,DIRECT') }}</label>
                    <InputText :model-value="data.members.join(',')" class="w-full"
                        :placeholder="t('policy.editor.members_hint')"
                        @update:model-value="updateMembers(data, String($event))" />
                  </div>
                  <div v-if="data.type === 'fallback'" class="flex flex-col gap-2 sm:col-span-2 lg:col-span-3">
                    <label class="font-semibold">Health-check URL</label>
                    <InputText v-model="data.url" class="w-full"
                        placeholder="https://www.gstatic.com/generate_204" />
                  </div>
                </div>
              </article>
              <Button icon="pi pi-plus" :label="t('policy.editor.add_group')" class="self-start" @click="addGroup" />
            </div>
          </Panel>

          <Panel :header="t('policy.editor.rules')" toggleable>
            <div class="flex flex-col gap-3">
              <Message severity="info" :closable="false">{{ t('policy.editor.order_help') }}</Message>
              <small class="text-surface-500">{{ t('policy.editor.geo_category_help', {
                geositeCount: geositeCategoryOptions.length,
                geoipCount: geoipDataCategoryCount,
              }) }}</small>
              <div class="flex flex-wrap items-center gap-2">
                <span class="font-semibold">{{ t('policy.editor.presets') }}</span>
                <Button :label="t('policy.editor.preset_china_direct')" severity="secondary" outlined
                  @click="applyPreset('china-direct')" />
                <Button :label="t('policy.editor.preset_global')" severity="secondary" outlined
                  @click="applyPreset('global')" />
                <Button :label="t('policy.editor.preset_direct')" severity="secondary" outlined
                  @click="applyPreset('direct')" />
              </div>
              <article v-for="(data, index) in document.rules" :key="rowKey(data)"
                class="flex flex-col gap-4 rounded-xl border border-surface-200 p-4 dark:border-surface-700">
                <div class="flex items-center justify-between gap-3">
                  <div class="min-w-0">
                    <span class="text-sm font-semibold text-surface-500">{{ t('policy.editor.rule_priority', { index: index + 1 }) }}</span>
                    <div class="mt-1 break-all text-xs text-surface-500">{{ ruleSummary(data) }}</div>
                  </div>
                  <div class="flex gap-1">
                    <Button :icon="isRowExpanded(data) ? 'pi pi-check' : 'pi pi-pencil'" severity="secondary" text
                      :label="isRowExpanded(data) ? t('policy.editor.finish_edit') : t('policy.editor.edit')"
                      :aria-expanded="isRowExpanded(data)" :data-testid="`policy-rule-edit-${index}`"
                      @click="toggleRow(data)" />
                    <Button icon="pi pi-arrow-up" severity="secondary" text :disabled="index === 0"
                      :aria-label="t('policy.editor.move_up')" @click="moveRule(index, -1)" />
                    <Button icon="pi pi-arrow-down" severity="secondary" text :disabled="index === document.rules.length - 1"
                      :aria-label="t('policy.editor.move_down')" @click="moveRule(index, 1)" />
                    <Button icon="pi pi-trash" severity="danger" text :aria-label="t('policy.editor.remove_rule')"
                      @click="removeAt(document.rules, index)" />
                  </div>
                </div>
                <div v-if="isRowExpanded(data)"
                  class="grid gap-4 sm:grid-cols-2 xl:grid-cols-[minmax(10rem,1fr)_minmax(12rem,2fr)_minmax(10rem,1fr)_9rem]">
                  <div class="flex flex-col gap-2">
                    <label class="font-semibold">{{ t('policy.editor.rule_type') }}</label>
                    <Select :model-value="data.type" :options="ruleTypeOptions" editable class="w-full"
                      :data-testid="`policy-rule-type-${index}`"
                      @update:model-value="updateRuleType(data, String($event))" />
                  </div>
                  <div class="flex flex-col gap-2">
                    <label class="font-semibold">{{ fieldLabel('policy.editor.rule_value', ruleOperandExample(data.type)) }}</label>
                    <Select v-if="ruleCategoryOptions(data.type).length" :model-value="data.operand"
                      :options="ruleCategoryOptions(data.type)" filter editable
                      :virtual-scroller-options="{ itemSize: 38 }" class="w-full"
                      :data-testid="`policy-rule-category-${index}`"
                      @update:model-value="updateRuleOperand(data, String($event))" />
                    <Select v-else-if="data.type.toUpperCase() === 'NETWORK'" :model-value="data.operand"
                      :options="['tcp', 'udp']" class="w-full"
                      @update:model-value="updateRuleOperand(data, String($event))" />
                    <InputText v-else-if="ruleNeedsOperand(data.type)" :model-value="data.operand" class="w-full"
                      @update:model-value="updateRuleOperand(data, String($event))" />
                    <div v-else class="flex min-h-10 items-center text-surface-500">{{ t('policy.editor.any') }}</div>
                  </div>
                  <div class="flex flex-col gap-2">
                    <label class="font-semibold">{{ fieldLabel('policy.editor.target', 'DIRECT / overseas-exit') }}</label>
                    <Select v-model="data.target" :options="actorOptions" editable class="w-full" />
                  </div>
                  <div class="flex flex-col gap-2">
                    <span class="font-semibold">{{ t('policy.editor.no_resolve') }}</span>
                    <div class="flex min-h-10 items-center">
                      <Checkbox v-if="policyRuleSupportsNoResolve(data.type, data.operand)" v-model="data.noResolve" binary />
                      <span v-else class="text-surface-400">-</span>
                    </div>
                  </div>
                </div>
              </article>
              <Button icon="pi pi-plus" :label="t('policy.editor.add_rule')" class="self-start" @click="addRule" />
            </div>
          </Panel>

          <Panel :header="t('policy.editor.rule_sets')" toggleable collapsed>
            <div class="flex flex-col gap-3">
              <Message severity="info" :closable="false">{{ t('policy.editor.rule_data_help') }}</Message>
              <Message v-if="ruleDataMessage" severity="success" :closable="false">{{ ruleDataMessage }}</Message>
              <Message v-if="ruleDataError" severity="error" :closable="false">{{ ruleDataError }}</Message>
              <article v-for="data in ruleDataRows" :key="data.type" :data-testid="`policy-rule-data-${data.type}`"
                class="grid gap-4 rounded-xl border border-surface-200 p-4 lg:grid-cols-[12rem_minmax(16rem,1fr)_12rem] dark:border-surface-700">
                <div class="flex flex-col gap-1">
                  <span class="font-semibold">{{ t(`policy.editor.resource_${data.type}`) }}</span>
                  <span class="text-xs text-surface-500">{{ data.type }}</span>
                </div>
                <div class="flex flex-col gap-2">
                  <label class="font-semibold">{{ fieldLabel('policy.editor.rule_data_source', managedRuleDataSource(data.type)) }}</label>
                  <InputText :model-value="ruleDataSource(data)" class="w-full"
                    :aria-label="t('policy.editor.rule_data_source')"
                    @update:model-value="setRuleDataSource(data, String($event))" />
                  <small class="text-surface-500">{{ usesBundledRuleData(data)
                    ? t('policy.editor.builtin_help')
                    : t('policy.editor.rule_data_source_help') }}</small>
                </div>
                <div class="flex flex-col items-start gap-2">
                  <span :class="isManagedRuleDataInstalled(data) || usesBundledRuleData(data) ? 'text-green-600' : 'text-surface-500'">
                    {{ managedRuleDataStatus(data) }}
                  </span>
                  <div v-if="managedRuleDataHash(data)" class="flex flex-col gap-1">
                    <span class="text-xs text-surface-500">SHA-256</span>
                    <code class="break-all text-xs">{{ managedRuleDataHash(data) }}</code>
                  </div>
                  <Button icon="pi pi-refresh" :data-testid="`policy-rule-data-update-${data.type}`"
                    :label="t('policy.editor.update_rule_data')" size="small"
                    :loading="updatingRuleData === data.type"
                    :disabled="!props.api?.update_policy_rule_data || !config.instance_id || Boolean(updatingRuleData)"
                    @click="updateRuleData(data)" />
                  <Button v-if="isManagedRuleDataInstalled(data) && !usesBundledRuleData(data)"
                    icon="pi pi-trash" severity="danger" text :aria-label="t('policy.editor.remove_rule_data')"
                    @click="removeRuleData(data)" />
                </div>
              </article>
            </div>
          </Panel>
        </template>

        <Panel :header="t('policy.editor.advanced_yaml')" toggleable collapsed>
          <label for="policy_config_inline" class="mb-2 block font-semibold">{{ fieldLabel('policy.editor.advanced_yaml', 'version: 1') }}</label>
          <Textarea id="policy_config_inline" v-model="config.policy_config_inline" rows="16" auto-resize
            class="w-full font-mono" :placeholder="t('policy_config_inline_placeholder')" />
        </Panel>
      </template>
    </template>
    </template>
  </div>
</template>
