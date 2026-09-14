//! 控制面消息：长度前缀 JSON 帧。
//!
//! 控制面刻意用 JSON 而不是 protobuf：消息频率极低（登录、订阅变更、偶发通知），
//! 可读性和排障便利压过字节效率。高频的音频走 `crate::wire` 的紧凑二进制。
//!
//! **这是跨实现契约，权威在 Go 侧的 `server/internal/control/message.go`。**
//! 字段名、`omitempty` 的有无、以及 `PROTO_VERSION` 的字面值都要照那边写。

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// 一个控制帧的上限。SUB 会携带订阅列表，64 KB 留了充足余量。
///
/// 服务端用的是同一个数字，而且它是承重的：一份大到超过这里的 SUBACK
/// 会以 `ack_undeliverable` 关掉连接（关闭码 3），**而那份 SUB 已经生效了**。
pub const MAX_FRAME: usize = 64 * 1024;

/// `HELLO.proto` 唯一被接受的值，等于服务端的 `control.ProtoVersion`。
///
/// 这是**字面值契约**而不是一个可以跟着服务端走的变量：服务端握手那道闸判的是
/// `h.Proto != control.ProtoVersion`，填错会以关闭码 1 + 原因串
/// `proto_unsupported` 被拒——而那一条要对用户说的是"请更新客户端"，
/// 不是"被拒绝"（见 P3 修订件 N2）。
pub const PROTO_VERSION: u32 = 1;

/// NOTICE 的 `kind` 取值，照抄服务端的常量。
pub mod notice_kind {
    /// 你往一个没有发射权的频率上发了音频。
    pub const TX_DENIED: &str = "tx_denied";
    /// 位置快照不可用，射程过滤已降级。
    pub const RANGE_UNAVAILABLE: &str = "range_unavailable";
    /// 声明里有东西没被接受。
    pub const SUB_REJECTED: &str = "sub_rejected";
    /// 你发来的那一帧服务端解不开。
    pub const UNKNOWN_MESSAGE: &str = "unknown_message";
}

/// 把 JSON 的 `null` 当成缺省值读。
///
/// **`#[serde(default)]` 一个人不够用,它只管"键缺席"。** Go 把 nil slice 编码成
/// `null`——键**在**,值是 `null`——而 serde 见到 `null` 要一个 sequence 时是硬报错,
/// 不是回退到 default。少了这一层,一份带 `"rejected_xc": null` 的 SUBACK 会解析
/// 失败,被上层读成"控制流结束",于是重连风暴。
///
/// 今天服务端其实不会发 `null`:`router.go` 的两个 SubAck 构造点都把四个切片
/// 显式初始化成非 nil 的空切片。但**Go 侧没有任何测试钉住这一点**,所以那是一个
/// 没人守着的实现细节,而每一个客户端都压在它上面。契约取宽的那一侧:
/// 服务端发 `[]` 或 `null` 都对,客户端两种都得读得进来。
fn null_as_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("control frame of {0} bytes exceeds the {MAX_FRAME} byte limit")]
    TooLarge(usize),
    #[error("control frame is not valid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unknown control message type {0:?}")]
    UnknownType(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hello {
    pub token: String,
    pub client: String,
    pub proto: u32,
    /// 只有观察员模式填：观察员没有 FSD 连接，位置取自它跟随的那架飞机。
    ///
    /// 服务端会校验它是不是一个合法呼号（`isValidCallsign`，照抄 can-fsd：
    /// 2–10 个字符，只许 `A-Z` `0-9` `-` `_`），不合规则的直接以 `refused` 拒掉。
    /// 所以空串不上线，非空的要在发出去**之前**自己判一次（修订件 N4）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub follow: String,
}

// 这里**没有** `transport` 字段，而且不是漏了（修订件 M1）。服务端的
// `control.Hello` 没有它，P2 Task 12（stream 回退）也因 P1 探针未部署而未做。
// 一个宣称了未构建行为的字段比没有这个字段更糟：真设成 "stream" 时服务端会
// 忽略它，客户端却以为自己走了回退通道。等 Task 12 真做了再加。

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ready {
    pub session: u32,
    pub server: String,
    pub max_tx: u32,
    pub max_rx: u32,
}

/// **全量**收发声明，不是增量。
///
/// 服务端收到即整体替换该会话的订阅集合。这是消除 sync 风暴的根本机制：
/// 幂等，没有"这次是加还是减"的状态推导，重连后重发一次即恢复。
/// 刻意没有 add/remove 字段 —— 任何增量语义都会把那一类 bug 请回来。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Sub {
    #[serde(default, deserialize_with = "null_as_default")]
    pub rx: Vec<u32>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub tx: Vec<u32>,
    /// 交叉耦合对。**声明了一对就必须真的在两个频率上各发一份数据报**——
    /// 服务端检查不了这件事，只发一份的客户端在另一个频率上完全静默，
    /// 而两端日志都正常（修订件 §九）。
    #[serde(default, deserialize_with = "null_as_default")]
    pub xc: Vec<[u32; 2]>,
}

