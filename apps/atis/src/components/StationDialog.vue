<script setup lang="ts">
import Modal from "./Modal.vue";
import StationEditor from "./StationEditor.vue";
import { t } from "../i18n";
import type { Station } from "../types";

defineProps<{ open: boolean; station: Station; presetName: string }>();
const emit = defineEmits<{ close: []; change: []; pick: [name: string]; remove: [] }>();
</script>

<template>
  <Modal :open="open" :title="t('station.title')" @close="emit('close')">
    <StationEditor
      :station="station"
      :preset-name="presetName"
      @change="emit('change')"
      @pick="emit('pick', $event)"
      @remove="emit('remove')"
    />
  </Modal>
</template>
