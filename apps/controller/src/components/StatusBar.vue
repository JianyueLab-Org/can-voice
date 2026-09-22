<script setup lang="ts">
/**
 * 底栏。can-audio `controller/gui.py:650-668`：PTT 灯、占满剩余宽度的状态文案、
 * 值守文案。
 *
 * **PTT 在这里是按钮，旧版是纯指示灯。** Linux 上全局按键监听走 Xlib，Wayland
 * 不给一个普通程序监听全局按键（见 README），所以屏幕上这一颗是那种情况下唯一
 * 能发话的路径，不能退化成一个只会亮的灯。
 *
 * 插槽在值守文案之后，给各客户端自己那一格——管制端放链路健康统计。
 *
 * 用到它的客户端：controller。xpc / msfs 的状态栏在后面的计划里接上。
 * 逐字节相同的一份。
 */
defineProps<{
  /** 正在发话。灯转成 active 色。 */
  talking: boolean;
  /** 中间那句话。can-audio 空闲时是"就绪"。 */
  status: string;
  /** 右边那句话。 */
  duty: string;
  /** 在席位上。着绿，旧版如此。 */
  dutyOn: boolean;
  /** PTT 按钮的提示文字。各客户端自己传，因为这是共用组件。 */
  pttTitle: string;
}>();

defineEmits<{ down: [e: PointerEvent]; up: [] }>();
</script>

<template>
  <footer class="flex items-center gap-3 border-t pt-3 text-xs">
    <button
      class="flex shrink-0 items-center gap-2"
      :title="pttTitle"
      @pointerdown="$emit('down', $event)"
      @pointerup="$emit('up')"
      @pointercancel="$emit('up')"
    >
      <span
        class="inline-block h-3 w-3 rounded-full"
        :style="{ background: talking ? 'var(--can-active)' : 'var(--can-idle)' }"
      />
      <span
        class="text-xs"
        :class="talking ? 'font-bold' : 'opacity-60'"
        :style="talking ? { color: 'var(--can-active)' } : {}"
      >
        PTT
      </span>
    </button>
    <span class="min-w-0 flex-1 truncate opacity-70">{{ status }}</span>
    <span
      class="shrink-0"
      :class="dutyOn ? '' : 'opacity-70'"
      :style="dutyOn ? { color: 'var(--can-on)' } : {}"
    >
      {{ duty }}
    </span>
    <slot />
  </footer>
</template>
