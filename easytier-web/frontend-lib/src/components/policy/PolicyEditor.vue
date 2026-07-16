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
  emptyPolicyDocument,
  parsePolicyDocument,
  policyRuleSupportsNoResolve,
  serializePolicyDocument,
  type PolicyEditorDocument,
  type PolicyGroupRow,
  type PolicyProxyRow,
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
const props = defineProps<{ api?: Api.RemoteClient }>()
const { t } = useI18n()

const document = ref<PolicyEditorDocument>(emptyPolicyDocument())
const ruleDataRows = ref<PolicyRuleSetRow[]>(managedRuleDataRows(document.value))
const parseError = ref('')
const editError = ref('')
const ruleDataMessage = ref('')
const ruleDataError = ref('')
const updatingRuleData = ref<PolicyRuleSetKind | ''>('')
const outboundInfo = ref<Api.ListPolicyOutboundInterfacesResponse>()
const outboundLoading = ref(false)
const outboundError = ref('')
let lastSerialized = ''
const rowKeys = new WeakMap<object, number>()
let nextRowKey = 1

const sourceOptions = computed(() => [
  { label: t('policy.editor.inline'), value: 'inline' },
  { label: t('policy.editor.file'), value: 'file' },
])
const proxyViaOptions = ['mesh', 'native']
const groupTypeOptions = ['fallback', 'chain']
const outboundOptions = computed(() => (outboundInfo.value?.interfaces ?? []).map(item => ({
  label: item.addresses.length
    ? `${item.name} (${item.addresses.join(', ')})${item.recommended ? ` · ${t('policy.editor.recommended')}` : ''}`
    : `${item.name}${item.recommended ? ` · ${t('policy.editor.recommended')}` : ''}`,
  value: item.name,
})))
const ruleTypeOptions = [
  'GEOSITE', 'GEOIP', 'COUNTRY', 'DOMAIN', 'DOMAIN-SUFFIX', 'DOMAIN-KEYWORD', 'IP-CIDR',
  'NETWORK', 'PORT-RANGE', 'INBOUND-TAG', 'EXTERNAL', 'MATCH',
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
  ? `policy.editor.runtime_${runtimeNotice.value.replaceAll('-', '_')}`
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
    if (!document.value.ruleSets.includes(row)) document.value.ruleSets.push(row)
    const size = Number(result.size)
    ruleDataMessage.value = t('policy.editor.rule_data_updated', {
      type: row.type,
      size: Number.isFinite(size) ? `${(size / 1024 / 1024).toFixed(1)} MiB` : String(result.size),
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
  }
  document.value.proxies.push(row)
}

function addGroup() {
  const row: PolicyGroupRow = {
    name: `fallback${document.value.groups.length + 1}`,
    type: 'fallback',
    members: [],
  }
  document.value.groups.push(row)
}

function addRule() {
  const row: PolicyRuleRow = { type: 'GEOSITE', operand: 'CN', target: 'DIRECT', noResolve: false }
  const finalIndex = document.value.rules.findIndex(rule => ['MATCH', 'FINAL'].includes(rule.type))
  if (finalIndex < 0) document.value.rules.push(row)
  else document.value.rules.splice(finalIndex, 0, row)
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
}

function removeAt<T>(rows: T[], index: number) {
  rows.splice(index, 1)
}

function rowKey(row: object) {
  const existing = rowKeys.get(row)
  if (existing !== undefined) return existing
  const key = nextRowKey++
  rowKeys.set(row, key)
  return key
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
  const previouslySupported = policyRuleSupportsNoResolve(row.type)
  row.type = value
  const supported = policyRuleSupportsNoResolve(value)
  if (!supported) row.noResolve = false
  else if (!previouslySupported) row.noResolve = true
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
  if (enabled) void loadOutboundInterfaces()
})

onMounted(() => {
  void loadOutboundInterfaces()
})
</script>

