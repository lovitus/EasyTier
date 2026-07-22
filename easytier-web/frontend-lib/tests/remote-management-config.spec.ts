import { flushPromises, mount } from '@vue/test-utils'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h, nextTick } from 'vue'
import RemoteManagement from '../src/components/RemoteManagement.vue'
import {
  DEFAULT_NETWORK_CONFIG,
  type NetworkConfig,
} from '../src/types/network'

const { toastAdd } = vi.hoisted(() => ({ toastAdd: vi.fn() }))

const BOOLEAN_CONFIG_FIELDS = [
  'dhcp',
  'enable_vpn_portal',
  'advanced_settings',
  'latency_first',
  'use_smoltcp',
  'disable_ipv6',
  'enable_kcp_proxy',
  'disable_kcp_input',
  'disable_p2p',
  'bind_device',
  'no_tun',
  'enable_exit_node',
  'relay_all_peer_rpc',
  'multi_thread',
  'enable_relay_network_whitelist',
  'enable_manual_routes',
  'proxy_forward_by_system',
  'disable_encryption',
  'enable_socks5',
  'disable_udp_hole_punching',
  'stealth_mode',
  'disable_legacy_udp_hole_punch',
  'underlay_candidate_guard',
  'enable_magic_dns',
  'enable_private_mode',
  'enable_quic_proxy',
  'disable_quic_input',
  'disable_sym_hole_punching',
  'p2p_only',
  'lazy_p2p',
  'need_p2p',
  'disable_upnp',
  'ipv6_public_addr_provider',
  'ipv6_public_addr_auto',
  'disable_relay_data',
  'enable_udp_broadcast_relay',
  'disable_tcp_hole_punching',
] as const satisfies readonly (keyof NetworkConfig)[]

vi.mock('vue-i18n', () => ({
  useI18n: () => ({
    t: (key: string) => key,
  }),
}))

vi.mock('primevue', async () => {
  const { defineComponent, h } = await import('vue')

  const PassThrough = defineComponent({
    name: 'PassThrough',
    props: {
      label: String,
      value: String,
    },
    setup(props, { slots }) {
      return () => h('div', {
        'data-label': props.label,
        'data-value': props.value,
        'data-stub': 'pass-through',
      }, slots.default?.())
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

  const SelectStub = defineComponent({
    name: 'Select',
    props: {
      modelValue: Object,
      options: Array,
    },
    emits: ['update:modelValue'],
    setup(props, { slots }) {
      return () => h('div', { 'data-stub': 'select' }, [
        slots.value?.({ value: props.modelValue, placeholder: '' }),
      ])
    },
  })

  const CheckboxStub = defineComponent({
    name: 'Checkbox',
    props: {
      modelValue: Boolean,
      disabled: Boolean,
    },
    emits: ['update:modelValue'],
    setup(props, { emit }) {
      return () => h('input', {
        type: 'checkbox',
        checked: props.modelValue,
        disabled: props.disabled,
        onChange: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).checked),
      })
    },
  })

  const DialogStub = defineComponent({
    name: 'Dialog',
    props: {
      visible: Boolean,
    },
    emits: ['update:visible'],
    setup(props, { slots }) {
      return () => props.visible
        ? h('div', { 'data-stub': 'dialog' }, [slots.default?.(), slots.footer?.()])
        : null
    },
  })

  const MenuStub = defineComponent({
    name: 'Menu',
    setup(_, { expose }) {
      expose({ toggle: vi.fn() })
      return () => h('div', { 'data-stub': 'menu' })
    },
  })

  return {
    Button: ButtonStub,
    Checkbox: CheckboxStub,
    ConfirmPopup: PassThrough,
    Dialog: DialogStub,
    Divider: PassThrough,
    IftaLabel: PassThrough,
    Menu: MenuStub,
    Message: PassThrough,
    Select: SelectStub,
    Tag: PassThrough,
    useConfirm: () => ({ require: vi.fn() }),
    useToast: () => ({ add: toastAdd }),
  }
})

const INSTANCE_ID = '00000000-0000-0000-0000-000000000001'
const INSTANCE_UUID = {
  part1: 0,
  part2: 0,
  part3: 0,
  part4: 1,
}
const SECOND_INSTANCE_ID = '00000000-0000-0000-0000-000000000002'
const SECOND_INSTANCE_UUID = {
  part1: 0,
  part2: 0,
  part3: 0,
  part4: 2,
}
let hiddenState = false

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((innerResolve, innerReject) => {
    resolve = innerResolve
    reject = innerReject
  })
  return { promise, resolve, reject }
}

