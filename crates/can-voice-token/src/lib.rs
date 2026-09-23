//! 拿凭据换语音 token，并在票过期时换一张再连一次。
//!
//! # 为什么它不在核心库里
//!
//! `can-voice-client` 刻意不碰凭据：它收一张票，验签和会话之外的事一概不知。
//! 换票要发 HTTP、要持密码，把那些塞进核心库等于让四个客户端、ATIS 机队和
//! 将来任何一个消费者都背上一个它们未必需要的依赖。
//!
//! # 换票是**唯一**一条进来的路
//!
//! [`TokenSource`] 只有一个要凭据的构造函数——没有 `from_token`、没有
//! `with_token`。能被塞一张长期票的东西，那张票就成了一个没人管的凭据。

use can_voice_client::client::Error as ClientError;
use can_voice_client::conn::Error as ConnError;
use can_voice_client::{Config, VoiceClient};
use can_voice_i18n::Message;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// **凭据不对**——和网络不通分开的理由是：要用户做的事完全不同。
    /// 两者以前是同一句 `could not reach can-api`，于是一个打错密码的人
    /// 被送去查网络。
    #[error("can-api rejected the CAN ID or password")]
    Credentials,
    /// **账号还没定级**——凭据是对的。can-api 给 rating < 1 的成员专门回 403
    /// `insufficient_rating`，和 401 分开的理由和上面一条一样：归进凭据那一类
    /// 的话，一个密码完全正确的人会被送去改密码，而每重试一次，can-api 按
    /// CAN ID 的限流就多记一格。机队用的账号也一样——操作员会去查
    /// `ATIS_PASSWORD`，而问题在 rating。
    #[error(
        "can-api refused the token exchange: this account has no rating yet (needs rating >= 1)"
    )]
    NotRated,
    /// can-api 答了，但答的不是成功。状态码原样带出来：这一类要看的是它，
    /// 而把它归到密码上会让人去改一个本来对的密码。
    #[error("can-api refused the token exchange: HTTP {0}")]
    Rejected(reqwest::StatusCode),
    #[error("voice authority is not yet visible in the FSD feed")]
    ScopePending,
    #[error("voice authority feed is temporarily unavailable")]
    AuthorityUnavailable,
    #[error("could not reach can-api: {0}")]
    Http(#[from] reqwest::Error),
    #[error("voice: {0}")]
    Voice(#[from] ClientError),
}

impl Error {
    /// 给人看的那一句（#29）。`Display` 是给日志的英文，这里是界面上的。
    ///
    /// 服务端拒绝的几种原因**各说各的**：版本太旧该去更新、票过期多半是时钟不对，
    /// 而一句笼统的"被拒绝"会把这些人都送去查密码。
    pub fn message(&self) -> Message {
        use can_voice_client::conn::RefusedReason;
        match self {
            Error::Credentials => Message::new("error.token.credentials"),
            Error::NotRated => Message::new("error.token.not_rated"),
            Error::Rejected(status) => {
                Message::new("error.token.rejected").with("status", status.as_u16())
            }
            Error::ScopePending => Message::new("error.token.rejected").with("status", 403),
            Error::AuthorityUnavailable => Message::new("error.token.rejected").with("status", 503),
            Error::Http(e) => Message::new("error.token.unreachable").with("detail", e),
            Error::Voice(ClientError::Conn(ConnError::Refused(reason))) => match reason {
                RefusedReason::TokenExpired => Message::new("error.voice.token_expired"),
                RefusedReason::TokenInvalid => Message::new("error.voice.token_invalid"),
                RefusedReason::Refused => Message::new("error.voice.refused"),
                RefusedReason::ProtoUnsupported => Message::new("error.voice.proto_unsupported"),
                RefusedReason::Other(why) => {
                    Message::new("error.voice.refused_other").with("reason", why)
                }
            },
            Error::Voice(ClientError::Conn(ConnError::BadCallsign(callsign))) => {
                Message::new("error.voice.bad_callsign").with("callsign", callsign)
            }
            Error::Voice(ClientError::BadAddress(address)) => {
                Message::new("error.voice.bad_address").with("address", address)
            }
            Error::Voice(e) => Message::new("error.voice.unreachable").with("detail", e),
        }
    }
}

/// 换票的地方。
///
/// **`Debug` 是手写的，密码印成 `***`。** 派生的那个会把明文密码带进任何一句
/// 调试日志或者一条 panic 信息：机队的密码进容器日志，桌面端的进那个可以一键
/// 回传的日志文件。装着它的结构（比如机队的 `VoiceSettings`）照样可以派生
/// `Debug`——挡在这一层，外面就不用记得这件事。
#[derive(Clone)]
pub struct TokenSource {
    endpoint: String,
    cid: String,
    password: String,
    scope: Option<TokenScope>,
    http: reqwest::Client,
}

/// Requested role and one radio frequency. The issuer independently verifies
/// the role and assignment; this value never grants authority by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenScope {
    Pilot { callsign: String, frequency: u32 },
    Observer { callsign: String, frequency: u32 },
    Controller { callsign: String, frequency: u32 },
    Atis { station: String, frequency: u32 },
}

