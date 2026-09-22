<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import WindowToggles from "./components/WindowToggles.vue";
import Panel from "./components/Panel.vue";
import SettingsDialog from "./components/SettingsDialog.vue";
import { appearance, loadAppearance } from "./appearance";
import { invoke } from "@tauri-apps/api/core";
import TrafficList from "./components/TrafficList.vue";
import UpdateBanner from "./components/UpdateBanner.vue";
import StartupGate from "./components/StartupGate.vue";
import ChatLog from "./components/ChatLog.vue";
import ControllerList from "./components/ControllerList.vue";
import PilotSettings from "./components/PilotSettings.vue";
import FlightPlanDialog from "./components/FlightPlanDialog.vue";
import type { View } from "./types";
import { mhz, khzText, xpdrText, voiceText, linkText, noticeText } from "./types";
import { errorText, language, t } from "./i18n";
import { attachPttKeys } from "./pttKeys";

/** 设置对话框开没开。 */
const showPrefs = ref(false);
/** 飞行计划对话框开没开。只有菜单「文件 → 飞行计划…」开它。 */
const showPlan = ref(false);
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
/**
 * 上一次命令失败的原样错误，`null` = 没有。**存的是错误本身不是那句话**：
 * 在模板里用 `errorText` 翻，切了语言跟着变。
 */
const error = ref<unknown>(null);
const busy = ref(false);
const view = ref<View | null>(null);
const pressed = ref(false);
/** 屏幕按钮按着。灯要立刻亮，不能等那一拍快照。 */
const holding = ref(false);
const talking = computed(() => holding.value || pressed.value);
/** 当前语音频率，kHz。观察员看手输/跟随，飞行员看 COM1。 */
const voiceKhz = computed(() => {
  const v = view.value;
  if (v?.observer?.frequency != null) return v.observer.frequency;
  const sim = v?.sim;
  if (!sim?.com1_power || sim.com1 == null) return null;
  const khz = Math.round(sim.com1 * 1000);
  return khz >= 118000 && khz <= 136975 ? khz : null;
});
const receiving = computed(() => {
  const khz = voiceKhz.value;
  if (khz == null) return false;
  return (view.value?.voice?.receiving?.[String(khz)]?.length ?? 0) > 0;
});
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

function pttDown(e: PointerEvent) {
  holding.value = true;
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  void invoke("set_transmitting", { on: true });
}
function pttUp() {
  holding.value = false;
  void invoke("set_transmitting", { on: false });
}

/**
 * 把菜单上的八个字交给 Rust，整条菜单重建。
 *
 * **字典在这一侧**，Rust 一句文案都不持有（`can_voice_i18n` 的模块注释写明了
 * 为什么：Rust 侧拼好一句中文交出去，切到英文之后那一句还是中文）。所以挂载时
 * 一次、之后每次 `language` 变一次，菜单跟着语言当场换。
 *
 * **参数名是驼峰**：`flightPlan` / `openLog`。tauri 把命令参数按驼峰交给 serde，
 * 写成下划线收不到，而报错长得像"前端漏传了一个字段"。
 */
async function pushMenu() {
  try {
    await invoke("set_menu", {
      labels: {
        file: t("menu.file"),
        flightPlan: t("menu.flight_plan"),
        settings: t("menu.settings"),
        quit: t("menu.quit"),
        help: t("menu.help"),
        openLog: t("menu.open_log"),
        update: t("menu.update"),
        about: t("menu.about"),
      },
    });
  } catch (e) {
    error.value = e;
  }
}

// `{ immediate: true }` 这一下就是"挂载时那一次"：setup 跑完紧接着就挂载，而
// `invoke` 不要求组件已经挂上。不写 immediate 的话，不切语言就永远没有菜单。
watch(language, () => void pushMenu(), { immediate: true });

async function refresh() {
  view.value = await invoke<View>("view");
  pressed.value = await invoke<boolean>("ptt_pressed");
  // 菜单那一项是**取走就没了**：上面这一次 `view` 已经把它从 Rust 侧清掉，
  // 这一拍不处理就没有下一拍。
  switch (view.value.menu) {
    case "flight_plan":
      showPlan.value = true;
      break;
    case "settings":
      showPrefs.value = true;
      break;
    case "update":
    case "about":
      // 任务 9（窗口几何、精简模式、最后两个菜单项、文档）接上。今天故意什么
      // 都不做：`UpdateBanner` 自己持有查到的那一版，从这里再调一次
      // `check_update` 它也看不见，那才是"看起来接上了其实没有"；关于框
      // 今天整个不存在。
      break;
    default:
      break;
  }
}

