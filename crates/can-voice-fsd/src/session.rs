//! 一条 FSD 连接的骨架：连上 → 登录 → 收发 → 掉线重连。
//!
//! # 为什么是泛型的
//!
//! 通播席位和飞行员登录的包不同、周期发的包不同、关心的回包也不同，但
//! **连接、登录、重连、超时这一套是同一套**。`can-audio` 那边它存在两份
//! （`atis/fsdclient.py` 和 `xpc/fsdpilot.py`），于是那三条踩出来的重连规矩
//! 也要维护两遍。这里只有一份，角色差异由 [`Role`] 提供。
//!
//! # 首次连不上**不重试**
//!
//! 那多半是呼号被占、密码不对或者地址填错——重试三次只会把同一条错误刷三遍，
//! 还可能触发服务端对认证失败的限流。只有"**登录成功过之后**掉的线"才重连：
//! 那种是服务器重启或者网络抖动，重连是对的。
//!
//! 次数用尽后报 [`FsdState::Offline`] 并结束，调用方据此把这个席位整个收掉
//! （语音也一起），而不是留一条谁也说不清状态的连接。
//!
//! **一次成功的重连把计数清零**——计的是"连着失败几次"，不是"这条连接一辈子
//! 断过几次"，否则一个连了八小时、中间抖过两次的席位会在第三次抖动时整个下线。
//!
//! # 错误在一个地方翻译成"重连中"
//!
//! 重连期间的失败**不是终态**。在 [`Session::emit`] 里翻一次比在每个报错点各判
//! 一次可靠：连接那一段有五条报错路径、收发循环还有一条，漏掉任何一条，界面就会
//! 在我们正准备重连的时候把这条连接当成彻底没了。

use crate::packet::{self, CallsignProblem, Incoming};
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, oneshot};

pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(10);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 每次重连之间等一下，别贴着服务器猛敲。
pub const RECONNECT_DELAY: Duration = Duration::from_secs(3);
/// 已经登录过之后掉线，最多再试这么多次。
///
/// **和语音那边是同一条策略**，两条链路的行为要一致，不然"整个下线"就没有
/// 统一的含义。
pub const RECONNECT_LIMIT: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum FsdState {
    Connecting,
    Online,
    Reconnecting,
    Error,
    /// 重连次数用尽，这条链路整个下线。**终态。**
    Offline,
    /// 有人按了停止。**终态。**
    Stopped,
}

/// 为什么。**是枚举不是字符串**：同一条原因在中英两种界面下要说两种话，
/// 而这一层不该知道现在是哪一种。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum Reason {
    Callsign(CallsignProblem),
    Connecting {
        rating: u32,
    },
    ConnectFailed(String),
    Rejected {
        code: String,
        message: String,
    },
    LoginTimeout,
    /// 对端在登录过程中关了连接。
    Closed,
    /// 登录之后连接断了。
    Dropped,
    SendFailed(String),
    /// 频率认不出来。发一个错的频率出去比不发更糟。
    BadFrequency(String),
    Online,
    Retrying {
        attempt: u32,
        limit: u32,
    },
    GaveUp {
        limit: u32,
    },
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FsdEvent {
    pub state: FsdState,
    pub reason: Reason,
}

/// 一条连接里**随角色而异**的那部分。
pub trait Role: Send + 'static {
    /// 角色自己的命令。
    type Command: Send + 'static;

    fn callsign(&self) -> &str;
    /// 呼号合不合规矩。飞行员和通播的规则不同。
    fn validated_callsign(&self) -> Result<String, CallsignProblem>;
    fn adopt_callsign(&mut self, callsign: String);
    fn rating(&self) -> u32;

    /// `$ID` / 登录 / `$CQ…CAPS`，按顺序发。
    ///
    /// **登录没有专门的成功包**，所以要主动问一次能力换个明确回应。
    fn handshake_packets(&self) -> Vec<String>;
    fn logoff_packet(&self) -> String;

    /// 周期性要发的包。登录成功之后先发一次，之后每 [`Role::tick_interval`] 再发。
    ///
    /// `Err` 表示这一轮发不出去且没法继续（比如频率认不出来）。
    fn tick(&mut self) -> Result<Vec<String>, Reason>;
    /// 下一次 tick 的间隔。**每轮重新问**——飞行员端停在地面时会自己降频。
    fn tick_interval(&self) -> Duration;

    /// 角色关心的回包。返回要回的包。
    ///
    /// 共有的几种（`$ER` / METAR / CAPS / ping / 心跳）由 [`Session`] 处理，
    /// 不会走到这里。
    fn on_packet(&mut self, incoming: &Incoming, raw: &str) -> Vec<String>;

    /// 吃一条命令，返回要**立刻**发的包。没连上时照样吃，但包会被丢掉——
    /// 界面照样在改东西，而重连之后要用的是最新那一份，不是掉线那一刻的。
    fn on_command(&mut self, command: Self::Command) -> Vec<String>;
}

