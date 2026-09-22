<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import { t } from "../i18n";

/**
 * 通用模态外壳。`SettingsDialog` 的骨架，加一个宽度属性和一个标题。
 *
 * 只有通播端有这个文件，所以不进 `SHARED_FRONTEND`。**哪天 xpc 也要一个，
 * 那时候把它登记进去**——两份不登记的副本会无声地漂开。
 */
const props = defineProps<{ open: boolean; title: string; width?: string }>();
const emit = defineEmits<{ close: [] }>();

const box = ref<HTMLElement | null>(null);

// 这个组件本身一直挂着，`v-if` 在它自己的根节点上，所以普通 watch 就够了。
// （`SettingsCommon` 要 `{ immediate: true }`，是因为它整个被挂在外层的
// `v-if` 里面，构造出来的那一刻 `open` 已经是 true，watch 永远不会触发。）
watch(
  () => props.open,
  async (open) => {
    if (!open) return;
    await nextTick();
    box.value?.focus();
  },
);

// 遮罩点击和底部关闭钮都先让输入框失焦，原生 blur 补一次 `change`，编辑才落盘。
// Esc 直接销毁子树，浏览器不会为一个已经不在 DOM 里的输入框补发 `change`——
// 所以这里要在 emit 之前手动 blur 一次，把还没提交的编辑先逼出来。
function closeOnEscape() {
  (document.activeElement as HTMLElement | null)?.blur();
  emit("close");
}
</script>

<template>
  <div
    v-if="open"
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/30 p-3"
    @click.self="emit('close')"
  >
    <div
      ref="box"
      tabindex="-1"
      class="flex max-h-full flex-col gap-4 overflow-auto rounded border bg-white p-4 text-sm outline-none"
      :class="width ?? 'w-[44rem]'"
      @keyup.escape="closeOnEscape"
    >
      <h2 class="font-semibold">{{ title }}</h2>

      <slot />

      <div class="flex justify-end">
        <button class="rounded border px-3 py-1 text-xs" @click="emit('close')">
          {{ t("common.close") }}
        </button>
      </div>
    </div>
  </div>
</template>
