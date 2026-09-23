<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import WindowToggles from "./components/WindowToggles.vue";
import Panel from "./components/Panel.vue";
import StateToggle from "./components/StateToggle.vue";
import StatusBar from "./components/StatusBar.vue";
import SettingsDialog from "./components/SettingsDialog.vue";
import Modal from "./components/Modal.vue";
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
/** 关于对话框开没开。只有菜单「帮助 → 关于」开它。 */
const showAbout = ref(false);
const version = ref("");
const logPath = ref("");

/**
 * 「帮助 → 关于」。版本号和日志路径**打开时才读**，不在 `onMounted` 里读：
 * `onMounted` 里一条 reject 的 invoke 会把它后面的 `setInterval` 和 PTT 绑定
 * 一起带走，界面连上之后画一帧就不动了。
 */
async function openAbout() {
  try {
    version.value = await invoke<string>("app_version");
  } catch {
    version.value = "";
  }
  try {
    logPath.value = (await invoke<string | null>("log_file")) ?? "";
  } catch {
    logPath.value = "";
  }
  showAbout.value = true;
}

/** 精简模式：只留值班时要盯的东西。开关在 WindowToggles 里，真相在设置文件里。 */
const compact = computed(() => appearance.value.compact);

const cid = ref("");
const password = ref("");
const callsign = ref("");
const aircraft = ref("");
const realName = ref("");
/** 观察员模式（双人机组的右座）。真相在设置文件里，只有下线时能改。 */
const observer = ref(false);
/** Observer's own FSD callsign. */
const observerCallsign = ref("");
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

/**
 * TX / RX 两块色块的三态。**和管制端同一套语义**（`RadioRow.vue:54-60`）：
 * 有频率就是 `on`，此刻真的在收 / 发是 `active`，一个频率都没有是 `off`。
 *
 * **红色（`muted`）不用。** 管制端那边红色只表示静音；同一个组件在两个端表示两件事，
 * 看代码的人就再也分不清红是什么意思。can-audio 的 `Indicator("TX", MUTED_COLOR)`
 * 和今天这里的 `bg-red-600` 都让出来，换四个端一套颜色。
 */
const txState = computed<"off" | "on" | "active">(() =>
  talking.value ? "active" : voiceKhz.value !== null ? "on" : "off",
);
const rxState = computed<"off" | "on" | "active">(() =>
  receiving.value ? "active" : voiceKhz.value !== null ? "on" : "off",
);

/**
 * 底栏中间那句话此刻在说什么。**存的是状态不是那句话**：存一句翻好的话，
 * 切了语言之后已经显示着的那一句不会跟着变（和 `error` 同一条规矩，见 `:34-38`）。
 *
 * 两种取值都由「帮助 → 检查更新」置上：`"update"` 是正在查，`"no_update"` 是查完了
 * 什么都没有——后一种自己会在几秒后收回去。
 */
const transient = ref<"update" | "no_update" | null>(null);

/** 底栏那句话。can-audio 空闲时说「就绪」，有事说那件事（`xpc/gui.py:200`）。 */
const barStatus = computed(() => {
  if (transient.value === "update") return t("update.checking");
  if (transient.value === "no_update") return t("update.current");
  if (talking.value) return t("chat.transmitting");
  // 机库扫一遍要几十秒，而注入要等它扫完（`wait_for_hangar`，最多 120 秒）。
  // 包目录填错的人在此之前只看得见「天上是空的」，而那和没连上模拟器、和机型码
  // 对不上长得一模一样。这条栏在精简模式下也在，所以这句话总有地方说；
  // 那一格是 `truncate` 的，所以这里只能放短句——长的两句在上面当横幅排。
  if (view.value?.hangar.loading) return t("hangar.loading");
  return t("status.ready");
});

