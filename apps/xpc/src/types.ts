import { t } from "./i18n";

// 这些形状是 Rust 侧序列化出来的。改那边的字段名，这里要跟着改。
export type XpdrMode = "Standby" | "ModeC" | "Ident";

export interface SimSnapshot {
  latitude: number;
  longitude: number;
  altitude: number;
  pressure_delta: number;
  agl: number;
  groundspeed: number;
  pitch: number;
  bank: number;
  heading: number;
  squawk: number;
  xpdr_mode: XpdrMode;
  com1: number | null;
  com2: number | null;
  com1_power: boolean;
  on_ground: boolean;
}

export interface TrafficEntry {
  callsign: string;
  squawk: number;
  latitude: number;
  longitude: number;
  altitude: number;
  heading: number;
  groundspeed: number;
  vertical_speed: number;
  equipment: string;
  airline: string;
  range_nm: number | null;
}

export type FsdState =
  | "Connecting"
  | "Online"
  | "Reconnecting"
  | "Error"
  | "Offline"
  | "Stopped";

/**
 * 菜单上刚点的那一项。和 Rust 侧 `MenuRequest` 一一对应。
 *
 * **读一次就没了**：`view` 每调一次就把它取走并清掉。所以读到非 null 要当场
 * 处理，不要存进别的地方等以后——不会有以后。
 */
export type MenuRequest = "flight_plan" | "settings" | "update" | "about";

export interface View {
  sim_connected: boolean;
  sim: SimSnapshot | null;
  link: FsdState | null;
  traffic: TrafficEntry[];
  voice: VoiceSnapshot | null;
  /** 收发过的文字消息，最旧的在前。 */
  messages: ChatMessage[];
  /** 在线管制席位，按呼号排序。 */
  controllers: ControllerEntry[];
  /** X-Plane 插件。`null` = 没听到过它：没装、没启用，或者 X-Plane 没开。 */
  plugin: PluginView | null;
  /** CSL 扫到了什么。 */
  csl: CslView;
  /** 以观察员身份连着时的状况；没连、或者正常上着网是 `null`。 */
  observer: ObserverView | null;
  /** 菜单上刚点的那一项，没点过是 `null`。见 `MenuRequest`。 */
  menu: MenuRequest | null;
}

/**
 * CSL 那一侧的状况。
 *
 * **要显示出来**：扫不到模型的表现是"天上是空的"，和没装插件、和 UDP 不通
 * 在界面上长得一模一样，而三者要做的事完全不同。
 */
export interface CslView {
  /** 扫的是哪个目录。 */
  root: string;
  /** 扫到几个模型。 */
  models: number;
  /** 还在扫。几个 GB 的包要几十秒，这段时间里的 0 不是"一个都没有"。 */
  loading: boolean;
}

export function mhz(khz: number | null | undefined): string {
  return khz === null || khz === undefined ? "—" : khz.toFixed(3);
}

/** kHz 写成 `121.800`。观察员那一栏的频率是 kHz，COM1 读数是 MHz，别混用。 */
export function khzText(khz: number | null | undefined): string {
  return khz === null || khz === undefined ? "—" : (khz / 1000).toFixed(3);
}

/**
 * 观察员那一侧的状况。
 *
 * `frequency` 是 `null` 时要说出来：没有频率的观察员连得上、灯是绿的，却什么也听不见。
 */
export interface ObserverView {
  /** 跟随的呼号。 */
  follow: string;
  /** 语音此刻该在的频率，kHz。 */
  frequency: number | null;
  /** 这个频率是手输的，不是跟着 COM1 来的。 */
  manual: boolean;
}

/** 应答机档位对人怎么说。在模板里调：切了语言要跟着变。 */
export function xpdrText(mode: XpdrMode | undefined): string {
  switch (mode) {
    case "Standby":
      return t("xpdr.standby");
    case "Ident":
      return t("xpdr.ident");
    case "ModeC":
      return t("xpdr.mode_c");
    default:
      return "—";
  }
}

/** FSD 的 `$FP` 一共 17 段，这里就是那 17 段。少一段服务端回 "Too few fields"。 */
export interface FlightPlan {
  rules: string;
  aircraft: string;
  cruise_speed: string;
  departure: string;
  departure_time: string;
  actual_time: string;
  cruise_altitude: string;
  arrival: string;
  enroute_hours: string;
  enroute_minutes: string;
  fuel_hours: string;
  fuel_minutes: string;
  alternate: string;
  remarks: string;
  route: string;
}

export function emptyFlightPlan(): FlightPlan {
  return {
    rules: "I",
    aircraft: "",
    cruise_speed: "",
    departure: "",
    departure_time: "",
    actual_time: "",
    cruise_altitude: "",
    arrival: "",
    enroute_hours: "",
    enroute_minutes: "",
    fuel_hours: "",
    fuel_minutes: "",
    alternate: "",
    remarks: "",
    route: "",
  };
}

