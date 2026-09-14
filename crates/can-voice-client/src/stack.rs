//! 无线电栈模型：一个管制员的若干频率，每个带 RX/TX/XC 三个开关。
//!
//! 耦合规则抄自 TrackAudio 的 `radio.tsx`，与 can-audio 的
//! `controller/radiostack.py` 一一对应。这里没有 I/O、没有网络、没有 UI ——
//! 和 Python 版一样，这是全库最值得单测的部分。
//!
//! 放在共享库而不是 controller 应用里：xpc/msfs 用的是它的退化版（单频率）。

use can_voice_proto::control::Sub;

/// 服务端一份声明里最多处理的耦合对数，等于 `router.go` 的 `maxXCPairs`。
///
/// **客户端必须自己夹到这个数以内。** 服务端在第 65 对上停下，并且只把
/// **接下来的 64 对**放进 `rejected_xc`（回报本身也得有界，否则 ACK 超过
/// 64 KiB 就根本发不出去）——**第 129 对往后是静默丢弃的**。
/// 17 个互相耦合的频率就是 C(17,2) = 136 对，而 `max_tx` 默认 32，
/// 所以这不是一个够不着的数。
pub const MAX_XC_PAIRS: usize = 64;

/// 一个频率及其开关。
#[derive(Debug, Clone, PartialEq)]
pub struct Radio {
    pub freq_khz: u32,
    pub rx: bool,
    pub tx: bool,
    pub xc: bool,
    /// 每频率音量，1.0 为原音量。
    pub gain: f32,
    /// 界面上选中的那一行（电台列表里的 ▸ 标记）。**不发给服务端。**
    ///
    /// 刻意**不叫 primary**：服务端也有一个"主频率"的概念，指的是发话人发射时
    /// 用的那个频率，它决定了当一个监听者同时订阅了一对耦合频率的两端时，
    /// 包头的 `freq_khz` 填哪一个。两件毫不相干的事，而把 Task 3 接到 Task 11 的人
    /// 一定会想把它们连起来。判错的代价很具体：恰好在"双订阅 + 交叉耦合"这个
    /// 情况下，音频会显示在错误的电台行上。
    pub selected: bool,
}

/// 一次全量声明，连同客户端自己夹掉的部分。
///
/// 两者一起返回而不是分两个方法，是因为它们必须出自同一次计算：
/// 分开算的话，"发出去的"和"报上去的"会在下一次开关变动时悄悄对不上。
#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub sub: Sub,
    /// 因为超过 [`MAX_XC_PAIRS`] 而**没有发出去**的耦合对。上层要把它们显示出来
    /// ——一个设好了耦合却不生效、又不知道为什么的管制员，正是整个重写要逃离的
    /// 那类故障。
    pub dropped_xc: Vec<[u32; 2]>,
}

/// 一组频率。
#[derive(Debug, Clone, Default)]
pub struct RadioStack {
    radios: Vec<Radio>,
}