impl TokenScope {
    pub fn pilot(callsign: impl Into<String>, frequency: u32) -> Self {
        Self::Pilot {
            callsign: callsign.into(),
            frequency,
        }
    }

    pub fn observer(callsign: impl Into<String>, frequency: u32) -> Self {
        Self::Observer {
            callsign: callsign.into(),
            frequency,
        }
    }

    pub fn controller(callsign: impl Into<String>, frequency: u32) -> Self {
        Self::Controller {
            callsign: callsign.into(),
            frequency,
        }
    }

    pub fn atis(station: impl Into<String>, frequency: u32) -> Self {
        Self::Atis {
            station: station.into(),
            frequency,
        }
    }
}

impl std::fmt::Debug for TokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // CID 留着：它不是秘密，而看日志的人要靠它认出是哪个账号。
        f.debug_struct("TokenSource")
            .field("endpoint", &self.endpoint)
            .field("cid", &self.cid)
            .field("password", &"***")
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

#[derive(serde::Deserialize)]
struct Reply {
    token: String,
}

impl TokenSource {
    /// `api_origin` 是 can-api 的根地址，例如 `https://api.ceruleanavi.net`。
    pub fn new(
        api_origin: &str,
        cid: impl Into<String>,
        password: impl Into<String>,
        http: reqwest::Client,
    ) -> Self {
        Self {
            endpoint: endpoint_for(api_origin),
            cid: cid.into(),
            password: password.into(),
            scope: None,
            http,
        }
    }

    pub fn with_scope(mut self, scope: TokenScope) -> Self {
        self.scope = Some(scope);
        self
    }

    pub fn with_optional_scope(mut self, scope: Option<TokenScope>) -> Self {
        self.scope = scope;
        self
    }

    pub fn scope(&self) -> Option<&TokenScope> {
        self.scope.as_ref()
    }

    fn request_body(&self) -> serde_json::Value {
        let mut body = serde_json::json!({ "cid": self.cid, "password": self.password });
        let Some(scope) = self.scope.as_ref() else {
            return body;
        };
        let (role, callsign, station, frequency) = match scope {
            TokenScope::Pilot {
                callsign,
                frequency,
            } => ("pilot", Some(callsign), None, frequency),
            TokenScope::Observer {
                callsign,
                frequency,
            } => ("observer", Some(callsign), None, frequency),
            TokenScope::Controller {
                callsign,
                frequency,
            } => ("controller", Some(callsign), None, frequency),
            TokenScope::Atis { station, frequency } => ("atis", None, Some(station), frequency),
        };
        let object = body.as_object_mut().expect("request body is an object");
        object.insert("role".into(), serde_json::json!(role));
        object.insert("frequency".into(), serde_json::json!(frequency));
        if let Some(callsign) = callsign {
            object.insert("callsign".into(), serde_json::json!(callsign));
        }
        if let Some(station) = station {
            object.insert("station".into(), serde_json::json!(station));
        }
        body
    }

    /// 换一张票。
    ///
    /// **每次连接都现换。** can-api 签的是 60 秒的票，攒着没有意义——而票的短寿命
    /// 就是这套设计里唯一的吊销机制。
    pub async fn fetch(&self) -> Result<String, Error> {
        let resp = self
            .http
            .post(&self.endpoint)
            .json(&self.request_body())
            .send()
            .await?;
        // 状态码在解 JSON **之前**分类：`error_for_status()` 把 401 和一个连不上
        // 的 socket 包成同一个 `reqwest::Error`，而那正是要分开的两件事。
        let status = resp.status();
        if !status.is_success() {
            // 403 还要看一眼**错误码**：can-api 用它把"还没定级"和"凭据不对"分开，
            // 而这两种人要做的事完全不同。读不出来的时候当凭据问题——
            // 那是 403 的另一种来由，也是两者里更常见的一种。
            let body = resp.text().await.unwrap_or_default();
            return Err(classify(status, &body));
        }
        Ok(resp.json::<Reply>().await?.token)
    }
}