export interface Settings {
  cid: string;
  callsign: string;
  aircraft: string;
  real_name: string;
  input_device: string | null;
  output_device: string | null;
  mic_volume: number;
  speaker_volume: number;
  inject: boolean;
  /** 收到管制消息时播放提示音。 */
  message_sound: boolean;
  /** 频率上的每条消息都提示，而不是只提示点到自己呼号的。 */
  message_sound_all: boolean;
  /** 提示音音量，百分比，0–200。 */
  message_sound_volume: number;
  /** 上次装插件用的 X-Plane 目录。 */
  xplane_root: string;
  /** 他机显示距离（海里）。 */
  traffic_range_nm: number;
  /** CSL 包放在哪。空的表示跟着 X-Plane 目录走。 */
  csl_dir: string;
  /** 观察员模式（双人机组的右座）：只连语音，不上 FSD。 */
  observer: boolean;
  /** 观察员跟随的呼号，机长那架飞机的。 */
  follow: string;
  /** 观察员手输的频率，kHz。`null` = 跟随 COM1。 */
  observer_frequency: number | null;
}

/** 他机插件装好了没有。取值和 Rust 那边的 `install::State` 一一对应。 */
export type InstallState = "NoRoot" | "NotXplane" | "Missing" | "Outdated" | "Current";

export interface InstallStatus {
  /** 看的是哪个目录。空的表示一个都没找到。 */
  root: string;
  state: InstallState;
  /** XPPython3 在不在。**不在的话插件装了也不会跑**，而那是另一个包。 */
  xppython3: boolean;
  installed_protocol: number | null;
  bundled_protocol: number;
  path: string;
  /** 装着的那份和这个客户端不是同一种话。 */
  protocol_mismatch: boolean;
  can_install: boolean;
}

/** 插件现状对人怎么说。在模板里调：切了语言要跟着变。 */
export function installText(s: InstallStatus | null): string {
  switch (s?.state) {
    case "NoRoot":
      return t("plugin.state.no_root");
    case "NotXplane":
      return t("plugin.state.not_xplane");
    case "Missing":
      return t("plugin.state.missing");
    case "Outdated":
      return t("plugin.state.outdated");
    case "Current":
      return t("plugin.state.current");
    default:
      return t("plugin.state.looking");
  }
}

/**
 * 语音那一侧的快照。
 *
 * **不要把它收窄成 `{ link: string }`**：那样语音被顶号、被拒、声卡打不开，
 * 飞行员一律看不见——而他正戴着耳机等人回话。
 */
export interface VoiceSnapshot {
  link: string;
  ended: unknown;
  notices: [string, number, string][];
  /** 每个频率上正在讲话的人。键是 kHz 的十进制写法。 */
  receiving?: Record<string, number[]>;
}

/** 语音链路对人怎么说。在模板里调：切了语言要跟着变。 */
export function voiceText(v: VoiceSnapshot | null | undefined): string {
  if (!v) return t("voice.none");
  switch (v.link) {
    case "Online":
      return t("voice.online");
    case "Connecting":
      return t("voice.connecting");
    case "Reconnecting":
      return t("voice.reconnecting");
    case "Evicted":
      return t("voice.evicted");
    default:
      return t("voice.offline");
  }
}

/**
 * 服务端通知的人话。措辞在 `common.*.json` 里，四个客户端共用一份：这些 kind
 * 是协议的一部分，同一条通知不该在管制端和飞行员端有两种说法。
 *
 * **不认识的 kind 也要显示出来**：一条服务端认为值得说、而客户端太旧不认识的
 * 通知，落到界面上是一句原文，总好过一片安静。
 */
export function noticeText([kind, freq, reason]: [string, number, string]): string {
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

/**
 * FSD 链路对人怎么说。在模板里调：切了语言要跟着变。
 *
 * 取值和 Rust 那边的 `FsdState` 一一对应，每一种都要有一句——漏掉的那种会在
 * 界面上显示成空白。
 */
export function linkText(link: FsdState | null | undefined): string {
  switch (link) {
    case "Connecting":
      return t("fsd.connecting");
    case "Online":
      return t("fsd.online");
    case "Reconnecting":
      return t("fsd.reconnecting");
    case "Error":
      return t("fsd.error");
    case "Offline":
      return t("fsd.offline");
    case "Stopped":
      return t("fsd.stopped");
    default:
      return "";
  }
}

export interface PluginView {
  version: number;
  drawn: number;
  /**
   * 协议版本对不对得上。
   *
   * **对不上时插件静默丢弃每一帧**，症状是"完全没有交通"而两边日志都干净。
   */
  version_ok: boolean;
}

/** 一条文字消息。`at` 是单调秒，只用来排序和做 key，不是时钟。 */
export interface ChatMessage {
  from: string;
  /** 收件人：呼号、`@` 加五位频率，或者 `*` / `*S`。 */
  to: string;
  text: string;
  /** 自己发出去的吗。 */
  outbound: boolean;
  at: number;
}

export interface ControllerEntry {
  callsign: string;
  /** MHz。 */
  frequency: number;
  facility: number;
  rating: number;
  /** 离本机多远，海里。不知道本机在哪时是 null。 */
  range_nm: number | null;
}

/**
 * 收件人对人怎么说。
 *
 * `@28750` 是"发到 128.750 上"——**不翻的话它读起来像一个乱码呼号**，
 * 而频率消息和点名叫你的私聊是两件事。
 */
export function recipientText(to: string): string {
  if (to === "*S") return t("recipient.supervisor");
  if (to === "*") return t("recipient.broadcast");
  if (to.startsWith("@")) {
    const digits = to.slice(1);
    if (/^\d{5}$/.test(digits)) return `1${digits.slice(0, 2)}.${digits.slice(2)}`;
    return to;
  }
  return to;
}