impl RadioStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// 加一个频率。已存在则什么也不做。新加的频率默认接收。
    pub fn add(&mut self, freq_khz: u32) {
        if self.radios.iter().any(|r| r.freq_khz == freq_khz) {
            return;
        }
        let selected = self.radios.is_empty();
        self.radios.push(Radio {
            freq_khz,
            rx: true,
            tx: false,
            xc: false,
            gain: 1.0,
            selected,
        });
    }

    pub fn remove(&mut self, freq_khz: u32) {
        self.radios.retain(|r| r.freq_khz != freq_khz);
        // 移掉的正好是选中的那一行时，把标记交给第一个。
        if !self.radios.iter().any(|r| r.selected) {
            if let Some(first) = self.radios.first_mut() {
                first.selected = true;
            }
        }
    }

    /// 关掉 RX 同时清掉 TX 和 XC：不接收，那么发送和耦合都没有意义。
    pub fn set_rx(&mut self, freq_khz: u32, on: bool) {
        if let Some(r) = self.get_mut(freq_khz) {
            r.rx = on;
            if !on {
                r.tx = false;
                r.xc = false;
            }
        }
    }

    /// 打开 TX 强制打开 RX：没有只发不收的电台。
    ///
    /// 关掉 TX 同时清掉 XC，而这一条是**必要**的而不是顺手：耦合的前提是两端
    /// 都能发（服务端的 `normaliseXC` 拿**授权后**的 TX 集合当依据），
    /// 一个关了 TX 却还标着 XC 的电台只会产出一对必然被拒的耦合。
    pub fn set_tx(&mut self, freq_khz: u32, on: bool) {
        if let Some(r) = self.get_mut(freq_khz) {
            r.tx = on;
            if on {
                r.rx = true;
            } else {
                r.xc = false;
            }
        }
    }

    /// 打开 XC 强制打开 RX 和 TX。
    pub fn set_xc(&mut self, freq_khz: u32, on: bool) {
        if let Some(r) = self.get_mut(freq_khz) {
            r.xc = on;
            if on {
                r.rx = true;
                r.tx = true;
            }
        }
    }

    pub fn set_gain(&mut self, freq_khz: u32, gain: f32) {
        if let Some(r) = self.get_mut(freq_khz) {
            r.gain = gain.clamp(0.0, 2.0);
        }
    }

    /// 把界面上的选中标记移到某个频率。**不产生任何线上效果。**
    pub fn set_selected(&mut self, freq_khz: u32) {
        for r in &mut self.radios {
            r.selected = r.freq_khz == freq_khz;
        }
    }

    pub fn radios(&self) -> &[Radio] {
        &self.radios
    }

    /// 把当前开关状态折算成一次全量订阅声明。
    ///
    /// 这是栈与网络之间唯一的接口：UI 改开关，这里产出完整意图，
    /// 由 `session` 发出去。没有"这次改了哪一个"的增量路径。
    ///
    /// 三份列表都**排序**：声明因此是这组开关的一个确定函数，
    /// "这次的声明和上次一样吗"才判得出来，而服务端的 ACK 本来也是排好序的。
    pub fn to_subscription(&self) -> Declaration {
        let mut rx: Vec<u32> = self.radios.iter().filter(|r| r.rx).map(|r| r.freq_khz).collect();
        let mut tx: Vec<u32> = self.radios.iter().filter(|r| r.tx).map(|r| r.freq_khz).collect();
        rx.sort_unstable();
        tx.sort_unstable();

        // XC 的语义是"这些频率互相转发"，所以要产生每一对组合。
        // 一个频率没法和自己耦合，所以单独一个 XC 频率不产生任何配对。
        //
        // 先排序再两两组合，于是每一对天然是 [小, 大]：服务端的 normaliseXC 在
        // **判完之后**才把次序颠倒的对调过来，所以被拒的对是按客户端发的原样回来的
        // ——自己先规范化，`rejected_xc` 才对得上自己的声明。
        let mut coupled: Vec<u32> =
            self.radios.iter().filter(|r| r.xc).map(|r| r.freq_khz).collect();
        coupled.sort_unstable();
        let mut xc = Vec::new();
        for (i, a) in coupled.iter().enumerate() {
            for b in &coupled[i + 1..] {
                xc.push([*a, *b]);
            }
        }
        let dropped_xc = if xc.len() > MAX_XC_PAIRS { xc.split_off(MAX_XC_PAIRS) } else { Vec::new() };

        Declaration { sub: Sub { rx, tx, xc }, dropped_xc }
    }

    fn get_mut(&mut self, freq_khz: u32) -> Option<&mut Radio> {
        self.radios.iter_mut().find(|r| r.freq_khz == freq_khz)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack_with(freqs: &[u32]) -> RadioStack {
        let mut s = RadioStack::new();
        for f in freqs {
            s.add(*f);
        }
        s
    }

    fn radio(s: &RadioStack, freq: u32) -> &Radio {
        s.radios().iter().find(|r| r.freq_khz == freq).expect("radio present")
    }

    // 以下三条耦合规则抄自 TrackAudio 的 radio.tsx，
    // 与 can-audio 的 controller/test_radiostack.py 一一对应。

    #[test]
    fn turning_rx_off_also_clears_tx_and_xc() {
        // 不接收，那么发送和耦合都没有意义。
        let mut s = stack_with(&[121_800]);
        s.set_xc(121_800, true);
        assert!(radio(&s, 121_800).tx, "xc should have forced tx on");

        s.set_rx(121_800, false);
        let r = radio(&s, 121_800);
        assert!(!r.rx && !r.tx && !r.xc, "clearing rx must clear tx and xc, got {r:?}");
    }

    #[test]
    fn turning_tx_on_forces_rx_on() {
        // 没有只发不收的电台。
        let mut s = stack_with(&[121_800]);
        s.set_rx(121_800, false);
        s.set_tx(121_800, true);
        assert!(radio(&s, 121_800).rx, "turning tx on must force rx on");
    }

    #[test]
    fn turning_xc_on_forces_both_rx_and_tx_on() {
        let mut s = stack_with(&[121_800]);
        s.set_rx(121_800, false);
        s.set_xc(121_800, true);
        let r = radio(&s, 121_800);
        assert!(r.rx && r.tx && r.xc, "xc must force rx and tx on, got {r:?}");
    }

    /// 修订件 L12：`set_tx(f, false)` 顺带清掉 `xc` 是**必要**的——耦合的前提是
    /// 两端都能发，一个关了 TX 却还标着 XC 的电台会让 `to_subscription` 产出
    /// 一对服务端必然拒掉的耦合（`normaliseXC` 拿授权后的 TX 集合当依据）。
    /// 原实现是对的，但没有任何东西钉住它。
    #[test]
    fn turning_tx_off_also_clears_xc() {
        let mut s = stack_with(&[121_800]);
        s.set_xc(121_800, true);
        s.set_tx(121_800, false);
        let r = radio(&s, 121_800);
        assert!(!r.xc, "a radio that cannot transmit cannot be cross-coupled, got {r:?}");
    }

    #[test]
    fn a_new_radio_starts_receiving() {
        let s = stack_with(&[118_000]);
        let r = radio(&s, 118_000);
        assert!(r.rx, "a freshly added radio should receive");
        assert!(!r.tx && !r.xc);
    }

    #[test]
    fn adding_the_same_frequency_twice_does_not_duplicate_it() {
        let mut s = RadioStack::new();
        s.add(118_000);
        s.add(118_000);
        assert_eq!(s.radios().len(), 1);
    }

    #[test]
    fn to_subscription_reflects_the_switches() {
        let mut s = stack_with(&[118_000, 121_800, 124_550]);
        s.set_tx(121_800, true);
        s.set_rx(124_550, false);

        let d = s.to_subscription();
        assert!(d.sub.rx.contains(&118_000));
        assert!(d.sub.rx.contains(&121_800));
        assert!(!d.sub.rx.contains(&124_550), "a radio with rx off must not be subscribed");
        assert_eq!(d.sub.tx, vec![121_800]);
    }

    #[test]
    fn to_subscription_pairs_every_cross_coupled_frequency() {
        // XC 是"这些频率互相转发"，所以要产生每一对组合。
        let mut s = stack_with(&[118_000, 121_800, 124_550]);
        s.set_xc(118_000, true);
        s.set_xc(121_800, true);
        s.set_xc(124_550, true);

        let d = s.to_subscription();
        assert_eq!(d.sub.xc.len(), 3, "three cross-coupled radios make three pairs, got {:?}", d.sub.xc);
    }

    #[test]
    fn a_single_cross_coupled_radio_produces_no_pairs() {
        // 一个频率没法和自己交叉耦合。
        let mut s = stack_with(&[118_000]);
        s.set_xc(118_000, true);
        assert!(s.to_subscription().sub.xc.is_empty());
    }

    #[test]
    fn removing_a_radio_drops_it_from_the_subscription() {
        let mut s = stack_with(&[118_000, 121_800]);
        s.remove(118_000);
        let d = s.to_subscription();
        assert_eq!(d.sub.rx, vec![121_800]);
    }

    // ——— 修订件 M4：耦合对要在客户端就夹住 ———

    /// 每一对都必须是 `[小, 大]`，而且整份声明有稳定顺序。
    ///
    /// 服务端的 `normaliseXC` 在**判完之后**才把 `p[0] > p[1]` 的对调过来，
    /// 所以被拒的对是**按客户端发的原样**回来的。客户端自己先规范化，
    /// `rejected_xc` 才对得上自己的声明。
    #[test]
    fn cross_couple_pairs_are_normalised_and_ordered() {
        let mut s = stack_with(&[124_550, 118_000, 121_800]);
        for f in [124_550, 118_000, 121_800] {
            s.set_xc(f, true);
        }
        let d = s.to_subscription();
        for p in &d.sub.xc {
            assert!(p[0] < p[1], "pair {p:?} must be ordered low-high");
        }
        assert_eq!(d.sub.xc, vec![[118_000, 121_800], [118_000, 124_550], [121_800, 124_550]]);
    }

    /// 超过 `MAX_XC_PAIRS` 的部分不上线，而且要报给上层。
    ///
    /// 17 个互相耦合的频率就是 C(17,2) = 136 对，而服务端在第 65 对上 `break`，
    /// 只把**接下来的 64 对**放进 `rejected_xc`——第 129 对往后是**静默丢弃**的。
    /// 不在客户端夹住，那 8 对就谁都不知道去哪了。
    #[test]
    fn cross_couple_pairs_beyond_the_server_limit_are_clamped_and_reported() {
        let freqs: Vec<u32> = (0..17).map(|i| 118_000 + i * 25).collect();
        let mut s = stack_with(&freqs);
        for f in &freqs {
            s.set_xc(*f, true);
        }
        let d = s.to_subscription();
        assert_eq!(d.sub.xc.len(), MAX_XC_PAIRS, "the declaration must not exceed the server limit");
        assert_eq!(d.dropped_xc.len(), 136 - MAX_XC_PAIRS, "everything clamped must be reported upward");
        // 夹掉的和发出去的合起来正好是全部意图，一对不多一对不少。
        assert_eq!(d.sub.xc.len() + d.dropped_xc.len(), 136);
        for p in &d.dropped_xc {
            assert!(!d.sub.xc.contains(p), "a pair cannot be both declared and dropped: {p:?}");
        }
    }

    #[test]
    fn a_declaration_within_the_limit_drops_nothing() {
        let mut s = stack_with(&[118_000, 121_800]);
        s.set_xc(118_000, true);
        s.set_xc(121_800, true);
        assert!(s.to_subscription().dropped_xc.is_empty());
    }

    // ——— 修订件 L13：命名陷阱 ———

    /// `Radio.selected` 是**界面标记**（电台列表里那个 ▸ 行），**不发给服务端**。
    /// 服务端的"主频率"是发话人发射时用的那个频率，决定了当一个监听者同时订阅
    /// 了一对耦合频率的两端时包头的 `freq_khz` 填哪一个——两件毫不相干的事。
    /// 字段叫 `selected` 就是为了让"把它俩连起来"这个诱惑消失。
    #[test]
    fn the_selected_marker_never_reaches_the_wire() {
        let mut s = stack_with(&[118_000, 121_800]);
        s.set_selected(121_800);
        let d = s.to_subscription();
        // Sub 只有 rx/tx/xc 三个字段，没有任何地方装得下"选中"这件事。
        assert_eq!(d.sub.rx, vec![118_000, 121_800]);
        assert!(radio(&s, 121_800).selected);
        assert!(!radio(&s, 118_000).selected);
    }

    #[test]
    fn removing_the_selected_radio_hands_the_marker_to_another() {
        let mut s = stack_with(&[118_000, 121_800]);
        assert!(radio(&s, 118_000).selected, "the first radio added starts selected");
        s.remove(118_000);
        assert!(radio(&s, 121_800).selected, "the marker must not vanish with the radio");
    }

    #[test]
    fn gain_is_clamped_to_a_sane_range() {
        let mut s = stack_with(&[118_000]);
        s.set_gain(118_000, 9.0);
        assert_eq!(radio(&s, 118_000).gain, 2.0);
        s.set_gain(118_000, -1.0);
        assert_eq!(radio(&s, 118_000).gain, 0.0);
    }
}
