//! 订阅状态机。
//!
//! 它持有"我想要什么"，负责在连接可用时把意图推给服务端，
//! 并在重连后自动重推。**它不持有"我现在在哪"** —— 那是服务端的事。
//!
//! 这是整个客户端设计的核心。旧实现里那一整类
//! "UI 是绿的但人还在 root 频道" 的 bug，根源是把
//! "我在哪个频道" 当成一个可以记住的事实：重连之后服务端把你放回
//! root，而客户端的记录还是掉线前的值，于是它认为"已经在那儿了"，
//! 永远不再重入。这里没有那个可以记错的字段。

use can_voice_proto::control::{Sub, SubAck};

/// 服务端在 READY 里给出的限额。
///
/// 留住它是为了让上层能在**声明之前**夹住，而不是靠 `rejected` 事后发现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_tx: u32,
    pub max_rx: u32,
}

/// 订阅意图与服务端的确认。
#[derive(Debug, Default)]
pub struct SubscriptionState {
    /// 最近一次声明的完整意图。
    desired: Option<Sub>,
    /// **已经发出去**的那一份。ACK 回答的是它，不是 `desired`——
    /// 这中间用户完全可能又改了台面。
    in_flight: Option<Sub>,
    /// Declaration confirmed by the most recent ACK.
    confirmed: Option<Sub>,
    /// 是否还没推给服务端。
    dirty: bool,
    connected: bool,
    epoch: u64,
    /// 服务端最近确认的内容。掉线即清空。
    acked: SubAck,
    /// READY 带来的限额。掉线即清空：下一次 READY 可能是另一组数。
    limits: Option<Limits>,
}

impl SubscriptionState {
    pub fn new() -> Self {
        Self::default()
    }

    /// 声明完整的收发意图。反复调用是廉价的：每次都是全量，
    /// 所以一连串 UI 操作只会产生最后那一条网络消息。
    pub fn declare(&mut self, sub: Sub) {
        if self.desired.as_ref() == Some(&sub) && !self.dirty {
            // 完全相同的声明不必重发。
            return;
        }
        self.desired = Some(sub);
        self.dirty = true;
    }

    /// 取出待发送的声明。没有待发的、或链路不可用时返回 None。
    pub fn take_pending(&mut self) -> Option<Sub> {
        if !self.connected || !self.dirty || self.in_flight.is_some() {
            return None;
        }
        self.dirty = false;
        self.in_flight = self.desired.clone();
        self.desired.clone()
    }

    /// 链路建立。会把当前意图重新标记为待发 ——
    /// 重连后服务端对我们一无所知，必须无条件重推。
    pub fn on_connected(&mut self, limits: Limits) {
        self.epoch = self.epoch.wrapping_add(1);
        self.connected = true;
        self.in_flight = None;
        self.confirmed = None;
        self.acked = SubAck::default();
        self.limits = Some(limits);
        if self.desired.is_some() {
            self.dirty = true;
        }
    }

    /// 链路断开。清空服务端的确认与限额：
    /// 保留确认会让上层以为订阅还生效着，保留限额会按一组过期的数去夹。
    pub fn on_disconnected(&mut self) {
        self.connected = false;
        self.in_flight = None;
        self.confirmed = None;
        self.acked = SubAck::default();
        self.limits = None;
    }

