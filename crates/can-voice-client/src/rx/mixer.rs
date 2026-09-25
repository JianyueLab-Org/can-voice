//! 接收侧的混音器：把收到的数据报变成一帧可以送进声卡的 PCM。
//!
//! 这是 §9.3 那一整块的落点，也是**把 `jitter`、`decode`、`mix` 三个模块真正接起来
//! 的地方**——在它之前，那三个模块都实现了、测过了，但接在空气上。
//!
//! # 它是纯逻辑的，不碰声卡
//!
//! [`RxMixer::feed`] 由网络那一侧喂，[`RxMixer::tick`] 由 20 毫秒的音频时钟拉，
//! 两边都不认识 cpal。这样它在没有声卡的机器上也测得了——CI 就是那种机器——
//! 而 cpal 回调那一侧只剩一次 memcpy。
//!
//! # tick 永远出一帧
//!
//! 输出接的是声卡时钟，它每 20 毫秒都要一帧，不管有没有人在说话。没人说话时
//! 出的是静音；有人说话但缓冲还没攒够时也是静音；缺了帧出的是 PLC。
//! **任何一条"这一拍没东西可放"的路径都必须产出 960 个采样**，
//! 否则声卡拿到的是一个短缓冲，听感是咔哒声。

use super::agc::Agc;
use super::decode::{Decoder, FRAME_SAMPLES};
use super::jitter::{self, Frame, JitterBuffer};
use super::mix::{apply_quality, interfere, mix_into, NoiseGen};
use can_voice_proto::wire::Header;
use std::collections::HashMap;

/// 一帧的时长，秒。`RxEvent::End` 的 `secs` 由帧数乘它得出。
const FRAME_SECS: f32 = 0.02;

/// 每频率音量的上下界。
const MAX_GAIN: f32 = 2.0;

/// 混音器向上层报告的事。
#[derive(Debug, Clone, PartialEq)]
pub enum RxEvent {
    /// 某个 `(speaker, freq)` 上来了**第一个包**。
    ///
    /// 判据是"第一个包"，**不是 `FLAG_FIRST`**：`SUB` 是全量声明、立即整体替换，
    /// 所以管制员在别人说到一半时把一个频率加进台面，下一帧就投给他，
    /// 而那一帧没有首帧位；飞机飞进射程同理。
    Start { freq_khz: u32, speaker: u32 },
    /// 这次发言结束了。
    ///
    /// `secs` 是**音频时长**（帧数 × 20 ms），不是墙上时钟——想知道的是"这次通话
    /// 有多长音频"，而且那样才是确定性的、测得了的。
    End {
        freq_khz: u32,
        speaker: u32,
        frames: u32,
        secs: f32,
    },
}

/// 一个发言者在一个频率上的接收状态。
///
/// **抖动缓冲和解码器都是 per `(speaker, freq)` 的。** 解码器是有状态的，
/// 共用一个会让两个人的音频互相污染，而丢包隐藏更是完全依赖前一帧的状态。
struct RxStream {
    jitter: JitterBuffer,
    decoder: Decoder,
    /// 最近一包的信号质量。服务端每包都填，越低说明越接近射程边缘。
    qual: u8,
    frames: u32,
    agc: Agc,
}

/// 一条连接上所有接收流的混音器。
pub struct RxMixer {
    streams: HashMap<(u32, u32), RxStream>,
    /// 每个 `(speaker, freq)` 上一次发言结束时的起播水位。
    ///
    /// **缓冲是一次发言一个，而起播只发生一次**——不记在这里的话，自适应只在
    /// 单次发言之内向上生效，学到的东西一结束就丢，真实抖动网络上每次通话的
    /// 前几帧都在重新学。记的是水位这一个 `usize`，不是缓冲本身。
    learned: HashMap<(u32, u32), usize>,
    /// 每频率音量。缺省 1.0。
    gains: HashMap<u32, f32>,
    speaker_gains: HashMap<(u32, u32), f32>,
    noise: NoiseGen,
    /// 干扰音的拍频相位，跨 tick 连续——每帧从零开始的话，拍频会变成
    /// 每 20 毫秒一次的周期性咔哒，而不是一段连续的啸叫。
    phase: f32,
}

