import { mount, type VueWrapper } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h, nextTick, reactive } from 'vue'
import Config from '../src/components/Config.vue'
import {
  DEFAULT_NETWORK_CONFIG,
  DEFAULT_STEALTH_PROTOCOLS,
  DEFAULT_TRANSPORT_PRIORITY,
  DEFAULT_UNDERLAY_EXCLUDE_CIDRS,
  toBackendNetworkConfig,
  type NetworkConfig,
} from '../src/types/network'

const CONFIG_FLAG_FIELDS = [
  'latency_first',
  'use_smoltcp',
  'disable_ipv6',
  'ipv6_public_addr_auto',
  'enable_kcp_proxy',
  'disable_kcp_input',
  'enable_quic_proxy',
  'disable_quic_input',
  'disable_p2p',
  'p2p_only',
  'lazy_p2p',
  'bind_device',
  'no_tun',
  'enable_exit_node',
  'relay_all_peer_rpc',
  'need_p2p',
  'multi_thread',
  'proxy_forward_by_system',
  'disable_encryption',
  'disable_tcp_hole_punching',
  'disable_udp_hole_punching',
  'stealth_mode',
  'disable_legacy_udp_hole_punch',
  'underlay_candidate_guard',
  'enable_policy_proxy',
  'enable_udp_broadcast_relay',
  'disable_upnp',
  'disable_sym_hole_punching',
  'enable_magic_dns',
  'enable_private_mode',
] as const satisfies readonly (keyof NetworkConfig)[]

const CONFIG_CHECKBOX_FIELDS = [
  ['dhcp', '#virtual_ip_auto'],
  ...CONFIG_FLAG_FIELDS.map((field) => [field, `#${field}`] as const),
] as const satisfies readonly (readonly [keyof NetworkConfig, string])[]

const CONFIG_TOGGLE_FIELDS = [
  'enable_vpn_portal',
  'enable_relay_network_whitelist',
  'enable_manual_routes',
  'enable_socks5',
] as const satisfies readonly (keyof NetworkConfig)[]

const CONFIG_UI_BOOLEAN_FIELDS = [
  ...CONFIG_CHECKBOX_FIELDS.map(([field]) => field),
  ...CONFIG_TOGGLE_FIELDS,
] as const satisfies readonly (keyof NetworkConfig)[]

vi.mock('vue-i18n', () => ({
  useI18n: () => ({
    t: (key: string, values?: unknown[]) => values ? `${key}:${values.join(',')}` : key,
  }),
}))

const PassThrough = defineComponent({
  name: 'PassThrough',
  setup(_, { slots }) {
    return () => h('div', slots.default?.())
  },
})

const PanelStub = defineComponent({
  name: 'Panel',
  props: {
    header: String,
  },
  setup(props, { slots }) {
    return () => h('section', { 'data-stub': 'panel', 'data-header': props.header }, slots.default?.())
  },
})

const DividerStub = defineComponent({
  name: 'Divider',
  setup() {
    return () => h('hr', { 'data-stub': 'divider' })
  },
})

function splitList(value: string): string[] {
  return value.split(',').map((item) => item.trim()).filter((item) => item.length > 0)
}

const InputTextStub = defineComponent({
  name: 'InputText',
  props: {
    modelValue: [String, Number],
    id: String,
    disabled: Boolean,
  },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('input', {
      ...attrs,
      id: props.id,
      disabled: props.disabled,
      value: props.modelValue ?? '',
      'data-stub': 'input-text',
      onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
    })
  },
})

const TextareaStub = defineComponent({
  name: 'Textarea',
  props: {
    modelValue: String,
    id: String,
  },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('textarea', {
      ...attrs,
      id: props.id,
      value: props.modelValue ?? '',
      'data-stub': 'textarea',
      onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLTextAreaElement).value),
    })
  },
})

const PasswordStub = defineComponent({
  name: 'Password',
  props: {
    modelValue: [String, Number],
    id: String,
    disabled: Boolean,
  },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('input', {
      ...attrs,
      id: props.id,
      disabled: props.disabled,
      type: 'password',
      value: props.modelValue ?? '',
      'data-stub': 'password',
      onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
    })
  },
})

