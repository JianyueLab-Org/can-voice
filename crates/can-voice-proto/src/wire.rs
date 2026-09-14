//! 数据面包头。
//!
//! 跨实现契约：Go 侧（can-voice 服务端）有一份独立实现，两边都测
//! `server/testdata/wire-golden.json`。改这里的布局等于改协议，
//! 必须同时改黄金文件和 Go 侧。
//!
//! ```text
//!  0        1        2        3               5               9              13
//!  +--------+--------+--------+---------------+---------------+--------------+
//!  |  ver   | flags  |  qual  |    seq(2)     |  freq_khz(4)  | speaker(4)   | opus…
//!  +--------+--------+--------+---------------+---------------+--------------+
//! ```
//!
//! **黄金文件只是编码样例，不是契约全文。** 保留位、下行 `qual` 的取值范围、
//! `freq_khz` 的不透明性、`seq` 的每会话语义与 16 位回绕，都写在这里和
//! `server/README.md` 的《数据面包头》一节。样例里的频率不带任何含义，
//! 别从取值上推断什么。

/// 包头字节数。
pub const HEADER_SIZE: usize = 13;

/// 本实现能处理的唯一协议版本。
pub const VERSION: u8 = 1;

/// 一次发言的首帧：收到它就点亮 RX 指示灯并重置抖动缓冲。
///
/// **但它不是"可以开始播放"的判据。接收端从它收到的第一个包起就播，
/// 不管那个包有没有这一位。** 首帧未必是你收到的第一个包，而且那是日常：
///
/// - `SUB` 是全量声明、立即整体替换，所以管制员在别人说到一半时把一个频率
///   加进台面，**下一帧**就投给他——那一帧没有 `FLAG_FIRST`。
/// - 一架飞机**飞进**射程也一样：投递在距离跌破 cutoff 的那一帧就开始。
///
/// 等首帧的实现于是这样坏：飞行员复诵到第 8 秒，管制员把 121.800 加进台面，
/// 他一声不响，直到飞行员下一次按下 PTT——而服务端日志完全正常。
pub const FLAG_FIRST: u8 = 1 << 0;

/// 一次发言的尾帧：收到它就可以**立刻**熄灭 RX 指示灯，不必等静默超时走完
/// ——那会让指示灯在松开 PTT 后多亮半秒。
///
/// **但它是尽力而为的优化，不是熄灯的机制。接收端必须同时跑一个静默超时。**
/// 它坐在不可靠数据报上，会丢；而服务端会在一次发言中途停止向某个听众投递
/// （对方飞出了射程）且**不发任何通知**，那种情况下尾帧**保证**到不了。
/// 只认尾帧的实现会让那盏 RX 灯在这条会话剩下的时间里一直亮着——那正是
/// can-audio 的老毛病，而且从"多亮半秒"升级成了"永久"。
pub const FLAG_LAST: u8 = 1 << 1;

/// `flags` 里今天没有含义的那六位（第 2–7 位）。
///
/// **规则：发送方置零，接收方忽略自己不认识的位，服务端原样转发。**
/// 所以 `parse` 既不校验也不清零这些位——这是一个决定而不是遗漏：
/// 收到就拒会让将来想用第 2 位的客户端必须先等所有服务端升级完；
/// 静默清零则会让那一位悄悄消失，而本协议在别处反复拒绝的正是这种静默。
/// 原样保留意味着将来的一位可以**只升级客户端**就部署。
pub const RESERVED_FLAGS: u8 = 0xFC;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("packet is {0} bytes, need at least {HEADER_SIZE}")]
    TooShort(usize),
    #[error("unknown protocol version {0}, this build speaks {VERSION}")]
    UnknownVersion(u8),
}

