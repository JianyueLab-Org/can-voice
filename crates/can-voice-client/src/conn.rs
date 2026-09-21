//! QUIC 连接与有界重连。
//!
//! # 为什么有界重连挡不住某些断开
//!
//! [`ReconnectPolicy`] 只数**连续失败**。被顶号之后的重连是**成功的**，
//! 一成功计数器就清零，于是两台机器登同一个账号会永远互相驱逐——
//! 三次上限根本拦不住，因为没有任何一次是失败。协议违规是同一个形状：
//! 重连立刻把同一个 bug 再演一遍，而重连本身照样"成功"。
//!
//! 所以断开之后必须读 **QUIC 的应用层关闭码**（[`classify`]），
//! 而不是一律交给重连策略。

use can_voice_proto::control::{self, Message};
use std::net::SocketAddr;
use std::sync::Arc;

/// 会话建立之后，一次掉线最多重连这么多次。
pub const RECONNECT_LIMIT: u32 = 3;

/// ALPN，与服务端一致。抄错的话 TLS 握手就失败，根本连不上。
pub const ALPN: &[u8] = b"can-voice/1";

/// 控制面消息队列深度。读取活在自己的 task 里，这是它与 `pump` 之间的缓冲。
const CONTROL_QUEUE: usize = 64;

// 关闭码，与服务端的 `internal/transport/codes.go` 一一对应。
// 它们是**协议的一部分**：客户端靠它们决定要不要重连。

/// 正常关闭。**进程重启部署走的就是这条**，所以它不是终态。
pub const CLOSE_NORMAL: u64 = 0;
/// 握手被拒。看原因串决定下一步，见 [`RefusedReason`]。
pub const CLOSE_HANDSHAKE_REFUSED: u64 = 1;
/// 同一个 CID 在别处登录，这一条被顶掉。**终态。**
pub const CLOSE_EVICTED: u64 = 2;
/// 协议违规。**终态**，去修客户端。
pub const CLOSE_PROTOCOL_VIOLATION: u64 = 3;

/// 链路状态。
///
/// `Reconnecting` 与 `Offline` 是对立的，上层必须区别对待：
/// `Reconnecting` 意味着链路还活着，**不要丢掉对象引用**；
/// `Offline` 意味着它彻底没了。`Evicted` 是 `Offline` 的一种，
/// 但要单独告诉用户"账号在别处登录了"。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum LinkState {
    Connecting,
    Online,
    Reconnecting,
    Offline,
    Evicted,
}

/// 握手被拒的原因。四个串都是服务端**专门为客户端造的**，有测试钉住它们稳定。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum RefusedReason {
    /// token 本身没问题，只是过期了。**唯一可恢复的一条。**
    TokenExpired,
    /// 形状、签名或内容不对。客户端或签发方的 bug。
    TokenInvalid,
    /// 这条消息不该出现在这里、`follow` 不是合法呼号，或者 rating 不够。
    Refused,
    /// `HELLO.proto` 不是本服务端讲的控制面版本。
    ///
    /// 动作和 [`RefusedReason::Refused`] 一样（都别原样重试），**但对人说的话
    /// 不一样**——这就是它值得单独占一个变体的全部理由：一个版本太旧的用户
    /// 该看到"请更新客户端"，而"被拒绝"会把他送去查密码、去换票、去怀疑账号，
    /// 三件事一件都帮不上忙。
    ProtoUnsupported,
    /// 服务端说了一个这一版客户端不认识的原因。
    Other(String),
}

impl RefusedReason {
    pub fn parse(s: &str) -> Self {
        match s {
            "token_expired" => Self::TokenExpired,
            "token_invalid" => Self::TokenInvalid,
            "refused" => Self::Refused,
            "proto_unsupported" => Self::ProtoUnsupported,
            other => Self::Other(other.to_string()),
        }
    }

    /// 去换一张新 token 再连一次就能好吗。只有 `token_expired` 是。
    pub fn is_recoverable(&self) -> bool {
        matches!(self, Self::TokenExpired)
    }
}

/// 协议违规的三种。处置相同（终态、去修客户端），但指向三个完全不同的 bug，
/// 所以必须把是哪一条透给上层——它们在本地都复现不了。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolViolation {
    /// 不读控制流了：服务端的写卡在流控上，于是它也不再读你的 SUB。
    ControlWriteStalled,
    /// 发了长度前缀却不把这一帧发完。
    ControlReadStalled,
    /// 声明太大，SUBACK 超过 64 KiB 发不出去——**而那份 SUB 已经生效了**，
    /// 意味着两端的订阅状态对不上。走到这里说明客户端侧的夹没夹住。
    AckUndeliverable,
    Other(String),
}

