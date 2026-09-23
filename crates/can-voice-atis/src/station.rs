//! 一路通播：连上去、订阅自己的频率、循环播报。
//!
//! # 这一侧**不设有界重连**
//!
//! 四个桌面客户端的规矩是"会话建立之后掉线最多重连三次，全失败就下线"。
//! **机器人反过来**：一支给三次机会就放弃的机队，会在一次网络抖动之后
//! 让全网 ATIS 悄无声息地下线，而没有任何人在看着它。所以这里一直重连，
//! 退避到一个上限。
//!
//! `can-voice-client` 的 `ReconnectPolicy` 管的是一条连接内部；这里是在它
//! 放弃之后**整路重来**，两者不冲突。

use crate::tts::{frames_of, CommandTts};
use crate::{readback, tts};
use can_voice_client::{Config, Event, LinkState, VoiceClient};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

/// 一帧的播出间隔。**必须按这个节奏喂**，见 [`crate::tts::frames_of`]。
const FRAME_INTERVAL: Duration = Duration::from_millis(20);

/// 两轮播报之间的停顿。
const CYCLE_GAP: Duration = Duration::from_secs(3);

/// 别人在这个频率上讲话时，等多久再看一次。
const YIELD_POLL: Duration = Duration::from_millis(500);

/// 整路重来的退避上限。
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// 一路通播要的全部东西。
pub struct Station {
    pub callsign: String,
    pub freq_khz: u32,
    pub voice: VoiceSettings,
    pub tts: CommandTts,
    /// 当前该播的报文。换文本只改这里，**不重开这一路**。
    pub text: tokio::sync::watch::Receiver<String>,
}

/// 连语音服务端要的东西。**没有 token 字段**——票只从 `TokenSource` 来。
#[derive(Debug, Clone)]
pub struct VoiceSettings {
    pub server: String,
    pub server_name: String,
    pub tokens: can_voice_token::TokenSource,
}

/// 一轮是怎么结束的。**退避按它决定**，见 [`wait_after`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ended {
    /// 链路掉了：照常从一秒重来。
    Disconnected,
    /// **被顶号**（关闭码 2）：同一个 `(CID, station)` 在别处登录了。
    Evicted,
    /// 这一轮根本没连上，或者中途报错。
    Failed,
}

/// 跑一路，直到被取消。
pub async fn run(station: Station) {
    let mut backoff = Duration::from_secs(1);
    loop {
        let ended = match cycle(&station).await {
            Ok(ended) => ended,
            Err(e) => {
                tracing::warn!(callsign = %station.callsign, error = %e, "station cycle ended");
                Ended::Failed
            }
        };
        if ended == Ended::Evicted {
            // 单独一条：这不是网络问题，而是**有第二个进程在用同一个账号播这个
            // 席位**。说清楚了，操作员才会去关掉那一套，而不是来查这一套的网络。
            tracing::warn!(
                callsign = %station.callsign,
                "evicted: another process is broadcasting this station with the same account; \
                 backing off instead of reconnecting"
            );
        }
        // 一直重来。见模块文档：放弃等于让这个席位悄无声息地下线。
        let wait = wait_after(ended, backoff);
        tokio::time::sleep(wait).await;
        backoff = (wait * 2).min(MAX_BACKOFF);
    }
}

/// 这一轮结束之后等多久再来一次。
///
/// # 被顶号**不重置退避**
///
/// 关闭码 2 的契约是"必须停止重连"（`server/README.md` 的关闭码表），而机队是
/// 这个仓库自己的客户端。这一侧不能真的停下——停了就没有人把这个席位拉回来，
/// 而顶掉它的那一套可能几分钟后就下线了——所以取最大退避：同一个 `ATIS_CID`
/// 跑了两套机队时，两边不再每秒互踢一轮，而一路等一分钟再试一次。
fn wait_after(ended: Ended, backoff: Duration) -> Duration {
    match ended {
        // 一次网络抖动不该让这个席位静默一分钟。
        Ended::Disconnected => Duration::from_secs(1),
        Ended::Evicted => MAX_BACKOFF,
        // 连都没连上：退避照常往上走。
        Ended::Failed => backoff,
    }
}

