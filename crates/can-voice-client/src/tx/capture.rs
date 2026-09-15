//! 发送侧的成帧与编码：把采集到的样本切成 20 毫秒的 Opus 帧。
//!
//! 和 [`crate::rx::mixer`] 一样，这里**不碰声卡**：采集回调把样本 `push` 进来，
//! 20 毫秒的时钟来 `tick`。所以它在没有麦克风的机器上也测得了。
//!
//! # `seq` 的三条契约都落在这里
//!
//! 服务端**原样转发 `seq`、从不重编号**，也没有任何检查——写错了没有一端会报错，
//! 症状是接收端的抖动缓冲莫名其妙地丢帧或卡住。三条：
//!
//! 1. **每编出一个 20 毫秒音频帧就加一。**
//! 2. **不按发言重置。** 按发言重置在不可靠数据报上是错的：首帧本来就可能丢，
//!    丢了之后接收端看到的是一个**倒退**的序号，和"一个很旧的乱序包"分不开，
//!    整段新发言会被当成过期的丢掉。
//! 3. **不说话的时候不走。** 没有帧，就没有号。

use super::{Encoder, Error};
use crate::rx::decode::FRAME_SAMPLES;
use can_voice_proto::wire::{Header, FLAG_FIRST, FLAG_LAST, VERSION};

/// 采集缓冲最多攒多少帧。
///
/// 上游（声卡回调）比 20 毫秒的时钟快时，多出来的要丢掉而不是攒着：攒着只会让
/// 发出去的音频越来越落后于实际说话的时间，而那是不可能追回来的。
pub const MAX_BUFFERED_FRAMES: usize = 4;

/// 一帧待发的音频。
///
/// 它**不带频率**：同一帧要发到每一个 TX 频率上去，每份带同一个 `seq`
/// （见 [`TxFrame::header`]）。扇出是调用方的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxFrame {
    pub opus: Vec<u8>,
    pub seq: u16,
    pub first: bool,
    pub last: bool,
}

impl TxFrame {
    /// 造这一帧发往某个频率的上行包头。
    ///
    /// **`qual` 与 `speaker` 恒为 0。** 这两个值客户端说了不算，服务端会填；
    /// 填了别的数不会被拒，只会被**忽略**——所以没有任何东西会提醒写错的人。
    pub fn header(&self, freq_khz: u32) -> Header {
        let mut flags = 0u8;
        if self.first {
            flags |= FLAG_FIRST;
        }
        if self.last {
            flags |= FLAG_LAST;
        }
        Header {
            ver: VERSION,
            flags,
            qual: 0,
            seq: self.seq,
            freq_khz,
            speaker: 0,
        }
    }

    /// 把包头和载荷拼成一个数据报。
    pub fn datagram(&self, freq_khz: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(13 + self.opus.len());
        self.header(freq_khz).write_to(&mut out);
        out.extend_from_slice(&self.opus);
        out
    }
}

/// 采集 → 成帧 → Opus 编码。
pub struct TxPipeline {
    encoder: Encoder,
    pending: Vec<i16>,
    seq: u16,
    speaking: bool,
    /// 下一帧是不是这次发言的首帧。
    at_start: bool,
}

impl TxPipeline {
    pub fn new() -> Result<Self, Error> {
        Ok(Self {
            encoder: Encoder::new()?,
            pending: Vec::with_capacity(FRAME_SAMPLES * MAX_BUFFERED_FRAMES),
            seq: 0,
            speaking: false,
            at_start: false,
        })
    }

    /// 采集回调把样本塞进来。必须已经是 48 kHz 单声道
    /// （设备采样率的适配在 [`crate::audio`] 里做）。
    pub fn push(&mut self, samples: &[i16]) {
        self.pending.extend_from_slice(samples);
        let cap = FRAME_SAMPLES * MAX_BUFFERED_FRAMES;
        if self.pending.len() > cap {
            // 丢**最旧**的：落后的音频追不回来，而最新的才是他正在说的话。
            let drop = self.pending.len() - cap;
            self.pending.drain(..drop);
        }
    }

    /// 这一拍要发什么。`ptt` 是当前的按键状态。
    pub fn tick(&mut self, ptt: bool) -> Option<TxFrame> {
        if !ptt {
            if !self.speaking {
                // 空闲时采到的音频要丢掉，不能攒着：攒着的话，按下 PTT 的第一瞬间
                // 发出去的是按下之前那几百毫秒的房间噪音。
                self.pending.clear();
                return None;
            }
            // 松开了：把剩下的补齐成一整帧发出去，标尾帧。
            //
            // 补齐成**真音频**而不是发一个空包，是为了让尾帧照常占一个序号——
            // 接收端的抖动缓冲不必为"一个没有载荷的序号"开一条特例。
            self.speaking = false;
            self.pending.resize(FRAME_SAMPLES, 0);
            let first = std::mem::take(&mut self.at_start);
            return self.emit(first, true);
        }

        if !self.speaking {
            self.speaking = true;
            self.at_start = true;
        }
        if self.pending.len() < FRAME_SAMPLES {
            return None;
        }
        let first = std::mem::take(&mut self.at_start);
        self.emit(first, false)
    }

