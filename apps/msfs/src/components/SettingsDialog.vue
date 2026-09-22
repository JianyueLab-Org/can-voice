<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import SettingsCommon from "./SettingsCommon.vue";
import { t } from "../i18n";

/**
 * 设置对话框的外壳（#45）。
 *
 * **这个文件每个客户端一份，不再共用**：共有的三段在 SettingsCommon 里，
 * 各客户端自己那几段直接写在这里。
 */
const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ close: [] }>();

const box = ref<HTMLElement | null>(null);
watch(
  () => props.open,
  async (open) => {
    if (!open) return;
    await nextTick();
    box.value?.focus();
  },
);
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
      class="flex max-h-full w-[30rem] flex-col gap-4 overflow-auto rounded border bg-white p-4 text-sm outline-none"
      @keyup.escape="emit('close')"
    >
      <h2 class="font-semibold">{{ t("settings.title") }}</h2>

      <SettingsCommon :open="open" />
      <slot />

      <div class="flex justify-end">
        <button class="rounded border px-3 py-1 text-xs" @click="emit('close')">
          {{ t("common.close") }}
        </button>
      </div>
    </div>
  </div>
</template>
