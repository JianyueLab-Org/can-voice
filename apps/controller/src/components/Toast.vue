<script setup lang="ts">
import { onUnmounted, ref, watch } from "vue";

/**
 * 右上角一条，4 秒后自己消失。can-audio 用 `qfluentwidgets.InfoBar.warning`
 * 报校验错误（`controller/gui.py:799-806`），不弹模态。
 *
 * **不用模态**：值班的人手上有飞机，一个要点确定才能继续的框比那条错误本身更碍事。
 */
const props = defineProps<{ message: string | null; seq: number }>();

const shown = ref<string | null>(null);
let timer: number | undefined;

// **盯着序号，不盯着话本身**：同一句话连着报两次，`message` 的值不变，
// 监听它不会再触发。调用方每次都会递增 `seq`，所以这里才看得出"又报了一次"。
watch(
  () => props.seq,
  () => {
    window.clearTimeout(timer);
    shown.value = props.message;
    if (props.message) timer = window.setTimeout(() => (shown.value = null), 4000);
  },
);

onUnmounted(() => window.clearTimeout(timer));
</script>

<template>
  <div
    v-if="shown"
    class="fixed right-3 top-3 z-40 max-w-80 rounded border border-amber-400 bg-white px-3 py-2 text-xs text-amber-700 shadow"
  >
    {{ shown }}
  </div>
</template>
