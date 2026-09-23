<script setup lang="ts">
/**
 * 带标题的卡片。can-audio 的 `Card(HeaderCardWidget)`（`xpc/gui.py:111`）：
 * 一行标题，一条分隔线，下面是内容。
 *
 * **底色写 `bg-white` 而不是 `var(--can-surface)`。** `style.css` 里
 * `.dark .bg-white` 就映到 `--can-surface`，所以这一个类在两套主题下都对；
 * 直接写那个变量的话，浅色主题下卡片会是深的。
 *
 * 用到它的客户端：xpc、msfs。**登记在 `SHARED_FRONTEND` 上**——不登记的副本
 * 会无声地漂开。
 */
defineProps<{ title: string }>();
</script>

<template>
  <section class="flex min-w-0 flex-col rounded border bg-white">
    <h2 class="border-b px-3 py-2 text-xs font-semibold opacity-80">{{ title }}</h2>
    <div class="flex min-h-0 min-w-0 flex-1 flex-col gap-2 p-3">
      <slot />
    </div>
  </section>
</template>
