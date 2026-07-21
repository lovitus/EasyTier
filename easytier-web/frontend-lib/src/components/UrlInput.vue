<script setup lang="ts">
import { AutoComplete, Button, Dialog, InputNumber, InputText } from 'primevue'
import InputGroup from 'primevue/inputgroup'
import InputGroupAddon from 'primevue/inputgroupaddon'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

const props = defineProps<{
    placeholder?: string
    protos: { [proto: string]: number }
}>()

const { t } = useI18n()
const url = defineModel<string>({ required: true })
const editing = ref(false)
const hostFocused = ref(false)

const QUIC_BRUTAL_PROTO = 'quic-brutal'
const MIN_QUIC_BRUTAL_TX_BPS = 1_000_000
const MAX_QUIC_BRUTAL_TX_BPS = 100_000_000_000

interface ParsedUrl {
    proto: string
    host: string
    port: number | null
    txBps: number | null
}

const parseUrl = (val: string | null | undefined): ParsedUrl => {
    const getValidPort = (portStr: string, proto: string) => {
        const p = parseInt(portStr)
        return isNaN(p) ? (props.protos[proto] ?? 11010) : p
    }
    const parseByPattern = (input: string) => {
        const trimmed = input.trim()
        if (!trimmed) {
            return null
        }
        const match = trimmed.match(/^([A-Za-z][A-Za-z0-9+.-]*):\/\/(.*)$/)
        const proto = match ? match[1] : 'tcp'
        const rest = match ? match[2] : trimmed
        const authority = rest.split(/[/?#]/)[0]
        if (!authority) {
            return null
        }
        const hostAndMaybePort = authority.includes('@') ? authority.slice(authority.lastIndexOf('@') + 1) : authority
        if (hostAndMaybePort.startsWith('[')) {
            const ipv6End = hostAndMaybePort.indexOf(']')
            if (ipv6End > 0) {
                const host = hostAndMaybePort.slice(0, ipv6End + 1)
                const remain = hostAndMaybePort.slice(ipv6End + 1)
                // null = no explicit port in URL; do not fabricate a default
                const port: number | null = remain.startsWith(':') ? getValidPort(remain.slice(1), proto) : null
                return { proto, host, port, txBps: parseTxBps(rest, proto) }
            }
        }
        const portMatch = hostAndMaybePort.match(/^(.*):(\d+)$/)
        const host = portMatch ? portMatch[1] : hostAndMaybePort
        // null = no explicit port in URL; buildUrlValue will omit the port entirely,
        // preserving the protocol's implied standard port (e.g. 443 for wss://).
        const port: number | null = portMatch ? parseInt(portMatch[2]) : null
        return { proto, host, port, txBps: parseTxBps(rest, proto) }
    }

    if (!val) {
        return { proto: 'tcp', host: '', port: props.protos['tcp'] ?? 11010, txBps: null }
    }
    const parsedByPattern = parseByPattern(val)
    if (parsedByPattern) {
        return parsedByPattern
    }
    return { proto: 'tcp', host: '', port: null, txBps: null }
}

const parseTxBps = (rest: string, proto: string): number | null => {
    if (proto !== QUIC_BRUTAL_PROTO) {
        return null
    }
    const query = rest.split('?')[1]?.split('#')[0]
    const rawValue = query ? new URLSearchParams(query).get('tx_bps') : null
    if (!rawValue || !/^\d+$/.test(rawValue)) {
        return null
    }
    const value = Number(rawValue)
    return Number.isSafeInteger(value) ? value : null
}

const internalValue = ref(parseUrl(url.value))
const defaultHost = '0.0.0.0'

const buildUrlValue = (value: ParsedUrl, forceDefaultHost = false) => {
    const proto = value.proto || 'tcp'
    const rawHost = (value.host ?? '').trim()
    const host = rawHost || (forceDefaultHost ? defaultHost : '')
    if (!host) {
        return null
    }
    // Omit port when the protocol uses no port (protos value = 0), or when the
    // original URL had no explicit port (port === null) – avoids overwriting an
    // implicit standard port (e.g. 443 for wss) with an EasyTier default (11012).
    const authority = props.protos[proto] === 0 || value.port === null
        ? `${proto}://${host}`
        : `${proto}://${host}:${value.port}`
    if (proto !== QUIC_BRUTAL_PROTO) {
        return authority
    }
    return value.txBps === null ? authority : `${authority}?tx_bps=${value.txBps}`
}

const syncUrlFromInternal = (forceDefaultHost = false) => {
    const nextUrl = buildUrlValue(internalValue.value, forceDefaultHost)
    if (!nextUrl || nextUrl === url.value) {
        return
    }
    url.value = nextUrl
}

const onHostBlur = () => {
    hostFocused.value = false
    syncUrlFromInternal(true)
}

const onHostFocus = () => {
    hostFocused.value = true
}

const onDialogConfirm = () => {
    syncUrlFromInternal(true)
    editing.value = false
}

const isNoPortProto = computed(() => {
    return props.protos[internalValue.value.proto] === 0
})

const isQuicBrutal = computed(() => internalValue.value.proto === QUIC_BRUTAL_PROTO)

// Sync from external
watch(() => url.value, (newVal) => {
    if (hostFocused.value) {
        return
    }
    const parsed = parseUrl(newVal)
    const internalHost = internalValue.value.host ?? ''
    const sameHost = parsed.host === internalHost || (!internalHost.trim() && parsed.host === defaultHost)
    if (parsed.proto !== internalValue.value.proto ||
        !sameHost ||
        parsed.port !== internalValue.value.port ||
        parsed.txBps !== internalValue.value.txBps) {
        internalValue.value = parsed
    }
})

// Sync to external
watch(internalValue, () => {
    syncUrlFromInternal(false)
}, { deep: true })

const protoOptions = computed(() => Object.keys(props.protos))
const filteredProtos = ref<string[]>([])

const searchProtos = (event: { query: string }) => {
    if (!event.query.trim().length) {
        filteredProtos.value = [...protoOptions.value]
    } else {
        filteredProtos.value = protoOptions.value.filter((proto) => {
            return proto.toLowerCase().startsWith(event.query.toLowerCase())
        })
    }
}

const onProtoChange = (newProto: string) => {
    const oldProto = internalValue.value.proto
    const oldDefault = props.protos[oldProto]
    const newDefault = props.protos[newProto]

    if (oldDefault !== undefined && internalValue.value.port === oldDefault && newDefault !== undefined) {
        internalValue.value.port = newDefault
    }
    if (newProto === QUIC_BRUTAL_PROTO && oldProto !== QUIC_BRUTAL_PROTO) {
        internalValue.value.txBps = null
    } else if (newProto !== QUIC_BRUTAL_PROTO) {
        internalValue.value.txBps = null
    }
    internalValue.value.proto = newProto
}
</script>

<template>
    <div class="url-input-container w-full min-w-0 overflow-hidden">
        <InputGroup class="url-input-full w-full min-w-0">
            <AutoComplete :model-value="internalValue.proto" :suggestions="filteredProtos" dropdown
                class="max-w-32 proto-autocomplete-in-group" @complete="searchProtos"
                @update:model-value="onProtoChange" />
            <InputText v-model="internalValue.host" :placeholder="placeholder || '0.0.0.0'" class="grow min-w-0"
                @focus="onHostFocus" @blur="onHostBlur" />
            <template v-if="!isNoPortProto">
                <InputGroupAddon>
                    <span style="font-weight: bold">:</span>
                </InputGroupAddon>
                <InputNumber v-model="internalValue.port" :format="false" :min="1" :max="65535" class="max-w-24"
                    :placeholder="String(protos[internalValue.proto] ?? 11010)" fluid />
            </template>
            <template v-if="isQuicBrutal">
                <InputGroupAddon>
                    <span>bit/s</span>
                </InputGroupAddon>
                <InputNumber v-model="internalValue.txBps" :format="false"
                    :min="MIN_QUIC_BRUTAL_TX_BPS" :max="MAX_QUIC_BRUTAL_TX_BPS" class="max-w-40"
                    :placeholder="t('quic_brutal_tx_bps_placeholder')" fluid />
            </template>
            <!-- Rendered in both responsive branches; keep action slot content free of side effects and duplicate IDs. -->
            <slot name="actions"></slot>
        </InputGroup>

        <div
            class="url-input-compact flex justify-between items-center p-2 border rounded w-full min-w-0 overflow-hidden">
            <span class="truncate mr-2 min-w-0 flex-1 overflow-hidden">{{ url }}</span>
            <div class="flex items-center shrink-0">
                <Button icon="pi pi-pencil" class="p-button-sm p-button-text" :aria-label="t('web.common.edit')"
                    @click="editing = true" />
                <slot name="actions"></slot>
            </div>
        </div>

        <Dialog v-model:visible="editing" modal :header="placeholder" :style="{ width: '90vw', maxWidth: '500px' }">
            <div class="flex flex-col gap-4 py-4">
                <div class="flex flex-col gap-2">
                    <label>{{ t('tunnel_proto') }}</label>
                    <AutoComplete :model-value="internalValue.proto" :suggestions="filteredProtos" dropdown fluid
                        @complete="searchProtos" @update:model-value="onProtoChange" />
                </div>
                <div class="flex flex-col gap-2">
                    <label>{{ t('web.common.address') || 'Address' }}</label>
                    <InputText v-model="internalValue.host" :placeholder="placeholder || '0.0.0.0'" class="w-full"
                        @focus="onHostFocus" @blur="onHostBlur" />
                </div>
                <div v-if="!isNoPortProto" class="flex flex-col gap-2">
                    <label>{{ t('port') }}</label>
                    <InputNumber v-model="internalValue.port" :format="false" :min="1" :max="65535" class="w-full"
                        :placeholder="String(protos[internalValue.proto] ?? 11010)" />
                </div>
                <div v-if="isQuicBrutal" class="flex flex-col gap-2">
                    <label>{{ t('quic_brutal_tx_bps') }}</label>
                    <InputNumber v-model="internalValue.txBps" :format="false"
                        :min="MIN_QUIC_BRUTAL_TX_BPS" :max="MAX_QUIC_BRUTAL_TX_BPS"
                        :placeholder="t('quic_brutal_tx_bps_placeholder')" class="w-full" />
                </div>
            </div>
            <template #footer>
                <Button :label="t('web.common.confirm') || 'Done'" icon="pi pi-check" @click="onDialogConfirm"
                    autofocus />
            </template>
        </Dialog>
    </div>
</template>

<style scoped>
.url-input-container {
    container-type: inline-size;
}

.url-input-full {
    display: none;
}

.url-input-compact {
    display: flex;
}

@container (min-width: 400px) {
    .url-input-full {
        display: flex;
    }

    .url-input-compact {
        display: none;
    }
}

.proto-autocomplete-in-group,
.proto-autocomplete-in-group :deep(.p-autocomplete-input),
.proto-autocomplete-in-group :deep(.p-autocomplete-dropdown) {
    border-top-right-radius: 0 !important;
    border-bottom-right-radius: 0 !important;
}

.proto-autocomplete-in-group :deep(.p-autocomplete-dropdown) {
    border-right: 0 !important;
}
</style>
