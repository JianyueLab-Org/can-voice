//! 前端随时可查的状态快照。
//!
//! # 为什么不能让前端自己从事件流拼
//!
//! `VoiceClient::events()` 是 **broadcast**：挂上之前发生的事收不到。窗口重开、
//! 前端热重载、一次慢启动都会打断它，而一个"从第一条事件开始拼状态"的界面在那之后
//! 显示的是一段它错过了的历史。所以这里持一份随时可查的当前状态。
//!
//! 它也是**唯一**需要知道"哪些事件改变了什么"的地方——前端只读这个结构。

use can_voice_client::conn::{LinkState, RefusedReason};
use can_voice_client::Event;
use std::collections::{BTreeMap, BTreeSet};

/// 这条链路是怎么结束的。`None` 表示还没结束。
///
/// **三种要分得开**，因为对人说的话完全不一样。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Ended {
    /// 普通下线。
    Offline,
    /// 同一个账号在别处登录，这一条被顶掉了。要单独说，
    /// 笼统的"连接断开"会把用户送去查网络。
    Evicted,
    /// 握手被拒。`ProtoUnsupported` 要说"请更新客户端"，
    /// `TokenExpired` 换张票还能救，其余的是"别试了"。
    Refused(RefusedReason),
}

/// 链路健康。掉线那一行要带着它一起打出来。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct Health {
    pub rtt_ms: u32,
    pub sent: u64,
    pub received: u64,
    pub lost: u64,
}

/// 界面要显示的一切。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Snapshot {
    pub link: LinkState,
    pub ended: Option<Ended>,
    /// 每个频率上**正在讲话的人**。
    ///
    /// 是集合不是布尔量：同频可能有两个人，第一个松手时灯不能灭。
    pub receiving: BTreeMap<u32, BTreeSet<u32>>,
    pub denied_tx: BTreeSet<u32>,
    pub denied_rx: BTreeSet<u32>,
    pub denied_xc: BTreeSet<[u32; 2]>,
    pub health: Option<Health>,
    /// 服务端的其它通知，最近的在最后。
    pub notices: Vec<(String, u32, String)>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            link: LinkState::Connecting,
            ended: None,
            receiving: BTreeMap::new(),
            denied_tx: BTreeSet::new(),
            denied_rx: BTreeSet::new(),
            denied_xc: BTreeSet::new(),
            health: None,
            notices: Vec::new(),
        }
    }
}

/// 通知最多留这么多条。界面只显示最近的几条，而一条会话可能跑一整天。
const MAX_NOTICES: usize = 32;

impl Snapshot {
    /// 这个频率上有人在讲话吗。RX 灯就是它。
    pub fn is_receiving(&self, freq_khz: u32) -> bool {
        self.receiving.get(&freq_khz).is_some_and(|s| !s.is_empty())
    }