/// can-api 给未定级成员回的错误码（`internal/api/voicetoken.go`）。
const INSUFFICIENT_RATING: &str = "insufficient_rating";
const VOICE_SCOPE_REFUSED: &str = "voice_scope_refused";
const VOICE_AUTHORITY_UNAVAILABLE: &str = "voice_authority_unavailable";

/// 状态码加错误码 → 哪一种失败。
fn classify(status: reqwest::StatusCode, body: &str) -> Error {
    match status {
        reqwest::StatusCode::FORBIDDEN
            if error_code(body).as_deref() == Some(INSUFFICIENT_RATING) =>
        {
            Error::NotRated
        }
        reqwest::StatusCode::FORBIDDEN
            if error_code(body).as_deref() == Some(VOICE_SCOPE_REFUSED) =>
        {
            Error::ScopePending
        }
        reqwest::StatusCode::SERVICE_UNAVAILABLE
            if error_code(body).as_deref() == Some(VOICE_AUTHORITY_UNAVAILABLE) =>
        {
            Error::AuthorityUnavailable
        }
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => Error::Credentials,
        other => Error::Rejected(other),
    }
}

/// can-api 错误响应里的 `error` 字段。
///
/// 解不出来不是错：**分类不能依赖响应体的形状**——中间挡着的一层代理很可能
/// 回的是一页 HTML，而那时该说的仍然是"凭据没通过"。
fn error_code(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_string)
}

/// 拼换票的地址。
///
/// 结尾那个斜杠要去掉：`origin//api/v1/...` 会打到一条不同的路由上，
/// 返回 404，而那读起来像"这个网络不跑语音"。
pub fn endpoint_for(api_origin: &str) -> String {
    format!("{}/api/v1/voice/token", api_origin.trim_end_matches('/'))
}

/// 连接，并在票过期时换一张**再试一次**。
///
/// `cfg.token` 里原有的值会被覆盖：票只从 [`TokenSource`] 来。
pub async fn connect(mut cfg: Config, tokens: &TokenSource) -> Result<VoiceClient, Error> {
    cfg.token = tokens.fetch().await?;
    match VoiceClient::connect(cfg.clone()).await {
        Ok(c) => Ok(c),
        Err(e) if should_renew(&e, false) => {
            tracing::info!("the token had expired on arrival; fetching a fresh one");
            cfg.token = tokens.fetch().await?;
            Ok(VoiceClient::connect(cfg).await?)
        }
        Err(e) => Err(Error::Voice(e)),
    }
}

