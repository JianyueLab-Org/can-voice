<script setup lang="ts">
import { appearance, setAppearance } from "../appearance";
import { t } from "../i18n";

/**
 * 置顶、精简、设置——**常驻在顶栏上**，不藏进设置对话框（#45）。
 *
 * 前两个是把窗口压在雷达屏或模拟器上用的，要频繁切。精简模式下它们也必须还在：
 * 旧版 can-audio 写过这条——整条顶栏都藏掉的话，精简之后就没有任何路径能切回来。
 *
 * 四个客户端逐字节相同的一份。
 */
/**
 * `only` 挑显示哪几个钮，不传是三个都显示。
 *
 * 通播端要把置顶/精简放进左栏标题行、设置留在顶栏——can-audio 就是这么放的
 * （`atis/gui.py:340-358` 对 `:323`），为的是精简模式下整条顶栏藏起来之后，
 * 置顶和精简还在。另外三个端不传，行为和以前一模一样。
 */
const props = defineProps<{ only?: ("on_top" | "compact" | "settings")[] }>();
const shows = (key: "on_top" | "compact" | "settings") => !props.only || props.only.includes(key);
defineEmits<{ settings: [] }>();
</script>

<template>
  <div class="flex shrink-0 items-center gap-1 text-xs">
    <button
      v-if="shows('on_top')"
      class="rounded border"
      :class="[
        appearance.compact ? 'h-[26px] w-[30px]' : 'px-2 py-0.5',
        appearance.always_on_top ? 'border-sky-500' : 'opacity-60',
      ]"
      :aria-pressed="appearance.always_on_top"
      :title="t('window.on_top_tip')"
      @click="setAppearance({ always_on_top: !appearance.always_on_top })"
    >
      {{ appearance.compact ? t("window.on_top_short") : t("window.on_top") }}
    </button>
    <button
      v-if="shows('compact')"
      class="rounded border"
      :class="[
        appearance.compact ? 'h-[26px] w-[30px]' : 'px-2 py-0.5',
        appearance.compact ? 'border-sky-500' : 'opacity-60',
      ]"
      :aria-pressed="appearance.compact"
      :title="t('window.compact_tip')"
      @click="setAppearance({ compact: !appearance.compact })"
    >
      {{ appearance.compact ? t("window.compact_short") : t("window.compact") }}
    </button>
    <!-- 精简时收起：窄窗口里它占的是值班要看的地方。退出精简就回来。 -->
    <button
      v-if="shows('settings') && !appearance.compact"
      class="rounded border px-2 py-0.5 opacity-60"
      :title="t('window.settings_tip')"
      @click="$emit('settings')"
    >
      {{ t("window.settings") }}
    </button>
  </div>
</template>
