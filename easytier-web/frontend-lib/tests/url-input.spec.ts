import { mount } from '@vue/test-utils'
import { defineComponent, h, nextTick } from 'vue'
import { describe, expect, it, vi } from 'vitest'
import UrlInput from '../src/components/UrlInput.vue'

vi.mock('vue-i18n', () => ({
  useI18n: () => ({ t: (key: string) => key }),
}))

const protos = {
  tcp: 11010,
  quic: 11012,
  faketcp: 11013,
  'quic-brutal': 11013,
}

function modelStub(name: string) {
  return defineComponent({
    name,
    inheritAttrs: false,
    props: {
      modelValue: [String, Number],
      placeholder: String,
    },
    emits: ['update:modelValue', 'focus', 'blur', 'complete'],
    setup(props, { attrs, emit }) {
      return () => h('input', {
        ...attrs,
        value: props.modelValue ?? '',
        placeholder: props.placeholder,
        onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value),
        onFocus: () => emit('focus'),
        onBlur: () => emit('blur'),
      })
    },
  })
}

const PassThrough = defineComponent({
  name: 'PassThrough',
  setup(_, { slots }) {
    return () => h('div', slots.default?.())
  },
})

function mountUrlInput(modelValue: string) {
  return mount(UrlInput, {
    props: { modelValue, protos },
    global: {
      stubs: {
        AutoComplete: modelStub('AutoComplete'),
        InputNumber: modelStub('InputNumber'),
        InputText: modelStub('InputText'),
        InputGroup: PassThrough,
        InputGroupAddon: PassThrough,
        Button: true,
        Dialog: PassThrough,
      },
    },
  })
}

describe('UrlInput quic-brutal support', () => {
  it('selects the +3 preset port without imposing a bandwidth cap', async () => {
    const wrapper = mountUrlInput('tcp://0.0.0.0:11010')

    wrapper.findAllComponents({ name: 'AutoComplete' })[0].vm.$emit('update:modelValue', 'quic-brutal')
    await nextTick()

    expect(wrapper.emitted('update:modelValue')?.at(-1)).toEqual([
      'quic-brutal://0.0.0.0:11013',
    ])
  })

  it('parses and edits quic-brutal tx_bps without changing its address', async () => {
    const wrapper = mountUrlInput('quic-brutal://[::]:11013?tx_bps=250000000')
    const inputs = wrapper.findAllComponents({ name: 'InputNumber' })
    const txBpsInput = inputs.find((input) => input.props('placeholder') === 'quic_brutal_tx_bps_placeholder')

    expect(wrapper.findAllComponents({ name: 'AutoComplete' })[0].props('modelValue')).toBe('quic-brutal')
    expect(inputs.some((input) => input.props('modelValue') === 11013)).toBe(true)
    expect(txBpsInput?.props('modelValue')).toBe(250000000)

    txBpsInput?.vm.$emit('update:modelValue', 300000000)
    await nextTick()

    expect(wrapper.emitted('update:modelValue')?.at(-1)).toEqual([
      'quic-brutal://[::]:11013?tx_bps=300000000',
    ])
  })

  it('removes the Brutal-only query when switching back to ordinary QUIC', async () => {
    const wrapper = mountUrlInput('quic-brutal://0.0.0.0:11013?tx_bps=100000000')

    wrapper.findAllComponents({ name: 'AutoComplete' })[0].vm.$emit('update:modelValue', 'quic')
    await nextTick()

    expect(wrapper.emitted('update:modelValue')?.at(-1)).toEqual(['quic://0.0.0.0:11012'])
  })
})
