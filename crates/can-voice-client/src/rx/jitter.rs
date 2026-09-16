//! 抖动缓冲。
//!
//! 按 `(speaker, freq)` 一个 —— 同频可能有多个发言者，各自网络路径不同，
//! 共用一个缓冲会让两个人的包互相当成对方的乱序。
//!
//! # 它靠什么吸收抖动
//!
//! **靠起播前攒够 [`JitterBuffer::target_depth`] 帧，不靠播放途中扣着不放。**
//! 生产里 `pop` 由声卡时钟以 20 ms 一次的固定节奏调用，而帧也以 50 包/秒到达，
//! 水位自然停在起播时攒下的那个高度。缓冲真被抽干只有一种可能——上游停了——
//! 而那时正确的输出是 [`Frame::Lost`]（交给 Opus 的 PLC），不是 `None`（静音）。
//!
//! # 它不看首帧标志
//!
//! [`JitterBuffer::push`] 只收 `last` 一个标志位，没有 `first`，这是有意的：
//! `SUB` 是全量声明、立即整体替换，所以管制员在别人说到一半时把一个频率加进台面，
//! **下一帧**就投给他，而那一帧没有首帧位；飞机飞进射程同理。等首帧的实现会让他
//! 一声不响，直到对方下一次按下 PTT，而服务端日志完全正常。
//!
//! # 水位会涨也会落，而且跨发言携带
//!
//! 涨得快、落得慢：一次欠载立刻加一格（20 ms），而退一格要一整段
//! [`SETTLED_FRAMES`] 帧没有欠载的发言。只涨不落的自适应等于没有自适应——
//! 一次抖动把水位顶到 120 ms，此后每一句话都多等 120 ms，网络好转也回不来。
//!
//! **学到的水位由调用方带进下一次发言**（`rx::mixer` 的 `learned`）：缓冲是一次
//! 发言一个，而起播只发生一次，不带的话自适应只在单次发言之内向上生效。
//!
//! # 起播也有超时
//!
//! 水位攒不满时最多等 [`START_TIMEOUT_TICKS`] 拍，然后拿手上这几帧起播。
//! 不设这道闸的话，"只到了一两帧、尾帧又丢了"的那一路会让 `pop` 永远返回
//! `None`：静音超时推不动，RX 灯常亮，mixer 里那条流永不回收。
//!
//! # 它自带静音超时
//!
//! `FLAG_LAST` 是尽力而为的优化，不是熄灯的机制——它走不可靠数据报会丢，而且
//! 服务端在听众飞出射程时**不打招呼就停发**，那种情况下它保证到不了。
//! 所以连续丢超过 [`MAX_CONSECUTIVE_LOST`] 帧就结束这次发言。

use std::collections::BTreeMap;

/// 起始深度：3 帧 × 20 ms = 60 ms。
pub const START_DEPTH: usize = 3;

/// 深度下限：2 帧 = 40 ms。
pub const MIN_DEPTH: usize = 2;

/// 深度上限：6 帧 = 120 ms。再深就是可感知的延迟了。
pub const MAX_DEPTH: usize = 6;

/// 连续丢多少帧就认为这次发言结束了。3 帧 = 60 ms 没有任何东西到达。
pub const MAX_CONSECUTIVE_LOST: usize = 3;

/// 一段发言放到这么多帧（50 帧 = 1 秒）而一次没欠载，才算"网络确实好了"。
///
/// 短句不算数：一串"收到"会把水位一路推到下限，而下一句长的立刻欠载。
pub const SETTLED_FRAMES: usize = 50;

/// 起播前最多等这么多拍（10 拍 = 200 ms）。
///
/// 等不到水位也要拿现有的帧起播。**这是那条永远亮着的 RX 灯的出路**：只到了
/// 一两帧、尾帧又丢了（或者听众此刻飞出射程，服务端不打招呼就停发）的时候，
/// 起播前的 `pop` 会一直返回 `None`，静音超时推不动，mixer 里那条流永不回收。
/// 帧到得这么慢的时候，多等下去也换不回一段连续的音频。
pub const START_TIMEOUT_TICKS: usize = 10;