impl Default for RxMixer {
    fn default() -> Self {
        Self::new()
    }
}

impl RxMixer {
    pub fn new() -> Self {
        Self {
            streams: HashMap::new(),
            learned: HashMap::new(),
            gains: HashMap::new(),
            speaker_gains: HashMap::new(),
            // 固定种子：静噪是确定性的，测试才不会时灵时不灵。
            noise: NoiseGen::new(0x5EED),
            phase: 0.0,
        }
    }

    /// 收下一包音频。第一次见到这个 `(speaker, freq)` 时返回 [`RxEvent::Start`]。
    pub fn feed(&mut self, h: &Header, opus: &[u8]) -> Option<RxEvent> {
        let key = (h.speaker, h.freq_khz);
        let mut started = None;
        let stream = match self.streams.get_mut(&key) {
            Some(s) => s,
            None => {
                let decoder = match Decoder::new() {
                    Ok(d) => d,
                    Err(e) => {
                        // 起不来解码器是环境问题，不该让整条连接倒下：
                        // 别人的声音还听得见，比整个客户端打不开好。
                        tracing::warn!(error = %e, speaker = h.speaker, freq = h.freq_khz,
                            "could not create a decoder for this stream");
                        return None;
                    }
                };
                started = Some(RxEvent::Start {
                    freq_khz: h.freq_khz,
                    speaker: h.speaker,
                });
                let depth = self
                    .learned
                    .get(&key)
                    .copied()
                    .unwrap_or(jitter::START_DEPTH);
                self.streams.entry(key).or_insert(RxStream {
                    jitter: JitterBuffer::with_target_depth(depth),
                    decoder,
                    qual: h.qual,
                    frames: 0,
                    agc: Agc::default(),
                })
            }
        };
        stream.qual = h.qual;
        stream.jitter.push(h.seq, opus.to_vec(), h.is_last());
        started
    }

    /// 这一拍的 PCM（**恒为 [`FRAME_SAMPLES`] 个采样**）与这一拍产生的事件。
    pub fn tick(&mut self) -> (Vec<i16>, Vec<RxEvent>) {
        // 拆开借用：下面要同时可变地用 streams、noise 和 phase。
        let Self {
            streams,
            learned,
            gains,
            speaker_gains,
            noise,
            phase,
        } = self;

        let mut events = Vec::new();
        let mut finished = Vec::new();
        // 按频率分组：同频多路要走干扰音，不同频率各走各的。
        let mut by_freq: HashMap<u32, Vec<Vec<i16>>> = HashMap::new();

        for (&(speaker, freq_khz), stream) in streams.iter_mut() {
            let mut buf = vec![0i16; FRAME_SAMPLES];
            match stream.jitter.pop() {
                Some(Frame::Audio(payload)) => {
                    stream.frames += 1;
                    decode_into(&mut stream.decoder, &Frame::Audio(payload), &mut buf);
                }
                Some(Frame::Lost) => {
                    // 丢包隐藏也是这次发言的一帧：时间过去了，时长要算上它。
                    stream.frames += 1;
                    decode_into(&mut stream.decoder, &Frame::Lost, &mut buf);
                }
                Some(Frame::End) => {
                    events.push(RxEvent::End {
                        freq_khz,
                        speaker,
                        frames: stream.frames,
                        secs: stream.frames as f32 * FRAME_SECS,
                    });
                    finished.push((speaker, freq_khz));
                    continue;
                }
                // 还没攒够水位。这一路这一拍不出声，但音频时钟照走。
                None => continue,
            }
            // 射程衰减与静噪。服务端**根本不投递**射程外的包，所以 qual 恒在 1–255，
            // 这里不给 0 写分支（和 mix::quality_gain 同一条理由）。
            stream.agc.process(&mut buf);
            let manual = speaker_gains
                .get(&(speaker, freq_khz))
                .copied()
                .unwrap_or(1.0);
            apply_quality(&mut buf, stream.qual, noise);
            if (manual - 1.0).abs() > f32::EPSILON {
                for sample in &mut buf {
                    *sample = (*sample as f32 * manual)
                        .round()
                        .clamp(i16::MIN as f32, i16::MAX as f32)
                        as i16;
                }
            }
            by_freq.entry(freq_khz).or_default().push(buf);
        }

        for key in finished {
            // 解码器是有状态的、而且不小：一场值班下来每个说过话的人都留一个的话，
            // 内存只涨不落。
            if let Some(stream) = streams.remove(&key) {
                // 水位留下来，缓冲和解码器不留。一个 usize 换掉"每次通话前几帧
                // 都在重新学"。
                remember_depth(learned, key, stream.jitter.target_depth());
            }
        }

        let mut out = vec![0i16; FRAME_SAMPLES];
        for (freq_khz, sources) in by_freq {
            let gain = gains.get(&freq_khz).copied().unwrap_or(1.0);
            if gain <= 0.0 {
                continue; // 这一行被用户静音了。
            }
            let mut per_freq = vec![0i16; FRAME_SAMPLES];
            let refs: Vec<&[i16]> = sources.iter().map(|s| s.as_slice()).collect();
            // 一路是一个人在讲；两路及以上才是"有人在压我的话"。
            interfere(&refs, &mut per_freq, phase);
            mix_into(&mut out, &per_freq, gain);
        }

        (out, events)
    }

