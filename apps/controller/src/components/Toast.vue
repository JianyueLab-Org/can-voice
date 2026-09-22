<script setup lang="ts">
import { ref, watch } from "vue";

/**
 * 右上角一条，4 秒后自己消失。can-audio 用 `qfluentwidgets.InfoBar.warning`
 * 报校验错误（`controller/gui.py:799-806`），不弹模态。
 *
 * **不用模态**：值班的人手上有飞机，一个要点确定才能继续的框比那条错误本身更碍事。
 */
const props = defineProps<{ message: string | null }>();

const shown = ref<string | null>(null);
let timer: number | undefined;

watch(
  () => props.message,
  (m) => {
    window.clearTimeout(timer);
    shown.value = m;
    if (m) timer = window.setTimeout(() => (shown.value = null), 4000);
  },
);
</script>

<template>
  <div
    v-if="shown"
    class="fixed right-3 top-3 z-40 max-w-80 rounded border border-amber-400 bg-white px-3 py-2 text-xs text-amber-700 shadow"
  >
    {{ shown }}
  </div>
</template>
