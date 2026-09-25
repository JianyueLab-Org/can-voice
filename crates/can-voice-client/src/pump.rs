//! 后台任务：把控制面、订阅状态机、数据面、音频与事件流接起来。
//!
//! # 掉线之后做什么，不是由重连策略一个人说了算
//!
//! [`crate::conn::ReconnectPolicy`] 只数连续失败，而**被顶号之后的重连是成功的**，
//! 一成功计数器就清零。所以每一次掉线都要先经 [`crate::conn::classify`] 读 QUIC 的
//! 应用层关闭码：码 2（顶号）和码 3（协议违规）是终态，重连只会把同一件事
//! 无限重演，而且每一轮都"成功"。
//!
//! # 这条循环就是音频时钟
//!
//! [`TICK`] 是 20 毫秒，也就是一个 Opus 帧。每一拍做四件事：把混音器的一帧送去
//! 播放、把采集到的音频喂进编码、该发就发、以及（每隔几秒）发一次 PING。
//! **混音器每一拍都出恰好一帧**，不管有没有人在说话——声卡那头每 20 毫秒都要一帧。

use crate::audio::{AudioIo, PlaybackStats};
use crate::client::{Command, Config, Event, InjectedAudio};
use crate::conn::{self, Disposition, Link, LinkState, ReconnectPolicy};
use crate::rx::mixer::{RxEvent, RxMixer};
use crate::session::{Limits, SubscriptionState};
use crate::tx::TxPipeline;
use can_voice_proto::control::{self, Message};
use can_voice_proto::wire::Header;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// 主循环的节拍，等于一个 Opus 帧。
const TICK: Duration = Duration::from_millis(20);

/// PING 的间隔。
const PING_EVERY: Duration = Duration::from_secs(5);

/// 一条连接上的收发记账。
///
/// 抽出来是为了让"哪个数字进哪个字段"这件事可测——它在 `select!` 里是不可测的，
/// 而它恰恰错过一次：`lost` 填的是 `unparsable`。
#[derive(Debug, Default, Clone, Copy)]
struct Counters {
    /// 交给 QUIC 的数据报数。
    sent: u64,
    /// 收下并且包头解开了的数据报数。
    received: u64,
    /// 包头解不开的数据报数。**这不是丢包**，是协议漂移。
    unparsable: u64,
}

impl Counters {
    /// 组装一条 `Health`。`lost_packets` 是 QUIC 自己的丢包计数，
    /// `playback` 是播放环那一侧的对账——**两者都要有**：链路全绿而播放环
    /// 跑干的时候，只看链路这几个数会得出"一切正常"。
    fn health(&self, rtt_ms: u32, lost_packets: u64, playback: PlaybackStats) -> Event {
        Event::Health {
            rtt_ms,
            sent: self.sent,
            received: self.received,
            lost: lost_packets,
            unparsable: self.unparsable,
            playback,
        }
    }
}

/// 这一条连接是怎么结束的。
enum Outcome {
    /// 上层要求关闭。
    Shutdown,
    /// 掉线，附带该怎么办。
    Dropped(Disposition),
}

#[derive(Default)]
struct SessionControls {
    ptt: bool,
    mic_gain: f32,
    speaker_gain: f32,
    gains: BTreeMap<u32, f32>,
}

impl SessionControls {
    fn new() -> Self {
        Self {
            mic_gain: 1.0,
            speaker_gain: 1.0,
            ..Self::default()
        }
    }

    fn apply(&mut self, command: &Command) {
        match command {
            Command::Transmit(on) => self.ptt = *on,
            Command::Volume { freq_khz, gain } => {
                self.gains.insert(*freq_khz, *gain);
            }
            Command::Master { mic, speaker } => {
                self.mic_gain = *mic;
                self.speaker_gain = *speaker;
            }
            _ => {}
        }
    }

    fn restore_gains(&self, mixer: &mut RxMixer) {
        for (&freq_khz, &gain) in &self.gains {
            mixer.set_gain(freq_khz, gain);
        }
    }
}

fn drain_queued(
    cmds: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
    subs: &mut SubscriptionState,
    controls: &mut SessionControls,
    audio: Option<&AudioIo>,
) -> bool {
    while let Ok(command) = cmds.try_recv() {
        match command {
            Command::Declare(sub) => subs.declare(sub),
            Command::Devices { input, output } => {
                if let Some(io) = audio {
                    io.set_devices(input.as_deref(), output.as_deref());
                }
            }
            Command::Shutdown => return false,
            other => controls.apply(&other),
        }
    }
    !cmds.is_closed()
}

