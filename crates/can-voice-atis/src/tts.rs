//! 文本转语音。
//!
//! # 为什么是外部命令
//!
//! 好的中文语音不在 Rust 生态里。这个网络现在播的就是 `edge-tts` 合成的声音，
//! 操作员听惯的也是它。把合成这一步做成"跑一条命令"，既留住了那把嗓子，
//! 也不用把一个重量级 TTS 塞进二进制——而且换后端只是改一行配置。
//!
//! 转码同理：Python 版的部署本来就要求 PATH 上有 ffmpeg。
//!
//! ```text
//!   文本 ──(TTS 命令)──► mp3 ──(ffmpeg -f s16le -ar 48000 -ac 1)──► PCM ──► push_audio
//! ```

use std::path::Path;
use std::process::Stdio;

/// 一帧的采样数，与 `can-voice-client` 的 20 毫秒帧一致。
pub const FRAME_SAMPLES: usize = 960;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("tts command failed: {0}")]
    Tts(String),
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// 用外部命令合成语音。
#[derive(Debug, Clone)]
pub struct CommandTts {
    /// 合成命令的 argv 模板。`{voice}`、`{text}`、`{out}` 会被替换。
    pub argv: Vec<String>,
    pub voice_en: String,
    pub voice_zh: String,
    pub ffmpeg: String,
}

impl CommandTts {
    /// 合成一段文本，返回 48 kHz 单声道 PCM。
    pub async fn speak(&self, text: &str, chinese: bool) -> Result<Vec<i16>, Error> {
        let dir = std::env::temp_dir();
        // 文件名带 pid 和一个自增号：一台机器上会有好几路同时在合成。
        let out = dir.join(format!(
            "can-voice-atis-{}-{}.media",
            std::process::id(),
            next_id()
        ));
        let voice = if chinese {
            &self.voice_zh
        } else {
            &self.voice_en
        };
        let argv = build_argv(&self.argv, voice, text, &out.to_string_lossy());

        let result = self.run(&argv, &out).await;
        // 无论成败都清掉：一路每几十秒合成一次，留着就是一天几千个文件。
        let _ = tokio::fs::remove_file(&out).await;
        result
    }

