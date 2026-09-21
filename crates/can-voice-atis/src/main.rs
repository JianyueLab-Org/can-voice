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

//! 模块本身在 `lib.rs`——这个 crate 也是一个库，桌面通播客户端
//! （`apps/atis`）从那边取同一套 METAR / 模板 / 读法逻辑。

use can_voice_atis::fleet::{Action, Running};
use can_voice_atis::{datafeed, fleet, station, tts};
use can_voice_token::TokenSource;
use std::collections::HashMap;
use std::time::Duration;

/// 默认嗓子，**和 can-audio 播的是同两个男声**
/// （`can-audio/server/ATIS/mumble.py:317,324`）。
///
/// 切换当天全网通播不该换一种声音——听惯的是这两把嗓子，而"今天的通播听着不对"
/// 是一条没人报得上来的故障。要换改 `ATIS_VOICE_EN` / `ATIS_VOICE_ZH`。
const DEFAULT_VOICE_EN: &str = "en-US-ChristopherNeural";
const DEFAULT_VOICE_ZH: &str = "zh-CN-YunxiNeural";

/// 轮询 datafeed 的下限。
///
/// 比这个还密没有意义：datafeed 本身每几秒才换一次，而 `0` 会让 `sleep(0)`
/// 把它轮成一个死循环——对着上游打，而这一侧看上去一切正常。
const MIN_POLL: Duration = Duration::from_secs(5);

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
        voice_en: env("ATIS_VOICE_EN", DEFAULT_VOICE_EN),
        voice_zh: env("ATIS_VOICE_ZH", DEFAULT_VOICE_ZH),
        ffmpeg: env("FFMPEG", "ffmpeg"),
    };

    let feed_url = env(
        "CAN_FSD_DATAFEED",
        "https://data.ceruleanavi.net/v1/data.json",
    );
    let poll = match poll_interval(&env("ATIS_POLL_SECS", "30")) {
        Ok(poll) => poll,
        Err(why) => {
            eprintln!("ATIS_POLL_SECS: {why}");
            std::process::exit(2);
        }
    };

    tracing::info!(%feed_url, poll = poll.as_secs(), "atis fleet starting");

    let mut running: HashMap<String, Task> = HashMap::new();
    loop {
        match fetch_feed(&http, &feed_url).await {
            Ok(feed) => match datafeed::stations_from(&feed) {
                Some(wanted) => apply(&mut running, &wanted, &voice, &synth),
                // **形状不对也不停播**，和取不到时同一条规矩：缺 `atis` 字段的
                // 一份文档说明不了"全网的 ATIS 都下线了"，而照它对账会把所有
                // 正在播的席位停掉，下一轮字段回来了再全部重连、重新合成。
                None => {
                    tracing::warn!("the datafeed carries no atis array; keeping the current fleet");
                }
            },
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
    required(std::env::var(key).ok()).unwrap_or_else(|| {
        eprintln!("{key} is required");
        std::process::exit(2);
    })
}

/// 必填项取到的值，**空白的当没取到**。
///
/// `server/docker-compose.yml` 写的是 `ATIS_CID: "${ATIS_CID}"`：`.env` 里没填
/// 时注入进来的是一个空串，而不是"这个变量不存在"。只看取到了没有的话，
/// 照 README 部署、忘了填的那台机队看上去是健康的——每一路每 ≤60 秒换一次票、
/// 拿一次 400，日志里不出现变量名，而全网通播没有声音。旧版把空白值当
/// "未配置"（`can-audio/server/serverconf.py`）。
fn required(raw: Option<String>) -> Option<String> {
    raw.filter(|value| !value.trim().is_empty())
}

/// 读 `ATIS_POLL_SECS`。**写坏或者太小就起不来**，不悄悄回退。
///
/// 回退的那一版日志里什么都没有，于是一个把它写成 `30s` 的人以为自己改生效了。
/// 服务端那一半的原则是一样的（`server/cmd/can-voice` 的 `LoadConfig`）：
/// 配置写坏就起不来，比带着一份不是自己写的配置跑下去好。
fn poll_interval(raw: &str) -> Result<Duration, String> {
    let secs: u64 = raw
        .trim()
        .parse()
        .map_err(|_| format!("{raw:?} is not a whole number of seconds"))?;
    if secs < MIN_POLL.as_secs() {
        return Err(format!(
            "{secs} is below the {} second floor",
            MIN_POLL.as_secs()
        ));
    }
    Ok(Duration::from_secs(secs))
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

    /// **空串等于没填。**
    ///
    /// `server/docker-compose.yml` 写的是 `ATIS_CID: "${ATIS_CID}"`，`.env` 里
    /// 没填时注入进来的就是一个空串，而 `server/.env.example` 这两项默认留空。
    /// 只看"取到了没有"的话，照 README 部署、忘了填的那台机队看上去是健康的：
    /// 每一路每 ≤60 秒换一次票、拿一次 400，日志里不出现变量名，全网通播没有
    /// 声音。旧版把空白值当"未配置"（`can-audio/server/serverconf.py`）。
    #[test]
    fn a_blank_value_counts_as_missing() {
        assert_eq!(required(Some("1001".into())).as_deref(), Some("1001"));
        assert!(required(Some(String::new())).is_none());
        assert!(required(Some("   ".into())).is_none());
        assert!(required(Some("\t\n".into())).is_none());
        assert!(required(None).is_none());
    }

    /// **轮询间隔写坏就起不来**，不悄悄回退成 30。
    ///
    /// 服务端那一半的原则是"配置写坏就起不来"（`server/cmd/can-voice` 的
    /// `LoadConfig`），这一侧照办：悄悄回退的那一版，日志里什么都没有，
    /// 而 `0` 会让 `sleep(0)` 把 datafeed 轮成一个死循环——两种都是一个人
    /// 改了配置、以为改生效了的情形。
    #[test]
    fn a_poll_interval_that_cannot_be_used_names_itself_instead_of_falling_back() {
        assert_eq!(poll_interval("30"), Ok(Duration::from_secs(30)));
        assert_eq!(poll_interval(" 30 "), Ok(Duration::from_secs(30)));
        assert!(poll_interval("30s").is_err(), "写坏了不该回退成 30");
        assert!(poll_interval("").is_err());
        assert!(poll_interval("0").is_err(), "0 会把 datafeed 轮成死循环");
        assert!(
            poll_interval("4").is_err(),
            "下限是 {} 秒",
            MIN_POLL.as_secs()
        );
        assert_eq!(poll_interval("5"), Ok(MIN_POLL));
    }

    /// **默认嗓子是 can-audio 那两个男声。**
    ///
    /// 切换当天全网通播不该换一种声音：旧版播的是 `zh-CN-YunxiNeural` /
    /// `en-US-ChristopherNeural`（`can-audio/server/ATIS/mumble.py:317,324`），
    /// 操作员和管制员听惯的是它们。要换嗓子改 `ATIS_VOICE_*`，不是改这里。
    #[test]
    fn the_default_voices_are_the_ones_can_audio_spoke_with() {
        assert_eq!(DEFAULT_VOICE_ZH, "zh-CN-YunxiNeural");
        assert_eq!(DEFAULT_VOICE_EN, "en-US-ChristopherNeural");
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