pub(crate) async fn run(
    cfg: Config,
    first: Link,
    events: tokio::sync::broadcast::Sender<Event>,
    audio_frames: tokio::sync::broadcast::Sender<Vec<i16>>,
    mut cmds: tokio::sync::mpsc::UnboundedReceiver<Command>,
    injected_audio: std::sync::Arc<InjectedAudio>,
) {
    let mut policy = ReconnectPolicy::new();
    // 第一次拨号已经由 `VoiceClient::connect` 做掉了，而且它真的拿到了 READY。
    policy.may_attempt();
    policy.on_session_established();

    // 声卡打不开**不该让会话起不来**：听不见总比连不上好，而服务端 ATIS 机器人
    // 根本没有麦克风——它的音频是 `push_audio` 注进来的。
    let audio = if cfg.audio_devices {
        match AudioIo::start(cfg.input_device.as_deref(), cfg.output_device.as_deref()) {
            Ok(io) => Some(io),
            Err(e) => {
                tracing::error!(error = %e, "could not open the audio devices; running deaf and mute");
                let _ = events.send(Event::Notice {
                    kind: "audio_unavailable".into(),
                    freq_khz: 0,
                    reason: e.to_string(),
                });
                None
            }
        }
    } else {
        None
    };

    let mut subs = SubscriptionState::new();
    let mut controls = SessionControls::new();
    let mut state = LinkState::Connecting;
    let mut link = Some(first);

    loop {
        let l = match link.take() {
            Some(l) => l,
            None => {
                if !policy.may_attempt() {
                    emit_state(&events, &mut state, policy.state());
                    tracing::warn!("giving up on the voice link");
                    return;
                }
                emit_state(&events, &mut state, policy.state());
                match dial(&cfg).await {
                    Ok(l) => {
                        // **只有真的收到 READY 才重置计数。** `conn::connect` 会等到
                        // READY，所以走到这里是安全的。
                        policy.on_session_established();
                        l
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "voice connect failed");
                        if let Some(reason) = dial_failure(&e) {
                            let _ = events.send(Event::Refused { reason });
                            emit_state(&events, &mut state, LinkState::Offline);
                            return;
                        }
                        continue;
                    }
                }
            }
        };

        if !drain_queued(&mut cmds, &mut subs, &mut controls, audio.as_ref()) {
            l.conn
                .close(conn::CLOSE_NORMAL.try_into().unwrap_or_default(), b"bye");
            emit_state(&events, &mut state, LinkState::Offline);
            return;
        };

        let limits = Limits {
            max_tx: l.max_tx,
            max_rx: l.max_rx,
        };
        subs.on_connected(limits);
        // 限额也要交上去，**在 Online 之前**：界面一看到"已连接"就该知道这一次的
        // 上限，不然重放的台面超额时，先冒出来的会是"发射被拒"。
        let _ = events.send(Event::Limits(limits));
        emit_state(&events, &mut state, LinkState::Online);

        let outcome = pump(
            l,
            &mut subs,
            &events,
            &audio_frames,
            &mut cmds,
            audio.as_ref(),
            &mut controls,
            &injected_audio,
        )
        .await;
        subs.on_disconnected();

        match outcome {
            Outcome::Shutdown => {
                emit_state(&events, &mut state, LinkState::Offline);
                return;
            }
            Outcome::Dropped(Disposition::Reconnect) => {
                emit_state(&events, &mut state, LinkState::Reconnecting);
            }
            Outcome::Dropped(Disposition::Evicted) => {
                // **终态。** 自动重连会顶掉刚刚顶掉自己的那条会话，对方再重连再
                // 顶回来，两个客户端无限互顶——而每一轮都"成功"，所以有界重连的
                // 计数器一次都不会累加。
                policy.on_evicted();
                tracing::warn!("this account signed in elsewhere; not reconnecting");
                emit_state(&events, &mut state, LinkState::Evicted);
                return;
            }
            Outcome::Dropped(Disposition::ProtocolViolation(v)) => {
                // 终态，理由同上：重连会立刻把同一个 bug 再演一遍。
                tracing::error!(violation = ?v, "the server says this client broke the protocol");
                emit_state(&events, &mut state, LinkState::Offline);
                return;
            }
            Outcome::Dropped(Disposition::Refused(reason)) => {
                let _ = events.send(Event::Refused {
                    reason: reason.clone(),
                });
                tracing::warn!(?reason, "handshake refused");
                // `token_expired` 是唯一可恢复的一条，但**换票不是这个库能做的事**
                // ——它拿不到新 token。所以进 Offline，由上层换一张再 connect 一次。
                emit_state(&events, &mut state, LinkState::Offline);
                return;
            }
        }
    }
}

