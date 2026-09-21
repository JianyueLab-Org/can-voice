//! 无线电栈模型：一个管制员的若干频率，每个带 RX/TX/XC 三个开关。
//!
//! 耦合规则抄自 TrackAudio 的 `radio.tsx`，与 can-audio 的
//! `controller/radiostack.py` 一一对应。这里没有 I/O、没有网络、没有 UI ——
//! 和 Python 版一样，这是全库最值得单测的部分。
//!
//! 放在共享库而不是 controller 应用里：xpc/msfs 用的是它的退化版（单频率）。

use can_voice_proto::control::Sub;
use std::collections::BTreeMap;

/// 服务端一份声明里最多处理的耦合对数，等于 `router.go` 的 `maxXCPairs`。
///
/// **客户端必须自己夹到这个数以内。** 服务端在第 65 对上停下，并且只把
/// **接下来的 64 对**放进 `rejected_xc`（回报本身也得有界，否则 ACK 超过
/// 64 KiB 就根本发不出去）——**第 129 对往后是静默丢弃的**。
/// 17 个互相耦合的频率就是 C(17,2) = 136 对，而 `max_tx` 默认 32，
/// 所以这不是一个够不着的数。
pub const MAX_XC_PAIRS: usize = 64;

/// 一个频率及其开关。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
    /// 静音。**`gain` 照旧留着**——取消静音要回到用户原来调的那个位置，
    /// 而不是回到 1.0。静音是一个开关，不是一个音量。
    ///
    /// `#[serde(default)]`：老的设置文件里没有这一项。
    #[serde(default)]
    pub muted: bool,
    /// 这个频率上那个席位的呼号。查不到就是空的。
    ///
    /// `#[serde(default)]`：老的设置文件里没有这一项，缺了它整份电台栈就读不
    /// 回来——而读不回来的表现是"一升级，我的频率全没了"。
    #[serde(default)]
    pub callsign: String,
}

impl Radio {
    /// 这个频率此刻该用多大音量播。静音就是 0。
    ///
    /// **播放层要的是这个数，不是 `gain`**：直接拿 `gain` 去播，静音就只是一个
    /// 画在界面上的图标。
    pub fn effective_gain(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            self.gain
        }
    }
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

/// 台面相对服务端发射上限（READY 的 `max_tx`）的处境。
///
/// 给界面在**声明之前**说话用：服务端对超额的声明只是把多出来的 TX 拒掉，界面上
/// 看到的是事后冒出来的"发射被拒"——而 READY 把限额带下来，躲的正是这个次序。
///
/// **"开哪一格会超额"在这里算，不在前端算。** 开 XC 会顺带开 TX，前端自己数的话
/// 就得把这条耦合规则再写一遍；而规则写成两份，迟早有一份跟不上。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TxBudget {
    /// 服务端给的上限。
    pub max_tx: u32,
    /// 这份台面此刻会声明几个 TX。**可以已经超过 `max_tx`**：存下来的台面在重连时
    /// 整份重放，而这一次的上限可能比存的时候低。
    pub declared: usize,
    /// 在这些频率上打开 TX，声明里的 TX 就会超过上限。
    pub tx_over: Vec<u32>,
    /// 在这些频率上打开 XC，声明里的 TX 就会超过上限。
    pub xc_over: Vec<u32>,
}

