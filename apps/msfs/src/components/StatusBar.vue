<script setup lang="ts">
import { onUnmounted } from "vue";

/**
 * 底栏。can-audio `controller/gui.py:650-668`：PTT 灯、占满剩余宽度的状态文案、
 * 值守文案。
 *
 * **PTT 在这里是按钮，旧版是纯指示灯。** Linux 上全局按键监听走 Xlib，Wayland
 * 不给一个普通程序监听全局按键（见 README），所以屏幕上这一颗是那种情况下唯一
 * 能发话的路径，不能退化成一个只会亮的灯。
 *
 * **PTT 那格和值守那格都是可选的。** 两个飞行员端的状态栏是"同一条栏少几格"
 * （spec §3、§6）：它们只显示"就绪"和瞬时状态，PTT 在无线电那一行上，也没有
 * 值守这个概念。不传就不画，那一条就是 can-audio 飞行员端的 `QStatusBar`；
 * 要它们传一串假值、或者各自分叉一份副本，都会把这个文件从共用件上拆下来。
 *
 * 插槽在值守文案之后，给各客户端自己那一格——管制端放链路健康统计。
 *
 * 用到它的客户端：controller、xpc、msfs。逐字节相同的一份。
 */
defineProps<{
  /** 正在发话。灯转成 active 色。 */
  talking: boolean;
  /** 中间那句话。can-audio 空闲时是"就绪"。 */
  status: string;
  /** 右边那句话。不传就没有这一格。 */
  duty?: string;
  /** 在席位上。着绿，旧版如此。 */
  dutyOn?: boolean;
  /** PTT 按钮的提示文字。各客户端自己传，因为这是共用组件；不传就没有这颗按钮。 */
  pttTitle?: string;
}>();

const emit = defineEmits<{ down: [e: PointerEvent]; up: [] }>();

/**
 * 指针捕获放在这里，不放在调用方。
 *
 * 按住说话时手是会动的：没有捕获，指针一滑出按钮，`pointerup` 就落到别的元素上，
 * 松手事件永远不来，麦克风一直开着。每个接这条栏的客户端各写一遍这一句，
 * 迟早漏掉一个。
 */
function down(e: PointerEvent) {
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  emit("down", e);
}

/**
 * **拆掉这条栏也要松手。** `down` 之后唯一的回程是这颗按钮自己的 `pointerup`；
 * 按着的时候它从 DOM 上消失（在设置里切精简、链路掉到重连把这一页换掉），
 * 那个事件就再也不会来，而麦克风还开着。调用方的松手处理是幂等的。
 */
onUnmounted(() => emit("up"));
</script>

<template>
  <footer class="flex items-center gap-3 border-t pt-3 text-xs">
    <button
      v-if="pttTitle"
      class="flex shrink-0 items-center gap-2"
      :title="pttTitle"
      @pointerdown="down"
      @pointerup="emit('up')"
      @pointercancel="emit('up')"
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
      v-if="duty"
      class="shrink-0"
      :class="dutyOn ? '' : 'opacity-70'"
      :style="dutyOn ? { color: 'var(--can-on)' } : {}"
    >
      {{ duty }}
    </span>
    <slot />
  </footer>
</template>