/**
 * 「帮助 → 检查更新」。`UpdateBanner` 自己在挂载时查一次，所以这里换掉它的 `key`
 * 让它重挂一次；自己这一趟 `check_update` 用来知道「查完了」，以及查到了没有。
 *
 * **查不到也要回一句。** 点了跟没点一样是最糟的形态。有新版时的反馈是卡片上方
 * 那条横幅，没有就在底栏说「没有可用的更新。」，停 4 秒回到「就绪」。
 *
 * **正连着的时候一定查不到**，这是 Rust 侧刻意的（一个更新框盖在台面上比晚一次
 * 更新糟得多），所以那句话说的是「没有可用的更新」而不是「已经是最新版本」。
 */
const updateNonce = ref(0);
let updateClear: number | undefined;
async function checkUpdate() {
  window.clearTimeout(updateClear);
  transient.value = "update";
  updateNonce.value += 1;
  let found = false;
  try {
    found = (await invoke<unknown>("check_update")) !== null;
  } catch {
    // 查不动就当没有更新——Rust 侧每条错误路径本来也返回「没有更新」。
  }
  transient.value = found ? null : "no_update";
  // 停一会儿就收回去：底栏那句话讲的是此刻，不是一条留着的记录。
  if (!found) updateClear = window.setTimeout(() => (transient.value = null), 4000);
}

/**
 * 无线电那一行右边那串数字。can-audio 的 `position_label`（`xpc/gui.py:532-535`）
 * 就是这一格——它装的是**本机的位置和姿态**，不是管制席位。
 *
 * 不画标签：五个标签摊在这一行上会把它挤散，而 can-audio 也没有标签。标签进
 * `title`（下面那个函数），悬停时仍说得出这串数字各是什么。
 */
function cockpitText(): string {
  const s = view.value?.sim;
  if (!s) return "—";
  const squawk = String(s.squawk).padStart(4, "0");
  const heading = String(Math.round(s.heading)).padStart(3, "0");
  return `A${squawk} ${xpdrText(s.xpdr_mode)}  ${s.altitude} ft  ${s.groundspeed} kt  ${heading}°  ${s.pressure_delta} ft`;
}

/** 上面那串数字的读法，按顺序列出五个标签。分隔符走字典：中文是「、」，英文是「, 」。 */
function cockpitTitle(): string {
  return [
    t("cockpit.transponder"),
    t("cockpit.altitude"),
    t("cockpit.groundspeed"),
    t("cockpit.heading"),
    t("cockpit.pressure_delta"),
  ].join(t("common.separator.list"));
}

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

/**
 * 屏幕上那颗 PTT。**两头都写成幂等的**：键盘 / 鼠标侧键那一路的按下状态在 Rust 侧
 * （`ptt_pressed`），而这里每按一下都发一趟命令。多余的一次 `set_transmitting(false)`
 * 会在人还按着键的时候把话切断——那时候他正在说话，而界面上一点异样都没有。
 */