fn dial_failure(error: &conn::Error) -> Option<conn::RefusedReason> {
    match error {
        conn::Error::Refused(reason) => Some(reason.clone()),
        _ => None,
    }
}

/// 只在状态真的变了的时候发事件。
///
/// 每轮都发一遍会把事件流灌满，而上层没法从中分辨"状态变了"和"心跳到了"。
fn emit_state(
    events: &tokio::sync::broadcast::Sender<Event>,
    current: &mut LinkState,
    next: LinkState,
) {
    if *current != next {
        *current = next;
        let _ = events.send(Event::State(next));
    }
}

async fn dial(cfg: &Config) -> Result<Link, conn::Error> {
    let addr = tokio::net::lookup_host(&cfg.server)
        .await
        .ok()
        .and_then(|mut a| a.next())
        .ok_or_else(|| conn::Error::Io(std::io::Error::other("address could not be resolved")))?;
    conn::connect(
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
    .await
}

enum PumpInput {
    Command(Option<Command>),
    Control(Option<Result<Message, conn::Error>>),
    Datagram(Result<bytes::Bytes, quinn::ConnectionError>),
    Tick,
}

async fn next_pump_input(
    cmds: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
    control: &mut tokio::sync::mpsc::Receiver<Result<Message, conn::Error>>,
    datagram: impl std::future::Future<Output = Result<bytes::Bytes, quinn::ConnectionError>>,
    ticker: &mut tokio::time::Interval,
) -> PumpInput {
    tokio::select! {
        cmd = cmds.recv() => PumpInput::Command(cmd),
        msg = control.recv() => PumpInput::Control(msg),
        dg = datagram => PumpInput::Datagram(dg),
        _ = ticker.tick() => PumpInput::Tick,
    }
}

/// 一条连接活着期间的主循环。
async fn pump(
    link: Link,
    subs: &mut SubscriptionState,
    events: &tokio::sync::broadcast::Sender<Event>,
    audio_frames: &tokio::sync::broadcast::Sender<Vec<i16>>,
    cmds: &mut tokio::sync::mpsc::UnboundedReceiver<Command>,
    audio: Option<&AudioIo>,
    controls: &mut SessionControls,
    injected_audio: &InjectedAudio,
) -> Outcome {
    let Link {
        conn: quic,
        mut control_send,
        control_recv,
        ..
    } = link;
    // **控制流的读取活在它自己的 task 里**，这里只 `recv()`。直接在 `select!` 里
    // 调用一个"读长度前缀再读包体"的 future 不是取消安全的：别的分支赢了的时候
    // 它会在两次读之间被丢掉，已经消费掉的字节回不来，控制流从此错位。
    let mut control = conn::spawn_control_reader(control_recv);

    let mut mixer = RxMixer::new();
    controls.restore_gains(&mut mixer);
    let mut tx = match TxPipeline::new() {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::error!(error = %e, "could not create the opus encoder; receive only");
            None
        }
    };
    if let Some(tx) = tx.as_mut() {
        tx.push(&injected_audio.take());
    }

    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let epoch = Instant::now();
    let subscription_epoch = subs.epoch();
    let mut last_ping = Instant::now();
    let mut rtt_ms = 0u32;
    let mut counters = Counters::default();
    // 音频一开始是好的：建不起来的话 `AudioIo::start` 已经报过了。
    let mut audio_ok = true;

    loop {
        // 有待发的声明就先推出去。`SubscriptionState` 保证这是幂等的全量声明，
        // 而且一连串 UI 操作只会剩下最后那一条。
        if let Some(sub) = subs.take_pending() {
            if let Err(e) = conn::write_msg(&mut control_send, &Message::Sub(sub)).await {
                tracing::warn!(error = %e, "could not push the subscription");
                return Outcome::Dropped(drop_reason(&quic));
            }
        }

        match next_pump_input(cmds, &mut control, quic.read_datagram(), &mut ticker).await {
            PumpInput::Command(cmd) => match cmd {
                Some(Command::Declare(sub)) => subs.declare(sub),
                Some(cmd @ Command::Transmit(_)) => controls.apply(&cmd),
                Some(Command::Volume { freq_khz, gain }) => {
                    mixer.set_gain(freq_khz, gain);
                    controls.apply(&Command::Volume { freq_khz, gain });
                }
                // 换设备**立刻生效**，不必等到下一次连接：重建在音频线程上做，
                // 因为 `cpal::Stream` 是 `!Send`。
                Some(Command::Devices { input, output }) => {
                    if let Some(io) = audio {
                        io.set_devices(input.as_deref(), output.as_deref());
                    }
                }
                Some(cmd @ Command::Master { .. }) => controls.apply(&cmd),
                Some(Command::Shutdown) | None => {
                    quic.close(conn::CLOSE_NORMAL.try_into().unwrap_or_default(), b"bye");
                    return Outcome::Shutdown;
                }
            },
            PumpInput::Control(msg) => match msg {
                Some(Ok(Message::SubAck(ack))) => on_ack(subs, events, subscription_epoch, ack),
                Some(Ok(Message::Pong(p))) => {
                    let now = epoch.elapsed().as_millis() as i64;
                    rtt_ms = now.saturating_sub(p.t).clamp(0, u32::MAX as i64) as u32;
                }
                Some(Ok(Message::Notice(n))) => {
                    on_notice(subs, events, &mut controls.ptt, n);
                }
                Some(Ok(Message::Bye(b))) => {
                    // BYE **会丢**——真正丢不掉的是关闭码与原因串，它们和关闭
                    // 原子地一起送达。所以这里只记日志，处置交给 `drop_reason`。
                    tracing::warn!(reason = %b.reason, "server closed the session");
                    return Outcome::Dropped(drop_reason(&quic));
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    tracing::warn!(error = %e, "control stream failed");
                    return Outcome::Dropped(drop_reason(&quic));
                }
                None => {
                    tracing::warn!("control stream ended");
                    return Outcome::Dropped(drop_reason(&quic));
                }
            },
            PumpInput::Datagram(dg) => match dg {
                Ok(bytes) => match Header::parse(&bytes) {
                    Ok((h, opus)) => {
                        counters.received += 1;
                        if let Some(e) = mixer.feed(&h, opus) {
                            send_rx_event(events, e);
                        }
                    }
                    Err(e) => {
                        counters.unparsable += 1;
                        tracing::debug!(error = %e, "dropping an unparsable datagram");
                    }
                },
                Err(e) => {
                    tracing::warn!(error = %e, "datagram stream ended");
                    return Outcome::Dropped(conn::classify(&e));
                }
            },
            PumpInput::Tick => {
                // **声卡掉了要说一句。** 只在日志里 warn 一行的后果是
                // "能连上、状态绿、说话没人听见"，而拔一次耳机就是这样。
                // 恢复也要说，那一条会把前一条撤掉（见 Snapshot::apply）。
                if let Some(io) = audio {
                    let ok = io.running();
                    if ok != audio_ok {
                        audio_ok = ok;
                        let (kind, reason) = if ok {
                            ("audio_restored", "the audio devices are open again")
                        } else {
                            (
                                "audio_unavailable",
                                "the audio devices went away; reopening",
                            )
                        };
                        let _ = events.send(Event::Notice {
                            kind: kind.into(),
                            freq_khz: 0,
                            reason: reason.into(),
                        });
                    }
                }

                // 接收：混音器每一拍都出恰好一帧，直接送去播放。
                let (mut pcm, rx_events) = mixer.tick();
                let _ = audio_frames.send(pcm.clone());
                if let Some(io) = audio {
                    scale_pcm(&mut pcm, controls.speaker_gain);
                    io.play(&pcm);
                }
                for e in rx_events {
                    send_rx_event(events, e);
                }

                // 发送：先把采集到的喂进去，再看这一拍有没有一帧要发。
                if let Some(t) = tx.as_mut() {
                    t.push(&injected_audio.take());
                    if let Some(io) = audio {
                        let mut captured = io.take_capture();
                        if !captured.is_empty() {
                            scale_pcm(&mut captured, controls.mic_gain);
                            t.push(&captured);
                        }
                    }
                    if let Some(frame) = t.tick(controls.ptt) {
                        // **每一个 TX 频率各发一份，每份带同一个 `seq`。** 服务端
                        // 检查不了这件事：只发一份的客户端在另一个频率上完全静默，
                        // 而两端日志都正常。
                        for freq in subs.acknowledged().tx.clone() {
                            let dg = bytes::Bytes::from(frame.datagram(freq));
                            match quic.send_datagram(dg) {
                                Ok(()) => counters.sent += 1,
                                Err(e) => {
                                    // 发不出去只丢这一帧：数据报本来就是不可靠的，
                                    // 为一帧音频断开整条连接是过度反应。
                                    tracing::debug!(error = %e, freq, "datagram not sent");
                                }
                            }
                        }
                    }
                }

                if last_ping.elapsed() >= PING_EVERY {
                    last_ping = Instant::now();
                    let t = epoch.elapsed().as_millis() as i64;
                    if conn::write_msg(&mut control_send, &Message::Ping(control::Ping { t }))
                        .await
                        .is_err()
                    {
                        return Outcome::Dropped(drop_reason(&quic));
                    }
                    // **掉线必须自己解释。** RTT 和收发计数正是区分"上行真的扛不住"
                    // 和"抖了一下"的东西，而那两者的处置完全不同。
                    // 没有声卡时是全零：服务端 ATIS 机器人根本不开设备。
                    let playback = audio.map(AudioIo::playback_stats).unwrap_or_default();
                    let _ = events.send(counters.health(
                        rtt_ms,
                        quic.stats().path.lost_packets,
                        playback,
                    ));
                }
            }
        }
    }
}