/// 一路通播拿什么身份去连语音服务端。
///
/// `station` names the signed ATIS seat; the server uses that signed identity
/// for both eviction and position lookup. Legacy `follow` stays empty.
fn voice_config(voice: &VoiceSettings, callsign: &str) -> Config {
    Config {
        server: voice.server.clone(),
        server_name: voice.server_name.clone(),
        // 这个值会被 `can_voice_token::connect` 覆盖；票只从 TokenSource 来。
        token: String::new(),
        client_id: concat!("can-voice-atis/", env!("CARGO_PKG_VERSION")).into(),
        follow: String::new(),
        station: callsign.to_string(),
        input_device: None,
        output_device: None,
        // 没有声卡：这台机器上没有麦克风，音频是 TTS 合成出来的。
        audio_devices: false,
        extra_roots: Vec::new(),
    }
}

async fn cycle(station: &Station) -> Result<Ended, Box<dyn std::error::Error + Send + Sync>> {
    // **每次连接都现换一张票，过期了就再换一张试一次**（`can_voice_token::connect`）。
    // token 的有效期是 60 秒，攒着没有意义；而且重连走的是票不是密码，
    // 不消耗登录限流的配额。
    let tokens = station
        .voice
        .tokens
        .clone()
        .with_scope(can_voice_token::TokenScope::atis(
            &station.callsign,
            station.freq_khz,
        ));
    let client =
        can_voice_token::connect(voice_config(&station.voice, &station.callsign), &tokens).await?;

    client.set_subscription(can_voice_proto::control::Sub {
        // 也订阅接收：要听得见别人有没有在这个频率上讲话，好让出频率。
        rx: vec![station.freq_khz],
        tx: vec![station.freq_khz],
        ..Default::default()
    });

    let busy = Arc::new(AtomicBool::new(false));
    let offline = Arc::new(AtomicBool::new(false));
    let evicted = Arc::new(AtomicBool::new(false));
    tokio::spawn(watch_events(
        client.events(),
        station.freq_khz,
        busy.clone(),
        offline.clone(),
        evicted.clone(),
    ));

    tracing::info!(callsign = %station.callsign, freq = station.freq_khz, "station on the air");

    // 合成结果按整篇稿子缓存。**播报循环每三秒要同一段 PCM**，不缓存的话一份
    // 一整天不变的通播会一天几千次去开 `edge-tts` 和 `ffmpeg` 两个子进程。
    let mut cache = crate::tts::PcmCache::default();

    while !offline.load(Ordering::Relaxed) {
        // **取文本是在一轮开始的时候。** 报文变了换的是下一轮，
        // 不会把正在播的那一轮从中间切断。
        let text = station.text.borrow().clone();
        if text.is_empty() {
            tokio::time::sleep(CYCLE_GAP).await;
            continue;
        }

        let pcm = match cache.get(&text) {
            Some(pcm) => pcm,
            None => {
                let pcm = Arc::new(synthesize(&station.tts, &text).await?);
                cache.put(text, pcm.clone());
                pcm
            }
        };
        broadcast(&client, &pcm, &busy, &offline).await;
        tokio::time::sleep(CYCLE_GAP).await;
    }
    client.shutdown().await;
    Ok(if evicted.load(Ordering::Relaxed) {
        Ended::Evicted
    } else {
        Ended::Disconnected
    })
}

/// 把报文合成成一整段 PCM。中英混合时两半各用各的嗓子，**中文在前**——
/// 顺序是 [`halves`] 给的，这里只负责按那个顺序接起来。
async fn synthesize(
    tts: &CommandTts,
    text: &str,
) -> Result<Vec<i16>, Box<dyn std::error::Error + Send + Sync>> {
    let mut pcm = Vec::new();
    for (half, chinese) in halves(text) {
        pcm.extend(tts.speak(&half, chinese).await?);
    }
    Ok(pcm)
}