const InputNumberStub = defineComponent({
  name: 'InputNumber',
  props: {
    modelValue: Number,
    id: String,
    inputId: String,
    disabled: Boolean,
  },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('input', {
      ...attrs,
      id: props.id ?? props.inputId,
      disabled: props.disabled,
      type: 'number',
      value: props.modelValue ?? '',
      'data-stub': 'input-number',
      onInput: (event: Event) => {
        const value = (event.target as HTMLInputElement).value
        emit('update:modelValue', value === '' ? null : Number(value))
      },
    })
  },
})

const CheckboxStub = defineComponent({
  name: 'Checkbox',
  props: {
    modelValue: Boolean,
    inputId: String,
  },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('input', {
      ...attrs,
      id: props.inputId,
      checked: props.modelValue,
      type: 'checkbox',
      'data-stub': 'checkbox',
      onChange: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).checked),
    })
  },
})

const ToggleButtonStub = defineComponent({
  name: 'ToggleButton',
  props: {
    modelValue: Boolean,
    onIcon: String,
    offIcon: String,
    onLabel: String,
    offLabel: String,
  },
  emits: ['update:modelValue'],
  setup(props, { emit }) {
    return () => h('button', {
      type: 'button',
      'aria-pressed': String(Boolean(props.modelValue)),
      'data-stub': 'toggle-button',
      onClick: () => emit('update:modelValue', !props.modelValue),
    }, props.modelValue ? props.onLabel : props.offLabel)
  },
})

const AutoCompleteStub = defineComponent({
  name: 'AutoComplete',
  props: {
    modelValue: Array,
    id: String,
    multiple: Boolean,
  },
  emits: ['update:modelValue', 'complete'],
  setup(props, { attrs, emit }) {
    return () => h('input', {
      ...attrs,
      id: props.id,
      value: (props.modelValue ?? []).join(','),
      'data-stub': 'auto-complete',
      onInput: (event: Event) => emit('update:modelValue', splitList((event.target as HTMLInputElement).value)),
    })
  },
})

const UrlListInputStub = defineComponent({
  name: 'UrlListInput',
  props: {
    modelValue: Array,
    id: String,
    addLabel: String,
  },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('input', {
      ...attrs,
      id: props.id,
      value: (props.modelValue ?? []).join(','),
      'data-stub': 'url-list-input',
      'data-add-label': props.addLabel,
      onInput: (event: Event) => emit('update:modelValue', splitList((event.target as HTMLInputElement).value)),
    })
  },
})

const SelectButtonStub = defineComponent({
  name: 'SelectButton',
  props: {
    modelValue: String,
    options: Array,
  },
  emits: ['update:modelValue'],
  setup(props, { emit }) {
    return () => h('select', {
      value: props.modelValue,
      'data-stub': 'select-button',
      onChange: (event: Event) => emit('update:modelValue', (event.target as HTMLSelectElement).value),
    }, (props.options ?? []).map((option) => h('option', { value: option as string }, option as string)))
  },
})

const ButtonStub = defineComponent({
  name: 'Button',
  props: {
    label: String,
    icon: String,
    disabled: Boolean,
  },
  emits: ['click'],
  setup(props, { slots, emit }) {
    return () => h('button', {
      type: 'button',
      disabled: props.disabled,
      'data-label': props.label ?? props.icon,
      onClick: (event: MouseEvent) => emit('click', event),
    }, slots.default?.() ?? props.label ?? props.icon)
  },
})

const DialogStub = defineComponent({
  name: 'Dialog',
  props: {
    visible: Boolean,
  },
  setup(props, { slots }) {
    return () => h('div', { hidden: !props.visible, 'data-stub': 'dialog' }, [
      slots.default?.(),
      slots.footer?.(),
    ])
  },
})

const AclManagerStub = defineComponent({
  name: 'AclManager',
  props: {
    modelValue: Object,
  },
  emits: ['update:modelValue'],
  setup(props) {
    return () => h('pre', { 'data-stub': 'acl-manager' }, JSON.stringify(props.modelValue))
  },
})

