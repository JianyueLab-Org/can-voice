//! 客户端和 X-Plane 插件之间的本地通道。
//!
//! # 插件为什么要单独一个进程
//!
//! X-Plane 的绘图 API 只有插件够得着，而插件跑在 X-Plane 自己的 Python 里
//! （XPPython3），装的包和客户端那套不是一回事。所以分工是：
//!
//! ```text
//! 客户端  收 FSD、算插值、匹配模型      ← 复杂的部分，在这里能测
//! 插件    照着给的数据画 + 写 TCAS      ← 薄，改动少，不用反复重启 X-Plane
//! ```
//!
//! 协议是**每个 UDP 包一行 JSON**，发到本机。选 UDP 不选 TCP 是因为这是纯位置
//! 流：丢一帧下一帧（200 ms 后）就补上了，为了可靠性去排队反而会积压出延迟。
//! 插件那边永远只认最后收到的一帧。
//!
//! # 分片按**字节**切，负载走 base64
//!
//! 64 架飞机的 JSON 能到十几 KB，本机回环 MTU 通常够，但不能赌。
//!
//! v1 是切完字节再按 UTF-8 解回字符串，切口落在多字节字符中间（呼号、CSL 路径
//! 里的中文）时直接编码失败——**从那一帧起插件再也收不到任何数据，整个天空
//! 清空**。按字节切、base64 装进 JSON 之后，切口可以落在任何位置。

use base64::Engine as _;
use can_voice_proto::wire::{seq_cmp, SeqOrder};
use std::collections::HashMap;

/// 客户端 → 插件。49900 往上是 X-Plane 自己不用的区间。
pub const PLUGIN_PORT: u16 = 49900;
/// 插件 → 客户端（握手和状态回报）。
pub const CLIENT_PORT: u16 = 49901;
pub const HOST: &str = "127.0.0.1";

/// 一个 UDP 包里放多少字节的负载。本机回环能扛更大，但 8 KB 是安全线。
pub const MAX_PAYLOAD: usize = 8000;

/// v2：分片按字节切并 base64。见模块头。
pub const PROTOCOL_VERSION: u32 = 2;

#[derive(serde::Serialize, serde::Deserialize)]
struct Frame {
    v: u32,
    seq: u16,
    part: usize,
    total: usize,
    data: String,
}

/// 插件回报的状态。
///
/// **这条通道以前只有一个常量。** `CLIENT_PORT` 定义在那儿、一个字节没走过，
/// 于是没装插件的人"能连能说、天上是空的"，而界面上 X-Plane 那盏灯还是绿的
/// ——它代表的是 UDP 数据源，不是插件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Status {
    /// 插件那一侧的协议版本。
    pub v: u32,
    /// 它此刻画着几架。
    pub drawn: usize,
}

/// 解析一条状态包。不是状态包就返回 `None`——这个口上什么都可能进来。
pub fn decode_status(packet: &[u8]) -> Option<Status> {
    let v: serde_json::Value = serde_json::from_slice(packet).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("status") {
        return None;
    }
    serde_json::from_value(v).ok()
}

/// 插件的协议版本和我们对不对得上。
///
/// **对不上时插件静默丢弃每一帧**（它自己那句 `header.get("v") != PROTOCOL_VERSION`），
/// 症状是"完全没有交通"而两边日志都干净。这是最难查的一种，所以要说出来。
pub fn version_matches(s: &Status) -> bool {
    s.v == PROTOCOL_VERSION
}

/// 一条给客户端的状态包。插件那边也要发同样形状的。
pub fn encode_status(drawn: usize) -> Vec<u8> {
    serde_json::json!({ "type": "status", "v": PROTOCOL_VERSION, "drawn": drawn })
        .to_string()
        .into_bytes()
}

/// 把一条消息切成若干个待发的 UDP 包。
pub fn encode(message: &serde_json::Value, sequence: u16) -> Vec<Vec<u8>> {
    encode_with(message, sequence, MAX_PAYLOAD)
}

