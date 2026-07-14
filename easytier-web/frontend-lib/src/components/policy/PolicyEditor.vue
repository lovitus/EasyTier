<script setup lang="ts">
import {
  Button,
  Checkbox,
  Column,
  DataTable,
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
import {
  emptyPolicyDocument,
  parsePolicyDocument,
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
    config.value.policy_config_inline = serializePolicyDocument(emptyPolicyDocument())
  }
}

function setEnabled(enabled: boolean) {
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
    port: 1080,
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

function ruleSupportsNoResolve(type: string) {
  return ['IP-CIDR', 'GEOIP', 'COUNTRY'].includes(type.toUpperCase())
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
  if (config.value.enable_policy_proxy) void loadOutboundInterfaces()
})
</script>

<template>
  <div class="flex flex-col gap-4">
    <div class="flex items-center gap-3">
      <Checkbox input-id="enable_policy_proxy" :model-value="config.enable_policy_proxy" binary
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
        <Message v-if="outboundError" severity="warn" :closable="false">
          {{ t('policy.editor.outbound_load_failed') }}: {{ outboundError }}
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
          <Panel :header="t('policy.editor.dns')" toggleable collapsed>
            <div class="grid gap-4 md:grid-cols-2">
              <div class="flex flex-col gap-2">
                <label for="policy_dns_direct" class="font-semibold">{{ t('policy.editor.dns_direct') }}</label>
                <Textarea id="policy_dns_direct" :model-value="document.dns.direct.join('\n')" rows="4"
                  auto-resize :placeholder="t('policy.editor.dns_direct_placeholder')"
                  @update:model-value="updateDnsServers('direct', String($event))" />
                <small class="text-surface-500">{{ t('policy.editor.dns_direct_help') }}</small>
              </div>
              <div class="flex flex-col gap-2">
                <label for="policy_dns_proxy" class="font-semibold">{{ t('policy.editor.dns_proxy') }}</label>
                <Textarea id="policy_dns_proxy" :model-value="document.dns.proxy.join('\n')" rows="4"
                  auto-resize placeholder="doh:cloudflare-dns.com@1.1.1.1"
                  @update:model-value="updateDnsServers('proxy', String($event))" />
                <small class="text-surface-500">{{ t('policy.editor.dns_proxy_help') }}</small>
              </div>
            </div>
            <Message severity="info" :closable="false">{{ t('policy.editor.dns_isolation_help') }}</Message>
          </Panel>

          <Panel :header="t('policy.editor.nodes')" toggleable>
            <div class="flex flex-col gap-3">
              <DataTable :value="document.proxies" data-key="name" responsive-layout="scroll">
                <Column field="name" :header="t('policy.editor.name')">
                  <template #body="{ data }"><InputText v-model="data.name" class="w-36" /></template>
                </Column>
                <Column field="via" :header="t('policy.editor.path')">
                  <template #body="{ data }">
                    <Select v-model="data.via" :options="proxyViaOptions" class="w-28" />
                  </template>
                </Column>
                <Column :header="t('policy.editor.server')">
                  <template #body="{ data }">
                    <div v-if="data.via === 'mesh'" class="flex flex-col gap-1 min-w-56">
                      <InputText v-model="data.instanceId" :placeholder="t('policy.editor.instance_id')" />
                      <InputText v-model="data.virtualIp" :placeholder="t('policy.editor.virtual_ip')" />
                    </div>
                    <InputText v-else v-model="data.address" placeholder="host / IP" class="min-w-48" />
                  </template>
                </Column>
                <Column field="port" :header="t('policy.editor.port')">
                  <template #body="{ data }">
                    <InputNumber v-model="data.port" :min="1" :max="65535" :use-grouping="false" class="w-28" />
                  </template>
                </Column>
                <Column field="udp" header="UDP">
                  <template #body="{ data }"><Checkbox v-model="data.udp" binary /></template>
                </Column>
                <Column :header="t('policy.editor.credentials')">
                  <template #body="{ data }">
                    <div class="flex flex-col gap-1 min-w-40">
                      <InputText v-model="data.username" :placeholder="t('username')" />
                      <Password v-model="data.password" :placeholder="t('password')" :feedback="false" toggle-mask />
                    </div>
                  </template>
                </Column>
                <Column header-style="width: 4rem">
                  <template #body="{ index }">
                    <Button icon="pi pi-trash" severity="danger" text
                      @click="removeAt(document.proxies, index)" />
                  </template>
                </Column>
              </DataTable>
              <Button icon="pi pi-plus" :label="t('policy.editor.add_node')" class="self-start" @click="addProxy" />
            </div>
          </Panel>

          <Panel :header="t('policy.editor.groups')" toggleable collapsed>
            <div class="flex flex-col gap-3">
              <DataTable :value="document.groups" data-key="name" responsive-layout="scroll">
                <Column field="name" :header="t('policy.editor.name')">
                  <template #body="{ data }"><InputText v-model="data.name" /></template>
                </Column>
                <Column field="type" :header="t('policy.editor.group_type')">
                  <template #body="{ data }"><Select v-model="data.type" :options="groupTypeOptions" /></template>
                </Column>
                <Column :header="t('policy.editor.members')">
                  <template #body="{ data }">
                    <InputText :model-value="data.members.join(',')" class="w-full min-w-64"
                      :placeholder="t('policy.editor.members_hint')"
                      @update:model-value="updateMembers(data, String($event))" />
                  </template>
                </Column>
                <Column header-style="width: 4rem">
                  <template #body="{ index }">
                    <Button icon="pi pi-trash" severity="danger" text @click="removeAt(document.groups, index)" />
                  </template>
                </Column>
              </DataTable>
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
              <DataTable :value="document.rules" responsive-layout="scroll"
                @row-reorder="document.rules = $event.value">
                <Column row-reorder header-style="width: 3rem" />
                <Column field="type" :header="t('policy.editor.rule_type')">
                  <template #body="{ data }">
                    <Select v-model="data.type" :options="ruleTypeOptions" editable class="min-w-44" />
                  </template>
                </Column>
                <Column :header="t('policy.editor.rule_value')">
                  <template #body="{ data }">
                    <InputText v-if="ruleNeedsOperand(data.type)" v-model="data.operand" class="w-full min-w-48" />
                    <span v-else class="text-surface-500">{{ t('policy.editor.any') }}</span>
                  </template>
                </Column>
                <Column field="target" :header="t('policy.editor.target')">
                  <template #body="{ data }">
                    <Select v-model="data.target" :options="actorOptions" editable class="w-full min-w-40" />
                  </template>
                </Column>
                <Column :header="t('policy.editor.no_resolve')" header-style="width: 7rem">
                  <template #body="{ data }">
                    <Checkbox v-if="ruleSupportsNoResolve(data.type)" v-model="data.noResolve" binary />
                  </template>
                </Column>
                <Column header-style="width: 4rem">
                  <template #body="{ index }">
                    <Button icon="pi pi-trash" severity="danger" text @click="removeAt(document.rules, index)" />
                  </template>
                </Column>
              </DataTable>
              <Button icon="pi pi-plus" :label="t('policy.editor.add_rule')" class="self-start" @click="addRule" />
            </div>
          </Panel>

          <Panel :header="t('policy.editor.rule_sets')" toggleable collapsed>
            <div class="flex flex-col gap-3">
              <Message severity="info" :closable="false">{{ t('policy.editor.rule_data_help') }}</Message>
              <Message v-if="ruleDataMessage" severity="success" :closable="false">{{ ruleDataMessage }}</Message>
              <Message v-if="ruleDataError" severity="error" :closable="false">{{ ruleDataError }}</Message>
              <DataTable :value="ruleDataRows" data-key="type" responsive-layout="scroll">
                <Column field="type" :header="t('policy.editor.rule_set_type')">
                  <template #body="{ data }">
                    <div class="flex flex-col gap-1">
                      <span class="font-semibold">{{ t(`policy.editor.resource_${data.type}`) }}</span>
                      <span class="text-xs text-surface-500">{{ data.type }}</span>
                    </div>
                  </template>
                </Column>
                <Column :header="t('policy.editor.managed_source')">
                  <template #body="{ data }">
                    <template v-if="usesBundledRuleData(data)">
                      <div class="font-mono text-xs break-all min-w-72">{{ managedRuleDataSource(data.type) }}</div>
                      <small class="text-surface-500">{{ t('policy.editor.builtin_help') }}</small>
                    </template>
                    <template v-else>
                      <InputText :model-value="ruleDataSource(data)" class="w-full min-w-96"
                        :aria-label="t('policy.editor.rule_data_source')"
                        @update:model-value="setRuleDataSource(data, String($event))" />
                      <small class="text-surface-500">{{ t('policy.editor.rule_data_source_help') }}</small>
                    </template>
                  </template>
                </Column>
                <Column :header="t('policy.editor.status')">
                  <template #body="{ data }">
                    <div class="flex flex-col gap-2 min-w-36">
                      <span :class="isManagedRuleDataInstalled(data) || usesBundledRuleData(data) ? 'text-green-600' : 'text-surface-500'">
                        {{ managedRuleDataStatus(data) }}
                      </span>
                      <Button v-if="!usesBundledRuleData(data)" icon="pi pi-refresh"
                        :label="t('policy.editor.update_rule_data')" size="small"
                        :loading="updatingRuleData === data.type"
                        :disabled="!props.api?.update_policy_rule_data || !config.instance_id || Boolean(updatingRuleData)"
                        @click="updateRuleData(data)" />
                    </div>
                  </template>
                </Column>
                <Column header-style="width: 4rem">
                  <template #body="{ data }">
                    <Button v-if="isManagedRuleDataInstalled(data) && !usesBundledRuleData(data)"
                      icon="pi pi-trash" severity="danger" text
                      @click="removeRuleData(data)" />
                  </template>
                </Column>
              </DataTable>
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