const PolicyEditorStub = defineComponent({
  name: 'PolicyEditor',
  props: {
    modelValue: { type: Object, required: true },
  },
  setup(props) {
    const config = props.modelValue as NetworkConfig
    return () => h('div', { 'data-stub': 'policy-editor' }, [
      h('input', {
        id: 'enable_policy_proxy',
        type: 'checkbox',
        checked: config.enable_policy_proxy,
        onChange: (event: Event) => {
          config.enable_policy_proxy = (event.target as HTMLInputElement).checked
        },
      }),
      h('input', {
        id: 'policy_config_file',
        value: config.policy_config_file ?? '',
        onInput: (event: Event) => {
          config.policy_config_file = (event.target as HTMLInputElement).value
          if (config.policy_config_file.trim()) config.policy_config_inline = ''
        },
      }),
      h('textarea', {
        id: 'policy_config_inline',
        value: config.policy_config_inline ?? '',
        onInput: (event: Event) => {
          config.policy_config_inline = (event.target as HTMLTextAreaElement).value
          if (config.policy_config_inline.trim()) config.policy_config_file = ''
        },
      }),
    ])
  },
})

function makeConfig(): NetworkConfig {
  const config = DEFAULT_NETWORK_CONFIG()

  return {
    ...config,
    dhcp: false,
    virtual_ipv4: '10.1.2.3',
    network_length: 24,
    network_name: 'mesh-a',
    network_secret: 'secret-a',
    peer_urls: ['tcp://peer-a:11010', 'udp://peer-b:11010'],
    latency_first: true,
    use_smoltcp: true,
    disable_ipv6: true,
    no_tun: true,
    hostname: 'host-a',
    proxy_cidrs: ['10.10.0.0/16', '172.16.1.0/24'],
    enable_vpn_portal: true,
    vpn_portal_client_network_addr: '10.144.0.0',
    vpn_portal_listen_port: 22023,
    listener_urls: ['tcp://0.0.0.0:12010'],
    dev_name: 'tun-test',
    underlay_exclude_cidrs: '198.18.0.0/15,192.19.0.0/24',
    mtu: 1280,
    instance_recv_bps_limit: '9007199254740993',
    enable_relay_network_whitelist: true,
    relay_network_whitelist: ['network-a'],
    enable_manual_routes: true,
    routes: ['192.168.0.0/16'],
    enable_socks5: true,
    socks5_port: 1086,
    exit_nodes: ['exit-a'],
    mapped_listeners: ['tcp://127.0.0.1:22000'],
    port_forwards: [{
      proto: 'udp',
      bind_ip: '0.0.0.0',
      bind_port: 18080,
      dst_ip: '10.0.0.2',
      dst_port: 8080,
    }],
  }
}

function mountConfig(config: NetworkConfig = makeConfig()) {
  const curNetwork = reactive(config) as NetworkConfig
  const wrapper = mount(Config, {
    props: {
      curNetwork,
      hostname: 'host-from-prop',
    },
    global: {
      directives: {
        tooltip: () => {},
      },
      stubs: {
        AclManager: AclManagerStub,
        AutoComplete: AutoCompleteStub,
        Button: ButtonStub,
        Checkbox: CheckboxStub,
        Dialog: DialogStub,
        Divider: DividerStub,
        InputGroup: PassThrough,
        InputGroupAddon: PassThrough,
        InputNumber: InputNumberStub,
        InputText: InputTextStub,
        Panel: PanelStub,
        Password: PasswordStub,
        PolicyEditor: PolicyEditorStub,
        SelectButton: SelectButtonStub,
        Textarea: TextareaStub,
        ToggleButton: ToggleButtonStub,
        UrlListInput: UrlListInputStub,
      },
    },
  })

  return { curNetwork, wrapper }
}

function input(wrapper: VueWrapper, selector: string): HTMLInputElement {
  return wrapper.find(selector).element as HTMLInputElement
}

async function setInput(wrapper: VueWrapper, selector: string, value: string) {
  await wrapper.find(selector).setValue(value)
  await nextTick()
}

