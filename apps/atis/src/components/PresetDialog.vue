<script setup lang="ts">
import Modal from "./Modal.vue";
import PresetEditor from "./PresetEditor.vue";
import { t } from "../i18n";
import type { Preset } from "../types";

const props = defineProps<{ open: boolean; preset: Preset; chineseShown: boolean }>();
const emit = defineEmits<{ close: []; change: [] }>();
</script>

<template>
  <Modal
    :open="open"
    :title="t('preset.title', { name: props.preset.name })"
    width="w-[40rem]"
    @close="emit('close')"
  >
    <PresetEditor :preset="preset" :chinese-shown="chineseShown" @change="emit('change')" />
  </Modal>
</template>