function runningInfo(hostname: string) {
  return {
    dev_name: 'utun-test',
    events: [],
    my_node_info: {
      virtual_ipv4: { address: { addr: 0 }, network_length: 24 },
      hostname,
      version: '2.6.7',
      peer_id: 1,
      listeners: [],
      ips: {
        public_ipv4: { addr: 0 },
        interface_ipv4s: [],
        public_ipv6: { part1: 0, part2: 0, part3: 0, part4: 0 },
        interface_ipv6s: [],
        listeners: [],
      },
      stun_info: {
        udp_nat_type: 0,
        tcp_nat_type: 0,
        last_update_time: 0,
      },
    },
    peer_route_pairs: [],
    peers: [],
    routes: [],
    running: true,
  }
}

function makeStatusApi(getNetworkInfo: ReturnType<typeof vi.fn>) {
  return {
    delete_network: vi.fn(),
    generate_config: vi.fn(),
    get_network_config: vi.fn(),
    get_network_info: getNetworkInfo,
    get_network_metas: vi.fn(async (instanceIds: string[]) => ({
      metas: Object.fromEntries(instanceIds.map((id) => [id, {
        config_permission: 0xffffffff,
        inst_id: id === SECOND_INSTANCE_ID ? SECOND_INSTANCE_UUID : INSTANCE_UUID,
        instance_name: id,
        network_name: id,
        source: 2,
      }])),
    })),
    list_network_instance_ids: vi.fn(async () => ({
      disabled_inst_ids: [],
      running_inst_ids: [INSTANCE_UUID, SECOND_INSTANCE_UUID],
    })),
    parse_config: vi.fn(),
    run_network: vi.fn(),
    save_config: vi.fn(),
    update_network_instance_state: vi.fn(),
    validate_config: vi.fn(),
  }
}

const StatusStub = defineComponent({
  name: 'Status',
  props: {
    curNetworkInst: Object,
  },
  setup(props) {
    return () => h('div', {
      'data-stub': 'status',
      'data-instance': (props.curNetworkInst as any)?.instance_id,
    }, (props.curNetworkInst as any)?.detail?.my_node_info?.hostname ?? 'empty')
  },
})

function makeFlagConfig(): NetworkConfig {
  const config = {
    ...DEFAULT_NETWORK_CONFIG(),
    instance_id: INSTANCE_ID,
    network_name: 'mesh-save',
  }

  BOOLEAN_CONFIG_FIELDS.forEach((field, index) => {
    config[field] = index % 2 === 0
  })

  return config
}

function cloneConfig(config: NetworkConfig): NetworkConfig {
  return JSON.parse(JSON.stringify(config)) as NetworkConfig
}

function snapshotBooleanConfigFields(config: NetworkConfig): Record<string, unknown> {
  return Object.fromEntries(
    BOOLEAN_CONFIG_FIELDS.map((field) => [field, config[field]]),
  )
}

async function settleRemoteManagement() {
  for (let i = 0; i < 3; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0))
    await flushPromises()
    await nextTick()
  }
}

async function settleAsync() {
  await flushPromises()
  await nextTick()
}

async function advanceAndSettle(ms: number) {
  await vi.advanceTimersByTimeAsync(ms)
  await settleAsync()
}

function setDocumentHidden(hidden: boolean) {
  hiddenState = hidden
  document.dispatchEvent(new Event('visibilitychange'))
}

beforeEach(() => {
  toastAdd.mockClear()
  vi.useRealTimers()
  hiddenState = false
  Object.defineProperty(document, 'hidden', {
    configurable: true,
    get: () => hiddenState,
  })
})

afterEach(() => {
  vi.useRealTimers()
  hiddenState = false
})

describe('RemoteManagement persisted selection', () => {
  it('ignores an empty Select update until async instance options are loaded', async () => {
    vi.useFakeTimers()
    const instanceResponse = deferred<{
      disabled_inst_ids: Array<typeof INSTANCE_UUID>
      running_inst_ids: Array<typeof INSTANCE_UUID>
    }>()
    const api = makeStatusApi(vi.fn(async () => undefined))
    api.list_network_instance_ids = vi.fn(() => instanceResponse.promise)

    const wrapper = mount(RemoteManagement, {
      props: {
        api,
        instanceId: INSTANCE_ID,
      },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: true,
        },
      },
    })

    try {
      const select = wrapper.findComponent({ name: 'Select' })
      select.vm.$emit('update:modelValue', undefined)
      await nextTick()

      expect(wrapper.emitted('update:instanceId')).toBeUndefined()

      vi.runOnlyPendingTimers()
      instanceResponse.resolve({
        disabled_inst_ids: [INSTANCE_UUID],
        running_inst_ids: [],
      })
      await flushPromises()
      await nextTick()

      expect(select.props('modelValue')).toMatchObject({ uuid: INSTANCE_ID })
    } finally {
      wrapper.unmount()
      vi.useRealTimers()
    }
  })
})

