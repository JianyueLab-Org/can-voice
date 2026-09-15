//! 发送路径：采集与 Opus 编码。

pub mod capture;

pub use capture::{TxFrame, TxPipeline};

use crate::rx::decode::FRAME_SAMPLES;
use audiopus::{coder::Encoder as OpusEncoder, Application, Bitrate, Channels, SampleRate};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("opus: {0}")]
    Opus(#[from] audiopus::Error),
    #[error("frame holds {0} samples, a 20 ms frame is {FRAME_SAMPLES}")]
    WrongFrameSize(usize),
}

type Result<T> = std::result::Result<T, Error>;

/// 每帧最大编码字节数。24 kbps 的 20 ms 帧约 60 字节，400 是充足上限。
const MAX_PACKET: usize = 400;

/// 无线电语音的目标码率：可懂度优先于音乐保真度。
const BITRATE_BPS: i32 = 24_000;

pub struct Encoder {
    inner: OpusEncoder,
    buf: Vec<u8>,
}

impl Encoder {
    pub fn new() -> Result<Self> {
        let mut inner = OpusEncoder::new(SampleRate::Hz48000, Channels::Mono, Application::Voip)?;
        inner.set_bitrate(Bitrate::BitsPerSecond(BITRATE_BPS))?;
        Ok(Self {
            inner,
            buf: vec![0u8; MAX_PACKET],
        })
    }

    /// 编一帧 20 ms 的单声道 PCM。
    pub fn encode(&mut self, pcm: &[i16]) -> Result<Vec<u8>> {
        // 尺寸不对必须响亮地失败：一个被悄悄接受的错误尺寸
        // 会表现为音频忽快忽慢，那比一条错误难查得多。
        if pcm.len() != FRAME_SAMPLES {
            return Err(Error::WrongFrameSize(pcm.len()));
        }
        let n = self.inner.encode(pcm, &mut self.buf)?;
        Ok(self.buf[..n].to_vec())
    }
}