/// 一组频率。
#[derive(Debug, Clone)]
pub struct RadioStack {
    radios: Vec<Radio>,
    /// 数据源上本人正在管的那个席位频率。**它不许删。**
    locked_khz: Option<u32>,
    /// 此刻允不允许发射。
    ///
    /// **默认放行。** 飞行员端（xpc / msfs）没有"在不在席位上"这件事，把默认值
    /// 设成 `false` 会让他们一句话也说不出去；只有管制端会按数据源上的席位把它
    /// 关掉。所以 [`Default`] 是手写的，不是 derive 的。
    transmit_allowed: bool,
    /// 每个频率上"不在席位却想发射"被拒了多少次，**第一次也算在内**。
    ///
    /// 不在这里存"报过没有"而存次数，是因为要报的就是这个数：第一次报出去，
    /// 后面的只往上加，等状态真的变了（重新可以发射、或者这个电台被删掉）
    /// 再把总数报一次。只闷掉不计数的话，这件事就彻底看不见了——而一份测试员
    /// 的日志里 7545 行有 7536 行是这一句，说明**有东西在一秒钟里问五遍**，
    /// 那个"有东西"至今没找到。
    ///
    /// **按频率分开数**，不是一个全局计数：现场是两个频率各自在被问，
    /// 合成一个数就看不出是哪一个。
    ///
    /// **TX 和 XC 共用一个计数器。** 它们成对到来（现场是相隔约 37 微秒的一对），
    /// 出自同一个调用方的同一个动作；分成两个计数器只会把同一件事的次数劈成两半，
    /// 还要多报一倍的行数。代价是一对里的第二句（XC）连第一次都不报——可以接受：
    /// 要认的是"谁在反复按"，不是"按的是哪一格"。
    ///
    /// `BTreeMap` 而不是 `HashMap`：汇报时按频率从小到大出，日志才是稳定的。
    refusals: BTreeMap<u32, u32>,
}

impl Default for RadioStack {
    fn default() -> Self {
        Self {
            radios: Vec::new(),
            locked_khz: None,
            transmit_allowed: true,
            refusals: BTreeMap::new(),
        }
    }
}

impl RadioStack {
    pub fn new() -> Self {
        Self::default()
    }

    /// 加一个频率。已存在则什么也不做。新加的三个开关都关——和原来 voice 一样，
    /// 加进来不等于开始收。
    pub fn add(&mut self, freq_khz: u32) {
        self.add_named(freq_khz, "");
    }

    /// 加一个频率，并记下它是谁的席位。
    ///
    /// 已经在栈里的频率**补呼号而不是跳过**：频率常常先被手工加进来，过一轮
    /// 数据源才知道那上面是谁。
    pub fn add_named(&mut self, freq_khz: u32, callsign: &str) {
        if let Some(r) = self.get_mut(freq_khz) {
            if !callsign.is_empty() {
                r.callsign = callsign.to_string();
            }
            return;
        }
        let selected = self.radios.is_empty();
        self.radios.push(Radio {
            freq_khz,
            rx: false,
            tx: false,
            xc: false,
            gain: 1.0,
            selected,
            muted: false,
            callsign: callsign.to_string(),
        });
        self.radios.sort_by_key(|r| r.freq_khz);
    }

    /// 把"正在管的那个频率"标出来。`None` = 此刻不在管任何席位。
    pub fn set_locked(&mut self, freq_khz: Option<u32>) {
        self.locked_khz = freq_khz;
    }

    /// 这个频率是不是本人正在管的那个席位。
    pub fn is_locked(&self, freq_khz: u32) -> bool {
        self.locked_khz == Some(freq_khz)
    }

    /// 允不允许发射。界面靠它把 TX / XC 画灰。
    pub fn transmit_allowed(&self) -> bool {
        self.transmit_allowed
    }

    /// 开关发射权，返回**有没有真的丢掉过什么**。
    ///
    /// 关掉时把每个电台的 TX / XC 都清掉：只把按钮画灰是不够的，一个下了席位却
    /// 还标着 TX 的电台会在下一次声明里照样把 TX 发上去。返回值是给上层说话用的
    /// ——"你已经不在席位上了，发射已关闭"这句话只有真的关掉了什么才该说。
    ///
    /// 重新打开时**不恢复**任何 TX：恢复的话，一个人在两个席位之间换班会突然在
    /// 上一个席位的频率上具备发射能力，而他并没有按过任何东西。
    pub fn set_transmit_allowed(&mut self, allowed: bool) -> bool {
        self.transmit_allowed = allowed;
        if allowed {
            // 状态变了：把这一段里压下来的拒绝一次性报出去，然后重新开始数。
            self.report_refusals(None);
            return false;
        }
        let mut dropped = false;
        for r in &mut self.radios {
            if r.tx || r.xc {
                dropped = true;
            }
            r.tx = false;
            r.xc = false;
        }
        dropped
    }

