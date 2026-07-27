import { mount } from '@vue/test-utils'
import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'
import MihomoYamlEditor from '../src/components/policy/MihomoYamlEditor.vue'

vi.mock('vue-i18n', () => ({
  useI18n: () => ({
    t: (key: string) => key,
  }),
}))

const TextareaStub = defineComponent({
  name: 'Textarea',
  inheritAttrs: false,
  props: {
    modelValue: String,
    readonly: Boolean,
  },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit }) {
    return () => h('textarea', {
      ...attrs,
      value: props.modelValue,
      readonly: props.readonly,
      onInput: (event: Event) =>
        emit('update:modelValue', (event.target as HTMLTextAreaElement).value),
    })
  },
})

describe('MihomoYamlEditor', () => {
  it('edits only the referenced Mihomo file contents', async () => {
    const wrapper = mount(MihomoYamlEditor, {
      props: {
        modelValue: 'secret: original\n',
        filePath: '/managed/mihomo/autogen.yaml',
      },
      global: {
        stubs: {
          Message: true,
          Textarea: TextareaStub,
        },
      },
    })

    expect(wrapper.get('[data-testid="policy-mihomo-file-path"]').text())
      .toBe('/managed/mihomo/autogen.yaml')
    const textarea = wrapper.get<HTMLTextAreaElement>('[data-testid="mihomo-yaml-contents"]')
    expect(textarea.element.value).toBe('secret: original\n')
    await textarea.setValue('secret: changed\n')
    expect(wrapper.emitted('update:modelValue')?.at(-1)).toEqual(['secret: changed\n'])
  })

  it('keeps the file contents read-only while the network is running', () => {
    const wrapper = mount(MihomoYamlEditor, {
      props: {
        modelValue: 'rules: []\n',
        filePath: '/managed/mihomo/config.yaml',
        readOnly: true,
      },
      global: {
        stubs: {
          Message: true,
          Textarea: TextareaStub,
        },
      },
    })

    expect(wrapper.get<HTMLTextAreaElement>('[data-testid="mihomo-yaml-contents"]').element.readOnly)
      .toBe(true)
  })
})
