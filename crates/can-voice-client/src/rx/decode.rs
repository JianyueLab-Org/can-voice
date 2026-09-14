//! Opus 解码与丢包隐藏。

use super::jitter::Frame;
use audiopus::{coder::Decoder as OpusDecoder, Channels, SampleRate};

/// 采样率。pymumble 时代 48 kHz 是"理想路径"而低采样率会导致变调；
/// 这里它是唯一路径 —— 设备采样率的适配在 `crate::audio` 里做。
pub const SAMPLE_RATE: u32 = 48_000;

/// 一帧的采样数：20 ms × 48 kHz。
///
/// 它和 [`SAMPLE_RATE`] 的关系（每秒 50 帧）是真的契约，不是巧合：
/// 服务端的 `seq` 按每帧加一推进，21.8 分钟的回绕周期、抖动缓冲的
/// 40–120 ms 深度、以及"连续丢 3 帧就结束发言"的静音超时，
/// 全都按这个数换算。钉子是 `a_frame_is_exactly_twenty_milliseconds`。
pub const FRAME_SAMPLES: usize = 960;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("opus: {0}")]
    Opus(#[from] audiopus::Error),
    #[error("output buffer holds {0} samples, need {FRAME_SAMPLES}")]
    BufferTooSmall(usize),
}

type Result<T> = std::result::Result<T, Error>;

/// 一个发言者的解码器。**每个 `(speaker, freq)` 一个** ——
/// Opus 解码器是有状态的，共用一个会让两个人的音频互相污染，
/// 而丢包隐藏更是完全依赖前一帧的状态。
pub struct Decoder {
    inner: OpusDecoder,
}

impl Decoder {
    pub fn new() -> Result<Self> {
        Ok(Self {
            inner: OpusDecoder::new(SampleRate::Hz48000, Channels::Mono)?,
        })
    }

    /// 解一帧。返回写入 `out` 的采样数。
    pub fn decode(&mut self, frame: &Frame, out: &mut [i16]) -> Result<usize> {
        if out.len() < FRAME_SAMPLES {
            return Err(Error::BufferTooSmall(out.len()));
        }
        match frame {
            Frame::Audio(p) => Ok(self.inner.decode(Some(p), &mut out[..FRAME_SAMPLES], false)?),
            // 丢包隐藏：让 Opus 用前一帧外推。填静音会在音频里留下一个
            // 清晰的"咔哒"，比稍微失真难听得多。
            Frame::Lost => Ok(self.inner.decode(None::<&[u8]>, &mut out[..FRAME_SAMPLES], false)?),
            Frame::End => Ok(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rx::jitter::Frame;
    use crate::tx::Encoder;

    fn tone(n: usize) -> Vec<i16> {
        (0..n)
            .map(|i| {
                let t = i as f32 / SAMPLE_RATE as f32;
                (0.4 * 32767.0 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()) as i16
            })
            .collect()
    }

    #[test]
    fn a_frame_survives_a_round_trip() {
        let mut enc = Encoder::new().expect("encoder");
        let mut dec = Decoder::new().expect("decoder");
        let pcm = tone(FRAME_SAMPLES);

        let packet = enc.encode(&pcm).expect("encode");
        assert!(!packet.is_empty(), "encoding produced no bytes");
        assert!(packet.len() < 400, "a 20 ms voice frame should be small, got {}", packet.len());

        let mut out = vec![0i16; FRAME_SAMPLES];
        let n = dec.decode(&Frame::Audio(packet), &mut out).expect("decode");
        assert_eq!(n, FRAME_SAMPLES, "a 20 ms frame decodes to {FRAME_SAMPLES} samples");

        // Opus 是有损的，不能比对样本；比对能量即可。
        let energy: f64 = out.iter().map(|&v| (v as f64).abs()).sum();
        assert!(energy > 0.0, "decoded frame is silent");
    }

    #[test]
    fn a_lost_frame_is_concealed_rather_than_silenced() {
        // Opus 的 PLC 会用前一帧外推。直接填静音会在音频里留下
        // 一个清晰的"咔哒"，比稍微失真难听得多。
        let mut enc = Encoder::new().expect("encoder");
        let mut dec = Decoder::new().expect("decoder");
        let pcm = tone(FRAME_SAMPLES);

        for _ in 0..3 {
            let p = enc.encode(&pcm).expect("encode");
            let mut out = vec![0i16; FRAME_SAMPLES];
            dec.decode(&Frame::Audio(p), &mut out).expect("decode");
        }

        let mut out = vec![0i16; FRAME_SAMPLES];
        let n = dec.decode(&Frame::Lost, &mut out).expect("conceal");
        assert_eq!(n, FRAME_SAMPLES);
        let energy: f64 = out.iter().map(|&v| (v as f64).abs()).sum();
        assert!(energy > 0.0, "packet loss concealment produced pure silence");
    }

    #[test]
    fn an_end_frame_produces_no_audio() {
        let mut dec = Decoder::new().expect("decoder");
        let mut out = vec![0i16; FRAME_SAMPLES];
        assert_eq!(dec.decode(&Frame::End, &mut out).expect("end"), 0);
    }

    #[test]
    fn encoding_silence_still_produces_a_packet() {
        // 静音帧也必须发出去：接收端的抖动缓冲靠连续的序号判断丢包，
        // 静音时不发会被当成丢了一大片。
        let mut enc = Encoder::new().expect("encoder");
        let packet = enc.encode(&vec![0i16; FRAME_SAMPLES]).expect("encode");
        assert!(!packet.is_empty());
    }

    #[test]
    fn encoding_rejects_a_wrong_sized_frame() {
        let mut enc = Encoder::new().expect("encoder");
        assert!(enc.encode(&vec![0i16; 123]).is_err(),
            "only exact 20 ms frames are valid; a wrong size must fail loudly");
    }

    #[test]
    fn a_short_output_buffer_is_an_error_rather_than_a_partial_decode() {
        let mut dec = Decoder::new().expect("decoder");
        let mut out = vec![0i16; FRAME_SAMPLES - 1];
        assert!(dec.decode(&Frame::Lost, &mut out).is_err());
    }

    /// 修订件 L8：原计划靠一行 `let _ = SAMPLE_RATE;` 来"引用"这个常量，
    /// 那是一条强制的空操作。两个常量的关系是真的契约——每帧 20 毫秒——
    /// 所以用一条真正会算的断言来表达它。
    #[test]
    fn a_frame_is_exactly_twenty_milliseconds() {
        assert_eq!(FRAME_SAMPLES * 50, SAMPLE_RATE as usize,
            "a 20 ms frame at {SAMPLE_RATE} Hz is {} samples", SAMPLE_RATE / 50);
    }
}
