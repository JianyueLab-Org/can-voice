<script setup lang="ts">
import { t } from "../i18n";

interface Radio {
  freq_khz: number;
  rx: boolean;
  tx: boolean;
  xc: boolean;
  gain: number;
  selected: boolean;
  muted: boolean;
  callsign: string;
}

const props = defineProps<{
  radio: Radio;
  receiving: boolean;
  txDenied: boolean;
  rxDenied: boolean;
  locked: boolean;
  transmitAllowed: boolean;
  overTxLimit: { tx: boolean; xc: boolean };
  maxTx: number | null;
  transmitting: boolean;
  lastTalk: { speaker: number; cid?: string; at: number } | null;
  /** CAN 号 → 呼号。最后通话的 who 从这里翻。 */
  roster: Record<string, string>;
  compact?: boolean;
}>();

const emit = defineEmits<{
  switch: [name: "rx" | "tx" | "xc", on: boolean];
  volume: [gain: number];
  mute: [on: boolean];
  select: [];
  remove: [];
}>();

const mhz = (khz: number) => (khz / 1000).toFixed(3);

/** TrackAudio 三态：关 / 开 / 正在响。静音时 RX 整颗变红。 */
const OFF = "#436384";
const ON = "#28a745";
const ACTIVE = "#c7861d";
const MUTED = "#dc3545";

const overLimit = (s: "rx" | "tx" | "xc") =>
  s !== "rx" && props.transmitAllowed && props.maxTx !== null && props.overTxLimit[s];

function switchTitle(s: "rx" | "tx" | "xc"): string {
  if (s !== "rx" && !props.transmitAllowed) return t("radio.no_transmit");
  if (overLimit(s)) return t("radio.over_tx_limit_tip", { max: props.maxTx ?? 0 });
  if (s === "rx") return t("radio.rx_tip");
  if (s === "tx") return t("radio.tx_tip");
  return t("radio.xc_tip");
}

function fill(s: "rx" | "tx" | "xc"): string {
  if (s === "rx" && props.radio.muted) return MUTED;
  if (!props.radio[s]) return OFF;
  if (s === "rx" && props.receiving) return ACTIVE;
  if (s === "tx" && props.transmitting) return ACTIVE;
  return ON;
}

function toggle(s: "rx" | "tx" | "xc") {
  if (s !== "rx" && !props.transmitAllowed) return;
  emit("switch", s, !props.radio[s]);
}

const lastTalkText = () => {
  if (!props.lastTalk) return t("radio.last_rx_none");
  const stamp = new Date(props.lastTalk.at * 1000).toLocaleTimeString();
  const cid = props.lastTalk.cid ?? "";
  const who = (cid && props.roster[cid]) || cid || "—";
  return t("radio.last_rx", { who, stamp });
};
</script>

<template>
  <!-- TrackAudio 卡片：频率是一张卡不是整宽的行，拉宽窗口只是一行多放几张。 -->
  <div
    class="flex flex-col rounded border"
    :class="[
      compact ? 'w-[200px] gap-1 p-2' : 'w-[232px] gap-1.5 p-2.5',
      radio.selected ? 'border-sky-500' : 'border-neutral-300',
      transmitting ? 'ring-1 ring-amber-600' : '',
    ]"
    @click="$emit('select')"
  >
    <div class="flex items-start justify-between gap-2">
      <div class="min-w-0">
        <p class="font-mono text-lg font-semibold tabular-nums leading-tight">
          <span v-if="radio.selected" class="text-sky-600">▸ </span>{{ mhz(radio.freq_khz) }}
        </p>
        <p class="truncate font-mono text-xs opacity-60">
          {{ radio.callsign || t("radio.no_callsign") }}
        </p>
        <p v-if="locked && !compact" class="text-xs text-sky-600" :title="t('radio.staffed_tip')">
          {{ t("radio.staffed") }}
        </p>
      </div>
      <div class="flex flex-col gap-1">
        <button
          type="button"
          class="h-[26px] w-[52px] rounded text-xs font-bold text-white"
          :style="{ background: fill('rx') }"
          :title="switchTitle('rx')"
          @click.stop="toggle('rx')"
        >
          RX
        </button>
        <button
          type="button"
          class="h-[26px] w-[52px] rounded text-xs font-bold text-white"
          :class="!transmitAllowed ? 'opacity-40' : ''"
          :disabled="!transmitAllowed"
          :style="{ background: fill('tx') }"
          :title="switchTitle('tx')"
          @click.stop="toggle('tx')"
        >
          TX
        </button>
      </div>
    </div>

    <div class="flex items-center gap-1">
      <button
        type="button"
        class="h-[22px] w-9 rounded text-xs font-bold text-white"
        :class="!transmitAllowed ? 'opacity-40' : ''"
        :disabled="!transmitAllowed"
        :style="{ background: fill('xc') }"
        :title="switchTitle('xc')"
        @click.stop="toggle('xc')"
      >
        XC
      </button>
      <button
        type="button"
        class="h-[22px] rounded px-1.5 text-xs font-bold text-white"
        :style="{ background: radio.muted ? MUTED : OFF }"
        :title="radio.muted ? t('radio.unmute_tip') : t('radio.mute_tip')"
        @click.stop="$emit('mute', !radio.muted)"
      >
        {{ t("radio.mute") }}
      </button>
      <input
        v-if="!compact"
        type="range"
        min="0"
        max="2"
        step="0.05"
        :value="radio.gain"
        class="h-[18px] min-w-0 flex-1"
        :class="radio.muted ? 'opacity-40' : ''"
        @click.stop
        @input="$emit('volume', Number(($event.target as HTMLInputElement).value))"
      />
      <button
        v-if="!compact"
        class="ml-auto text-xs opacity-50 hover:opacity-100 disabled:opacity-25"
        :disabled="locked"
        :title="locked ? t('radio.remove_locked') : t('radio.remove_tip')"
        @click.stop="$emit('remove')"
      >
        ×
      </button>
    </div>

    <p v-if="!compact" class="text-[11px] opacity-50">{{ lastTalkText() }}</p>

    <p v-if="txDenied" class="text-xs text-amber-600" :title="t('radio.tx_denied_tip')">
      {{ t("radio.tx_denied") }}
    </p>
    <p v-if="rxDenied" class="text-xs text-amber-600" :title="t('radio.rx_denied_tip')">
      {{ t("radio.rx_denied") }}
    </p>
  </div>
</template>
