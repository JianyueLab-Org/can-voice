<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import WindowToggles from "./components/WindowToggles.vue";
import SettingsDialog from "./components/SettingsDialog.vue";
import Toast from "./components/Toast.vue";
import { appearance, loadAppearance } from "./appearance";
import { invoke } from "@tauri-apps/api/core";
import RadioRow from "./components/RadioRow.vue";
import UpdateBanner from "./components/UpdateBanner.vue";
import StartupGate from "./components/StartupGate.vue";
import SettingsPanel from "./components/SettingsPanel.vue";
import OnlineList from "./components/OnlineList.vue";
import StatusBar from "./components/StatusBar.vue";
import LoginCard from "./components/LoginCard.vue";
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
  effective_rx: number[];
  effective_tx: number[];
  effective_xc: number[][];
  health: {
    rtt_ms: number;
    sent: number;
    received: number;
    lost: number;
    unparsable: number;
    /**
     * 播放环的对账。**链路好和听得清是两件事**：上面那几个数只讲到声卡门口
     * 为止，链路全绿而播放环跑干时它们一个都不会动，而用户听到的是电音加
     * 卡顿。`underruns` 是欠载的**段数**，不是回调数。
     */
    playback: {
      depth_ms: number;
      silence_ms: number;
      underruns: number;
      trimmed_ms: number;
      steer_added: number;
      steer_removed: number;
    };
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
const busy = ref(false);
/** 版本号，登录页那行。读的是 Rust 侧 `app_version` 命令，和日志、User-Agent 同一个常量。 */
const version = ref("");
/** 真的按过一次连接没有。区分"还没试"和"链路正在连"，登录页那行字要分开说。 */
const attempted = ref(false);

/**
 * 要显示的那条错误。
 *
 * **存原样的东西，渲染时再翻**：存一句翻好的话，切了语言之后已经显示着的那一句
 * 还是原来的语言。
 */
type Problem = { kind: "command"; error: unknown } | { kind: "frequency" };
const error = ref<Problem | null>(null);

/**
 * 报错的序号。**同一句话连着报两次，prop 的值不变，Vue 不会重绘子组件**
 * ——右上角那条就再也不出现了，而这一次改动把另外两个显示错误的地方都拿掉了。
 * 递增一个计数器，让 Toast 盯着它而不是盯着那句话。
 */
const errorSeq = ref(0);

/**
 * 报一条错。
 *
 * **台面这一页只有右上角那一个报错出口。** 登录页有它自己那行状态文案，
 * 台面上就只剩这一条 4 秒自动消失的提示：连上之后再挂一条不会自己消失的横条，
 * 值班的人得腾出手去关它。
 */
function report(p: Problem) {
  error.value = p;
  errorSeq.value++;
}

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

let timer: number | undefined;
let refreshInFlight = false;

// 界面**读快照**，不从事件流拼：事件是广播，窗口重开之前发生的事收不到。
async function refresh() {
  if (refreshInFlight) return;
  refreshInFlight = true;
  const snapshotRequest = invoke<Snapshot>("snapshot");
  const radiosRequest = invoke<Radio[]>("radios");
  const pressedRequest = invoke<boolean>("ptt_pressed");
  const feedRequest = invoke<FeedView>("feed");
  const request = Promise.all([snapshotRequest, radiosRequest, pressedRequest, feedRequest]);
  void Promise.allSettled([snapshotRequest, radiosRequest, pressedRequest, feedRequest]).then(() => {
    refreshInFlight = false;
  });
  try {
    const [nextSnap, nextRadios, nextPressed, nextFeed] = await Promise.race([
      request,
      new Promise<never>((_, reject) => window.setTimeout(() => reject(new Error("refresh timeout")), 1500)),
    ]);
    snap.value = nextSnap;
    radios.value = nextRadios;
    pressed.value = nextPressed;
    feed.value = nextFeed;
  } catch (e) {
    report({ kind: "command", error: e });
  }
}

let detachPtt: (() => void) | undefined;
onMounted(() => {
  void init();
});

async function init() {
  void loadAppearance().catch((e) => report({ kind: "command", error: e }));
  timer = window.setInterval(() => void refresh(), 200);
  detachPtt = attachPttKeys();
  window.addEventListener("blur", pttUp);
  document.addEventListener("visibilitychange", pttUp);
  // 上次用的 CAN 号预填。密码不存——它只换一张 60 秒的票。
  try {
    cid.value = (await invoke<{ cid: string }>("settings")).cid;
  } catch (e) {
    report({ kind: "command", error: e });
  }
  try {
    version.value = await invoke<string>("app_version");
  } catch {
    // 版本号显示不出来不该拖垮轮询：下面这几行不能因为这一句失败而不跑。
  }
  await refresh();
}
onUnmounted(() => {
  window.clearInterval(timer);
  detachPtt?.();
  window.removeEventListener("blur", pttUp);
  document.removeEventListener("visibilitychange", pttUp);
});

/** 此刻是不是真的在线。**只给"就在这一秒"的地方用**，别拿它切页面，见 `signedIn`。 */
const connected = computed(() => snap.value?.link === "Online");

/**
 * 这一场会话上过线没有——**登录页和台面是按它切的，不是按 `connected`**。
 *
 * `Reconnecting` 是网络抖一下就会进的正常状态（`can-voice-client/src/pump.rs`），
 * 后端刻意让台面活着穿过它（`can-voice-app/src/snapshot.rs` 只清 `receiving`）。
 * 拿 `connected` 切页的话，值班时抖那么一下，卡片栈、在线条、值守横幅和屏幕上
 * 那颗 PTT 会一起换成一张密码卡——而 Wayland 上那颗按钮是唯一能发话的路径
 * （见 StatusBar.vue），等于在手上有飞机的时候悄悄拿走发射能力；人在那里按
 * 「连接」，还会在重连途中再发一次 connect。
 *
 * 回登录页只有三条路：还没连过、自己点了断开、以及 `Offline` / `Evicted` 这两个
 * 终态——链路自己放弃了，台面已经没有意义。
 */
const signedIn = ref(false);
watch(
  () => snap.value?.link,
  (link) => {
    if (link === "Online") signedIn.value = true;
    else if (link === "Offline" || link === "Evicted") {
      // 从台面退回登录页时丢掉那条报错：它讲的是刚才那一场里的某个命令，
      // 而登录页那行状态文案会把它当成"这一次连接失败的原因"。
      if (signedIn.value) error.value = null;
      signedIn.value = false;
    }
  },
);

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

/**
 * 登录页那行字。没连上时说链路状态，连接失败时说失败的原因。
 *
 * `statusText` 在 Offline 时已经会把 `ended` 说成人话，所以这里只需要在
 * 命令本身失败时盖掉它。
 */
const loginStatus = computed(() => {
  if (error.value?.kind === "command") return problemText(error.value);
  return attempted.value && snap.value ? statusText.value : t("login.idle");
});
const loginFailed = computed(() => error.value?.kind === "command");

/**
 * 底栏中间那句话。can-audio 的状态栏空闲时说"就绪"，有事说那件事。
 *
 * 和顶栏那句 `statusText` 分开：那一句讲链路，这一句讲刚刚发生了什么。
 */
const barStatus = computed(() => t("status.ready"));

/**
 * 顶栏那句会话文案（spec §4 的「会话文案」）。can-audio 放的是
 * `用户名 · 服务器`（`controller/gui.py:1066`），这里说的是同一件事：哪个账号在线。
 *
 * **和底栏那句值守文案是两句话**：这一句讲会话，那一句讲在不在席位上。
 * 两处都画值守文案的话，同一句中文会同时出现在右上角和右下角。
 */
const sessionText = computed(() =>
  cid.value ? t("link.session", { cid: cid.value }) : t("login.idle"),
);

/** 右边那句话。can-audio 值守时着绿，不值守说"只收不发"。 */
const dutyText = computed(() =>
  onDuty.value
    ? t("duty.staffing", { callsign: feed.value?.duty.callsign ?? "" })
    : t("duty.observer"),
);

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

async function connect(enteredCid: string, enteredPassword: string) {
  attempted.value = true;
  error.value = null;
  busy.value = true;
  try {
    await invoke("connect", { cid: enteredCid, password: enteredPassword });
    cid.value = enteredCid;
    await refresh();
  } catch (e) {
    report({ kind: "command", error: e });
  } finally {
    busy.value = false;
  }
}

async function disconnect() {
  // 自己点的断开是回登录页的那三条路之一，立刻切，不等下一拍快照。
  signedIn.value = false;
  error.value = null;
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
  return khz >= 118000 && khz <= 136975 && khz % 5 === 0 ? khz : null;
}

async function addFrequency() {
  const khz = parseFreq(freqInput.value);
  if (khz === null) {
    report({ kind: "frequency" });
    return;
  }
  error.value = null;
  freqInput.value = "";
  const callsign = callsignInput.value.trim();
  callsignInput.value = "";
  await act("add_frequency", { freqKhz: khz, callsign: callsign || null });
}

/** 从在线一览点过来的，把呼号一起带上。 */
const addOnline = (khz: number, callsign: string) =>
  act("add_frequency", { freqKhz: khz, callsign });

/** 在席位上没有。**查不到不算不在**——那两句话要分开说。 */
const onDuty = computed(() => !!feed.value?.reachable && !!feed.value?.duty.callsign);
const tuned = computed(() => radios.value.map((r) => r.freq_khz));
const locked = (khz: number) =>
  !!feed.value?.reachable && feed.value.duty.freq_khz === khz;
/** 画灰与否照着台面的真相，不自己推：推出来的那份迟早和它对不上。 */
const mayTransmit = computed(() => feed.value?.transmit_allowed ?? true);

/**
 * 屏幕上那颗 PTT。**两头都写成幂等的**：指针捕获在 StatusBar 里，而那条栏被拆掉
 * 时（切精简）它会补发一次松手，重复的那一次不该再发一趟命令。
 */
function pttDown() {
  if (!connected.value) return;
  if (holding.value) return;
  holding.value = true;
  void invoke("set_transmitting", { on: true });
}
function pttUp() {
  if (!holding.value) return;
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

/**
 * 发一条命令再刷一次快照。
 *
 * **失败要说出来。** 台面上每一个开关都走这里，而 `invoke` 失败原来是一个没人
 * 接的 Promise：`set_switch` 被拒时界面什么都不说，开关自己弹回去，看起来像点
 * 空了一下。
 */
async function act(name: string, args: Record<string, unknown>) {
  try {
    await invoke(name, args);
  } catch (e) {
    report({ kind: "command", error: e });
  }
  await refresh();
}
</script>

<template>
  <StartupGate>
    <!-- 登录页也要有置顶/精简/设置这一排，而且留白也要跟着精简缩。
         端点、PTT、日志这三块都在设置对话框里：装了 msi 的人改地址、发日志，
         都是在还没连上的时候要做的事，把它们关在登录之后等于关在门里
         （SettingsCommon.vue 开头那段）。精简开关留着，才有路从 248×186 出来
         ——Rust 侧启动时就把存着的 compact 应用上去了（lib.rs 的 setup）。 -->
    <main
      v-if="!signedIn"
      class="app-shell flex h-screen w-full flex-col text-sm"
      :class="compact ? 'gap-1 p-1.5' : 'gap-3 p-5'"
    >
      <header class="flex items-center gap-2">
        <WindowToggles class="ml-auto" @settings="showPrefs = true" />
      </header>
      <UpdateBanner v-if="!compact" />
      <LoginCard
        :cid="cid"
        :status="loginStatus"
        :failed="loginFailed"
        :busy="busy"
        :version="version"
        @connect="connect"
      />
    </main>

    <!-- 精简时留白也跟着缩：留着正常模式的边距，一张卡的窗口里有一半是空的。 -->
    <main
      v-else
      class="app-shell flex h-screen w-full flex-col text-sm"
      :class="compact ? 'gap-1 p-1.5' : 'gap-4 p-5'"
    >
      <!-- can-audio 的顺序（controller/gui.py:536-583）：灯、链路、会话、撑开、
           置顶、精简、设置、断开。标题不在这里——旧版主页面没有标题，它在窗口
           装饰和登录卡片上。 -->
      <!-- **不换行。** 精简时这一条要在 186px 的窗口里装下灯、两颗 30×26 的按钮，
           而 `truncate` 碰上 `flex-wrap` 会先换行再截断——换行的那一行把卡片挤出
           窗口。改成不换行、文案那两格 `min-w-0` 允许收缩，挤不下时截断。 -->
      <header class="flex items-center gap-2">
        <!-- 灯此刻是真的两色：页面按「这一场上过线没有」切，所以重连时这里是台面，
             灯照着 `connected` 转红，`statusText` 说「重连中…」。 -->
        <span
          class="inline-block h-2.5 w-2.5 shrink-0 rounded-full"
          :style="{ background: connected ? 'var(--can-on)' : 'var(--can-muted)' }"
        />
        <p v-if="!compact" class="min-w-0 truncate text-xs opacity-70">{{ statusText }}</p>
        <p v-if="!compact" class="min-w-0 truncate text-xs font-semibold">{{ sessionText }}</p>
        <WindowToggles class="ml-auto" @settings="showPrefs = true" />
        <button v-if="!compact" class="rounded border px-3 py-1 text-xs" @click="disconnect">
          {{ t("login.disconnect") }}
        </button>
      </header>

      <UpdateBanner v-if="!compact" />

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

      <!-- 在不在席位上。这件事此前界面上完全没有，而它决定了能不能发射。
           `connected` 这个条件是活的：重连途中这一页还在，而那时 `feed` 说的是
           上一拍的席位，报「你不在席位上」只会把人吓一跳。链路回来再说。 -->
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

      <!-- can-audio 的比例（controller/gui.py:586-617）：频率 1、呼号 2、按钮定宽。
           设置的入口不在这一行——旧版只有一个设置对话框。 -->
      <section v-if="!compact" class="flex items-center gap-2">
        <input
          v-model="freqInput"
          :placeholder="t('freq.hint')"
          class="min-w-[120px] max-w-[200px] flex-1 rounded border px-2 py-1"
          @keyup.enter="addFrequency"
        />
        <input
          v-model="callsignInput"
          :placeholder="t('freq.callsign_hint')"
          class="min-w-[160px] max-w-[340px] flex-[2] rounded border px-2 py-1 font-mono uppercase"
          @keyup.enter="addFrequency"
        />
        <button
          class="shrink-0 rounded border px-3 py-1 font-semibold text-white"
          :style="{ background: 'var(--can-theme)' }"
          @click="addFrequency"
        >
          {{ t("freq.add") }}
        </button>
        <span class="flex-1" />
      </section>

      <section class="flex min-h-0 flex-1 flex-col">
        <div class="flex flex-1 flex-wrap content-start gap-2 overflow-auto">
          <RadioRow
            v-for="r in radios"
            :key="r.freq_khz"
            :radio="r"
            :effective-rx="snap?.effective_rx?.includes(r.freq_khz) ?? false"
            :effective-tx="snap?.effective_tx?.includes(r.freq_khz) ?? false"
            :effective-xc="snap?.effective_xc?.some(([a, b]) => a === r.freq_khz || b === r.freq_khz) ?? false"
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
          <!-- 空栈提示在精简模式下没有意义（can-audio `gui.py:688` 同样藏它）：
               那个窗口里除了卡片什么都不该有。 -->
          <p
            v-if="!radios.length && !compact"
            class="w-full py-6 text-center text-xs opacity-50"
          >
            {{ t("freq.empty") }}
          </p>
        </div>

        <!-- 在线一览。没有它的话，加一个别人的频率要先去别的地方查他在守什么。 -->
        <!-- can-audio 把标题和药丸放在同一行（controller/gui.py:636-647）。 -->
        <div v-if="connected && !compact" class="mt-2 flex items-center gap-2 border-t pt-2">
          <p class="shrink-0 text-xs opacity-60">{{ t("online.title") }}</p>
          <OnlineList :online="feed?.online ?? []" :tuned="tuned" @add="addOnline" />
        </div>
      </section>

      <StatusBar
        v-if="!compact"
        :talking="talking"
        :status="barStatus"
        :duty="dutyText"
        :duty-on="connected && onDuty"
        :ptt-title="t('ptt.hold_tip')"
        @down="pttDown"
        @up="pttUp"
      >
        <span v-if="snap?.health" class="shrink-0 opacity-60">
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
      </StatusBar>
      <!-- 台面这一页唯一的报错出口：频率填错、开关被拒，都走这一条。 -->
      <Toast :message="error ? problemText(error) : null" :seq="errorSeq" />
    </main>

    <!-- 两页共用的一张对话框。**摆在两个 main 外面**：它是 `fixed` 覆盖层，
         位置和哪一页无关，而端点、日志这两块恰恰是没连上时才要用的。 -->
    <SettingsDialog :open="showPrefs" @close="showPrefs = false">
      <SettingsPanel :cid="cid" />
    </SettingsDialog>
  </StartupGate>
</template>