    /// 把一个事件吃进来。
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::State(state) => self.on_state(*state),
            Event::Refused { reason } => self.ended = Some(Ended::Refused(reason.clone())),
            Event::RxStart { freq_khz, speaker } => {
                self.receiving
                    .entry(*freq_khz)
                    .or_default()
                    .insert(*speaker);
            }
            Event::RxEnd {
                freq_khz, speaker, ..
            } => {
                if let Some(set) = self.receiving.get_mut(freq_khz) {
                    set.remove(speaker);
                }
            }
            Event::TxDenied { freq_khz, .. } => {
                self.denied_tx.insert(*freq_khz);
            }
            Event::RxDenied { freq_khz } => {
                self.denied_rx.insert(*freq_khz);
            }
            Event::XcDenied { a_khz, b_khz, .. } => {
                self.denied_xc.insert([*a_khz, *b_khz]);
            }
            Event::Notice {
                kind,
                freq_khz,
                reason,
            } => {
                self.notices.push((kind.clone(), *freq_khz, reason.clone()));
                if self.notices.len() > MAX_NOTICES {
                    self.notices.remove(0);
                }
            }
            Event::Health {
                rtt_ms,
                sent,
                received,
                lost,
            } => {
                self.health = Some(Health {
                    rtt_ms: *rtt_ms,
                    sent: *sent,
                    received: *received,
                    lost: *lost,
                });
            }
        }
    }

    fn on_state(&mut self, state: LinkState) {
        self.link = state;
        match state {
            LinkState::Online => {
                // 一份新的声明正在路上，旧的拒绝清掉——留着会让界面一直显示
                // 一条已经不存在的失败。
                self.denied_tx.clear();
                self.denied_rx.clear();
                self.denied_xc.clear();
                self.ended = None;
                self.receiving.clear();
            }
            LinkState::Connecting => {}
            LinkState::Reconnecting => {
                // 链路还活着，所以不是结束；但**没有人在对我们讲话**了，
                // 留着亮的灯会让界面显示一个不存在的通话。
                self.receiving.clear();
            }
            LinkState::Evicted => {
                self.receiving.clear();
                self.ended = Some(Ended::Evicted);
            }
            LinkState::Offline => {
                self.receiving.clear();
                // 已经有更具体的死因（被拒、被顶）就别盖掉它：
                // "连接断开"救不了任何人。
                self.ended.get_or_insert(Ended::Offline);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_voice_client::conn::RefusedReason;

    fn online() -> Snapshot {
        let mut s = Snapshot::default();
        s.apply(&Event::State(LinkState::Online));
        s
    }

    #[test]
    fn a_fresh_snapshot_is_not_connected() {
        let s = Snapshot::default();
        assert_eq!(s.link, LinkState::Connecting);
        assert!(s.ended.is_none());
        assert!(s.receiving.is_empty());
    }

    /// **这就是快照存在的理由。** 事件流是广播，挂上之前发生的事收不到——
    /// 窗口重开、前端热重载都会打断它。所以状态要有一份随时可查的，
    /// 而不是靠"从第一条事件开始拼"。
    #[test]
    fn attaching_late_still_sees_the_current_state() {
        let mut s = online();
        s.apply(&Event::RxStart {
            freq_khz: 121_800,
            speaker: 7,
        });
        s.apply(&Event::TxDenied {
            freq_khz: 118_000,
            reason: String::new(),
        });

        // 前端此刻才挂上：它读到的是当前状态，不是一段它错过了的历史。
        assert_eq!(s.link, LinkState::Online);
        assert!(s.is_receiving(121_800));
        assert!(s.denied_tx.contains(&118_000));
    }

    // ——— RX 灯 ———

    #[test]
    fn a_transmission_lights_the_frequency_and_ends_it() {
        let mut s = online();
        s.apply(&Event::RxStart {
            freq_khz: 121_800,
            speaker: 7,
        });
        assert!(s.is_receiving(121_800));
        s.apply(&Event::RxEnd {
            freq_khz: 121_800,
            speaker: 7,
            frames: 10,
            secs: 0.2,
        });
        assert!(!s.is_receiving(121_800));
    }

    /// **同频两个人，一个说完了，灯不能灭。** 灯是"这个频率上有人在讲"，
    /// 不是"最后一个开口的人还在讲"——按布尔量记的实现会在第一个人松手时
    /// 把灯灭掉，而另一个还在说话。
    #[test]
    fn one_speaker_finishing_does_not_darken_a_frequency_someone_else_is_using() {
        let mut s = online();
        s.apply(&Event::RxStart {
            freq_khz: 121_800,
            speaker: 7,
        });
        s.apply(&Event::RxStart {
            freq_khz: 121_800,
            speaker: 9,
        });
        s.apply(&Event::RxEnd {
            freq_khz: 121_800,
            speaker: 7,
            frames: 5,
            secs: 0.1,
        });
        assert!(s.is_receiving(121_800), "speaker 9 is still talking");
        s.apply(&Event::RxEnd {
            freq_khz: 121_800,
            speaker: 9,
            frames: 5,
            secs: 0.1,
        });
        assert!(!s.is_receiving(121_800));
    }

    #[test]
    fn frequencies_light_independently() {
        let mut s = online();
        s.apply(&Event::RxStart {
            freq_khz: 118_000,
            speaker: 7,
        });
        assert!(s.is_receiving(118_000));
        assert!(!s.is_receiving(121_800));
    }

    /// 掉线之后**没有人在对我们讲话**。留着亮的灯会让界面显示一个不存在的通话，
    /// 而那正是"UI 是绿的但人还在 root 频道"那类故障的形状。
    #[test]
    fn dropping_the_link_darkens_every_light() {
        let mut s = online();
        s.apply(&Event::RxStart {
            freq_khz: 121_800,
            speaker: 7,
        });
        s.apply(&Event::State(LinkState::Reconnecting));
        assert!(!s.is_receiving(121_800));
    }

    // ——— 三种终态要分得开 ———

    /// `Reconnecting` 与 `Offline` 是对立的：前者意味着链路还活着，
    /// 界面**不要丢掉对象引用**；后者意味着它没了。
    #[test]
    fn reconnecting_is_not_an_ending() {
        let mut s = online();
        s.apply(&Event::State(LinkState::Reconnecting));
        assert_eq!(s.link, LinkState::Reconnecting);
        assert!(s.ended.is_none(), "still alive");
    }

    /// 被顶号要单独说"账号在别处登录了"，而不是笼统的"连接断开"——
    /// 后者会把用户送去查网络。
    #[test]
    fn eviction_is_its_own_ending() {
        let mut s = online();
        s.apply(&Event::State(LinkState::Evicted));
        assert_eq!(s.ended, Some(Ended::Evicted));
    }

    /// `proto_unsupported` 的动作和 `refused` 一样，但对人说的话不一样：
    /// 一个版本太旧的用户该看到"请更新客户端"，而"被拒绝"会把他送去查密码、
    /// 去换票、去怀疑自己的账号——三件事一件都帮不上忙。
    #[test]
    fn an_outdated_client_ends_differently_from_a_refused_one() {
        let mut a = online();
        a.apply(&Event::Refused {
            reason: RefusedReason::ProtoUnsupported,
        });
        a.apply(&Event::State(LinkState::Offline));

        let mut b = online();
        b.apply(&Event::Refused {
            reason: RefusedReason::Refused,
        });
        b.apply(&Event::State(LinkState::Offline));

        assert_ne!(a.ended, b.ended, "these two need different words on screen");
        assert_eq!(
            a.ended,
            Some(Ended::Refused(RefusedReason::ProtoUnsupported))
        );
    }

    /// 换票可以救回来的那一条，界面要知道它和别的拒绝不一样。
    #[test]
    fn a_recoverable_refusal_says_so() {
        let mut s = online();
        s.apply(&Event::Refused {
            reason: RefusedReason::TokenExpired,
        });
        s.apply(&Event::State(LinkState::Offline));
        assert!(matches!(s.ended, Some(Ended::Refused(ref r)) if r.is_recoverable()));
    }

    // ——— 被拒的频率 ———

    #[test]
    fn rx_and_tx_denials_are_kept_apart() {
        let mut s = online();
        s.apply(&Event::TxDenied {
            freq_khz: 121_800,
            reason: String::new(),
        });
        s.apply(&Event::RxDenied { freq_khz: 118_000 });
        assert!(s.denied_tx.contains(&121_800) && !s.denied_rx.contains(&121_800));
        assert!(s.denied_rx.contains(&118_000) && !s.denied_tx.contains(&118_000));
    }

    #[test]
    fn a_rejected_cross_couple_pair_is_kept() {
        let mut s = online();
        s.apply(&Event::XcDenied {
            a_khz: 121_800,
            b_khz: 124_550,
            reason: "x".into(),
        });
        assert!(s.denied_xc.contains(&[121_800, 124_550]));
    }

    /// 重新连上之后那些拒绝要清掉：一份新的声明正在路上，
    /// 留着旧的会让界面一直显示一条已经不存在的失败。
    #[test]
    fn coming_back_online_clears_the_previous_denials() {
        let mut s = online();
        s.apply(&Event::TxDenied {
            freq_khz: 121_800,
            reason: String::new(),
        });
        s.apply(&Event::State(LinkState::Reconnecting));
        s.apply(&Event::State(LinkState::Online));
        assert!(s.denied_tx.is_empty());
    }

    #[test]
    fn health_is_kept_for_the_drop_line() {
        let mut s = online();
        s.apply(&Event::Health {
            rtt_ms: 42,
            sent: 100,
            received: 99,
            lost: 1,
        });
        assert_eq!(s.health.as_ref().map(|h| h.rtt_ms), Some(42));
    }

    /// **快照的 JSON 形状是一份跨语言契约，这里把它钉住。**
    ///
    /// 四个 Tauri 客户端的界面照着这些串写判断——`"Online"`、`"Evicted"`、
    /// `{"Refused":"ProtoUnsupported"}`。改一个变体名，Rust 这边照样编译、
    /// 照样通过所有别的测试，而界面会安静地落到"连接被拒"那条兜底分支上：
    /// 一个版本太旧的用户于是去查密码，而不是去更新客户端。
    ///
    /// 特别钉住两件容易在重构里丢掉的事：`receiving` 的整数键被 serde_json
    /// 写成**字符串**（前端因此必须用 `String(khz)` 去查），以及
    /// `denied_xc` 的 `[u32; 2]` 是一个**二元数组**而不是对象。
    #[test]
    fn the_snapshot_json_is_the_shape_the_ui_reads() {
        let mut s = online();
        s.apply(&Event::RxStart {
            freq_khz: 121_800,
            speaker: 7,
        });
        s.apply(&Event::XcDenied {
            a_khz: 121_800,
            b_khz: 124_550,
            reason: String::new(),
        });
        let v = serde_json::to_value(&s).expect("serialize");

        assert_eq!(v["link"], serde_json::json!("Online"));
        assert_eq!(v["ended"], serde_json::Value::Null);
        // 整数键是字符串键。
        assert_eq!(v["receiving"]["121800"], serde_json::json!([7]));
        // 二元数组，不是 {a,b}。
        assert_eq!(v["denied_xc"], serde_json::json!([[121_800, 124_550]]));

        // 三种终态各自的串——界面对它们说三句不同的话。
        for (ended, want) in [
            (Ended::Offline, serde_json::json!("Offline")),
            (Ended::Evicted, serde_json::json!("Evicted")),
            (
                Ended::Refused(RefusedReason::ProtoUnsupported),
                serde_json::json!({ "Refused": "ProtoUnsupported" }),
            ),
            (
                Ended::Refused(RefusedReason::TokenExpired),
                serde_json::json!({ "Refused": "TokenExpired" }),
            ),
            (
                Ended::Refused(RefusedReason::Other("mystery".into())),
                serde_json::json!({ "Refused": { "Other": "mystery" } }),
            ),
        ] {
            assert_eq!(serde_json::to_value(&ended).expect("serialize"), want);
        }
    }
}