describe('Config.vue network config projection', () => {
  it('uses the stealth and transport defaults for new networks', () => {
    const config = DEFAULT_NETWORK_CONFIG()

    expect(config.stealth_mode).toBe(true)
    expect(config.stealth_protocols).toBe(DEFAULT_STEALTH_PROTOCOLS)
    expect(config.transport_priority).toBe(DEFAULT_TRANSPORT_PRIORITY)
    expect(config.enable_policy_proxy).toBe(false)
  })

  it('round-trips policy settings and keeps file and inline sources exclusive', async () => {
    const config = makeConfig()
    config.enable_policy_proxy = true
    config.policy_config_file = 'policy/default.yaml'
    config.policy_outbound_interface = 'eth0'
    config.policy_leaf_executable = 'easytier-leaf-worker'
    const { curNetwork, wrapper } = mountConfig(config)
    await nextTick()

    expect(input(wrapper, '#policy_config_file').value).toBe('policy/default.yaml')
    await wrapper.find<HTMLTextAreaElement>('#policy_config_inline').setValue(
      'version: 1\nrules: ["FINAL,DIRECT"]\n',
    )
    await nextTick()
    expect(curNetwork.policy_config_file).toBe('')
    expect(curNetwork.policy_config_inline).toContain('FINAL,DIRECT')

    const backend = toBackendNetworkConfig(curNetwork)
    expect(backend).toMatchObject({
      enable_policy_proxy: true,
      policy_config_file: '',
      policy_config_inline: 'version: 1\nrules: ["FINAL,DIRECT"]\n',
      policy_outbound_interface: 'eth0',
      policy_leaf_executable: 'easytier-leaf-worker',
    })
  })

  it('round-trips the Leaf packet batch experiment without dropping unknown features', async () => {
    const config = makeConfig()
    config.experimental_features = ['future-feature']
    const { curNetwork, wrapper } = mountConfig(config)
    await nextTick()

    const checkbox = wrapper.find('#leaf-packet-batch')
    expect(input(wrapper, '#leaf-packet-batch').checked).toBe(false)
    await checkbox.setValue(true)
    await nextTick()
    expect(curNetwork.experimental_features).toEqual(['future-feature', 'leaf-packet-batch'])
    expect(toBackendNetworkConfig(curNetwork).experimental_features)
      .toEqual(['future-feature', 'leaf-packet-batch'])

    await checkbox.setValue(false)
    await nextTick()
    expect(curNetwork.experimental_features).toEqual(['future-feature'])
  })

  it('disables and visually clears stealth while the network secret is empty', async () => {
    const config = DEFAULT_NETWORK_CONFIG()
    const { curNetwork, wrapper } = mountConfig(config)
    await nextTick()

    const stealth = input(wrapper, '#stealth_mode')
    expect(stealth.disabled).toBe(true)
    expect(stealth.checked).toBe(false)
    expect(curNetwork.stealth_mode).toBe(true)

    await setInput(wrapper, '#network_secret', 'secret')
    expect(input(wrapper, '#stealth_mode').disabled).toBe(false)
    expect(input(wrapper, '#stealth_mode').checked).toBe(true)
  })

  it('projects config values into the visible form controls', async () => {
    const { curNetwork, wrapper } = mountConfig()
    await nextTick()

    expect(input(wrapper, '#network_name').value).toBe('mesh-a')
    expect(input(wrapper, '#network_secret').value).toBe('secret-a')
    expect(input(wrapper, '#virtual_ip').value).toBe('10.1.2.3')
    expect(input(wrapper, '#initial_nodes').value).toBe('tcp://peer-a:11010,udp://peer-b:11010')
    expect(input(wrapper, '#virtual_ip_auto').checked).toBe(false)
    expect(input(wrapper, '#latency_first').checked).toBe(true)
    expect(input(wrapper, '#use_smoltcp').checked).toBe(true)
    expect(input(wrapper, '#disable_ipv6').checked).toBe(true)
    expect(input(wrapper, '#no_tun').checked).toBe(true)

    expect(input(wrapper, '#hostname').value).toBe('host-a')
    expect(input(wrapper, '#subnet-proxy').value).toBe('10.10.0.0/16,172.16.1.0/24')
    expect(input(wrapper, 'input[placeholder="vpn_portal_client_network"]').value).toBe('10.144.0.0')
    expect(input(wrapper, '#dev_name').value).toBe('tun-test')
    expect(input(wrapper, '#underlay_exclude_cidrs').value).toBe('198.18.0.0/15,192.19.0.0/24')
    expect(input(wrapper, '#underlay_exclude_cidrs').placeholder).toBe(DEFAULT_UNDERLAY_EXCLUDE_CIDRS)
    expect(input(wrapper, '#mtu').value).toBe('1280')
    expect(input(wrapper, '#instance_recv_bps_limit').value).toBe('9007199254740993')
    expect(input(wrapper, '#relay_network_whitelist').value).toBe('network-a')
    expect(input(wrapper, '#routes').value).toBe('192.168.0.0/16')
    expect(input(wrapper, '#socks5_port').value).toBe('1086')
    expect(input(wrapper, '#exit_nodes').value).toBe('exit-a')
    expect(input(wrapper, 'input[data-add-label="add_listener_url"]').value).toBe('tcp://0.0.0.0:12010')
    expect(input(wrapper, 'input[data-add-label="add_mapped_listener"]').value).toBe('tcp://127.0.0.1:22000')

    expect(wrapper.find<HTMLSelectElement>('select[data-stub="select-button"]').element.value).toBe('udp')
    expect(input(wrapper, 'input[placeholder="port_forwards_bind_addr"]').value).toBe('0.0.0.0')
    expect(input(wrapper, 'input[placeholder="port_forwards_dst_addr"]').value).toBe('10.0.0.2')
    expect(wrapper.findComponent(AclManagerStub).props('modelValue')).toStrictEqual(curNetwork.acl)
  })

  it('projects form edits back into config and backend JSON', async () => {
    const { curNetwork, wrapper } = mountConfig()
    await nextTick()

    await wrapper.find('#virtual_ip_auto').setValue(false)
    await setInput(wrapper, '#network_name', 'mesh-edited')
    await setInput(wrapper, '#network_secret', 'secret-edited')
    await setInput(wrapper, '#virtual_ip', '10.7.7.7')
    await setInput(wrapper, '#initial_nodes', ' tcp://peer-x:11010, , udp://peer-y:11010 ')
    await wrapper.find('#no_tun').setValue(false)
    await wrapper.find('#disable_ipv6').setValue(false)
    await setInput(wrapper, '#hostname', 'host-edited')
    await setInput(wrapper, '#subnet-proxy', '10.7.0.0/16,172.17.0.0/16')
    await setInput(wrapper, 'input[placeholder="vpn_portal_client_network"]', '10.200.0.0')
    await setInput(wrapper, 'input[data-add-label="add_listener_url"]', 'tcp://0.0.0.0:13010')
    await setInput(wrapper, '#dev_name', 'tun-edited')
    await setInput(wrapper, '#underlay_exclude_cidrs', '198.18.0.0/15,fdfe:dcba:9876::/48')
    await setInput(wrapper, '#mtu', '1260')
    await setInput(wrapper, '#instance_recv_bps_limit', '9007199254740993')
    await setInput(wrapper, '#relay_network_whitelist', 'network-edited')
    await setInput(wrapper, '#routes', '192.168.10.0/24')
    await setInput(wrapper, '#socks5_port', '1089')
    await setInput(wrapper, '#exit_nodes', 'exit-edited')
    await setInput(wrapper, 'input[data-add-label="add_mapped_listener"]', 'tcp://127.0.0.1:23000')
    await wrapper.find('select[data-stub="select-button"]').setValue('tcp')
    await setInput(wrapper, 'input[placeholder="port_forwards_bind_addr"]', '127.0.0.1')
    await setInput(wrapper, 'input[placeholder="port_forwards_dst_addr"]', '10.9.0.2')

    const portNumbers = wrapper.findAll<HTMLInputElement>('input#horizontal-buttons')
    await portNumbers[1].setValue('19090')
    await portNumbers[2].setValue('9090')

    expect(curNetwork).toMatchObject({
      dhcp: false,
      virtual_ipv4: '10.7.7.7',
      network_name: 'mesh-edited',
      network_secret: 'secret-edited',
      peer_urls: ['tcp://peer-x:11010', 'udp://peer-y:11010'],
      no_tun: false,
      disable_ipv6: false,
      hostname: 'host-edited',
      proxy_cidrs: ['10.7.0.0/16', '172.17.0.0/16'],
      vpn_portal_client_network_addr: '10.200.0.0',
      listener_urls: ['tcp://0.0.0.0:13010'],
      dev_name: 'tun-edited',
      underlay_exclude_cidrs: '198.18.0.0/15,fdfe:dcba:9876::/48',
      mtu: 1260,
      instance_recv_bps_limit: '9007199254740993',
      relay_network_whitelist: ['network-edited'],
      routes: ['192.168.10.0/24'],
      socks5_port: 1089,
      exit_nodes: ['exit-edited'],
      mapped_listeners: ['tcp://127.0.0.1:23000'],
      port_forwards: [{
        proto: 'tcp',
        bind_ip: '127.0.0.1',
        bind_port: 19090,
        dst_ip: '10.9.0.2',
        dst_port: 9090,
      }],
    })

    const backend = toBackendNetworkConfig(curNetwork)
    expect(backend).toMatchObject({
      virtual_ipv4: '10.7.7.7',
      network_name: 'mesh-edited',
      network_secret: 'secret-edited',
      peer_urls: ['tcp://peer-x:11010', 'udp://peer-y:11010'],
      listener_urls: ['tcp://0.0.0.0:13010'],
      underlay_exclude_cidrs: '198.18.0.0/15,fdfe:dcba:9876::/48',
      mtu: 1260,
      instance_recv_bps_limit: '9007199254740993',
      port_forwards: [{
        proto: 'tcp',
        bind_ip: '127.0.0.1',
        bind_port: 19090,
        dst_ip: '10.9.0.2',
        dst_port: 9090,
      }],
    })
  })

  it('round-trips every visible boolean config control into backend JSON', async () => {
    const config = makeConfig()
    const originalFlagValues = new Map(
      CONFIG_UI_BOOLEAN_FIELDS.map((field, index) => {
        const value = index % 2 === 0
        config[field] = value
        return [field, value]
      }),
    )

    const { curNetwork, wrapper } = mountConfig(config)
    await nextTick()

    for (const [field, selector] of CONFIG_CHECKBOX_FIELDS) {
      const value = originalFlagValues.get(field)
      expect(input(wrapper, selector).checked, `${field} should project into UI`).toBe(value)
      await wrapper.find(selector).setValue(!value)
      await nextTick()
    }

    const toggleButtons = wrapper.findAll('button[data-stub="toggle-button"]')
    expect(toggleButtons).toHaveLength(CONFIG_TOGGLE_FIELDS.length)
    for (const [index, field] of CONFIG_TOGGLE_FIELDS.entries()) {
      const value = originalFlagValues.get(field)
      expect(toggleButtons[index].attributes('aria-pressed'), `${field} should project into UI`)
        .toBe(String(value))
      await toggleButtons[index].trigger('click')
      await nextTick()
    }

    const backend = toBackendNetworkConfig(curNetwork) as Record<string, unknown>
    for (const [field, value] of originalFlagValues) {
      const expectedValue = !value
      expect(curNetwork[field], `${field} should update config`).toBe(expectedValue)
      expect(backend[field], `${field} should be preserved in backend JSON`).toBe(expectedValue)
    }
  })

  it('keeps uint64 input editable without losing large values', async () => {
    const { curNetwork, wrapper } = mountConfig()
    await nextTick()

    await setInput(wrapper, '#instance_recv_bps_limit', '1234')
    expect(curNetwork.instance_recv_bps_limit).toBe(1234)

    await setInput(wrapper, '#instance_recv_bps_limit', 'not-a-number')
    expect(curNetwork.instance_recv_bps_limit).toBe(1234)

    await setInput(wrapper, '#instance_recv_bps_limit', '0')
    expect(curNetwork.instance_recv_bps_limit).toBeNull()
    expect(input(wrapper, '#instance_recv_bps_limit').value).toBe('')

    await setInput(wrapper, '#instance_recv_bps_limit', '9007199254740993')
    expect(curNetwork.instance_recv_bps_limit).toBe('9007199254740993')

    await setInput(wrapper, '#instance_recv_bps_limit', '18446744073709551616')
    expect(curNetwork.instance_recv_bps_limit).toBe('9007199254740993')

    await setInput(wrapper, '#instance_recv_bps_limit', '')
    expect(curNetwork.instance_recv_bps_limit).toBeNull()
  })

  it('emits runNetwork with the current projected config', async () => {
    const { curNetwork, wrapper } = mountConfig()
    await nextTick()

    await setInput(wrapper, '#network_name', 'mesh-running')
    await wrapper.find('button[data-label="run_network"]').trigger('click')

    expect(wrapper.emitted('runNetwork')?.[0]).toEqual([curNetwork])
    expect((wrapper.emitted('runNetwork')?.[0][0] as NetworkConfig).network_name).toBe('mesh-running')
  })
})