/// 展开 16 位序号时给"比第一帧还早的迟到帧"留出的余量，
/// 免得 `u64` 在起点附近下溢。
const ROLLOVER_GUARD: u64 = 0x1_0000;

/// 从缓冲里取出的一帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// 一帧 Opus 数据。
    Audio(Vec<u8>),
    /// 这一帧丢了。交给解码器走 Opus 的丢包隐藏。
    Lost,
    /// 这次发言结束了。
    End,
}

/// 一个发言者在一个频率上的抖动缓冲。
///
/// 内部按**展开成 64 位的序号**存放，而不是原始的 `u16`。一次发言完全可能跨过
/// 16 位回绕点（`seq` 是每会话单调、不按发言重置的，21.8 分钟回绕一次），
/// 而 `BTreeMap<u16, _>` 会在那里把 0 排到 65534 前面，整段发言顺序全乱，
/// 日志里什么都看不出来。
#[derive(Debug)]
pub struct JitterBuffer {
    frames: BTreeMap<u64, Vec<u8>>,
    /// 见过的最高序号（展开后），也是展开新序号时的参照点。
    high: Option<u64>,
    /// 下一个该播的序号（展开后）。None 表示还没起播。
    next: Option<u64>,
    /// 尾帧的序号（展开后），收到后才知道。
    last: Option<u64>,
    finished: bool,
    /// 起播水位。**跨发言携带**：每次发言都从 60 ms 重新开始的话，自适应等于没有，
    /// 因为起播只发生一次而缓冲是一次发言一个。
    target_depth: usize,
    consecutive_lost: usize,
    /// 这一段里欠载过吗。欠载过就不许退水位——欠载正是水位被顶上去的理由。
    underran: bool,
    /// 这一段放了多少帧（含丢包隐藏的那些）。见 [`SETTLED_FRAMES`]。
    played: usize,
    /// 起播前空转了多少拍。见 [`START_TIMEOUT_TICKS`]。
    waiting: usize,
}

impl Default for JitterBuffer {
    fn default() -> Self {
        Self::with_target_depth(START_DEPTH)
    }
}

