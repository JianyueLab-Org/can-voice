<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import WindowToggles from "./components/WindowToggles.vue";
import SettingsDialog from "./components/SettingsDialog.vue";
import { appearance, loadAppearance } from "./appearance";
import { invoke } from "@tauri-apps/api/core";
import TrafficList from "./components/TrafficList.vue";
import UpdateBanner from "./components/UpdateBanner.vue";
import ChatLog from "./components/ChatLog.vue";
import ControllerList from "./components/ControllerList.vue";
import PilotPanel from "./components/PilotPanel.vue";
import type { View } from "./types";
import { mhz, xpdrText, voiceText } from "./types";

/** 设置对话框开没开。 */
const showPrefs = ref(false);
/** 精简模式：只留值班时要盯的东西。开关在 WindowToggles 里，真相在设置文件里。 */
const compact = computed(() => appearance.value.compact);

const cid = ref("");
const password = ref("");
const callsign = ref("");
const aircraft = ref("");
const realName = ref("");
const error = ref("");
const busy = ref(false);
const view = ref<View | null>(null);
const pressed = ref(false);
const mouseSupported = ref(true);
const recipient = ref("");
const message = ref("");

let timer: number | undefined;

const connected = computed(() => view.value?.link === "Online");
const online = computed(() => view.value?.link != null);

async function refresh() {
  view.value = await invoke<View>("view");
  pressed.value = await invoke<boolean>("ptt_pressed");
}

onMounted(async () => {
  void loadAppearance();
  // 上次用的那一组预填。密码不存：它换的是一张短寿命的票。
  const saved = await invoke<import("./types").Settings>("settings");
  cid.value = saved.cid;
  callsign.value = saved.callsign;
  aircraft.value = saved.aircraft;
  realName.value = saved.real_name;
  mouseSupported.value = await invoke<boolean>("mouse_ptt_supported");
  await refresh();
  // 轮询而不是订阅：事件流是广播，窗口重开之前发生的事收不到。
  timer = window.setInterval(refresh, 250);
});
onUnmounted(() => window.clearInterval(timer));

async function guard(fn: () => Promise<unknown>) {
  error.value = "";
  busy.value = true;
  try {
    await fn();
  } catch (e) {
    error.value = String(e);
  } finally {
    busy.value = false;
    await refresh();
  }
}

const connect = () =>
  guard(async () => {
    await invoke("connect", {
      cid: cid.value,
      password: password.value,
      callsign: callsign.value,
      aircraft: aircraft.value,
      realName: realName.value,
    });
    // 密码用过就丢：它只需要换一张短期票，之后重连带的是票不是密码。
    password.value = "";
  });

const disconnect = () => guard(() => invoke("disconnect"));
const ident = () => guard(() => invoke("ident"));

/** 点席位或者点发件人就把他填进收件人框——管制员叫你的时候，回话要快。 */
function setRecipient(callsign: string) {
  recipient.value = callsign;
}

const send = () =>
  guard(async () => {
    if (!message.value.trim()) return;
    await invoke("send_text", { recipient: recipient.value, message: message.value });
    message.value = "";
  });
</script>

