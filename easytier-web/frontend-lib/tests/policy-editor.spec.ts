import { flushPromises, mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h, nextTick, reactive } from 'vue'
import PolicyEditor from '../src/components/policy/PolicyEditor.vue'
import { MANAGED_RULE_DATA } from '../src/components/policy/managedRuleData'
import type * as Api from '../src/modules/api'
import {
  DEFAULT_NETWORK_CONFIG,
  type NetworkConfig,
  type NetworkInstanceRunningInfo,
} from '../src/types/network'

vi.mock('vue-i18n', () => ({
  useI18n: () => ({ t: (key: string) => key }),
}))

const CheckboxStub = defineComponent({
  name: 'Checkbox',
  props: { modelValue: Boolean, inputId: String },
  emits: ['update:modelValue'],
  setup(props, { emit }) {
    return () => h('input', {
      id: props.inputId,
      type: 'checkbox',
      checked: props.modelValue,
      onChange: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).checked),
    })
  },
})

const ButtonStub = defineComponent({
  name: 'Button',
  inheritAttrs: false,
  props: { label: String, disabled: Boolean },
  emits: ['click'],
  setup(props, { attrs, emit }) {
    return () => h('button', {
      ...attrs,
      disabled: props.disabled,
      onClick: () => emit('click'),
    }, props.label)
  },
})

const PanelStub = defineComponent({
  name: 'Panel',
  props: { header: String },
  setup(props, { slots }) {
    return () => h('section', { 'data-header': props.header }, slots.default?.())
  },
})

const MessageStub = defineComponent({
  name: 'Message',
  setup(_, { slots }) {
    return () => h('div', { 'data-stub': 'message' }, slots.default?.())
  },
})

const TextareaStub = defineComponent({
  name: 'Textarea',
  props: { modelValue: String, id: String, readonly: Boolean },
  emits: ['update:modelValue'],
  setup(props, { emit }) {
    return () => h('textarea', {
      id: props.id,
      value: props.modelValue,
      readonly: props.readonly,
      onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLTextAreaElement).value),
    })
  },
})

const InputTextStub = defineComponent({
  name: 'InputText',
  inheritAttrs: false,
  props: { modelValue: String, id: String, readonly: Boolean },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('input', {
      ...attrs,
      id: props.id,
      value: props.modelValue,
      readonly: props.readonly,
      onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
    })
  },
})

const SelectButtonStub = defineComponent({
  name: 'SelectButton',
  inheritAttrs: false,
  props: {
    modelValue: String,
    disabled: Boolean,
    options: Array,
    optionValue: String,
    optionLabel: String,
  },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('select', {
      ...attrs,
      disabled: props.disabled,
      value: props.modelValue,
      'data-stub': 'select-button',
      onChange: (event: Event) =>
        emit('update:modelValue', (event.target as HTMLSelectElement).value),
    }, (props.options as Array<Record<string, unknown>> | undefined)?.map(option =>
      h('option', {
        value: option[props.optionValue ?? 'value'] as string,
        disabled: Boolean(option.disabled),
      }, option[props.optionLabel ?? 'label'] as string),
    ))
  },
})

const SelectStub = defineComponent({
  name: 'Select',
  inheritAttrs: false,
  props: { modelValue: String, options: Array },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('select', {
      ...attrs,
      value: props.modelValue,
      onChange: (event: Event) => emit('update:modelValue', (event.target as HTMLSelectElement).value),
    }, (props.options as unknown[] | undefined ?? [props.modelValue]).map(option => {
      const value = typeof option === 'string' ? option : String((option as { value: unknown }).value)
      return h('option', { value }, value)
    }))
  },
})

