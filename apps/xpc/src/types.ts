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

/** 应答机档位对人怎么说。 */
export function xpdrText(mode: XpdrMode | undefined): string {
  switch (mode) {
    case "Standby":
      return "待机";
    case "Ident":
      return "识别中";
    case "ModeC":
      return "C 模式";
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

/** 插件现状对人怎么说。 */
export function installText(s: InstallStatus | null): string {
  switch (s?.state) {
    case "NoRoot":
      return "没找到 X-Plane。请在下面填它装在哪。";
    case "NotXplane":
      return "这个目录不像 X-Plane 装的地方——里面没有 Resources/plugins。";
    case "Missing":
      return "还没装。";
    case "Outdated":
      return "装着的那份和这个客户端带的不一样，建议更新。";
    case "Current":
      return "已经是最新的。";
    default:
      return "正在看…";
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
}

/** 语音链路对人怎么说。 */
export function voiceText(v: VoiceSnapshot | null | undefined): string {
  if (!v) return "未连接";
  switch (v.link) {
    case "Online":
      return "语音已连接";
    case "Connecting":
      return "语音连接中…";
    case "Reconnecting":
      return "语音重连中…";
    case "Evicted":
      return "这个账号在别处登录了，语音已断开";
    default:
      return "语音已断开";
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
  if (to === "*S") return "督导";
  if (to === "*") return "全网广播";
  if (to.startsWith("@")) {
    const digits = to.slice(1);
    if (/^\d{5}$/.test(digits)) return `1${digits.slice(0, 2)}.${digits.slice(2)}`;
    return to;
  }
  return to;
}