fn scale_pcm(samples: &mut [i16], gain: f32) {
    if (gain - 1.0).abs() < f32::EPSILON {
        return;
    }
    for s in samples {
        *s = (*s as f32 * gain)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    }
}

fn send_rx_event(events: &tokio::sync::broadcast::Sender<Event>, e: RxEvent) {
    let _ = events.send(match e {
        RxEvent::Start { freq_khz, speaker } => Event::RxStart { freq_khz, speaker },
        RxEvent::End {
            freq_khz,
            speaker,
            frames,
            secs,
        } => Event::RxEnd {
            freq_khz,
            speaker,
            frames,
            secs,
        },
    });
}

/// 连接已经没了时，从 quinn 那里读出应用层关闭码。
fn drop_reason(quic: &quinn::Connection) -> Disposition {
    match quic.close_reason() {
        Some(e) => conn::classify(&e),
        None => Disposition::Reconnect,
    }
}

fn on_ack(
    subs: &mut SubscriptionState,
    events: &tokio::sync::broadcast::Sender<Event>,
    epoch: u64,
    ack: control::SubAck,
) {
    tracing::debug!(
        rx = ack.rx.len(),
        tx = ack.tx.len(),
        rejected = ack.rejected.len(),
        truncated = ack.rejected_truncated,
        "subscription acknowledged"
    );
    if !subs.on_ack_epoch(epoch, ack.clone()) {
        tracing::warn!("ignoring an unmatched subscription acknowledgement");
        return;
    }
    let _ = events.send(Event::SubscriptionAck {
        rx: ack.rx.clone(),
        tx: ack.tx.clone(),
        xc: subs.effective_xc(),
    });
    // 交叉耦合被拒是**另一张单子**：耦合对不在 rx/tx 里，差集公式管不到它。
    for pair in &ack.rejected_xc {
        let _ = events.send(Event::XcDenied {
            a_khz: pair[0],
            b_khz: pair[1],
            reason: "not granted".into(),
        });
    }

    // **按差集分派，不要把 `rejected` 里的每一项都当成 TxDenied。** 一个频率同时
    // 出现在 `ack.rx` 和 `rejected` 里是正常的（TX 被限额拒了、RX 给了），
    // 照 `rejected` 派会让一个能听的频率显示成失败；而 `rejected` 本身有上界，
    // 截断之后基于它的推断全部失效。
    for f in subs.denied_tx() {
        let _ = events.send(Event::TxDenied {
            freq_khz: f,
            reason: String::new(),
        });
    }
    for f in subs.denied_rx() {
        let _ = events.send(Event::RxDenied { freq_khz: f });
    }
}