/// 服务端对一份 `Sub` 的回报。
///
/// **对账只有一条规则，用差集，不要读 `rejected` 去推断方向：**
///
/// ```text
/// 被拒的 TX = 我声明的 tx − ack.tx
/// 被拒的 RX = 我声明的 rx − ack.rx
/// ```
///
/// `rx`/`tx` 是完整且权威的（长度受 `max_rx`/`max_tx` 约束，不存在截断），
/// 所以差集在任何情况下都对；而 `rejected` 有上界，截断之后基于它的推断会失效。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubAck {
    #[serde(default, deserialize_with = "null_as_default")]
    pub rx: Vec<u32>,
    #[serde(default, deserialize_with = "null_as_default")]
    pub tx: Vec<u32>,
    /// 便利字段，不是权威记录。一个频率同时出现在 `rx` 和 `rejected` 里是
    /// 正常的，意思是"TX 被限额拒了，但 RX 给了"。
    #[serde(default, deserialize_with = "null_as_default")]
    pub rejected: Vec<u32>,
    /// 没有生效的交叉耦合对。**它和 `rejected` 不重叠，差集公式也管不到它**
    /// ——耦合对不在 `rx`/`tx` 里，所以少了这个字段就等于把服务端专门发来的
    /// 拒绝理由丢掉（修订件 C1）。
    ///
    /// 服务端这个字段**没有** `omitempty`，所以 `null` 是常态：`#[serde(default)]`
    /// 是承重的，不是保险。
    #[serde(default, deserialize_with = "null_as_default")]
    pub rejected_xc: Vec<[u32; 2]>,
    /// `rejected` 这张单子本身不全。
    ///
    /// 服务端这个字段**带** `omitempty`，所以常规 SUBACK 里根本不出现——
    /// 少了 `#[serde(default)]` 不是"读成 false"，而是**每一份正常的 SUBACK
    /// 都解析失败**，读成控制流结束，于是重连风暴（修订件 N1）。
    ///
    /// 为 true 时不要再读 `rejected`，改用上面的差集。
    #[serde(default)]
    pub rejected_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Notice {
    pub kind: String,
    #[serde(default)]
    pub freq: u32,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ping {
    pub t: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Pong {
    pub t: i64,
    pub server_t: i64,
}

/// 服务端的告别。
///
/// **它会丢**：一个还没开始读控制流的客户端收不到它。真正丢不掉的是 QUIC 的
/// 关闭码与原因串（`ApplicationError`），那两个是原子地一起送达的。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bye {
    pub reason: String,
}

/// 控制面消息。判别字段是 JSON 里的 `type`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Hello(Hello),
    Ready(Ready),
    Sub(Sub),
    SubAck(SubAck),
    Notice(Notice),
    Ping(Ping),
    Pong(Pong),
    Bye(Bye),
}

