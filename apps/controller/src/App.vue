<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import RadioRow from "./components/RadioRow.vue";
import UpdateBanner from "./components/UpdateBanner.vue";
import SettingsPanel from "./components/SettingsPanel.vue";
import OnlineList from "./components/OnlineList.vue";

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
  last_talk: Record<string, { speaker: number; at: number }>;
}

const cid = ref("");
const password = ref("");
const busy = ref(false);
const error = ref("");
const freqInput = ref("");
const radios = ref<Radio[]>([]);
const snap = ref<Snapshot | null>(null);
const feed = ref<FeedView | null>(null);
const pressed = ref(false);
const showSettings = ref(false);

let timer: number | undefined;

// 界面**读快照**，不从事件流拼：事件是广播，窗口重开之前发生的事收不到。
async function refresh() {
  snap.value = await invoke<Snapshot>("snapshot");
  radios.value = await invoke<Radio[]>("radios");
  pressed.value = await invoke<boolean>("ptt_pressed");
  feed.value = await invoke<FeedView>("feed");
}

onMounted(async () => {
  // 上次用的 CAN 号预填。密码不存——它只换一张 60 秒的票。
  cid.value = (await invoke<{ cid: string }>("settings")).cid;
  await refresh();
  timer = window.setInterval(refresh, 200);
});
onUnmounted(() => window.clearInterval(timer));

const connected = computed(() => snap.value?.link === "Online");

/** 三种终态要说三句不同的话。 */
const statusText = computed(() => {
  const s = snap.value;
  if (!s) return "…";
  switch (s.link) {
    case "Online":
      return "已连接";
    case "Connecting":
      return "连接中…";
    // **`Reconnecting` 与 `Offline` 是对立的**：前者链路还活着，别把台面清掉。
    case "Reconnecting":
      return "重连中…";
    case "Evicted":
      return "这个账号在别处登录了，语音已断开";
    case "Offline":
      return endedText(s.ended);
  }
});

function endedText(ended: Ended | null): string {
  if (!ended || ended === "Offline") return "已断开";
  if (ended === "Evicted") return "这个账号在别处登录了";
  const r = ended.Refused;
  // `proto_unsupported` 的动作和 `refused` 一样，但**要说的话不一样**：
  // 告诉一个版本太旧的人"被拒绝"，他会去查密码、去怀疑账号，而原因是那个旧 exe。
  if (r === "ProtoUnsupported") return "客户端版本太旧，请更新后再连";
  if (r === "TokenExpired") return "登录凭据已过期，请重新连接";
  if (r === "TokenInvalid" || r === "Refused") return "服务端拒绝了这次连接";
  return "连接被拒绝";
}