/// 数据面包头。
///
/// 每个字段的上下行契约（跨实现的部分）：
///
/// | 字段 | 上行（客户端 → 服务端） | 下行（服务端 → 客户端） |
/// |---|---|---|
/// | `ver` | 必须是 [`VERSION`] | 同左 |
/// | `flags` | bit0 首帧、bit1 尾帧，**bit2–7 置零** | 原样转发，一位都不改 |
/// | `qual` | **必须填 0，服务端完全忽略** | 服务端填，**恒在 1–255** |
/// | `seq` | 每会话的音频帧序号 | 原样转发，服务端从不重编号 |
/// | `freq_khz` | `round(MHz × 1000)` | 实际投递的那个频率（耦合时与上行不同） |
/// | `speaker` | **必须填 0** | 发言者的会话 id |
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Header {
    pub ver: u8,
    pub flags: u8,
    /// 信号质量。
    ///
    /// **下行恒在 1–255，永远不会是 0。** 射程之外服务端**根本不投递**；
    /// 落在衰减带最外侧、四舍五入本来会得到 0 的那一小段被夹到 1。
    /// 所以**不要**给下行的 0 写一条"最弱信号"的处理分支——那种包不存在，
    /// 那条分支永远不会执行，也就永远不会被发现写错了。
    /// 上行相反：客户端填 0，服务端连看都不看。
    pub qual: u8,
    /// 每会话的音频帧序号，**不按发言重置，不说话的时候也不走**。
    ///
    /// 比较用 [`seq_cmp`]，不要用 `<`/`>`。
    pub seq: u16,
    /// **一个不透明的 32 位路由键。** 服务端不做范围校验，也不给任何具体取值
    /// 赋予含义：整个 u32 都能用，包括 0 和 u32::MAX，没有哨兵值。想把频率
    /// 夹在 VHF 波段（118000–136975）里的客户端必须自己夹。
    ///
    /// 相邻项目里确实有约定（can-audio 拿 199998 当"没设频率"的占位值），
    /// 但那是那边的约定，can-voice 不知道它。
    pub freq_khz: u32,
    /// 发言者的会话 id。让接收端能分辨"同一频率上有两个人在讲"
    /// 与"一个人的包乱序了"。抖动缓冲按 `(speaker, freq_khz)` 建。
    pub speaker: u32,
}

impl Header {
    /// 把包头追加到 `out`。
    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.push(self.ver);
        out.push(self.flags);
        out.push(self.qual);
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&self.freq_khz.to_be_bytes());
        out.extend_from_slice(&self.speaker.to_be_bytes());
    }

    /// 解出包头，并返回其后的 Opus 载荷。载荷可以为空 —— 尾帧不必携带音频。
    ///
    /// **不认识的 flags 位不算错误**，见 [`RESERVED_FLAGS`]。
    pub fn parse(b: &[u8]) -> Result<(Header, &[u8]), Error> {
        if b.len() < HEADER_SIZE {
            return Err(Error::TooShort(b.len()));
        }
        let h = Header {
            ver: b[0],
            flags: b[1],
            qual: b[2],
            seq: u16::from_be_bytes([b[3], b[4]]),
            freq_khz: u32::from_be_bytes([b[5], b[6], b[7], b[8]]),
            speaker: u32::from_be_bytes([b[9], b[10], b[11], b[12]]),
        };
        // 版本不认识就拒绝：默默接受等于把未来的布局当成现在的来解，
        // 那会表现为音频乱码而不是一条清晰的错误。
        if h.ver != VERSION {
            return Err(Error::UnknownVersion(h.ver));
        }
        Ok((h, &b[HEADER_SIZE..]))
    }

    /// 这一帧是不是一次发言的开始。**不要拿它当起播判据**，见 [`FLAG_FIRST`]。
    pub fn is_first(&self) -> bool {
        self.flags & FLAG_FIRST != 0
    }

    /// 这一帧是不是一次发言的结束。**它会丢**，见 [`FLAG_LAST`]。
    pub fn is_last(&self) -> bool {
        self.flags & FLAG_LAST != 0
    }
}

/// 两个 `seq` 的先后关系。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqOrder {
    /// `b` 在 `a` 之前（含"差值恰好半个空间"那一侧）。
    Before,
    /// 同一个号。**重复帧**，不是新帧。
    Same,
    /// `b` 在 `a` 之后。
    After,
}

/// 按 16 位回绕算术比较两个 `seq`：返回 `b` 相对 `a` 的位置。
///
/// **这是本仓库唯一的 `seq` 比较实现，也是整个协议里 Rust 第一个落地的部分。**
/// 服务端只转发、从不重排，所以 Go 侧没有对应代码、也没有测试——那段语义在
/// 那边是纯散文。这里写错了没有任何东西会红，钉子只能在这个 crate 里。
///
/// 基础式子是 RFC 1982 的 `(b - a) mod 2¹⁶ < 2¹⁵`，**有两处与 RFC 不一致，
/// 照这里写**：
///
/// 1. **`b == a` 时那个式子给"在后面"**（`0 < 32768` 为真）。这里先判相等返回
///    [`SeqOrder::Same`]，否则一个重复帧会被当成新帧收下。
/// 2. **差值恰好是 2¹⁵ 时 RFC 说"未定义"**，这里定成 [`SeqOrder::Before`]
///    （式子本身就给这个答案），也就是丢弃那一侧。半个序号空间是 10.9 分钟的
///    连续音频，抖动缓冲的深度离它有三个数量级，实际流量里到不了；写死它只是
///    为了两个实现不会在这里分岔。
pub fn seq_cmp(a: u16, b: u16) -> SeqOrder {
    if a == b {
        return SeqOrder::Same;
    }
    if b.wrapping_sub(a) < 0x8000 {
        SeqOrder::After
    } else {
        SeqOrder::Before
    }
}

