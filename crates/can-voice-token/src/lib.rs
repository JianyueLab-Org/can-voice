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

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not reach can-api: {0}")]
    Http(#[from] reqwest::Error),
    #[error("voice: {0}")]
    Voice(#[from] ClientError),
}

/// 换票的地方。
#[derive(Debug, Clone)]
pub struct TokenSource {
    endpoint: String,
    cid: String,
    password: String,
    http: reqwest::Client,
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
            http,
        }
    }

    /// 换一张票。
    ///
    /// **每次连接都现换。** can-api 签的是 60 秒的票，攒着没有意义——而票的短寿命
    /// 就是这套设计里唯一的吊销机制。
    pub async fn fetch(&self) -> Result<String, Error> {
        let reply: Reply = self
            .http
            .post(&self.endpoint)
            .json(&serde_json::json!({ "cid": self.cid, "password": self.password }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(reply.token)
    }
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
