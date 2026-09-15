<script setup lang="ts">
import type { TrafficEntry } from "../types";

defineProps<{ traffic: TrafficEntry[] }>();
</script>

<template>
  <div class="flex flex-col gap-1 overflow-auto text-xs">
    <div v-if="!traffic.length" class="py-6 text-center opacity-50">附近没有其他飞机</div>
    <div
      v-for="t in traffic"
      :key="t.callsign"
      class="grid grid-cols-5 gap-2 rounded border px-2 py-1 font-mono tabular-nums"
    >
      <span class="truncate">{{ t.callsign }}</span>
      <span class="truncate opacity-70">{{ t.equipment || "—" }}</span>
      <span>{{ Math.round(t.altitude) }} ft</span>
      <span>{{ Math.round(t.groundspeed) }} kt</span>
      <!-- 距离是按本机位置算的；没有本机位置时是 null，不要显示成 0。 -->
      <span class="text-right">{{ t.range_nm === null ? "—" : t.range_nm.toFixed(1) + " nm" }}</span>
    </div>
  </div>
</template>