/// 一次发言之内从 `prev` 到 `next` 少了几帧。相邻（`next == prev + 1`）时为 0。
///
/// **只在 `seq_cmp(prev, next) == After` 时才有意义。** 而且只在**一次发言之内**
/// 才是丢包：跨发言的跳变不是丢包，中间那些帧可能发在别的频率上。
/// 发言之内的空档也不一定是网络丢的——服务端会在一次发言中途停止向某个听众
/// 投递（对方飞出了射程），那在接收端看来和丢包一模一样。两种都只能交给 PLC。
pub fn seq_gap(prev: u16, next: u16) -> u16 {
    next.wrapping_sub(prev).wrapping_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Golden {
        header_size: usize,
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        name: String,
        header: GoldenHeader,
        opus_hex: String,
        encoded_hex: String,
    }

    #[derive(Deserialize)]
    struct GoldenHeader {
        ver: u8,
        flags: u8,
        qual: u8,
        seq: u16,
        freq_khz: u32,
        speaker: u32,
    }

    fn golden() -> Golden {
        // 与 Go 侧同一份文件。改它等于改协议。
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../server/testdata/wire-golden.json");
        let raw = std::fs::read(path)
            .unwrap_or_else(|e| panic!("read {path}: {e} — P2 Task 1 must have produced it"));
        serde_json::from_slice(&raw).expect("parse golden")
    }

    #[test]
    fn header_size_matches_the_golden_file() {
        assert_eq!(golden().header_size, HEADER_SIZE);
    }

    /// 用例数量不写死：黄金文件是会长的（P2 终审就加过一个 qual=1 的样例），
    /// 而写死数量只会让"加了一个用例"表现成一条测试失败。
    #[test]
    fn the_golden_file_has_cases_to_check() {
        assert!(!golden().cases.is_empty(), "a golden file with no cases proves nothing");
    }

    #[test]
    fn encoding_matches_the_golden_file() {
        for c in golden().cases {
            let opus = hex::decode(&c.opus_hex).expect("opus_hex");
            let h = Header {
                ver: c.header.ver,
                flags: c.header.flags,
                qual: c.header.qual,
                seq: c.header.seq,
                freq_khz: c.header.freq_khz,
                speaker: c.header.speaker,
            };
            let mut out = Vec::new();
            h.write_to(&mut out);
            out.extend_from_slice(&opus);
            assert_eq!(hex::encode(&out), c.encoded_hex, "case {:?}", c.name);
        }
    }

    #[test]
    fn parsing_matches_the_golden_file() {
        for c in golden().cases {
            let raw = hex::decode(&c.encoded_hex).expect("encoded_hex");
            let (h, opus) = Header::parse(&raw).expect("parse");
            assert_eq!(h.ver, c.header.ver, "case {:?}", c.name);
            assert_eq!(h.flags, c.header.flags, "case {:?}", c.name);
            assert_eq!(h.qual, c.header.qual, "case {:?}", c.name);
            assert_eq!(h.seq, c.header.seq, "case {:?}", c.name);
            assert_eq!(h.freq_khz, c.header.freq_khz, "case {:?}", c.name);
            assert_eq!(h.speaker, c.header.speaker, "case {:?}", c.name);
            assert_eq!(hex::encode(opus), c.opus_hex, "case {:?}", c.name);
        }
    }

    #[test]
    fn parse_rejects_a_short_packet() {
        assert!(Header::parse(&[0u8; HEADER_SIZE - 1]).is_err());
    }

    #[test]
    fn parse_rejects_an_unknown_version() {
        let mut out = Vec::new();
        Header { ver: 2, ..Default::default() }.write_to(&mut out);
        assert!(
            Header::parse(&out).is_err(),
            "silently accepting an unknown version means decoding a future layout as if it were this one"
        );
    }

    #[test]
    fn a_header_with_no_payload_is_valid() {
        // 尾帧可以不带 Opus 载荷。
        let mut out = Vec::new();
        Header { ver: VERSION, flags: FLAG_LAST, freq_khz: 118_000, ..Default::default() }
            .write_to(&mut out);
        let (_, opus) = Header::parse(&out).expect("parse");
        assert!(opus.is_empty());
    }

    // ——— 保留位（修订件 §八.3）———

    /// 与 Go 侧 `header_test.go` 的那两句对应：flags 的每一位要么有定义、
    /// 要么被保留，且三组互不重叠。少了这两句，把 RESERVED_FLAGS 写成 0xF8
    /// 之类的值不会被任何东西发现。
    #[test]
    fn every_flag_bit_is_either_defined_or_reserved() {
        assert_eq!(
            FLAG_FIRST | FLAG_LAST | RESERVED_FLAGS,
            0xFF,
            "every bit of the flags byte must be either defined or reserved"
        );
        assert_eq!(FLAG_FIRST & FLAG_LAST, 0);
        assert_eq!((FLAG_FIRST | FLAG_LAST) & RESERVED_FLAGS, 0);
    }

    /// **收到不认识的位要忽略，不是拒绝。** 收到就拒的实现会让将来想用第 2 位
    /// 的客户端必须等所有服务端升级完；而服务端是原样转发的，所以这些位真的会
    /// 到达。一个 `if flags & RESERVED_FLAGS != 0 { return Err }` 看起来像在
    /// 收紧协议，实际上是在给未来上锁。
    #[test]
    fn parse_ignores_reserved_flag_bits_rather_than_rejecting_them() {
        let mut out = Vec::new();
        Header {
            ver: VERSION,
            flags: FLAG_FIRST | RESERVED_FLAGS,
            freq_khz: 121_800,
            ..Default::default()
        }
        .write_to(&mut out);
        let (h, _) = Header::parse(&out).expect("reserved bits must not be rejected");
        assert!(h.is_first());
        assert!(!h.is_last());
        assert_eq!(h.flags & RESERVED_FLAGS, RESERVED_FLAGS, "the bits must survive parsing unchanged");
    }

    // ——— seq 的回绕比较（修订件 §八.1）———
    //
    // **Rust 是这一条的第一个实现。** 服务端只转发、从不重排，Go 侧没有任何
    // seq 比较的代码，也没有测试——那段语义在那边是纯散文。写错了没有任何
    // 东西会红，所以钉子必须在这里。

    #[test]
    fn seq_compares_equal_to_itself() {
        // 这是与 RFC 1982 不一致的第一处：`(b-a) mod 2¹⁶ < 2¹⁵` 对 b==a 给
        // "在后面"（0 < 32768 为真）。**先判相等**，否则一个重复帧会被当成新帧收下。
        assert_eq!(seq_cmp(1234, 1234), SeqOrder::Same);
        assert_eq!(seq_cmp(0, 0), SeqOrder::Same);
        assert_eq!(seq_cmp(65535, 65535), SeqOrder::Same);
    }

    #[test]
    fn seq_orders_normally_within_the_window() {
        assert_eq!(seq_cmp(10, 11), SeqOrder::After);
        assert_eq!(seq_cmp(11, 10), SeqOrder::Before);
    }

    #[test]
    fn seq_wraps_around_the_sixteen_bit_space() {
        // 20 毫秒一帧时每 65536 帧、约 21.8 分钟连续发话回绕一次。直接比大小的
        // 实现会在那一刻把整条流卡住，直到序号重新追上来。
        assert_eq!(seq_cmp(65535, 0), SeqOrder::After);
        assert_eq!(seq_cmp(65530, 5), SeqOrder::After);
        assert_eq!(seq_cmp(0, 65535), SeqOrder::Before);
    }

    #[test]
    fn a_difference_of_exactly_half_the_space_is_not_after() {
        // 与 RFC 1982 不一致的第二处：RFC 说"未定义"，这里定成**不在后面**
        // （式子本身就给这个答案），也就是丢弃那一侧。半个序号空间是 10.9 分钟
        // 的连续音频，实际流量里到不了；写死它只是为了两个实现不会在这里分岔。
        assert_eq!(seq_cmp(0, 32768), SeqOrder::Before);
        assert_eq!(seq_cmp(32768, 0), SeqOrder::Before);
        assert_eq!(seq_cmp(1000, 1000u16.wrapping_add(32768)), SeqOrder::Before);
    }

    #[test]
    fn seq_distance_counts_the_gap_inside_one_transmission() {
        // 发言之内少 n 个号就是丢了 n 帧。跨发言的跳变不是丢包——中间那些帧
        // 可能发在别的频率上。
        assert_eq!(seq_gap(10, 11), 0);
        assert_eq!(seq_gap(10, 14), 3);
        assert_eq!(seq_gap(65534, 1), 2);
    }
}