/// 这次失败该不该换张票再试。
///
/// **只有 `token_expired`，而且只试一次。**
///
/// 换回来的票还是过期的，说明问题不在票上而在**时钟**上——can-api 签的是 60 秒的
/// 票，而一端的时钟偏了更多。那时再换只是把同一件事重演，而换票要走 can-api 的
/// 鉴权路由，它和 FSD 登录**共用同一个按 CAN ID 的限流桶**：循环换票会把这个账号
/// 连 FSD 一起锁出去。
pub fn should_renew(err: &ClientError, already_retried: bool) -> bool {
    if already_retried {
        return false;
    }
    matches!(err, ClientError::Conn(ConnError::Refused(r)) if r.is_recoverable())
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_voice_client::client::Error as ClientError;
    use can_voice_client::conn::{Error as ConnError, RefusedReason};

    /// 界面上的话按原因分开说（#29）：版本太旧的人该去更新，打错密码的人该重打
    /// 一遍，而这两种人都不该看到一句笼统的"连不上"。
    #[test]
    fn each_failure_tells_the_member_what_to_do_about_it() {
        let refused = |r| Error::Voice(ClientError::Conn(ConnError::Refused(r)));
        assert_eq!(Error::Credentials.message().key, "error.token.credentials");
        assert_eq!(
            refused(RefusedReason::ProtoUnsupported).message().key,
            "error.voice.proto_unsupported"
        );
        assert_eq!(
            refused(RefusedReason::TokenExpired).message().key,
            "error.voice.token_expired"
        );
        let other = refused(RefusedReason::Other("maintenance".into())).message();
        assert_eq!(other.key, "error.voice.refused_other");
        assert_eq!(other.values["reason"], "maintenance");
        let status = Error::Rejected(reqwest::StatusCode::BAD_GATEWAY).message();
        assert_eq!(status.key, "error.token.rejected");
        assert_eq!(status.values["status"], "502");
    }

    #[test]
    fn scope_refusal_is_not_a_password_error() {
        assert!(matches!(
            classify(
                reqwest::StatusCode::FORBIDDEN,
                r#"{"error":"voice_scope_refused"}"#
            ),
            Error::ScopePending
        ));
        assert!(matches!(
            classify(
                reqwest::StatusCode::SERVICE_UNAVAILABLE,
                r#"{"error":"voice_authority_unavailable"}"#
            ),
            Error::AuthorityUnavailable
        ));
    }

    fn refused(r: RefusedReason) -> ClientError {
        ClientError::Conn(ConnError::Refused(r))
    }

    // ——— 端点怎么拼 ———

    #[test]
    fn the_endpoint_is_built_from_the_api_origin() {
        assert_eq!(
            endpoint_for("https://api.ceruleanavi.net"),
            "https://api.ceruleanavi.net/api/v1/voice/token"
        );
    }

    /// 结尾那个斜杠不该变成一个双斜杠的地址——它会打到一条不同的路由上，
    /// 而返回的是 404，读起来像"这个网络不跑语音"。
    #[test]
    fn a_trailing_slash_does_not_become_a_double_slash() {
        assert_eq!(
            endpoint_for("https://api.ceruleanavi.net/"),
            "https://api.ceruleanavi.net/api/v1/voice/token"
        );
    }

    // ——— 换票重试的策略 ———

    /// **`token_expired` 是唯一可恢复的一条。** 票本身没问题，只是过期了；
    /// 去换一张新的再连一次就好，而不是走重连策略——重连带的是同一张过期的票。
    #[test]
    fn an_expired_token_is_worth_one_more_try() {
        assert!(should_renew(&refused(RefusedReason::TokenExpired), false));
    }

    /// **只试一次。**
    ///
    /// 换回来的票还是过期的，说明问题不在票上而在**时钟**上：can-api 签的是
    /// 60 秒的票，而一端的时钟偏了更多。那时再换只是把同一件事重演，
    /// 而换票要走 can-api 的鉴权路由——它和 FSD 登录**共用同一个按 CAN ID 的限流桶**，
    /// 循环换票会把这个账号连 FSD 一起锁出去。
    #[test]
    fn a_second_expiry_is_a_clock_problem_not_a_token_problem() {
        assert!(!should_renew(&refused(RefusedReason::TokenExpired), true));
    }

    #[test]
    fn the_unrecoverable_refusals_are_not_retried() {
        for r in [
            RefusedReason::TokenInvalid,
            RefusedReason::Refused,
            RefusedReason::ProtoUnsupported,
            RefusedReason::Other("evicted".into()),
        ] {
            assert!(
                !should_renew(&refused(r.clone()), false),
                "{r:?} must not be retried"
            );
        }
    }

    /// 地址解析不了换多少张票都没用。
    #[test]
    fn a_bad_address_is_not_a_token_problem() {
        assert!(!should_renew(
            &ClientError::BadAddress("nope".into()),
            false
        ));
    }

    /// 普通的连接失败也不是：换票走的是另一个服务，而它没坏。
    #[test]
    fn an_ordinary_connection_failure_is_not_a_token_problem() {
        let e = ClientError::Conn(ConnError::Io(std::io::Error::other("refused")));
        assert!(!should_renew(&e, false));
    }

    // ——— 换票失败要说对是哪一种失败 ———

    /// 起一个只答一次的 HTTP 服务，返回它的 origin。
    ///
    /// 手写而不是拉一个 mock 库：要断言的是"状态码归到哪一类错误"，
    /// 而那只需要一条真的 HTTP 响应。
    async fn serve_once(status_line: &'static str, body: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            // 请求体不关心，读一轮把它从缓冲里拿走就行。
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 {status_line}\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
        format!("http://{addr}")
    }

    /// **密码打错不是网络不通。** 两者以前是同一句 `could not reach can-api`,
    /// 于是一个打错密码的人被送去查网络。
    #[tokio::test]
    async fn wrong_credentials_are_not_reported_as_a_network_problem() {
        let origin = serve_once("401 Unauthorized", r#"{"error":"bad credentials"}"#).await;
        let src = TokenSource::new(&origin, "1001", "wrong", reqwest::Client::new());

        let err = src.fetch().await.expect_err("401 must not be a success");
        let msg = err.to_string();

        assert!(
            !msg.contains("could not reach"),
            "401 报成了网络问题: {msg}"
        );
        assert!(msg.contains("password"), "401 该说到密码: {msg}");
    }

    /// **未定级不是密码不对。** can-api 给 rating < 1 的成员专门回 403
    /// `insufficient_rating`（can-api `internal/api/voicetoken.go`）。归到凭据那一类
    /// 的话，一个密码完全正确的人会被送去改密码，而每重试一次，can-api 按 CAN ID
    /// 的限流就多记一格——旧版 can-audio 把 "rating >= 1" 单列为登录条件之一，
    /// `test_mumble.py` 还专门断言提示里要出现 rating，注释写着"这条最容易漏"。
    #[tokio::test]
    async fn an_unrated_account_is_not_reported_as_a_wrong_password() {
        let origin = serve_once(
            "403 Forbidden",
            r#"{"error":"insufficient_rating","message":"That account is not permitted to use voice."}"#,
        )
        .await;
        let src = TokenSource::new(&origin, "1001", "right", reqwest::Client::new());

        let err = src.fetch().await.expect_err("403 must not be a success");
        assert!(matches!(err, Error::NotRated), "{err:?}");

        let msg = err.to_string();
        assert!(!msg.contains("password"), "未定级归到了密码上: {msg}");
        assert!(msg.contains("rating"), "这一句该说到 rating: {msg}");
        assert_eq!(err.message().key, "error.token.not_rated");
    }

    /// 认不出错误码的 403 照旧当凭据问题：能说的只有"没通过"。
    #[tokio::test]
    async fn a_forbidden_without_that_code_is_still_a_credential_problem() {
        let origin = serve_once("403 Forbidden", r#"{"error":"forbidden"}"#).await;
        let src = TokenSource::new(&origin, "1001", "wrong", reqwest::Client::new());

        let err = src.fetch().await.expect_err("403 must not be a success");
        assert!(matches!(err, Error::Credentials), "{err:?}");
    }

    // ——— 密码不许进日志 ———

    /// **`Debug` 里没有密码。**
    ///
    /// 目前没有一处用 `{:?}` 打印它，但以后任何一句调试日志或者一条 panic 信息
    /// 都会把密码写进日志：机队的进容器日志，桌面端的进那个可以一键回传的日志
    /// 文件。旧版 `can-audio/server/login.py` 不打印密码，`test_login.py` 守着
    /// 这一条。CID 留着——它不是秘密，而看日志的人要靠它认出是哪个账号。
    #[test]
    fn debug_shows_the_account_but_never_the_password() {
        let src = TokenSource::new(
            "https://api.example",
            "1001",
            "correct-horse-battery-staple",
            reqwest::Client::new(),
        );
        let shown = format!("{src:?}");
        assert!(!shown.contains("correct-horse"), "密码进了 Debug: {shown}");
        assert!(shown.contains("***"), "{shown}");
        assert!(shown.contains("1001"), "CID 该留着: {shown}");
    }

    #[test]
    fn scoped_request_has_only_the_selected_authority() {
        let src = TokenSource::new(
            "https://api.example",
            "1001",
            "private-password",
            reqwest::Client::new(),
        )
        .with_scope(TokenScope::controller("ZSPD_TWR", 118_500));
        assert_eq!(
            src.request_body(),
            serde_json::json!({
                "cid": "1001", "password": "private-password", "role": "controller",
                "callsign": "ZSPD_TWR", "frequency": 118_500
            })
        );
        assert!(!format!("{src:?}").contains("private-password"));
    }

    #[test]
    fn observer_without_a_frequency_still_requests_observer_identity() {
        let src = TokenSource::new(
            "https://api.example",
            "1001",
            "secret",
            reqwest::Client::new(),
        )
        .with_scope(TokenScope::observer("OBS123", 0));
        let body = src.request_body();
        assert_eq!(body["role"], "observer");
        assert_eq!(body["callsign"], "OBS123");
        assert_eq!(body["frequency"], 0);
    }

    /// 而 can-api 自己坏了要看得出来是它坏了，不要归到密码上。
    #[tokio::test]
    async fn a_server_error_is_not_blamed_on_the_password() {
        let origin = serve_once("500 Internal Server Error", "{}").await;
        let src = TokenSource::new(&origin, "1001", "right", reqwest::Client::new());

        let err = src.fetch().await.expect_err("500 must not be a success");
        let msg = err.to_string();

        assert!(!msg.contains("password"), "500 归到了密码上: {msg}");
        assert!(msg.contains("500"), "500 该把状态码说出来: {msg}");
    }

    // ——— 没有一条塞票进来的路 ———

    /// [`TokenSource`] 只有一个要凭据的构造函数。
    ///
    /// 这不是纪律，是类型层面的事实：能被塞一张长期票的东西，那张票就成了一个
    /// 没人管的凭据。ATIS 机队为此有一条扫源码的测试，这里是它的上游。
    #[test]
    fn there_is_no_way_in_but_credentials() {
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs"))
            .expect("read");
        let mut in_tests = false;
        for line in src.lines() {
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
            for banned in ["from_token", "with_token", "skip_auth", "bypass"] {
                assert!(!t.contains(banned), "{banned:?}: {t}");
            }
        }
    }
}
