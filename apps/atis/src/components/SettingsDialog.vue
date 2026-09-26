<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import SettingsCommon from "./SettingsCommon.vue";
import { t } from "../i18n";

/**
 * 设置对话框的外壳（#45）。
 *
 * 共有的三段在 SettingsCommon 里；各客户端自己那几段从 `<slot />` 进来，所以
 * **四份至今逐字节相同**，也就登记在 `SHARED_FRONTEND` 上——改了一份忘了其余三份，
 * CI 会失败。真到某个计划把它们改得不一样了，那时候再把那一行从清单上删掉。
 */
const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ close: [] }>();

const box = ref<HTMLElement | null>(null);
const titleId = `settings-title-${Math.random().toString(36).slice(2)}`;
let previousFocus: HTMLElement | null = null;
const inertSiblings = new Map<HTMLElement, boolean>();

function setBackgroundInert(active: boolean) {
  const host = box.value?.parentElement;
  const parent = host?.parentElement;
  if (!parent) return;
  if (active) {
    for (const child of Array.from(parent.children)) {
      if (child === host || !(child instanceof HTMLElement)) continue;
      inertSiblings.set(child, child.inert);
      child.inert = true;
    }
  } else {
    for (const [child, wasInert] of inertSiblings) child.inert = wasInert;
    inertSiblings.clear();
  }
}
watch(
  () => props.open,
  async (open) => {
    if (!open) {
      setBackgroundInert(false);
      previousFocus?.focus();
      previousFocus = null;
      return;
    }
    previousFocus = document.activeElement as HTMLElement | null;
    await nextTick();
    setBackgroundInert(true);
    const first = box.value?.querySelector<HTMLElement>(
      'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled])',
    );
    (first ?? box.value)?.focus();
  },
);

function onKeydown(e: KeyboardEvent) {
  if (e.key === "Escape") {
    e.preventDefault();
    close();
    return;
  }
  if (e.key !== "Tab" || !box.value) return;
  const focusable = [...box.value.querySelectorAll<HTMLElement>(
    'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled])',
  )];
  if (!focusable.length) return;
  const current = document.activeElement;
  const index = focusable.indexOf(current as HTMLElement);
  const next = e.shiftKey
    ? focusable[(index <= 0 ? focusable.length : index) - 1]
    : focusable[(index + 1) % focusable.length];
  e.preventDefault();
  next.focus();
}

// 三条关闭路径（遮罩点击、「关闭」钮、Esc）都先手动 blur 当前焦点元素再 emit
// `close`，编辑才保证落盘。遮罩点击和「关闭」钮原先指望浏览器点击时自己把输入框
// 失焦、补一次原生 `change`——这只是假设，没有验证过。`Esc` 会直接销毁子树，
// 浏览器不会为一个已经不在 DOM 里的输入框补发 `change`，所以三条路径统一走这里。
function close() {
  (document.activeElement as HTMLElement | null)?.blur();
  emit("close");
}
</script>

<template>
  <div
    v-if="open"
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/30 p-3"
    role="presentation"
    @click.self="close"
  >
    <div
      ref="box"
      role="dialog"
      aria-modal="true"
      :aria-labelledby="titleId"
      tabindex="-1"
      class="flex max-h-full w-[30rem] flex-col gap-4 overflow-auto rounded border bg-white p-4 text-sm outline-none"
      @keydown="onKeydown"
    >
      <h2 :id="titleId" class="font-semibold">{{ t("settings.title") }}</h2>

      <SettingsCommon :open="open" />
      <slot />

      <div class="flex justify-end">
        <button class="rounded border px-3 py-1 text-xs" @click="close">
          {{ t("common.close") }}
        </button>
      </div>
    </div>
  </div>
</template>