impl ProtocolViolation {
    pub fn parse(s: &str) -> Self {
        match s {
            "control_write_stalled" => Self::ControlWriteStalled,
            "control_read_stalled" => Self::ControlReadStalled,
            "ack_undeliverable" => Self::AckUndeliverable,
            other => Self::Other(other.to_string()),
        }
    }
}

/// 断开之后该怎么办。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    /// 走 [`ReconnectPolicy`]。
    Reconnect,
    /// 握手被拒；按 [`RefusedReason::is_recoverable`] 决定换票重连还是停。
    Refused(RefusedReason),
    /// 被顶号。**停止重连，并告诉用户账号在别处登录了。**
    Evicted,
    /// 客户端把协议用坏了。**停止重连，去修客户端。**
    ProtocolViolation(ProtocolViolation),
}

impl Disposition {
    /// 这是不是"别再连了"。
    ///
    /// 注意 `Refused(TokenExpired)` 不算终态：换一张票再连一次是对的。
    pub fn is_terminal(&self) -> bool {
        match self {
            Self::Reconnect => false,
            Self::Refused(r) => !r.is_recoverable(),
            Self::Evicted | Self::ProtocolViolation(_) => true,
        }
    }
}

/// 把关闭码与原因串翻译成处置。
///
/// **未知的码当成掉线**：未知只可能来自更新的服务端，而三条路里只有这一条
/// 不会把一个还能用的客户端变成砖头。
pub fn classify_close(code: u64, reason: &str) -> Disposition {
    match code {
        CLOSE_NORMAL => Disposition::Reconnect,
        CLOSE_HANDSHAKE_REFUSED => Disposition::Refused(RefusedReason::parse(reason)),
        CLOSE_EVICTED => Disposition::Evicted,
        CLOSE_PROTOCOL_VIOLATION => {
            Disposition::ProtocolViolation(ProtocolViolation::parse(reason))
        }
        _ => Disposition::Reconnect,
    }
}

/// 从 quinn 的连接错误里读出应用层关闭码与原因串。
///
/// 原因串装在 `ApplicationError.ErrorMessage` 里，**与关闭码原子地一起送达**，
/// 所以这是唯一一条丢不掉的通道——BYE 会丢（一个还没开始读的客户端收不到它）。
pub fn classify(err: &quinn::ConnectionError) -> Disposition {
    match err {
        quinn::ConnectionError::ApplicationClosed(close) => classify_close(
            close.error_code.into_inner(),
            &String::from_utf8_lossy(&close.reason),
        ),
        // 传输层错误就是掉线。
        _ => Disposition::Reconnect,
    }
}

