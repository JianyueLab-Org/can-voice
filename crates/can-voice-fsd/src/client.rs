//! 一条 FSD 连接：连上 → 收发 → 掉线重连。
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
//! # 错误在一个地方翻译成"重连中"
//!
//! 重连期间的失败**不是终态**。在 [`Session::emit`] 里翻一次比在每个报错点各判
//! 一次可靠：连接那一段有五条报错路径、收发循环还有一条，漏掉任何一条，界面就会
//! 在我们正准备重连的时候把这条连接当成彻底没了。

use crate::packet::{self, CallsignProblem, Identity, Incoming, Position};
use std::collections::HashMap;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, oneshot};

/// 服务端 150 秒收不到位置包就断线。
pub const POSITION_INTERVAL: Duration = Duration::from_secs(15);
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(10);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 每次重连之间等一下，别贴着服务器猛敲。
pub const RECONNECT_DELAY: Duration = Duration::from_secs(3);

/// 已经登录过之后掉线，最多再试这么多次；都失败就这个席位整个下线。
///
/// **和语音那边是同一条策略**，两条链路的行为要一致，不然"整个下线"就没有
/// 统一的含义。
pub const RECONNECT_LIMIT: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsdState {
    Connecting,
    Online,
    Reconnecting,
    Error,
    /// 重连次数用尽，这个席位整个下线。**终态。**
    Offline,
    /// 有人按了停止。**终态。**
    Stopped,
}

/// 为什么。**是枚举不是字符串**：同一条原因在中英两种界面下要说两种话，
/// 而这一层不该知道现在是哪一种。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    Callsign(CallsignProblem),
    Connecting {
        rating: u32,
    },
    ConnectFailed(String),
    /// 服务端拒绝了登录。
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsdEvent {
    pub state: FsdState,
    pub reason: Reason,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub identity: Identity,
    pub position: Position,
    pub atis_lines: Vec<String>,
    pub reconnect_limit: u32,
}

enum Command {
    SetAtisLines(Vec<String>),
    SetFrequency(String),
    RequestMetar {
        icao: String,
        reply: oneshot::Sender<Option<String>>,
    },
    Stop,
}

/// 对外的把手。克隆随便，命令走一条队列。
#[derive(Debug, Clone)]
pub struct FsdHandle {
    cmd: mpsc::UnboundedSender<Command>,
    events: broadcast::Sender<FsdEvent>,
}

impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Command")
    }
}

impl FsdHandle {
    pub fn events(&self) -> broadcast::Receiver<FsdEvent> {
        self.events.subscribe()
    }

    /// 换一份通播文字。**超过 64 行的部分丢掉**——服务端只收这么多。
    pub fn set_atis_lines(&self, mut lines: Vec<String>) {
        lines.truncate(packet::MAX_ATIS_LINES);
        let _ = self.cmd.send(Command::SetAtisLines(lines));
    }

    pub fn set_frequency(&self, frequency: impl Into<String>) {
        let _ = self.cmd.send(Command::SetFrequency(frequency.into()));
    }

    /// 向服务端要一份 METAR。拿不到返回 `None`。
    pub async fn request_metar(&self, icao: &str, timeout: Duration) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        self.cmd
            .send(Command::RequestMetar {
                icao: icao.trim().to_uppercase(),
                reply: tx,
            })
            .ok()?;
        tokio::time::timeout(timeout, rx).await.ok()?.ok()?
    }

    pub fn stop(&self) {
        let _ = self.cmd.send(Command::Stop);
    }
}

/// 起一条连接。返回把手；后台任务自己跑到终态为止。
pub fn connect(config: Config) -> FsdHandle {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, _) = broadcast::channel(64);
    let handle = FsdHandle {
        cmd: cmd_tx,
        events: event_tx.clone(),
    };
    tokio::spawn(Session::new(config, cmd_rx, event_tx).run());
    handle
}