/// 把一段报文切成"要念的几段"，每段带它该用哪种嗓子，**按播出的顺序**。
///
/// # 嗓子按文本里有没有汉字选，不看分隔符
///
/// 机队播 datafeed 里**所有** `_ATIS` 席位，不只是 atis-for-can 发出来的那些。
/// EuroScope 或者别的来源发的纯中文通播没有 `|`，按分隔符判的那一版会把整段
/// 交给 `en-US-*` 嗓子，要么念不出来，要么乱念。旧版判的是文本里有没有汉字
/// （`can-audio/server/ATIS/mumble.py:307`，`[一-鿿]`），那就是
/// [`readback::has_chinese`]。
///
/// **[`readback::process`] 选读法调的是同一个函数。** 两处各判一次的时候正是
/// 这样对不上的：这一步按汉字选对了嗓子，那一步按分隔符把整段当英文，于是
/// 中文嗓子念出 "one zero zero seven"、"niner"。分隔符现在只管一件事——有且
/// 只有一个时，它把英文那一半和中文那一半分开。
///
/// # 中文在前
///
/// 照 can-audio 的播出顺序：先中文、再英文（`mumble.py:371-388`）。稿子里
/// 英文在前那是分隔符的约定，不是播出的顺序。
///
/// # 空的那一半跳过
///
/// 一个只播中文的席位把英文那半留空（`|中文`），那是它表达"这一份只有中文"的
/// 唯一写法。拿空串去开一个 TTS 子进程，好一点是白开一次，差一点是它报错，
/// 而报错会让整段合成失败，于是这个席位一声不出。
fn halves(text: &str) -> Vec<(String, bool)> {
    let spoken = readback::process(text);
    let chunks = match spoken.split_once(readback::SEPARATOR) {
        Some((en, zh)) => vec![en, zh],
        None => vec![spoken.as_str()],
    };
    let mut out: Vec<(String, bool)> = chunks
        .into_iter()
        .map(str::trim)
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| (chunk.to_string(), readback::has_chinese(chunk)))
        .collect();
    // 稳定排序：中文那几段排到前面，各自内部的顺序不变。
    out.sort_by_key(|(_, chinese)| !*chinese);
    out
}

/// 给 [`crate::script`] 的测试用：语言怎么切是那边要断言的事，
/// 但切分本身归这里管，**不要在那边抄一份**。
#[cfg(test)]
pub fn halves_for_test(text: &str) -> Vec<(String, bool)> {
    halves(text)
}

/// 播一遍。别人开口就让出频率。
async fn broadcast(client: &VoiceClient, pcm: &[i16], busy: &AtomicBool, offline: &AtomicBool) {
    let frames = frames_of(pcm);
    tracing::debug!(
        frames = frames.len(),
        secs = frames.len() as f32 * 0.02,
        "broadcasting"
    );

    client.set_transmitting(true);
    for frame in frames {
        if offline.load(Ordering::Relaxed) {
            break;
        }
        // **别人在讲话就让开。** 通播压住一次真实的通话，比晚播一轮糟得多。
        if busy.load(Ordering::Relaxed) {
            client.set_transmitting(false);
            while busy.load(Ordering::Relaxed) && !offline.load(Ordering::Relaxed) {
                tokio::time::sleep(YIELD_POLL).await;
            }
            // 让开之后这一轮就不接着播了：从中间续上会让听的人接到半句话。
            return;
        }
        client.push_audio(&frame);
        tokio::time::sleep(FRAME_INTERVAL).await;
    }
    client.set_transmitting(false);
}

/// 这个频率上谁在讲话。布尔量不够：同频两个人时，第一个人的 `RxEnd`
/// 会把通播放回去压到还在说话的人身上。
struct Occupancy {
    speakers: HashSet<u32>,
}

impl Occupancy {
    fn new() -> Self {
        Self {
            speakers: HashSet::new(),
        }
    }

