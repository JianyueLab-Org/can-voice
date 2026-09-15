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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

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

/// 跑一路，直到被取消。
pub async fn run(station: Station) {
    let mut backoff = Duration::from_secs(1);
    loop {
        match cycle(&station).await {
            Ok(()) => backoff = Duration::from_secs(1),
            Err(e) => {
                tracing::warn!(callsign = %station.callsign, error = %e, "station cycle ended");
            }
        }
        // 一直重来。见模块文档：放弃等于让这个席位悄无声息地下线。
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

async fn cycle(station: &Station) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // **每次连接都现换一张票，过期了就再换一张试一次**（`can_voice_token::connect`）。
    // token 的有效期是 60 秒，攒着没有意义；而且重连走的是票不是密码，
    // 不消耗登录限流的配额。
    let client = can_voice_token::connect(
        Config {
            server: station.voice.server.clone(),
            server_name: station.voice.server_name.clone(),
            // 这个值会被 `can_voice_token::connect` 覆盖；票只从 TokenSource 来。
            token: String::new(),
            client_id: concat!("can-voice-atis/", env!("CARGO_PKG_VERSION")).into(),
            follow: String::new(),
            input_device: None,
            output_device: None,
            // 没有声卡：这台机器上没有麦克风，音频是 TTS 合成出来的。
            audio_devices: false,
            extra_roots: Vec::new(),
        },
        &station.voice.tokens,
    )
    .await?;

    client.set_subscription(can_voice_proto::control::Sub {
        // 也订阅接收：要听得见别人有没有在这个频率上讲话，好让出频率。
        rx: vec![station.freq_khz],
        tx: vec![station.freq_khz],
        ..Default::default()
    });

    let busy = Arc::new(AtomicBool::new(false));
    let offline = Arc::new(AtomicBool::new(false));
    tokio::spawn(watch_events(
        client.events(),
        station.freq_khz,
        busy.clone(),
        offline.clone(),
    ));

    tracing::info!(callsign = %station.callsign, freq = station.freq_khz, "station on the air");

    while !offline.load(Ordering::Relaxed) {
        // **取文本是在一轮开始的时候。** 报文变了换的是下一轮，
        // 不会把正在播的那一轮从中间切断。
        let text = station.text.borrow().clone();
        if text.is_empty() {
            tokio::time::sleep(CYCLE_GAP).await;
            continue;
        }

        let pcm = synthesize(&station.tts, &text).await?;
        broadcast(&client, &pcm, &busy, &offline).await;
        tokio::time::sleep(CYCLE_GAP).await;
    }
    client.shutdown().await;
    Ok(())
}

/// 把报文合成成一整段 PCM。中英混合时两半各用各的嗓子，按序接起来。
async fn synthesize(
    tts: &CommandTts,
    text: &str,
) -> Result<Vec<i16>, Box<dyn std::error::Error + Send + Sync>> {
    let spoken = readback::process(text);
    let mut pcm = Vec::new();
    match spoken.split_once(readback::SEPARATOR) {
        Some((en, zh)) => {
            pcm.extend(tts.speak(en.trim(), false).await?);
            pcm.extend(tts.speak(zh.trim(), true).await?);
        }
        None => pcm.extend(tts.speak(spoken.trim(), false).await?),
    }
    Ok(pcm)
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

/// 盯着事件流：别人在我们的频率上开口了吗，链路还在吗。
async fn watch_events(
    mut events: tokio::sync::broadcast::Receiver<Event>,
    freq_khz: u32,
    busy: Arc<AtomicBool>,
    offline: Arc<AtomicBool>,
) {
    loop {
        match events.recv().await {
            Ok(Event::RxStart { freq_khz: f, .. }) if f == freq_khz => {
                busy.store(true, Ordering::Relaxed)
            }
            Ok(Event::RxEnd { freq_khz: f, .. }) if f == freq_khz => {
                busy.store(false, Ordering::Relaxed)
            }
            Ok(Event::State(LinkState::Offline | LinkState::Evicted)) => {
                offline.store(true, Ordering::Relaxed);
                return;
            }
            Ok(_) => {}
            // 跟不上就重来：事件是广播，落后的订阅者会被丢帧，而我们只关心
            // 最新的状态。
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(_) => {
                offline.store(true, Ordering::Relaxed);
                return;
            }
        }
    }
}

/// 一帧的采样数，转出去给对账用。
pub const FRAME_SAMPLES: usize = tts::FRAME_SAMPLES;