let detachPtt: (() => void) | undefined;
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
  detachPtt = attachPttKeys();
});
onUnmounted(() => {
  window.clearInterval(timer);
  detachPtt?.();
});

async function guard(fn: () => Promise<unknown>) {
  error.value = null;
  busy.value = true;
  try {
    await fn();
  } catch (e) {
    error.value = e;
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
  error.value = null;
  try {
    await invoke("set_observer", { on });
  } catch (e) {
    observer.value = !on;
    error.value = e;
  }
}

/**
 * 提交手输的频率（回车或者离开输入框）。回填的是 Rust 侧真正存下的那一份，
 * 所以 `121.8` 会变成 `121.800`。读不出来就换回存着的那个：框里留着打错的字，
 * 看起来就像它生效了。
 */
async function applyFrequency() {
  error.value = null;
  try {
    const khz = await invoke<number | null>("set_observer_frequency", {
      text: manualFrequency.value,
    });
    manualFrequency.value = boxText(khz);
  } catch (e) {
    error.value = e;
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
  <StartupGate>
    <main
      class="mx-auto flex h-screen max-w-5xl flex-col text-sm"
      :class="compact ? 'gap-2 p-2' : 'gap-3 p-4'"
    >
      <header class="flex items-center gap-2">
        <!-- 精简时也在：藏掉的话精简之后就切不回来了。 -->
        <WindowToggles class="ml-auto" @settings="showPrefs = true" />
      </header>

      <UpdateBanner v-if="!compact" />

      <p v-if="error !== null" class="rounded border border-red-400 px-3 py-2 text-xs text-red-600">
        {{ errorText(error) }}
      </p>

      <!-- 服务端说的话。不显示的话，"能连上、状态绿、说话没人听见"就是全部症状。
           声卡掉了也走这一条（`audio_unavailable`），而界面上再没有第二处提它：
           拔掉 USB 耳机的人此前一点提示都没有。 -->
      <!-- 精简时也在：说的正是"此刻听不见、也发不出去"，而精简就是在飞。 -->
      <p
        v-for="(n, i) in view?.voice?.notices ?? []"
        :key="`${n[0]}-${n[1]}-${i}`"
        class="rounded border border-amber-400 px-3 py-2 text-xs text-amber-700"
      >
        {{ noticeText(n) }}
      </p>

      <!-- 插件是另一件事。X-Plane 那盏灯只代表 UDP 数据源：没装插件的人
           连得上、说得了话，而天上一架飞机都没有。 -->
      <p class="flex items-center gap-1 text-xs opacity-70">
        <span
          class="h-2 w-2 shrink-0 rounded-full"
          :style="{ background: view?.plugin ? 'var(--can-on)' : 'var(--can-idle)' }"
        />
        {{ t("status.plugin") }}
        <span v-if="view?.plugin" class="opacity-60">{{ view.plugin.drawn }}</span>
      </p>

      <!-- 观察员不上 FSD，天上本来就不会有他机：插件装没装和他无关，别拿这两条吓他。 -->
      <p
        v-if="!observer && view?.plugin && !view.plugin.version_ok"
        class="rounded border border-red-400 px-3 py-2 text-xs text-red-600"
      >
        {{ t("plugin.version_mismatch", { version: view.plugin.version }) }}
      </p>
      <p
        v-else-if="!observer && !view?.plugin"
        class="rounded border border-amber-400 px-3 py-2 text-xs text-amber-700"
      >
        {{ t("plugin.not_heard") }}
      </p>

      <p v-if="!mouseSupported && !compact" class="rounded border px-3 py-2 text-xs opacity-60">
        {{ t("status.no_mouse_ptt") }}
      </p>

      <Panel :title="t('connect.title')">
        <!-- can-audio 的网格顺序（xpc/gui.py:251-263）：呼号 · CID · 密码 · 机型 · 连接。
             follow 和姓名是 can-voice 多出来的两格，留在同一行。 -->
        <div class="grid grid-cols-7 gap-2">
          <!-- 观察员填的是机长的呼号：语音服务端按那架飞机的位置给他算距离。 -->
          <input
            v-if="observer"
            v-model="follow"
            :disabled="online"
            :placeholder="t('login.follow')"
            :title="t('login.follow_tip')"
            class="rounded border px-2 py-1 font-mono text-xs uppercase"
          />
          <input
            v-else
            v-model="callsign"
            :disabled="online"
            :placeholder="t('login.callsign')"
            class="rounded border px-2 py-1 font-mono text-xs uppercase"
          />
          <input
            v-model="cid"
            :disabled="online"
            :placeholder="t('login.cid')"
            class="rounded border px-2 py-1 text-xs"
          />
          <input
            v-model="password"
            type="password"
            :disabled="online"
            :placeholder="t('login.password')"
            class="rounded border px-2 py-1 text-xs"
          />
          <input
            v-model="aircraft"
            :disabled="observer || online"
            :placeholder="t('login.aircraft')"
            class="rounded border px-2 py-1 text-xs"
          />
          <input
            v-model="realName"
            :disabled="observer || online"
            :placeholder="t('login.real_name')"
            class="rounded border px-2 py-1 text-xs"
          />
          <div class="col-span-2 flex flex-wrap items-center gap-2">
            <button
              v-if="!online"
              :disabled="busy"
              class="min-w-[110px] rounded border px-3 py-1 text-xs"
              @click="connect"
            >
              {{ t("login.connect") }}
            </button>
            <template v-else>
              <button
                :disabled="busy"
                class="min-w-[110px] rounded border px-3 py-1 text-xs"
                @click="disconnect"
              >
                {{ t("session.disconnect") }}
              </button>
              <span v-if="observing" class="text-xs opacity-70">
                {{ t("session.following") }}
                <span class="font-mono">{{ view?.observer?.follow }}</span>
              </span>
            </template>
          </div>
        </div>

        <!-- can-audio 把三格状态文案排在网格第二行（xpc/gui.py:265-270）：模拟器、FSD、语音。
             页头那三个 pill 就是搬到这里来的。三格自带名字：FSD 和语音那两句译文本来就以
             它们的名字开头，第一格里 `X-Plane` 是拉丁字面量。
             **插件那盏灯不在这三格里**：它跟着版本不符和没听到那两条横幅留在卡片上方，
             因为这一格说的是 UDP 数据源通不通，而插件说的是天上画不画得出他机——
             没装插件的人这一格是绿的。别把两盏灯并回一格。 -->
        <div class="grid grid-cols-3 gap-2 text-xs">
          <span class="flex items-center gap-1 opacity-70">
            <!-- 模拟器连没连上要一眼看得见：没连上时下面所有数字都是空的，
                 而"空的"和"零"在座舱里是两件事。 -->
            <span
              class="h-2 w-2 shrink-0 rounded-full"
              :style="{ background: view?.sim_connected ? 'var(--can-on)' : 'var(--can-idle)' }"
            />
            X-Plane
          </span>
          <span class="truncate opacity-70">
            {{ observing ? t("status.observing") : linkText(view?.link) }}
          </span>
          <!-- 语音是另一条链路。不显示的话，被顶号或者声卡打不开时飞行员戴着耳机
               等人回话，而两边都不知道他听不见。 -->
          <span class="truncate opacity-70">{{ voiceText(view?.voice) }}</span>
        </div>

        <!-- 观察员开关排在状态文案下面，和 can-audio 一样（xpc/gui.py:277-287）：
             它决定这一次连接会不会在网络上多出一架飞机，是每次点「连接」之前该看一眼的事。 -->
        <div class="flex flex-wrap items-start gap-2 text-xs">
          <!-- 右座用。两个人要用各自的账号：同一个成员号第二次登录会把第一条顶掉。 -->
          <label class="flex shrink-0 items-center gap-2">
            <input v-model="observer" type="checkbox" :disabled="online" @change="toggleObserver" />
            {{ t("login.observer_mode") }}
          </label>
          <p class="min-w-0 flex-1 opacity-60">{{ t("login.observer_note") }}</p>
        </div>
      </Panel>

      <!-- 观察员的频率。正常上网络时频率只跟 COM1 走，界面上没有第二个框；观察员是
           例外——右座的人未必开着模拟器，开着的那台也未必调在机长那个频率上。
           精简时也在：这是他唯一的调频手段。 -->
      <section
        v-if="observer"
        class="flex flex-wrap items-center gap-2 rounded border px-3 py-2 text-xs"
      >
        <span class="opacity-60">{{ t("observer.frequency") }}</span>
        <input
          v-model="manualFrequency"
          :placeholder="t('observer.frequency_placeholder')"
          :title="t('observer.frequency_tip')"
          class="w-28 rounded border px-2 py-1 font-mono"
          @change="applyFrequency"
        />
        <template v-if="view?.observer">
          <span v-if="view.observer.frequency !== null" class="font-mono">
            {{ khzText(view.observer.frequency) }}
            <span class="opacity-60">{{
              view.observer.manual ? t("observer.manual") : t("observer.follow_com1")
            }}</span>
          </span>
          <span v-else class="text-amber-700">
            {{ t("observer.no_frequency") }}
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
          <p class="opacity-60">{{ t("cockpit.transponder") }}</p>
          {{ view?.sim ? String(view.sim.squawk).padStart(4, "0") : "—" }}
          <span class="opacity-60">{{ xpdrText(view?.sim?.xpdr_mode) }}</span>
        </div>
        <div><p class="opacity-60">{{ t("cockpit.altitude") }}</p>{{ view?.sim?.altitude ?? "—" }} ft</div>
        <div><p class="opacity-60">{{ t("cockpit.groundspeed") }}</p>{{ view?.sim?.groundspeed ?? "—" }} kt</div>
        <div><p class="opacity-60">{{ t("cockpit.heading") }}</p>{{ view?.sim ? Math.round(view.sim.heading) : "—" }}°</div>
        <div>
          <p class="opacity-60">{{ t("cockpit.pressure_delta") }}</p>
          {{ view?.sim?.pressure_delta ?? "—" }} ft
        </div>
      </section>

      <!-- 左边是天上的，右边是网上的。文字消息此前整块不存在：管制员打字
           飞行员看不见，而他会以为对方没理他。 -->
      <!-- 精简时只留文字消息：管制员打的字飞行员必须看得见，附近的飞机和在线席位
           是参考，不是值班时要盯的东西。 -->
      <section class="flex min-h-0 flex-1 gap-3">
        <Panel :title="t('messages.title')" class="min-h-0 flex-[3]">
          <ChatLog class="min-h-0 flex-1" :messages="view?.messages ?? []" @reply="setRecipient" />
          <!-- can-audio 的发送行（xpc/gui.py:307-320）：收件人最大 240、正文撑开、发送。 -->
          <div class="flex items-center gap-2">
            <input
              v-if="!compact"
              v-model="recipient"
              :placeholder="t('chat.recipient')"
              class="min-w-0 max-w-[240px] flex-1 rounded border px-2 py-1 text-xs"
            />
            <!-- .wallop 在 Rust 侧翻成发往督导，不跟着这个收件人框走。 -->
            <!-- 文字消息走 FSD，观察员没有那条连接，管制员的字只到机长那边。 -->
            <input
              v-model="message"
              :placeholder="observing ? t('chat.observer_no_text') : t('chat.message')"
              class="min-w-0 flex-[2] rounded border px-2 py-1 text-xs"
              :disabled="!connected"
              @keyup.enter="send"
            />
            <button class="rounded border px-3 py-1 text-xs" :disabled="!connected" @click="send">
              {{ t("chat.send") }}
            </button>
          </div>
        </Panel>

        <Panel
          v-if="!compact"
          :title="t('lists.controllers', { count: view?.controllers.length ?? 0 })"
          class="min-h-0 flex-1"
        >
          <ControllerList
            class="min-h-0 flex-1"
            :controllers="view?.controllers ?? []"
            @reply="setRecipient"
          />
          <!-- can-audio 在列表下面放一条换行提示（xpc/gui.py:329-332）：
               单击就填收件人这件事，界面上不说没人会去试。 -->
          <p class="text-xs opacity-60">{{ t("controllers.hint") }}</p>
        </Panel>

        <Panel
          v-if="!compact"
          :title="t('lists.traffic', { count: view?.traffic.length ?? 0 })"
          class="min-h-0 flex-1"
        >
          <TrafficList class="min-h-0 flex-1" :traffic="view?.traffic ?? []" />
        </Panel>
      </section>

      <footer class="flex items-center gap-2 border-t" :class="compact ? 'pt-2' : 'pt-3'">
        <span
          class="rounded border px-2 py-1 font-mono text-xs"
          :class="talking ? 'bg-red-600 text-white' : 'opacity-50'"
        >TX</span>
        <span
          class="rounded border px-2 py-1 font-mono text-xs"
          :class="receiving ? 'bg-green-500 text-white' : 'opacity-50'"
        >RX</span>
        <button
          class="rounded border px-4 py-2 text-xs"
          :class="talking ? 'bg-red-600 text-white' : ''"
          :title="t('chat.push_to_talk_tip')"
          @pointerdown="pttDown"
          @pointerup="pttUp"
          @pointercancel="pttUp"
        >
          {{ t("chat.push_to_talk") }}
        </button>
      </footer>
      <FlightPlanDialog :open="showPlan" :observer="observer" @close="showPlan = false" />
      <SettingsDialog :open="showPrefs" @close="showPrefs = false">
        <PilotSettings :cid="cid" :csl="view?.csl" />
      </SettingsDialog>
    </main>
  </StartupGate>
</template>
