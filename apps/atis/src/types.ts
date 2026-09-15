// 这些形状是 Rust 侧 `can_voice_atis::profile` / `script` 的**序列化结果**。
// 改那边的字段名，这里要跟着改——两边没有共用的类型，只有一份约定。
export type AtisType = "combined" | "departure" | "arrival";
export type VoiceLanguage = "en" | "zh" | "both";

export interface Preset {
  name: string;
  template: string;
  airport_conditions: string;
  notams: string;
  transition_level: string;
  chinese_runway: string;
  closing: string;
  chinese_extra: string;
}

export interface Station {
  identifier: string;
  name: string;
  frequency: string;
  atis_type: AtisType;
  code_range: [string, string];
  letter: string;
  presets: Preset[];
  contractions: Record<string, [string, string]>;
  latitude: number;
  longitude: number;
  voice_language: VoiceLanguage;
  chinese_name: string;
  chinese_runway: string;
}

export interface Rendered {
  text: string;
  voice_en: string;
  voice_zh: string;
  /** 发上 FSD 的那一份。中英混播时带一个 `|`，英文在前。 */
  wire: string;
}

export type FsdState =
  | "Connecting"
  | "Online"
  | "Reconnecting"
  | "Error"
  | "Offline"
  | "Stopped";

export type Reason =
  | "LoginTimeout"
  | "Closed"
  | "Dropped"
  | "Online"
  | "Stopped"
  | { Callsign: unknown }
  | { Connecting: { rating: number } }
  | { ConnectFailed: string }
  | { Rejected: { code: string; message: string } }
  | { SendFailed: string }
  | { BadFrequency: string }
  | { Retrying: { attempt: number; limit: number } }
  | { GaveUp: { limit: number } };

export interface Live extends Rendered {
  state: FsdState | null;
  reason: Reason | null;
  letter: string;
  preset: string;
  metar: string;
}

export function callsignOf(s: Station): string {
  const suffix =
    s.atis_type === "departure" ? "_D_ATIS" : s.atis_type === "arrival" ? "_A_ATIS" : "_ATIS";
  return s.identifier + suffix;
}

/** 一条连接此刻该对人说什么。 */
export function stateText(live: Live | undefined): string {
  if (!live || !live.state) return "未上线";
  switch (live.state) {
    case "Online":
      return "在播";
    case "Connecting":
      return "连接中…";
    case "Reconnecting":
      return "重连中…";
    case "Stopped":
      return "已停止";
    case "Offline":
      return "已下线（重连用尽）";
    case "Error":
      return reasonText(live.reason);
  }
}

/** 说得出是哪一条，而不是一句"连接失败"。 */
export function reasonText(reason: Reason | null): string {
  if (!reason) return "出错了";
  if (typeof reason === "string") {
    switch (reason) {
      case "LoginTimeout":
        return "登录超时：服务端没有回应";
      case "Closed":
        return "登录时对端关掉了连接";
      case "Dropped":
        return "连接断了";
      case "Online":
        return "在播";
      case "Stopped":
        return "已停止";
    }
  }
  if ("Rejected" in reason)
    return `服务端拒绝登录（${reason.Rejected.code}）：${reason.Rejected.message}`;
  if ("ConnectFailed" in reason) return `连不上：${reason.ConnectFailed}`;
  if ("Callsign" in reason) return "呼号不合服务端的规矩";
  if ("BadFrequency" in reason) return `认不出这个频率：${reason.BadFrequency}`;
  if ("SendFailed" in reason) return `发送失败：${reason.SendFailed}`;
  if ("Retrying" in reason)
    return `重连中（第 ${reason.Retrying.attempt} / ${reason.Retrying.limit} 次）`;
  if ("GaveUp" in reason) return `重连 ${reason.GaveUp.limit} 次都没成，这个席位已下线`;
  if ("Connecting" in reason) return "连接中…";
  return "出错了";
}