/// 有界重连策略。
///
/// **在"尝试之前"计数，不在断开回调里计数。** 按回调计数统计的是
/// "掉线次数"而不是"尝试次数"：服务器一直不可用时只会触发一次回调，
/// 然后无限静默重试 —— 而服务端对登录失败是按账号限流的，
/// 一个僵尸重连循环足以把账号锁出语音。
///
/// **它挡不住驱逐和协议违规**，那两条要靠 [`classify`]，见模块文档。
#[derive(Debug)]
pub struct ReconnectPolicy {
    attempts: u32,
    ever_established: bool,
    state: LinkState,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl ReconnectPolicy {
    pub fn new() -> Self {
        Self {
            attempts: 0,
            ever_established: false,
            state: LinkState::Connecting,
        }
    }

    /// 现在可以（再）拨一次吗。
    pub fn may_attempt(&mut self) -> bool {
        // 第一次总是允许。
        if self.attempts == 0 && !self.ever_established {
            self.attempts = 1;
            self.state = LinkState::Connecting;
            return true;
        }
        // 从未建立过会话就不重试：那是密码错或地址错。
        if !self.ever_established {
            self.state = LinkState::Offline;
            return false;
        }
        if self.attempts >= RECONNECT_LIMIT {
            self.state = LinkState::Offline;
            return false;
        }
        self.attempts += 1;
        self.state = LinkState::Reconnecting;
        true
    }

    /// 会话**真的**建立了（收到 READY），此时才重置计数。
    ///
    /// 不要在"拨号返回成功"时调用它 —— 那只表示 TLS 套接字建好了，
    /// 密码错也会走到同一步。
    pub fn on_session_established(&mut self) {
        self.ever_established = true;
        self.attempts = 0;
        self.state = LinkState::Online;
    }

    /// 被顶号：进终态，并且不再允许任何重连。
    pub fn on_evicted(&mut self) {
        self.ever_established = false;
        self.attempts = RECONNECT_LIMIT;
        self.state = LinkState::Evicted;
    }

    pub fn state(&self) -> LinkState {
        self.state
    }
}

/// 呼号的形状，照抄 can-fsd 的 `IsValidCallsign`——服务端的 `isValidCallsign`
/// 也是照抄的那一份。**2–10 个字符，只许 `A-Z` `0-9` `-` `_`。**
///
/// 在发 HELLO **之前**自己判一次：服务端对不合规则的 `follow` 只回 `refused`，
/// 而那个串的意思是"停，别重试"，用户看到的是一个没有解释的终态。
/// 这是四个客户端里唯一一个**用户直接手输**的协议字段，输错是日常。
///
/// 别把规则放松或收紧：松了会放进永远查不到位置的值（它是去 datafeed 快照里
/// 按呼号查位置的键），紧了会把合法呼号挡在外面。
pub fn is_valid_callsign(s: &str) -> bool {
    (2..=10).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("connect: {0}")]
    Connect(#[from] quinn::ConnectError),
    #[error("connection: {0}")]
    Connection(#[from] quinn::ConnectionError),
    #[error("control: {0}")]
    Control(#[from] control::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("tls: {0}")]
    Tls(#[from] rustls::Error),
    #[error("the rustls configuration is not usable for QUIC")]
    QuicCrypto,
    #[error("server refused the session: {0:?}")]
    Refused(RefusedReason),
    #[error("server replied with {0} instead of READY")]
    UnexpectedReply(String),
    #[error("follow callsign {0:?} is not a callsign: 2-10 chars of A-Z 0-9 - _")]
    BadCallsign(String),
}

/// 校验服务端证书用哪套信任根。
///
/// **没有"跳过校验"这个选项，而且不会有。** 一个 `insecure` 开关一旦存在就会有人
/// 在生产里打开它，而这条链路上跑的是成员的网络密码。端到端测试要用自签证书时，
/// 把那张证书的 DER 传进来——在测试里和"不校验"一样方便，在生产里天差地别。
pub enum TrustRoots {
    /// 生产：系统信任链（服务端用 Let's Encrypt）。
    Platform,
    /// 额外的根证书。给端到端测试用。
    Extra(Vec<rustls_pki_types::CertificateDer<'static>>),
}

fn crypto_config(roots: &TrustRoots) -> Result<rustls::ClientConfig, Error> {
    // **显式指定 CryptoProvider，不依赖进程默认值。** 依赖默认值的话，哪天
    // 依赖图里多出一个 aws-lc-rs，第一次拨号就会 panic 在
    // "no process-level CryptoProvider available"——而那发生在运行时，不是构建时。
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?;

    let mut cfg = match roots {
        TrustRoots::Platform => {
            use rustls_platform_verifier::BuilderVerifierExt;
            builder.with_platform_verifier()?.with_no_client_auth()
        }
        TrustRoots::Extra(certs) => {
            let mut store = rustls::RootCertStore::empty();
            for c in certs {
                store.add(c.clone())?;
            }
            builder
                .with_root_certificates(Arc::new(store))
                .with_no_client_auth()
        }
    };
    cfg.alpn_protocols = vec![ALPN.to_vec()];
    Ok(cfg)
}

/// 一条已经握手完成的连接。
pub struct Link {
    pub conn: quinn::Connection,
    pub control_send: quinn::SendStream,
    pub control_recv: quinn::RecvStream,
    pub session: u32,
    /// READY 带来的限额。**不要在这里把它丢掉**——上层要靠它在声明之前夹住，
    /// 而不是靠 `rejected` 事后发现。
    pub max_tx: u32,
    pub max_rx: u32,
}

/// 握手时要报的身份。
///
/// 用结构体而不是四个相邻的 `&str`：它们类型相同，位置写反编译器一声不响，
/// 而 `follow` 和 `station` 写反的表现最难查——两个字段都按呼号校验，所以
/// 两边都过闸，只是射程按错误的位置算、顶号按错误的席位判。
#[derive(Debug, Clone, Copy)]
pub struct Identity<'a> {
    /// can-api 签发的短期 token。
    pub token: &'a str,
    /// 客户端标识，只进服务端日志。
    pub client_id: &'a str,
    /// 观察员跟随的呼号；不是观察员时留空。
    pub follow: &'a str,
    /// 席位标记；整队共用一个 CID 时填，否则留空。
    pub station: &'a str,
}

/// 客户端套接字的地址族必须和对端一致。
///
/// 绑死 `0.0.0.0` 的话，`lookup_host` 先给出 AAAA 时握手在套接字层就失败，
/// 错误长得像"语音服务挂了"。IPv6 对端绑 `[::]:0`，IPv4 对端绑 `0.0.0.0:0`。
fn client_bind(peer: SocketAddr) -> SocketAddr {
    if peer.is_ipv6() {
        SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 0))
    } else {
        SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, 0))
    }
}

