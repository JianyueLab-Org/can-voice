<script setup lang="ts">
import type { ControllerEntry } from "../types";
import { t } from "../i18n";

defineProps<{ controllers: ControllerEntry[] }>();
const emit = defineEmits<{ (e: "reply", callsign: string): void }>();
</script>

<template>
  <div class="flex flex-col gap-1 overflow-auto text-xs">
    <div v-if="!controllers.length" class="py-4 text-center opacity-50">
      {{ t("lists.no_controllers") }}
    </div>
    <button
      v-for="c in controllers"
      :key="c.callsign"
      class="grid grid-cols-3 gap-2 rounded border px-2 py-1 text-left font-mono tabular-nums"
      @click="emit('reply', c.callsign)"
    >
      <span class="truncate">{{ c.callsign }}</span>
      <span>{{ c.frequency.toFixed(3) }}</span>
      <!-- 距离按本机位置算；不知道本机在哪时是 null，不要显示成 0。 -->
      <span class="text-right opacity-70">
        {{ c.range_nm === null ? "—" : Math.round(c.range_nm) + " nm" }}
      </span>
    </button>
  </div>
</template>
