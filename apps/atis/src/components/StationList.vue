<script setup lang="ts">
import type { Live, Station } from "../types";
import { callsignOf, stationSummary } from "../types";
import { t } from "../i18n";

const props = defineProps<{
  stations: Station[];
  live: Record<string, Live>;
  selected: string;
}>();
defineEmits<{ pick: [callsign: string] }>();

/**
 * 在播那个圆点的颜色。can-audio 的圆点不上色（`atis/script.py:105` 就是一个字符），
 * 这边上——状态是 can-voice 有而 can-audio 没有的东西，§1 说留着。
 */
function dot(callsign: string): string | null {
  const state = props.live[callsign]?.state;
  if (!state) return null;
  if (state === "Online") return "var(--can-on)";
  if (state === "Connecting" || state === "Reconnecting") return "var(--can-active)";
  if (state === "Error" || state === "Offline") return "var(--can-muted)";
  return "var(--can-idle)";
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col overflow-auto">
    <button
      v-for="s in stations"
      :key="callsignOf(s)"
      class="flex items-center gap-2 rounded px-2 py-1 text-left font-mono text-xs"
      :class="callsignOf(s) === selected ? 'bg-[var(--can-off)] text-white' : 'hover:opacity-80'"
      @click="$emit('pick', callsignOf(s))"
    >
      <span
        class="w-2 shrink-0"
        :style="{ color: dot(callsignOf(s)) ?? 'transparent' }"
        aria-hidden="true"
        >●</span
      >
      <span class="truncate">{{ stationSummary(s, live[callsignOf(s)]) }}</span>
    </button>
    <p v-if="!stations.length" class="px-2 py-1 text-xs opacity-60">{{ t("station.none") }}</p>
  </div>
</template>
