<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import RadioRow from "./components/RadioRow.vue";

type LinkState = "Connecting" | "Online" | "Reconnecting" | "Offline" | "Evicted";
type Ended = "Offline" | "Evicted" | { Refused: string | { Other: string } };

interface Radio {
  freq_khz: number;
  rx: boolean;
  tx: boolean;
  xc: boolean;
  gain: number;
  selected: boolean;
}

interface Snapshot {
  link: LinkState;
  ended: Ended | null;
  receiving: Record<string, number[]>;
  denied_tx: number[];
  denied_rx: number[];
  denied_xc: number[][];
  health: { rtt_ms: number; sent: number; received: number; lost: number } | null;
}

const cid = ref("");
const password = ref("");
const busy = ref(false);
const error = ref("");
const freqInput = ref("");
const radios = ref<Radio[]>([]);
const snap = ref<Snapshot | null>(null);
const pressed = ref(false);
const mouseSupported = ref(true);

let timer: number | undefined;

// 界面**读快照**，不从事件流拼：事件是广播，窗口重开之前发生的事收不到。
async function refresh() {
  snap.value = await invoke<Snapshot>("snapshot");
  radios.value = await invoke<Radio[]>("radios");
  pressed.value = await invoke<boolean>("ptt_pressed");
}

onMounted(async () => {
  mouseSupported.value = await invoke<boolean>("mouse_ptt_supported");
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
  await invoke("add_frequency", { freqKhz: khz });
  await refresh();
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

    <p v-if="error" class="rounded border border-red-400 px-3 py-2 text-xs text-red-600">
      {{ error }}
    </p>

    <section class="flex items-center gap-2">
      <input
        v-model="freqInput"
        placeholder="121.800"
        class="w-28 rounded border px-2 py-1"
        @keyup.enter="addFrequency"
      />
      <button class="rounded border px-3 py-1" @click="addFrequency">添加频率</button>
      <span v-if="!mouseSupported" class="ml-auto text-xs opacity-60">
        本系统不支持鼠标侧键作 PTT，请用键盘或手柄
      </span>
    </section>

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
        @select="act('set_selected', { freqKhz: r.freq_khz })"
        @remove="act('remove_frequency', { freqKhz: r.freq_khz })"
      />
      <p v-if="!radios.length" class="py-6 text-center text-xs opacity-50">
        还没有频率。在上面填一个，例如 121.800
      </p>
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
        RTT {{ snap.health.rtt_ms }} ms · 收 {{ snap.health.received }} · 丢 {{ snap.health.lost }}
      </span>
    </footer>
  </main>
</template>
