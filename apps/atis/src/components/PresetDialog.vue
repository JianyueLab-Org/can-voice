<script setup lang="ts">
import Modal from "./Modal.vue";
import PresetEditor from "./PresetEditor.vue";
import { errorText, t } from "../i18n";
import type { Preset } from "../types";

const props = defineProps<{ open: boolean; preset: Preset; chineseShown: boolean; error: unknown }>();
const emit = defineEmits<{ close: []; change: [] }>();
</script>

<template>
  <Modal
    :open="open"
    :title="t('preset.title', { name: props.preset.name })"
    width="w-[40rem]"
    @close="emit('close')"
  >
    <!-- 出错的动作是对话框里发生的，错误也要在对话框里现身——这条 banner 在
         App.vue 顶部那份原样还在渲染，只是被这个模态的遮罩挡住了。 -->
    <p v-if="error !== null" class="rounded border border-red-400 px-3 py-2 text-xs text-red-600">
      {{ errorText(error) }}
    </p>
    <PresetEditor :preset="preset" :chinese-shown="chineseShown" @change="emit('change')" />
  </Modal>
</template>
