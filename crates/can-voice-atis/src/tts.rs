//! 文本转语音。
//!
//! # 为什么是外部命令
//!
//! 好的中文语音不在 Rust 生态里。这个网络现在播的就是 `edge-tts` 合成的声音，
//! 操作员听惯的也是它。把合成这一步做成"跑一条命令"，既留住了那把嗓子，
//! 也不用把一个重量级 TTS 塞进二进制——而且换后端只是改一行配置。
//!
//! **"那把嗓子"有具体的值，也有东西钉着。** 默认是 `zh-CN-YunxiNeural` /
//! `en-US-ChristopherNeural`，和 can-audio 播的同两个男声
//! （`can-audio/server/ATIS/mumble.py:317,324`）；值在机队的 `DEFAULT_VOICE_*`
//! （`main.rs`），`the_default_voices_are_the_ones_can_audio_spoke_with` 守着。
//! 这一段一度写着"留住了那把嗓子"而默认值是另外两个女声——切换当天全网通播
//! 换了一种声音，而没有一行字说过这件事。这里只管把 `{voice}` 填进命令行：
//! 换嗓子是改 `ATIS_VOICE_*`，不是改这里。
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

/// 合成结果最多缓存几条。
///
/// 一份 45 秒的通播是 48 kHz 16 位单声道，约 4 MB。留几条是为了覆盖"改了一版又
/// 改回去"和多语言那两半，**不是为了留住历史**：一个跑了几天的机队要是把每一版
/// 稿子都留着，光缓存就能吃掉几个 G。
pub const CACHE_ENTRIES: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("tts command failed: {0}")]
    Tts(String),
    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// 合成结果的缓存，键是**整篇稿子**。
///
/// 播报循环每三秒要同一段 PCM。不缓存的话，一份一整天不变的通播会一天几千次去开
/// `edge-tts` 和 `ffmpeg` 两个子进程——白烧 CPU，也白打人家的接口。键是整篇稿子
/// 而不是席位：报文一变、模板一改就该是新的一条，而那正是要重新合成的时候。
///
/// LRU 而不是先进先出：一个在两套跑道构型之间来回切的席位，两篇稿子都该留着。
#[derive(Debug)]
pub struct PcmCache {
    /// 最久没用过的在前。条数以个位数计，线性扫比哈希表加链表简单得多。
    entries: Vec<(String, std::sync::Arc<Vec<i16>>)>,
    limit: usize,
}

impl PcmCache {
    pub fn new(limit: usize) -> Self {
        Self {
            entries: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// 取一条，并把它记成"刚用过"。
    pub fn get(&mut self, text: &str) -> Option<std::sync::Arc<Vec<i16>>> {
        let i = self.entries.iter().position(|(k, _)| k == text)?;
        let entry = self.entries.remove(i);
        let pcm = entry.1.clone();
        self.entries.push(entry);
        Some(pcm)
    }

    /// 放一条。同一篇稿子再放一次是替换，不是多一条。
    pub fn put(&mut self, text: String, pcm: std::sync::Arc<Vec<i16>>) {
        self.entries.retain(|(k, _)| *k != text);
        self.entries.push((text, pcm));
        while self.entries.len() > self.limit {
            self.entries.remove(0);
        }
    }
}

impl Default for PcmCache {
    fn default() -> Self {
        Self::new(CACHE_ENTRIES)
    }
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

    fn pcm(n: i16) -> std::sync::Arc<Vec<i16>> {
        std::sync::Arc::new(vec![n; 4])
    }

    /// **报文没变就不重新合成。**
    ///
    /// 播报循环每三秒要同一段 PCM。不缓存的话，一份一整天不变的通播会一天几千次
    /// 去开 `edge-tts` 和 `ffmpeg` 两个子进程——白烧 CPU，也白打人家的接口。
    #[test]
    fn the_same_report_is_not_synthesised_twice() {
        let mut c = PcmCache::new(4);
        c.put("ZSPD ATIS A".into(), pcm(1));

        assert_eq!(c.get("ZSPD ATIS A").map(|p| p[0]), Some(1));
        // 报文一变就是新的一条：键是整篇稿子，而不是席位。
        assert!(c.get("ZSPD ATIS B").is_none());
    }

    /// 缓存**有上限**，满了先丢最久没用过的那条。
    ///
    /// 一份 45 秒的通播是 48 kHz 16 位单声道，约 4 MB。不设上限的话，一个跑了
    /// 几天的机队会把每一版稿子都留着。
    #[test]
    fn the_cache_drops_the_least_recently_used_entry() {
        let mut c = PcmCache::new(2);
        c.put("a".into(), pcm(1));
        c.put("b".into(), pcm(2));
        // 读一下 a，它就不再是"最久没用过的"那条了。
        assert!(c.get("a").is_some());
        c.put("c".into(), pcm(3));

        assert!(c.get("b").is_none(), "b was the least recently used");
        assert!(c.get("a").is_some());
        assert!(c.get("c").is_some());
    }

    /// 同一篇稿子放两次不会把缓存撑大。
    #[test]
    fn putting_the_same_text_again_replaces_it() {
        let mut c = PcmCache::new(2);
        c.put("a".into(), pcm(1));
        c.put("a".into(), pcm(9));
        c.put("b".into(), pcm(2));

        assert_eq!(c.get("a").map(|p| p[0]), Some(9));
        assert!(c.get("b").is_some());
    }
}
