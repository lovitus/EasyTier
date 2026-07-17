import { flushPromises, mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h, nextTick, reactive } from 'vue'
import PolicyEditor from '../src/components/policy/PolicyEditor.vue'
import { DEFAULT_NETWORK_CONFIG, type NetworkConfig } from '../src/types/network'

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
  props: { label: String, disabled: Boolean },
  emits: ['click'],
  setup(props, { emit }) {
    return () => h('button', {
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
  props: { modelValue: String, id: String },
  emits: ['update:modelValue'],
  setup(props, { emit }) {
    return () => h('textarea', {
      id: props.id,
      value: props.modelValue,
      onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLTextAreaElement).value),
    })
  },
})

const InputTextStub = defineComponent({
  name: 'InputText',
  inheritAttrs: false,
  props: { modelValue: String, id: String },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('input', {
      ...attrs,
      id: props.id,
      value: props.modelValue,
      onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
    })
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
    }, [
      h('option', { value: props.modelValue }, props.modelValue),
      h('option', { value: 'GEOIP' }, 'GEOIP'),
      h('option', { value: 'IP-CIDR' }, 'IP-CIDR'),
    ])
  },
})

function mountEditor(config: NetworkConfig, api?: import('../src/modules/api').RemoteClient) {
  const model = reactive(config) as NetworkConfig
  const wrapper = mount(PolicyEditor, {
    props: { modelValue: model, api },
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
        SelectButton: true,
        Textarea: TextareaStub,
      },
    },
  })
  return { model, wrapper }
}

describe('PolicyEditor', () => {
  it('keeps policy mode plugin-like and initializes a safe inline document only when enabled', async () => {
    const { model, wrapper } = mountEditor(DEFAULT_NETWORK_CONFIG())
    expect(model.enable_policy_proxy).toBe(false)
    expect(model.policy_config_inline).toBe('')
    expect(wrapper.find('[data-header="policy.editor.nodes"]').exists()).toBe(false)

    await wrapper.find<HTMLInputElement>('#enable_policy_proxy').setValue(true)
    await nextTick()

    expect(model.enable_policy_proxy).toBe(true)
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

  it('adds the managed mesh HEV actor with automatic port and UDP enabled', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
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

  it('edits direct and proxy DNS sets without losing ordered policy rules', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
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
    config.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    const { model, wrapper } = mountEditor(config)
    expect(wrapper.find('[data-header="policy.editor.dns"]').exists()).toBe(true)
    expect(wrapper.find('[data-header="policy.editor.groups"]').exists()).toBe(true)
    expect(wrapper.find('#policy_advanced_features').exists()).toBe(false)
  })

  it('keeps existing advanced documents visible and byte-stable', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
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
    config.policy_config_inline = `version: 1
proxies:
  exit:
    type: socks5
    server: { virtual-ip: 10.44.0.8 }
    via: mesh
rules: ["MATCH,exit"]
`
    const { model, wrapper } = mountEditor(config)
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
    config.policy_config_inline = 'version: 1\nrules: ["DOMAIN,example.com,DIRECT"]\n'
    const { model, wrapper } = mountEditor(config)

    await wrapper.find<HTMLSelectElement>('[data-testid="policy-rule-type-0"]').setValue('GEOIP')
    await nextTick()

    expect(model.policy_config_inline).toContain('GEOIP,example.com,DIRECT,no-resolve')
  })

  it('does not overwrite invalid advanced YAML with the last visual document', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
    config.policy_config_inline = 'version: 1\nrules: ["MATCH,DIRECT"]\n'
    const { model, wrapper } = mountEditor(config)

    await wrapper.find<HTMLTextAreaElement>('#policy_config_inline').setValue('version: [')
    await nextTick()

    expect(model.policy_config_inline).toBe('version: [')
    expect(wrapper.text()).toContain('policy.editor.yaml_error')
    expect(wrapper.find('[data-header="policy.editor.nodes"]').exists()).toBe(false)
  })

  it('selects the recommended desktop outbound interface instead of requiring text input', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
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

  it('does not request an outbound interface on Android policy mode', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
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
      expect(wrapper.find<HTMLInputElement>('#enable_policy_proxy').element.disabled).toBe(true)
      wrapper.unmount()
    }
  })

  it('applies the GeoSite and GeoIP preset without serializing resource paths', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    config.enable_policy_proxy = true
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
})