impl Message {
    /// 按 `type` 字段分发。未知类型直接拒绝，不静默忽略 ——
    /// 静默忽略会让一个拼错的类型表现为"消息发出去了但什么都没发生"。
    pub fn decode(b: &[u8]) -> Result<Message> {
        #[derive(Deserialize)]
        struct Probe {
            #[serde(rename = "type")]
            ty: String,
        }
        let probe: Probe = serde_json::from_slice(b)?;
        Ok(match probe.ty.as_str() {
            "HELLO" => Message::Hello(serde_json::from_slice(b)?),
            "READY" => Message::Ready(serde_json::from_slice(b)?),
            "SUB" => Message::Sub(serde_json::from_slice(b)?),
            "SUBACK" => Message::SubAck(serde_json::from_slice(b)?),
            "NOTICE" => Message::Notice(serde_json::from_slice(b)?),
            "PING" => Message::Ping(serde_json::from_slice(b)?),
            "PONG" => Message::Pong(serde_json::from_slice(b)?),
            "BYE" => Message::Bye(serde_json::from_slice(b)?),
            other => return Err(Error::UnknownType(other.to_string())),
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        // serde 的 tagged enum 在这里不好用：每个变体的字段是平铺的，
        // 手写一次比为八个类型各加一个 #[serde(tag)] 属性更直白。
        fn tag<T: Serialize>(ty: &str, v: &T) -> Result<Vec<u8>> {
            let mut val = serde_json::to_value(v)?;
            if let serde_json::Value::Object(ref mut m) = val {
                m.insert("type".into(), serde_json::Value::String(ty.into()));
            }
            Ok(serde_json::to_vec(&val)?)
        }
        match self {
            Message::Hello(v) => tag("HELLO", v),
            Message::Ready(v) => tag("READY", v),
            Message::Sub(v) => tag("SUB", v),
            Message::SubAck(v) => tag("SUBACK", v),
            Message::Notice(v) => tag("NOTICE", v),
            Message::Ping(v) => tag("PING", v),
            Message::Pong(v) => tag("PONG", v),
            Message::Bye(v) => tag("BYE", v),
        }
    }
}

/// 写一个 4 字节大端长度前缀加载荷。
pub fn write_frame<W: Write>(w: &mut W, b: &[u8]) -> Result<()> {
    if b.len() > MAX_FRAME {
        return Err(Error::TooLarge(b.len()));
    }
    w.write_all(&(b.len() as u32).to_be_bytes())?;
    w.write_all(b)?;
    Ok(())
}

/// 读一个长度前缀帧。长度检查在分配之前。
///
/// **只有一处长度检查，而且被测的就是上线的那一份。** 异步那一侧（`conn.rs`）
/// 不得自己重写一遍这段算术，否则两份实现一旦分叉就是协议错位且没有测试能抓到
/// （修订件 M3）。
pub fn read_frame<R: Read>(r: &mut R) -> Result<Vec<u8>> {
    let mut hdr = [0u8; 4];
    r.read_exact(&mut hdr)?;
    let n = check_frame_len(u32::from_be_bytes(hdr))?;
    let mut b = vec![0u8; n];
    r.read_exact(&mut b)?;
    Ok(b)
}

/// 长度前缀的上限检查，同步与异步两条读路径共用。
pub fn check_frame_len(n: u32) -> Result<usize> {
    let n = n as usize;
    if n > MAX_FRAME {
        return Err(Error::TooLarge(n));
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_dispatches_on_the_type_field() {
        let raw = br#"{"type":"HELLO","token":"abc","client":"can-controller/3.0.0","proto":1}"#;
        match Message::decode(raw).expect("decode") {
            Message::Hello(h) => {
                assert_eq!(h.token, "abc");
                assert_eq!(h.proto, 1);
            }
            other => panic!("decoded {other:?}, want Hello"),
        }
    }

    #[test]
    fn decode_rejects_an_unknown_type() {
        assert!(Message::decode(br#"{"type":"NOPE"}"#).is_err());
    }

    #[test]
    fn sub_is_a_full_declaration_with_no_delta_fields() {
        let raw = br#"{"type":"SUB","rx":[118000,121800],"tx":[121800],"xc":[[121800,124550]]}"#;
        match Message::decode(raw).expect("decode") {
            Message::Sub(s) => {
                assert_eq!(s.rx, vec![118000, 121800]);
                assert_eq!(s.tx, vec![121800]);
                assert_eq!(s.xc, vec![[121800, 124550]]);
            }
            other => panic!("decoded {other:?}, want Sub"),
        }
    }

    #[test]
    fn encode_round_trips_through_decode() {
        let sub = Message::Sub(Sub {
            rx: vec![118000, 121800],
            tx: vec![121800],
            xc: vec![[121800, 124550]],
        });
        let bytes = sub.encode().expect("encode");
        let back = Message::decode(&bytes).expect("decode");
        assert_eq!(back, sub);
    }

    #[test]
    fn encode_emits_the_type_discriminator() {
        let bytes = Message::Ping(Ping { t: 42 }).encode().expect("encode");
        let text = String::from_utf8(bytes).expect("utf8");
        assert!(text.contains(r#""type":"PING""#), "encoded as {text}");
    }

    #[test]
    fn read_frame_rejects_an_oversized_length_prefix() {
        // 一个恶意的长度前缀不能让客户端去分配 4 GB。
        let mut cursor = std::io::Cursor::new(vec![0xff, 0xff, 0xff, 0xff]);
        assert!(read_frame(&mut cursor).is_err());
    }

    #[test]
    fn frames_round_trip() {
        let mut buf = Vec::new();
        write_frame(&mut buf, br#"{"type":"PING","t":1}"#).expect("write");
        let mut cursor = std::io::Cursor::new(buf);
        let got = read_frame(&mut cursor).expect("read");
        assert_eq!(&got, br#"{"type":"PING","t":1}"#);
    }

    // ——— 以下钉住 P3 修订件的裁定（C1 / N1 / M1 / M2）———

    /// M2：Go 把 nil slice 编码成 `null`，而 `rejected_xc` 在服务端**没有**
    /// `omitempty`（`server/internal/control/message.go`），所以
    /// `"rejected_xc": null` 是常态而不是边角。少一个 `#[serde(default)]`，
    /// **每一份 SUBACK 都解不出来**，pump 会把它读成"控制流结束"→ 重连风暴。
    #[test]
    fn a_suback_whose_vectors_are_null_still_decodes() {
        let raw = br#"{"type":"SUBACK","rx":null,"tx":null,"rejected":null,"rejected_xc":null}"#;
        match Message::decode(raw).expect("a null vector must decode as empty, not fail") {
            Message::SubAck(a) => {
                assert!(a.rx.is_empty());
                assert!(a.tx.is_empty());
                assert!(a.rejected.is_empty());
                assert!(a.rejected_xc.is_empty());
            }
            other => panic!("decoded {other:?}, want SubAck"),
        }
    }

    /// C1：交叉耦合被拒必须到得了客户端。serde 默认忽略未知键，所以少了这个
    /// 字段的客户端会把服务端专门发来的拒绝理由丢掉。
    #[test]
    fn a_suback_carries_the_cross_couple_rejections() {
        let raw = br#"{"type":"SUBACK","rx":[121800],"tx":[121800],"rejected":[],"rejected_xc":[[121800,124550]]}"#;
        match Message::decode(raw).expect("decode") {
            Message::SubAck(a) => assert_eq!(a.rejected_xc, vec![[121800, 124550]]),
            other => panic!("decoded {other:?}, want SubAck"),
        }
    }

    /// N1：`rejected_truncated` 在服务端带 `omitempty`，所以常规 SUBACK 里
    /// **根本不出现**。少了 default 不是"读成 null"，而是每一份正常的 SUBACK
    /// 都解析失败。
    #[test]
    fn a_normal_suback_omits_the_truncation_flag_and_still_decodes() {
        let raw = br#"{"type":"SUBACK","rx":[118000],"tx":[],"rejected":[],"rejected_xc":null}"#;
        match Message::decode(raw).expect("the flag is omitempty; its absence must decode") {
            Message::SubAck(a) => assert!(!a.rejected_truncated),
            other => panic!("decoded {other:?}, want SubAck"),
        }
    }

    /// N1 的另一半：真截断了要读得到。为 true 时客户端不能再信 `rejected`，
    /// 要改用 H2 的差集公式。
    #[test]
    fn a_truncated_suback_says_so() {
        let raw = br#"{"type":"SUBACK","rx":[118000],"tx":[],"rejected":[121800],"rejected_xc":null,"rejected_truncated":true}"#;
        match Message::decode(raw).expect("decode") {
            Message::SubAck(a) => assert!(a.rejected_truncated),
            other => panic!("decoded {other:?}, want SubAck"),
        }
    }

    /// M1：`Hello` 不得有 `transport` 字段。服务端（`control.Hello`）没有它，
    /// Task 12 也没做——一个宣称了未构建行为的字段比没有这个字段更糟：
    /// 真设成 "stream" 时服务端会忽略它，客户端却以为自己走了回退通道。
    #[test]
    fn hello_declares_no_transport_field() {
        let bytes = Message::Hello(Hello {
            token: "t".into(),
            client: "can-controller/3.0.0".into(),
            proto: PROTO_VERSION,
            follow: String::new(),
        })
        .encode()
        .expect("encode");
        let text = String::from_utf8(bytes).expect("utf8");
        assert!(
            !text.contains("transport"),
            "HELLO must not claim a transport it does not have; encoded as {text}"
        );
    }

    /// 空的 `follow` 不上线：只有观察员模式填它，而服务端会校验它是不是一个
    /// 合法呼号（N4），所以发一个空串过去等于自找一条 `refused`。
    #[test]
    fn an_empty_follow_is_not_put_on_the_wire() {
        let bytes = Message::Hello(Hello {
            token: "t".into(),
            client: "c".into(),
            proto: PROTO_VERSION,
            follow: String::new(),
        })
        .encode()
        .expect("encode");
        let text = String::from_utf8(bytes).expect("utf8");
        assert!(!text.contains("follow"), "encoded as {text}");
    }

    /// `PROTO_VERSION` 是**字面值契约**：服务端判的是
    /// `h.Proto != control.ProtoVersion`，两边一起变就还是绿的。
    #[test]
    fn the_proto_version_is_the_literal_value_the_protocol_names() {
        assert_eq!(PROTO_VERSION, 1);
    }
}