    fn apply(&mut self, event: &Event, freq_khz: u32) {
        match event {
            Event::RxStart {
                freq_khz: f,
                speaker,
            } if *f == freq_khz => {
                self.speakers.insert(*speaker);
            }
            Event::RxEnd {
                freq_khz: f,
                speaker,
                ..
            } if *f == freq_khz => {
                self.speakers.remove(speaker);
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.speakers.clear();
    }

    fn busy(&self) -> bool {
        !self.speakers.is_empty()
    }
}

enum Watch {
    Continue,
    Stop,
    /// 被顶号（关闭码 2）。和 `Stop` 分开只为了退避那一步，见 [`wait_after`]。
    Evicted,
}

/// 一条事件（或一次落后）对占用状态做什么。抽出来才能钉住"两个人同频"
/// 和"落后时清空"，那两条都在 `watch_events` 的 `select` 里测不到。
fn note(occupancy: &mut Occupancy, freq_khz: u32, incoming: Result<Event, RecvError>) -> Watch {
    match incoming {
        Ok(Event::State(LinkState::Evicted)) => Watch::Evicted,
        Ok(Event::State(LinkState::Offline)) => Watch::Stop,
        Ok(event) => {
            occupancy.apply(&event, freq_khz);
            Watch::Continue
        }
        // 丢掉的可能是 RxEnd。不清空的话 busy 卡在 true，这一路一直让。
        // 落后等于最新状态未知：当空闲，下一帧 RxStart 会再占上。
        Err(RecvError::Lagged(_)) => {
            occupancy.reset();
            Watch::Continue
        }
        Err(_) => Watch::Stop,
    }
}

/// 盯着事件流：别人在我们的频率上开口了吗，链路还在吗。
async fn watch_events(
    mut events: tokio::sync::broadcast::Receiver<Event>,
    freq_khz: u32,
    busy: Arc<AtomicBool>,
    offline: Arc<AtomicBool>,
    evicted: Arc<AtomicBool>,
) {
    let mut occupancy = Occupancy::new();
    loop {
        match note(&mut occupancy, freq_khz, events.recv().await) {
            Watch::Continue => busy.store(occupancy.busy(), Ordering::Relaxed),
            Watch::Evicted => {
                evicted.store(true, Ordering::Relaxed);
                offline.store(true, Ordering::Relaxed);
                return;
            }
            Watch::Stop => {
                offline.store(true, Ordering::Relaxed);
                return;
            }
        }
    }
}

/// 一帧的采样数，转出去给对账用。
pub const FRAME_SAMPLES: usize = tts::FRAME_SAMPLES;

#[cfg(test)]
mod tests {
    use super::*;

    /// 中英混合切成两段，各用各的嗓子，**中文在前**。
    ///
    /// 顺序照 can-audio：那一版先播中文、再播英文
    /// （`can-audio/server/ATIS/mumble.py:371-388`）。按稿子里的顺序播的话英文
    /// 在前，而切换当天听的人会以为播错了——稿子的顺序是分隔符的约定
    /// （英文在前），不是播出的顺序。
    #[test]
    fn a_bilingual_report_is_spoken_in_two_voices_chinese_first() {
        let got = halves("QNH 1007|修正海压 1007");
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got[0].1, "the chinese half is spoken first: {got:?}");
        assert!(got[0].0.contains("幺"), "{got:?}");
        assert!(!got[1].1, "the english half must not use the chinese voice");
        assert!(got[1].0.contains("one"), "{got:?}");
    }

    /// **只有中文的那一份把英文半边留空**，切出来只有一段，而且是中文嗓子。
    ///
    /// 不跳过的话会拿空串去开一个 TTS 子进程，而它一报错整段合成就失败，
    /// 这个席位一声不出。
    #[test]
    fn an_empty_half_is_not_handed_to_the_synthesizer() {
        let got = halves("|修正海压 1007");
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].1, "a chinese-only report must use the chinese voice");
        assert!(got[0].0.contains("幺"), "{got:?}");

