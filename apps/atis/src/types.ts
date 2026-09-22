import { errorText, messageText, t, type Message } from "./i18n";

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

/**
 * METAR 里的风组和气压组。can-audio 那边是解析过的 METAR 对象
 * （`atis/script.py:99`），这边只有原文，所以现抓两组。
 *
 * **抓不到就不显示**——和 can-audio 的 `if text:` 一样。列表那一行是给人扫一眼的，
 * 宁可少两段，不能显示一段错的。
 */
const WIND = /\b(?:VRB|\d{3})\d{2,3}(?:G\d{2,3})?(?:MPS|KT|KMH)\b/;
const QNH = /\b(?:Q\d{3,4}|A\d{4})\b/;

/**
 * 席位列表里的那一行：`ZSPD  J  09004MPS  Q1013`。
 *
 * 没上线（`live` 是 undefined）就只有机场和字母：这个函数读的是 `live.metar`，
 * 而 `live` 只在真正播出时才有（`live.value[callsign]`），不在播就没有 METAR
 * 可读——can-audio 不是这样，它是不管播没播都留着每个席位一份 METAR。这里的
 * 差异是有意的，`docs/manual-test.md` 4.6 记着这条摘要该长什么样。圆点不在
 * 这里，颜色要按状态画，归组件。
 */
export function stationSummary(s: Station, live: Live | undefined): string {
  const marker = s.atis_type === "departure" ? " D" : s.atis_type === "arrival" ? " A" : "";
  const parts = [s.identifier + marker, live?.letter ?? s.letter];
  for (const pattern of [WIND, QNH]) {
    const found = (live?.metar ?? "").match(pattern);
    if (found) parts.push(found[0]);
  }
  return parts.join("  ");
}

/**
 * 一条连接此刻该对人说什么。
 *
 * **在渲染时调**（#29）：Rust 给的是状态码，翻译在这里现翻，切了语言跟着变。
 */
export function stateText(live: Live | undefined): string {
  if (!live || !live.state) return t("state.not_online");
  switch (live.state) {
    case "Online":
      return t("state.online");
    case "Connecting":
      return t("state.connecting");
    case "Reconnecting":
      return t("state.reconnecting");
    case "Stopped":
      return t("state.stopped");
    case "Offline":
      return t("state.offline");
    case "Error":
      return reasonText(live.reason);
  }
}

/** 说得出是哪一条，而不是一句"连接失败"。 */
export function reasonText(reason: Reason | null): string {
  if (!reason) return t("state.reason.unknown");
  if (typeof reason === "string") {
    switch (reason) {
      case "LoginTimeout":
        return t("state.reason.login_timeout");
      case "Closed":
        return t("state.reason.closed");
      case "Dropped":
        return t("state.reason.dropped");
      case "Online":
        return t("state.online");
      case "Stopped":
        return t("state.stopped");
    }
  }
  // 服务端的拒绝理由、底层的网络错误是原样的数据，不翻。
  if ("Rejected" in reason)
    return t("state.reason.rejected", {
      code: reason.Rejected.code,
      message: reason.Rejected.message,
    });
  if ("ConnectFailed" in reason)
    return t("state.reason.connect_failed", { detail: reason.ConnectFailed });
  if ("Callsign" in reason) return t("state.reason.callsign");
  if ("BadFrequency" in reason)
    return t("state.reason.bad_frequency", { frequency: reason.BadFrequency });
  if ("SendFailed" in reason) return t("state.reason.send_failed", { detail: reason.SendFailed });
  if ("Retrying" in reason)
    return t("state.reason.retrying", {
      attempt: reason.Retrying.attempt,
      limit: reason.Retrying.limit,
    });
  if ("GaveUp" in reason) return t("state.reason.gave_up", { limit: reason.GaveUp.limit });
  if ("Connecting" in reason) return t("state.connecting");
  return t("state.reason.unknown");
}

// ——— 从外面取（#44）———

