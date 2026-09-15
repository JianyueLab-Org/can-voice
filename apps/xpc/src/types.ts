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
  voice: { link: string } | null;
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