<template>
  <main
    class="mx-auto flex h-screen max-w-5xl flex-col text-sm"
    :class="compact ? 'gap-2 p-2' : 'gap-3 p-4'"
  >
    <header class="flex flex-wrap items-center gap-3">
      <h1 v-if="!compact" class="text-base font-semibold">X-Plane 飞行客户端</h1>
      <!-- 模拟器连没连上要一眼看得见：没连上时下面所有数字都是空的，
           而"空的"和"零"在座舱里是两件事。 -->
      <span
        class="flex items-center gap-1 rounded border px-2 py-0.5 text-xs"
        :class="view?.sim_connected ? 'border-green-500' : 'border-neutral-300 opacity-60'"
      >
        <span
          class="h-2 w-2 rounded-full"
          :class="view?.sim_connected ? 'bg-green-500' : 'bg-neutral-300'"
        />
        X-Plane
      </span>
      <!-- 插件是另一件事。X-Plane 那盏灯只代表 UDP 数据源：没装插件的人
           连得上、说得了话，而天上一架飞机都没有。 -->
      <span
        class="flex items-center gap-1 rounded border px-2 py-0.5 text-xs"
        :class="view?.plugin ? 'border-green-500' : 'border-neutral-300 opacity-60'"
      >
        <span
          class="h-2 w-2 rounded-full"
          :class="view?.plugin ? 'bg-green-500' : 'bg-neutral-300'"
        />
        插件
        <span v-if="view?.plugin" class="opacity-60">{{ view.plugin.drawn }}</span>
      </span>
      <span v-if="online" class="text-xs opacity-70">{{ view?.link }}</span>
      <!-- 语音是另一条链路。不显示的话，被顶号或者声卡打不开时飞行员戴着耳机
           等人回话，而两边都不知道他听不见。 -->
      <span class="text-xs opacity-70">· {{ voiceText(view?.voice) }}</span>
      <span v-if="!mouseSupported && !compact" class="text-xs opacity-60">
        本系统不支持鼠标侧键作 PTT
      </span>
      <!-- 精简时也在：藏掉的话精简之后就切不回来了。 -->
      <WindowToggles class="ml-auto" @settings="showPrefs = true" />
    </header>

    <UpdateBanner v-if="!compact" />

    <p v-if="error" class="rounded border border-red-400 px-3 py-2 text-xs text-red-600">
      {{ error }}
    </p>

    <p
      v-if="view?.plugin && !view.plugin.version_ok"
      class="rounded border border-red-400 px-3 py-2 text-xs text-red-600"
    >
      X-Plane 插件的协议版本是 {{ view.plugin.version }}，和本客户端对不上。
      它会丢掉每一帧，天上不会有任何飞机——到下面「本机 → X-Plane 他机插件」点一下重新安装。
    </p>
    <p
      v-else-if="!view?.plugin"
      class="rounded border border-amber-400 px-3 py-2 text-xs text-amber-700"
    >
      没有听到 X-Plane 插件。语音不受影响，但天上不会画出任何他机——
      到下面「本机 → X-Plane 他机插件」装一下，并确认 XPPython3 已装、X-Plane 正在运行。
    </p>

    <section v-if="!online" class="grid grid-cols-5 gap-2">
      <input v-model="cid" placeholder="CAN 号" class="rounded border px-2 py-1 text-xs" />
      <input
        v-model="password"
        type="password"
        placeholder="密码"
        class="rounded border px-2 py-1 text-xs"
      />
      <input
        v-model="callsign"
        placeholder="呼号 CES123"
        class="rounded border px-2 py-1 font-mono text-xs uppercase"
      />
      <input v-model="aircraft" placeholder="机型 A320" class="rounded border px-2 py-1 text-xs" />
      <input v-model="realName" placeholder="姓名" class="rounded border px-2 py-1 text-xs" />
      <button
        :disabled="busy"
        class="col-span-5 rounded border px-3 py-1 text-xs"
        @click="connect"
      >
        上线
      </button>
    </section>

    <section v-else class="flex items-center gap-2">
      <button class="rounded border px-3 py-1 text-xs" @click="ident">识别（8 秒）</button>
      <button class="rounded border px-3 py-1 text-xs" @click="disconnect">下线</button>
    </section>

    <!-- 座舱读数。频率跟着 COM1 走，界面上没有第二个频率框——
         客户端上再有一个就会有两个真相。 -->
    <!-- 精简时收起：这几个数模拟器里都有，压在模拟器上的窗口不必再显示一遍。 -->
    <section
      v-if="!compact"
      class="grid grid-cols-6 gap-2 rounded border px-3 py-2 font-mono text-xs tabular-nums"
    >
      <div><p class="opacity-60">COM1</p>{{ mhz(view?.sim?.com1) }}</div>
      <div>
        <p class="opacity-60">应答机</p>
        {{ view?.sim ? String(view.sim.squawk).padStart(4, "0") : "—" }}
        <span class="opacity-60">{{ xpdrText(view?.sim?.xpdr_mode) }}</span>
      </div>
      <div><p class="opacity-60">高度</p>{{ view?.sim?.altitude ?? "—" }} ft</div>
      <div><p class="opacity-60">地速</p>{{ view?.sim?.groundspeed ?? "—" }} kt</div>
      <div><p class="opacity-60">航向</p>{{ view?.sim ? Math.round(view.sim.heading) : "—" }}°</div>
      <div>
        <p class="opacity-60">气压修正</p>
        {{ view?.sim?.pressure_delta ?? "—" }} ft
      </div>
    </section>

    <PilotPanel v-if="!compact" :cid="cid" :csl="view?.csl" />

    <!-- 左边是天上的，右边是网上的。文字消息此前整块不存在：管制员打字
         飞行员看不见，而他会以为对方没理他。 -->
    <!-- 精简时只留文字消息：管制员打的字飞行员必须看得见，附近的飞机和在线席位
         是参考，不是值班时要盯的东西。 -->
    <section class="grid min-h-0 flex-1 gap-3" :class="compact ? '' : 'md:grid-cols-2'">
      <div v-if="!compact" class="flex min-h-0 flex-col gap-2">
        <p class="text-xs opacity-60">附近的飞机（{{ view?.traffic.length ?? 0 }}）</p>
        <TrafficList :traffic="view?.traffic ?? []" />
      </div>
      <div class="flex min-h-0 flex-col gap-2">
        <p v-if="!compact" class="text-xs opacity-60">在线席位（{{ view?.controllers.length ?? 0 }}）</p>
        <!-- 点一行就把那个席位填进收件人框。 -->
        <ControllerList
          v-if="!compact"
          class="max-h-28 shrink-0"
          :controllers="view?.controllers ?? []"
          @reply="setRecipient"
        />
        <p v-if="!compact" class="text-xs opacity-60">文字消息</p>
        <ChatLog :messages="view?.messages ?? []" @reply="setRecipient" />
      </div>
    </section>

    <footer class="flex items-center gap-2 border-t" :class="compact ? 'pt-2' : 'pt-3'">
      <button
        class="rounded border px-4 py-2 text-xs"
        :class="pressed ? 'bg-red-600 text-white' : ''"
        @pointerdown="invoke('set_transmitting', { on: true })"
        @pointerup="invoke('set_transmitting', { on: false })"
        @pointerleave="invoke('set_transmitting', { on: false })"
      >
        {{ pressed ? "发话中" : "按住发话" }}
      </button>
      <input
        v-if="!compact"
        v-model="recipient"
        placeholder="收件人（留空发到频率）"
        class="w-40 rounded border px-2 py-1 text-xs"
      />
      <!-- .wallop 在 Rust 侧翻成发往督导，不跟着这个收件人框走。 -->
      <input
        v-model="message"
        placeholder="文字消息，.wallop 呼叫督导"
        class="min-w-0 flex-1 rounded border px-2 py-1 text-xs"
        :disabled="!connected"
        @keyup.enter="send"
      />
      <button class="rounded border px-3 py-1 text-xs" :disabled="!connected" @click="send">
        发送
      </button>
    </footer>
    <SettingsDialog :open="showPrefs" @close="showPrefs = false" />
  </main>
</template>
