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
  /** 这是数据源上本人正在管的那个席位频率。**它不许删。** */
  locked: boolean;
  /** 此刻允不允许发射。不在席位上时 TX / XC 是灰的。 */
  transmitAllowed: boolean;
  /**
   * 打开这一行的 TX / XC 会不会让发射频率超过服务端的上限。Rust 算好的——
   * 开 XC 会顺带开 TX，这里不数。
   */
  overTxLimit: { tx: boolean; xc: boolean };
  /** 服务端的发射上限，提示里那句话要用。`null` = 没连上。 */
  maxTx: number | null;
  /** 这一行此刻正在发射（TX 开着而且 PTT 按着）。 */
  transmitting: boolean;
  /** 这个频率上最近一次通话。`null` = 从挂上到现在没人在这里说过话。 */
  lastTalk: { speaker: number; at: number } | null;
  /** 精简模式：只留频率、呼号、收发开关和静音。音量、最近通话、移除都收起来。 */
  compact?: boolean;
}>();

defineEmits<{
  switch: [name: "rx" | "tx" | "xc", on: boolean];
  volume: [gain: number];
  mute: [on: boolean];
  select: [];
  remove: [];
}>();

const mhz = (khz: number) => (khz / 1000).toFixed(3);

/**
 * 这一格按下去会超额。**只提示，不拦**：拦下来的开关和"不在席位上"长得一模一样，
 * 而先关哪一个腾出位置是管制员的判断，界面替他选不出来。
 */
const overLimit = (s: "rx" | "tx" | "xc") =>
  s !== "rx" && props.transmitAllowed && props.maxTx !== null && props.overTxLimit[s];

function switchTitle(s: "rx" | "tx" | "xc"): string {
  if (s !== "rx" && !props.transmitAllowed) return t("radio.no_transmit");
  if (overLimit(s)) return t("radio.over_tx_limit_tip", { max: props.maxTx ?? 0 });
  return "";
}

/**
 * 最后一次通话是什么时候。
 *
 * 只有时刻，**没有名字**：协议里 `speaker` 是服务端给会话编的号，客户端手上
 * 没有它到 CAN 号的映射，呼号那一半还欠着（见 issue #46）。
 */
const lastTalkText = (talk: { at: number } | null) =>
  talk ? new Date(talk.at * 1000).toLocaleTimeString() : "";
</script>

<template>
  <!-- 正在发射的那一行要一眼看得出来：一个人同时在三个频率上开着 TX 的时候，
       他按下 PTT 说的那句话到底进了哪几条，是要能看见的。 -->
  <div
    class="flex items-center rounded border"
    :class="[
      compact ? 'gap-2 px-2 py-1' : 'gap-3 px-3 py-2',
      radio.selected ? 'border-sky-500' : '',
      transmitting ? 'bg-red-50 ring-1 ring-red-400' : '',
    ]"
  >
    <!-- `selected` 是界面标记，**不发给服务端**：它和服务端的"主频率"是两件
         毫不相干的事，所以字段不叫 primary。 -->
    <button class="w-4 text-sky-600" :title="t('radio.select')" @click="$emit('select')">
      {{ radio.selected ? "▸" : "" }}
    </button>

    <span class="w-20 font-mono tabular-nums">{{ mhz(radio.freq_khz) }}</span>

    <!-- 频率上那个人是谁。只有一个数字的电台行读起来是"121.800"，
         而管制员要找的是"ZSPD_TWR"。 -->
    <span class="w-24 truncate font-mono text-xs opacity-70" :title="radio.callsign">
      {{ radio.callsign }}
    </span>
    <span v-if="locked && !compact" class="text-xs text-sky-600" :title="t('radio.staffed_tip')">
      {{ t("radio.staffed") }}
    </span>

    <!-- RX 灯：这个频率上**有人在讲**，不是"最后一个开口的人还在讲"。 -->
    <span
      class="h-2.5 w-2.5 rounded-full"
      :class="receiving ? 'bg-green-500' : 'bg-neutral-300'"
      :title="receiving ? t('radio.receiving') : ''"
    />

    <!-- 三个开关。**耦合规则在核心库里**，这里只发意图：
         关 RX 会连带清掉 TX/XC，开 TX 会强制开 RX，开 XC 会强制开 RX+TX。
         前端不要自己实现一遍，否则同一条规则就有了两份。 -->
    <label
      v-for="s in (['rx', 'tx', 'xc'] as const)"
      :key="s"
      class="flex items-center gap-1 uppercase"
      :class="[
        s !== 'rx' && !transmitAllowed ? 'opacity-40' : '',
        overLimit(s) ? 'text-amber-600' : '',
      ]"
      :title="switchTitle(s)"
    >
      <input
        type="checkbox"
        :checked="radio[s]"
        :disabled="s !== 'rx' && !transmitAllowed"
        @change="$emit('switch', s, ($event.target as HTMLInputElement).checked)"
      />
      {{ s }}
    </label>

    <!-- 静音是一个开关，不是把音量拉到 0：拉到 0 的话，取消静音回不到用户
         原来调的那个刻度。和关 RX 也不是一件事——那是退订，下次有人叫你时
         连灯都不亮。 -->
    <button
      class="w-6 text-center"
      :class="radio.muted ? 'text-red-600' : 'opacity-50'"
      :title="radio.muted ? t('radio.unmute_tip') : t('radio.mute_tip')"
      @click="$emit('mute', !radio.muted)"
    >
      {{ radio.muted ? "🔇" : "🔈" }}
    </button>

    <input
      v-if="!compact"
      type="range"
      min="0"
      max="2"
      step="0.05"
      :value="radio.gain"
      class="w-24"
      :class="radio.muted ? 'opacity-40' : ''"
      @input="$emit('volume', Number(($event.target as HTMLInputElement).value))"
    />

    <!-- 最后一次通话。绿点只说"此刻有没有人在讲"，而"多久没人说话了"才是
         管制员判断这条频率还活着没有的依据。 -->
    <span
      v-if="lastTalk && !compact"
      class="font-mono text-xs opacity-50"
      :title="t('radio.last_talk')"
    >
      {{ lastTalkText(lastTalk) }}
    </span>

    <!-- 被拒要说出来：一个设好了却不生效、又不知道为什么的开关，
         正是整个重写要逃离的那类故障。 -->
    <span v-if="txDenied" class="text-xs text-amber-600" :title="t('radio.tx_denied_tip')">
      {{ t("radio.tx_denied") }}
    </span>
    <span v-if="rxDenied" class="text-xs text-amber-600" :title="t('radio.rx_denied_tip')">
      {{ t("radio.rx_denied") }}
    </span>

    <!-- 本席频率删不掉：删掉它的人还坐在席位上，而飞行员在那个频率上叫他
         听不见，两边都以为对方在。 -->
    <button
      v-if="!compact"
      class="ml-auto text-xs opacity-60 hover:opacity-100 disabled:opacity-25 disabled:hover:opacity-25"
      :disabled="locked"
      :title="locked ? t('radio.remove_locked') : ''"
      @click="$emit('remove')"
    >
      {{ t("radio.remove") }}
    </button>
  </div>
</template>
