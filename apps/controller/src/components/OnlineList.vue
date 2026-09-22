<script setup lang="ts">
import { computed } from "vue";
import { t } from "../i18n";

/** 一个在线席位，来自 can-fsd 的 datafeed。 */
interface Position {
  cid: string;
  callsign: string;
  freq_khz: number;
  facility: number;
}

const props = defineProps<{
  online: Position[];
  /** 已经在台面上的频率。这些按钮画灰，而不是藏起来。 */
  tuned: number[];
}>();

defineEmits<{ add: [freqKhz: number, callsign: string] }>();

const mhz = (khz: number) => (khz / 1000).toFixed(3);

/**
 * 同频率的多个席位**不合并**。
 *
 * ZSPD_1_TWR 和 ZSPD_2_TWR 可以在同一个频率上，合起来会让人以为只有一个人在。
 */
const rows = computed(() =>
  props.online.map((p) => ({ ...p, already: props.tuned.includes(p.freq_khz) })),
);
</script>

<template>
  <div class="flex min-w-0 flex-wrap items-center gap-1">
    <!-- can-audio 的 PillPushButton（controller/gui.py:889-934）：只显示呼号，
         高 24px，频率进 tooltip。已经加过的画灰而不是藏起来——藏起来的话，
         "没人在线"和"都已经加过了"在界面上长得一模一样。 -->
    <button
      v-for="p in rows"
      :key="`${p.callsign}-${p.freq_khz}`"
      class="h-6 shrink-0 rounded-full border px-3 font-mono text-xs disabled:opacity-40"
      :disabled="p.already"
      :title="
        p.already
          ? t('online.tuned', { frequency: mhz(p.freq_khz) })
          : t('online.add', { frequency: mhz(p.freq_khz) })
      "
      @click="$emit('add', p.freq_khz, p.callsign)"
    >
      {{ p.callsign }}
    </button>
    <p v-if="!rows.length" class="text-xs opacity-50">{{ t("online.none") }}</p>
  </div>
</template>