function pttDown(e: PointerEvent) {
  // 指针捕获：按住说话时手是会动的，滑出按钮之后 `pointerup` 就落到别的元素上，
  // 松手事件永远不来，麦克风一直开着。
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  if (holding.value) return;
  holding.value = true;
  void invoke("set_transmitting", { on: true });
}
function pttUp() {
  if (!holding.value) return;
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
      void checkUpdate();
      break;
    case "about":
      void openAbout();
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
  observerCallsign.value = saved.observer_callsign;
  manualFrequency.value = boxText(saved.observer_frequency);
  mouseSupported.value = await invoke<boolean>("mouse_ptt_supported");
  await refresh();
  // 轮询而不是订阅：事件流是广播，窗口重开之前发生的事收不到。
  timer = window.setInterval(refresh, 250);
  detachPtt = attachPttKeys();
});
onUnmounted(() => {
  window.clearInterval(timer);
  // 不清的话它会朝着一个已经拆掉的组件写值。
  window.clearTimeout(updateClear);
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
      observerCallsign: observerCallsign.value,
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

      <!-- `key` 是「帮助 → 检查更新」那条路：横幅自己只在挂载时查一次，换掉 `key`
           就是让它重挂一次，再查一次。

           精简时也让它挂上去，条件是有人真的点过一次「检查更新」（`updateNonce > 0`）。
           菜单在精简模式下照样点得到，横幅和底栏又是 `checkUpdate` 仅有的两块画布，
           两块都藏起来就成了「点了跟没点一样」。挂上去不等于看得见：没查到新版时
           `UpdateBanner` 的 `latest` 是 `null`，它什么都不画，精简的版面不受影响。 -->
      <UpdateBanner v-if="!compact || updateNonce > 0" :key="updateNonce" />

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

      <!-- 连不上要说得出原因。非 Windows 上就是「这个系统没有 SimConnect」——
           让人对着一个永远灰着的灯猜，是这个项目反复要躲开的那类故障。
           **当横幅排，不当胶囊排**：它装的是 SimConnect 原样的英文 detail，长度没有
           上限，塞进下面那三格 `grid-cols-3` 会把那一行挤散。
           **精简时也在**：非 Windows 上这是屏幕上最要紧的一句话，和 xpc 的插件横幅
           一样待遇。 -->
      <p
        v-if="!view?.sim_connected && view?.sim_problem"
        class="rounded border border-amber-400 px-3 py-2 text-xs text-amber-700"
      >
        {{ errorText(view.sim_problem) }}
      </p>

      <!-- 机库扫空了要在对话框外面说。包目录那一格在设置对话框的「他机」页上，而对话框
           默认是关着的；机库扫一遍要几十秒，注入还要等它扫完（`wait_for_hangar`，最多
           `HANGAR_WAIT` 120 秒）。填错路径的人在此之前只看得见「天上是空的」，而那和
           没连上模拟器、和机型码对不上长得一模一样。
           **只在模拟器连上之后才说**：没连上模拟器的机器上根本不注入，上面那条
           `sim_problem` 已经把话说完了，再叠一条常驻的琥珀横幅只是噪音（非 Windows 上
           机库永远是空的，它会一直挂着）。
           观察员不上 FSD，天上本来就不会有他机，别拿这两条吓他。 -->
      <p
        v-if="!observer && view?.sim_connected && !view.hangar.loading && !view.hangar.types && view.hangar.liveries"
        class="rounded border border-amber-400 px-3 py-2 text-xs text-amber-700"
      >
        {{ t("hangar.no_types", { liveries: view.hangar.liveries }) }}
      </p>
      <p
        v-else-if="!observer && view?.sim_connected && !view.hangar.loading && !view.hangar.liveries"
        class="rounded border border-amber-400 px-3 py-2 text-xs text-amber-700"
      >
        {{ t("hangar.empty") }}
      </p>

      <p v-if="!mouseSupported && !compact" class="rounded border px-3 py-2 text-xs opacity-60">
        {{ t("status.no_mouse_ptt") }}
      </p>

      <!-- 精简时收起来：`COMPACT_SIZE`（`src-tauri/src/lib.rs`）那 460×340 算的就是
           「消息」加「无线电」两张卡片，这张漏了 `v-if` 只是没补上。
           **代价是真的**——断开按钮只长在这张卡片上，所以精简模式下连不上也断不开。
           这一条是认下的：`WindowToggles` 里那颗「简」一直在，退出精简就是一下；而精简
           本来就是飞起来以后才切进去的形态，连和断都发生在切进去之前。 -->
      <Panel v-if="!compact" :title="t('connect.title')">
        <!-- can-audio 的网格顺序（xpc/gui.py:251-263）：呼号 · CID · 密码 · 机型 · 连接。
             follow 和姓名是 can-voice 多出来的两格，留在同一行。 -->
        <div class="grid grid-cols-7 gap-2">
          <!-- 观察员填的是机长的呼号：语音服务端按那架飞机的位置给他算距离。 -->
          <input
            v-if="observer"
            v-model="observerCallsign"
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
                <span class="font-mono">{{ view?.observer?.callsign }}</span>
              </span>
            </template>
          </div>
        </div>

        <!-- can-audio 把三格状态文案排在网格第二行（xpc/gui.py:265-270）：模拟器、FSD、语音。
             页头那三个 pill 就是搬到这里来的。三格自带名字：FSD 和语音那两句译文本来就以
             它们的名字开头，第一格里 `MSFS` 是拉丁字面量。
             **`sim_problem` 不在这三格里**：它装的是 SimConnect 原样的英文 detail，长度
             没有上限，塞进这三格会把这一行挤散，所以它在卡片上方当横幅排。 -->
        <div class="grid grid-cols-3 gap-2 text-xs">
          <span class="flex items-center gap-1 opacity-70">
            <!-- 模拟器连没连上要一眼看得见：没连上时下面所有数字都是空的，
                 而"空的"和"零"在座舱里是两件事。 -->
            <span
              class="h-2 w-2 shrink-0 rounded-full"
              :style="{ background: view?.sim_connected ? 'var(--can-on)' : 'var(--can-idle)' }"
            />
            MSFS
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

      <!-- 三张卡片：消息、附近管制、他机。文字消息此前整块不存在：管制员打字
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

      <!-- 无线电。can-audio 的 `_build_radio_bar`（`xpc/gui.py:334-380`）：这张卡片
           本来**就是**模拟器状态行，COM1、频道、他机计数和座舱读数都属于这里。 -->
      <!-- 精简时也在，只收起他机计数和座舱读数：屏幕上这颗 PTT 在 Wayland 上是唯一
           能发话的路径，手输频率框是观察员唯一的调频手段，两样都不能跟着精简消失。 -->
      <Panel :title="t('radio.title')">
        <!-- 排不下就换行，不是裁掉：900×600 的最小尺寸只管正常模式，精简时窗口能
             拖到 320px 宽。 -->
        <div class="flex flex-wrap items-center gap-2">
          <StateToggle label="TX" :state="txState" :width="52" :height="26" />
          <StateToggle label="RX" :state="rxState" :width="52" :height="26" />

          <!-- 电门关着时照样显示调在哪：这是座舱里的读数，而「听不见」由左边两块
               色块转暗去说。 -->
          <span class="font-mono text-[15px] font-bold tabular-nums">
            {{
              view?.sim?.com1
                ? t("radio.com1", { frequency: mhz(view.sim.com1) })
                : t("radio.com1_none")
            }}
          </span>

          <!-- 手输频率**只有观察员有**。正常上网络的飞行员要是能把语音频率和座舱 COM1
               分开设，迟早出现「管制以为你在 121.8、你人在别的频道」，那比听不见更糟。 -->
          <input
            v-if="observer"
            v-model="manualFrequency"
            :placeholder="t('observer.frequency_placeholder')"
            :title="t('observer.frequency_tip')"
            class="w-28 rounded border px-2 py-1 font-mono text-xs"
            @change="applyFrequency"
          />
          <!-- 频道文案也只有观察员有：飞行员的频道就是左边那个 COM1，同一个数字写两遍
               就是两个真相（`:352-353` 原来那条注释说的就是这件事）。 -->
          <template v-if="observer && view?.observer">
            <span v-if="view.observer.frequency !== null" class="font-mono text-xs opacity-60">
              {{ khzText(view.observer.frequency) }}
              {{ view.observer.manual ? t("observer.manual") : t("observer.follow_com1") }}
            </span>
            <span
              v-else
              class="min-w-0 truncate text-xs text-amber-700"
              :title="t('observer.no_frequency')"
            >
              {{ t("observer.no_frequency") }}
            </span>
          </template>

          <span class="grow" />

          <span v-if="!compact" class="font-mono text-xs tabular-nums opacity-60">
            {{ t("radio.traffic", { count: view?.traffic.length ?? 0 }) }}
          </span>

          <!-- 座舱读数。精简时收起：这几个数模拟器里都有，压在模拟器上的窗口不必再
               显示一遍。 -->
          <span
            v-if="!compact"
            class="font-mono text-xs tabular-nums opacity-60"
            :title="cockpitTitle()"
          >
            {{ cockpitText() }}
          </span>

          <!-- 一直画着，没连上就是灰的：格子来回出现会让右边所有东西横着跳。
               观察员没有 FSD 链路，`connected` 对他恒为假，所以这颗对他一直是灰的。 -->
          <button
            class="rounded border px-3 py-1 text-xs"
            :disabled="!connected || busy"
            @click="ident"
          >
            {{ t("session.ident") }}
          </button>

          <button
            class="rounded border px-4 py-2 text-xs"
            :class="talking ? 'text-white' : ''"
            :style="talking ? { background: 'var(--can-active)' } : {}"
            :title="t('chat.push_to_talk_tip')"
            @pointerdown="pttDown"
            @pointerup="pttUp"
            @pointercancel="pttUp"
          >
            {{ t("chat.push_to_talk") }}
          </button>
        </div>
      </Panel>

      <!-- can-audio 飞行员端的 `QStatusBar`（`xpc/gui.py:200`）：只说「就绪」和瞬时状态。
           **不传 `pttTitle`**，所以这条栏上不画 PTT——那颗按钮在上面那一行里。
           也**不接 `@down` / `@up`**：`StatusBar` 卸载时会补发一次 `up` 当保险
           （`StatusBar.vue:54`），而 xpc 的 PTT 不在这条栏上，拆这条栏不该松开麦克风。
           `talking` 仍然要传，它是必填属性。

           **精简时这条栏也一直在**，插槽里补上三格状态文案。连接卡片在精简模式下
           收了起来（见它那里的注释），而它把「模拟器通没通、FSD 在不在线、语音是
           不是好的」一并带走了——换布局之前这三句在页头，而页头不随精简收起。
           飞起来之后唯一要盯的就是这三件事，不在这里接住就是退步。
           顺带，「检查更新」在精简模式下也就有地方说话了：藏掉这条栏，那条菜单
           在精简模式下是空操作。 -->
      <StatusBar :talking="talking" :status="barStatus">
        <span v-if="compact" class="flex shrink-0 items-center gap-2 opacity-70">
          <span class="flex items-center gap-1">
            <span
              class="h-2 w-2 shrink-0 rounded-full"
              :style="{ background: view?.sim_connected ? 'var(--can-on)' : 'var(--can-idle)' }"
            />
            MSFS
          </span>
          <span class="truncate">
            {{ observing ? t("status.observing") : linkText(view?.link) }}
          </span>
          <span class="truncate">{{ voiceText(view?.voice) }}</span>
        </span>
      </StatusBar>

      <FlightPlanDialog :open="showPlan" :observer="observer" @close="showPlan = false" />
      <SettingsDialog :open="showPrefs" @close="showPrefs = false">
        <PilotSettings :cid="cid" :hangar="view?.hangar" />
      </SettingsDialog>

      <!-- `about.body` 里有 `\n`。`whitespace-pre-line` 让浏览器照着换行就够了，
           **不要 `v-html`**：那句话里插着版本号和日志文件路径，都是从程序外面来的字符串。 -->
      <Modal
        :open="showAbout"
        :title="t('about.title')"
        width="w-[28rem]"
        @close="showAbout = false"
      >
        <p class="whitespace-pre-line text-xs leading-relaxed">
          {{
            t("about.body", {
              name: t("app.title"),
              version,
              log: logPath || t("about.no_log"),
            })
          }}
        </p>
      </Modal>
    </main>
  </StartupGate>
</template>