pub fn encode_with(message: &serde_json::Value, sequence: u16, max_payload: usize) -> Vec<Vec<u8>> {
    let raw = serde_json::to_vec(message).unwrap_or_else(|_| b"null".to_vec());
    // base64 会膨胀 4/3，切片尺寸要按**编码后**不超过 max_payload 算。
    let step = (max_payload * 3 / 4).max(1);
    let chunks: Vec<&[u8]> = if raw.is_empty() {
        vec![&[]]
    } else {
        raw.chunks(step).collect()
    };
    let total = chunks.len();
    chunks
        .into_iter()
        .enumerate()
        .map(|(part, chunk)| {
            let frame = Frame {
                v: PROTOCOL_VERSION,
                seq: sequence,
                part,
                total,
                data: base64::engine::general_purpose::STANDARD.encode(chunk),
            };
            serde_json::to_vec(&frame).unwrap_or_default()
        })
        .collect()
}

/// 插件侧：把分片拼回完整消息。
///
/// **只保留最新的那一帧。** 旧帧收不齐就直接扔——位置流里迟到的数据没有价值，
/// 留着反而会让飞机往回跳。
#[derive(Debug, Default)]
pub struct Reassembler {
    sequence: Option<u16>,
    parts: HashMap<usize, Vec<u8>>,
    total: usize,
}

