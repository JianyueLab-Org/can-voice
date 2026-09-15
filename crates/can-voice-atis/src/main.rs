//! 服务端通播机器人：把 can-fsd datafeed 里每一个 `_ATIS` 席位播到语音网上。
//!
//! # 它没有任何绕过账号的捷径，而且那是有意的
//!
//! 这支机队用**一个真实成员账号**登录（`ATIS_CID` + `ATIS_PASSWORD`），走的是
//! 和所有人一样的那条路：拿凭据去 can-api 换一张短期票。旧实现曾经有过一条
//! 保留账号的旁路，`can-audio` 为此专门写了一条测试
//! （`test_there_is_no_shortcut_for_any_account`）防止它被加回来；这里的对应物是
//! `no_shortcut_for_any_account`。
//!
//! `can-voice-token` 的 `TokenSource` 只有一个构造函数，它要凭据。
//! **没有一个入口能直接塞一张票进来**——这不是纪律，是类型层面的事实。

pub mod datafeed;
pub mod fleet;
pub mod readback;
pub mod station;
pub mod tts;

use can_voice_token::TokenSource;
use fleet::{Action, Running};
use std::collections::HashMap;
use std::time::Duration;

struct Task {
    handle: tokio::task::JoinHandle<()>,
    text: tokio::sync::watch::Sender<String>,
    freq_khz: u32,
    last_text: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(env("RUST_LOG", "info"))
        .init();

    let http = match reqwest::Client::builder()
        .user_agent(datafeed::USER_AGENT)
        .timeout(Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("could not build the http client: {e}");
            std::process::exit(1);
        }
    };

    let cid = require("ATIS_CID");
    let password = require("ATIS_PASSWORD");
    let tokens = TokenSource::new(
        &env("CAN_API_ORIGIN", "https://api.ceruleanavi.net"),
        cid,
        password,
        http.clone(),
    );

    let voice = station::VoiceSettings {
        server: env("CAN_VOICE_SERVER", "audio.ceruleanavi.net:64738"),
        server_name: env("CAN_VOICE_SERVER_NAME", "audio.ceruleanavi.net"),
        tokens,
    };

    let synth = tts::CommandTts {
        argv: shell_words(&env(
            "ATIS_TTS_ARGV",
            "edge-tts --voice {voice} --text {text} --write-media {out}",
        )),
        voice_en: env("ATIS_VOICE_EN", "en-US-AriaNeural"),
        voice_zh: env("ATIS_VOICE_ZH", "zh-CN-XiaoxiaoNeural"),
        ffmpeg: env("FFMPEG", "ffmpeg"),
    };

    let feed_url = env(
        "CAN_FSD_DATAFEED",
        "https://data.ceruleanavi.net/v1/data.json",
    );
    let poll = Duration::from_secs(env("ATIS_POLL_SECS", "30").parse().unwrap_or(30));

    tracing::info!(%feed_url, poll = poll.as_secs(), "atis fleet starting");

    let mut running: HashMap<String, Task> = HashMap::new();
    loop {
        match fetch_feed(&http, &feed_url).await {
            Ok(feed) => {
                let wanted = datafeed::stations_from(&feed);
                apply(&mut running, &wanted, &voice, &synth);
            }
            Err(e) => {
                // 取不到 datafeed **不停播**：正在播的那几路照常，
                // 报文停在最后一次取到的那份。一次网络抖动不该让全网 ATIS 静默。
                tracing::warn!(error = %e, "could not read the datafeed; keeping the current fleet");
            }
        }
        tokio::time::sleep(poll).await;
    }
}