/// 建立连接并完成 HELLO/READY 握手。
///
/// **它等握手真的完成才返回**，所以它的 `Err` 是有意义的：错的主机名、
/// 过期的 token、版本太旧，都在这里变成一条说得出原因的错误，而不是几秒之后
/// 一个没有理由的 `Offline`。
pub async fn connect(
    addr: SocketAddr,
    server_name: &str,
    id: Identity<'_>,
    roots: TrustRoots,
) -> Result<Link, Error> {
    // 观察员的呼号是用户手输的，先自己判一次，别拿一条 `refused` 去问用户。
    if !id.follow.is_empty() && !is_valid_callsign(id.follow) {
        return Err(Error::BadCallsign(id.follow.to_string()));
    }
    // 席位标记走同一道闸。它不是用户手输的，但通播机队是无人值守的：
    // 一条 `refused` 在那边的表现是这一路永远重连、永远被拒，而且没人在看。
    if !id.station.is_empty() && !is_valid_callsign(id.station) {
        return Err(Error::BadCallsign(id.station.to_string()));
    }

    let crypto = crypto_config(&roots)?;
    let client_cfg = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto).map_err(|_| Error::QuicCrypto)?,
    ));

    let mut endpoint = quinn::Endpoint::client(client_bind(addr))?;
    endpoint.set_default_client_config(client_cfg);

    let conn = endpoint.connect(addr, server_name)?.await?;
    let (mut send, mut recv) = conn.open_bi().await?;

    let hello = Message::Hello(control::Hello {
        token: id.token.to_string(),
        client: id.client_id.to_string(),
        proto: control::PROTO_VERSION,
        follow: id.follow.to_string(),
        station: id.station.to_string(),
    });
    write_msg(&mut send, &hello).await?;

    // 读握手回复时的错误**先问关闭码**，再当成 I/O 错误。
    //
    // 服务端拒绝握手时同时发 BYE 和 CONNECTION_CLOSE 的原因串，而**后者才是权威**
    // ——BYE 走控制流，一个还没开始读的客户端收不到它。只认 BYE 的话，
    // BYE 丢了就退化成一条没有原因的 I/O 错误，而"没有理由的失败"正是这一整套
    // 原因串存在的理由。
    let reply = match read_one(&mut recv).await {
        Ok(m) => m,
        Err(e) => return Err(handshake_error(classify_close_of(&conn), e)),
    };
    match reply {
        Message::Ready(r) => {
            tracing::info!(session = r.session, server = %r.server, "voice session established");
            Ok(Link {
                conn,
                control_send: send,
                control_recv: recv,
                session: r.session,
                max_tx: r.max_tx,
                max_rx: r.max_rx,
            })
        }
        Message::Bye(b) => Err(Error::Refused(RefusedReason::parse(&b.reason))),
        other => Err(Error::UnexpectedReply(format!("{other:?}"))),
    }
}

/// 连接已经带着关闭码没了吗。没有关闭码时返回 `None`。
fn classify_close_of(conn: &quinn::Connection) -> Option<Disposition> {
    conn.close_reason().as_ref().map(classify)
}

/// 握手期读失败时，把关闭码翻译成一条说得出原因的错误。
///
/// 纯函数，因为它是这条路径上唯一一处判断，而制造一个"BYE 丢了"的真实场景
/// 需要一个会说谎的服务端。
fn handshake_error(disposition: Option<Disposition>, fallback: Error) -> Error {
    match disposition {
        Some(Disposition::Refused(reason)) => Error::Refused(reason),
        Some(Disposition::Evicted) => Error::Refused(RefusedReason::Other("evicted".into())),
        Some(Disposition::ProtocolViolation(v)) => {
            Error::UnexpectedReply(format!("protocol violation: {v:?}"))
        }
        // 码 0 或者传输层错误：那就是普通的掉线，原样回去。
        Some(Disposition::Reconnect) | None => fallback,
    }
}

/// 在控制流上写一条消息。
pub async fn write_msg(s: &mut quinn::SendStream, m: &Message) -> Result<(), Error> {
    let body = m.encode()?;
    let mut framed = Vec::with_capacity(body.len() + 4);
    control::write_frame(&mut framed, &body)?;
    s.write_all(&framed)
        .await
        .map_err(|e| Error::Io(std::io::Error::other(e)))?;
    Ok(())
}

