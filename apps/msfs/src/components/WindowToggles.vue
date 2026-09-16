<script setup lang="ts">
import { appearance, setAppearance } from "../appearance";

/**
 * 置顶、精简、设置——**常驻在顶栏上**，不藏进设置对话框（#45）。
 *
 * 前两个是把窗口压在雷达屏或模拟器上用的，要频繁切。精简模式下它们也必须还在：
 * 旧版 can-audio 写过这条——整条顶栏都藏掉的话，精简之后就没有任何路径能切回来。
 *
 * 四个客户端逐字节相同的一份。
 */
defineEmits<{ settings: [] }>();
</script>

<template>
  <div class="flex shrink-0 items-center gap-1 text-xs">
    <button
      class="rounded border px-2 py-0.5"
      :class="appearance.always_on_top ? 'border-sky-500' : 'opacity-60'"
      :aria-pressed="appearance.always_on_top"
      title="窗口置顶：压在雷达屏或模拟器上面，不被别的窗口盖住"
      @click="setAppearance({ always_on_top: !appearance.always_on_top })"
    >
      置顶
    </button>
    <button
      class="rounded border px-2 py-0.5"
      :class="appearance.compact ? 'border-sky-500' : 'opacity-60'"
      :aria-pressed="appearance.compact"
      title="精简模式：只留值班时要盯的东西，窗口可以缩到很小"
      @click="setAppearance({ compact: !appearance.compact })"
    >
      精简
    </button>
    <!-- 精简时收起：窄窗口里它占的是值班要看的地方。退出精简就回来。 -->
    <button
      v-if="!appearance.compact"
      class="rounded border px-2 py-0.5 opacity-60"
      title="主题、服务器地址、调试日志"
      @click="$emit('settings')"
    >
      设置
    </button>
  </div>
</template>