    /// 记录服务端的确认。
    pub fn on_ack(&mut self, ack: SubAck) {
        self.on_ack_epoch(self.epoch, ack);
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Returns false for an ACK from another connection or with no matching SUB.
    pub fn on_ack_epoch(&mut self, epoch: u64, ack: SubAck) -> bool {
        if !self.connected || epoch != self.epoch {
            return false;
        }
        let Some(sent) = self.in_flight.take() else {
            return false;
        };
        self.dirty = self.desired.as_ref() != Some(&sent);
        self.confirmed = Some(sent);
        self.acked = ack;
        true
    }

    /// 服务端实际接受了什么。上层显示这一份，而不是我们请求的那一份 ——
    /// 服务端可能拒掉超出 max_tx 的频率。
    ///
    /// 里面的 `rejected_xc` 是**另一张单子**：耦合对不在 `rx`/`tx` 里，
    /// 差集公式管不到它，所以那一份要直接读。
    pub fn acknowledged(&self) -> &SubAck {
        &self.acked
    }

    pub fn effective_xc(&self) -> Vec<[u32; 2]> {
        self.confirmed.as_ref().map_or_else(Vec::new, |sub| {
            sub.xc
                .iter()
                .copied()
                .filter(|pair| {
                    self.acked.tx.contains(&pair[0])
                        && self.acked.tx.contains(&pair[1])
                        && !self.acked.rejected_xc.contains(pair)
                })
                .collect()
        })
    }

    /// A later authority-loss notice revokes TX without changing RX.
    pub fn revoke_tx(&mut self) {
        self.acked.tx.clear();
        self.acked.rejected_xc.clear();
    }

    /// READY 给出的限额，掉线后为 `None`。
    pub fn limits(&self) -> Option<Limits> {
        self.limits
    }

    /// 声明了 RX 却没拿到的频率。
    ///
    /// **用差集，不要去读 `rejected` 推断方向。** `ack.rx`/`ack.tx` 是完整且
    /// 权威的（服务端把授权后的全集放进去，长度受 `max_rx`/`max_tx` 约束，
    /// 不存在截断），所以差集在任何情况下都对；而 `rejected` 有上界
    /// （`maxRejected = 256`），截断之后基于它的推断会失效——服务端还会用
    /// `rejected_truncated` 明说这件事。
    ///
    /// 另一个坑：一个频率同时出现在 `rx` 和 `rejected` 里是**正常**的，意思是
    /// "TX 被限额拒了，但 RX 给了"。按"出现在 rejected 里就当没订阅上"处理的
    /// 客户端会把一个能听的频率显示成失败。
    pub fn denied_rx(&self) -> Vec<u32> {
        Self::difference(self.confirmed.as_ref().map(|s| &s.rx), &self.acked.rx)
    }

    /// 声明了 TX 却没拿到的频率。规则同 [`Self::denied_rx`]。
    pub fn denied_tx(&self) -> Vec<u32> {
        Self::difference(self.confirmed.as_ref().map(|s| &s.tx), &self.acked.tx)
    }

    fn difference(declared: Option<&Vec<u32>>, granted: &[u32]) -> Vec<u32> {
        let Some(declared) = declared else {
            return Vec::new();
        };
        declared
            .iter()
            .copied()
            .filter(|f| !granted.contains(f))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_voice_proto::control::{Sub, SubAck};

    fn sub(rx: &[u32]) -> Sub {
        Sub {
            rx: rx.to_vec(),
            ..Default::default()
        }
    }

    fn online() -> SubscriptionState {
        let mut s = SubscriptionState::new();
        s.on_connected(Limits {
            max_tx: 32,
            max_rx: 32,
        });
        s
    }

    #[test]
    fn a_declaration_made_while_offline_is_sent_on_connect() {
        let mut s = SubscriptionState::new();
        s.declare(sub(&[118_000]));
        assert!(
            s.take_pending().is_none(),
            "nothing can be sent before the link is up"
        );

        s.on_connected(Limits {
            max_tx: 32,
            max_rx: 32,
        });
        assert_eq!(s.take_pending(), Some(sub(&[118_000])));
    }

    #[test]
    fn taking_the_pending_declaration_twice_yields_nothing_the_second_time() {
        let mut s = online();
        s.declare(sub(&[118_000]));
        assert!(s.take_pending().is_some());
        assert!(
            s.take_pending().is_none(),
            "an unchanged declaration must not be resent every tick"
        );
    }

    #[test]
    fn the_latest_declaration_wins_and_the_intermediate_ones_are_dropped() {
        // 一连串 UI 操作不该变成一连串网络消息。
        // 每一次声明都是全量的，所以中间那些做的是同样的事。
        let mut s = online();
        s.declare(sub(&[118_000]));
        s.declare(sub(&[118_000, 121_800]));
        s.declare(sub(&[124_550]));
        assert_eq!(s.take_pending(), Some(sub(&[124_550])));
        assert!(s.take_pending().is_none());
    }

    #[test]
    fn a_second_subscription_waits_for_the_first_ack() {
        let mut s = online();
        s.declare(sub(&[118_000]));
        assert_eq!(s.take_pending(), Some(sub(&[118_000])));
        s.declare(sub(&[121_800]));
        assert_eq!(s.take_pending(), None);
        s.on_ack(SubAck {
            rx: vec![118_000],
            ..Default::default()
        });
        assert_eq!(s.take_pending(), Some(sub(&[121_800])));
    }

    #[test]
    fn an_ack_from_an_old_connection_cannot_change_current_state() {
        let mut s = online();
        s.declare(sub(&[118_000]));
        s.take_pending();
        let old_epoch = s.epoch();
        s.on_disconnected();
        s.on_connected(Limits {
            max_tx: 32,
            max_rx: 32,
        });
        s.take_pending();
        assert!(!s.on_ack_epoch(
            old_epoch,
            SubAck {
                rx: vec![118_000],
                ..Default::default()
            }
        ));
        assert!(s.acknowledged().rx.is_empty());
    }

    #[test]
    fn reconnecting_resends_the_declaration_without_being_asked() {
        // 这是整个声明式设计的要点。重连后服务端对我们一无所知，
        // 而客户端不需要记得"我原来在哪个频道"——它只是重发意图。
        let mut s = online();
        s.declare(sub(&[118_000, 121_800]));
        s.take_pending();
        s.on_ack(SubAck {
            rx: vec![118_000, 121_800],
            ..Default::default()
        });

        s.on_disconnected();
        s.on_connected(Limits {
            max_tx: 32,
            max_rx: 32,
        });
        assert_eq!(
            s.take_pending(),
            Some(sub(&[118_000, 121_800])),
            "a reconnect must resend the full declaration unprompted"
        );
    }

    #[test]
    fn a_disconnect_clears_what_the_server_had_acknowledged() {
        // 保留旧的 ack 会让上层以为订阅还生效着 ——
        // 那正是"UI 是绿的但人还在 root 频道"的形状。
        let mut s = online();
        s.declare(sub(&[118_000]));
        s.take_pending();
        s.on_ack(SubAck {
            rx: vec![118_000],
            ..Default::default()
        });
        assert_eq!(s.acknowledged().rx, vec![118_000]);

        s.on_disconnected();
        assert!(
            s.acknowledged().rx.is_empty(),
            "after a drop the server knows nothing about us; claiming otherwise is the bug this design exists to prevent"
        );
    }

    #[test]
    fn acknowledged_reflects_what_the_server_actually_accepted() {
        // 服务端可能拒掉超出 max_tx 的频率。上层要显示被接受的那一份，
        // 不是我们请求的那一份。
        let mut s = online();
        s.declare(Sub {
            rx: vec![118_000],
            tx: vec![118_000, 121_800],
            ..Default::default()
        });
        s.take_pending();
        s.on_ack(SubAck {
            rx: vec![118_000],
            tx: vec![118_000],
            rejected: vec![121_800],
            ..Default::default()
        });
        assert_eq!(s.acknowledged().tx, vec![118_000]);
        assert_eq!(s.acknowledged().rejected, vec![121_800]);
    }

    #[test]
    fn there_is_no_api_to_remember_where_we_are() {
        // 编译期保证：SubscriptionState 只暴露"我想要什么"和
        // "服务端确认了什么"，没有任何 join/leave/current_channel。
        // 这个测试存在的意义是让有人试图加那种方法时看到这段话。
        let s = SubscriptionState::new();
        let _ = s.acknowledged();
    }

    // ——— H2：用差集，不要读 `rejected` 去推断方向 ———

    /// `ack.rx` 和 `ack.tx` 是**完整且权威**的（服务端把授权后的全集放进去，
    /// 长度受 `max_rx`/`max_tx` 约束，不存在截断），所以差集是精确的、完备的。
    /// `rejected` 有上界（`maxRejected = 256`），超出的部分根本不进那张单子。
    #[test]
    fn denials_come_from_the_difference_not_from_the_rejected_list() {
        let mut s = online();
        s.declare(Sub {
            rx: vec![118_000, 121_800, 124_550],
            tx: vec![118_000, 121_800],
            ..Default::default()
        });
        s.take_pending();
        // 服务端只给了一部分，而且 `rejected` 是空的（被截断了，或者服务端
        // 这一版根本没填）。差集照样算得出来。
        s.on_ack(SubAck {
            rx: vec![118_000, 121_800],
            tx: vec![118_000],
            ..Default::default()
        });
        assert_eq!(s.denied_rx(), vec![124_550]);
        assert_eq!(s.denied_tx(), vec![121_800]);
    }

    /// 一个频率同时出现在 `rx` 和 `rejected` 里是**正常**的，意思是
    /// "TX 被限额拒了，但 RX 给了"。按"出现在 rejected 里就当没订阅上"处理的
    /// 客户端会把一个能听的频率显示成失败。
    #[test]
    fn a_frequency_rejected_for_tx_but_granted_for_rx_is_not_an_rx_failure() {
        let mut s = online();
        s.declare(Sub {
            rx: vec![121_800],
            tx: vec![121_800],
            ..Default::default()
        });
        s.take_pending();
        s.on_ack(SubAck {
            rx: vec![121_800],
            tx: vec![],
            rejected: vec![121_800],
            ..Default::default()
        });
        assert!(s.denied_rx().is_empty(), "the frequency was granted for rx");
        assert_eq!(s.denied_tx(), vec![121_800]);
    }

    /// N1：`rejected` 被截断时它不再是权威记录，但差集仍然对。
    #[test]
    fn a_truncated_rejection_list_does_not_disturb_the_difference() {
        let mut s = online();
        let rx: Vec<u32> = (0..40).map(|i| 118_000 + i * 25).collect();
        s.declare(Sub {
            rx: rx.clone(),
            ..Default::default()
        });
        s.take_pending();
        s.on_ack(SubAck {
            rx: rx[..32].to_vec(),
            rejected: vec![rx[32]], // 只报了一个，其余被截断
            rejected_truncated: true,
            ..Default::default()
        });
        assert!(s.acknowledged().rejected_truncated);
        assert_eq!(
            s.denied_rx(),
            rx[32..].to_vec(),
            "the difference is complete even when the list is not"
        );
    }

    /// 差集要对着**发出去的那一份**算，不是对着当前意图。
    /// ACK 回答的是已经发出的声明；这中间用户完全可能又改了台面。
    #[test]
    fn the_difference_is_taken_against_the_declaration_that_was_actually_sent() {
        let mut s = online();
        s.declare(sub(&[118_000, 121_800]));
        s.take_pending();
        // ACK 在路上时用户又改了。
        s.declare(sub(&[136_975]));
        s.on_ack(SubAck {
            rx: vec![118_000],
            ..Default::default()
        });
        assert_eq!(
            s.denied_rx(),
            vec![121_800],
            "the ack answers what was sent, not what the user has since typed"
        );
    }

    // ——— C1：交叉耦合被拒必须留得住 ———

    #[test]
    fn rejected_cross_couple_pairs_are_kept_for_the_ui() {
        let mut s = online();
        s.declare(Sub {
            rx: vec![121_800, 124_550],
            tx: vec![121_800, 124_550],
            xc: vec![[121_800, 124_550]],
        });
        s.take_pending();
        s.on_ack(SubAck {
            rx: vec![121_800, 124_550],
            tx: vec![121_800],
            rejected_xc: vec![[121_800, 124_550]],
            ..Default::default()
        });
        assert_eq!(s.acknowledged().rejected_xc, vec![[121_800, 124_550]],
            "a controller whose coupling silently did nothing is the failure this whole rewrite exists to escape");
    }

    // ——— M5：READY 的限额不能在边界上丢掉 ———

    /// 上层要能在**声明之前**夹住，而不是靠 `rejected` 事后发现。
    #[test]
    fn the_server_limits_survive_into_the_state() {
        let mut s = SubscriptionState::new();
        assert_eq!(
            s.limits(),
            None,
            "before READY there is nothing to clamp against"
        );
        s.on_connected(Limits {
            max_tx: 4,
            max_rx: 8,
        });
        assert_eq!(
            s.limits(),
            Some(Limits {
                max_tx: 4,
                max_rx: 8
            })
        );
    }

    #[test]
    fn a_disconnect_forgets_the_limits_too() {
        let mut s = online();
        s.on_disconnected();
        assert_eq!(
            s.limits(),
            None,
            "the next READY may carry different limits; a stale one would clamp wrongly"
        );
    }
}