/// 把控制流的读取搬进**一个独立的 task**，通过 mpsc 交付解析好的消息。
///
/// 这是整个 C3 的形状。直接在 `select!` 里调用一个"读长度前缀再读包体"的
/// future 不是取消安全的：别的分支赢了的时候它会在两次读之间被丢掉，
/// **已经消费掉的字节回不来了**，控制流从此错位，之后每一帧都解析失败——
/// 最可能的表现是被读成"控制流结束"→ 断开 → 重连，而链路其实一直是好的，
/// 本地必现不了。
///
/// 独立 task 把这一整类问题消掉：`pump` 的 `select!` 只 `recv()`，
/// 而 `mpsc::Receiver::recv()` 本身是取消安全的。**不要**试图用"加个缓冲区
/// 记住读了多少"来救——那是在手写一个状态机去模拟取消安全。
pub fn spawn_control_reader<R>(reader: R) -> tokio::sync::mpsc::Receiver<Result<Message, Error>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::channel(CONTROL_QUEUE);
    tokio::spawn(async move {
        let mut reader = reader;
        loop {
            match read_frame_async(&mut reader).await {
                Ok(Some(body)) => {
                    let msg = Message::decode(&body).map_err(Error::from);
                    let fatal = msg.is_err();
                    if tx.send(msg).await.is_err() || fatal {
                        return;
                    }
                }
                // 对端正常收尾。
                Ok(None) => return,
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
            }
        }
    });
    rx
}

/// 读一个长度前缀帧。流正常结束时返回 `Ok(None)`。
///
/// 长度上限走 [`control::check_frame_len`]，**不在这里重写那段算术**：
/// 两份实现一旦分叉就是协议错位，而且没有任何测试能抓到——被测的那份
/// 从不上线，上线的那份从不被测。
async fn read_frame_async<R>(r: &mut R) -> Result<Option<Vec<u8>>, Error>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut hdr = [0u8; 4];
    match r.read_exact(&mut hdr).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(Error::Io(e)),
    }
    let n = control::check_frame_len(u32::from_be_bytes(hdr))?;
    let mut body = vec![0u8; n];
    r.read_exact(&mut body).await.map_err(Error::Io)?;
    Ok(Some(body))
}

