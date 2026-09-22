<script setup lang="ts">
import Modal from "./Modal.vue";
import StationEditor from "./StationEditor.vue";
import { errorText, t } from "../i18n";
import type { Station } from "../types";

defineProps<{ open: boolean; station: Station; presetName: string; error: unknown }>();
const emit = defineEmits<{ close: []; change: []; pick: [name: string]; remove: [] }>();
</script>

<template>
  <Modal :open="open" :title="t('station.title')" @close="emit('close')">
    <!-- 出错的动作是对话框里发生的（改机场、删席位），错误也要在对话框里现身——
         这条 banner 在 App.vue 顶部那份原样还在渲染，只是被这个模态的遮罩挡住了。 -->
    <p v-if="error !== null" class="rounded border border-red-400 px-3 py-2 text-xs text-red-600">
      {{ errorText(error) }}
    </p>
    <StationEditor
      :station="station"
      :preset-name="presetName"
      @change="emit('change')"
      @pick="emit('pick', $event)"
      @remove="emit('remove')"
    />
  </Modal>
</template>