enum Envelope<C> {
    Stop,
    RequestMetar {
        icao: String,
        reply: oneshot::Sender<Option<String>>,
    },
    Role(C),
}

/// 对外的把手。克隆随便，命令走一条队列。
pub struct SessionHandle<C> {
    cmd: mpsc::UnboundedSender<Envelope<C>>,
    events: broadcast::Sender<FsdEvent>,
}

impl<C> Clone for SessionHandle<C> {
    fn clone(&self) -> Self {
        Self {
            cmd: self.cmd.clone(),
            events: self.events.clone(),
        }
    }
}

impl<C> std::fmt::Debug for SessionHandle<C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionHandle")
    }
}

impl<C> SessionHandle<C> {
    pub fn events(&self) -> broadcast::Receiver<FsdEvent> {
        self.events.subscribe()
    }

    pub fn send(&self, command: C) {
        let _ = self.cmd.send(Envelope::Role(command));
    }

    pub fn stop(&self) {
        let _ = self.cmd.send(Envelope::Stop);
    }

    /// 向服务端要一份 METAR。拿不到返回 `None`。
    pub async fn request_metar(&self, icao: &str, timeout: Duration) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        self.cmd
            .send(Envelope::RequestMetar {
                icao: icao.trim().to_uppercase(),
                reply: tx,
            })
            .ok()?;
        tokio::time::timeout(timeout, rx).await.ok()?.ok()?
    }
}

/// 起一条连接。返回把手；后台任务自己跑到终态为止。
pub fn spawn<R: Role>(
    host: String,
    port: u16,
    role: R,
    reconnect_limit: u32,
) -> SessionHandle<R::Command> {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, _) = broadcast::channel(64);
    let handle = SessionHandle {
        cmd: cmd_tx,
        events: event_tx.clone(),
    };
    tokio::spawn(
        Session {
            host,
            port,
            role,
            reconnect_limit,
            cmd: cmd_rx,
            events: event_tx,
            retryable: false,
            stopping: false,
            metar_waiters: HashMap::new(),
        }
        .run(),
    );
    handle
}

struct Session<R: Role> {
    host: String,
    port: u16,
    role: R,
    reconnect_limit: u32,
    cmd: mpsc::UnboundedReceiver<Envelope<R::Command>>,
    events: broadcast::Sender<FsdEvent>,
    /// 登录成功过之后，失败就先当"可以重连"。见模块头。
    retryable: bool,
    stopping: bool,
    metar_waiters: HashMap<String, Vec<oneshot::Sender<Option<String>>>>,
}

enum Handled {
    Continue,
    LoggedIn,
    Fatal,
}

impl<R: Role> Session<R> {
    fn emit(&self, state: FsdState, reason: Reason) {
        let state = if self.retryable && matches!(state, FsdState::Error | FsdState::Stopped) {
            FsdState::Reconnecting
        } else {
            state
        };
        tracing::info!(callsign = %self.role.callsign(), ?state, ?reason, "fsd");
        let _ = self.events.send(FsdEvent { state, reason });
    }

    async fn run(mut self) {
        match self.role.validated_callsign() {
            Ok(c) => self.role.adopt_callsign(c),
            Err(problem) => {
                self.emit(FsdState::Error, Reason::Callsign(problem));
                return;
            }
        }

        let mut attempts = 0u32;
        let mut established_once = false;

        loop {
            match self.attach().await {
                Ok(()) => {
                    attempts = 0;
                    established_once = true;
                }
                Err(fatal) => {
                    if fatal {
                        return;
                    }
                }
            }
            self.fail_metar_waiters();

            if !established_once {
                return; // 首次就没连上，原因已经报过了
            }
            if self.stopping {
                self.retryable = false;
                self.emit(FsdState::Stopped, Reason::Stopped);
                return;
            }

            attempts += 1;
            if attempts > self.reconnect_limit {
                self.retryable = false;
                self.emit(
                    FsdState::Offline,
                    Reason::GaveUp {
                        limit: self.reconnect_limit,
                    },
                );
                return;
            }
            self.emit(
                FsdState::Reconnecting,
                Reason::Retrying {
                    attempt: attempts,
                    limit: self.reconnect_limit,
                },
            );
            if self.sleep_or_stop(RECONNECT_DELAY).await {
                self.retryable = false;
                self.emit(FsdState::Stopped, Reason::Stopped);
                return;
            }
        }
    }