struct Session {
    config: Config,
    cmd: mpsc::UnboundedReceiver<Command>,
    events: broadcast::Sender<FsdEvent>,
    /// 登录成功过之后，失败就先当"可以重连"。见模块头。
    retryable: bool,
    stopping: bool,
    metar_waiters: HashMap<String, Vec<oneshot::Sender<Option<String>>>>,
}

impl Session {
    fn new(
        config: Config,
        cmd: mpsc::UnboundedReceiver<Command>,
        events: broadcast::Sender<FsdEvent>,
    ) -> Self {
        Self {
            config,
            cmd,
            events,
            retryable: false,
            stopping: false,
            metar_waiters: HashMap::new(),
        }
    }

    fn emit(&self, state: FsdState, reason: Reason) {
        // 重连期间的失败不是终态，在这里翻一次。见模块头。
        let state = if self.retryable && matches!(state, FsdState::Error | FsdState::Stopped) {
            FsdState::Reconnecting
        } else {
            state
        };
        tracing::info!(callsign = %self.config.identity.callsign, ?state, ?reason, "fsd");
        let _ = self.events.send(FsdEvent { state, reason });
    }

    async fn run(mut self) {
        let callsign = match packet::check_atis_callsign(&self.config.identity.callsign) {
            Ok(c) => c,
            Err(problem) => {
                self.emit(FsdState::Error, Reason::Callsign(problem));
                return;
            }
        };
        self.config.identity.callsign = callsign;

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
                        // 有人按了停止，或者呼号根本不对。
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
            if attempts > self.config.reconnect_limit {
                self.retryable = false;
                self.emit(
                    FsdState::Offline,
                    Reason::GaveUp {
                        limit: self.config.reconnect_limit,
                    },
                );
                return;
            }
            self.emit(
                FsdState::Reconnecting,
                Reason::Retrying {
                    attempt: attempts,
                    limit: self.config.reconnect_limit,
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
                    Some(Command::Stop) | None => { self.stopping = true; return true; }
                    Some(other) => self.apply_offline(other),
                },
            }
        }
    }

    /// 没连上的时候也要吃命令——界面照样在改频率和文字，
    /// 而重连之后要用的是**最新**那一份，不是掉线那一刻的。
    fn apply_offline(&mut self, command: Command) {
        match command {
            Command::SetAtisLines(lines) => self.config.atis_lines = lines,
            Command::SetFrequency(f) => self.config.position.frequency = f,
            Command::RequestMetar { reply, .. } => {
                let _ = reply.send(None);
            }
            Command::Stop => self.stopping = true,
        }
    }

    /// 连上、登录、跑收发循环。`Err(true)` 表示别再试了。
    async fn attach(&mut self) -> Result<(), bool> {
        self.emit(
            FsdState::Connecting,
            Reason::Connecting {
                rating: self.config.identity.rating,
            },
        );

        let address = format!("{}:{}", self.config.host, self.config.port);
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

        for p in [
            self.config.identity.id_packet(),
            self.config.identity.login_packet(),
            self.config.identity.caps_query(),
        ] {
            if let Err(e) = send(&mut write, &p).await {
                self.emit(FsdState::Error, Reason::SendFailed(e.to_string()));
                return Err(false);
            }
        }

        // 登录没有专门的成功包：等服务端答我们的 CAPS 查询。
        let logged_in = self.await_login(&mut lines, &mut write).await?;
        if !logged_in {
            return Err(false);
        }

        if !self.send_position(&mut write).await {
            return Err(false);
        }
        self.retryable = true;
        self.emit(FsdState::Online, Reason::Online);

        self.pump(&mut lines, &mut write).await;
        let _ = send(&mut write, &self.config.identity.logoff_packet()).await;
        Ok(())
    }

    async fn await_login<R>(
        &mut self,
        lines: &mut tokio::io::Lines<BufReader<R>>,
        write: &mut tokio::net::tcp::OwnedWriteHalf,
    ) -> Result<bool, bool>
    where
        R: tokio::io::AsyncRead + Unpin,
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
                    Some(Command::Stop) | None => { self.stopping = true; return Err(true); }
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

    async fn pump<R>(
        &mut self,
        lines: &mut tokio::io::Lines<BufReader<R>>,
        write: &mut tokio::net::tcp::OwnedWriteHalf,
    ) where
        R: tokio::io::AsyncRead + Unpin,
    {
        let mut ticker = tokio::time::interval(POSITION_INTERVAL);
        ticker.tick().await; // 第一次立刻返回，位置刚发过
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if !self.send_position(write).await { return; }
                }
                got = self.cmd.recv() => match got {
                    Some(Command::Stop) | None => { self.stopping = true; return; }
                    Some(Command::SetAtisLines(l)) => self.config.atis_lines = l,
                    Some(Command::SetFrequency(f)) => {
                        self.config.position.frequency = f;
                        // 频率变了立刻补一个位置包，别等下一个 15 秒——
                        // 在线列表里那 15 秒显示的是旧频率。
                        if !self.send_position(write).await { return; }
                    }
                    Some(Command::RequestMetar { icao, reply }) => {
                        let p = packet::metar_request(&self.config.identity.callsign, &icao);
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

    async fn handle(
        &mut self,
        raw: &str,
        write: &mut tokio::net::tcp::OwnedWriteHalf,
        logged_in: bool,
    ) -> Handled {
        tracing::debug!(packet = raw, "←");
        let callsign = self.config.identity.callsign.clone();
        match packet::parse(raw, &callsign) {
            Incoming::Error { code, message } => {
                if !logged_in {
                    self.emit(FsdState::Error, Reason::Rejected { code, message });
                    return Handled::Fatal;
                }
                // 登录之后的 $ER 多半是某次查询失败（比如没有该机场的气象），
                // **不该把整条连接拆掉**。
                tracing::info!(%code, %message, "the server returned an error");
                self.fail_metar_waiters();
            }
            Incoming::Metar(report) => self.resolve_metar(&report),
            Incoming::AtisQuery { sender } => {
                for p in packet::atis_reply(&callsign, &sender, &self.config.atis_lines) {
                    if send(write, &p).await.is_err() {
                        return Handled::Fatal;
                    }
                }
            }
            Incoming::CapsQuery { sender } => {
                let p = format!("$CR{callsign}:{sender}:CAPS:ATCINFO=1");
                if send(write, &p).await.is_err() {
                    return Handled::Fatal;
                }
            }
            Incoming::CapsReply | Incoming::Heartbeat => return Handled::LoggedIn,
            Incoming::Ping { sender, rest } => {
                let p = packet::pong(&callsign, &sender, &rest);
                if send(write, &p).await.is_err() {
                    return Handled::Fatal;
                }
            }
            Incoming::TextMessage(text) => tracing::info!(%text, "server message"),
            Incoming::Other => {}
        }
        Handled::Continue
    }

    async fn send_position(&mut self, write: &mut tokio::net::tcp::OwnedWriteHalf) -> bool {
        let Some(p) = self.config.position.packet(&self.config.identity.callsign) else {
            self.emit(
                FsdState::Error,
                Reason::BadFrequency(self.config.position.frequency.clone()),
            );
            return false;
        };
        match send(write, &p).await {
            Ok(()) => true,
            Err(e) => {
                self.emit(FsdState::Error, Reason::SendFailed(e.to_string()));
                false
            }
        }
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

enum Handled {
    Continue,
    LoggedIn,
    Fatal,
}

async fn send<W>(write: &mut W, p: &str) -> std::io::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    tracing::debug!(packet = %packet::redact(p), "→");
    write.write_all(p.as_bytes()).await?;
    write.write_all(b"\r\n").await
}