/// 握手期间读一条消息。此时还没有 `pump`，所以直接读，没有取消安全的问题。
async fn read_one(r: &mut quinn::RecvStream) -> Result<Message, Error> {
    let mut hdr = [0u8; 4];
    r.read_exact(&mut hdr)
        .await
        .map_err(|e| Error::Io(std::io::Error::other(e)))?;
    let n = control::check_frame_len(u32::from_be_bytes(hdr))?;
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)
        .await
        .map_err(|e| Error::Io(std::io::Error::other(e)))?;
    Ok(Message::decode(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ——— 重连策略 ———

    #[test]
    fn a_first_connection_failure_is_not_retried() {
        // 首次连接失败是密码错或地址错。重试只是把同一个错误打印三遍。
        let mut p = ReconnectPolicy::new();
        assert!(p.may_attempt(), "the first attempt is always allowed");
        assert!(
            !p.may_attempt(),
            "a never-established link must not be retried"
        );
        assert_eq!(p.state(), LinkState::Offline);
    }

    #[test]
    fn an_established_session_gets_exactly_three_reconnects() {
        let mut p = ReconnectPolicy::new();
        assert!(p.may_attempt());
        p.on_session_established();

        for i in 0..RECONNECT_LIMIT {
            assert!(
                p.may_attempt(),
                "reconnect {} of {RECONNECT_LIMIT} must be allowed",
                i + 1
            );
            assert_eq!(p.state(), LinkState::Reconnecting);
        }
        assert!(!p.may_attempt(), "the fourth reconnect must be refused");
        assert_eq!(p.state(), LinkState::Offline);
    }

    #[test]
    fn the_counter_resets_only_when_a_session_is_really_established() {
        // connect() 返回成功不等于连上了 —— 它只是建了 TLS 套接字。
        // 密码错也会返回同样的结果，然后死在后面。按返回值重置计数
        // 等于重建了无限循环，而且正对着服务端的按账号登录限流。
        let mut p = ReconnectPolicy::new();
        p.may_attempt();
        p.on_session_established();

        p.may_attempt();
        p.may_attempt();
        p.on_session_established(); // 这次是真连上了
        for i in 0..RECONNECT_LIMIT {
            assert!(
                p.may_attempt(),
                "the counter should have reset; attempt {} refused",
                i + 1
            );
        }
        assert!(!p.may_attempt());
    }

    #[test]
    fn state_is_connecting_before_the_first_attempt_resolves() {
        let p = ReconnectPolicy::new();
        assert_eq!(p.state(), LinkState::Connecting);
    }

    #[test]
    fn state_is_online_while_a_session_is_up() {
        let mut p = ReconnectPolicy::new();
        p.may_attempt();
        p.on_session_established();
        assert_eq!(p.state(), LinkState::Online);
    }

    // ——— C2：关闭码决定要不要重连 ———

    /// **`ReconnectPolicy` 的三次上限挡不住驱逐循环**，因为驱逐后的重连是
    /// **成功**的，计数器一成功就清零。两台机器登同一个账号会互相踢到天荒地老，
    /// 两边界面都显示"已连接"，而服务端日志被登录风暴淹没。
    /// 唯一的出路是读关闭码，把码 2 当终态。
    #[test]
    fn eviction_is_terminal_and_the_reconnect_counter_cannot_save_us() {
        assert_eq!(
            classify_close(CLOSE_EVICTED, "evicted"),
            Disposition::Evicted
        );
        assert!(Disposition::Evicted.is_terminal());

        // 演示计数器为什么挡不住：每一次重连都"成功"，于是永远重置。
        let mut p = ReconnectPolicy::new();
        p.may_attempt();
        for _ in 0..100 {
            p.on_session_established(); // 连上了……
            assert!(p.may_attempt(), "…然后被顶掉，而计数器刚刚被清零");
        }
    }

    #[test]
    fn a_normal_close_is_the_deploy_path_and_may_reconnect() {
        // 服务端进程重启部署走的正是码 0。把它当终态意味着**每次部署之后
        // 所有客户端永不回来**。
        assert_eq!(classify_close(CLOSE_NORMAL, ""), Disposition::Reconnect);
        assert!(!Disposition::Reconnect.is_terminal());
    }

    #[test]
    fn a_handshake_refusal_carries_a_reason_the_client_must_act_on() {
        assert_eq!(
            classify_close(CLOSE_HANDSHAKE_REFUSED, "token_expired"),
            Disposition::Refused(RefusedReason::TokenExpired)
        );
        // 只有这一条是可恢复的：去换一张新票再连一次，而不是走重连策略。
        assert!(RefusedReason::TokenExpired.is_recoverable());
        assert!(!RefusedReason::TokenInvalid.is_recoverable());
        assert!(!RefusedReason::Refused.is_recoverable());
        assert!(!RefusedReason::ProtoUnsupported.is_recoverable());
    }

    /// 修订件 N2：`proto_unsupported` 的**动作**和 `refused` 一样（都别重试），
    /// 分开是因为**对人说的话不一样**。一个版本太旧的用户该看到"请更新客户端"；
    /// 告诉他"被拒绝"会把他送去查密码、去换票、去怀疑自己的账号——
    /// 三件事一件都帮不上忙，他目录里那个旧 exe 才是原因。
    #[test]
    fn an_outdated_client_is_told_to_update_not_that_it_was_refused() {
        let d = classify_close(CLOSE_HANDSHAKE_REFUSED, "proto_unsupported");
        assert_eq!(d, Disposition::Refused(RefusedReason::ProtoUnsupported));
        assert_ne!(
            d,
            Disposition::Refused(RefusedReason::Refused),
            "collapsing it into Refused throws away the one thing the server made this string for"
        );
    }

    /// 修订件 N3：关闭码 3 现在有**三个**原因串，处置相同但指向三个不同的 bug。
    #[test]
    fn a_protocol_violation_names_which_bug_it_is() {
        for (reason, want) in [
            (
                "control_write_stalled",
                ProtocolViolation::ControlWriteStalled,
            ),
            (
                "control_read_stalled",
                ProtocolViolation::ControlReadStalled,
            ),
            ("ack_undeliverable", ProtocolViolation::AckUndeliverable),
        ] {
            let d = classify_close(CLOSE_PROTOCOL_VIOLATION, reason);
            assert_eq!(
                d,
                Disposition::ProtocolViolation(want.clone()),
                "reason {reason}"
            );
            assert!(d.is_terminal(), "reconnecting replays the same bug forever");
        }
    }

    #[test]
    fn an_unknown_close_code_is_treated_as_a_network_drop() {
        // 未知的码只可能来自更新的服务端。当成掉线走重连策略，
        // 是三条路里唯一不会把一个能用的客户端变成砖头的。
        assert_eq!(classify_close(99, "whatever"), Disposition::Reconnect);
    }

    // ——— N4：`follow` 要在发出去之前自己判 ———

    /// QUIC 客户端套接字的地址族必须和对端一致。绑死 `0.0.0.0` 的话，
    /// `lookup_host` 先给出 AAAA 时握手在套接字层就失败，看起来像语音服务挂了。
    #[test]
    fn the_client_socket_matches_the_peer_address_family() {
        let v4: SocketAddr = "1.2.3.4:64738".parse().expect("v4");
        let v6: SocketAddr = "[2001:db8::1]:64738".parse().expect("v6");
        assert!(client_bind(v4).is_ipv4(), "an IPv4 peer needs an IPv4 bind");
        assert_eq!(client_bind(v4).port(), 0);
        assert!(client_bind(v6).is_ipv6(), "an IPv6 peer needs an IPv6 bind");
        assert_eq!(client_bind(v6).port(), 0);
    }

    /// 规则照抄 can-fsd 的 `IsValidCallsign`（服务端的 `isValidCallsign` 也是照抄的）：
    /// 2–10 个字符，只许 `A-Z` `0-9` `-` `_`。松了会放进永远查不到位置的值，
    /// 紧了会把合法呼号挡在外面。
    #[test]
    fn a_follow_callsign_is_checked_before_it_goes_on_the_wire() {
        for good in ["CCA", "CCA1501", "ZSPD_TWR", "A-1", "AB"] {
            assert!(is_valid_callsign(good), "{good} should be valid");
        }
        for bad in [
            "",
            "A",
            "ABCDEFGHIJK",
            "cca150",
            "CCA 150",
            "CCA.150",
            "呼号",
        ] {
            assert!(!is_valid_callsign(bad), "{bad:?} should be rejected");
        }
    }

    // ——— C3：控制流的读取必须是取消安全的 ———

    fn frame_of(m: &Message) -> Vec<u8> {
        let body = m.encode().expect("encode");
        let mut out = Vec::new();
        can_voice_proto::control::write_frame(&mut out, &body).expect("frame");
        out
    }

    #[tokio::test]
    async fn the_control_reader_delivers_whole_messages() {
        let (mut w, r) = tokio::io::duplex(4096);
        let mut rx = spawn_control_reader(r);
        for t in [1i64, 2, 3] {
            let bytes = frame_of(&Message::Ping(control::Ping { t }));
            tokio::io::AsyncWriteExt::write_all(&mut w, &bytes)
                .await
                .expect("write");
        }
        for t in [1i64, 2, 3] {
            match rx.recv().await.expect("a message").expect("ok") {
                Message::Ping(p) => assert_eq!(p.t, t),
                other => panic!("got {other:?}"),
            }
        }
    }

    /// **这是 C3 要防的那个 bug。**
    ///
    /// `read_msg` 连着做两次 `read_exact`（4 字节长度前缀，然后包体）。写在
    /// `select!` 里的话，别的分支赢了的时候这个 future 会在两次读之间被丢掉，
    /// **已经消费掉的字节回不来了**，控制流从此错位——之后每一帧都解析失败，
    /// 最可能的表现是被读成"控制流结束"→ 断开 → 重连，而链路其实一直是好的。
    ///
    /// 这里把消息拆成一个字节一个字节地写，中间让消费端反复在 `select!` 里被超时
    /// 打断。读取活在自己的 task 里、只通过 mpsc 交付，所以一个字节都不会丢；
    /// `mpsc::Receiver::recv()` 本身是取消安全的。
    #[tokio::test]
    async fn a_cancelled_select_does_not_lose_control_stream_bytes() {
        let (mut w, r) = tokio::io::duplex(4096);
        let mut rx = spawn_control_reader(r);

        let writer = tokio::spawn(async move {
            for t in 0i64..8 {
                for b in frame_of(&Message::Ping(control::Ping { t })) {
                    tokio::io::AsyncWriteExt::write_all(&mut w, &[b])
                        .await
                        .expect("write");
                    tokio::task::yield_now().await;
                }
            }
        });

        let mut got = Vec::new();
        while got.len() < 8 {
            tokio::select! {
                m = rx.recv() => {
                    match m.expect("channel alive").expect("ok") {
                        Message::Ping(p) => got.push(p.t),
                        other => panic!("got {other:?}"),
                    }
                }
                // 这一支不断地取消上面那一支。取消安全的实现毫发无损。
                _ = tokio::time::sleep(std::time::Duration::from_micros(50)) => {}
            }
        }
        writer.await.expect("writer");
        assert_eq!(
            got,
            (0i64..8).collect::<Vec<_>>(),
            "a cancel-safe reader loses neither bytes nor ordering"
        );
    }

    /// M3：长度上限只有一处实现，异步这一侧不许自己重写一遍那段算术。
    #[tokio::test]
    async fn an_oversized_length_prefix_is_refused_by_the_shared_check() {
        let (mut w, r) = tokio::io::duplex(64);
        let mut rx = spawn_control_reader(r);
        tokio::io::AsyncWriteExt::write_all(&mut w, &[0xff, 0xff, 0xff, 0xff])
            .await
            .expect("write");
        let err = rx.recv().await.expect("a result").expect_err("must refuse");
        assert!(
            matches!(
                err,
                Error::Control(can_voice_proto::control::Error::TooLarge(_))
            ),
            "got {err:?}"
        );
    }

    /// **这条测试证明 C3 要防的 bug 真的存在**，而且是确定性的、不靠时序。
    ///
    /// 直接调用"读长度前缀再读包体"的 future 并在中途取消它（这里用
    /// `timeout` 制造取消，`select!` 里别的分支赢了是同一回事）：
    /// 4 字节前缀已经被消费掉、包体还没到，future 被丢弃——**那 4 个字节回不来了**。
    /// 下一次读会把包体的前 4 个字节当成长度前缀，控制流从此错位。
    ///
    /// 它是确定性的：前缀一定读得到，包体一定读不到，所以取消一定发生在两次读之间。
    #[tokio::test]
    async fn cancelling_a_read_between_the_prefix_and_the_body_eats_the_prefix() {
        let (mut w, mut r) = tokio::io::duplex(4096);
        let frame = frame_of(&Message::Ping(control::Ping { t: 7 }));
        let (prefix, body) = frame.split_at(4);

        // 只给前缀。
        tokio::io::AsyncWriteExt::write_all(&mut w, prefix)
            .await
            .expect("write");
        let cancelled = tokio::time::timeout(
            std::time::Duration::from_millis(20),
            read_frame_async(&mut r),
        )
        .await;
        assert!(
            cancelled.is_err(),
            "the read must still be waiting for the body"
        );

        // 现在把包体和一整帧都给它。
        tokio::io::AsyncWriteExt::write_all(&mut w, body)
            .await
            .expect("write");
        let next = frame_of(&Message::Ping(control::Ping { t: 8 }));
        tokio::io::AsyncWriteExt::write_all(&mut w, &next)
            .await
            .expect("write");

        // 那 4 个字节已经没了，于是下一次读拿包体的头 4 个字节当长度前缀。
        let out = read_frame_async(&mut r).await;
        let misread = match out {
            Ok(Some(b)) => Message::decode(&b).is_err(),
            Ok(None) | Err(_) => true,
        };
        assert!(
            misread,
            "the stream should be desynchronised — if this ever passes cleanly, \
             the cancel-safety hazard has changed and spawn_control_reader's rationale needs rechecking"
        );
    }
    // ——— 握手期的错误要说得出原因 ———

    /// **BYE 会丢，关闭码不会。** 服务端拒绝握手时同时发两样，而原因串是和关闭码
    /// 原子地一起送达的；BYE 走控制流，一个还没开始读的客户端收不到它。
    ///
    /// 只认 BYE 的实现会在 BYE 丢掉时退化成一条没有原因的 I/O 错误——而
    /// "没有理由的失败"正是这一整套原因串存在的理由，也是上层决定"换张票再试"
    /// 还是"别试了"的唯一依据。
    #[test]
    fn a_lost_bye_still_leaves_the_reason_in_the_close_code() {
        let io = || Error::Io(std::io::Error::other("stream closed"));

        let e = handshake_error(
            Some(Disposition::Refused(RefusedReason::TokenExpired)),
            io(),
        );
        assert!(
            matches!(e, Error::Refused(RefusedReason::TokenExpired)),
            "got {e:?}"
        );

        let e = handshake_error(
            Some(Disposition::Refused(RefusedReason::ProtoUnsupported)),
            io(),
        );
        assert!(
            matches!(e, Error::Refused(RefusedReason::ProtoUnsupported)),
            "got {e:?}"
        );
    }

    /// 被顶号也要说得出口：上层据此告诉用户"账号在别处登录了"，
    /// 而不是让他去查密码。
    #[test]
    fn eviction_during_the_handshake_is_not_a_bare_io_error() {
        let e = handshake_error(
            Some(Disposition::Evicted),
            Error::Io(std::io::Error::other("x")),
        );
        assert!(!matches!(e, Error::Io(_)), "got {e:?}");
    }

    /// 普通掉线没有可说的原因，原样回去——**不要编一个**。
    #[test]
    fn an_ordinary_drop_keeps_its_own_error() {
        let e = handshake_error(
            Some(Disposition::Reconnect),
            Error::Io(std::io::Error::other("x")),
        );
        assert!(matches!(e, Error::Io(_)), "got {e:?}");
        let e = handshake_error(None, Error::Io(std::io::Error::other("x")));
        assert!(matches!(e, Error::Io(_)), "got {e:?}");
    }
}