<template>
  <div class="flex flex-col gap-4">
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
          <label for="policy_leaf_executable">{{ t('policy_leaf_executable') }}</label>
          <InputText id="policy_leaf_executable" v-model="config.policy_leaf_executable"
            placeholder="easytier-leaf-worker" />
        </div>
      </div>

      <template v-if="sourceMode === 'file'">
        <div class="flex items-center">
          <label for="policy_config_file">{{ t('policy_config_file') }}</label>
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
                <label for="policy_dns_direct" class="font-semibold">{{ t('policy.editor.dns_direct') }}</label>
                <Textarea id="policy_dns_direct" :model-value="document.dns.direct.join('\n')" rows="4"
                  auto-resize class="w-full" :placeholder="t('policy.editor.dns_direct_placeholder')"
                  @update:model-value="updateDnsServers('direct', String($event))" />
                <small class="text-surface-500">{{ t('policy.editor.dns_direct_help') }}</small>
              </div>
                <div class="flex flex-col gap-2 rounded-xl border border-surface-200 bg-surface-50 p-4 dark:border-surface-700 dark:bg-surface-900">
                  <span class="text-xs font-semibold uppercase tracking-wide text-surface-500">PROXY</span>
                <label for="policy_dns_proxy" class="font-semibold">{{ t('policy.editor.dns_proxy') }}</label>
                <Textarea id="policy_dns_proxy" :model-value="document.dns.proxy.join('\n')" rows="4"
                  auto-resize class="w-full" placeholder="doh:cloudflare-dns.com@1.1.1.1"
                  @update:model-value="updateDnsServers('proxy', String($event))" />
                <small class="text-surface-500">{{ t('policy.editor.dns_proxy_help') }}</small>
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
                  <div class="flex items-center gap-2">
                    <span class="rounded-full bg-primary-100 px-2 py-1 text-xs font-semibold text-primary-700 dark:bg-primary-900 dark:text-primary-200">#{{ index + 1 }}</span>
                    <strong>{{ data.name || t('policy.editor.unnamed_node') }}</strong>
                  </div>
                  <Button icon="pi pi-trash" severity="danger" text :aria-label="t('policy.editor.remove_node')"
                    @click="removeAt(document.proxies, index)" />
                </div>
                <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
                  <div class="flex flex-col gap-2">
                    <label :for="`policy_proxy_name_${index}`" class="font-semibold">{{ t('policy.editor.name') }}</label>
                    <InputText :id="`policy_proxy_name_${index}`" v-model="data.name" class="w-full"
                      :data-testid="`policy-proxy-name-${index}`" />
                  </div>
                  <div class="flex flex-col gap-2">
                    <label class="font-semibold">{{ t('policy.editor.path') }}</label>
                    <Select v-model="data.via" :options="proxyViaOptions" class="w-full" />
                  </div>
                  <div class="flex flex-col gap-2 sm:col-span-2">
                    <label class="font-semibold">{{ t('policy.editor.server') }}</label>
                    <div v-if="data.via === 'mesh'" class="grid gap-2 sm:grid-cols-2">
                      <InputText v-model="data.instanceId" class="w-full" :placeholder="t('policy.editor.instance_id')" />
                      <InputText v-model="data.virtualIp" class="w-full" :placeholder="t('policy.editor.virtual_ip')" />
                    </div>
                    <InputText v-else v-model="data.address" placeholder="host / IP" class="w-full" />
                  </div>
                  <div class="flex flex-col gap-2">
                    <label class="font-semibold">{{ t('policy.editor.port') }}</label>
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
                    <span class="font-semibold">UDP</span>
                    <div class="flex min-h-10 items-center gap-2">
                      <Checkbox :input-id="`policy_proxy_udp_${index}`" v-model="data.udp" binary />
                      <label :for="`policy_proxy_udp_${index}`">{{ t('policy.editor.udp_capable') }}</label>
                    </div>
                  </div>
                  <div class="flex flex-col gap-2 sm:col-span-2">
                    <label class="font-semibold">{{ t('policy.editor.credentials') }}</label>
                    <div class="grid gap-2 sm:grid-cols-2">
                      <InputText v-model="data.username" class="w-full" :placeholder="t('username')" />
                      <Password v-model="data.password" class="w-full" :placeholder="t('password')" :feedback="false" toggle-mask />
                    </div>
                  </div>
                </div>
              </article>
              <Button icon="pi pi-plus" :label="t('policy.editor.add_node')" class="self-start" @click="addProxy" />
            </div>
          </Panel>

          <Panel :header="t('policy.editor.groups')" toggleable collapsed>
            <div class="flex flex-col gap-3">
              <article v-for="(data, index) in document.groups" :key="rowKey(data)"
                class="grid gap-4 rounded-xl border border-surface-200 p-4 sm:grid-cols-2 lg:grid-cols-[minmax(10rem,1fr)_12rem_minmax(16rem,2fr)_auto] dark:border-surface-700">
                <div class="flex flex-col gap-2">
                  <label class="font-semibold">{{ t('policy.editor.name') }}</label>
                  <InputText v-model="data.name" class="w-full" />
                </div>
                <div class="flex flex-col gap-2">
                  <label class="font-semibold">{{ t('policy.editor.group_type') }}</label>
                  <Select v-model="data.type" :options="groupTypeOptions" class="w-full" />
                </div>
                <div class="flex flex-col gap-2 sm:col-span-2 lg:col-span-1">
                  <label class="font-semibold">{{ t('policy.editor.members') }}</label>
                  <InputText :model-value="data.members.join(',')" class="w-full"
                      :placeholder="t('policy.editor.members_hint')"
                      @update:model-value="updateMembers(data, String($event))" />
                </div>
                <Button icon="pi pi-trash" severity="danger" text class="self-end justify-self-end"
                  :aria-label="t('policy.editor.remove_group')" @click="removeAt(document.groups, index)" />
              </article>
              <Button icon="pi pi-plus" :label="t('policy.editor.add_group')" class="self-start" @click="addGroup" />
            </div>
          </Panel>

          <Panel :header="t('policy.editor.rules')" toggleable>
            <div class="flex flex-col gap-3">
              <Message severity="info" :closable="false">{{ t('policy.editor.order_help') }}</Message>
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
                  <span class="text-sm font-semibold text-surface-500">{{ t('policy.editor.rule_priority', { index: index + 1 }) }}</span>
                  <div class="flex gap-1">
                    <Button icon="pi pi-arrow-up" severity="secondary" text :disabled="index === 0"
                      :aria-label="t('policy.editor.move_up')" @click="moveRule(index, -1)" />
                    <Button icon="pi pi-arrow-down" severity="secondary" text :disabled="index === document.rules.length - 1"
                      :aria-label="t('policy.editor.move_down')" @click="moveRule(index, 1)" />
                    <Button icon="pi pi-trash" severity="danger" text :aria-label="t('policy.editor.remove_rule')"
                      @click="removeAt(document.rules, index)" />
                  </div>
                </div>
                <div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-[minmax(10rem,1fr)_minmax(12rem,2fr)_minmax(10rem,1fr)_9rem]">
                  <div class="flex flex-col gap-2">
                    <label class="font-semibold">{{ t('policy.editor.rule_type') }}</label>
                    <Select :model-value="data.type" :options="ruleTypeOptions" editable class="w-full"
                      :data-testid="`policy-rule-type-${index}`"
                      @update:model-value="updateRuleType(data, String($event))" />
                  </div>
                  <div class="flex flex-col gap-2">
                    <label class="font-semibold">{{ t('policy.editor.rule_value') }}</label>
                    <InputText v-if="ruleNeedsOperand(data.type)" v-model="data.operand" class="w-full" />
                    <div v-else class="flex min-h-10 items-center text-surface-500">{{ t('policy.editor.any') }}</div>
                  </div>
                  <div class="flex flex-col gap-2">
                    <label class="font-semibold">{{ t('policy.editor.target') }}</label>
                    <Select v-model="data.target" :options="actorOptions" editable class="w-full" />
                  </div>
                  <div class="flex flex-col gap-2">
                    <span class="font-semibold">{{ t('policy.editor.no_resolve') }}</span>
                    <div class="flex min-h-10 items-center">
                      <Checkbox v-if="policyRuleSupportsNoResolve(data.type)" v-model="data.noResolve" binary />
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
              <article v-for="data in ruleDataRows" :key="data.type"
                class="grid gap-4 rounded-xl border border-surface-200 p-4 lg:grid-cols-[12rem_minmax(16rem,1fr)_12rem] dark:border-surface-700">
                <div class="flex flex-col gap-1">
                  <span class="font-semibold">{{ t(`policy.editor.resource_${data.type}`) }}</span>
                  <span class="text-xs text-surface-500">{{ data.type }}</span>
                </div>
                <div class="flex flex-col gap-2">
                  <template v-if="usesBundledRuleData(data)">
                    <div class="break-all font-mono text-xs">{{ managedRuleDataSource(data.type) }}</div>
                    <small class="text-surface-500">{{ t('policy.editor.builtin_help') }}</small>
                  </template>
                  <template v-else>
                    <InputText :model-value="ruleDataSource(data)" class="w-full"
                      :aria-label="t('policy.editor.rule_data_source')"
                      @update:model-value="setRuleDataSource(data, String($event))" />
                    <small class="text-surface-500">{{ t('policy.editor.rule_data_source_help') }}</small>
                  </template>
                </div>
                <div class="flex flex-col items-start gap-2">
                  <span :class="isManagedRuleDataInstalled(data) || usesBundledRuleData(data) ? 'text-green-600' : 'text-surface-500'">
                    {{ managedRuleDataStatus(data) }}
                  </span>
                  <Button v-if="!usesBundledRuleData(data)" icon="pi pi-refresh"
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
          <Textarea id="policy_config_inline" v-model="config.policy_config_inline" rows="16" auto-resize
            class="w-full font-mono" :placeholder="t('policy_config_inline_placeholder')" />
        </Panel>
      </template>
    </template>
  </div>
</template>
