//! 公开 API。
//!
//! **声明式，不是命令式。** 调用方声明"我要收哪些频率、发哪些频率"，
//! 库负责让服务端状态收敛过去，重连后自动重发。
//!
//! 这里刻意**没有** `join_channel()`、`leave_channel()`、`channel_id`。
//! 旧的 Python 实现里那一整类"UI 是绿的但人还在 root 频道"的 bug，
//! 根源就是把"我在哪个频道"当成一个可以记住的事实 —— 重连之后服务端
//! 把人放回 root，而客户端的记录还是掉线前的值，于是它认为
//! "已经在那儿了"，永远不再重入，而界面全程是绿的。
//! 声明式 API 里没有那个可以记错的字段。

use crate::conn::{self, LinkState, RefusedReason};
use crate::session::Limits;
use can_voice_proto::control::Sub;

/// 建立连接所需的一切。
#[derive(Debug, Clone)]
pub struct Config {
    /// `host:port`。
    pub server: String,
    /// TLS 的服务器名，通常与 `server` 的主机部分相同。
    pub server_name: String,
    /// can-api 签发的短期 token。
    pub token: String,
    /// 客户端标识，如 `can-controller/3.0.0`，只进服务端日志。
    pub client_id: String,
    /// 观察员模式跟随的呼号；不是观察员时留空。
    pub follow: String,
    /// 席位标记：同一个账号下的哪一路。**整队共用一个 CID 时必须填。**
    ///
    /// 顶号按 `(cid, station)` 判。四个桌面客户端留空——它们一人一个账号，
    /// 留空时的行为和以前完全一样：同一个成员号第二次登录顶掉第一条。
    /// 服务端 ATIS 机队不能留空：整队一个 `ATIS_CID`，不带席位标记的话
    /// 每一路都在顶掉另一路，而两端日志都写着"成功"。
    pub station: String,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    /// 要不要打开声卡。
    ///
    /// 四个桌面客户端当然要。**服务端 ATIS 机器人不要**——它跑在没有声卡的主机上，
    /// 音频来自 TTS，经 [`VoiceClient::push_audio`] 注进来。端到端测试同理。
    ///
    /// 打不开声卡**不是致命错误**：会话照常建立，只是既听不见也说不出，
    /// 而那比"整个客户端起不来"好——can-audio 那边的规矩也是这一条。
    pub audio_devices: bool,
    /// 额外信任的根证书，DER 编码。**生产留空。**
    ///
    /// 这**不是**"跳过校验"的开关，而且这里永远不会有那样一个开关：一个
    /// `insecure` 标志一旦存在就会有人在生产里打开它，而这条链路上跑的是成员的
    /// 网络密码。传进来的证书仍然要过完整的链校验，只是多了一个根。
    /// 端到端测试用它信任自签的服务端证书——在测试里和"不校验"一样方便，
    /// 在生产里天差地别。
    pub extra_roots: Vec<Vec<u8>>,
}

impl Config {
    pub(crate) fn trust_roots(&self) -> crate::conn::TrustRoots {
        if self.extra_roots.is_empty() {
            crate::conn::TrustRoots::Platform
        } else {
            crate::conn::TrustRoots::Extra(
                self.extra_roots.iter().cloned().map(Into::into).collect(),
            )
        }
    }
}