describe('RemoteManagement config save', () => {
  it('saves the home policy toggle for a stopped network without starting it', async () => {
    const config = {
      ...DEFAULT_NETWORK_CONFIG(),
      instance_id: INSTANCE_ID,
      enable_policy_proxy: false,
      policy_config_inline: '',
    }
    const api = makeStatusApi(vi.fn(async () => undefined))
    api.get_network_config = vi.fn(async () => cloneConfig(config))
    api.get_network_metas = vi.fn(async () => ({
      metas: {
        [INSTANCE_ID]: {
          config_permission: 0,
          network_name: 'policy-home',
        },
      },
    }))
    api.list_network_instance_ids = vi.fn(async () => ({
      disabled_inst_ids: [INSTANCE_UUID],
      running_inst_ids: [],
    }))
    api.validate_config = vi.fn(async () => ({ policy_diagnostics: [] }))

    const wrapper = mount(RemoteManagement, {
      props: { api, instanceId: INSTANCE_ID },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          PolicyEditor: true,
          Status: StatusStub,
        },
      },
    })

    try {
      await settleRemoteManagement()

      const toggle = wrapper.find('[data-testid="policy-home-toggle"]')
      expect(toggle.exists()).toBe(true)
      expect(toggle.attributes('disabled')).toBeUndefined()
      const header = wrapper.get('.network-header')
      const headerRow = header.get('.network-header-row')
      const buttonContainer = headerRow.get('.button-container')
      expect(headerRow.find('[data-testid="policy-home-controls"]').exists()).toBe(false)
      expect(buttonContainer.find('.create-button').exists()).toBe(true)
      expect(header.get('[data-testid="policy-home-controls"]').element.parentElement)
        .toBe(header.element)
      expect(wrapper.find('[data-testid="policy-runtime-status"]').attributes('data-value'))
        .toBe('web.device_management.policy_runtime_stopped')

      await toggle.setValue(true)
      await settleAsync()

      expect(api.validate_config).toHaveBeenCalledOnce()
      expect(api.save_config).toHaveBeenCalledOnce()
      expect(api.save_config.mock.calls[0][0]).toMatchObject({
        instance_id: INSTANCE_ID,
        enable_policy_proxy: true,
      })
      expect((api.save_config.mock.calls[0][0] as NetworkConfig).policy_config_inline).toContain('version: 1')
      expect(api.run_network).not.toHaveBeenCalled()
      expect(api.update_network_instance_state).not.toHaveBeenCalled()
      expect(toastAdd).toHaveBeenCalledWith(expect.objectContaining({
        detail: 'web.device_management.policy_saved_restart_required',
      }))
    } finally {
      wrapper.unmount()
    }
  })

  it('keeps policy controls read-only while the network is running', async () => {
    const config = {
      ...DEFAULT_NETWORK_CONFIG(),
      instance_id: INSTANCE_ID,
      enable_policy_proxy: true,
      policy_config_inline: 'version: 1\nrules:\n  - FINAL,DIRECT\n',
    }
    const api = makeStatusApi(vi.fn(async () => ({
      ...runningInfo('policy-read-only'),
      policy_runtime_running: true,
    })))
    api.get_network_config = vi.fn(async () => cloneConfig(config))
    api.get_network_metas = vi.fn(async () => ({
      metas: {
        [INSTANCE_ID]: {
          config_permission: 0,
          network_name: 'policy-read-only',
        },
      },
    }))
    api.list_network_instance_ids = vi.fn(async () => ({
      disabled_inst_ids: [],
      running_inst_ids: [INSTANCE_UUID],
    }))

    const wrapper = mount(RemoteManagement, {
      props: { api, instanceId: INSTANCE_ID },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          PolicyEditor: true,
          Status: StatusStub,
        },
      },
    })

    try {
      await settleRemoteManagement()

      const toggle = wrapper.get('[data-testid="policy-home-toggle"]')
      const editYaml = wrapper.get('[data-testid="policy-home-edit-yaml"]')
      expect(toggle.attributes('disabled')).toBeDefined()
      expect(editYaml.attributes('disabled')).toBeDefined()
      expect(wrapper.get('[data-testid="policy-runtime-status"]').attributes('data-value'))
        .toBe('web.device_management.policy_runtime_running')

      await toggle.setValue(false)
      await editYaml.trigger('click')
      await settleAsync()

      expect(api.save_config).not.toHaveBeenCalled()
      expect(wrapper.findComponent({ name: 'PolicyEditor' }).exists()).toBe(false)
    } finally {
      wrapper.unmount()
    }
  })

  it('opens the focused policy YAML editor for a stopped network and saves its draft without starting', async () => {
    const config = {
      ...DEFAULT_NETWORK_CONFIG(),
      instance_id: INSTANCE_ID,
      enable_policy_proxy: true,
      policy_config_inline: 'version: 1\nrules:\n  - FINAL,DIRECT\n',
    }
    const api = makeStatusApi(vi.fn(async () => undefined))
    api.get_network_config = vi.fn(async () => cloneConfig(config))
    api.get_network_metas = vi.fn(async () => ({
      metas: {
        [INSTANCE_ID]: {
          config_permission: 0,
          network_name: 'policy-yaml',
        },
      },
    }))
    api.list_network_instance_ids = vi.fn(async () => ({
      disabled_inst_ids: [INSTANCE_UUID],
      running_inst_ids: [],
    }))
    api.validate_config = vi.fn(async () => ({ policy_diagnostics: [] }))

    const wrapper = mount(RemoteManagement, {
      props: { api, instanceId: INSTANCE_ID },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          PolicyEditor: true,
          Status: StatusStub,
        },
      },
    })

    try {
      await settleRemoteManagement()
      await wrapper.find('[data-testid="policy-home-edit-yaml"]').trigger('click')
      await nextTick()

      const editor = wrapper.findComponent({ name: 'PolicyEditor' })
      expect(editor.exists()).toBe(true)
      expect(editor.props('yamlOnly')).toBe(true)
      editor.vm.$emit('update:modelValue', {
        ...cloneConfig(config),
        policy_config_inline: 'version: 1\nrules:\n  - FINAL,REJECT\n',
      })
      await nextTick()
      await wrapper.find('[data-testid="policy-yaml-save"]').trigger('click')
      await settleAsync()

      expect(api.save_config).toHaveBeenCalledOnce()
      expect((api.save_config.mock.calls[0][0] as NetworkConfig).policy_config_inline).toContain('FINAL,REJECT')
      expect(api.run_network).not.toHaveBeenCalled()
      expect(api.update_network_instance_state).not.toHaveBeenCalled()
    } finally {
      wrapper.unmount()
    }
  })

  it('saves the current network config without dropping boolean fields', async () => {
    const config = makeFlagConfig()
    const expectedFlags = snapshotBooleanConfigFields(config)
    const api = {
      delete_network: vi.fn(),
      generate_config: vi.fn(),
      get_network_config: vi.fn(async () => cloneConfig(config)),
      get_network_info: vi.fn(),
      get_network_metas: vi.fn(async (instanceIds: string[]) => ({
        metas: Object.fromEntries(instanceIds.map((id) => [id, {
          config_permission: 0xffffffff,
          inst_id: INSTANCE_UUID,
          instance_name: 'mesh-save',
          network_name: 'mesh-save',
          source: 2,
        }])),
      })),
      list_network_instance_ids: vi.fn(async () => ({
        disabled_inst_ids: [INSTANCE_UUID],
        running_inst_ids: [],
      })),
      parse_config: vi.fn(),
      run_network: vi.fn(),
      save_config: vi.fn(async () => undefined),
      update_network_instance_state: vi.fn(),
      validate_config: vi.fn(async () => ({
        toml_config: '',
        policy_diagnostics: [{
          severity: 'warning',
          code: 'rule.udp_fallthrough',
          path: 'rules[0]',
          message: 'UDP continues with the next rule',
        }],
      })),
    }

    const wrapper = mount(RemoteManagement, {
      props: {
        api,
        instanceId: INSTANCE_ID,
      },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: true,
        },
      },
    })

    try {
      await settleRemoteManagement()

      const saveButton = wrapper.find('button[data-label="web.device_management.save_config"]')
      expect(saveButton.exists()).toBe(true)
      expect(saveButton.attributes('disabled')).toBeUndefined()

      await saveButton.trigger('click')
      await flushPromises()

      expect(api.validate_config).toHaveBeenCalledOnce()
      expect(api.save_config).toHaveBeenCalledOnce()
      expect(toastAdd).toHaveBeenCalledWith(expect.objectContaining({
        severity: 'warn',
        summary: 'Policy rule.udp_fallthrough',
      }))
      const savedConfig = api.save_config.mock.calls[0][0] as NetworkConfig

      for (const field of BOOLEAN_CONFIG_FIELDS) {
        expect(savedConfig[field], `${field} should be saved`).toBe(expectedFlags[field])
      }
    } finally {
      wrapper.unmount()
    }
  })

  it('shows validation failure and does not save invalid config', async () => {
    const config = makeFlagConfig()
    const api = {
      delete_network: vi.fn(),
      generate_config: vi.fn(),
      get_network_config: vi.fn(async () => cloneConfig(config)),
      get_network_info: vi.fn(),
      get_network_metas: vi.fn(async (instanceIds: string[]) => ({
        metas: Object.fromEntries(instanceIds.map((id) => [id, {
          config_permission: 0xffffffff,
          inst_id: INSTANCE_UUID,
          instance_name: 'mesh-save',
          network_name: 'mesh-save',
          source: 2,
        }])),
      })),
      list_network_instance_ids: vi.fn(async () => ({
        disabled_inst_ids: [INSTANCE_UUID],
        running_inst_ids: [],
      })),
      parse_config: vi.fn(),
      run_network: vi.fn(),
      save_config: vi.fn(),
      update_network_instance_state: vi.fn(),
      validate_config: vi.fn(async () => {
        throw new Error('failed to parse transport_priority')
      }),
    }

    const wrapper = mount(RemoteManagement, {
      props: {
        api,
        instanceId: INSTANCE_ID,
      },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: true,
        },
      },
    })

    try {
      await settleRemoteManagement()

      await wrapper.find('button[data-label="web.device_management.save_config"]').trigger('click')
      await flushPromises()

      expect(api.validate_config).toHaveBeenCalledOnce()
      expect(api.save_config).not.toHaveBeenCalled()
      expect(api.update_network_instance_state).not.toHaveBeenCalled()
    } finally {
      wrapper.unmount()
    }
  })
})