/** 并完之后发生了什么。装的是呼号。和 `netconfig::Merged` 一一对应。 */
export interface Merged {
  added: string[];
  replaced: string[];
  kept: string[];
  /** 正在播出而被跳过的。 */
  skipped: string[];
}

export interface ImportReport {
  /** vATIS 那份配置自己的名字，可能是空的。 */
  source: string;
  merged: Merged;
  /** 没导进来的席位，各自为什么。 */
  failures: Message[];
  /** vATIS 那边有、这里没有对应功能的设置，每一项是它的说明。 */
  skipped: Message[];
}

/** 网络配置和本地那份的差异。**只是给人看的**，并不并要另一个命令。 */
export interface NetworkPreview {
  /** 版本说明。括号怎么写、"未知版本"怎么说归字典，用 `messageText` 翻。 */
  label: Message;
  version: string;
  /** 服务端写的那一行说明，原样显示。 */
  notes: string;
  problems: Message[];
  /** 上一次整份并进来的版本，空的表示从没并过。 */
  previous: string;
  missing: string[];
  differing: string[];
  same: string[];
  /** 有差异、但正在播出的。并的时候会被跳过。 */
  on_air: string[];
}

/** 一串呼号，太长时截断成"前几个等 N 个"。在渲染时调。 */
export function nameList(list: string[], limit: number): string {
  const sep = t("common.separator.list");
  return list.length > limit
    ? t("list.more", { list: list.slice(0, limit).join(sep), count: list.length })
    : list.join(sep);
}

/**
 * 一句话说清楚并了什么。列表太长时截断——全列出来会刷满整块提示。
 *
 * **在渲染时调**（#29）：存下来的是 `Merged`，不是这几行字。
 */
export function describeMerge(m: Merged): string[] {
  const names = (list: string[]) => nameList(list, 8);
  const lines: string[] = [];
  if (m.added.length) {
    lines.push(t("merge.added", { count: m.added.length, list: names(m.added) }));
  }
  if (m.replaced.length) {
    lines.push(t("merge.replaced", { count: m.replaced.length, list: names(m.replaced) }));
  }
  if (m.kept.length) lines.push(t("merge.kept", { count: m.kept.length, list: names(m.kept) }));
  if (m.skipped.length) {
    lines.push(t("merge.skipped", { count: m.skipped.length, list: names(m.skipped) }));
  }
  if (!lines.length) lines.push(t("merge.nothing"));
  return lines;
}

/**
 * 导入 / 取配置之后给人看的结果。
 *
 * **存的是发生了什么，不是那几行字**（#29）：存成句子的话，切了语言那块提示还停在
 * 旧语言上。
 */
export type Notice =
  | { kind: "vatis"; source: string; report: ImportReport }
  | { kind: "online"; merged: Merged }
  | { kind: "network"; merged: Merged };

/** 提示的那几行，第一行是标题。在渲染时调。 */
export function noticeLines(n: Notice): string[] {
  switch (n.kind) {
    case "vatis": {
      const lines = [
        t("import.vatis_heading", { source: n.source }),
        ...describeMerge(n.report.merged),
      ];
      const { failures, skipped } = n.report;
      if (failures.length) {
        // 报前三条。全列出来的话一份坏文件会刷满整块提示。
        lines.push(
          t("import.vatis_failures", {
            count: failures.length,
            list: failures.slice(0, 3).map(errorText).join(t("common.separator.sentence")),
          }),
        );
      }
      if (skipped.length) {
        lines.push(
          t("import.vatis_skipped", {
            list: skipped.map(messageText).join(t("common.separator.list")),
          }),
        );
      }
      return lines;
    }
    case "online":
      return [
        t("import.online_heading"),
        ...describeMerge(n.merged),
        ...(n.merged.added.length ? [t("import.online_defaults")] : []),
      ];
    case "network":
      return [t("import.network_heading"), ...describeMerge(n.merged)];
  }
}
