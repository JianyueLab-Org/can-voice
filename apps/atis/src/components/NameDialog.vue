<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import { t } from "../i18n";

/**
 * 要一个名字。
 *
 * 替掉 `window.prompt`：它在 Tauri 的 webview 里行为不一致，有的平台直接返回
 * null，于是"新建席位"这个按钮在那些机器上什么也不做，而界面看不出任何异常。
 */
const props = defineProps<{
  open: boolean;
  title: string;
  placeholder?: string;
  initial?: string;
}>();
const emit = defineEmits<{ confirm: [value: string]; cancel: [] }>();

const value = ref("");
const box = ref<HTMLInputElement | null>(null);

watch(
  () => props.open,
  async (open) => {
    if (!open) return;
    value.value = props.initial ?? "";
    await nextTick();
    box.value?.focus();
    box.value?.select();
  },
);

function confirm() {
  const v = value.value.trim();
  if (!v) return;
  emit("confirm", v);
}
</script>

<template>
  <div
    v-if="open"
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/30"
    @click.self="emit('cancel')"
  >
    <div class="flex w-72 flex-col gap-3 rounded border bg-white p-4 text-sm dark:bg-neutral-900">
      <h2 class="font-semibold">{{ title }}</h2>
      <input
        ref="box"
        v-model="value"
        :placeholder="placeholder"
        class="rounded border px-2 py-1"
        @keyup.enter="confirm"
        @keyup.escape="emit('cancel')"
      />
      <div class="flex justify-end gap-2">
        <button class="rounded border px-3 py-1 text-xs" @click="emit('cancel')">
          {{ t("dialog.cancel") }}
        </button>
        <button class="rounded border px-3 py-1 text-xs" :disabled="!value.trim()" @click="confirm">
          {{ t("dialog.confirm") }}
        </button>
      </div>
    </div>
  </div>
</template>