    async fn run(&self, argv: &[String], out: &Path) -> Result<Vec<i16>, Error> {
        let Some((program, args)) = argv.split_first() else {
            return Err(Error::Tts("empty command".into()));
        };
        let status = tokio::process::Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .status()
            .await?;
        if !status.success() {
            return Err(Error::Tts(format!("{program} exited with {status}")));
        }

        let decoded = tokio::process::Command::new(&self.ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(out)
            .args(["-f", "s16le", "-ar", "48000", "-ac", "1", "-"])
            .stdin(Stdio::null())
            .output()
            .await?;
        if !decoded.status.success() {
            return Err(Error::Ffmpeg(
                String::from_utf8_lossy(&decoded.stderr).trim().to_string(),
            ));
        }
        Ok(pcm_from_le(&decoded.stdout))
    }
}

fn next_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// 把模板里的占位符换掉。
///
/// **文本是整个一个 argv 元素，不拼进 shell 命令行。** ATIS 报文里有 `&`、`/`、
/// 引号，交给 shell 会被切开或者被当成操作符。
pub fn build_argv(template: &[String], voice: &str, text: &str, out: &str) -> Vec<String> {
    template
        .iter()
        .map(|a| match a.as_str() {
            "{voice}" => voice.to_string(),
            "{text}" => text.to_string(),
            "{out}" => out.to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// 小端 16 位 PCM 字节 → 采样。
///
/// 末尾多出来的半个采样**丢掉**，不要留着和下一段拼——拼起来会让整段音频从那里
/// 开始错位一个字节，听感是持续的噪声，而日志里什么都没有。
pub fn pcm_from_le(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect()
}

/// 把一整段 PCM 切成整帧，最后一帧补静音。
///
/// **必须按帧喂给 `push_audio`，不能一次灌完。** `TxPipeline` 只缓四帧、
/// 溢出丢**最旧**的——一段 30 秒的报文一次灌进去，留下的是最后那 80 毫秒，
/// 而前面 29.9 秒无声无息地没了。
///
/// 最后一帧补静音而不是短一截：短帧到了编码器那里是一条 `WrongFrameSize`，
/// 整帧被丢掉，于是报文的结尾被切掉。
pub fn frames_of(pcm: &[i16]) -> Vec<Vec<i16>> {
    pcm.chunks(FRAME_SAMPLES)
        .map(|c| {
            let mut f = c.to_vec();
            f.resize(FRAME_SAMPLES, 0);
            f
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_argv_template_substitutes_every_placeholder() {
        let argv = build_argv(
            &[
                "edge-tts".into(),
                "--voice".into(),
                "{voice}".into(),
                "--text".into(),
                "{text}".into(),
                "--write-media".into(),
                "{out}".into(),
            ],
            "zh-CN-XiaoxiaoNeural",
            "通播 幺两三",
            "/tmp/a.mp3",
        );
        assert_eq!(
            argv,
            vec![
                "edge-tts",
                "--voice",
                "zh-CN-XiaoxiaoNeural",
                "--text",
                "通播 幺两三",
                "--write-media",
                "/tmp/a.mp3",
            ]
        );
    }

    /// 文本是**整个一个 argv 元素**，不拼进 shell 命令行。
    /// ATIS 报文里有 `&`、`/`、引号，交给 shell 会被切开或者被当成操作符。
    #[test]
    fn the_text_is_one_argument_not_a_shell_string() {
        let argv = build_argv(
            &["say".into(), "{text}".into()],
            "v",
            "A & B / C \"D\"",
            "/tmp/o",
        );
        assert_eq!(argv.len(), 2);
        assert_eq!(argv[1], "A & B / C \"D\"");
    }

    #[test]
    fn a_template_without_placeholders_is_left_alone() {
        let argv = build_argv(&["true".into()], "v", "t", "/tmp/o");
        assert_eq!(argv, vec!["true"]);
    }

    // ——— PCM 解码 ———

    #[test]
    fn little_endian_pairs_become_samples() {
        // 0x0100 = 256, 0xFFFF = -1
        assert_eq!(pcm_from_le(&[0x00, 0x01, 0xff, 0xff]), vec![256, -1]);
    }

    /// 奇数个字节说明上游被截断了。**丢掉那半个采样，不要把它和下一段拼起来**
    /// ——拼起来会让整段音频从那里开始错位一个字节，听感是持续的噪声。
    #[test]
    fn a_dangling_byte_is_dropped_rather_than_misaligning_everything() {
        assert_eq!(pcm_from_le(&[0x00, 0x01, 0x7f]), vec![256]);
        assert!(pcm_from_le(&[0x7f]).is_empty());
    }

    #[test]
    fn empty_input_decodes_to_nothing() {
        assert!(pcm_from_le(&[]).is_empty());
    }

    // ——— 播出的切块 ———

    /// **必须按帧喂，不能一次灌完。** `TxPipeline` 只缓
    /// `MAX_BUFFERED_FRAMES` 帧、溢出丢**最旧**的——一段 30 秒的报文一次灌进去，
    /// 留下的是最后那 80 毫秒，而前面 29.9 秒无声无息地没了。
    #[test]
    fn playout_is_cut_into_whole_frames() {
        let pcm = vec![1i16; FRAME_SAMPLES * 3 + 7];
        let frames = frames_of(&pcm);
        assert_eq!(frames.len(), 4, "the remainder gets its own frame");
        assert!(
            frames.iter().all(|f| f.len() == FRAME_SAMPLES),
            "every frame is a whole frame"
        );
    }

    /// 最后那一帧补静音而不是短一截：短帧到了编码器那里是一条
    /// `WrongFrameSize` 错误，整帧被丢掉，于是报文的结尾被切掉。
    #[test]
    fn the_last_frame_is_padded_with_silence() {
        let pcm = vec![7i16; FRAME_SAMPLES + 3];
        let frames = frames_of(&pcm);
        assert_eq!(frames.len(), 2);
        assert_eq!(&frames[1][..3], &[7, 7, 7]);
        assert!(frames[1][3..].iter().all(|&v| v == 0));
    }

    #[test]
    fn nothing_in_nothing_out() {
        assert!(frames_of(&[]).is_empty());
    }

    #[test]
    fn an_exact_multiple_does_not_get_an_empty_tail_frame() {
        assert_eq!(frames_of(&vec![1i16; FRAME_SAMPLES * 2]).len(), 2);
    }
}