async function connect() {
  error.value = "";
  busy.value = true;
  try {
    await invoke("connect", { cid: cid.value, password: password.value });
    // 密码用过就丢：它只需要换一张 60 秒的票，之后重连带的是票不是密码。
    password.value = "";
    await refresh();
  } catch (e) {
    error.value = String(e);
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
  const t = raw.trim();
  if (!t) return null;
  const n = Number(t);
  if (!Number.isFinite(n)) return null;
  const khz = t.includes(".") ? Math.round(n * 1000) : Math.round(n);
  // 夹在 VHF 波段里：服务端把 freq_khz 当不透明路由键，不做范围校验，
  // 所以打错的那个数会变成一个谁也不在的频率，而一切看起来正常。
  return khz >= 118000 && khz <= 136975 ? khz : null;
}

async function addFrequency() {
  const khz = parseFreq(freqInput.value);
  if (khz === null) {
    error.value = "频率要在 118.000 – 136.975 之间";
    return;
  }
  error.value = "";
  freqInput.value = "";
  await invoke("add_frequency", { freqKhz: khz, callsign: null });
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

/** 这一行此刻正在发射：TX 开着，而且 PTT 按着。 */
function isTransmitting(r: Radio): boolean {
  return pressed.value && r.tx;
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

/**
 * 服务端通知的人话。
 *
 * **不认识的 kind 也要显示出来**：一条服务端认为值得说、而客户端太旧不认识的
 * 通知，落到界面上是一句原文，总好过一片安静。
 */
function noticeText([kind, freq, reason]: [string, number, string]): string {
  const where = freq ? ` · ${(freq / 1000).toFixed(3)}` : "";
  switch (kind) {
    case "audio_unavailable":
      return "声卡不见了（拔了设备？），正在重开：现在听不见也发不出";
    case "range_unavailable":
      return "位置服务不可用，射程过滤已关闭：现在这条频率是全网可听";
    case "unknown_message":
      return `服务端不认识客户端发的一帧（${reason}），可能需要更新客户端`;
    default:
      return `${kind}${where}：${reason}`;
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
  <main class="mx-auto flex h-screen max-w-3xl flex-col gap-4 p-5 text-sm">
    <header class="flex items-center justify-between gap-3">
      <div>
        <h1 class="text-base font-semibold">管制语音</h1>
        <p class="text-xs opacity-70">{{ statusText }}</p>
      </div>
      <div v-if="!connected" class="flex items-center gap-2">
        <input v-model="cid" placeholder="CAN 号" class="w-24 rounded border px-2 py-1" />
        <input
          v-model="password"
          type="password"
          placeholder="密码"
          class="w-32 rounded border px-2 py-1"
          @keyup.enter="connect"
        />
        <button :disabled="busy" class="rounded border px-3 py-1" @click="connect">连接</button>
      </div>
      <button v-else class="rounded border px-3 py-1" @click="disconnect">断开</button>
    </header>

    <UpdateBanner />

    <p v-if="error" class="rounded border border-red-400 px-3 py-2 text-xs text-red-600">
      {{ error }}
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
      这些交叉耦合没有生效：{{ deniedPairs.join("、") }}
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
        你此刻不在任何席位上（数据源里没有你），发射已关闭，但还听得见。
        在 EuroScope 上线之后，这里会在一分钟内跟上。
        <span v-if="feed?.duty.dropped_tx">台面上原来开着的发射已经全部关掉。</span>
      </template>
      <template v-else>还没查到数据源，席位状态未知——下一轮（一分钟内）会再试。</template>
    </p>
    <p v-else-if="connected" class="text-xs text-sky-700">
      在席位 {{ feed?.duty.callsign }}
      <span v-if="feed?.duty.freq_khz" class="font-mono opacity-70">
        · {{ (feed.duty.freq_khz / 1000).toFixed(3) }}
      </span>
    </p>

    <section class="flex items-center gap-2">
      <input
        v-model="freqInput"
        placeholder="121.800"
        class="w-28 rounded border px-2 py-1"
        @keyup.enter="addFrequency"
      />
      <button class="rounded border px-3 py-1" @click="addFrequency">添加频率</button>
      <button class="ml-auto rounded border px-3 py-1" @click="showSettings = !showSettings">
        {{ showSettings ? "收起设置" : "设置" }}
      </button>
    </section>

    <SettingsPanel v-if="showSettings" :cid="cid" />

    <section class="flex flex-1 flex-col gap-2 overflow-auto">
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
        :transmit-allowed="mayTransmit"
        @remove="act('remove_frequency', { freqKhz: r.freq_khz })"
      />
      <p v-if="!radios.length" class="py-6 text-center text-xs opacity-50">
        还没有频率。在上面填一个，例如 121.800
      </p>

      <!-- 在线一览。没有它的话，加一个别人的频率要先去别的地方查他在守什么。 -->
      <div v-if="connected" class="mt-2 flex flex-col gap-1 border-t pt-2">
        <p class="text-xs opacity-60">在线席位</p>
        <OnlineList :online="feed?.online ?? []" :tuned="tuned" @add="addOnline" />
      </div>
    </section>

    <footer class="flex items-center justify-between gap-3 border-t pt-3 text-xs">
      <button
        class="rounded border px-4 py-2"
        :class="pressed ? 'bg-red-600 text-white' : ''"
        @pointerdown="invoke('set_transmitting', { on: true })"
        @pointerup="invoke('set_transmitting', { on: false })"
        @pointerleave="invoke('set_transmitting', { on: false })"
      >
        {{ pressed ? "发话中" : "按住发话" }}
      </button>
      <span v-if="snap?.health" class="opacity-60">
        RTT {{ snap.health.rtt_ms }} ms · 收 {{ snap.health.received }} · 丢
        {{ snap.health.lost }}
        <span v-if="snap.health.unparsable" class="text-amber-700">
          · 解不开 {{ snap.health.unparsable }}
        </span>
      </span>
    </footer>
  </main>
</template>
