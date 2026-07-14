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

function mountEditor(config: NetworkConfig, api?: import('../src/modules/api').RemoteClient) {
  const model = reactive(config) as NetworkConfig
  const wrapper = mount(PolicyEditor, {
    props: { modelValue: model, api },
    global: {
      directives: { tooltip: () => {} },
      stubs: {
        Button: true,
        Checkbox: CheckboxStub,
        Column: true,
        DataTable: true,
        InputNumber: true,
        InputText: true,
        Message: MessageStub,
        Panel: PanelStub,
        Password: true,
        Select: true,
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
    expect(model.policy_config_inline).toContain('MATCH,DIRECT')
    expect(wrapper.find('[data-header="policy.editor.nodes"]').exists()).toBe(true)
    expect(wrapper.find('[data-header="policy.editor.rules"]').exists()).toBe(true)
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
  })
})
