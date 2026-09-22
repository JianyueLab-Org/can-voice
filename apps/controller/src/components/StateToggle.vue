<script setup lang="ts">
/**
 * 三态开关。can-audio `controller/gui.py:202-238` 那个手绘的 StateToggle。
 *
 * 旧版刻意不用 qfluentwidgets 的按钮：那个库会把主题色重新刷上去，三种状态就
 * 扁成一种。这边的等价问题是 Tailwind 的颜色工具类会被 style.css 的深色覆盖
 * 改写，所以颜色走 `--can-*`，尺寸走行内 style——**这颗按钮讲的是电台的状态，
 * 不跟主题走**。
 *
 * 边框是填充色压暗到 71%（旧版 `fill.darker(140)`）。
 *
 * 用到它的客户端：controller。xpc / msfs 的 TX / RX 色块在后面的计划里接上。
 * 逐字节相同的一份。
 */
const props = defineProps<{
  label: string;
  state: "off" | "on" | "active" | "muted";
  /** 宽高（px）。can-audio 的尺寸：RX / TX 52×26，XC 36×22，静音 46×22。 */
  width: number;
  height: number;
  disabled?: boolean;
}>();

defineEmits<{ press: [] }>();

const fill = () => `var(--can-${props.state})`;
</script>

<template>
  <button
    type="button"
    class="shrink-0 rounded border text-xs font-bold text-white"
    :class="disabled ? 'opacity-40' : ''"
    :style="{
      background: fill(),
      borderColor: `color-mix(in srgb, ${fill()} 71%, black)`,
      width: `${width}px`,
      height: `${height}px`,
    }"
    :disabled="disabled"
    @click.stop="$emit('press')"
  >
    {{ label }}
  </button>
</template>