    /// 设置某个频率的播放音量。
    pub fn set_gain(&mut self, freq_khz: u32, gain: f32) {
        self.gains.insert(freq_khz, gain.clamp(0.0, MAX_GAIN));
    }

    /// Sets the manual coefficient for one speaker on one frequency.
    pub fn set_speaker_gain(&mut self, speaker: u32, freq_khz: u32, gain: f32) {
        self.speaker_gains
            .insert((speaker, freq_khz), gain.clamp(0.0, MAX_GAIN));
    }

    /// 当前还活着的接收流数。
    pub fn active_streams(&self) -> usize {
        self.streams.len()
    }
}

/// 记得住多少个 `(speaker, freq)` 的水位。
///
/// 有上界是因为这张表**不随发言结束而缩**：一场八小时的值班里听过的发言者
/// 全都留在里面。满了就丢一个——丢掉的那个下次从 60 ms 重新学，
/// 代价是一次通话的前几帧，比无界增长便宜得多。
const LEARNED_LIMIT: usize = 256;

fn remember_depth(learned: &mut HashMap<(u32, u32), usize>, key: (u32, u32), depth: usize) {
    if learned.len() >= LEARNED_LIMIT && !learned.contains_key(&key) {
        if let Some(&victim) = learned.keys().next() {
            learned.remove(&victim);
        }
    }
    learned.insert(key, depth);
}

