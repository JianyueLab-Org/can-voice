//! 前端随时可查的状态快照。
//!
//! # 为什么不能让前端自己从事件流拼
//!
//! `VoiceClient::events()` 是 **broadcast**：挂上之前发生的事收不到。窗口重开、
//! 前端热重载、一次慢启动都会打断它，而一个"从第一条事件开始拼状态"的界面在那之后
//! 显示的是一段它错过了的历史。所以这里持一份随时可查的当前状态。
//!
//! 它也是**唯一**需要知道"哪些事件改变了什么"的地方——前端只读这个结构。

use can_voice_client::audio::PlaybackStats;
use can_voice_client::conn::{LinkState, RefusedReason};
use can_voice_client::stack::TxBudget;
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

/// 声卡打不开。
pub const AUDIO_UNAVAILABLE: &str = "audio_unavailable";
/// 声卡又开起来了。**它不是一条要显示的通知**，是撤掉上面那条的信号。
pub const AUDIO_RESTORED: &str = "audio_restored";

/// 链路健康。掉线那一行要带着它一起打出来。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct Health {
    pub rtt_ms: u32,
    pub sent: u64,
    pub received: u64,
    /// QUIC 认定丢掉的包。
    pub lost: u64,
    /// 包头解不开的数据报——协议漂移，不是网络丢包。
    pub unparsable: u64,
    /// 播放环的对账。**链路好和听得清是两件事**：链路全绿而播放环跑干的时候，
    /// 上面那几个数一个都不会动，而用户听到的是电音加卡顿。
    pub playback: PlaybackStats,
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
    pub effective_rx: BTreeSet<u32>,
    pub effective_tx: BTreeSet<u32>,
    pub effective_xc: BTreeSet<[u32; 2]>,
    pub health: Option<Health>,
    /// 服务端的其它通知，最近的在最后。
    pub notices: Vec<(String, u32, String)>,
    /// 会话 id → CAN 号。来自 NOTICE talker，每个发言者只来一次。
    #[serde(default)]
    pub speakers: BTreeMap<u32, String>,
    /// Manual remote receive coefficients keyed by stable CAN id.
    #[serde(default)]
    pub talker_volumes: BTreeMap<String, f32>,
    /// 每个频率上最近一次通话。
    ///
    /// **绿点只说"此刻有没有人在讲"。** 管制员真正要判断的是"这个频率还活着
    /// 吗"——三秒前有人说过话和二十分钟没动静是两种处境，而绿点灭了之后这两者
    /// 长得一模一样。
    pub last_talk: BTreeMap<u32, LastTalk>,
    /// 服务端给这一条链路的发射上限（READY 的 `max_tx`）。没连上、或者掉了线是
    /// `None`：上限写在票里、按人签，下一次 READY 可能是另一个数。
    pub max_tx: Option<u32>,
    /// 台面相对 `max_tx` 的处境：此刻声明几个 TX、在哪几格上再开会超额。
    ///
    /// **由 [`crate::Bridge::snapshot`] 在读的那一刻填**，事件从不碰它：它一半是台面
    /// 的状态，存一份副本就会有两个真相，而对不上的那一刻界面提示的是一份过期的
    /// 处境。`max_tx` 是 `None` 时它也是 `None`，界面据此什么都不说。
    pub tx_budget: Option<TxBudget>,
}

