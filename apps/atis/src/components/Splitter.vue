<script setup lang="ts">
import { ref } from "vue";

/**
 * 两栏可拖分割条。左栏宽度由调用方持有，**只活在会话里**——存下来的话，
 * 换一台屏幕小的机器就得先去拖窗口才看得见右栏。can-audio 也不存
 * （`atis/gui.py:461`）。
 */
const props = withDefaults(
  defineProps<{ width: number; min?: number; max?: number; collapsed?: boolean }>(),
  { min: 180, max: 520, collapsed: false },
);
const emit = defineEmits<{ "update:width": [value: number] }>();

const root = ref<HTMLElement | null>(null);

function grab(event: PointerEvent) {
  const handle = event.currentTarget as HTMLElement;
  // 指针捕获：拖到右栏上面、拖出窗口，事件还是回到这根条上。没有它的话，
  // 手快一点就会"拖着拖着松开了"，而松手那一下发生在别人身上。
  handle.setPointerCapture(event.pointerId);
  const left = root.value?.getBoundingClientRect().left ?? 0;
  const move = (e: PointerEvent) =>
    emit("update:width", Math.min(props.max, Math.max(props.min, e.clientX - left)));
  const done = () => {
    handle.releasePointerCapture(event.pointerId);
    handle.removeEventListener("pointermove", move);
    handle.removeEventListener("pointerup", done);
    handle.removeEventListener("pointercancel", done);
  };
  handle.addEventListener("pointermove", move);
  handle.addEventListener("pointerup", done);
  // 系统抢走指针（触控板手势、窗口失焦）时也要收工，否则条子会黏在手上。
  handle.addEventListener("pointercancel", done);
}
</script>

<template>
  <div ref="root" class="flex min-h-0 flex-1">
    <div
      class="flex min-w-0 flex-col overflow-hidden"
      :class="collapsed ? 'flex-1' : ''"
      :style="collapsed ? undefined : { width: `${width}px`, flex: '0 0 auto' }"
    >
      <slot name="left" />
    </div>
    <div
      v-if="!collapsed"
      class="mx-2 w-1 shrink-0 cursor-col-resize rounded hover:bg-[var(--can-theme)]"
      role="separator"
      aria-orientation="vertical"
      @pointerdown="grab"
    />
    <div v-if="!collapsed" class="flex min-w-0 flex-1 flex-col overflow-hidden">
      <slot name="right" />
    </div>
  </div>
</template>
