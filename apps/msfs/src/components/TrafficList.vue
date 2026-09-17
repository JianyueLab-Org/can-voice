<script setup lang="ts">
import type { TrafficEntry } from "../types";
import { t } from "../i18n";

defineProps<{ traffic: TrafficEntry[] }>();
</script>

<template>
  <div class="flex flex-col gap-1 overflow-auto text-xs">
    <div v-if="!traffic.length" class="py-6 text-center opacity-50">
      {{ t("lists.no_traffic") }}
    </div>
    <!-- 循环变量不叫 `t`：那个名字是翻译函数。 -->
    <div
      v-for="a in traffic"
      :key="a.callsign"
      class="grid grid-cols-5 gap-2 rounded border px-2 py-1 font-mono tabular-nums"
    >
      <span class="truncate">{{ a.callsign }}</span>
      <span class="truncate opacity-70">{{ a.equipment || "—" }}</span>
      <span>{{ Math.round(a.altitude) }} ft</span>
      <span>{{ Math.round(a.groundspeed) }} kt</span>
      <!-- 距离是按本机位置算的；没有本机位置时是 null，不要显示成 0。 -->
      <span class="text-right">{{ a.range_nm === null ? "—" : a.range_nm.toFixed(1) + " nm" }}</span>
    </div>
  </div>
</template>