    /// 取出一帧、编码、推进序号。
    fn emit(&mut self, first: bool, last: bool) -> Option<TxFrame> {
        let frame: Vec<i16> = self.pending.drain(..FRAME_SAMPLES).collect();
        match self.encoder.encode(&frame) {
            Ok(opus) => {
                let seq = self.seq;
                // 回绕而不是饱和：20 毫秒一帧时每 21.8 分钟连续发话绕一圈，
                // 而接收端只看差值。
                self.seq = self.seq.wrapping_add(1);
                Some(TxFrame {
                    opus,
                    seq,
                    first,
                    last,
                })
            }
            Err(e) => {
                // 编不出来只该丢这一帧。**序号不推进**——推进的话接收端会把它
                // 当成一个丢了的帧去做 PLC，而实际上它从来没存在过。
                tracing::debug!(error = %e, "a capture frame did not encode; dropping it");
                None
            }
        }
    }

    /// 当前攒着几帧。
    pub fn buffered_frames(&self) -> usize {
        self.pending.len() / FRAME_SAMPLES
    }

    /// 是否正在发话。
    pub fn is_speaking(&self) -> bool {
        self.speaking
    }

    #[cfg(test)]
    fn set_seq_for_test(&mut self, seq: u16) {
        self.seq = seq;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rx::decode::FRAME_SAMPLES;
    use can_voice_proto::wire::{FLAG_FIRST, FLAG_LAST, VERSION};

    fn tone(n: usize) -> Vec<i16> {
        (0..n)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                (0.4 * 32767.0 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()) as i16
            })
            .collect()
    }

    fn pipeline() -> TxPipeline {
        TxPipeline::new().expect("encoder")
    }

    #[test]
    fn an_idle_pipeline_sends_nothing() {
        let mut p = pipeline();
        p.push(&tone(FRAME_SAMPLES * 3));
        assert!(p.tick(false).is_none());
    }

    /// 不说话时采到的音频要**丢掉**，不能攒着。攒着的话，按下 PTT 的第一瞬间
    /// 发出去的是按下之前那几百毫秒的房间噪音——对方先听到一段无关的杂音，
    /// 而且它还占着这次发言开头的位置。
    #[test]
    fn audio_captured_while_idle_is_discarded() {
        let mut p = pipeline();
        p.push(&tone(FRAME_SAMPLES * 3));
        p.tick(false);
        // 现在按下 PTT，但一个新样本都还没来。
        assert!(
            p.tick(true).is_none(),
            "the stale buffer must not be transmitted"
        );
    }

    #[test]
    fn pressing_ptt_marks_the_first_frame() {
        let mut p = pipeline();
        p.push(&tone(FRAME_SAMPLES));
        let f = p.tick(true).expect("a frame");
        assert!(
            f.first,
            "the first frame of a talkspurt carries the first flag"
        );
        assert!(!f.last);

        p.push(&tone(FRAME_SAMPLES));
        let f2 = p.tick(true).expect("a frame");
        assert!(!f2.first, "only the first one");
    }

    #[test]
    fn releasing_ptt_produces_a_final_frame_with_the_last_flag() {
        let mut p = pipeline();
        p.push(&tone(FRAME_SAMPLES));
        p.tick(true).expect("a frame");
        let f = p.tick(false).expect("a closing frame");
        assert!(f.last, "releasing ptt must close the talkspurt on the wire");
    }

    /// 尾帧是**补齐到整帧的真音频**，不是一个空包。这样它照常占一个序号，
    /// 接收端的抖动缓冲不必为"一个没有载荷的序号"开一条特例。
    #[test]
    fn the_closing_frame_is_a_real_audio_frame() {
        let mut p = pipeline();
        p.push(&tone(FRAME_SAMPLES / 2)); // 半帧
        assert!(p.tick(true).is_none(), "half a frame is not enough to send");
        let f = p.tick(false).expect("a closing frame");
        assert!(
            !f.opus.is_empty(),
            "the closing frame carries the padded remainder"
        );
    }

    #[test]
    fn a_frame_needs_a_whole_twenty_milliseconds_of_audio() {
        let mut p = pipeline();
        p.push(&tone(FRAME_SAMPLES - 1));
        assert!(p.tick(true).is_none());
        p.push(&tone(1));
        assert!(p.tick(true).is_some());
    }

    // ——— seq 的契约（协议里最容易写错的一条）———

    #[test]
    fn seq_advances_by_one_per_encoded_frame() {
        let mut p = pipeline();
        let mut seqs = Vec::new();
        for _ in 0..4 {
            p.push(&tone(FRAME_SAMPLES));
            seqs.push(p.tick(true).expect("a frame").seq);
        }
        assert_eq!(seqs, vec![seqs[0], seqs[0] + 1, seqs[0] + 2, seqs[0] + 3]);
    }

    /// **不说话的时候序号不走。** "每编出一个 20 毫秒音频帧就加一"——没有帧，
    /// 就没有号。让它跟着时钟走的话，两次发言之间会凭空出现一个巨大的空档，
    /// 而接收端把发言之内的空档当丢包。
    #[test]
    fn seq_does_not_advance_while_silent() {
        let mut p = pipeline();
        p.push(&tone(FRAME_SAMPLES));
        let before = p.tick(true).expect("a frame").seq;
        let closing = p.tick(false).expect("closing").seq;
        for _ in 0..50 {
            p.push(&tone(FRAME_SAMPLES));
            assert!(p.tick(false).is_none());
        }
        p.push(&tone(FRAME_SAMPLES));
        let after = p.tick(true).expect("a frame").seq;
        assert_eq!(
            after,
            closing + 1,
            "seq jumped from {closing} to {after} across the silence"
        );
        assert_eq!(closing, before + 1);
    }

    /// **不按发言重置。** 按发言重置在不可靠数据报上是错的：首帧本来就可能丢，
    /// 丢了之后接收端看到的是一个**倒退**的序号，和"一个很旧的乱序包"分不开，
    /// 整段新发言会被当成过期的丢掉。
    #[test]
    fn seq_does_not_reset_between_talkspurts() {
        let mut p = pipeline();
        p.push(&tone(FRAME_SAMPLES));
        p.tick(true);
        let end_of_first = p.tick(false).expect("closing").seq;

        p.push(&tone(FRAME_SAMPLES));
        let start_of_second = p.tick(true).expect("a frame").seq;
        assert!(
            start_of_second > end_of_first,
            "a new talkspurt must not rewind seq"
        );
    }

    #[test]
    fn seq_wraps_at_sixteen_bits() {
        let mut p = pipeline();
        p.set_seq_for_test(u16::MAX);
        p.push(&tone(FRAME_SAMPLES));
        assert_eq!(p.tick(true).expect("a frame").seq, u16::MAX);
        p.push(&tone(FRAME_SAMPLES));
        assert_eq!(
            p.tick(true).expect("a frame").seq,
            0,
            "seq must wrap, not saturate"
        );
    }

    // ——— 上行包头的契约 ———

    /// 上行**必须**填 `qual = 0`、`speaker = 0`：这两个值客户端说了不算，
    /// 服务端会填。填了别的数不会被拒，只会被忽略——所以这里没有任何东西会
    /// 提醒写错的人，钉子只能在这儿。
    #[test]
    fn an_uplink_header_leaves_qual_and_speaker_to_the_server() {
        let f = TxFrame {
            opus: vec![1, 2, 3],
            seq: 42,
            first: true,
            last: false,
        };
        let h = f.header(121_800);
        assert_eq!(h.ver, VERSION);
        assert_eq!(
            h.qual, 0,
            "the client does not get to claim a signal quality"
        );
        assert_eq!(
            h.speaker, 0,
            "the client does not get to claim a session id"
        );
        assert_eq!(h.seq, 42);
        assert_eq!(h.freq_khz, 121_800);
        assert_eq!(h.flags, FLAG_FIRST);
    }

    #[test]
    fn a_closing_header_carries_only_the_last_flag() {
        let f = TxFrame {
            opus: vec![],
            seq: 1,
            first: false,
            last: true,
        };
        assert_eq!(f.header(118_000).flags, FLAG_LAST);
    }

    /// 同一个音频帧发到多个 TX 频率上时，**每一份带同一个 `seq`**。
    /// 服务端检查不了这件事：一个声明了耦合却只发一份的客户端在另一个频率上
    /// 完全静默，而两端日志都正常。
    #[test]
    fn one_frame_fans_out_to_every_tx_frequency_with_the_same_seq() {
        let f = TxFrame {
            opus: vec![9],
            seq: 7,
            first: false,
            last: false,
        };
        let headers: Vec<_> = [118_000u32, 121_800, 124_550]
            .iter()
            .map(|f2| f.header(*f2))
            .collect();
        assert!(
            headers.iter().all(|h| h.seq == 7),
            "every copy carries the same seq"
        );
        let freqs: Vec<u32> = headers.iter().map(|h| h.freq_khz).collect();
        assert_eq!(freqs, vec![118_000, 121_800, 124_550]);
    }

    // ——— 上游比时钟快时不许把内存吃光 ———

    #[test]
    fn the_capture_buffer_is_bounded() {
        let mut p = pipeline();
        for _ in 0..500 {
            p.push(&tone(FRAME_SAMPLES));
        }
        assert!(
            p.buffered_frames() <= MAX_BUFFERED_FRAMES,
            "capture buffer grew to {} frames",
            p.buffered_frames()
        );
    }
}
