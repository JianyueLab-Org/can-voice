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
import { mhz, khzText, xpdrText, voiceText } from "./types";

/** 设置对话框开没开。 */
const showPrefs = ref(false);
/** 精简模式：只留值班时要盯的东西。开关在 WindowToggles 里，真相在设置文件里。 */
const compact = computed(() => appearance.value.compact);

const cid = ref("");
const password = ref("");
const callsign = ref("");
const aircraft = ref("");
const realName = ref("");
/** 观察员模式（双人机组的右座）。真相在设置文件里，只有下线时能改。 */
const observer = ref(false);
/** 跟随的呼号：机长那架飞机的。 */
const follow = ref("");
/** 手输频率框里的字。空的 = 跟随 COM1。 */
const manualFrequency = ref("");
const error = ref("");
const busy = ref(false);
const view = ref<View | null>(null);
const pressed = ref(false);
const mouseSupported = ref(true);
const recipient = ref("");
const message = ref("");

let timer: number | undefined;

/** 以观察员身份连着。观察员没有 FSD 链路，`link` 恒为 null——只看它的话连上之后登录表单不消失。 */
const observing = computed(() => view.value?.observer != null);
const connected = computed(() => view.value?.link === "Online");
const online = computed(() => view.value?.link != null || observing.value);

/** 频率框里显示的样子：存的是 kHz，框里写 `121.800`，没有就空着。 */
const boxText = (khz: number | null) => (khz === null ? "" : khzText(khz));

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
  observer.value = saved.observer;
  follow.value = saved.follow;
  manualFrequency.value = boxText(saved.observer_frequency);
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
      follow: follow.value,
    });
    // 密码用过就丢：它只需要换一张短期票，之后重连带的是票不是密码。
    password.value = "";
  });

const disconnect = () => guard(() => invoke("disconnect"));

/** 切换观察员模式。Rust 侧在连着的时候会拒绝——拒了就把勾选框扳回去。 */
async function toggleObserver() {
  const on = observer.value;
  error.value = "";
  try {
    await invoke("set_observer", { on });
  } catch (e) {
    observer.value = !on;
    error.value = String(e);
  }
}

/**
 * 提交手输的频率（回车或者离开输入框）。回填的是 Rust 侧真正存下的那一份，
 * 所以 `121.8` 会变成 `121.800`。读不出来就换回存着的那个：框里留着打错的字，
 * 看起来就像它生效了。
 */
async function applyFrequency() {
  error.value = "";
  try {
    const khz = await invoke<number | null>("set_observer_frequency", {
      text: manualFrequency.value,
    });
    manualFrequency.value = boxText(khz);
  } catch (e) {
    error.value = String(e);
    const saved = await invoke<import("./types").Settings>("settings");
    manualFrequency.value = boxText(saved.observer_frequency);
  }
}
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
      <h1 v-if="!compact" class="text-base font-semibold">MSFS 飞行客户端</h1>
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
        MSFS
      </span>
      <span v-if="observing" class="text-xs opacity-70">观察员（不上网络）</span>
      <span v-else-if="online" class="text-xs opacity-70">{{ view?.link }}</span>
      <!-- 语音是另一条链路。不显示的话，被顶号或者声卡打不开时飞行员戴着耳机
           等人回话，而两边都不知道他听不见。 -->
      <span class="text-xs opacity-70">· {{ voiceText(view?.voice) }}</span>
      <!-- 连不上要说得出原因。非 Windows 上就是"这个系统没有 SimConnect"——
           让人对着一个永远灰着的灯猜，是这个项目反复要躲开的那类故障。 -->
      <span v-if="!view?.sim_connected && view?.sim_problem" class="text-xs text-amber-600">
        {{ view.sim_problem }}
      </span>
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

    <section v-if="!online" class="grid grid-cols-5 gap-2">
      <!-- 右座用。两个人要用各自的账号：同一个成员号第二次登录会把第一条顶掉。 -->
      <label class="col-span-5 flex flex-wrap items-center gap-2 text-xs">
        <input v-model="observer" type="checkbox" @change="toggleObserver" />
        观察员模式（双人机组）
        <span class="opacity-60">右座用：只连语音，不在网络上产生第二架飞机。两个人要用各自的账号。</span>
      </label>
      <input v-model="cid" placeholder="CAN 号" class="rounded border px-2 py-1 text-xs" />
      <input
        v-model="password"
        type="password"
        placeholder="密码"
        class="rounded border px-2 py-1 text-xs"
      />
      <!-- 观察员填的是机长的呼号：语音服务端按那架飞机的位置给他算距离。 -->
      <input
        v-if="observer"
        v-model="follow"
        placeholder="跟随的呼号（机长的）"
        title="机长那架飞机的呼号。语音服务端按它的位置给你算距离。"
        class="rounded border px-2 py-1 font-mono text-xs uppercase"
      />
      <input
        v-else
        v-model="callsign"
        placeholder="呼号 CES123"
        class="rounded border px-2 py-1 font-mono text-xs uppercase"
      />
      <input
        v-model="aircraft"
        :disabled="observer"
        placeholder="机型 A320"
        class="rounded border px-2 py-1 text-xs"
      />
      <input
        v-model="realName"
        :disabled="observer"
        placeholder="姓名"
        class="rounded border px-2 py-1 text-xs"
      />
      <button
        :disabled="busy"
        class="col-span-5 rounded border px-3 py-1 text-xs"
        @click="connect"
      >
        上线
      </button>
    </section>

    <section v-else class="flex items-center gap-2">
      <!-- 识别是 FSD 的事，观察员没有那条连接。 -->
      <button v-if="!observing" class="rounded border px-3 py-1 text-xs" @click="ident">
        识别（8 秒）
      </button>
      <span v-else class="text-xs opacity-70">
        跟随 <span class="font-mono">{{ view?.observer?.follow }}</span>
      </span>
      <button class="rounded border px-3 py-1 text-xs" @click="disconnect">下线</button>
    </section>

    <!-- 观察员的频率。正常上网络时频率只跟 COM1 走，界面上没有第二个框；观察员是
         例外——右座的人未必开着模拟器，开着的那台也未必调在机长那个频率上。
         精简时也在：这是他唯一的调频手段。 -->
    <section
      v-if="observer"
      class="flex flex-wrap items-center gap-2 rounded border px-3 py-2 text-xs"
    >
      <span class="opacity-60">语音频率</span>
      <input
        v-model="manualFrequency"
        placeholder="留空跟随 COM1"
        title="观察员模式专用：手输一个频率，语音就待在那里。清空则回到跟随座舱 COM1。"
        class="w-28 rounded border px-2 py-1 font-mono"
        @change="applyFrequency"
      />
      <template v-if="view?.observer">
        <span v-if="view.observer.frequency !== null" class="font-mono">
          {{ khzText(view.observer.frequency) }}
          <span class="opacity-60">{{ view.observer.manual ? "手输" : "跟随 COM1" }}</span>
        </span>
        <span v-else class="text-amber-700">
          还没有频率：在这里输入一个，或者开着模拟器跟 COM1 走
        </span>
      </template>
    </section>

    <!-- 座舱读数。频率跟着 COM1 走，界面上没有第二个频率框——
         客户端上再有一个就会有两个真相。（观察员例外，见上面那一栏。） -->
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

    <PilotPanel v-if="!compact" :cid="cid" :hangar="view?.hangar" :observer="observer" />

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
      <!-- 文字消息走 FSD，观察员没有那条连接，管制员的字只到机长那边。 -->
      <input
        v-model="message"
        :placeholder="observing ? '观察员不收发文字消息' : '文字消息，.wallop 呼叫督导'"
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