    /// 删一个频率，返回删掉了没有。
    ///
    /// **正在管的那个席位频率删不掉**：删掉它的人还坐在席位上，而飞行员在那个
    /// 频率上叫他他听不见，两边都以为对方在。界面上那个按钮本来就该是灰的，
    /// 所以这里只记一条日志，不当成错误往上报。
    pub fn remove(&mut self, freq_khz: u32) -> bool {
        if self.is_locked(freq_khz) {
            tracing::info!(
                freq_khz,
                "this is the frequency of the position being staffed, refusing to remove it"
            );
            return false;
        }
        let before = self.radios.len();
        self.radios.retain(|r| r.freq_khz != freq_khz);
        // 电台没了，它的计数器也不该留着：留着的话，下一次汇报会报出一个
        // 已经不存在的频率。
        if self.radios.len() != before {
            self.report_refusals(Some(freq_khz));
        }
        // 移掉的正好是选中的那一行时，把标记交给第一个。
        if !self.radios.iter().any(|r| r.selected) {
            if let Some(first) = self.radios.first_mut() {
                first.selected = true;
            }
        }
        self.radios.len() != before
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
        if on && !self.transmit_allowed {
            if self.note_refusal(freq_khz) {
                tracing::info!(
                    freq_khz,
                    "not staffing a position on the datafeed, refusing to turn TX on"
                );
            }
            return;
        }
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
        if on && !self.transmit_allowed {
            if self.note_refusal(freq_khz) {
                tracing::info!(
                    freq_khz,
                    "not staffing a position on the datafeed, refusing to turn XC on"
                );
            }
            return;
        }
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

    /// 静音 / 取消静音。**不产生任何线上效果**：包照收，只是不播出来。
    ///
    /// 退订才是线上的事，而那是 RX 开关。两者刻意分开：一个临时插话的频率
    /// 静音掉就行，退订它会让下一次有人叫你时连灯都不亮。
    pub fn set_muted(&mut self, freq_khz: u32, on: bool) {
        if let Some(r) = self.get_mut(freq_khz) {
            r.muted = on;
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
        let mut rx: Vec<u32> = self
            .radios
            .iter()
            .filter(|r| r.rx)
            .map(|r| r.freq_khz)
            .collect();
        let mut tx: Vec<u32> = self
            .radios
            .iter()
            .filter(|r| r.tx)
            .map(|r| r.freq_khz)
            .collect();
        rx.sort_unstable();
        tx.sort_unstable();

        // XC 的语义是"这些频率互相转发"，所以要产生每一对组合。
        // 一个频率没法和自己耦合，所以单独一个 XC 频率不产生任何配对。
        //
        // 先排序再两两组合，于是每一对天然是 [小, 大]：服务端的 normaliseXC 在
        // **判完之后**才把次序颠倒的对调过来，所以被拒的对是按客户端发的原样回来的
        // ——自己先规范化，`rejected_xc` 才对得上自己的声明。
        let mut coupled: Vec<u32> = self
            .radios
            .iter()
            .filter(|r| r.xc)
            .map(|r| r.freq_khz)
            .collect();
        coupled.sort_unstable();
        let mut xc = Vec::new();
        for (i, a) in coupled.iter().enumerate() {
            for b in &coupled[i + 1..] {
                xc.push([*a, *b]);
            }
        }
        let dropped_xc = if xc.len() > MAX_XC_PAIRS {
            xc.split_off(MAX_XC_PAIRS)
        } else {
            Vec::new()
        };

        Declaration {
            sub: Sub { rx, tx, xc },
            dropped_xc,
        }
    }

    /// 对着一个发射上限，看这份台面的处境。见 [`TxBudget`]。
    ///
    /// 每一格都是**真的在一份副本上按一下再数**，而不是照着"开 XC 会开 TX"推：
    /// 那样推就是把耦合规则抄了第二份。数的是 [`Self::to_subscription`] 里的 TX，
    /// 也就是真正会发出去的那一份。
    ///
    /// 只标**会让 TX 变多**的那几格：已经开着的开关再按一次什么也不多，不在席位上
    /// 时开关本来就按不动——那是另一句话，不该再叠一句"会超额"。
    pub fn tx_budget(&self, max_tx: u32) -> TxBudget {
        let declared = self.declared_tx();
        // 不在席位上时每一格都按不动，两张表必然是空的（`off_duty_nothing_is_
        // flagged_as_over_the_limit` 钉的就是这个）。**要提前返回，不能照常试按**：
        // 试按会在副本上真的走一遍"拒绝"那条路，而界面每隔几百毫秒读一次快照，
        // 于是每读一次就按出 2N 次拒绝。试按不是按——它既不该进日志，也不该算进
        // 计数（副本上加的那一笔还会随副本一起丢掉，把真实次数也弄脏）。
        if !self.transmit_allowed {
            return TxBudget {
                max_tx,
                declared,
                tx_over: Vec::new(),
                xc_over: Vec::new(),
            };
        }
        let limit = usize::try_from(max_tx).unwrap_or(usize::MAX);
        let over = |press: fn(&mut RadioStack, u32)| -> Vec<u32> {
            self.radios
                .iter()
                .map(|r| r.freq_khz)
                .filter(|&f| {
                    let mut after = self.clone();
                    press(&mut after, f);
                    let n = after.declared_tx();
                    n > declared && n > limit
                })
                .collect()
        };
        TxBudget {
            max_tx,
            declared,
            tx_over: over(|s, f| s.set_tx(f, true)),
            xc_over: over(|s, f| s.set_xc(f, true)),
        }
    }

    /// 这个频率上被压掉、还没报出去的拒绝次数。**第一次那一条不算**——它已经
    /// 出现在日志里了。没被拒过就是 0。
    pub fn suppressed_refusals(&self, freq_khz: u32) -> u32 {
        self.refusals
            .get(&freq_khz)
            .map_or(0, |n| n.saturating_sub(1))
    }

    /// 记一次被拒的发射，返回**该不该把它写进日志**——只有一段里的第一次该。
    fn note_refusal(&mut self, freq_khz: u32) -> bool {
        let n = self.refusals.entry(freq_khz).or_insert(0);
        *n += 1;
        *n == 1
    }

    /// 把某个频率（`None` = 全部）压下来的拒绝次数报出来一次，然后忘掉。
    ///
    /// 只压掉了 0 次就什么也不说：第一次那一条已经把话讲完了。
    fn report_refusals(&mut self, freq_khz: Option<u32>) {
        let freqs: Vec<u32> = match freq_khz {
            Some(f) => self.refusals.keys().copied().filter(|k| *k == f).collect(),
            None => self.refusals.keys().copied().collect(),
        };
        for f in freqs {
            let suppressed = self.refusals.remove(&f).unwrap_or(0).saturating_sub(1);
            if suppressed > 0 {
                tracing::info!(
                    freq_khz = f,
                    suppressed,
                    "further transmit attempts were refused while not staffing a position"
                );
            }
        }
    }

    fn declared_tx(&self) -> usize {
        self.to_subscription().sub.tx.len()
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
            s.set_rx(*f, true);
        }
        s
    }

    fn radio(s: &RadioStack, freq: u32) -> &Radio {
        s.radios()
            .iter()
            .find(|r| r.freq_khz == freq)
            .expect("radio present")
    }

    /// **正在管的那个席位频率不许删。**
    ///
    /// 删掉它的人还坐在席位上，而飞行员在那个频率上叫他，他听不见——两边都
    /// 以为对方在。按钮该画灰，但状态本身也得拦一道：热键、脚本、以及下一个
    /// 接线的人都够得着这个方法。
    #[test]
    fn the_frequency_of_the_position_i_am_staffing_cannot_be_removed() {
        let mut s = stack_with(&[118_350, 121_800]);
        s.set_locked(Some(118_350));

        assert!(!s.remove(118_350));
        assert_eq!(s.radios().len(), 2);
        // 别的照删不误。
        assert!(s.remove(121_800));
        assert_eq!(s.radios().len(), 1);
    }

    /// **不在席位上就不许发射。**
    ///
    /// 只把按钮画灰是不够的：状态本身要真的关掉，否则一个下了席位却还标着 TX
    /// 的电台，在下一次声明里照样把 TX 发上去。
    #[test]
    fn off_duty_clears_every_transmit_and_refuses_new_ones() {
        let mut s = stack_with(&[118_350, 121_800]);
        s.set_tx(118_350, true);
        s.set_xc(121_800, true);

        assert!(s.set_transmit_allowed(false));

        assert!(!radio(&s, 118_350).tx);
        assert!(!radio(&s, 121_800).tx);
        assert!(!radio(&s, 121_800).xc);
        // 关着的时候再想打开也不行。
        s.set_tx(118_350, true);
        s.set_xc(118_350, true);
        assert!(!radio(&s, 118_350).tx);
        assert!(!radio(&s, 118_350).xc);
        // RX 不受影响：下了席位还是可以听。
        assert!(radio(&s, 118_350).rx);
    }

    /// 重新上席位**不自动把 TX 恢复回来**。
    ///
    /// 恢复的话，一个人在两个席位之间换班时会突然在上一个席位的频率上具备发射
    /// 能力，而他并没有按过任何东西。该开哪一个由数据源上的席位频率决定。
    #[test]
    fn coming_back_on_duty_does_not_silently_restore_transmit() {
        let mut s = stack_with(&[118_350]);
        s.set_tx(118_350, true);
        s.set_transmit_allowed(false);

        // 第二次关是空操作：没有东西可丢了。
        assert!(!s.set_transmit_allowed(false));
        s.set_transmit_allowed(true);

        assert!(!radio(&s, 118_350).tx);
        s.set_tx(118_350, true);
        assert!(radio(&s, 118_350).tx);
    }

    /// **单频静音记着原来的音量。**
    ///
    /// 把音量拉到 0 也能不出声，但那样取消静音就回不到原来那个位置了——
    /// 用户得重新找一遍他调了半天的那个刻度。静音是一个开关，不是一个音量。
    #[test]
    fn muting_one_frequency_remembers_the_volume_it_had() {
        let mut s = stack_with(&[118_350]);
        s.set_gain(118_350, 0.4);

        s.set_muted(118_350, true);
        assert!(radio(&s, 118_350).muted);
        assert_eq!(radio(&s, 118_350).effective_gain(), 0.0);
        // 静音期间调音量，调的是解除之后要回到的那个数。
        s.set_gain(118_350, 0.8);
        assert_eq!(radio(&s, 118_350).effective_gain(), 0.0);

        s.set_muted(118_350, false);
        assert_eq!(radio(&s, 118_350).effective_gain(), 0.8);
    }

    /// 频率上那个人是谁，电台行上要认得出来。
    ///
    /// 只有一个数字的电台行读起来是"121.800"，而管制员要找的是"ZSPD_TWR"。
    #[test]
    fn a_radio_remembers_whose_frequency_it_is() {
        let mut s = RadioStack::new();
        s.add_named(118_350, "ZSPD_TWR");
        assert_eq!(radio(&s, 118_350).callsign, "ZSPD_TWR");

        // 已经在栈里的频率后来才查到呼号：补上，而不是当成"已存在"什么都不做。
        s.add(121_800);
        s.add_named(121_800, "ZSPD_GND");
        assert_eq!(radio(&s, 121_800).callsign, "ZSPD_GND");
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
        assert!(
            !r.rx && !r.tx && !r.xc,
            "clearing rx must clear tx and xc, got {r:?}"
        );
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
        assert!(
            r.rx && r.tx && r.xc,
            "xc must force rx and tx on, got {r:?}"
        );
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
        assert!(
            !r.xc,
            "a radio that cannot transmit cannot be cross-coupled, got {r:?}"
        );
    }

    #[test]
    fn a_new_radio_starts_with_every_switch_off() {
        let mut s = RadioStack::new();
        s.add(118_000);
        let r = radio(&s, 118_000);
        assert!(!r.rx && !r.tx && !r.xc);
    }

    #[test]
    fn radios_are_kept_in_frequency_order() {
        let mut s = RadioStack::new();
        s.add(136_000);
        s.add(118_000);
        s.add(121_800);
        let freqs: Vec<u32> = s.radios().iter().map(|r| r.freq_khz).collect();
        assert_eq!(freqs, vec![118_000, 121_800, 136_000]);
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
        assert!(
            !d.sub.rx.contains(&124_550),
            "a radio with rx off must not be subscribed"
        );
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
        assert_eq!(
            d.sub.xc.len(),
            3,
            "three cross-coupled radios make three pairs, got {:?}",
            d.sub.xc
        );
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
        assert_eq!(
            d.sub.xc,
            vec![[118_000, 121_800], [118_000, 124_550], [121_800, 124_550]]
        );
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
        assert_eq!(
            d.sub.xc.len(),
            MAX_XC_PAIRS,
            "the declaration must not exceed the server limit"
        );
        assert_eq!(
            d.dropped_xc.len(),
            136 - MAX_XC_PAIRS,
            "everything clamped must be reported upward"
        );
        // 夹掉的和发出去的合起来正好是全部意图，一对不多一对不少。
        assert_eq!(d.sub.xc.len() + d.dropped_xc.len(), 136);
        for p in &d.dropped_xc {
            assert!(
                !d.sub.xc.contains(p),
                "a pair cannot be both declared and dropped: {p:?}"
            );
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
        assert!(
            radio(&s, 118_000).selected,
            "the first radio added starts selected"
        );
        s.remove(118_000);
        assert!(
            radio(&s, 121_800).selected,
            "the marker must not vanish with the radio"
        );
    }

    // ——— 服务端的发射上限（max_tx）：要在声明之前说 ———

    #[test]
    fn below_the_tx_limit_no_switch_is_flagged() {
        let mut s = stack_with(&[118_000, 121_800, 124_550]);
        s.set_tx(118_000, true);

        let b = s.tx_budget(3);

        assert_eq!((b.max_tx, b.declared), (3, 1));
        assert!(b.tx_over.is_empty() && b.xc_over.is_empty(), "{b:?}");
    }

    /// 到了上限，再开哪一个会超额要在**按下去之前**标出来——服务端超额时只是把
    /// 多出来的拒掉，界面上看到的是事后冒出来的"发射被拒"。
    #[test]
    fn at_the_tx_limit_every_switch_that_would_add_a_transmit_is_flagged() {
        let mut s = stack_with(&[118_000, 121_800, 124_550]);
        s.set_tx(118_000, true);
        s.set_tx(121_800, true);

        let b = s.tx_budget(2);

        assert_eq!(b.declared, 2);
        assert_eq!(b.tx_over, vec![124_550]);
        assert_eq!(b.xc_over, vec![124_550]);
    }

    /// **开 XC 会顺带开 TX，所以它也占一个。** 这正是这件事不能在前端数的原因：
    /// 只看 TX 开关的话，XC 那一格就是一条没人提示的超额路。
    ///
    /// 反过来，TX 已经开着的那一行再开 XC 不多占——标上它只会让人以为 XC 超额了。
    #[test]
    fn cross_coupling_counts_against_the_limit_because_it_forces_transmit() {
        let mut s = stack_with(&[118_000, 121_800]);
        s.set_tx(118_000, true);

        let b = s.tx_budget(1);

        assert_eq!(b.xc_over, vec![121_800]);
        assert!(!b.xc_over.contains(&118_000), "{b:?}");
    }

    /// **存下来的台面在重连时整份重放，而这一次的上限可能比存的时候低。**
    ///
    /// 超额的是整份台面，不是哪一行：已经开着的开关再按一次什么也不多，所以只标
    /// 会再往上加的那几格，总数和上限一起交出去让界面说"超了几个"。
    #[test]
    fn a_stack_already_over_the_limit_reports_the_count_and_flags_only_additions() {
        let mut s = stack_with(&[118_000, 121_800, 124_550]);
        s.set_tx(118_000, true);
        s.set_xc(121_800, true);

        let b = s.tx_budget(1);

        assert_eq!((b.max_tx, b.declared), (1, 2));
        assert_eq!(b.tx_over, vec![124_550]);
        // 118.000 的 TX 已经开着，再开 XC 不多占。
        assert_eq!(b.xc_over, vec![124_550]);
    }

    /// 不在席位上时 TX / XC 本来就开不了，那是另一句话（"你不在席位上"），
    /// 不该再叠一句"会超额"。
    #[test]
    fn off_duty_nothing_is_flagged_as_over_the_limit() {
        let mut s = stack_with(&[118_000, 121_800]);
        s.set_transmit_allowed(false);

        let b = s.tx_budget(0);

        assert_eq!(b.declared, 0);
        assert!(b.tx_over.is_empty() && b.xc_over.is_empty(), "{b:?}");
    }

    // ——— 拒绝发射的日志不许刷屏 ———

    /// 把一段代码期间的日志收下来。
    ///
    /// `with_default` 是**按线程**装的，所以并行跑的测试互不干扰；装全局的那个
    /// （`init`）一个进程只许装一次，第二个测试就会 panic。
    fn capture(f: impl FnOnce()) -> String {
        use std::io::Write;
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Default)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("sink").extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Sink {
            type Writer = Sink;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let sink = Sink::default();
        let sub = tracing_subscriber::fmt()
            .with_writer(sink.clone())
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(sub, f);
        let bytes = sink.0.lock().expect("sink").clone();
        String::from_utf8(bytes).expect("utf-8 log")
    }

    fn count_of(log: &str, needle: &str) -> usize {
        log.matches(needle).count()
    }

    const REFUSED_TX: &str = "refusing to turn TX on";
    const REFUSED_XC: &str = "refusing to turn XC on";
    const SUMMARY: &str = "further transmit attempts were refused";

    /// **一个频率只报第一次，后面的只记数。**
    ///
    /// 测试员那份日志里 7545 行有 7536 行是这两句，把唯一一条 ERROR 之前的历史
    /// 全部挤出了轮转——日志被刷爆的代价不是磁盘，是查不了问题。
    #[test]
    fn the_first_refusal_is_reported_and_the_rest_are_only_counted() {
        let mut s = stack_with(&[119_250]);
        s.set_transmit_allowed(false);

        let log = capture(|| {
            for _ in 0..500 {
                s.set_tx(119_250, true);
                s.set_xc(119_250, true);
            }
        });

        assert_eq!(count_of(&log, REFUSED_TX), 1, "log was:\n{log}");
        assert_eq!(
            count_of(&log, REFUSED_XC),
            0,
            "TX 和 XC 共用一个计数器，成对到来的第二句不该再报一次：\n{log}"
        );
        // 1000 次尝试，报了 1 次，压掉 999 次。
        assert_eq!(s.suppressed_refusals(119_250), 999);
    }

    /// 压掉的次数要准——那个数就是这次修的全部意义：它告诉下一个看日志的人
    /// "有东西在一秒钟里问五遍"，而只是把话闷掉会把这件事一起藏掉。
    #[test]
    fn the_suppressed_count_is_accurate() {
        let mut s = stack_with(&[119_250]);
        s.set_transmit_allowed(false);

        assert_eq!(s.suppressed_refusals(119_250), 0, "还没被拒过");
        s.set_tx(119_250, true);
        assert_eq!(s.suppressed_refusals(119_250), 0, "第一次是报出去的那一次");
        s.set_xc(119_250, true);
        assert_eq!(s.suppressed_refusals(119_250), 1);
        for _ in 0..7 {
            s.set_tx(119_250, true);
        }
        assert_eq!(s.suppressed_refusals(119_250), 8);
    }

    /// 重新可以发射时，把压掉的次数一次性报出来，然后忘掉。
    #[test]
    fn becoming_permitted_reports_the_count_once_and_forgets_it() {
        let mut s = stack_with(&[119_250]);
        s.set_transmit_allowed(false);
        for _ in 0..42 {
            s.set_tx(119_250, true);
        }

        let log = capture(|| {
            s.set_transmit_allowed(true);
            // 再放行一次不该再报一遍：已经报过了，也已经清干净了。
            s.set_transmit_allowed(true);
        });

        assert_eq!(count_of(&log, SUMMARY), 1, "log was:\n{log}");
        assert!(log.contains("suppressed=41"), "log was:\n{log}");
        assert_eq!(s.suppressed_refusals(119_250), 0);
    }

    /// 压掉 0 次就没什么可报的——第一次那句已经说完了。
    #[test]
    fn becoming_permitted_after_a_single_refusal_adds_no_summary() {
        let mut s = stack_with(&[119_250]);
        s.set_transmit_allowed(false);
        s.set_tx(119_250, true);

        let log = capture(|| {
            s.set_transmit_allowed(true);
        });

        assert_eq!(count_of(&log, SUMMARY), 0, "log was:\n{log}");
    }

    /// 第二次下席位是**新的一段**：重新报一次第一句，也重新数一次。
    #[test]
    fn a_second_outage_reports_again() {
        let mut s = stack_with(&[119_250]);

        let log = capture(|| {
            for _ in 0..2 {
                s.set_transmit_allowed(false);
                for _ in 0..5 {
                    s.set_tx(119_250, true);
                }
                s.set_transmit_allowed(true);
            }
        });

        assert_eq!(count_of(&log, REFUSED_TX), 2, "log was:\n{log}");
        assert_eq!(count_of(&log, SUMMARY), 2, "log was:\n{log}");
        assert_eq!(count_of(&log, "suppressed=4"), 2, "log was:\n{log}");
    }

    /// 删掉的频率不许留下一个计数器：它已经不存在了，而留着的那个数会在下一次
    /// 放行时报出一个没有电台的频率。
    #[test]
    fn removing_a_radio_takes_its_suppressed_count_with_it() {
        let mut s = stack_with(&[119_250, 124_550]);
        s.set_transmit_allowed(false);
        for _ in 0..4 {
            s.set_tx(119_250, true);
        }

        let log = capture(|| {
            assert!(s.remove(119_250));
        });

        assert_eq!(count_of(&log, SUMMARY), 1, "log was:\n{log}");
        assert!(log.contains("suppressed=3"), "log was:\n{log}");
        assert_eq!(s.suppressed_refusals(119_250), 0);

        // 报过就没了：之后放行不该再提它一次。
        let log = capture(|| {
            s.set_transmit_allowed(true);
        });
        assert_eq!(count_of(&log, SUMMARY), 0, "log was:\n{log}");
    }

    /// **两个频率分开数。** 测试员那份日志里就是两个（119.250 三千多次、
    /// 124.550 五百多次），合成一个数就看不出是哪一个在被问。
    #[test]
    fn two_frequencies_are_counted_separately() {
        let mut s = stack_with(&[119_250, 124_550]);
        s.set_transmit_allowed(false);

        let log = capture(|| {
            for _ in 0..10 {
                s.set_tx(119_250, true);
            }
            for _ in 0..3 {
                s.set_tx(124_550, true);
            }
        });

        // 每个频率各报一次第一句。
        assert_eq!(count_of(&log, REFUSED_TX), 2, "log was:\n{log}");
        assert_eq!(s.suppressed_refusals(119_250), 9);
        assert_eq!(s.suppressed_refusals(124_550), 2);

        let log = capture(|| {
            s.set_transmit_allowed(true);
        });
        assert_eq!(count_of(&log, SUMMARY), 2, "log was:\n{log}");
        assert!(log.contains("suppressed=9"), "log was:\n{log}");
        assert!(log.contains("suppressed=2"), "log was:\n{log}");
    }

    /// **试按不是按。** `tx_budget` 在副本上把每一格都按一遍来数 TX，界面每隔
    /// 几百毫秒读一次快照——那些按下去的"拒绝"既不该进日志，也不该算进计数。
    #[test]
    fn probing_the_tx_budget_off_duty_neither_logs_nor_counts() {
        let mut s = stack_with(&[119_250, 124_550]);
        s.set_transmit_allowed(false);

        let log = capture(|| {
            for _ in 0..50 {
                let b = s.tx_budget(2);
                assert!(b.tx_over.is_empty() && b.xc_over.is_empty(), "{b:?}");
            }
        });

        assert_eq!(count_of(&log, REFUSED_TX), 0, "log was:\n{log}");
        assert_eq!(count_of(&log, REFUSED_XC), 0, "log was:\n{log}");
        assert_eq!(s.suppressed_refusals(119_250), 0);
        assert_eq!(s.suppressed_refusals(124_550), 0);
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
