<script setup lang="ts">
import { Message } from 'primevue'
import { useI18n } from 'vue-i18n'
import YamlCodeEditor from './YamlCodeEditor.vue'

const contents = defineModel<string>({ required: true })
const valid = defineModel<boolean>('valid', { default: true })
defineProps<{
  filePath: string
  readOnly?: boolean
}>()

const { t } = useI18n()
</script>

<template>
  <div class="flex flex-col gap-4" data-testid="mihomo-yaml-editor">
    <Message severity="info" :closable="false">
      {{ t('policy_mihomo_native_config_notice') }}
    </Message>
    <div class="flex flex-col gap-1">
      <span class="font-semibold">{{ t('policy_config_file') }}</span>
      <code class="text-sm break-all" data-testid="policy-mihomo-file-path">
        {{ filePath }}
      </code>
    </div>
    <Message severity="info" :closable="false">
      {{ t('policy_mihomo_file_edit_notice') }}
    </Message>
    <label for="policy_mihomo_yaml_contents" class="font-semibold">
      {{ t('policy.editor.advanced_yaml') }}
    </label>
    <YamlCodeEditor v-model="contents" v-model:valid="valid"
      :read-only="readOnly" data-testid="mihomo-yaml-contents" />
  </div>
</template>