impl JitterBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// 用一个指定的起播水位建缓冲，夹在 [`MIN_DEPTH`]–[`MAX_DEPTH`] 之间。
    /// 上一次发言结束时的 [`JitterBuffer::target_depth`] 应当由调用方接着传进来。
    pub fn with_target_depth(target_depth: usize) -> Self {
        Self {
            frames: BTreeMap::new(),
            high: None,
            next: None,
            last: None,
            finished: false,
            target_depth: target_depth.clamp(MIN_DEPTH, MAX_DEPTH),
            consecutive_lost: 0,
            underran: false,
            played: 0,
            waiting: 0,
        }
    }

    /// 把一个 16 位序号展开成参照 `high` 的 64 位序号。
    fn extend(&self, seq: u16) -> u64 {
        match self.high {
            None => ROLLOVER_GUARD + u64::from(seq),
            Some(high) => {
                let high16 = high as u16;
                let forward = seq.wrapping_sub(high16);
                if forward < 0x8000 {
                    high + u64::from(forward)
                } else {
                    high - u64::from(high16.wrapping_sub(seq))
                }
            }
        }
    }

    /// 收下一帧。
    pub fn push(&mut self, seq: u16, payload: Vec<u8>, last: bool) {
        let ext = self.extend(seq);
        if last {
            self.last = Some(ext);
        }
        // 已经播过的序号是迟到帧，丢弃 —— 收下它会把播放指针拉回去，
        // 听感上是一小段音频重复。比较必须走展开后的序号，直接比 u16 会在
        // 回绕点把"更早"判成"更晚"。
        if let Some(next) = self.next {
            if ext < next {
                return;
            }
        }
        let is_highest = match self.high {
            None => true,
            Some(high) => ext > high,
        };
        if is_highest {
            self.high = Some(ext);
        }
        // 重复帧忽略。
        self.frames.entry(ext).or_insert(payload);

        // 上限保护：一个比实时更快的发送方不该把内存吃光。
        while self.frames.len() > MAX_DEPTH {
            let Some(&oldest) = self.frames.keys().next() else {
                break;
            };
            self.frames.remove(&oldest);
            // **只有起播之后才推进播放指针。** 起播之前推进它等于绕过水位等待，
            // 那正是它存在的理由。
            if let Some(next) = self.next {
                if oldest >= next {
                    self.next = Some(oldest + 1);
                }
            }
        }
    }

    /// 取这一拍该播的东西。
    ///
    /// 起播之前，水位没到就返回 `None`（调用方播静音）。**起播之后永远有东西返回**
    /// ——声卡每 20 ms 都要一帧，缺了就得是 `Lost` 交给 PLC，而不是让它去播静音。
    pub fn pop(&mut self) -> Option<Frame> {
        if self.finished {
            return None;
        }
        // 还没起播：等攒够水位。尾帧已到时不必再等 —— 那意味着不会再有更多数据了。
        if self.next.is_none() {
            if self.frames.len() < self.target_depth && self.last.is_none() {
                self.waiting += 1;
                // 等够了就拿手上这几帧起播。它们是真音频，播完自然走静音超时。
                if self.waiting <= START_TIMEOUT_TICKS || self.frames.is_empty() {
                    return None;
                }
            }
            self.next = self.frames.keys().next().copied();
        }
        let next = self.next?;

        if let Some(payload) = self.frames.remove(&next) {
            self.next = Some(next + 1);
            self.consecutive_lost = 0;
            self.played += 1;
            return Some(Frame::Audio(payload));
        }

        // 尾帧已经播过了 —— 这次发言到此为止。
        if let Some(last) = self.last {
            if next > last {
                return Some(self.finish());
            }
        }

        // 这一格是空的。缓冲里一个帧都没有，说明上游停了（或者飞出了射程，
        // 服务端**不打招呼就停发**），这就是欠载：水位不够，下一次发言攒深一点。
        if self.frames.is_empty() {
            self.target_depth = (self.target_depth + 1).min(MAX_DEPTH);
            self.underran = true;
        }

        self.next = Some(next + 1);
        self.consecutive_lost += 1;
        self.played += 1;
        if self.consecutive_lost > MAX_CONSECUTIVE_LOST {
            // 静音超时。这是 `FLAG_LAST` 丢了、或者服务端悄悄停发时唯一的出路。
            return Some(self.finish());
        }
        Some(Frame::Lost)
    }

    /// 收尾，并把这一段学到的东西结算进水位。
    ///
    /// **涨得快、落得慢**：一次欠载立刻加一格，而退一格要一整段（[`SETTLED_FRAMES`]
    /// 帧）没有欠载。反过来的话，水位会在一条时好时坏的线路上来回震荡，
    /// 而每一次调低都是下一次卡顿。
    fn finish(&mut self) -> Frame {
        self.finished = true;
        if !self.underran && self.played >= SETTLED_FRAMES {
            self.target_depth = self.target_depth.saturating_sub(1).max(MIN_DEPTH);
        }
        Frame::End
    }

    /// 当前缓存的帧数（**占用量**，不是水位）。
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    /// 起播水位（**不是**占用量）。发言结束后把它传给下一个缓冲。
    pub fn target_depth(&self) -> usize {
        self.target_depth
    }

    /// 这次发言是否已经放完。
    pub fn finished(&self) -> bool {
        self.finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_voice_proto::wire::{seq_cmp, SeqOrder};

    fn push_n(j: &mut JitterBuffer, seqs: &[u16]) {
        for s in seqs {
            j.push(*s, vec![*s as u8], false);
        }
    }

    /// 一直 pop 到不再是音频为止，返回放出来的载荷首字节。
    fn drain(j: &mut JitterBuffer) -> Vec<u8> {
        let mut got = Vec::new();
        while let Some(Frame::Audio(b)) = j.pop() {
            got.push(b[0]);
        }
        got
    }

    #[test]
    fn nothing_comes_out_until_the_buffer_has_filled() {
        // 缓冲的意义就是先攒一点再放 —— 立刻输出等于没有抖动缓冲。
        let mut j = JitterBuffer::new();
        j.push(0, vec![0], false);
        assert!(
            j.pop().is_none(),
            "a single frame must not be released immediately"
        );
    }

    /// **注意这里期望的是全部五帧，不是前三帧。**
    ///
    /// 计划里这条测试期望 `[0,1,2]`，也就是"播放途中也要把水位扣住"。那是错的，
    /// 而且和它旁边那条 `out_of_order_frames_are_reordered` 在算术上互相矛盾：
    /// 任何"len >= t 才放"的实现停下时 len 恰好是 t-1，这条要求 t=3
    /// （5 帧放 3 留 2），那条要求 t=2（4 帧放 3 留 1）。同一个 t 不可能两者都是。
    ///
    /// 真正的语义是：**吸收抖动靠的是起播前攒够 target_depth，不是播放途中扣着不放。**
    /// 生产里 pop 由声卡时钟以 20 ms 一次的固定节奏调用，而帧也以 50 包/秒到达，
    /// 水位自然停在起播时攒下的那个高度。缓冲真被抽干只有一种可能——上游停了——
    /// 那时正确的输出是 `Lost`（交给 PLC），而不是 `None`（静音）。
    /// 扣着不放恰恰会把它声称要防的"断音"做出来。
    #[test]
    fn every_buffered_frame_plays_in_sequence_order() {
        let mut j = JitterBuffer::new();
        push_n(&mut j, &[0, 1, 2, 3, 4]);
        assert_eq!(drain(&mut j), vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn out_of_order_frames_are_reordered() {
        let mut j = JitterBuffer::new();
        push_n(&mut j, &[2, 0, 1, 3]);
        assert_eq!(
            drain(&mut j),
            vec![0, 1, 2, 3],
            "out-of-order arrival must be reordered"
        );
    }

    #[test]
    fn a_gap_yields_a_lost_frame_rather_than_stalling() {
        // 丢了一个包就永远等它，会让整条音频卡死。
        let mut j = JitterBuffer::new();
        push_n(&mut j, &[0, 1, 3, 4, 5, 6]);
        let mut got = Vec::new();
        for _ in 0..4 {
            match j.pop() {
                Some(Frame::Audio(b)) => got.push(Some(b[0])),
                Some(Frame::Lost) => got.push(None),
                _ => break,
            }
        }
        assert_eq!(
            got,
            vec![Some(0), Some(1), None, Some(3)],
            "seq 2 is missing and must surface as Frame::Lost, got {got:?}"
        );
    }

    #[test]
    fn a_frame_that_arrives_too_late_is_dropped() {
        let mut j = JitterBuffer::new();
        push_n(&mut j, &[0, 1, 2, 3, 4, 5]);
        j.pop();
        j.pop();
        j.pop();
        // seq 0 现在已经播过了，再来一份必须被丢弃而不是把播放指针拉回去。
        j.push(0, vec![99], false);
        match j.pop() {
            Some(Frame::Audio(b)) => assert_ne!(b[0], 99, "a late frame must not rewind playback"),
            other => panic!("pop returned {other:?}"),
        }
    }

    #[test]
    fn the_last_flag_ends_the_talkspurt_after_the_buffer_drains() {
        let mut j = JitterBuffer::new();
        j.push(0, vec![0], false);
        j.push(1, vec![1], false);
        j.push(2, vec![2], true);

        let mut audio = 0;
        loop {
            match j.pop() {
                Some(Frame::Audio(_)) => audio += 1,
                Some(Frame::End) => break,
                Some(Frame::Lost) => {}
                None => panic!("buffer stalled before delivering End"),
            }
        }
        assert_eq!(audio, 3, "every buffered frame must play before End");
        assert!(j.finished());
    }

    #[test]
    fn the_buffer_does_not_grow_without_bound() {
        let mut j = JitterBuffer::new();
        for s in 0..1000u16 {
            j.push(s, vec![0], false);
        }
        assert!(
            j.depth() <= MAX_DEPTH,
            "depth grew to {} frames; a sender faster than realtime must not exhaust memory",
            j.depth()
        );
    }

    #[test]
    fn a_duplicate_frame_is_ignored() {
        let mut j = JitterBuffer::new();
        push_n(&mut j, &[0, 0, 1, 2]);
        assert_eq!(
            drain(&mut j),
            vec![0, 1, 2],
            "a repeated seq must not be played twice"
        );
    }

    // ——— seq 回绕（修订件 §八.1）———

    /// `seq` 是**每会话单调、不按发言重置**的，所以一次发言完全可能跨过回绕点。
    /// 20 毫秒一帧时每 21.8 分钟回绕一次，而一次发言只有几秒——概率小，但一场
    /// 13 小时的值班里会撞上几十次回绕，总有一次落在发言中间。
    ///
    /// 用 `BTreeMap<u16, _>` 直接按数值排序的实现会在这里把 0 排到 65534 前面，
    /// 整段发言的顺序全乱，而日志里什么都看不出来。
    #[test]
    fn a_talkspurt_that_straddles_the_wraparound_still_plays_in_order() {
        let mut j = JitterBuffer::new();
        push_n(&mut j, &[65534, 65535, 0, 1]);
        assert_eq!(drain(&mut j), vec![65534u16 as u8, 65535u16 as u8, 0, 1]);
    }

    #[test]
    fn a_late_frame_from_before_the_wraparound_is_dropped() {
        let mut j = JitterBuffer::new();
        push_n(&mut j, &[65534, 65535, 0, 1]);
        j.pop();
        j.pop();
        j.pop();
        // 现在 next 是 1（已播 65534、65535、0）。65533 是更早的迟到帧。
        j.push(65533, vec![99], false);
        match j.pop() {
            Some(Frame::Audio(b)) => {
                assert_ne!(b[0], 99, "a pre-wrap late frame must not rewind playback")
            }
            other => panic!("pop returned {other:?}"),
        }
    }

    /// 起播判据是**水位**，不是首帧标志。
    ///
    /// 这个缓冲根本收不到 `FLAG_FIRST`——`push` 只有 `last` 一个标志位，这是有意的
    /// （修订件 N5）：`SUB` 是全量声明、立即整体替换，所以管制员在别人说到一半时
    /// 把一个频率加进台面，下一帧就投给他，而那一帧没有首帧位。等首帧的实现会让
    /// 他一声不响，直到对方下一次按下 PTT。
    #[test]
    fn playback_starts_from_whatever_arrives_first_not_from_a_flag() {
        let mut j = JitterBuffer::new();
        // 从一段发言的中间接进来：序号是任意的，没有任何"这是开头"的信号。
        push_n(&mut j, &[5000, 5001, 5002]);
        assert_eq!(
            drain(&mut j).len(),
            3,
            "a mid-talkspurt join must play, not wait for a first flag"
        );
    }

    // ——— M9：连续丢帧结束发言（也就是静音超时）———

    /// `FLAG_LAST` 是尽力而为的优化，不是熄灯的机制：它走不可靠数据报会丢，
    /// 而且服务端在听众飞出射程时**不打招呼就停发**——那种情况下它保证到不了。
    /// 只认尾帧的实现会让 RX 指示灯亮一整条会话。
    ///
    /// 所以缓冲自己带静音超时：连续丢超过 `MAX_CONSECUTIVE_LOST` 帧就结束这次发言。
    #[test]
    fn a_talkspurt_ends_on_silence_even_without_a_last_flag() {
        let mut j = JitterBuffer::new();
        push_n(&mut j, &[0, 1, 2]);
        let mut kinds = Vec::new();
        for _ in 0..20 {
            match j.pop() {
                Some(f) => kinds.push(f),
                None => break,
            }
        }
        assert!(
            j.finished(),
            "the buffer must time out instead of waiting for a last flag forever"
        );
        assert_eq!(kinds.last(), Some(&Frame::End));
        let lost = kinds.iter().filter(|f| **f == Frame::Lost).count();
        assert_eq!(
            lost, MAX_CONSECUTIVE_LOST,
            "exactly the losses that triggered the timeout, got {kinds:?}"
        );
    }

    #[test]
    fn a_gap_shorter_than_the_timeout_does_not_end_the_talkspurt() {
        let mut j = JitterBuffer::new();
        // 0,1,2 然后缺 3,4，再来 5 —— 两帧的空档，短于超时。
        push_n(&mut j, &[0, 1, 2, 5, 6, 7]);
        let mut got = Vec::new();
        for _ in 0..8 {
            match j.pop() {
                Some(f) => got.push(f),
                None => break,
            }
        }
        assert!(
            !j.finished(),
            "a two-frame gap must not end the talkspurt, got {got:?}"
        );
        assert!(
            got.contains(&Frame::Audio(vec![5])),
            "playback must resume after the gap"
        );
    }

    // ——— M10：深度自适应，以及 L2 的改名 ———

    /// 只涨不落的自适应等于没有自适应：一次抖动把水位顶到 120 ms，此后每一句话
    /// 都多等 120 ms，网络好转也回不来。一段**干净**的发言之后要退一格。
    /// 一帧一帧地喂，一拍一拍地取——生产里就是这样：包以 50/秒到达，
    /// `pop` 由声卡时钟每 20 ms 调一次。一口气 push 几十帧会撞上防暴涨的上限，
    /// 测出来的就不是同一件事了。
    fn play_talkspurt(j: &mut JitterBuffer, frames: usize) {
        for s in 0..frames {
            j.push(s as u16, vec![1], s == frames - 1);
            // 起播前水位还没到，这几拍取不出东西来，正常。
            if s >= j.target_depth() {
                j.pop();
            }
        }
        while !j.finished() && j.pop().is_some() {}
    }

    #[test]
    fn a_clean_talkspurt_lowers_the_water_level_for_the_next_one() {
        let mut j = JitterBuffer::with_target_depth(MAX_DEPTH);
        play_talkspurt(&mut j, SETTLED_FRAMES + MAX_DEPTH);
        assert!(j.finished());
        assert_eq!(
            j.target_depth(),
            MAX_DEPTH - 1,
            "a clean talkspurt must give one frame back"
        );
    }

    /// 欠载过的那一段不退：它正是水位被顶上去的理由。
    #[test]
    fn a_talkspurt_that_underran_keeps_the_deeper_water_level() {
        let mut j = JitterBuffer::with_target_depth(START_DEPTH);
        // 够长，不是被"短句不退水位"那条挡回去的。中间有一次抖动尖峰：
        // 连着四拍什么都没到，缓冲被抽干——**那才是欠载**（漏一帧不是：
        // 缓冲还有货，只是缺了中间那一格）。
        let total = SETTLED_FRAMES + START_DEPTH;
        let spike = total / 2;
        let mut seq = 0usize;
        for tick in 0..total + 8 {
            if !(spike..spike + 4).contains(&tick) && seq < total {
                j.push(seq as u16, vec![1], seq == total - 1);
                seq += 1;
            }
            if tick >= START_DEPTH {
                j.pop();
            }
        }
        while !j.finished() && j.pop().is_some() {}
        assert!(j.finished());
        assert!(
            j.target_depth() > START_DEPTH,
            "an underrun must not be rewarded with a shallower buffer, got {}",
            j.target_depth()
        );
    }

    /// 一句"收到"不足以判断网络好了。短发言退水位的话，水位会被一串短句
    /// 一路推到下限，而下一句长的立刻欠载。
    #[test]
    fn a_short_talkspurt_does_not_lower_the_water_level() {
        let mut j = JitterBuffer::with_target_depth(MAX_DEPTH);
        for s in 0..5u16 {
            j.push(s, vec![1], s == 4);
        }
        while j.pop().is_some() {}
        assert!(j.finished());
        assert_eq!(j.target_depth(), MAX_DEPTH);
    }

    /// **水位永远攒不满的那一路要自己走出来。**
    ///
    /// 只到了一两帧、尾帧又丢了（或者听众此刻飞出射程，服务端不打招呼就停发）
    /// 时，起播前的 `pop` 会一直返回 `None`：静音超时推不动，RX 灯常亮，
    /// mixer 里那条流永不回收。等够了就拿现有的这几帧起播——它们是真音频，
    /// 播完自然走静音超时那条路。
    #[test]
    fn a_stream_that_never_fills_starts_anyway_and_then_ends() {
        let mut j = JitterBuffer::with_target_depth(MAX_DEPTH);
        j.push(0, vec![7], false);
        for tick in 0..START_TIMEOUT_TICKS {
            assert_eq!(j.pop(), None, "tick {tick} should still be waiting");
        }
        assert_eq!(j.pop(), Some(Frame::Audio(vec![7])), "it must start anyway");

        // 起播之后静音超时接手，这次发言收尾，流才回收得掉。
        let mut guard = 0;
        while !j.finished() {
            j.pop();
            guard += 1;
            assert!(guard < 100, "the talkspurt never ended");
        }
    }

    /// L2：`depth()` 是**占用量**，`target_depth()` 是**起播水位**。
    /// 原实现里一个字段叫 `depth`、一个方法也叫 `depth()`，八行里一个词两个意思。
    #[test]
    fn occupancy_and_water_level_are_different_things() {
        let mut j = JitterBuffer::new();
        assert_eq!(j.depth(), 0);
        assert_eq!(j.target_depth(), START_DEPTH);
        j.push(0, vec![0], false);
        assert_eq!(j.depth(), 1, "depth() is how many frames are held");
        assert_eq!(
            j.target_depth(),
            START_DEPTH,
            "the water level does not move just because a frame arrived"
        );
    }

    /// 欠载（缓冲空了还得出一帧）说明水位不够，下一次发言要攒得更深。
    #[test]
    fn an_underrun_deepens_the_water_level_for_next_time() {
        let mut j = JitterBuffer::new();
        push_n(&mut j, &[0, 1, 2]);
        for _ in 0..4 {
            j.pop();
        }
        assert!(
            j.target_depth() > START_DEPTH,
            "an underrun must deepen the buffer"
        );
        assert!(
            j.target_depth() <= MAX_DEPTH,
            "but never past the 120 ms ceiling"
        );
    }

    /// 深度是**跨发言**携带的：每次发言都从 60 ms 重新开始的话，自适应等于没有，
    /// 因为起播只发生一次，而缓冲是一次发言一个。
    #[test]
    fn the_water_level_is_carried_into_the_next_talkspurt() {
        let mut j = JitterBuffer::with_target_depth(MAX_DEPTH);
        assert_eq!(j.target_depth(), MAX_DEPTH);
        push_n(&mut j, &[0, 1, 2]);
        assert!(
            j.pop().is_none(),
            "a deeper buffer must wait longer before starting"
        );
    }

    #[test]
    fn the_water_level_stays_inside_the_spec_range() {
        assert_eq!(JitterBuffer::with_target_depth(0).target_depth(), MIN_DEPTH);
        assert_eq!(
            JitterBuffer::with_target_depth(99).target_depth(),
            MAX_DEPTH
        );
    }

    /// 尾帧已到时不必再等水位：不会再有更多数据了。
    #[test]
    fn a_last_flag_starts_playback_without_waiting_for_the_water_level() {
        let mut j = JitterBuffer::with_target_depth(MAX_DEPTH);
        j.push(0, vec![0], true);
        assert!(
            matches!(j.pop(), Some(Frame::Audio(_))),
            "a complete one-frame talkspurt must not wait for frames that will never come"
        );
    }

    /// `extend` 与 [`seq_cmp`] 必须对"谁在后面"给出同一个答案。
    ///
    /// 两处各写了一遍 16 位回绕算术（一处在 `wire`，一处在这里把序号展开成 64 位），
    /// 而它们分叉的话，缓冲会在回绕点附近按一套规则排序、按另一套规则丢迟到帧——
    /// 那种错只在 21.8 分钟一次的窗口里露头，本地必现不了。
    #[test]
    fn extension_agrees_with_the_protocol_ordering() {
        for high in [0u16, 1, 1000, 32767, 32768, 65534, 65535] {
            let mut j = JitterBuffer::new();
            j.push(high, vec![0], false);
            let high_ext = j.high.expect("high set by the first push");
            for delta in [1u16, 2, 100, 32767] {
                for seq in [high.wrapping_add(delta), high.wrapping_sub(delta)] {
                    let ext = j.extend(seq);
                    let by_ext = ext > high_ext;
                    let by_proto = matches!(seq_cmp(high, seq), SeqOrder::After);
                    assert_eq!(
                        by_ext, by_proto,
                        "high={high} seq={seq}: extension says after={by_ext}, seq_cmp says after={by_proto}"
                    );
                }
            }
        }
    }
}