/// 解一帧进 `buf`。解不开时按静音处理并记一行 DEBUG。
///
/// **解不开不能让音频时钟停下。** 一个畸形的载荷（中间设备改过、或者对端有 bug）
/// 只该让这一路这一帧没声音，不该让整个输出短一帧——那听起来是咔哒声，
/// 而且会被报成"语音系统坏了"。
fn decode_into(decoder: &mut Decoder, frame: &Frame, buf: &mut [i16]) {
    if let Err(e) = decoder.decode(frame, buf) {
        tracing::debug!(error = %e, "a frame did not decode; substituting silence");
        buf.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rx::decode::FRAME_SAMPLES;
    use crate::tx::Encoder;
    use can_voice_proto::wire::{Header, FLAG_FIRST, FLAG_LAST, VERSION};

    fn tone(n: usize, freq: f32) -> Vec<i16> {
        (0..n)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                (0.4 * 32767.0 * (2.0 * std::f32::consts::PI * freq * t).sin()) as i16
            })
            .collect()
    }

    fn rms(s: &[i16]) -> f32 {
        if s.is_empty() {
            return 0.0;
        }
        let sum: f64 = s.iter().map(|&v| (v as f64) * (v as f64)).sum();
        (sum / s.len() as f64).sqrt() as f32
    }

    /// 造一包"服务端发下来的"音频。
    fn packet(
        enc: &mut Encoder,
        freq: u32,
        speaker: u32,
        seq: u16,
        qual: u8,
        flags: u8,
    ) -> (Header, Vec<u8>) {
        let opus = enc.encode(&tone(FRAME_SAMPLES, 440.0)).expect("encode");
        (
            Header {
                ver: VERSION,
                flags,
                qual,
                seq,
                freq_khz: freq,
                speaker,
            },
            opus,
        )
    }

    /// 灌够起播水位再 tick，返回第一帧真正出声的 PCM。
    fn prime_and_pull(m: &mut RxMixer, enc: &mut Encoder, freq: u32, speaker: u32) -> Vec<i16> {
        for seq in 0..4u16 {
            let flags = if seq == 0 { FLAG_FIRST } else { 0 };
            let (h, opus) = packet(enc, freq, speaker, seq, 255, flags);
            m.feed(&h, &opus);
        }
        m.tick().0
    }

    /// **上一次发言学到的水位要带进下一次。**
    ///
    /// 缓冲是一次发言一个，而起播只发生一次——每次 `JitterBuffer::new()` 的话，
    /// 自适应只在单次发言之内向上生效，学到的东西发言一结束全丢。真实抖动网络上
    /// 的表现是每次通话的前几帧都在重新学，而每一次重新学都是一次卡顿。
    #[test]
    fn the_water_level_learned_in_one_talkspurt_is_carried_into_the_next() {
        let mut enc = Encoder::new().expect("encoder");
        let mut m = RxMixer::new();

        // 第一段：灌几帧，然后上游断供。缓冲被抽干，水位一路涨到上限，
        // 静音超时收尾。
        for seq in 0..4u16 {
            let (h, opus) = packet(&mut enc, 121_800, 7, seq, 255, 0);
            m.feed(&h, &opus);
        }
        let mut guard = 0;
        while m.active_streams() > 0 {
            m.tick();
            guard += 1;
            assert!(guard < 100, "the first talkspurt never ended");
        }

        // 第二段：同一个人、同一个频率。水位现在是 MAX_DEPTH=6 帧，
        // 所以 5 帧还不该起播。
        for seq in 20..25u16 {
            let (h, opus) = packet(&mut enc, 121_800, 7, seq, 255, 0);
            m.feed(&h, &opus);
        }
        let (pcm, _) = m.tick();
        assert!(
            rms(&pcm) == 0.0,
            "five frames must not start playback when the learned water level is six"
        );

        let (h, opus) = packet(&mut enc, 121_800, 7, 25, 255, 0);
        m.feed(&h, &opus);
        let (pcm, _) = m.tick();
        assert!(rms(&pcm) > 0.0, "the sixth frame must start it");
    }

    /// **一包就没了下文的那一路也要收得掉。**
    ///
    /// 只到了一两帧、尾帧又丢了（或者听众此刻飞出射程，服务端不打招呼就停发）
    /// 时，起播前的 `pop` 永远返回 `None`：静音超时推不动，`RxEvent::End` 发不
    /// 出来，RX 灯常亮，这条流也永不回收。mixer 自己没有时间概念——
    /// 它的时钟就是 `tick` 本身。
    #[test]
    fn a_stream_that_gets_one_packet_and_nothing_more_is_reclaimed() {
        let mut enc = Encoder::new().expect("encoder");
        let mut m = RxMixer::new();
        let (h, opus) = packet(&mut enc, 121_800, 7, 0, 255, FLAG_FIRST);
        assert!(matches!(m.feed(&h, &opus), Some(RxEvent::Start { .. })));

        let mut ended = false;
        for _ in 0..100 {
            let (pcm, events) = m.tick();
            assert_eq!(
                pcm.len(),
                FRAME_SAMPLES,
                "the audio clock never skips a beat"
            );
            if events.iter().any(|e| matches!(e, RxEvent::End { .. })) {
                ended = true;
                break;
            }
        }
        assert!(ended, "the talkspurt must end on its own");
        assert_eq!(m.active_streams(), 0, "and the stream must be reclaimed");
    }

    #[test]
    fn a_tick_always_produces_exactly_one_frame_of_audio() {
        // 输出接的是声卡时钟，它每 20 毫秒都要一帧，不管有没有人在说话。
        let mut m = RxMixer::new();
        let (pcm, events) = m.tick();
        assert_eq!(pcm.len(), FRAME_SAMPLES);
        assert!(events.is_empty());
        assert!(
            pcm.iter().all(|&v| v == 0),
            "an idle mixer must produce silence, not noise"
        );
    }

    #[test]
    fn a_single_speaker_comes_out_audible() {
        let mut enc = Encoder::new().expect("encoder");
        let mut m = RxMixer::new();
        let pcm = prime_and_pull(&mut m, &mut enc, 121_800, 7);
        assert_eq!(pcm.len(), FRAME_SAMPLES);
        assert!(
            rms(&pcm) > 0.0,
            "a speaker on a subscribed frequency must be audible"
        );
    }

    /// 起播的判据是水位，**不是首帧标志**（修订件 N5）。管制员在别人说到一半时把
    /// 一个频率加进台面，收到的第一个包没有 FLAG_FIRST——等首帧的实现会让他
    /// 一声不响，直到对方下一次按下 PTT。
    #[test]
    fn a_stream_joined_mid_talkspurt_still_plays() {
        let mut enc = Encoder::new().expect("encoder");
        let mut m = RxMixer::new();
        // 序号从 5000 起，一个首帧位都没有。
        for seq in 5000..5004u16 {
            let (h, opus) = packet(&mut enc, 121_800, 7, seq, 255, 0);
            m.feed(&h, &opus);
        }
        let (pcm, _) = m.tick();
        assert!(
            rms(&pcm) > 0.0,
            "joining mid-talkspurt must produce audio, not silence"
        );
    }

    #[test]
    fn the_first_packet_of_a_stream_reports_rx_start() {
        let mut enc = Encoder::new().expect("encoder");
        let mut m = RxMixer::new();
        let (h, opus) = packet(&mut enc, 121_800, 7, 0, 255, FLAG_FIRST);
        let started = m.feed(&h, &opus);
        assert_eq!(
            started,
            Some(RxEvent::Start {
                freq_khz: 121_800,
                speaker: 7
            })
        );

        let (h2, opus2) = packet(&mut enc, 121_800, 7, 1, 255, 0);
        assert_eq!(
            m.feed(&h2, &opus2),
            None,
            "only the first packet starts the talkspurt"
        );
    }

    /// `RxEnd` 的时长按**帧数**算，不按墙上时钟——想知道的是"这次通话有多长音频"，
    /// 而且那样测试才是确定性的。
    #[test]
    fn rx_end_reports_the_frame_count_and_derived_duration() {
        let mut enc = Encoder::new().expect("encoder");
        let mut m = RxMixer::new();
        for seq in 0..5u16 {
            let last = if seq == 4 { FLAG_LAST } else { 0 };
            let first = if seq == 0 { FLAG_FIRST } else { 0 };
            let (h, opus) = packet(&mut enc, 121_800, 7, seq, 255, first | last);
            m.feed(&h, &opus);
        }
        let mut ended = None;
        for _ in 0..12 {
            let (_, events) = m.tick();
            for e in events {
                if let RxEvent::End { .. } = e {
                    ended = Some(e);
                }
            }
            if ended.is_some() {
                break;
            }
        }
        match ended.expect("the talkspurt must end") {
            RxEvent::End {
                freq_khz,
                speaker,
                frames,
                secs,
            } => {
                assert_eq!((freq_khz, speaker), (121_800, 7));
                assert_eq!(frames, 5);
                assert!(
                    (secs - 0.1).abs() < 1e-4,
                    "5 frames of 20 ms is 0.1 s, got {secs}"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    /// 尾帧会丢，而且服务端在听众飞出射程时**不打招呼就停发**——那种情况下它保证
    /// 到不了。所以没有尾帧也必须结束，靠抖动缓冲自带的静音超时。
    #[test]
    fn a_talkspurt_ends_without_a_last_flag_too() {
        let mut enc = Encoder::new().expect("encoder");
        let mut m = RxMixer::new();
        for seq in 0..4u16 {
            let (h, opus) = packet(&mut enc, 121_800, 7, seq, 255, 0);
            m.feed(&h, &opus);
        }
        let mut ended = false;
        for _ in 0..20 {
            let (_, events) = m.tick();
            if events.iter().any(|e| matches!(e, RxEvent::End { .. })) {
                ended = true;
                break;
            }
        }
        assert!(
            ended,
            "silence must end the talkspurt even with no last flag"
        );
    }

    /// 结束之后那一路要被清掉，否则一场值班下来每个说过话的人都留着一个
    /// 解码器和一个缓冲——解码器是有状态的，而且不小。
    #[test]
    fn a_finished_stream_is_dropped() {
        let mut enc = Encoder::new().expect("encoder");
        let mut m = RxMixer::new();
        let (h, opus) = packet(&mut enc, 121_800, 7, 0, 255, FLAG_FIRST | FLAG_LAST);
        m.feed(&h, &opus);
        for _ in 0..20 {
            m.tick();
        }
        assert_eq!(
            m.active_streams(),
            0,
            "a finished talkspurt must not leak its decoder"
        );
    }

    /// 同频两个人在讲 → 干扰音。这是"同频叠加"那条选型的落点：
    /// 听感上是"有人在压我的话"，而不是两段可分辨的语音。
    #[test]
    fn two_speakers_on_one_frequency_interfere() {
        let mut enc = Encoder::new().expect("encoder");
        let mut solo = RxMixer::new();
        let a = prime_and_pull(&mut solo, &mut enc, 121_800, 7);

        let mut both = RxMixer::new();
        let mut enc2 = Encoder::new().expect("encoder");
        for seq in 0..4u16 {
            for speaker in [7u32, 9] {
                let (h, opus) = packet(&mut enc2, 121_800, speaker, seq, 255, 0);
                both.feed(&h, &opus);
            }
        }
        let mixed = both.tick().0;
        assert_ne!(mixed, a, "two speakers must not sound like one");
        assert!(
            rms(&mixed) > rms(&a),
            "two people talking over each other should be more, not less"
        );
    }

    /// 两个人在**不同**频率上讲，各走各的——不是干扰，是两路都要听见。
    #[test]
    fn two_speakers_on_different_frequencies_are_summed_not_interfered() {
        let mut enc = Encoder::new().expect("encoder");
        let mut m = RxMixer::new();
        for seq in 0..4u16 {
            for (freq, speaker) in [(118_000u32, 7u32), (121_800, 9)] {
                let (h, opus) = packet(&mut enc, freq, speaker, seq, 255, 0);
                m.feed(&h, &opus);
            }
        }
        let (pcm, _) = m.tick();
        assert!(rms(&pcm) > 0.0);
        assert_eq!(m.active_streams(), 2);
    }

    #[test]
    fn a_weaker_signal_is_quieter() {
        let mut enc = Encoder::new().expect("encoder");
        let mut strong = RxMixer::new();
        for seq in 0..4u16 {
            let (h, opus) = packet(&mut enc, 121_800, 7, seq, 255, 0);
            strong.feed(&h, &opus);
        }
        let mut weak = RxMixer::new();
        let mut enc2 = Encoder::new().expect("encoder");
        for seq in 0..4u16 {
            let (h, opus) = packet(&mut enc2, 121_800, 7, seq, 30, 0);
            weak.feed(&h, &opus);
        }
        assert!(rms(&weak.tick().0) < rms(&strong.tick().0));
    }

    /// 每频率音量是用户设置的，0 就是静音这一行。
    #[test]
    fn a_per_frequency_gain_of_zero_silences_that_frequency_only() {
        let mut enc = Encoder::new().expect("encoder");
        let mut m = RxMixer::new();
        m.set_gain(118_000, 0.0);
        for seq in 0..4u16 {
            for (freq, speaker) in [(118_000u32, 7u32), (121_800, 9)] {
                let (h, opus) = packet(&mut enc, freq, speaker, seq, 255, 0);
                m.feed(&h, &opus);
            }
        }
        let with_both = m.tick().0;

        let mut only = RxMixer::new();
        let mut enc2 = Encoder::new().expect("encoder");
        for seq in 0..4u16 {
            let (h, opus) = packet(&mut enc2, 121_800, 9, seq, 255, 0);
            only.feed(&h, &opus);
        }
        let only_one = only.tick().0;
        // 静音掉的那一路不该再贡献能量；两者应当很接近（Opus 有损，不比样本）。
        let d = (rms(&with_both) - rms(&only_one)).abs();
        assert!(
            d < rms(&only_one) * 0.25,
            "a muted frequency still leaked: {d}"
        );
    }

    #[test]
    fn each_speaker_can_have_an_independent_manual_gain() {
        let mut enc = Encoder::new().expect("encoder");
        let mut quiet = RxMixer::new();
        quiet.set_speaker_gain(7, 121_800, 0.0);
        for seq in 0..4u16 {
            for speaker in [7u32, 9] {
                let (h, opus) = packet(&mut enc, 121_800, speaker, seq, 255, 0);
                quiet.feed(&h, &opus);
            }
        }
        let muted = quiet.tick().0;

        let mut only = RxMixer::new();
        let mut enc2 = Encoder::new().expect("encoder");
        for seq in 0..4u16 {
            let (h, opus) = packet(&mut enc2, 121_800, 9, seq, 255, 0);
            only.feed(&h, &opus);
        }
        let reference = only.tick().0;
        assert!((rms(&muted) - rms(&reference)).abs() < rms(&reference) * 0.35);
    }

    /// 服务端**根本不投递**射程外的包，所以下行 `qual` 恒在 1–255。
    /// 这里不给 0 写分支，和 `mix::quality_gain` 同一条理由。
    #[test]
    fn a_quality_of_zero_is_not_a_special_case_here_either() {
        let mut enc = Encoder::new().expect("encoder");
        let mut m = RxMixer::new();
        for seq in 0..4u16 {
            let (h, opus) = packet(&mut enc, 121_800, 7, seq, 0, 0);
            m.feed(&h, &opus);
        }
        // 不 panic、不静音——只是最弱的那一档。
        assert!(rms(&m.tick().0) > 0.0);
    }

    #[test]
    fn a_packet_whose_payload_will_not_decode_does_not_kill_the_stream() {
        let mut m = RxMixer::new();
        let h = Header {
            ver: VERSION,
            flags: 0,
            qual: 255,
            seq: 0,
            freq_khz: 121_800,
            speaker: 7,
        };
        for seq in 0..4u16 {
            let mut h = h;
            h.seq = seq;
            m.feed(&h, &[0xff, 0xff, 0xff]); // 不是合法的 Opus
        }
        let (pcm, _) = m.tick();
        assert_eq!(
            pcm.len(),
            FRAME_SAMPLES,
            "a bad payload must not stop the audio clock"
        );
    }
}
