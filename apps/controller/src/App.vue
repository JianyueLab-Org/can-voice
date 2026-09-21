<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import WindowToggles from "./components/WindowToggles.vue";
import SettingsDialog from "./components/SettingsDialog.vue";
import { appearance, loadAppearance } from "./appearance";
import { invoke } from "@tauri-apps/api/core";
import RadioRow from "./components/RadioRow.vue";
import UpdateBanner from "./components/UpdateBanner.vue";
import StartupGate from "./components/StartupGate.vue";
import SettingsPanel from "./components/SettingsPanel.vue";
import OnlineList from "./components/OnlineList.vue";
import { errorText, t } from "./i18n";
import { attachPttKeys } from "./pttKeys";

type LinkState = "Connecting" | "Online" | "Reconnecting" | "Offline" | "Evicted";
type Ended = "Offline" | "Evicted" | { Refused: string | { Other: string } };

interface Radio {
  freq_khz: number;
  rx: boolean;
  tx: boolean;
  xc: boolean;
  gain: number;
  selected: boolean;
  /** 静音。`gain` 照旧留着——取消静音回到原来那个位置。 */
  muted: boolean;
  /** 这个频率上那个席位的呼号。查不到就是空的。 */
  callsign: string;
}

/** 一个在线席位，来自 can-fsd 的 datafeed。 */
interface Position {
  cid: string;
  callsign: string;
  freq_khz: number;
  facility: number;
}

/**
 * 数据源那一份快照。
 *
 * **语音服务端不知道谁在管哪个席位**，那是 FSD 的事实。所以"我在管什么"
 * 只能从 datafeed 查，而查不到（`reachable === false`）和"确实不在管制"
 * 是两件不同的事。
 */
interface FeedView {
  duty: { callsign: string; freq_khz: number | null; dropped_tx: boolean };
  online: Position[];
  roster: Record<string, string>;
  /** 此刻允不允许发射。台面自己的状态，界面照着画灰。 */
  transmit_allowed: boolean;
  reachable: boolean;
}

interface Snapshot {
  link: LinkState;
  ended: Ended | null;
  receiving: Record<string, number[]>;
  denied_tx: number[];
  denied_rx: number[];
  denied_xc: number[][];
  health: {
    rtt_ms: number;
    sent: number;
    received: number;
    lost: number;
    unparsable: number;
  } | null;
  /** 服务端的其它通知：`[kind, freq_khz, reason]`，最近的在最后。 */
  notices: [string, number, string][];
  /** 每个频率上最近一次通话。键是频率（kHz）的十进制写法。 */
  last_talk: Record<string, { speaker: number; cid?: string; at: number }>;
  /** 服务端给这条链路的发射上限。没连上、掉了线是 `null`。 */
  max_tx: number | null;
  /**
   * 台面相对 `max_tx` 的处境。`max_tx` 是 `null` 时它也是，界面据此什么都不说。
   *
   * **哪一格会超额是 Rust 算好的**：开 XC 会顺带开 TX，这里自己数就得把耦合规则
   * 再写一遍。
   */
  tx_budget: {
    max_tx: number;
    /** 此刻会声明几个 TX。可以已经超过上限：重连时整份重放的台面可能比这一次的上限多。 */
    declared: number;
    /** 在这些频率上打开 TX 会超额。 */
    tx_over: number[];
    /** 在这些频率上打开 XC 会超额。 */
    xc_over: number[];
  } | null;
}

/** 设置对话框开没开。 */
const showPrefs = ref(false);
/** 精简模式：只留值班时要盯的东西。开关在 WindowToggles 里，真相在设置文件里。 */
const compact = computed(() => appearance.value.compact);

const cid = ref("");
const password = ref("");
const busy = ref(false);

/**
 * 要显示的那条错误。
 *
 * **存原样的东西，渲染时再翻**：存一句翻好的话，切了语言之后已经显示着的那一句
 * 还是原来的语言。
 */
type Problem = { kind: "command"; error: unknown } | { kind: "frequency" };
const error = ref<Problem | null>(null);