describe('RemoteManagement floating network action', () => {
  it('reuses the existing stop action for a running network', async () => {
    const api = makeStatusApi(vi.fn(async () => runningInfo('floating-stop')))
    api.get_network_config = vi.fn(async () => ({
      ...DEFAULT_NETWORK_CONFIG(),
      instance_id: INSTANCE_ID,
    }))
    api.get_network_metas = vi.fn(async () => ({
      metas: {
        [INSTANCE_ID]: {
          config_permission: 0,
          network_name: 'floating-stop',
        },
      },
    }))

    const wrapper = mount(RemoteManagement, {
      props: { api, instanceId: INSTANCE_ID },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: StatusStub,
        },
      },
    })

    try {
      await settleRemoteManagement()

      const action = wrapper.get('[data-testid="floating-network-action"]')
      expect(action.attributes('data-label')).toBe('web.device_management.disable_network')
      expect(action.attributes('disabled')).toBeUndefined()

      await action.trigger('click')
      await settleAsync()

      expect(api.update_network_instance_state).toHaveBeenCalledOnce()
      expect(api.update_network_instance_state).toHaveBeenCalledWith(INSTANCE_ID, true)
      expect(api.run_network).not.toHaveBeenCalled()
    } finally {
      wrapper.unmount()
    }
  })

  it('reuses the existing save-and-run action for a disabled network', async () => {
    const config = {
      ...DEFAULT_NETWORK_CONFIG(),
      instance_id: INSTANCE_ID,
    }
    const api = makeStatusApi(vi.fn(async () => undefined))
    api.get_network_config = vi.fn(async () => cloneConfig(config))
    api.list_network_instance_ids = vi.fn(async () => ({
      disabled_inst_ids: [INSTANCE_UUID],
      running_inst_ids: [],
    }))
    api.validate_config = vi.fn(async () => ({ policy_diagnostics: [] }))

    const wrapper = mount(RemoteManagement, {
      props: { api, instanceId: INSTANCE_ID },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: true,
        },
      },
    })

    try {
      await settleRemoteManagement()

      const action = wrapper.get('[data-testid="floating-network-action"]')
      expect(action.attributes('data-label')).toBe('run_network')

      await action.trigger('click')
      await settleAsync()

      expect(api.validate_config).toHaveBeenCalledOnce()
      expect(api.save_config).toHaveBeenCalledWith(expect.objectContaining({ instance_id: INSTANCE_ID }))
      expect(api.update_network_instance_state).toHaveBeenCalledWith(INSTANCE_ID, false)
      expect(api.run_network).not.toHaveBeenCalled()
    } finally {
      wrapper.unmount()
    }
  })

  it('does not show the floating action without a selected network', async () => {
    const api = makeStatusApi(vi.fn(async () => undefined))
    api.list_network_instance_ids = vi.fn(async () => ({
      disabled_inst_ids: [],
      running_inst_ids: [],
    }))

    const wrapper = mount(RemoteManagement, {
      props: { api },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: true,
        },
      },
    })

    try {
      await settleRemoteManagement()
      expect(wrapper.find('[data-testid="floating-network-action"]').exists()).toBe(false)
    } finally {
      wrapper.unmount()
    }
  })
})

