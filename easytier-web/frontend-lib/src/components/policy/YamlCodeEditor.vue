<script setup lang="ts">
import { basicSetup } from 'codemirror'
import { yaml } from '@codemirror/lang-yaml'
import { linter, lintGutter, type Diagnostic } from '@codemirror/lint'
import { Compartment, EditorState } from '@codemirror/state'
import { EditorView } from '@codemirror/view'
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { yamlSyntaxDiagnostics } from './yamlValidation'

const contents = defineModel<string | undefined>({ required: true })
const valid = defineModel<boolean>('valid', { default: true })
const props = defineProps<{
  readOnly?: boolean
  minHeight?: string
}>()

const host = ref<HTMLElement>()
const readOnlyCompartment = new Compartment()
let editor: EditorView | undefined
let applyingExternalValue = false

function diagnostics(source: string): Diagnostic[] {
  const result = yamlSyntaxDiagnostics(source)
  valid.value = result.length === 0
  return result.map(item => ({ ...item, severity: 'error' }))
}

onMounted(() => {
  if (!host.value) return
  const initialContents = contents.value ?? ''
  diagnostics(initialContents)
  editor = new EditorView({
    parent: host.value,
    state: EditorState.create({
      doc: initialContents,
      extensions: [
        basicSetup,
        yaml(),
        lintGutter(),
        linter(view => diagnostics(view.state.doc.toString()), { delay: 100 }),
        readOnlyCompartment.of(EditorState.readOnly.of(Boolean(props.readOnly))),
        EditorView.contentAttributes.of({
          autocapitalize: 'off',
          autocomplete: 'off',
          autocorrect: 'off',
          spellcheck: 'false',
        }),
        EditorView.lineWrapping,
        EditorView.updateListener.of((update) => {
          if (!update.docChanged || applyingExternalValue) return
          contents.value = update.state.doc.toString()
        }),
        EditorView.theme({
          '&': {
            minHeight: props.minHeight ?? '24rem',
            height: '100%',
            fontSize: '13px',
          },
          '.cm-scroller': {
            fontFamily: '"SFMono-Regular", Menlo, Monaco, Consolas, "Liberation Mono", monospace',
          },
          '.cm-content': { padding: '12px 0' },
          '.cm-gutters': { borderRight: '1px solid var(--p-content-border-color)' },
          '&.cm-focused': { outline: '2px solid var(--p-primary-color)' },
        }),
      ],
    }),
  })
})

watch(contents, (value) => {
  const nextValue = value ?? ''
  if (!editor || nextValue === editor.state.doc.toString()) return
  applyingExternalValue = true
  editor.dispatch({
    changes: { from: 0, to: editor.state.doc.length, insert: nextValue },
  })
  applyingExternalValue = false
})

watch(() => props.readOnly, (value) => {
  editor?.dispatch({
    effects: readOnlyCompartment.reconfigure(EditorState.readOnly.of(Boolean(value))),
  })
})

onBeforeUnmount(() => {
  editor?.destroy()
  editor = undefined
})
</script>

<template>
  <div ref="host" class="yaml-code-editor overflow-hidden rounded-md border border-surface-300"
    data-testid="yaml-code-editor" />
</template>