impl Reassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂一个 UDP 包。拼齐了返回消息，否则 `None`。
    pub fn feed(&mut self, packet: &[u8]) -> Option<serde_json::Value> {
        let frame: Frame = serde_json::from_slice(packet).ok()?;
        if frame.v != PROTOCOL_VERSION {
            return None;
        }

        if Some(frame.seq) != self.sequence {
            // **序号是 16 位环回的，只有往前走才算新帧。** 乱序迟到的旧分片
            // 会顶掉刚拼好的新帧，飞机往回跳——正是这套设计要防的事。
            // 比较用的是线协议那一份 `seq_cmp`，不在这儿再写一遍环回逻辑。
            if let Some(current) = self.sequence {
                if seq_cmp(current, frame.seq) != SeqOrder::After {
                    return None;
                }
            }
            self.sequence = Some(frame.seq);
            self.parts.clear();
            self.total = frame.total;
        }

        let chunk = base64::engine::general_purpose::STANDARD
            .decode(frame.data.as_bytes())
            .ok()?;
        self.parts.insert(frame.part, chunk);
        if self.parts.len() != self.total {
            return None;
        }

        let mut raw = Vec::new();
        for part in 0..self.total {
            raw.extend_from_slice(self.parts.get(&part)?);
        }
        self.parts.clear();
        serde_json::from_slice(&raw).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ——— 插件回报 ———

    /// **插件要能被看见。** 在这之前这条通道只有一个常量（`CLIENT_PORT`），
    /// 一个字节都没走过：没装插件的人"能连能说、天上是空的"，而界面上
    /// X-Plane 那盏灯还是绿的——它只代表 UDP 数据源，不代表插件。
    #[test]
    fn a_status_packet_from_the_plugin_is_understood() {
        let raw = br#"{"type":"status","v":2,"drawn":7}"#;
        let s = decode_status(raw).expect("status");
        assert_eq!(s.v, 2);
        assert_eq!(s.drawn, 7);
    }

    /// 别的包不是状态包。位置流是客户端发给插件的，方向相反，
    /// 但这个口上什么都可能进来。
    #[test]
    fn anything_that_is_not_a_status_packet_is_ignored() {
        assert!(decode_status(br#"{"type":"traffic"}"#).is_none());
        assert!(decode_status(b"not json").is_none());
        assert!(decode_status(b"").is_none());
    }

    /// **版本对不上要说出来。** 对不上时插件静默丢弃每一帧，症状是
    /// "完全没有交通"而两边日志都干净——这正是最难查的那一种。
    #[test]
    fn a_version_mismatch_is_visible_rather_than_silent() {
        assert!(version_matches(&Status {
            v: PROTOCOL_VERSION,
            drawn: 0
        }));
        assert!(!version_matches(&Status {
            v: PROTOCOL_VERSION + 1,
            drawn: 0
        }));
    }

    use serde_json::json;

    fn round_trip(message: &serde_json::Value, max_payload: usize) -> Option<serde_json::Value> {
        let mut r = Reassembler::new();
        let packets = encode_with(message, 1, max_payload);
        let mut last = None;
        for p in packets {
            last = r.feed(&p);
        }
        last
    }

    #[test]
    fn a_small_message_goes_in_one_packet() {
        let m = json!({"traffic": []});
        assert_eq!(encode(&m, 0).len(), 1);
        assert_eq!(round_trip(&m, MAX_PAYLOAD), Some(m));
    }

    /// **切口可以落在多字节字符中间。**
    ///
    /// v1 是切完字节再按 UTF-8 解回字符串，呼号和 CSL 路径里的中文一旦被切开
    /// 就编码失败——从那一帧起插件再也收不到任何数据，整个天空清空。
    #[test]
    fn a_cut_through_a_multibyte_character_survives() {
        let m = json!({"csl": "中文机型路径".repeat(50), "callsign": "国航一零一"});
        // 故意切得很碎，保证切口落在汉字中间。
        let packets = encode_with(&m, 1, 8);
        assert!(packets.len() > 10, "{}", packets.len());
        assert_eq!(round_trip(&m, 8), Some(m));
    }

    #[test]
    fn a_big_message_is_split_and_put_back_together() {
        let traffic: Vec<serde_json::Value> = (0..64)
            .map(|i| json!({"callsign": format!("CES{i:04}"), "lat": 31.0, "lon": 121.0}))
            .collect();
        let m = json!({"traffic": traffic});
        assert!(encode_with(&m, 1, 512).len() > 1);
        assert_eq!(round_trip(&m, 512), Some(m));
    }

    /// 每个包都不超过负载上限——base64 的 4/3 膨胀要算进去。
    #[test]
    fn no_packet_exceeds_the_payload_limit_after_base64() {
        let m = json!({"x": "a".repeat(100_000)});
        for p in encode_with(&m, 1, 1024) {
            // JSON 头本身有几十字节，所以放宽一点；关键是 base64 之后的负载
            // 没有超过 step 换算回来的上限。
            assert!(p.len() <= 1024 + 128, "packet is {} bytes", p.len());
        }
    }

    /// **迟到的旧分片不能顶掉刚拼好的新帧。**
    ///
    /// 不判方向的话飞机会往回跳，而那正是"只保留最新一帧"要防的事。
    #[test]
    fn a_late_fragment_from_an_older_frame_is_dropped() {
        let mut r = Reassembler::new();
        let old = encode_with(&json!({"n": 1}), 5, 8);
        let new = encode_with(&json!({"n": 2}), 6, 8);
        assert!(old.len() > 1 && new.len() > 1, "要多片才测得到这件事");

        for p in &new {
            r.feed(p);
        }
        // 新帧已经拼好了；现在来一片旧的。
        assert_eq!(r.feed(&old[0]), None);
        // 再喂新帧的一片，应当还是接着新帧走，而不是从旧帧重来。
        assert_eq!(r.sequence, Some(6));
    }

    /// 序号是 16 位环回的：65535 之后的 0 是**新**帧，不是旧帧。
    #[test]
    fn the_sequence_wraps_without_stalling() {
        let mut r = Reassembler::new();
        assert_eq!(
            r.feed(&encode_with(&json!({"n": 1}), 65_535, MAX_PAYLOAD)[0]),
            Some(json!({"n": 1}))
        );
        assert_eq!(
            r.feed(&encode_with(&json!({"n": 2}), 0, MAX_PAYLOAD)[0]),
            Some(json!({"n": 2}))
        );
    }

    #[test]
    fn a_frame_from_another_protocol_version_is_ignored() {
        let mut r = Reassembler::new();
        let bad = br#"{"v":1,"seq":0,"part":0,"total":1,"data":"e30="}"#;
        assert_eq!(r.feed(bad), None);
        assert_eq!(r.feed(b"not json"), None);
    }

    /// 收不齐就不交货——半帧数据画出来是一半飞机凭空消失。
    #[test]
    fn an_incomplete_frame_yields_nothing() {
        let mut r = Reassembler::new();
        let packets = encode_with(&json!({"x": "y".repeat(1000)}), 1, 64);
        assert!(packets.len() > 2);
        for p in &packets[..packets.len() - 1] {
            assert_eq!(r.feed(p), None);
        }
    }
}