describe('RemoteManagement status refresh', () => {
  it('keeps one empty response grace and clears stale status on the second empty response', async () => {
    vi.useFakeTimers()
    const getNetworkInfo = vi.fn()
      .mockResolvedValue(runningInfo('stable-a'))
    const api = makeStatusApi(getNetworkInfo)

    const wrapper = mount(RemoteManagement, {
      props: {
        api,
        instanceId: INSTANCE_ID,
      },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: StatusStub,
        },
      },
    })

    try {
      vi.runOnlyPendingTimers()
      await settleAsync()
      expect(wrapper.find('[data-stub="status"]').text()).toBe('stable-a')

      getNetworkInfo.mockReset()
      getNetworkInfo
        .mockResolvedValueOnce(undefined)
        .mockResolvedValueOnce(undefined)

      vi.advanceTimersByTime(2000)
      await settleAsync()
      expect(wrapper.find('[data-stub="status"]').text()).toBe('stable-a')

      vi.advanceTimersByTime(2000)
      await settleAsync()
      expect(wrapper.find('[data-stub="status"]').text()).toBe('empty')
    } finally {
      wrapper.unmount()
    }
  })

  it('drops an old empty response that arrives after a newer success', async () => {
    vi.useFakeTimers()
    const requests: ReturnType<typeof deferred<any>>[] = []
    const getNetworkInfo = vi.fn(() => {
      const request = deferred<any>()
      requests.push(request)
      return request.promise
    })
    const api = makeStatusApi(getNetworkInfo)

    const wrapper = mount(RemoteManagement, {
      props: {
        api,
        instanceId: INSTANCE_ID,
      },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: StatusStub,
        },
      },
    })

    try {
      vi.runOnlyPendingTimers()
      await settleAsync()
      if (requests.length < 2) {
        vi.advanceTimersByTime(2000)
        await settleAsync()
      }
      expect(requests.length).toBeGreaterThanOrEqual(2)

      const oldRequest = requests[0]
      const newestRequest = requests[requests.length - 1]
      newestRequest.resolve(runningInfo('new-success'))
      await settleAsync()
      expect(wrapper.find('[data-stub="status"]').text()).toBe('new-success')

      oldRequest.resolve(undefined)
      await settleAsync()
      expect(wrapper.find('[data-stub="status"]').text()).toBe('new-success')
    } finally {
      wrapper.unmount()
    }
  })

  it('drops old-instance responses after switching selected instance', async () => {
    vi.useFakeTimers()
    const oldInstanceRequests: ReturnType<typeof deferred<any>>[] = []
    const newInstanceRequests: ReturnType<typeof deferred<any>>[] = []
    const getNetworkInfo = vi.fn((id: string) => {
      const request = deferred<any>()
      if (id === SECOND_INSTANCE_ID) {
        newInstanceRequests.push(request)
      } else {
        oldInstanceRequests.push(request)
      }
      return request.promise
    })
    const api = makeStatusApi(getNetworkInfo)

    const wrapper = mount(RemoteManagement, {
      props: {
        api,
        instanceId: INSTANCE_ID,
      },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: StatusStub,
        },
      },
    })

    try {
      vi.runOnlyPendingTimers()
      await settleAsync()
      expect(getNetworkInfo).toHaveBeenCalledWith(INSTANCE_ID)

      await wrapper.setProps({ instanceId: SECOND_INSTANCE_ID })
      await settleAsync()
      expect(wrapper.find('[data-stub="status"]').exists()).toBe(false)
      expect(getNetworkInfo).toHaveBeenCalledWith(SECOND_INSTANCE_ID)

      const newestNewInstanceRequest = newInstanceRequests[newInstanceRequests.length - 1]
      newestNewInstanceRequest.resolve(runningInfo('instance-b'))
      await settleAsync()
      const status = wrapper.find('[data-stub="status"]')
      expect(status.text()).toBe('instance-b')
      expect(status.attributes('data-instance')).toBe(SECOND_INSTANCE_ID)

      for (const request of oldInstanceRequests) {
        request.resolve(runningInfo('late-instance-a'))
      }
      await settleAsync()
      expect(wrapper.find('[data-stub="status"]').text()).toBe('instance-b')
    } finally {
      wrapper.unmount()
    }
  })

  it('drops old-instance empty responses after switching selected instance', async () => {
    vi.useFakeTimers()
    const oldInstanceRequests: ReturnType<typeof deferred<any>>[] = []
    const newInstanceRequests: ReturnType<typeof deferred<any>>[] = []
    const getNetworkInfo = vi.fn((id: string) => {
      const request = deferred<any>()
      if (id === SECOND_INSTANCE_ID) {
        newInstanceRequests.push(request)
      } else {
        oldInstanceRequests.push(request)
      }
      return request.promise
    })
    const api = makeStatusApi(getNetworkInfo)

    const wrapper = mount(RemoteManagement, {
      props: {
        api,
        instanceId: INSTANCE_ID,
      },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: StatusStub,
        },
      },
    })

    try {
      vi.runOnlyPendingTimers()
      await settleAsync()

      await wrapper.setProps({ instanceId: SECOND_INSTANCE_ID })
      await settleAsync()

      const newestNewInstanceRequest = newInstanceRequests[newInstanceRequests.length - 1]
      newestNewInstanceRequest.resolve(runningInfo('instance-b-empty-race'))
      await settleAsync()
      expect(wrapper.find('[data-stub="status"]').text()).toBe('instance-b-empty-race')

      for (const request of oldInstanceRequests) {
        request.resolve(undefined)
      }
      await settleAsync()
      expect(wrapper.find('[data-stub="status"]').text()).toBe('instance-b-empty-race')
    } finally {
      wrapper.unmount()
    }
  })

  it('switches from active 2s refresh to idle 10s refresh after 60 seconds of no activity', async () => {
    vi.useFakeTimers()
    const getNetworkInfo = vi.fn().mockResolvedValue(runningInfo('idle-mode'))
    const api = makeStatusApi(getNetworkInfo)

    const wrapper = mount(RemoteManagement, {
      props: {
        api,
        instanceId: INSTANCE_ID,
      },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: StatusStub,
        },
      },
    })

    try {
      await advanceAndSettle(0)
      getNetworkInfo.mockClear()

      await advanceAndSettle(59000)
      expect(getNetworkInfo).toHaveBeenCalledTimes(29)

      await advanceAndSettle(1000)
      expect(getNetworkInfo).toHaveBeenCalledTimes(30)

      await advanceAndSettle(9000)
      expect(getNetworkInfo).toHaveBeenCalledTimes(30)

      await advanceAndSettle(1000)
      expect(getNetworkInfo).toHaveBeenCalledTimes(31)
    } finally {
      wrapper.unmount()
    }
  })

  it('stops polling while hidden and refreshes immediately when visible again', async () => {
    vi.useFakeTimers()
    const getNetworkInfo = vi.fn().mockResolvedValue(runningInfo('visibility'))
    const api = makeStatusApi(getNetworkInfo)

    const wrapper = mount(RemoteManagement, {
      props: {
        api,
        instanceId: INSTANCE_ID,
      },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: StatusStub,
        },
      },
    })

    try {
      await advanceAndSettle(0)
      getNetworkInfo.mockClear()

      setDocumentHidden(true)
      await settleAsync()
      expect(vi.getTimerCount()).toBe(0)

      await advanceAndSettle(10000)
      expect(getNetworkInfo).not.toHaveBeenCalled()

      setDocumentHidden(false)
      await settleAsync()
      expect(getNetworkInfo).toHaveBeenCalledTimes(1)

      getNetworkInfo.mockClear()
      await advanceAndSettle(1000)
      expect(getNetworkInfo).not.toHaveBeenCalled()
      await advanceAndSettle(1000)
      expect(getNetworkInfo).toHaveBeenCalledTimes(1)
    } finally {
      wrapper.unmount()
    }
  })

  it('stops polling when pauseAutoRefresh is true and resumes immediately when false', async () => {
    vi.useFakeTimers()
    const getNetworkInfo = vi.fn().mockResolvedValue(runningInfo('pause-auto-refresh'))
    const api = makeStatusApi(getNetworkInfo)

    const wrapper = mount(RemoteManagement, {
      props: {
        api,
        instanceId: INSTANCE_ID,
      },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: StatusStub,
        },
      },
    })

    try {
      await advanceAndSettle(0)
      getNetworkInfo.mockClear()

      await wrapper.setProps({ pauseAutoRefresh: true })
      await settleAsync()
      expect(vi.getTimerCount()).toBe(0)

      await advanceAndSettle(10000)
      expect(getNetworkInfo).not.toHaveBeenCalled()

      await wrapper.setProps({ pauseAutoRefresh: false })
      await settleAsync()
      expect(getNetworkInfo).toHaveBeenCalledTimes(1)

      getNetworkInfo.mockClear()
      await advanceAndSettle(1000)
      expect(getNetworkInfo).not.toHaveBeenCalled()
      await advanceAndSettle(1000)
      expect(getNetworkInfo).toHaveBeenCalledTimes(1)
    } finally {
      wrapper.unmount()
    }
  })

  it('refreshes immediately on user activity after entering idle mode', async () => {
    vi.useFakeTimers()
    const getNetworkInfo = vi.fn().mockResolvedValue(runningInfo('user-activity'))
    const api = makeStatusApi(getNetworkInfo)

    const wrapper = mount(RemoteManagement, {
      props: {
        api,
        instanceId: INSTANCE_ID,
      },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: StatusStub,
        },
      },
    })

    try {
      await advanceAndSettle(0)

      await advanceAndSettle(60000)
      getNetworkInfo.mockClear()

      await advanceAndSettle(9000)
      expect(getNetworkInfo).not.toHaveBeenCalled()

      document.dispatchEvent(new Event('pointerdown'))
      await settleAsync()
      expect(getNetworkInfo).toHaveBeenCalledTimes(1)

      getNetworkInfo.mockClear()
      await advanceAndSettle(1000)
      expect(getNetworkInfo).not.toHaveBeenCalled()
      await advanceAndSettle(1000)
      expect(getNetworkInfo).toHaveBeenCalledTimes(1)
    } finally {
      wrapper.unmount()
    }
  })

  it('does not recreate timers after unmount when an in-flight refresh resolves later', async () => {
    vi.useFakeTimers()
    const getNetworkInfo = vi.fn()
      .mockResolvedValue(runningInfo('initial'))
    const api = makeStatusApi(getNetworkInfo)

    const wrapper = mount(RemoteManagement, {
      props: {
        api,
        instanceId: INSTANCE_ID,
      },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: StatusStub,
        },
      },
    })

    try {
      await advanceAndSettle(0)
      const initialCallCount = getNetworkInfo.mock.calls.length
      expect(initialCallCount).toBeGreaterThan(0)

      const pendingRequest = deferred<any>()
      getNetworkInfo.mockImplementationOnce(() => pendingRequest.promise)

      await advanceAndSettle(2000)
      expect(getNetworkInfo).toHaveBeenCalledTimes(initialCallCount + 1)

      wrapper.unmount()
      expect(vi.getTimerCount()).toBe(0)

      pendingRequest.resolve(runningInfo('late-success'))
      await settleAsync()
      expect(vi.getTimerCount()).toBe(0)

      await advanceAndSettle(30000)
      expect(getNetworkInfo).toHaveBeenCalledTimes(initialCallCount + 1)
      expect(vi.getTimerCount()).toBe(0)
    } catch (error) {
      wrapper.unmount()
      throw error
    }
  })

  it('backs off repeated RPC failures and resumes normal polling after recovery', async () => {
    vi.useFakeTimers()
    const debugSpy = vi.spyOn(console, 'debug').mockImplementation(() => {})
    const getNetworkInfo = vi.fn().mockResolvedValue(runningInfo('initial-success'))
    const api = makeStatusApi(getNetworkInfo)

    const wrapper = mount(RemoteManagement, {
      props: {
        api,
        instanceId: INSTANCE_ID,
      },
      global: {
        stubs: {
          Config: true,
          ConfigEditDialog: true,
          Status: StatusStub,
        },
      },
    })

    try {
      await advanceAndSettle(0)

      getNetworkInfo.mockReset()
      getNetworkInfo.mockRejectedValue(new Error('rpc failed'))

      await advanceAndSettle(2000)
      expect(getNetworkInfo).toHaveBeenCalledTimes(1)

      getNetworkInfo.mockClear()
      await advanceAndSettle(4000)
      expect(getNetworkInfo).not.toHaveBeenCalled()
      await advanceAndSettle(1000)
      expect(getNetworkInfo).toHaveBeenCalledTimes(1)

      getNetworkInfo.mockClear()
      await advanceAndSettle(9000)
      expect(getNetworkInfo).not.toHaveBeenCalled()
      await advanceAndSettle(1000)
      expect(getNetworkInfo).toHaveBeenCalledTimes(1)

      getNetworkInfo.mockClear()
      getNetworkInfo.mockResolvedValue(runningInfo('recovered'))

      await advanceAndSettle(29000)
      expect(getNetworkInfo).not.toHaveBeenCalled()
      await advanceAndSettle(1000)
      expect(getNetworkInfo).toHaveBeenCalledTimes(1)

      getNetworkInfo.mockClear()
      await advanceAndSettle(1000)
      expect(getNetworkInfo).not.toHaveBeenCalled()
      await advanceAndSettle(1000)
      expect(getNetworkInfo).toHaveBeenCalledTimes(1)
    } finally {
      debugSpy.mockRestore()
      wrapper.unmount()
    }
  })
})