function problemText(p: Problem): string {
  switch (p.kind) {
    case "frequency":
      return t("freq.out_of_band");
    case "command":
      return errorText(p.error);
  }
}

const freqInput = ref("");
const callsignInput = ref("");
const radios = ref<Radio[]>([]);
const snap = ref<Snapshot | null>(null);
const feed = ref<FeedView | null>(null);
const pressed = ref(false);
/** 屏幕按钮按着。灯要立刻亮，不能等 200ms 那一拍快照。 */
const holding = ref(false);
const talking = computed(() => holding.value || pressed.value);
const showSettings = ref(false);

let timer: number | undefined;

// 界面**读快照**，不从事件流拼：事件是广播，窗口重开之前发生的事收不到。
async function refresh() {
  snap.value = await invoke<Snapshot>("snapshot");
  radios.value = await invoke<Radio[]>("radios");
  pressed.value = await invoke<boolean>("ptt_pressed");
  feed.value = await invoke<FeedView>("feed");
}

let detachPtt: (() => void) | undefined;
onMounted(async () => {
  void loadAppearance();
  // 上次用的 CAN 号预填。密码不存——它只换一张 60 秒的票。
  cid.value = (await invoke<{ cid: string }>("settings")).cid;
  await refresh();
  timer = window.setInterval(refresh, 200);
  detachPtt = attachPttKeys();
});
onUnmounted(() => {
  window.clearInterval(timer);
  detachPtt?.();
});

const connected = computed(() => snap.value?.link === "Online");

/** 三种终态要说三句不同的话。 */
const statusText = computed(() => {
  const s = snap.value;
  if (!s) return "…";
  switch (s.link) {
    case "Online":
      return t("link.connected");
    case "Connecting":
      return t("link.connecting");
    // **`Reconnecting` 与 `Offline` 是对立的**：前者链路还活着，别把台面清掉。
    case "Reconnecting":
      return t("link.reconnecting");
    case "Evicted":
      return t("link.evicted");
    case "Offline":
      return endedText(s.ended);
  }
});

function endedText(ended: Ended | null): string {
  if (!ended || ended === "Offline") return t("ended.offline");
  if (ended === "Evicted") return t("ended.evicted");
  const r = ended.Refused;
  // `proto_unsupported` 的动作和 `refused` 一样，但**要说的话不一样**：
  // 告诉一个版本太旧的人"被拒绝"，他会去查密码、去怀疑账号，而原因是那个旧 exe。
  if (r === "ProtoUnsupported") return t("ended.proto_unsupported");
  if (r === "TokenExpired") return t("ended.token_expired");
  if (r === "TokenInvalid" || r === "Refused") return t("ended.server_refused");
  return t("ended.refused");
}

async function connect() {
  error.value = null;
  busy.value = true;
  try {
    await invoke("connect", { cid: cid.value, password: password.value });
    // 密码用过就丢：它只需要换一张 60 秒的票，之后重连带的是票不是密码。
    password.value = "";
    await refresh();
  } catch (e) {
    error.value = { kind: "command", error: e };
  } finally {
    busy.value = false;
  }
}

async function disconnect() {
  await invoke("disconnect");
  await refresh();
}

/** `121.800` / `121800` 都收，统一成 kHz。 */
function parseFreq(raw: string): number | null {
  // 不叫 `t`：那是翻译函数的名字。
  const text = raw.trim();
  if (!text) return null;
  const n = Number(text);
  if (!Number.isFinite(n)) return null;
  const khz = text.includes(".") ? Math.round(n * 1000) : Math.round(n);
  // 夹在 VHF 波段里：服务端把 freq_khz 当不透明路由键，不做范围校验，
  // 所以打错的那个数会变成一个谁也不在的频率，而一切看起来正常。
  return khz >= 118000 && khz <= 136975 ? khz : null;
}