fn apply(
    running: &mut HashMap<String, Task>,
    wanted: &[datafeed::Station],
    voice: &station::VoiceSettings,
    synth: &tts::CommandTts,
) {
    let snapshot: HashMap<String, Running> = running
        .iter()
        .map(|(k, t)| {
            (
                k.clone(),
                Running {
                    freq_khz: t.freq_khz,
                    text: t.last_text.clone(),
                    // **判据是任务还活着，不是"在不在表里"。** 见 fleet::reconcile。
                    alive: !t.handle.is_finished(),
                },
            )
        })
        .collect();

    for action in fleet::reconcile(&snapshot, wanted) {
        match action {
            Action::Stop(callsign) => {
                if let Some(t) = running.remove(&callsign) {
                    tracing::info!(%callsign, "stopping station");
                    t.handle.abort();
                }
            }
            Action::Start(s) => {
                tracing::info!(callsign = %s.callsign, freq = s.freq_khz, "starting station");
                let (tx, rx) = tokio::sync::watch::channel(s.text.clone());
                let handle = tokio::spawn(station::run(station::Station {
                    callsign: s.callsign.clone(),
                    freq_khz: s.freq_khz,
                    voice: voice.clone(),
                    tts: synth.clone(),
                    text: rx,
                }));
                running.insert(
                    s.callsign.clone(),
                    Task {
                        handle,
                        text: tx,
                        freq_khz: s.freq_khz,
                        last_text: s.text,
                    },
                );
            }
            Action::UpdateText { callsign, text } => {
                if let Some(t) = running.get_mut(&callsign) {
                    tracing::debug!(%callsign, "report changed");
                    let _ = t.text.send(text.clone());
                    t.last_text = text;
                }
            }
        }
    }
}

async fn fetch_feed(
    http: &reqwest::Client,
    url: &str,
) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
    Ok(http
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn require(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| {
        eprintln!("{key} is required");
        std::process::exit(2);
    })
}

/// 极简的命令行切分：空格分隔，支持双引号包住带空格的一段。
///
/// 不用 shell：`{text}` 会被替换成整段 ATIS 报文，而那里面有 `&`、`/` 和引号。
fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quoted_argument_survives_splitting() {
        assert_eq!(shell_words("a \"b c\" d"), vec!["a", "b c", "d"]);
        assert_eq!(shell_words("  spaced   out  "), vec!["spaced", "out"]);
        assert!(shell_words("").is_empty());
    }

    /// **这支机队没有任何绕过账号的捷径。**
    ///
    /// 旧实现曾经有过一条保留账号的旁路，`can-audio` 为此写了一条测试
    /// （`test_there_is_no_shortcut_for_any_account`）防止它被加回来。这是对应物。
    ///
    /// 它扫两件事：源码里没有"跳过鉴权"那一类的开关，以及
    /// **`TokenSource` 只有一个要凭据的构造函数**——没有 `from_token`，
    /// 配置里也没有 token 字段。一支机队要是能被塞一张长期票，那张票就成了一个
    /// 没人管的凭据，而它恰好拥有一个真实成员账号的全部权限。
    #[test]
    fn no_shortcut_for_any_account() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut scanned = 0usize;
        for entry in std::fs::read_dir(&dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            if !matches!(path.extension().and_then(|e| e.to_str()), Some("rs")) {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("read");
            scanned += 1;
            let mut in_tests = false;
            for line in src.lines() {
                // 测试模块自己要写下这些词才能扫它们，跳过——
                // 否则这条测试会在自己的禁用词表上失败。
                if line.trim_start().starts_with("mod tests") {
                    in_tests = true;
                }
                if in_tests {
                    continue;
                }
                let t = line.trim_start();
                if t.starts_with("//") {
                    continue;
                }
                for banned in ["skip_auth", "bypass", "no_auth", "from_token", "with_token"] {
                    assert!(
                        !t.contains(banned),
                        "{}: {banned:?} — the fleet authenticates as a real member, \
                         with no way in but credentials: {t}",
                        path.display()
                    );
                }
            }
        }
        assert!(
            scanned >= 5,
            "only {scanned} files scanned; the walk is probably broken"
        );
    }
}