        // 中文那半留空同理：切出来就是纯英文那一段。
        assert_eq!(halves("QNH 1007|"), halves("QNH 1007"));
        assert!(halves("|").is_empty());
        assert!(halves("").is_empty());
    }

    /// **不带分隔符的报文按它自己写的语言选嗓子**，不是一律交给英文嗓子。
    ///
    /// 机队播 datafeed 里所有 `_ATIS` 席位，不只是 atis-for-can 发出来的那些：
    /// EuroScope 或者别的来源发的纯中文通播没有 `|`，交给 `en-US-*` 嗓子要么
    /// 念不出来，要么乱念。旧版按文本里有没有汉字选嗓子
    /// （`can-audio/server/ATIS/mumble.py:307`，`[一-鿿]`），与分隔符无关。
    #[test]
    fn a_report_without_a_separator_is_read_in_the_language_it_is_written_in() {
        let english = halves("QNH 1007");
        assert_eq!(english.len(), 1, "{english:?}");
        assert!(!english[0].1, "{english:?}");

        let chinese = halves("上海浦东机场通播 修正海压 1007");
        assert_eq!(chinese.len(), 1, "{chinese:?}");
        assert!(
            chinese[0].1,
            "a chinese report must use the chinese voice: {chinese:?}"
        );
    }

    /// **选嗓子和选读法必须是同一个判据。**
    ///
    /// 嗓子归这里选（按有没有汉字），读法归 [`readback::process`] 选。两步各用
    /// 各的判据时，一份没有 `|` 的纯中文通播用中文嗓子念出 "one zero zero
    /// seven"、"niner"——嗓子对了，数字还是英文的。
    #[test]
    fn the_voice_and_the_readback_agree_on_the_language() {
        let got = halves("上海浦东机场通播 修正海压 1007 使用跑道 29");
        assert_eq!(got.len(), 1, "{got:?}");
        let (spoken, chinese) = &got[0];
        assert!(chinese, "{got:?}");
        assert!(spoken.contains("幺 洞 洞 拐"), "{spoken}");
        assert!(!spoken.contains("one"), "{spoken}");
        assert!(!spoken.contains("niner"), "{spoken}");
    }

    fn voice_for_test() -> VoiceSettings {
        VoiceSettings {
            server: "voice.example:64738".into(),
            server_name: "voice.example".into(),
            tokens: can_voice_token::TokenSource::new(
                "https://api.example",
                "900",
                "pw",
                reqwest::Client::new(),
            ),
        }
    }

    /// 一路通播用自己的呼号报两个字段，两个都是必需的，修的是同一件事的两半。
    ///
    /// `station`：整队共用一个 `ATIS_CID`，而顶号按 CID。不带席位标记时
    /// 两路以上的机队每秒互踢一轮，而两端日志都写着"成功"。
    ///
    /// `follow`：射程过滤要查这一路的位置，查不到 `follow` 就退回按 CID 查，
    /// 而 fsdfeed 对同一个 CID 下的多条 ATIS 只留呼号字典序最小的那一条——
    /// 于是浦东的通播按北京的位置过滤，安静地放行或者安静地挡掉。
    #[test]
    fn a_station_dials_with_its_own_callsign_in_both_fields() {
        let cfg = voice_config(&voice_for_test(), "ZSPD_ATIS");
        assert_eq!(cfg.station, "ZSPD_ATIS");
        assert!(cfg.follow.is_empty());
        // 票只从 TokenSource 来：这个字段会被 can_voice_token::connect 覆盖。
        assert!(cfg.token.is_empty(), "{:?}", cfg.token);
        // 这台机器上没有声卡，音频是 TTS 合成出来的。
        assert!(!cfg.audio_devices);
    }

    fn start(speaker: u32) -> Event {
        Event::RxStart {
            freq_khz: 121_800,
            speaker,
        }
    }

    fn end(speaker: u32) -> Event {
        Event::RxEnd {
            freq_khz: 121_800,
            speaker,
            frames: 1,
            secs: 0.02,
        }
    }

    /// 同频两个人，一个说完了，频率还是忙的。布尔量会在第一个人松手时让通播
    /// 压到还在说话的人身上。桌面快照已经钉过同一件事。
    #[test]
    fn one_speaker_finishing_does_not_free_the_frequency_someone_else_is_using() {
        let mut o = Occupancy::new();
        o.apply(&start(7), 121_800);
        o.apply(&start(9), 121_800);
        o.apply(&end(7), 121_800);
        assert!(o.busy(), "speaker 9 is still talking");
        o.apply(&end(9), 121_800);
        assert!(!o.busy());
    }

    #[test]
    fn a_start_on_another_frequency_is_ignored() {
        let mut o = Occupancy::new();
        o.apply(
            &Event::RxStart {
                freq_khz: 118_000,
                speaker: 1,
            },
            121_800,
        );
        assert!(!o.busy());
    }

    /// 事件流落后时丢掉的可能是 RxEnd。不清空的话 busy 会卡在 true，这一路
    /// 一直让、一直不播。落后等于"最新状态未知"，乐观地当空闲，下一帧 RxStart
    /// 会再占上。
    #[test]
    fn a_lagged_event_stream_stops_looking_busy() {
        let mut o = Occupancy::new();
        o.apply(&start(7), 121_800);
        assert!(o.busy());
        let watch = note(
            &mut o,
            121_800,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(3)),
        );
        assert!(matches!(watch, Watch::Continue));
        assert!(!o.busy());
    }

    #[test]
    fn going_offline_stops_the_watch() {
        let mut o = Occupancy::new();
        let watch = note(&mut o, 121_800, Ok(Event::State(LinkState::Offline)));
        assert!(matches!(watch, Watch::Stop));
    }

    /// 被顶号要和普通掉线**分得开**，否则下面那条规矩无从谈起。
    #[test]
    fn an_eviction_is_told_apart_from_an_ordinary_disconnect() {
        let mut o = Occupancy::new();
        let evicted = note(&mut o, 121_800, Ok(Event::State(LinkState::Evicted)));
        assert!(matches!(evicted, Watch::Evicted));
        let offline = note(&mut o, 121_800, Ok(Event::State(LinkState::Offline)));
        assert!(matches!(offline, Watch::Stop));
    }

    /// **被顶号之后不回到一秒。**
    ///
    /// 关闭码 2 的契约是"必须停止重连"（`server/README.md` 的关闭码表），
    /// 而机队是这个仓库自己的客户端。一秒就重连的那一版，在同一个 `ATIS_CID`
    /// 跑了两套机队时（演练环境连到了生产服务端）两边按席位每秒互踢一轮，
    /// 报文永远播不完，而两边的日志看上去都正常。
    #[test]
    fn being_evicted_does_not_send_this_station_straight_back() {
        assert_eq!(
            wait_after(Ended::Evicted, Duration::from_secs(1)),
            MAX_BACKOFF
        );
        // 干净地掉线照旧从一秒重来：一次网络抖动不该让这个席位静默一分钟。
        assert_eq!(
            wait_after(Ended::Disconnected, MAX_BACKOFF),
            Duration::from_secs(1)
        );
        // 连都没连上：退避照常往上走，不重置也不跳到上限。
        assert_eq!(
            wait_after(Ended::Failed, Duration::from_secs(8)),
            Duration::from_secs(8)
        );
    }

    /// **一路通播的配置打进日志时不许带出密码。**
    ///
    /// `VoiceSettings` 装着一个 `TokenSource`，而里面是网站账号的明文密码——
    /// 机队的日志进容器日志。挡在 `TokenSource` 那一层，这里派生 `Debug` 就够。
    #[test]
    fn the_voice_settings_do_not_carry_the_password_into_the_log() {
        let mut voice = voice_for_test();
        voice.tokens = can_voice_token::TokenSource::new(
            "https://api.example",
            "900",
            "correct-horse-battery-staple",
            reqwest::Client::new(),
        );
        let shown = format!("{voice:?}");
        assert!(!shown.contains("correct-horse"), "密码进了日志: {shown}");
    }
}