    /// `true` 表示期间被要求停止。
    async fn sleep_or_stop(&mut self, how_long: Duration) -> bool {
        let deadline = tokio::time::sleep(how_long);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => return false,
                got = self.cmd.recv() => match got {
                    Some(Envelope::Stop) | None => { self.stopping = true; return true; }
                    Some(other) => self.apply_offline(other),
                },
            }
        }
    }

    fn apply_offline(&mut self, envelope: Envelope<R::Command>) {
        match envelope {
            Envelope::Role(c) => {
                // 包发不出去，但状态要更新——重连之后用的是最新那一份。
                let _ = self.role.on_command(c);
            }
            Envelope::RequestMetar { reply, .. } => {
                let _ = reply.send(None);
            }
            Envelope::Stop => self.stopping = true,
        }
    }

    /// 连上、登录、跑收发循环。`Err(true)` 表示别再试了。
    async fn attach(&mut self) -> Result<(), bool> {
        self.emit(
            FsdState::Connecting,
            Reason::Connecting {
                rating: self.role.rating(),
            },
        );

        let address = format!("{}:{}", self.host, self.port);
        let stream = match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&address)).await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                self.emit(FsdState::Error, Reason::ConnectFailed(e.to_string()));
                return Err(false);
            }
            Err(_) => {
                self.emit(
                    FsdState::Error,
                    Reason::ConnectFailed("timed out".to_string()),
                );
                return Err(false);
            }
        };
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();

        for p in self.role.handshake_packets() {
            if let Err(e) = send(&mut write, &p).await {
                self.emit(FsdState::Error, Reason::SendFailed(e.to_string()));
                return Err(false);
            }
        }

        if !self.await_login(&mut lines, &mut write).await? {
            return Err(false);
        }
        if !self.run_tick(&mut write).await {
            return Err(false);
        }
        self.retryable = true;
        self.emit(FsdState::Online, Reason::Online);

        self.pump(&mut lines, &mut write).await;
        let _ = send(&mut write, &self.role.logoff_packet()).await;
        Ok(())
    }

    async fn await_login<S>(
        &mut self,
        lines: &mut tokio::io::Lines<BufReader<S>>,
        write: &mut tokio::net::tcp::OwnedWriteHalf,
    ) -> Result<bool, bool>
    where
        S: tokio::io::AsyncRead + Unpin,
    {
        let deadline = tokio::time::sleep(LOGIN_TIMEOUT);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                _ = &mut deadline => {
                    self.emit(FsdState::Error, Reason::LoginTimeout);
                    return Ok(false);
                }
                got = self.cmd.recv() => match got {
                    Some(Envelope::Stop) | None => { self.stopping = true; return Err(true); }
                    Some(other) => self.apply_offline(other),
                },
                line = lines.next_line() => match line {
                    Ok(Some(raw)) => {
                        let raw = raw.trim();
                        if raw.is_empty() { continue; }
                        match self.handle(raw, write, false).await {
                            Handled::Continue => {}
                            Handled::LoggedIn => return Ok(true),
                            Handled::Fatal => return Ok(false),
                        }
                    }
                    Ok(None) | Err(_) => {
                        self.emit(FsdState::Error, Reason::Closed);
                        return Ok(false);
                    }
                },
            }
        }
    }

    async fn pump<S>(
        &mut self,
        lines: &mut tokio::io::Lines<BufReader<S>>,
        write: &mut tokio::net::tcp::OwnedWriteHalf,
    ) where
        S: tokio::io::AsyncRead + Unpin,
    {
        loop {
            // **每轮重新问间隔**：飞行员端停在地面时会自己降频，而一个建好就
            // 不变的 interval 会让那件事永远不生效。
            let tick = tokio::time::sleep(self.role.tick_interval());
            tokio::pin!(tick);
            tokio::select! {
                _ = &mut tick => {
                    if !self.run_tick(write).await { return; }
                }
                got = self.cmd.recv() => match got {
                    Some(Envelope::Stop) | None => { self.stopping = true; return; }
                    Some(Envelope::Role(c)) => {
                        for p in self.role.on_command(c) {
                            if send(write, &p).await.is_err() { return; }
                        }
                    }
                    Some(Envelope::RequestMetar { icao, reply }) => {
                        let p = packet::metar_request(self.role.callsign(), &icao);
                        if send(write, &p).await.is_err() {
                            let _ = reply.send(None);
                        } else {
                            self.metar_waiters.entry(icao).or_default().push(reply);
                        }
                    }
                },
                line = lines.next_line() => match line {
                    Ok(Some(raw)) => {
                        let raw = raw.trim();
                        if raw.is_empty() { continue; }
                        if matches!(self.handle(raw, write, true).await, Handled::Fatal) {
                            return;
                        }
                    }
                    Ok(None) | Err(_) => {
                        self.emit(FsdState::Error, Reason::Dropped);
                        return;
                    }
                },
            }
        }
    }

    async fn run_tick(&mut self, write: &mut tokio::net::tcp::OwnedWriteHalf) -> bool {
        let packets = match self.role.tick() {
            Ok(p) => p,
            Err(reason) => {
                self.emit(FsdState::Error, reason);
                return false;
            }
        };
        for p in packets {
            if let Err(e) = send(write, &p).await {
                self.emit(FsdState::Error, Reason::SendFailed(e.to_string()));
                return false;
            }
        }
        true
    }

    async fn handle(
        &mut self,
        raw: &str,
        write: &mut tokio::net::tcp::OwnedWriteHalf,
        logged_in: bool,
    ) -> Handled {
        tracing::debug!(packet = raw, "←");
        let callsign = self.role.callsign().to_string();
        let incoming = packet::parse(raw, &callsign);
        match &incoming {
            Incoming::Error { code, message } => {
                if !logged_in {
                    self.emit(
                        FsdState::Error,
                        Reason::Rejected {
                            code: code.clone(),
                            message: message.clone(),
                        },
                    );
                    return Handled::Fatal;
                }
                // 登录之后的 `$ER` 多半是某次查询失败（比如没有该机场的气象），
                // **不该把整条连接拆掉**。
                tracing::info!(%code, %message, "the server returned an error");
                self.fail_metar_waiters();
                return Handled::Continue;
            }
            Incoming::Metar(report) => {
                let report = report.clone();
                self.resolve_metar(&report);
                return Handled::Continue;
            }
            Incoming::CapsReply | Incoming::Heartbeat => return Handled::LoggedIn,
            Incoming::Ping { sender, rest } => {
                let p = packet::pong(&callsign, sender, rest);
                return match send(write, &p).await {
                    Ok(()) => Handled::Continue,
                    Err(_) => Handled::Fatal,
                };
            }
            Incoming::TextMessage(text) => tracing::info!(%text, "server message"),
            _ => {}
        }
        for p in self.role.on_packet(&incoming, raw) {
            if send(write, &p).await.is_err() {
                return Handled::Fatal;
            }
        }
        Handled::Continue
    }

    fn resolve_metar(&mut self, report: &str) {
        let station = packet::metar_station(report);
        // 报文里的电台代号优先；只有一个请求在等时也认——服务端偶尔回一份
        // 头部对不上的报文，而把它丢掉的话调用方就一直等到超时。
        let key = match station {
            Some(s) if self.metar_waiters.contains_key(&s) => Some(s),
            _ if self.metar_waiters.len() == 1 => self.metar_waiters.keys().next().cloned(),
            _ => None,
        };
        if let Some(key) = key {
            if let Some(waiters) = self.metar_waiters.remove(&key) {
                for w in waiters {
                    let _ = w.send(Some(report.to_string()));
                }
            }
        }
    }

    fn fail_metar_waiters(&mut self) {
        for (_, waiters) in std::mem::take(&mut self.metar_waiters) {
            for w in waiters {
                let _ = w.send(None);
            }
        }
    }
}

async fn send<W>(write: &mut W, p: &str) -> std::io::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    tracing::debug!(packet = %packet::redact(p), "→");
    write.write_all(p.as_bytes()).await?;
    write.write_all(b"\r\n").await
}
