//! 文字消息记录：飞行员客户端收发过的 `#TM`。
//!
//! 攒在这里而不是靠事件流推给界面，理由和语音快照那边一样：事件是广播，
//! 窗口重开之前发生的事收不到，而"管制员刚才说了什么"正是最不能丢的一类。

use std::collections::VecDeque;

/// 最多留这么多条。见 `the_log_is_bounded_and_drops_the_oldest`。
pub const MAX_MESSAGES: usize = 200;

/// 一条文字消息。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ChatMessage {
    /// 发件人呼号。自己发出去的那条填自己的呼号。
    pub from: String,
    /// 收件人：一个呼号，或者 `@<五位频率>`（发到频率上的），或者 `*` / `*S`。
    pub to: String,
    pub text: String,
    /// 自己发出去的吗。界面靠它决定贴哪一边。
    pub outbound: bool,
    /// 单调秒。**由调用方给**，这一层不读时钟——不然就没法测。
    pub at: f64,
}

/// 一条真要发出去的消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    /// `#TM` 的收件人：一个呼号，`@` 加五位频率，或者 `*S`。
    pub to: String,
    pub text: String,
}

/// 算出一条消息实际发给谁、正文是什么；发不出去时是 `None`。
///
/// 三条规则合在一处，因为它们互相压着：
///
/// 1. **点命令压过收件人框**（`.wallop` → 督导）。`can-audio` 的飞行员端原来
///    没有这一步，用户打的 `.wallop 求助` 被当成正文发到了频率上。
/// 2. **收件人留空发到 COM1 频率**（`@` 加五位，去掉开头的 1 和小数点）。
///    界面上那个输入框的提示语就是这么承诺的。
/// 3. 正文为空、或者既没收件人也没有 COM1 时**不发**——服务端会把这样的包
///    丢掉，而界面会回一行"已发送"。
///
/// 算出来的结果可以原样交给 `PilotHandle::send_text`：它自己也会跑一次点命令
/// 解析，而一条已经翻好的消息不以点开头，跑第二次得到的是同一个答案。
pub fn outgoing(
    typed_recipient: &str,
    typed_message: &str,
    com1_khz: Option<u32>,
) -> Option<Outgoing> {
    let (dot, body) = can_voice_fsd::pilot::parse_dot_command(typed_message);
    if body.is_empty() {
        return None;
    }
    let to = match dot {
        Some(fixed) => fixed.to_string(),
        None => {
            let typed = typed_recipient.trim();
            if typed.is_empty() {
                // `@` 后面是五位电台频率：去掉开头的 1 和小数点。
                format!("@{:05}", com1_khz? % 100_000)
            } else {
                typed.to_uppercase()
            }
        }
    };
    Some(Outgoing { to, text: body })
}

/// 收发过的消息，最新的在最后。
#[derive(Debug, Default)]
pub struct ChatLog {
    entries: VecDeque<ChatMessage>,
}

impl ChatLog {
    /// 记一条。超出上界时从最旧的那头丢。
    pub fn record(&mut self, m: ChatMessage) {
        self.entries.push_back(m);
        while self.entries.len() > MAX_MESSAGES {
            self.entries.pop_front();
        }
    }