/// 库向上层报告的事件。
///
/// 上层（Tauri 应用）据此更新界面并写日志。**这里的字符串都是英文且只进日志**；
/// 面向用户的中文文案属于上层，不属于这里。
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// 链路状态变了。`Reconnecting` 与 `Offline` 是对立的：
    /// 前者意味着链路还活着，上层**不要丢掉对象引用**；后者意味着它没了。
    ///
    /// **只在真的变了的时候发。** 每轮都发一遍会把事件流灌满，
    /// 而上层没法从中分辨"状态变了"和"心跳到了"。
    State(LinkState),
    /// 这一条链路的限额，来自 READY。**每次连上都发一次，在 `State(Online)` 之前。**
    ///
    /// 上层要靠它在**声明之前**提示超额，而不是等 `TxDenied` 事后冒出来。掉线之后
    /// 它就不作数了：上限写在票里、按人签，下一次 READY 可能是另一个数。
    Limits(Limits),
    /// 握手被拒，带服务端给的原因。
    ///
    /// 这几个串是服务端**专门为客户端造的**：只把它们送进日志，等于协议里
    /// 唯一一处专门为客户端设计的东西客户端用不上，而用户只看到一个没有理由的
    /// `Offline`。`TokenExpired` 是唯一可恢复的一条。
    Refused { reason: RefusedReason },
    /// 某个频率上有人开始讲话。
    RxStart { freq_khz: u32, speaker: u32 },
    /// 发言者的会话号对应哪个 CAN 号。每个发言者只来一次。
    ///
    /// 数据面包头里的 `speaker` 是会话 id，电台行要显示的是呼号：先拿到 CID，
    /// 再去 datafeed 花名册翻。
    Talker {
        session: u32,
        cid: String,
        freq_khz: u32,
    },
    /// 某个频率上有人讲完了。
    ///
    /// 带上帧数和时长，是因为日志约定是**每次通话一行**而不是两行 ——
    /// 开始那一行在 DEBUG，这一行在 INFO 且自带全部信息。
    /// 这两个字段要**实算**：唯一的生产者填 0 的话，这条约定就只剩一句空话。
    RxEnd {
        freq_khz: u32,
        speaker: u32,
        frames: u32,
        secs: f32,
    },
    /// 声明了 TX 却没拿到。由**差集**得出（`发出去的 tx − ack.tx`），
    /// 不是从 `rejected` 里推断方向；`reason` 为空表示它来自差集而不是 NOTICE。
    TxDenied { freq_khz: u32, reason: String },
    /// 声明了 RX 却没拿到。同样由差集得出。
    ///
    /// 它和 [`Event::TxDenied`] 必须是两件事：一个频率同时出现在 `ack.rx` 和
    /// `rejected` 里是**正常**的（TX 被限额拒了、RX 给了），混成一件会把一个
    /// 能听的频率显示成失败。
    RxDenied { freq_khz: u32 },
    /// Effective radio state from the latest matching SUBACK.
    SubscriptionAck {
        rx: Vec<u32>,
        tx: Vec<u32>,
        xc: Vec<[u32; 2]>,
    },
    /// 一对交叉耦合没有生效。
    ///
    /// 耦合对不在 `rx`/`tx` 里，差集公式管不到它，所以它必须有自己的事件。
    XcDenied {
        a_khz: u32,
        b_khz: u32,
        reason: String,
    },
    /// 服务端的其它通知（`range_unavailable`、`sub_rejected`、`unknown_message`）。
    Notice {
        kind: String,
        freq_khz: u32,
        reason: String,
    },
    /// 周期性的链路健康报告。
    ///
    /// 掉线时这些数字要跟着掉线日志一起打出来：
    /// "上行真的跟不上"和"网络抖了一下"需要完全不同的处理，
    /// 而旧实现的日志只写了一句 "voice connection dropped"。
    Health {
        rtt_ms: u32,
        sent: u64,
        received: u64,
        /// QUIC 自己认定丢掉的包。**不是包头解不开的那些**——见 `unparsable`。
        lost: u64,
        /// 包头解不开的数据报。这是协议漂移，不是网络丢包，
        /// 混进 `lost` 会让后者在正常运行里恒为 0。
        unparsable: u64,
        /// 播放环的对账数字。**链路好和听得清是两件事。**
        ///
        /// 上面那几个数只讲到声卡门口为止：链路一切正常而播放环跑干的时候，
        /// 它们全是绿的，而用户听到的是电音加卡顿。没有声卡（服务端 ATIS
        /// 机器人、`audio_devices: false`）时它是全零。
        playback: crate::audio::PlaybackStats,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Conn(#[from] conn::Error),
    #[error("address {0:?} could not be resolved")]
    BadAddress(String),
}

/// 语音客户端。
#[derive(Debug)]
pub struct VoiceClient {
    events_tx: tokio::sync::broadcast::Sender<Event>,
    /// 和通道一起建的那个接收端，留给**第一次** [`VoiceClient::events`]。见那里。
    first_events: std::sync::Mutex<Option<tokio::sync::broadcast::Receiver<Event>>>,
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
    injected_audio: std::sync::Arc<InjectedAudio>,
}

pub(crate) const MAX_INJECTED_AUDIO_SAMPLES: usize =
    crate::rx::decode::FRAME_SAMPLES * crate::tx::capture::MAX_BUFFERED_FRAMES;

#[derive(Debug, Default)]
pub(crate) struct InjectedAudio {
    pending: std::sync::Mutex<Vec<i16>>,
}

impl InjectedAudio {
    fn push(&self, samples: &[i16]) {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        if samples.len() >= MAX_INJECTED_AUDIO_SAMPLES {
            pending.clear();
            pending.extend_from_slice(&samples[samples.len() - MAX_INJECTED_AUDIO_SAMPLES..]);
        } else {
            let excess = (pending.len() + samples.len()).saturating_sub(MAX_INJECTED_AUDIO_SAMPLES);
            pending.drain(..excess);
            pending.extend_from_slice(samples);
        }
    }

    pub(crate) fn take(&self) -> Vec<i16> {
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        std::mem::take(&mut *pending)
    }
}

/// 上层发给后台任务的指令。内部类型。
#[derive(Debug)]
pub(crate) enum Command {
    Declare(Sub),
    Transmit(bool),
    Volume {
        freq_khz: u32,
        gain: f32,
    },
    SpeakerVolume {
        speaker: u32,
        freq_khz: u32,
        gain: f32,
    },
    /// 换录音 / 播放设备。
    Devices {
        input: Option<String>,
        output: Option<String>,
    },
    /// 麦克风 / 喇叭总音量。1.0 是原声，和原来 voice 的 100% 同一量纲。
    Master {
        mic: f32,
        speaker: f32,
    },
    Shutdown,
}

impl VoiceClient {
    /// 连接并**完成握手**之后才返回。
    ///
    /// 这一点是承重的：错的主机名、过期的 token、版本太旧的客户端，都在这里变成
    /// 一条说得出原因的 `Err`。原设计在 `tokio::spawn` 之后立刻 `Ok`，任何 I/O 都
    /// 还没发生，于是上面三种全都退化成几秒之后一个没有理由的 `Offline`，
    /// 而 `Error::BadAddress` 根本没有构造点。
    ///
    /// 失败时**不残留任何资源** —— 旧实现里没做到这一点的后果是：PyAudio 没关，
    /// 麦克风被占着，下一次尝试报"打不开音频设备"，把用户指向声卡；
    /// 而带 reconnect 的连接对象还活着，变成一个僵尸无限重试，
    /// 把账号锁出语音，改对密码也没用，只能重启应用。
    pub async fn connect(cfg: Config) -> Result<Self, Error> {
        let addr = resolve(&cfg.server).await?;
        let link = conn::connect(
            addr,
            &cfg.server_name,
            conn::Identity {
                token: &cfg.token,
                client_id: &cfg.client_id,
                follow: &cfg.follow,
                station: &cfg.station,
            },
            cfg.trust_roots(),
        )
        .await?;

        let (client, events_tx, cmd_rx) = Self::wired();
        let injected_audio = std::sync::Arc::clone(&client.injected_audio);
        tokio::spawn(crate::pump::run(
            cfg,
            link,
            events_tx,
            cmd_rx,
            injected_audio,
        ));
        Ok(client)
    }

    /// 建好客户端和它的两条通道，后台任务那一端交给调用方。
    ///
    /// 事件通道的接收端**在后台任务起来之前就建好、留着**，见 [`Self::events`]。
    fn wired() -> (
        Self,
        tokio::sync::broadcast::Sender<Event>,
        tokio::sync::mpsc::UnboundedReceiver<Command>,
    ) {
        let (events_tx, first) = tokio::sync::broadcast::channel(256);
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();
        let client = VoiceClient {
            events_tx: events_tx.clone(),
            first_events: std::sync::Mutex::new(Some(first)),
            commands: cmd_tx,
            injected_audio: std::sync::Arc::new(InjectedAudio::default()),
        };
        (client, events_tx, cmd_rx)
    }

    /// 声明完整的收发意图。反复调用是廉价的 —— 每次都是全量，
    /// 所以一连串 UI 操作只会产生最后那一条网络消息。
    pub fn set_subscription(&self, sub: Sub) {
        let _ = self.commands.send(Command::Declare(sub));
    }

    /// 按下 / 松开 PTT。
    pub fn set_transmitting(&self, on: bool) {
        let _ = self.commands.send(Command::Transmit(on));
    }

    /// 设置某个频率的播放音量。
    pub fn set_frequency_volume(&self, freq_khz: u32, gain: f32) {
        let _ = self.commands.send(Command::Volume { freq_khz, gain });
    }

    /// Sets the manual playback coefficient for one remote speaker on one frequency.
    pub fn set_speaker_volume(&self, speaker: u32, freq_khz: u32, gain: f32) {
        let _ = self.commands.send(Command::SpeakerVolume {
            speaker,
            freq_khz,
            gain,
        });
    }

    /// 直接送一段 48 kHz 单声道 PCM 去发送，绕过麦克风。
    ///
    /// 给**没有声卡的调用方**用：服务端 ATIS 机器人跑在一台没有麦克风的主机上，
    /// 它的音频是 TTS 合成出来的。照样要按 [`Self::set_transmitting`] 开关 PTT
    /// ——序号、首帧尾帧、扇出到每个 TX 频率，走的是同一条路。
    pub fn push_audio(&self, pcm48: &[i16]) {
        self.injected_audio.push(pcm48);
    }

    /// 换录音 / 播放设备。`None` 是跟系统默认。
    ///
    /// **立刻生效**，不必重连——重建在音频线程上做，因为 `cpal::Stream`
    /// 是 `!Send`。设备没变时是空操作：重建会让声音断一下。
    pub fn set_audio_devices(&self, input: Option<String>, output: Option<String>) {
        let _ = self.commands.send(Command::Devices { input, output });
    }

    /// 麦克风 / 喇叭总音量。`1.0` 是原声。
    pub fn set_master_volume(&self, mic: f32, speaker: f32) {
        let _ = self.commands.send(Command::Master { mic, speaker });
    }

    /// 订阅事件流。
    ///
    /// **第一个订阅者看得见 `connect` 之后发生的一切，之后的订阅者从订阅那一刻看起。**
    ///
    /// 后台任务在 `connect` 返回之前就起了，而第一条链路已经拿到 READY：它一打开
    /// 声卡（不开声卡的通播机器人是立刻）就发 `Limits` 和 `Online`，而上层要等
    /// `connect` 返回才订阅得上。broadcast 在没有接收端时直接丢，所以抢输的那一次，
    /// 这条会话在界面上既没有"已连接"，也没有发射上限，超额提示从此不响——
    /// 而且没有任何报错。所以接收端和通道一起建、留到第一次调用。
    ///
    /// 没人订阅也不要紧：缓冲有 256 条的上界，落后的接收端只会丢旧的，从不挡住发送。
    pub fn events(&self) -> tokio::sync::broadcast::Receiver<Event> {
        let first = match self.first_events.lock() {
            Ok(mut slot) => slot.take(),
            Err(p) => p.into_inner().take(),
        };
        first.unwrap_or_else(|| self.events_tx.subscribe())
    }

    /// 关闭。
    pub fn request_shutdown(&self) {
        let _ = self.commands.send(Command::Shutdown);
    }

    /// 关闭。
    pub async fn shutdown(self) {
        self.request_shutdown();
    }
}

/// 把 `host:port` 解析成一个地址。
async fn resolve(server: &str) -> Result<std::net::SocketAddr, Error> {
    tokio::net::lookup_host(server)
        .await
        .map_err(|_| Error::BadAddress(server.to_string()))?
        .next()
        .ok_or_else(|| Error::BadAddress(server.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pushed_audio_does_not_accumulate_in_the_control_command_channel() {
        let (client, _events, mut commands) = VoiceClient::wired();
        for _ in 0..128 {
            client.push_audio(&[1; 960]);
        }
        client.set_transmitting(true);
        client.set_subscription(Sub {
            tx: vec![118_000],
            ..Default::default()
        });

        assert!(
            matches!(commands.try_recv(), Ok(Command::Transmit(true))),
            "audio must stay out of the unbounded control channel"
        );
        assert!(matches!(
            commands.try_recv(),
            Ok(Command::Declare(sub)) if sub.tx == [118_000]
        ));
        assert!(commands.try_recv().is_err(), "only controls were queued");
        assert_eq!(client.injected_audio.take(), vec![1; 3840]);
    }

    #[test]
    fn injected_audio_keeps_the_newest_samples_in_order() {
        let (client, _events, _commands) = VoiceClient::wired();
        client.push_audio(&vec![1; 3840]);
        client.push_audio(&vec![2; 1920]);

        let audio = client.injected_audio.take();
        assert_eq!(audio.len(), 3840);
        assert_eq!(&audio[..1920], &[1; 1920]);
        assert_eq!(&audio[1920..], &[2; 1920]);
        assert!(client.injected_audio.take().is_empty());
    }

    #[test]
    fn oversized_injected_audio_keeps_only_its_tail() {
        let (client, _events, _commands) = VoiceClient::wired();
        let audio: Vec<i16> = (0..4000).collect();
        client.push_audio(&audio);

        let queued = client.injected_audio.take();
        assert_eq!(queued.len(), 3840);
        assert_eq!(queued.first(), Some(&160));
        assert_eq!(queued.last(), Some(&3999));
    }

    /// 编译期契约：公开 API 里不得出现 join/leave/channel。
    ///
    /// 这不是命名洁癖。旧实现里一整类"UI 是绿的但人还在 root 频道"的 bug，
    /// 根源就是把"我在哪个频道"当成可以记住的事实。声明式 API 里
    /// 没有"记住"这个动作，所以那类 bug 没有藏身之处。
    ///
    /// **修订件 L4：扫全部模块文件，不是只扫 `client.rs`。** 只扫一个文件的话，
    /// 在 `session.rs` 里加一个 `pub fn join_frequency` 它发现不了——那是
    /// **看起来在防守**。这里更进一步：在测试运行时遍历 `src/` 下的每一个 `.rs`，
    /// 所以新加一个模块而忘了把它加进名单这件事根本不会发生。
    ///
    /// 真做对得看**导出符号**而不是源码文本，但那要 proc-macro 或
    /// `cargo public-api`；遍历源码是成本合适的近似。
    #[test]
    fn the_public_api_has_no_imperative_channel_verbs() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let files = rust_sources(&root);
        assert!(
            files.len() >= 8,
            "expected the whole module tree, found {files:?}"
        );

        let mut scanned = 0usize;
        for path in &files {
            let source = std::fs::read_to_string(path).expect("read source");
            for line in source.lines() {
                let t = line.trim();
                if !t.starts_with("pub fn") && !t.starts_with("pub async fn") {
                    continue;
                }
                scanned += 1;
                for banned in ["join", "leave", "channel_id", "current_channel"] {
                    assert!(
                        !t.to_lowercase().contains(banned),
                        "public API must not expose {banned:?} — see the module docs for why: \
                         {} in {}",
                        t,
                        path.display()
                    );
                }
            }
        }
        assert!(
            scanned > 20,
            "only {scanned} public fns scanned; the walk is probably broken"
        );
    }

    fn rust_sources(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let entries = std::fs::read_dir(dir).expect("read_dir");
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(rust_sources(&p));
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
        out.sort();
        out
    }

    #[test]
    fn events_can_be_subscribed_to_before_anything_happens() {
        let (tx, _) = tokio::sync::broadcast::channel::<Event>(16);
        let mut rx = tx.subscribe();
        tx.send(Event::State(crate::conn::LinkState::Online))
            .expect("send");
        assert!(matches!(
            rx.try_recv(),
            Ok(Event::State(crate::conn::LinkState::Online))
        ));
    }

    /// **第一个订阅者看得见 `connect` 之后发生的一切。**
    ///
    /// 后台任务在 `connect` 返回之前就起了，而第一条链路已经握过手：`Limits` 和
    /// `Online` 可以在任何人订阅之前就发出去。broadcast 在没有接收端时直接丢，
    /// 于是上层订阅得晚一拍，这一整条会话就既没有"已连接"也没有发射上限。
    #[test]
    fn the_first_subscriber_sees_what_was_sent_before_it_subscribed() {
        let (client, events_tx, _cmds) = VoiceClient::wired();
        let limits = Limits {
            max_tx: 8,
            max_rx: 32,
        };
        events_tx
            .send(Event::Limits(limits))
            .expect("the receiver made with the channel is still alive");
        events_tx
            .send(Event::State(crate::conn::LinkState::Online))
            .expect("send");

        let mut first = client.events();
        assert_eq!(first.try_recv().ok(), Some(Event::Limits(limits)));
        assert_eq!(
            first.try_recv().ok(),
            Some(Event::State(crate::conn::LinkState::Online))
        );

        // 之后的订阅者照旧从订阅那一刻看起。
        let mut later = client.events();
        assert!(later.try_recv().is_err(), "a later subscriber starts fresh");
        events_tx
            .send(Event::State(crate::conn::LinkState::Offline))
            .expect("send");
        assert_eq!(
            later.try_recv().ok(),
            Some(Event::State(crate::conn::LinkState::Offline))
        );
    }

    #[test]
    fn rx_end_carries_enough_to_log_one_line_per_transmission() {
        // 日志约定：每次收到的通话一行，不是两行。开始是 DEBUG，
        // 结束那一行要自带时长和帧数。
        let e = Event::RxEnd {
            freq_khz: 121_800,
            speaker: 7,
            frames: 127,
            secs: 2.5,
        };
        match e {
            Event::RxEnd { frames, secs, .. } => {
                assert_eq!(frames, 127);
                assert!((secs - 2.5).abs() < f32::EPSILON);
            }
            other => panic!("{other:?}"),
        }
    }

    // ——— 修订件要求存在的事件变体 ———

    /// H3：BYE / 关闭原因不能死在一行日志里。服务端这几个串是**专门为客户端
    /// 造的**，送进 `tracing::warn!` 就完事等于协议里唯一一处专门为客户端设计的
    /// 东西客户端用不上，用户只看到一个没有理由的 Offline。
    #[test]
    fn a_refusal_reaches_the_upper_layer_with_its_reason() {
        let e = Event::Refused {
            reason: crate::conn::RefusedReason::TokenExpired,
        };
        match e {
            Event::Refused { reason } => assert!(reason.is_recoverable()),
            other => panic!("{other:?}"),
        }
    }

    /// C1：交叉耦合被拒必须能显示出来。一个设好了耦合却不生效、又不知道为什么的
    /// 管制员，正是整个重写要逃离的那类故障。
    #[test]
    fn a_rejected_cross_couple_pair_has_an_event_of_its_own() {
        let e = Event::XcDenied {
            a_khz: 121_800,
            b_khz: 124_550,
            reason: "tx_not_granted".into(),
        };
        assert!(matches!(e, Event::XcDenied { a_khz: 121_800, .. }));
    }

    /// H2：RX 和 TX 的拒绝是两个不同的事件，因为它们由**差集**分别算出，
    /// 而不是从 `rejected` 里推断方向。
    #[test]
    fn rx_and_tx_denials_are_separate_events() {
        let a = Event::RxDenied { freq_khz: 118_000 };
        let b = Event::TxDenied {
            freq_khz: 118_000,
            reason: String::new(),
        };
        assert_ne!(a, b);
    }

    /// M11：`Health` 留着而且要真发。掉线必须自己解释——RTT、收发帧数，
    /// 正是区分"上行真的扛不住"和"抖了一下"的东西，而那两者的处置完全不同。
    #[test]
    fn health_carries_what_a_drop_needs_to_explain_itself() {
        let e = Event::Health {
            rtt_ms: 42,
            sent: 1000,
            received: 995,
            lost: 5,
            unparsable: 0,
            playback: crate::audio::PlaybackStats::default(),
        };
        match e {
            Event::Health {
                rtt_ms,
                sent,
                received,
                lost,
                ..
            } => {
                assert_eq!(rtt_ms, 42);
                assert_eq!(sent - received, lost);
            }
            other => panic!("{other:?}"),
        }
    }

    // ——— M6：connect 要真的等握手 ———

    /// 一个解析不出来的地址必须变成 `connect` 的 `Err`，而不是几秒之后一个
    /// 没有理由的 `Offline`。这也是 `Error::BadAddress` 的构造点——原计划里
    /// 它没有任何地方会构造，因为 `connect` 在 `tokio::spawn` 之后立刻 `Ok`。
    #[tokio::test]
    async fn connecting_to_an_unresolvable_address_fails_loudly() {
        let cfg = Config {
            server: "no-such-host.invalid:64738".into(),
            server_name: "no-such-host.invalid".into(),
            token: "t".into(),
            client_id: "test/0".into(),
            follow: String::new(),
            station: String::new(),
            input_device: None,
            output_device: None,
            audio_devices: false,
            extra_roots: Vec::new(),
        };
        let err = VoiceClient::connect(cfg)
            .await
            .expect_err("must not pretend to succeed");
        assert!(matches!(err, Error::BadAddress(_)), "got {err:?}");
    }

    /// N4 的入口一侧：观察员呼号不合规则时，`connect` 在**发出任何东西之前**
    /// 就失败，而不是拿一条 `refused` 去问用户。
    #[tokio::test]
    async fn an_invalid_follow_callsign_fails_before_any_network_traffic() {
        let cfg = Config {
            server: "127.0.0.1:1".into(),
            server_name: "localhost".into(),
            token: "t".into(),
            client_id: "test/0".into(),
            follow: "bad callsign".into(),
            station: String::new(),
            input_device: None,
            output_device: None,
            audio_devices: false,
            extra_roots: Vec::new(),
        };
        let err = VoiceClient::connect(cfg)
            .await
            .expect_err("must reject the callsign");
        assert!(
            matches!(err, Error::Conn(crate::conn::Error::BadCallsign(_))),
            "got {err:?}"
        );
    }

    /// `station` 走的是同一道闸。服务端对不合规则的席位标记只回一条 `refused`，
    /// 而通播机队是无人值守的——它会照着那条"被拒绝"一直重连下去。
    #[tokio::test]
    async fn an_invalid_station_fails_before_any_network_traffic() {
        let cfg = Config {
            server: "127.0.0.1:1".into(),
            server_name: "localhost".into(),
            token: "t".into(),
            client_id: "test/0".into(),
            follow: String::new(),
            station: "not a callsign".into(),
            input_device: None,
            output_device: None,
            audio_devices: false,
            extra_roots: Vec::new(),
        };
        let err = VoiceClient::connect(cfg)
            .await
            .expect_err("must reject the station");
        assert!(
            matches!(err, Error::Conn(crate::conn::Error::BadCallsign(_))),
            "got {err:?}"
        );
    }
}