/// 某个频率上最近一次通话。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LastTalk {
    /// 说话的那个人的会话 id。
    pub speaker: u32,
    /// CAN 号。空串表示 talker 通知还没到。界面用花名册翻成呼号。
    #[serde(default)]
    pub cid: String,
    /// Unix 秒。界面自己按本地时区格式化——这一层不知道用户在哪个时区。
    pub at: u64,
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
            effective_rx: BTreeSet::new(),
            effective_tx: BTreeSet::new(),
            effective_xc: BTreeSet::new(),
            health: None,
            notices: Vec::new(),
            speakers: BTreeMap::new(),
            talker_volumes: BTreeMap::new(),
            last_talk: BTreeMap::new(),
            max_tx: None,
            tx_budget: None,
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
    ///
    /// 时间由 [`Snapshot::apply_at`] 那一版注入——这一版读的是系统时钟。
    pub fn apply(&mut self, event: &Event) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        self.apply_at(event, now);
    }

    /// 把一个事件吃进来，时间由调用方给。
    ///
    /// **时钟是参数而不是全局**：不然"最后一次通话"这件事就只能靠 sleep 来测，
    /// 而一条靠 sleep 的测试要么慢要么不稳。
    pub fn apply_at(&mut self, event: &Event, now_unix: u64) {
        match event {
            Event::State(state) => self.on_state(*state),
            Event::Limits(limits) => self.max_tx = Some(limits.max_tx),
            Event::Refused { reason } => self.ended = Some(Ended::Refused(reason.clone())),
            Event::RxStart { freq_khz, speaker } => {
                self.receiving
                    .entry(*freq_khz)
                    .or_default()
                    .insert(*speaker);
            }
            Event::Talker {
                session,
                cid,
                freq_khz: _,
            } => {
                self.speakers.insert(*session, cid.clone());
                for t in self.last_talk.values_mut() {
                    if t.speaker == *session {
                        t.cid = cid.clone();
                    }
                }
            }
            Event::RxEnd {
                freq_khz, speaker, ..
            } => {
                if let Some(set) = self.receiving.get_mut(freq_khz) {
                    set.remove(speaker);
                }
                // 记的是**说完**的那一刻：开始说的时间在一段长通话里越来越不像
                // "最近"，而管制员问的正是"多久没人说话了"。
                self.last_talk.insert(
                    *freq_khz,
                    LastTalk {
                        speaker: *speaker,
                        cid: self.speakers.get(speaker).cloned().unwrap_or_default(),
                        at: now_unix,
                    },
                );
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
            Event::SubscriptionAck { rx, tx, xc } => {
                self.effective_rx = rx.iter().copied().collect();
                self.effective_tx = tx.iter().copied().collect();
                self.effective_xc = xc.iter().copied().collect();
                self.denied_rx.clear();
                self.denied_tx.clear();
                self.denied_xc.clear();
            }
            Event::Notice {
                kind,
                freq_khz,
                reason,
            } => {
                // **恢复是撤掉，不是再叠一条。** 两条并排显示的话，用户看到的是
                // "声卡打不开"和"声卡好了"同时在列，而他分不清哪一条说的是现在。
                if kind == AUDIO_RESTORED {
                    self.notices.retain(|(k, _, _)| k != AUDIO_UNAVAILABLE);
                    return;
                }
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
                unparsable,
                playback,
            } => {
                self.health = Some(Health {
                    rtt_ms: *rtt_ms,
                    sent: *sent,
                    received: *received,
                    lost: *lost,
                    unparsable: *unparsable,
                    playback: *playback,
                });
            }
        }
    }

    fn on_state(&mut self, state: LinkState) {
        self.link = state;
        // 链路一掉，这一条的上限就不作数了。`Online` 不清：`Limits` 在它之前到。
        if state != LinkState::Online {
            self.max_tx = None;
            self.effective_rx.clear();
            self.effective_tx.clear();
            self.effective_xc.clear();
        }
        match state {
            LinkState::Online => {
                // 一份新的声明正在路上，旧的拒绝清掉——留着会让界面一直显示
                // 一条已经不存在的失败。
                self.denied_tx.clear();
                self.denied_rx.clear();
                self.denied_xc.clear();
                self.ended = None;
                self.receiving.clear();
                self.speakers.clear();
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
    use can_voice_client::session::Limits;

    fn online() -> Snapshot {
        let mut s = Snapshot::default();
        s.apply(&Event::State(LinkState::Online));
        s
    }

    #[test]
    fn effective_radios_follow_the_latest_server_ack() {
        let mut snapshot = online();
        snapshot.apply(&Event::SubscriptionAck {
            rx: vec![118_000, 121_800],
            tx: vec![118_000],
            xc: vec![],
        });
        assert_eq!(snapshot.effective_rx, BTreeSet::from([118_000, 121_800]));
        assert_eq!(snapshot.effective_tx, BTreeSet::from([118_000]));
        snapshot.apply(&Event::TxDenied {
            freq_khz: 121_800,
            reason: String::new(),
        });
        snapshot.apply(&Event::SubscriptionAck {
            rx: vec![121_800],
            tx: vec![],
            xc: vec![],
        });
        assert!(snapshot.effective_tx.is_empty());
        assert!(snapshot.denied_tx.is_empty());
    }

    /// **每一行要记得最后一次通话是什么时候。**
    ///
    /// 一个绿点只说"此刻有没有人在讲"。管制员真正要判断的是"这个频率还活着吗"
    /// ——三秒前有人说过话和二十分钟没动静，是两种完全不同的处境，而绿点灭了
    /// 之后这两者长得一模一样。
    ///
    /// 记的是**会话 id 不是呼号**：协议里 `speaker` 是服务端给这条会话编的号，
    /// 客户端手上没有它到 CAN 号的映射。见 #46。
    #[test]
    fn a_frequency_remembers_when_somebody_last_spoke_on_it() {
        let mut s = Snapshot::default();

        s.apply_at(
            &Event::RxStart {
                freq_khz: 121_800,
                speaker: 7,
            },
            1_700_000_000,
        );
        s.apply_at(
            &Event::RxEnd {
                freq_khz: 121_800,
                speaker: 7,
                frames: 200,
                secs: 4.0,
            },
            1_700_000_004,
        );

        let last = s.last_talk.get(&121_800).cloned().expect("recorded");
        assert_eq!(last.speaker, 7);
        // 记的是**说完**的那一刻：开始说的时间在一段长通话里越来越不像"最近"。
        assert_eq!(last.at, 1_700_000_004);
        assert!(!s.is_receiving(121_800));
        assert!(last.cid.is_empty());
    }

    /// talker 通知把会话号换成 CAN 号。后到也要补上已经记下的最后通话。
    #[test]
    fn a_talker_notice_fills_in_the_cid() {
        let mut s = Snapshot::default();
        s.apply_at(
            &Event::RxStart {
                freq_khz: 121_800,
                speaker: 7,
            },
            1,
        );
        s.apply_at(
            &Event::RxEnd {
                freq_khz: 121_800,
                speaker: 7,
                frames: 10,
                secs: 0.2,
            },
            2,
        );
        s.apply_at(
            &Event::Talker {
                session: 7,
                cid: "1000".into(),
                freq_khz: 121_800,
            },
            3,
        );
        let last = s.last_talk.get(&121_800).cloned().expect("recorded");
        assert_eq!(last.cid, "1000");
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

    /// **声卡恢复要把那条"坏了"撤掉，而不是再叠一条。**
    ///
    /// 两条并排显示的话，用户看到的是"声卡打不开"和"声卡好了"同时在列，
    /// 而他分不清哪一条说的是现在。
    #[test]
    fn audio_coming_back_clears_the_notice_that_said_it_was_gone() {
        let mut s = online();
        s.apply(&Event::Notice {
            kind: "audio_unavailable".into(),
            freq_khz: 0,
            reason: "gone".into(),
        });
        assert_eq!(s.notices.len(), 1);

        s.apply(&Event::Notice {
            kind: "audio_restored".into(),
            freq_khz: 0,
            reason: "back".into(),
        });

        assert!(
            s.notices.is_empty(),
            "恢复之后不该还留着任何一条声卡通知：{:?}",
            s.notices
        );
    }

    /// 别的通知不受影响——撤掉的只有声卡那一条。
    #[test]
    fn other_notices_survive_an_audio_recovery() {
        let mut s = online();
        s.apply(&Event::Notice {
            kind: "range_unavailable".into(),
            freq_khz: 0,
            reason: "no feed".into(),
        });
        s.apply(&Event::Notice {
            kind: "audio_restored".into(),
            freq_khz: 0,
            reason: "back".into(),
        });
        assert_eq!(s.notices.len(), 1);
        assert_eq!(s.notices[0].0, "range_unavailable");
    }

    // ——— 服务端的发射上限 ———

    /// **READY 的发射上限要走到快照里。** 核心库留住了它，却一路没人往上交，
    /// 于是界面只能从"发射被拒"事后知道——正是 READY 把它带下来要躲开的次序。
    #[test]
    fn the_server_tx_limit_reaches_the_snapshot() {
        let mut s = Snapshot::default();
        assert_eq!(s.max_tx, None, "before READY there is no limit to show");

        s.apply(&Event::Limits(Limits {
            max_tx: 4,
            max_rx: 32,
        }));
        s.apply(&Event::State(LinkState::Online));

        assert_eq!(s.max_tx, Some(4));
    }

    /// 掉线就忘掉：下一次 READY 可能是另一个数（上限在票里，按人签），
    /// 拿旧的去提示会说错话。
    #[test]
    fn losing_the_link_forgets_the_tx_limit() {
        for state in [
            LinkState::Reconnecting,
            LinkState::Offline,
            LinkState::Evicted,
        ] {
            let mut s = online();
            s.apply(&Event::Limits(Limits {
                max_tx: 4,
                max_rx: 32,
            }));
            s.apply(&Event::State(state));
            assert_eq!(s.max_tx, None, "{state:?} must forget the limit");
        }
    }

    #[test]
    fn health_is_kept_for_the_drop_line() {
        let mut s = online();
        s.apply(&Event::Health {
            rtt_ms: 42,
            sent: 100,
            received: 99,
            lost: 1,
            unparsable: 0,
            playback: PlaybackStats {
                depth_ms: 60,
                underruns: 2,
                ..Default::default()
            },
        });
        assert_eq!(s.health.as_ref().map(|h| h.rtt_ms), Some(42));
        // 播放环那一半也要跟着留下：掉线那一行要能分清"链路垮了"和"声卡这边
        // 跑干了"。
        assert_eq!(s.health.as_ref().map(|h| h.playback.underruns), Some(2));
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
        // 没收到上限就是 null，界面据此什么都不说。
        assert_eq!(v["max_tx"], serde_json::Value::Null);
        assert_eq!(v["tx_budget"], serde_json::Value::Null);

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