    /// 全部消息，最旧的在前。
    pub fn snapshot(&self) -> Vec<ChatMessage> {
        self.entries.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(text: &str) -> ChatMessage {
        ChatMessage {
            from: "ZSPD_TWR".into(),
            to: "CCA1501".into(),
            text: text.into(),
            outbound: false,
            at: 0.0,
        }
    }

    /// 点命令压过收件人框。`.wallop` 就是发给督导的，框里写着什么都不该改变
    /// 这一点——翻错的表现是求助发到了频率上，督导一个字都收不到，而界面
    /// 还回一行"已发送"。
    #[test]
    fn a_dot_command_wins_over_the_recipient_box() {
        let got = outgoing("ZSPD_TWR", ".wallop 有人抢频率", Some(121_800)).expect("sendable");
        assert_eq!(got.to, "*S");
        assert_eq!(got.text, "有人抢频率");
    }

    /// 收件人留空就发到 COM1 频率上。`@` 后面是五位，去掉开头的 1 和小数点。
    #[test]
    fn an_empty_recipient_goes_to_the_com1_frequency() {
        let got = outgoing("", "request pushback", Some(128_750)).expect("sendable");
        assert_eq!(got.to, "@28750");
        // 118.350 的那个 0 不能被吃掉：五位是定长。
        assert_eq!(
            outgoing("", "hi", Some(118_350)).expect("sendable").to,
            "@18350"
        );
    }

    /// 既没填收件人、COM1 也没有频率时，说不出该发给谁。
    ///
    /// **不能默默发到一个编出来的地址上**：`#TM呼号::正文` 会被服务端按
    /// "收件人是空串"处理，谁也收不到，而界面看起来一切正常。
    #[test]
    fn with_no_recipient_and_no_com1_there_is_nothing_to_send() {
        assert!(outgoing("", "anybody there", None).is_none());
    }

    /// 正文是空的就别发。服务端那边 `text_message` 也会把它丢掉，
    /// 记进聊天区的话就成了一条"发出去过"的幻影。
    #[test]
    fn an_empty_body_is_not_sent() {
        assert!(outgoing("ZSPD_TWR", "   ", Some(121_800)).is_none());
        // 只有一个点命令、正文为空的也一样。
        assert!(outgoing("", ".wallop", Some(121_800)).is_none());
    }

    /// 呼号一律大写：FSD 那边的呼号是大写的，小写发过去等于发给一个不存在的人。
    #[test]
    fn a_typed_recipient_is_upper_cased() {
        let got = outgoing("zspd_twr", "hi", None).expect("sendable");
        assert_eq!(got.to, "ZSPD_TWR");
    }

    /// 最新的在最后：界面照这个顺序往下贴，反过来的话新消息出现在最上面，
    /// 而用户正盯着最下面等回话。
    #[test]
    fn the_newest_message_is_last() {
        let mut log = ChatLog::default();
        log.record(msg("first"));
        log.record(msg("second"));
        let got = log.snapshot();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].text, "first");
        assert_eq!(got[1].text, "second");
    }

    /// 记录有上界，旧的先丢。
    ///
    /// 不设界的话它就是一条**永不回收的内存**：一次长飞行里飞行员会路过十几个
    /// 频率，每个频率上的每一句话都在这条链路上。
    #[test]
    fn the_log_is_bounded_and_drops_the_oldest() {
        let mut log = ChatLog::default();
        for i in 0..MAX_MESSAGES + 10 {
            log.record(msg(&format!("m{i}")));
        }
        let got = log.snapshot();
        assert_eq!(got.len(), MAX_MESSAGES);
        assert_eq!(
            got[0].text, "m10",
            "the oldest ones must be the ones dropped"
        );
        assert_eq!(got[MAX_MESSAGES - 1].text, format!("m{}", MAX_MESSAGES + 9));
    }

    /// 自己发出去的那条也记下来，并且标着方向。
    ///
    /// 只记收到的话，聊天区里只有对方的半边对话——而管制员的"洛杉矶，明白"
    /// 是接在自己刚发的那句后面才有意义的。
    #[test]
    fn an_outbound_message_is_recorded_with_its_direction() {
        let mut log = ChatLog::default();
        log.record(ChatMessage {
            from: "CCA1501".into(),
            to: "ZSPD_TWR".into(),
            text: "request pushback".into(),
            outbound: true,
            at: 1.0,
        });
        let got = log.snapshot();
        assert_eq!(got.len(), 1);
        assert!(got[0].outbound);
        assert_eq!(got[0].from, "CCA1501");
    }
}