fn on_notice(
    subs: &mut SubscriptionState,
    events: &tokio::sync::broadcast::Sender<Event>,
    ptt: &mut bool,
    n: control::Notice,
) {
    use can_voice_proto::control::notice_kind;
    if n.kind == notice_kind::TX_DENIED {
        let _ = events.send(Event::TxDenied {
            freq_khz: n.freq,
            reason: n.reason,
        });
    } else if n.kind == notice_kind::AUTHORITY_LOST {
        *ptt = false;
        subs.revoke_tx();
        subs.request_replay();
        let _ = events.send(Event::SubscriptionAck {
            rx: subs.acknowledged().rx.clone(),
            tx: Vec::new(),
            xc: Vec::new(),
        });
        let _ = events.send(Event::Notice {
            kind: n.kind,
            freq_khz: n.freq,
            reason: n.reason,
        });
    } else if n.kind == notice_kind::AUTHORITY_RESTORED {
        *ptt = false;
        subs.request_replay();
        let _ = events.send(Event::Notice {
            kind: n.kind,
            freq_khz: n.freq,
            reason: n.reason,
        });
    } else if n.kind == notice_kind::TALKER {
        let _ = events.send(Event::Talker {
            session: n.session,
            cid: n.cid,
            freq_khz: n.freq,
        });
    } else {
        let _ = events.send(Event::Notice {
            kind: n.kind,
            freq_khz: n.freq,
            reason: n.reason,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn queued_commands_do_not_starve_control_datagrams_or_audio_ticks() {
        let (send_command, mut commands) = tokio::sync::mpsc::unbounded_channel();
        for _ in 0..128 {
            send_command
                .send(Command::Transmit(true))
                .expect("queue command");
        }
        let (send_control, mut control) = tokio::sync::mpsc::channel(1);
        send_control
            .send(Ok(Message::Pong(control::Pong { t: 0, server_t: 0 })))
            .await
            .expect("queue control message");
        let mut ticker = tokio::time::interval(TICK);
        ticker.tick().await;
        tokio::time::sleep(TICK).await;
        let mut saw_control = false;
        let mut saw_datagram = false;
        let mut saw_tick = false;

        for _ in 0..128 {
            match next_pump_input(
                &mut commands,
                &mut control,
                std::future::ready(Ok(bytes::Bytes::new())),
                &mut ticker,
            )
            .await
            {
                PumpInput::Command(Some(Command::Transmit(true))) => {}
                PumpInput::Control(Some(Ok(Message::Pong(_)))) => saw_control = true,
                PumpInput::Datagram(Ok(_)) => saw_datagram = true,
                PumpInput::Tick => saw_tick = true,
                _ => panic!("unexpected pump input"),
            }
            if saw_control && saw_datagram && saw_tick {
                break;
            }
        }

        assert!(saw_control, "a ready control message must be processed");
        assert!(saw_datagram, "a ready datagram must be processed");
        assert!(saw_tick, "the ready audio tick must be processed");
    }

    /// **包头解不开不是丢包。** 前者是协议漂移（对端版本不对、字段改了名），
    /// 后者是网络。两者填进同一个字段的后果是 `lost` 在正常运行里恒为 0——
    /// 而它的用途正是回答"上行是真的扛不住，还是抖了一下"。
    #[test]
    fn unparsable_datagrams_are_not_counted_as_packet_loss() {
        let c = Counters {
            sent: 1000,
            received: 995,
            unparsable: 7,
        };

        match c.health(42, 3, PlaybackStats::default()) {
            Event::Health {
                lost, unparsable, ..
            } => {
                assert_eq!(lost, 3, "lost 该是 QUIC 的丢包计数，不是 unparsable");
                assert_eq!(unparsable, 7, "包头解不开的要自己有一个字段");
            }
            other => panic!("expected Health, got {other:?}"),
        }
    }

    #[test]
    fn authority_loss_releases_local_ptt_state() {
        let (events, _rx) = tokio::sync::broadcast::channel(8);
        let mut subs = SubscriptionState::new();
        let mut ptt = true;
        on_notice(
            &mut subs,
            &events,
            &mut ptt,
            can_voice_proto::control::Notice {
                kind: can_voice_proto::control::notice_kind::AUTHORITY_LOST.into(),
                freq: 118_000,
                reason: "seat changed".into(),
                session: 0,
                cid: String::new(),
            },
        );
        assert!(!ptt, "authority loss must force a local PTT release");
    }

    #[test]
    fn expired_handshake_refusal_reaches_the_supervisor() {
        assert_eq!(
            dial_failure(&conn::Error::Refused(conn::RefusedReason::TokenExpired)),
            Some(conn::RefusedReason::TokenExpired)
        );
    }

    #[test]
    fn queued_release_and_volume_changes_survive_reconnect() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut subs = SubscriptionState::new();
        let mut controls = SessionControls::new();
        controls.apply(&Command::Transmit(true));
        controls.apply(&Command::Volume {
            freq_khz: 118_000,
            gain: 0.5,
        });
        controls.apply(&Command::Master {
            mic: 0.8,
            speaker: 0.7,
        });
        sender
            .send(Command::Transmit(false))
            .expect("queue release");
        sender
            .send(Command::Volume {
                freq_khz: 118_000,
                gain: 0.0,
            })
            .expect("queue mute");
        assert!(drain_queued(&mut receiver, &mut subs, &mut controls, None));
        assert!(!controls.ptt);
        assert_eq!(controls.gains.get(&118_000), Some(&0.0));
        assert_eq!((controls.mic_gain, controls.speaker_gain), (0.8, 0.7));
    }

    #[test]
    fn held_ptt_survives_ordinary_reconnect() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut subs = SubscriptionState::new();
        let mut controls = SessionControls::new();
        controls.apply(&Command::Transmit(true));
        assert!(drain_queued(&mut receiver, &mut subs, &mut controls, None));
        assert!(controls.ptt);
        drop(sender);
    }

    #[test]
    fn authority_restoration_replays_subscription_without_resuming_ptt() {
        let (events, _rx) = tokio::sync::broadcast::channel(8);
        let mut subs = SubscriptionState::new();
        subs.on_connected(Limits {
            max_tx: 2,
            max_rx: 2,
        });
        let wanted = control::Sub {
            rx: vec![118_000],
            tx: vec![118_000],
            ..Default::default()
        };
        subs.declare(wanted.clone());
        let _ = subs.take_pending();
        let mut ptt = true;
        on_notice(
            &mut subs,
            &events,
            &mut ptt,
            control::Notice {
                kind: control::notice_kind::AUTHORITY_LOST.into(),
                freq: 118_000,
                reason: "seat changed".into(),
                session: 0,
                cid: String::new(),
            },
        );
        assert!(!ptt);
        on_notice(
            &mut subs,
            &events,
            &mut ptt,
            control::Notice {
                kind: control::notice_kind::AUTHORITY_RESTORED.into(),
                freq: 0,
                reason: String::new(),
                session: 0,
                cid: String::new(),
            },
        );
        assert!(subs.take_pending().is_none());
        assert!(subs.on_ack_epoch(subs.epoch(), control::SubAck::default()));
        assert_eq!(subs.take_pending(), Some(wanted));
        assert!(!ptt);
    }

    #[test]
    fn restoration_before_delayed_loss_never_keeps_ptt_or_strands_replay() {
        let (events, _rx) = tokio::sync::broadcast::channel(8);
        let mut subs = SubscriptionState::new();
        subs.on_connected(Limits {
            max_tx: 1,
            max_rx: 1,
        });
        let wanted = control::Sub {
            tx: vec![118_000],
            ..Default::default()
        };
        subs.declare(wanted.clone());
        assert_eq!(subs.take_pending(), Some(wanted.clone()));
        assert!(subs.on_ack_epoch(
            subs.epoch(),
            control::SubAck {
                tx: vec![118_000],
                ..Default::default()
            }
        ));

        let mut ptt = true;
        on_notice(
            &mut subs,
            &events,
            &mut ptt,
            control::Notice {
                kind: control::notice_kind::AUTHORITY_RESTORED.into(),
                freq: 0,
                reason: String::new(),
                session: 0,
                cid: String::new(),
            },
        );
        assert!(!ptt, "restoration must be safe even when loss is delayed");
        assert_eq!(subs.take_pending(), Some(wanted.clone()));
        assert!(subs.on_ack_epoch(
            subs.epoch(),
            control::SubAck {
                tx: vec![118_000],
                ..Default::default()
            }
        ));

        ptt = true; // a new explicit press after restoration
        on_notice(
            &mut subs,
            &events,
            &mut ptt,
            control::Notice {
                kind: control::notice_kind::AUTHORITY_LOST.into(),
                freq: 0,
                reason: String::new(),
                session: 0,
                cid: String::new(),
            },
        );
        assert!(!ptt, "a delayed loss still releases a newer press");
        assert_eq!(subs.take_pending(), Some(wanted));
    }
}