async function addFrequency() {
  const khz = parseFreq(freqInput.value);
  if (khz === null) {
    error.value = { kind: "frequency" };
    return;
  }
  error.value = null;
  freqInput.value = "";
  const callsign = callsignInput.value.trim();
  callsignInput.value = "";
  await invoke("add_frequency", { freqKhz: khz, callsign: callsign || null });
  await refresh();
}

/** 从在线一览点过来的，把呼号一起带上。 */
const addOnline = (khz: number, callsign: string) =>
  act("add_frequency", { freqKhz: khz, callsign });

/** 在席位上没有。**查不到不算不在**——那两句话要分开说。 */
const onDuty = computed(() => !!feed.value?.duty.callsign);
const tuned = computed(() => radios.value.map((r) => r.freq_khz));
const locked = (khz: number) => feed.value?.duty.freq_khz === khz;
/** 画灰与否照着台面的真相，不自己推：推出来的那份迟早和它对不上。 */
const mayTransmit = computed(() => feed.value?.transmit_allowed ?? true);

function pttDown(e: PointerEvent) {
  holding.value = true;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  void invoke("set_transmitting", { on: true });
}
function pttUp() {
  holding.value = false;
  void invoke("set_transmitting", { on: false });
}

/** 这一行此刻正在发射：TX 开着，而且 PTT 按着。 */
function isTransmitting(r: Radio): boolean {
  return talking.value && r.tx;
}
function lastTalk(khz: number) {
  return snap.value?.last_talk?.[String(khz)] ?? null;
}
function isReceiving(khz: number): boolean {
  return (snap.value?.receiving?.[String(khz)]?.length ?? 0) > 0;
}
function txDenied(khz: number): boolean {
  return snap.value?.denied_tx?.includes(khz) ?? false;
}
function rxDenied(khz: number): boolean {
  return snap.value?.denied_rx?.includes(khz) ?? false;
}

/** 发射上限的处境。没连上就是 `null`。 */
const txBudget = computed(() => snap.value?.tx_budget ?? null);
/** 这一行再开 TX / XC 会不会超额。照着 Rust 给的单子查，不自己数。 */
function overTxLimit(khz: number) {
  const b = txBudget.value;
  return { tx: b?.tx_over.includes(khz) ?? false, xc: b?.xc_over.includes(khz) ?? false };
}

/**
 * 服务端通知的人话。
 *
 * **不认识的 kind 也要显示出来**：一条服务端认为值得说、而客户端太旧不认识的
 * 通知，落到界面上是一句原文，总好过一片安静。
 */
function noticeText([kind, freq, reason]: [string, number, string]): string {
  switch (kind) {
    case "audio_unavailable":
      return t("notice.audio_unavailable");
    case "range_unavailable":
      return t("notice.range_unavailable");
    case "unknown_message":
      return t("notice.unknown_message", { reason });
    // 服务端按**会话**算这个桶，freq 恒为 0，所以这一条不能落进 other_on 去
    // 说成某一条频率的问题。
    case "rate_limited":
      return t("notice.rate_limited");
    default:
      return freq
        ? t("notice.other_on", { kind, frequency: (freq / 1000).toFixed(3), reason })
        : t("notice.other", { kind, reason });
  }
}

/** 被夹掉的耦合对。设了不生效而界面不说，是这条最初的样子。 */
const deniedPairs = computed(() =>
  (snap.value?.denied_xc ?? []).map(
    ([a, b]) => `${(a / 1000).toFixed(3)} ↔ ${(b / 1000).toFixed(3)}`,
  ),
);

async function act(name: string, args: Record<string, unknown>) {
  await invoke(name, args);
  await refresh();
}
</script>