function mountEditor(
  config: NetworkConfig,
  api?: import('../src/modules/api').RemoteClient,
  options: {
    yamlOnly?: boolean
    readOnly?: boolean
    hideBackendSelector?: boolean
    runtimeInfo?: NetworkInstanceRunningInfo
  } = {},
) {
  const model = reactive(config) as NetworkConfig
  const wrapper = mount(PolicyEditor, {
    props: {
      modelValue: model,
      api,
      ...options,
    },
    global: {
      directives: { tooltip: () => {} },
      stubs: {
        Button: ButtonStub,
        Checkbox: CheckboxStub,
        Column: true,
        DataTable: true,
        InputNumber: true,
        InputText: InputTextStub,
        Message: MessageStub,
        Panel: PanelStub,
        Password: true,
        Select: SelectStub,
        SelectButton: SelectButtonStub,
        Textarea: TextareaStub,
      },
    },
  })
  return { model, wrapper }
}

async function expandRow(wrapper: ReturnType<typeof mountEditor>['wrapper'], testId: string) {
  await wrapper.get(`[data-testid="${testId}"]`).trigger('click')
  await nextTick()
}

describe('PolicyEditor', () => {
  it('can hide backend selection without mutating a conflicting legacy flag on mount', () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.policy_proxy_backend = 'mihomo'
    config.enable_policy_proxy = true
    config.policy_mihomo_config_file = '/managed/mihomo.yaml'
    const { model, wrapper } = mountEditor(config, undefined, { hideBackendSelector: true })

    expect(wrapper.find('[data-testid="policy-backend-selector"]').exists()).toBe(false)
    expect(model.policy_proxy_backend).toBe('mihomo')
    expect(model.enable_policy_proxy).toBe(true)
    expect(model.policy_mihomo_config_file).toBe('/managed/mihomo.yaml')
  })

  it('makes the focused YAML controls read-only in view mode', () => {
    const inline = DEFAULT_NETWORK_CONFIG()
    inline.enable_policy_proxy = true
    inline.policy_proxy_backend = 'leaf'
    inline.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    const { wrapper: inlineWrapper } = mountEditor(
      inline,
      undefined,
      { yamlOnly: true, readOnly: true },
    )

    expect(inlineWrapper.getComponent({ name: 'YamlCodeEditor' }).props('readOnly')).toBe(true)
    expect(inlineWrapper.get<HTMLSelectElement>('[data-stub="select-button"]').element.disabled)
      .toBe(true)

    const file = DEFAULT_NETWORK_CONFIG()
    file.enable_policy_proxy = true
    file.policy_proxy_backend = 'leaf'
    file.policy_config_file = '/etc/easytier/policy.yaml'
    const { wrapper: fileWrapper } = mountEditor(
      file,
      undefined,
      { yamlOnly: true, readOnly: true },
    )

    expect(fileWrapper.get<HTMLInputElement>('#policy_config_file_quick').element.readOnly)
      .toBe(true)
  })

  it('keeps policy mode plugin-like and initializes a safe inline document only when enabled', async () => {
    const { model, wrapper } = mountEditor(DEFAULT_NETWORK_CONFIG())
    expect(model.enable_policy_proxy).toBe(false)
    expect(model.policy_config_inline).toBe('')
    expect(wrapper.find('[data-header="policy.editor.nodes"]').exists()).toBe(false)

    await wrapper.find<HTMLSelectElement>('[data-testid="policy-backend-selector"]').setValue('leaf')
    await nextTick()

    expect(model.enable_policy_proxy).toBe(true)
    expect(model.policy_proxy_backend).toBe('leaf')
    expect(model.policy_config_inline).toContain('MATCH,default-exit')
    expect(model.policy_config_inline).toContain('GEOSITE,github,github-exit')
    expect(model.policy_config_inline).toContain('port: 7890')
    expect(model.policy_config_inline).toContain('type: chain')
    expect(wrapper.find('[data-header="policy.editor.nodes"]').exists()).toBe(true)
    expect(wrapper.find('[data-header="policy.editor.rules"]').exists()).toBe(true)
    expect(wrapper.find('[data-header="policy.editor.dns"]').exists()).toBe(true)
    expect(wrapper.find('[data-header="policy.editor.groups"]').exists()).toBe(true)
    expect(wrapper.find('#policy_advanced_features').exists()).toBe(false)
    expect(model.policy_config_inline).toContain('114.114.114.114')
    expect(model.policy_config_inline).toContain('doh:cloudflare-dns.com@1.1.1.1')
    expect(model.policy_config_inline).toContain('doh:dns.google@8.8.8.8')
    expect(model.policy_config_inline).toContain('doh:dns.quad9.net@9.9.9.9')
  })

  it('keeps Leaf and Mihomo sources isolated while exposing only the native file', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'mihomo'
    config.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    config.policy_outbound_interface = 'eth0'
    config.policy_mihomo_config_inline = 'proxies: [native-mihomo]'
    const { model, wrapper } = mountEditor(config)
    await nextTick()

    expect(model).toMatchObject({
      enable_policy_proxy: true,
      policy_proxy_backend: 'mihomo',
      policy_config_inline: 'version: 1\nrules: ["MATCH,DIRECT"]\n',
      policy_mihomo_config_inline: 'proxies: [native-mihomo]',
      policy_outbound_interface: 'eth0',
    })
    expect(wrapper.find('#policy_config_file').exists()).toBe(true)
    expect(wrapper.find('#policy_mihomo_config_inline').exists()).toBe(false)
    expect(wrapper.find('[data-header="policy.editor.nodes"]').exists()).toBe(false)

    await wrapper.find<HTMLSelectElement>('[data-testid="policy-backend-selector"]')
      .setValue('leaf')
    await nextTick()
    expect(model.enable_policy_proxy).toBe(true)
    expect(model.policy_config_inline).toBe('version: 1\nrules: ["MATCH,DIRECT"]\n')
    expect(model.policy_mihomo_config_inline).toBe('proxies: [native-mihomo]')
  })

  it('opens an empty Mihomo source as a file without borrowing Leaf', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.policy_proxy_backend = 'mihomo'
    config.enable_policy_proxy = false
    config.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    const { model, wrapper } = mountEditor(config)
    await nextTick()

    expect(wrapper.find('#policy_config_file').exists()).toBe(true)
    expect(wrapper.find('#policy_mihomo_config_inline').exists()).toBe(false)
    expect(model.policy_mihomo_config_inline).toBe('')
    expect(model.policy_config_inline).toContain('MATCH,DIRECT')

    await wrapper.find<HTMLSelectElement>('[data-testid="policy-backend-selector"]')
      .setValue('leaf')
    await nextTick()

    expect(model.policy_config_inline).toContain('MATCH,DIRECT')
    expect(model.policy_mihomo_config_file).toBe('')
    expect(wrapper.find('#policy_config_inline').exists()).toBe(true)
  })

  it('switches Leaf sources without losing either draft', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = 'version: 1\nrules: ["DOMAIN,private.example,DIRECT"]\n'
    const { model, wrapper } = mountEditor(config)
    await nextTick()

    const sourceSelector = wrapper.get<HTMLSelectElement>('[data-testid="leaf-source-selector"]')
    await sourceSelector.setValue('file')
    await nextTick()

    expect(wrapper.find('#policy_config_file').exists()).toBe(true)
    expect(model.policy_config_inline).toBe('')
    model.policy_config_file = '/etc/easytier/leaf-private.yaml'

    await sourceSelector.setValue('inline')
    await nextTick()
    expect(model.policy_config_file).toBe('')
    expect(model.policy_config_inline).toBe('version: 1\nrules: ["DOMAIN,private.example,DIRECT"]\n')

    await sourceSelector.setValue('file')
    await nextTick()
    expect(model.policy_config_file).toBe('/etc/easytier/leaf-private.yaml')
    expect(model.policy_config_inline).toBe('')
  })

  it('shows Mihomo lifecycle and the selected process-wide mesh entry', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'mihomo'
    config.policy_config_file = '/etc/easytier/mihomo.yaml'
    const runtimeInfo = {
      policy_runtime_running: true,
      mihomo_status: {
        state: 'running',
        pid: 321,
        restart_count: 3,
      },
      neutral_mesh_entry_status: {
        state: 'running',
        endpoint: '127.0.0.1:11081',
        selected_port: 11081,
        backend: 'hev-native',
        core_leases: 1,
        restart_count: 0,
      },
    } as NetworkInstanceRunningInfo

    const { wrapper } = mountEditor(config, undefined, { runtimeInfo })

    expect(wrapper.get('[data-testid="mihomo-runtime-state"]').text()).toContain('running')
    expect(wrapper.get('[data-testid="mihomo-restart-count"]').text()).toContain('3')
    expect(wrapper.get('[data-testid="mesh-entry-status"]').text())
      .toContain('127.0.0.1:11081')
    expect(wrapper.get('[data-testid="mesh-entry-status"]').text()).toContain('hev-native')
  })

  it('adds the managed mesh HEV actor with automatic port and UDP enabled', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    const { model, wrapper } = mountEditor(config)
    const addNode = wrapper.findAllComponents({ name: 'Button' })
      .find(button => button.props('label') === 'policy.editor.add_node')

    expect(addNode).toBeDefined()
    await addNode?.trigger('click')
    await nextTick()

    expect(model.policy_config_inline).toContain('proxy1:')
    expect(model.policy_config_inline).toContain('via: mesh')
    expect(model.policy_config_inline).toContain('udp: true')
    expect(model.policy_config_inline).not.toContain('port:')
  })

  it('edits Shadowsocks cipher and UoT without dropping protocol fields', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = `version: 1
proxies:
  ss:
    type: shadowsocks
    server: 203.0.113.10
    port: 8388
    via: native
    cipher: aes-256-gcm
    password: secret
    udp: native
rules: ["MATCH,ss"]
`
    const { model, wrapper } = mountEditor(config)

    await expandRow(wrapper, 'policy-proxy-edit-0')
    expect(wrapper.find<HTMLSelectElement>('[data-testid="policy-proxy-type-0"]').element.value).toBe('shadowsocks')
    await wrapper.find<HTMLSelectElement>('[data-testid="policy-proxy-udp-mode-0"]').setValue('uot-v2')
    await nextTick()

    expect(model.policy_config_inline).toContain('type: shadowsocks')
    expect(model.policy_config_inline).toContain('cipher: aes-256-gcm')
    expect(model.policy_config_inline).toContain('password: secret')
    expect(model.policy_config_inline).toContain('udp: uot-v2')
    expect(model.policy_config_inline).toContain('via: native')
  })

  it('edits VMess WebSocket and TLS fields without changing group composition', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = `version: 1
proxies:
  vmess:
    type: vmess
    server: edge.example
    port: 443
    uuid: 00000000-0000-0000-0000-000000000001
    alter-id: 0
    cipher: auto
    transport: { type: websocket, path: /vmess, headers: { Host: cdn.example } }
    tls: { server-name: cdn.example }
groups:
  through-mesh: { type: chain, members: [mesh-exit, vmess] }
rules: ["MATCH,through-mesh"]
`
    const { model, wrapper } = mountEditor(config)

    await expandRow(wrapper, 'policy-proxy-edit-0')
    expect(wrapper.find<HTMLSelectElement>('[data-testid="policy-proxy-type-0"]').element.value).toBe('vmess')
    expect(wrapper.text()).toContain('policy.editor.websocket_path')
    await nextTick()

    expect(model.policy_config_inline).toContain('type: vmess')
    expect(model.policy_config_inline).toContain('cipher: auto')
    expect(model.policy_config_inline).toContain('path: /vmess')
    expect(model.policy_config_inline).toContain('server-name: cdn.example')
    expect(model.policy_config_inline).toContain('members:')
    expect(model.policy_config_inline).toContain('mesh-exit')
  })

  it('edits direct and proxy DNS sets without losing ordered policy rules', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = `version: 1
dns:
  direct: [223.5.5.5]
  proxy: ["doh:cloudflare-dns.com@1.1.1.1"]
rules: ["MATCH,DIRECT"]
`
    const { model, wrapper } = mountEditor(config)

    await wrapper.find<HTMLTextAreaElement>('#policy_dns_direct').setValue('223.5.5.5\ndoh:dns.alidns.com@223.5.5.5')
    await wrapper.find<HTMLTextAreaElement>('#policy_dns_proxy').setValue('doh:dns.google@8.8.8.8')
    await nextTick()

    expect(model.policy_config_inline).toContain('direct:')
    expect(model.policy_config_inline).toContain('223.5.5.5')
    expect(model.policy_config_inline).toContain('doh:dns.alidns.com@223.5.5.5')
    expect(model.policy_config_inline).toContain('doh:dns.google@8.8.8.8')
    expect(model.policy_config_inline).toContain('MATCH,DIRECT')
  })

  it('keeps DNS and groups visible without an experimental feature gate', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    const { model, wrapper } = mountEditor(config)
    expect(wrapper.find('[data-header="policy.editor.dns"]').exists()).toBe(true)
    expect(wrapper.find('[data-header="policy.editor.groups"]').exists()).toBe(true)
    expect(wrapper.find('#policy_advanced_features').exists()).toBe(false)
  })

  it('keeps existing advanced documents visible and byte-stable', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = `version: 1
proxies:
  exit:
    type: socks5
    server: 192.0.2.10
    port: 1080
    via: native
    udp: true
groups:
  preferred:
    type: fallback
    members: [exit, DIRECT]
rules: ["MATCH,preferred"]
`
    const { model, wrapper } = mountEditor(config)
    const serialized = model.policy_config_inline

    expect(wrapper.find('[data-header="policy.editor.groups"]').exists()).toBe(true)
    await nextTick()

    expect(model.policy_config_inline).toBe(serialized)
    expect(model.policy_config_inline).toContain('type: fallback')
    expect(model.policy_config_inline).toContain('via: native')
    expect(model.policy_config_inline).toContain('udp: true')
  })

  it('keeps the Android keyboard target mounted while a proxy name changes', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = `version: 1
proxies:
  exit:
    type: socks5
    server: { virtual-ip: 10.44.0.8 }
    via: mesh
rules: ["MATCH,exit"]
`
    const { model, wrapper } = mountEditor(config)
    await expandRow(wrapper, 'policy-proxy-edit-0')
    const input = wrapper.find<HTMLInputElement>('[data-testid="policy-proxy-name-0"]')
    const originalElement = input.element

    await input.setValue('renamed-exit')
    await nextTick()

    expect(wrapper.find('[data-testid="policy-proxy-name-0"]').element).toBe(originalElement)
    expect(model.policy_config_inline).toContain('renamed-exit:')
  })

  it('enables no-resolve when a custom rule changes to an IP-based kind', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = 'version: 1\nrules: ["DOMAIN,example.com,DIRECT"]\n'
    const { model, wrapper } = mountEditor(config)

    await expandRow(wrapper, 'policy-rule-edit-0')
    await wrapper.find<HTMLSelectElement>('[data-testid="policy-rule-type-0"]').setValue('GEOIP')
    await nextTick()

    expect(model.policy_config_inline).toContain('GEOIP,example.com,DIRECT,no-resolve')
  })

  it('does not overwrite invalid advanced YAML with the last visual document', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    const { model, wrapper } = mountEditor(config)

    wrapper.getComponent({ name: 'YamlCodeEditor' })
      .vm.$emit('update:modelValue', 'version: [')
    await nextTick()

    expect(model.policy_config_inline).toBe('version: [')
    expect(wrapper.text()).toContain('policy.editor.yaml_error')
    expect(wrapper.find('[data-header="policy.editor.nodes"]').exists()).toBe(false)
  })

  it('selects the recommended desktop outbound interface instead of requiring text input', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    const api = {
      list_policy_outbound_interfaces: vi.fn(async () => ({
        platform: 'linux',
        required: true,
        supported: true,
        interfaces: [
          { name: 'eth1', addresses: ['192.0.2.2/24'], recommended: false },
          { name: 'eth0', addresses: ['192.0.2.1/24'], recommended: true },
        ],
      })),
    } as unknown as import('../src/modules/api').RemoteClient

    const { model } = mountEditor(config, api)
    await flushPromises()

    expect(api.list_policy_outbound_interfaces).toHaveBeenCalledOnce()
    expect(model.policy_outbound_interface).toBe('eth0')
  })

  it('treats protobuf-omitted address arrays as empty for macOS interfaces', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    const api = {
      list_policy_outbound_interfaces: vi.fn(async () => ({
        platform: 'macos',
        required: true,
        supported: true,
        interfaces: [
          {
            name: 'en0',
            addresses: ['192.168.234.162/24'],
            recommended: true,
          },
          { name: 'bridge0' },
          { name: 'en1' },
        ],
      })),
    } as unknown as import('../src/modules/api').RemoteClient

    const { model, wrapper } = mountEditor(config, api)
    await flushPromises()

    expect(model.policy_outbound_interface).toBe('en0')
    expect(
      wrapper.findAll<HTMLOptionElement>('#policy_outbound_interface option')
        .map(option => option.element.value),
    ).toEqual(['en0', 'bridge0', 'en1'])
  })

  it('does not request an outbound interface on Android policy mode', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    const api = {
      list_policy_outbound_interfaces: vi.fn(async () => ({
        platform: 'android',
        required: false,
        supported: true,
        interfaces: [],
      })),
    } as unknown as import('../src/modules/api').RemoteClient

    const { model, wrapper } = mountEditor(config, api)
    await flushPromises()

    expect(model.policy_outbound_interface).toBe('')
    expect(wrapper.text()).toContain('policy.editor.outbound_automatic')
    expect(wrapper.text()).toContain('policy.editor.runtime_android_experimental')
  })

  it('shows an available Windows runtime when the backend reports support', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    const api = {
      list_policy_outbound_interfaces: vi.fn(async () => ({
        platform: 'windows',
        required: true,
        supported: true,
        interfaces: [{ name: 'Ethernet', addresses: ['192.0.2.10/24'], recommended: true }],
      })),
    } as unknown as import('../src/modules/api').RemoteClient
    const { model, wrapper } = mountEditor(config, api)
    await flushPromises()

    expect(model.policy_outbound_interface).toBe('auto')
    expect(
      wrapper.findAll<HTMLOptionElement>('#policy_outbound_interface option')
        .map(option => option.element.value),
    ).toEqual(['auto', 'Ethernet'])
    expect(wrapper.text()).toContain('policy.editor.runtime_windows_supported')
      expect(wrapper.find<HTMLOptionElement>('option[value="leaf"]').element.disabled).toBe(false)
  })

  it('shows partial macOS and unavailable Windows runtime status even while disabled', async () => {
    for (const [platform, key] of [
      ['darwin', 'policy.editor.runtime_macos_partial'],
      ['windows', 'policy.editor.runtime_windows_unsupported'],
    ] as const) {
      const api = {
        list_policy_outbound_interfaces: vi.fn(async () => ({
          platform,
          required: false,
          supported: false,
          interfaces: [],
        })),
      } as unknown as import('../src/modules/api').RemoteClient
      const { wrapper } = mountEditor(DEFAULT_NETWORK_CONFIG(), api)
      await flushPromises()

      expect(wrapper.text()).toContain(key)
      expect(wrapper.find<HTMLOptionElement>('option[value="leaf"]').element.disabled).toBe(true)
      wrapper.unmount()
    }
  })

  it('applies the GeoSite and GeoIP preset without serializing resource paths', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    const { model, wrapper } = mountEditor(config)
    const preset = wrapper.findAllComponents({ name: 'Button' })
      .find(button => button.props('label') === 'policy.editor.preset_china_direct')

    expect(preset).toBeDefined()
    expect(preset?.attributes('disabled')).toBeUndefined()
    await preset?.trigger('click')
    await nextTick()

    expect(model.policy_config_inline).toContain('GEOSITE,CN,DIRECT')
    expect(model.policy_config_inline).toContain('GEOIP,CN,DIRECT,no-resolve')
    expect(model.policy_config_inline).not.toContain('rule-sets:')
    expect(model.policy_config_inline).not.toContain('sha256:')
  })

  it('offers sources and manual updates for all Geo resources and accepts empty MMDB categories', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    const update = vi.fn(async (_instanceId: string, resource: Api.PolicyRuleDataResource) => ({
      path: `/managed/${resource}`,
      sha256: 'd'.repeat(64),
      size: 4096,
      source_url: `https://example.invalid/${resource}`,
      updated: true,
    }))
    const api = {
      update_policy_rule_data: update,
      list_policy_rule_data_categories: vi.fn(async (_instanceId: string, resource: Api.PolicyRuleDataResource) => ({
        resource,
        sha256: resource === 'geosite' ? 'a'.repeat(64) : 'b'.repeat(64),
        size: 1024,
        categories: resource === 'geosite' ? ['cn'] : ['cn'],
      })),
    } as unknown as Api.RemoteClient
    const { model, wrapper } = mountEditor(config, api)
    await flushPromises()

    for (const resource of ['geosite', 'geoip', 'mmdb']) {
      const card = wrapper.get(`[data-testid="policy-rule-data-${resource}"]`)
      expect(card.find('input').exists()).toBe(true)
      expect(card.find(`[data-testid="policy-rule-data-update-${resource}"]`).exists()).toBe(true)
    }
    expect(wrapper.get('[data-testid="policy-rule-data-geosite"]').text()).toContain('a'.repeat(64))
    expect(wrapper.get('[data-testid="policy-rule-data-geoip"]').text()).toContain('b'.repeat(64))

    await wrapper.get('[data-testid="policy-rule-data-update-mmdb"]').trigger('click')
    await flushPromises()

    expect(update).toHaveBeenCalledWith(config.instance_id, 'mmdb', undefined)
    expect(wrapper.text()).toContain('policy.editor.rule_data_updated')
    expect(model.policy_config_inline).toContain('type: mmdb')
    expect(model.policy_config_inline).toContain('/managed/mmdb')
    expect(model.policy_config_inline).toContain('d'.repeat(64))
  })

  it('offers searchable GeoSite categories returned by normal GUI mode', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = 'version: 1\nrules: ["GEOSITE,CN,DIRECT"]\n'
    const api = {
      list_policy_rule_data_categories: vi.fn(async (_instanceId: string, resource: Api.PolicyRuleDataResource) => ({
        resource,
        sha256: resource === 'geosite' ? 'a'.repeat(64) : 'b'.repeat(64),
        size: 1024,
        categories: resource === 'geosite' ? ['cn', 'github', 'google'] : ['cn'],
      })),
    } as unknown as Api.RemoteClient
    const { wrapper } = mountEditor(config, api)
    await flushPromises()

    await expandRow(wrapper, 'policy-rule-edit-0')

    const category = wrapper.get('[data-testid="policy-rule-category-0"]')
    expect(category.attributes('filter')).toBeDefined()
    expect(
      category.findAll<HTMLOptionElement>('option').map(option => option.element.value),
    ).toEqual(['CN', 'GITHUB', 'GOOGLE'])
  })

  it('reports an unchanged remote size without replacing saved rule data', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = `version: 1
rule-sets:
  country:
    type: mmdb
    path: /managed/country-lite.mmdb
    sha256: ${'e'.repeat(64)}
rules: ["MATCH,DIRECT"]
`
    const api = {
      update_policy_rule_data: vi.fn(async () => ({
        path: '/managed/country-lite.mmdb',
        sha256: 'e'.repeat(64),
        size: 4096,
        source_url: MANAGED_RULE_DATA.mmdb.source,
        updated: false,
      })),
    } as unknown as Api.RemoteClient
    const { model, wrapper } = mountEditor(config, api)
    const before = model.policy_config_inline

    await wrapper.get('[data-testid="policy-rule-data-update-mmdb"]').trigger('click')
    await flushPromises()

    expect(wrapper.text()).toContain('policy.editor.rule_data_unchanged')
    expect(model.policy_config_inline).toBe(before)
    expect(wrapper.get('[data-testid="policy-rule-data-mmdb"]').text()).toContain('e'.repeat(64))
  })

  it('adds a verified existing file to YAML when the remote size is unchanged', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    const api = {
      update_policy_rule_data: vi.fn(async () => ({
        path: '/managed/country-lite.mmdb',
        sha256: 'f'.repeat(64),
        size: 4096,
        source_url: MANAGED_RULE_DATA.mmdb.source,
        updated: false,
      })),
    } as unknown as Api.RemoteClient
    const { model, wrapper } = mountEditor(config, api)

    await wrapper.get('[data-testid="policy-rule-data-update-mmdb"]').trigger('click')
    await flushPromises()

    expect(wrapper.text()).toContain('policy.editor.rule_data_unchanged')
    expect(model.policy_config_inline).toContain('/managed/country-lite.mmdb')
    expect(model.policy_config_inline).toContain('f'.repeat(64))
  })

  it('keeps existing node, group, and rule cards compact until Edit is selected', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = `version: 1
proxies:
  exit:
    type: socks5
    server: 192.0.2.10
    port: 1080
    udp: true
groups:
  preferred: { type: fallback, members: [exit, DIRECT] }
rules: ["MATCH,preferred"]
`
    const { wrapper } = mountEditor(config)

    expect(wrapper.find('[data-testid="policy-proxy-name-0"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="policy-group-name-0"]').exists()).toBe(false)
    expect(wrapper.find('[data-testid="policy-rule-type-0"]').exists()).toBe(false)
    expect(wrapper.text()).toContain('192.0.2.10')
    expect(wrapper.text()).toContain('exit -> DIRECT')
    expect(wrapper.text()).toContain('MATCH -> preferred')

    await expandRow(wrapper, 'policy-proxy-edit-0')
    const typeOptions = wrapper.find<HTMLSelectElement>('[data-testid="policy-proxy-type-0"]')
      .findAll('option').map(option => option.attributes('value'))
    expect(typeOptions).toEqual(['socks5', 'shadowsocks', 'trojan', 'vmess', 'vless'])
    expect(typeOptions).not.toContain('http')
  })

  it('edits both FakeIP pools in the visual DNS menu', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_proxy_backend = 'leaf'
    config.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    const { model, wrapper } = mountEditor(config)

    await wrapper.get<HTMLInputElement>('#policy_fake_ip_range').setValue('198.19.64.0/22')
    await wrapper.get<HTMLInputElement>('#policy_fake_ip_range6').setValue('fd12:3456:789a::/112')
    await nextTick()

    expect(model.policy_config_inline).toContain('fake-ip-range: 198.19.64.0/22')
    expect(model.policy_config_inline).toContain('fake-ip-range6: fd12:3456:789a::/112')
  })
})
