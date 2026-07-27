<script setup lang="ts">
import { Message, Textarea } from 'primevue'
import { useI18n } from 'vue-i18n'

const contents = defineModel<string>({ required: true })
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
    <Textarea id="policy_mihomo_yaml_contents" v-model="contents" rows="20"
      auto-resize class="w-full font-mono" :placeholder="t('policy_config_inline_placeholder')"
      :readonly="readOnly" data-testid="mihomo-yaml-contents" />
  </div>
</template>