<template>
  <StartupGate>
    <!-- 精简时留白也跟着缩：留着正常模式的边距，一张卡的窗口里有一半是空的。 -->
    <main
      class="flex h-screen w-full flex-col text-sm"
      :class="compact ? 'gap-2 p-2' : 'gap-4 p-5'"
    >
      <header class="flex flex-wrap items-center justify-between gap-3">
        <div class="min-w-0">
          <h1 v-if="!compact" class="text-base font-semibold">{{ t("app.title") }}</h1>
          <p class="flex items-center gap-2 truncate text-xs opacity-70">
            <span
              class="inline-block h-2.5 w-2.5 shrink-0 rounded-full"
              :style="{ background: connected ? '#28a745' : '#dc3545' }"
            />
            {{ statusText }}
          </p>
        </div>
        <!-- 连接状态、置顶、精简**精简时也都在**：藏掉的话精简之后就切不回来了。 -->
        <WindowToggles class="ml-auto" @settings="showPrefs = true" />
        <div v-if="!connected" class="flex items-center gap-2">
          <input v-model="cid" :placeholder="t('login.cid')" class="w-24 rounded border px-2 py-1" />
          <input
            v-model="password"
            type="password"
            :placeholder="t('login.password')"
            class="w-32 rounded border px-2 py-1"
            @keyup.enter="connect"
          />
          <button :disabled="busy" class="rounded border px-3 py-1" @click="connect">
            {{ t("login.connect") }}
          </button>
        </div>
        <!-- 精简时收起断开：和旧版一样，精简就是在值班，那颗按钮在窄窗口里只会被误点。 -->
        <button v-else-if="!compact" class="rounded border px-3 py-1" @click="disconnect">
          {{ t("login.disconnect") }}
        </button>
      </header>

      <UpdateBanner v-if="!compact" />

      <p v-if="error" class="rounded border border-red-400 px-3 py-2 text-xs text-red-600">
        {{ problemText(error) }}
      </p>

      <!-- 服务端说的话。不显示的话，"能连上、状态绿、说话没人听见"就是全部症状。 -->
      <p
        v-for="(n, i) in snap?.notices ?? []"
        :key="`${n[0]}-${n[1]}-${i}`"
        class="rounded border border-amber-400 px-3 py-2 text-xs text-amber-700"
      >
        {{ noticeText(n) }}
      </p>

      <p
        v-if="deniedPairs.length"
        class="rounded border border-amber-400 px-3 py-2 text-xs text-amber-700"
      >
        {{ t("radio.xc_denied", { pairs: deniedPairs.join(t("common.separator.list")) }) }}
      </p>

      <!-- 发射频率数对着服务端的上限。**要在声明之前说**：超额时服务端只是把多出来的
           拒掉，只靠"发射被拒"的话，人是按下去之后才知道的。
           超额那一句是给重连的：存下来的台面整份重放，而这一次的上限可能比存的时候低。 -->
      <p
        v-if="txBudget && txBudget.declared > txBudget.max_tx"
        class="rounded border border-amber-400 px-3 py-2 text-xs text-amber-700"
      >
        {{ t("radio.tx_over_limit", { declared: txBudget.declared, max: txBudget.max_tx }) }}
      </p>
      <p
        v-else-if="txBudget && txBudget.declared === txBudget.max_tx"
        class="rounded border border-neutral-400 px-3 py-2 text-xs opacity-70"
      >
        {{ t("radio.tx_at_limit", { max: txBudget.max_tx }) }}
      </p>

      <!-- 在不在席位上。这件事此前界面上完全没有，而它决定了能不能发射。 -->
      <p
        v-if="connected && !onDuty"
        class="rounded border px-3 py-2 text-xs"
        :class="
          feed?.reachable
            ? 'border-amber-400 text-amber-700'
            : 'border-neutral-400 opacity-70'
        "
      >
        <template v-if="feed?.reachable">
          {{ t("duty.off") }}
          <span v-if="feed?.duty.dropped_tx">{{ t("duty.dropped_tx") }}</span>
        </template>
        <template v-else>{{ t("duty.unknown") }}</template>
      </p>
      <p v-else-if="connected" class="text-xs text-sky-700">
        {{ t("duty.on", { callsign: feed?.duty.callsign ?? "" }) }}
        <span v-if="feed?.duty.freq_khz" class="font-mono opacity-70">
          · {{ (feed.duty.freq_khz / 1000).toFixed(3) }}
        </span>
      </p>

      <section v-if="!compact" class="flex items-center gap-2">
        <input
          v-model="freqInput"
          :placeholder="t('freq.hint')"
          class="w-28 rounded border px-2 py-1"
          @keyup.enter="addFrequency"
        />
        <input
          v-model="callsignInput"
          :placeholder="t('freq.callsign_hint')"
          class="w-40 rounded border px-2 py-1 font-mono uppercase"
          @keyup.enter="addFrequency"
        />
        <button class="rounded border px-3 py-1" @click="addFrequency">
          {{ t("freq.add") }}
        </button>
        <button class="ml-auto rounded border px-3 py-1" @click="showSettings = !showSettings">
          {{ showSettings ? t("panel.close") : t("panel.open") }}
        </button>
      </section>

      <SettingsPanel v-if="showSettings && !compact" :cid="cid" />

      <section class="flex min-h-0 flex-1 flex-col">
        <div class="flex flex-1 flex-wrap content-start gap-2 overflow-auto">
          <RadioRow
            v-for="r in radios"
            :key="r.freq_khz"
            :radio="r"
            :receiving="isReceiving(r.freq_khz)"
            :tx-denied="txDenied(r.freq_khz)"
            :rx-denied="rxDenied(r.freq_khz)"
            @switch="(s, on) => act('set_switch', { freqKhz: r.freq_khz, switch: s, on })"
            @volume="(g) => act('set_volume', { freqKhz: r.freq_khz, gain: g })"
            @mute="(on) => act('set_muted', { freqKhz: r.freq_khz, on })"
            @select="act('set_selected', { freqKhz: r.freq_khz })"
            :locked="locked(r.freq_khz)"
            :transmitting="isTransmitting(r)"
            :last-talk="lastTalk(r.freq_khz)"
            :roster="feed?.roster ?? {}"
            :compact="compact"
            :transmit-allowed="mayTransmit"
            :over-tx-limit="overTxLimit(r.freq_khz)"
            :max-tx="txBudget?.max_tx ?? null"
            @remove="act('remove_frequency', { freqKhz: r.freq_khz })"
          />
          <p v-if="!radios.length" class="w-full py-6 text-center text-xs opacity-50">
            {{ t("freq.empty") }}
          </p>
        </div>

        <!-- 在线一览。没有它的话，加一个别人的频率要先去别的地方查他在守什么。 -->
        <div v-if="connected && !compact" class="mt-2 flex flex-col gap-1 border-t pt-2">
          <p class="text-xs opacity-60">{{ t("online.title") }}</p>
          <OnlineList :online="feed?.online ?? []" :tuned="tuned" @add="addOnline" />
        </div>
      </section>

      <footer
        v-if="!compact"
        class="flex items-center justify-between gap-3 border-t pt-3 text-xs"
      >
        <button
          class="flex items-center gap-2"
          :title="t('ptt.hold_tip')"
          @pointerdown="pttDown"
          @pointerup="pttUp"
          @pointercancel="pttUp"
        >
          <span
            class="inline-block h-3 w-3 rounded-full"
            :style="{ background: talking ? '#c7861d' : '#8b90a4' }"
          />
          <span
            class="text-xs"
            :class="talking ? 'font-bold' : 'opacity-60'"
            :style="talking ? { color: '#c7861d' } : {}"
          >PTT</span>
        </button>
        <span class="opacity-70">
          <template v-if="connected && onDuty">{{ t("duty.staffing", { callsign: feed?.duty.callsign ?? "" }) }}</template>
          <template v-else-if="connected">{{ t("duty.observer") }}</template>
        </span>
        <span v-if="snap?.health && !compact" class="opacity-60">
          {{
            t("health.summary", {
              rtt: snap.health.rtt_ms,
              received: snap.health.received,
              lost: snap.health.lost,
            })
          }}
          <span v-if="snap.health.unparsable" class="text-amber-700">
            {{ t("health.unparsable", { count: snap.health.unparsable }) }}
          </span>
        </span>
      </footer>
      <SettingsDialog :open="showPrefs" @close="showPrefs = false" />
    </main>
  </StartupGate>
</template>
