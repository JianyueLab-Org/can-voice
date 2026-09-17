//! 取 METAR 电码：HTTP 气象源。
//!
//! # 为什么非要有它
//!
//! 桌面那一支问报文走的是 FSD 的 `$AX`，而那条路要求**已经上线**。于是"先起
//! 客户端把稿子写好，再上线播"这件事做不成：没连 FSD 就一份真实报文都取不到,
//! 只能对着一份编出来的电码写模板。
//!
//! 照着 `can-audio/atis/weather.py` 移植。默认地址也是 can-fsd 自己在用的那个
//! （它 `config.json` 的 weather 项），所以和网络上其它地方看到的天气是同一份。

use can_voice_i18n::Message;
use std::time::Duration;

/// 默认气象源。ICAO 直接接在后面。
pub const DEFAULT_URL: &str = "https://metar.vatsim.net/metar.php?id=";

/// 这里**不需要**伪装成浏览器。
///
/// datafeed 那一侧前面挡着 Cloudflare，所以 `can_voice_datafeed::USER_AGENT`
/// 带着 `Mozilla/5.0` 前缀；气象源不挡，照实说自己是谁就好。两处不一样是有
/// 理由的，不是漏改。
pub const USER_AGENT: &str = "can-atis";

/// 默认重试次数。
///
/// 气象源在 CDN 后面，偶发一次连接/TLS 抖动是常事。一次抖动不该让这个席位
/// 整整一个刷新周期（默认 60 秒）都没有天气。
pub const RETRIES: u32 = 1;

/// 两次尝试之间等多久。
const RETRY_DELAY: Duration = Duration::from_secs(1);

/// `Display` 是给日志的英文；界面上的那一句走 [`WeatherError::message`]（#29）。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WeatherError {
    #[error("{0} is not a four-letter ICAO code")]
    NotIcao(String),
    #[error("fetching the METAR for {icao} failed: {detail}")]
    Unreachable { icao: String, detail: String },
    /// 证书校验没过。和 `Unreachable` 分开，因为该查的地方不一样，见 [`unreachable`]。
    #[error("fetching the METAR for {icao} failed: {detail} (certificate verification)")]
    Certificate { icao: String, detail: String },
    #[error("the weather source has no report for {0}")]
    Missing(String),
}

impl WeatherError {
    /// 给人看的那一句。
    pub fn message(&self) -> Message {
        match self {
            WeatherError::NotIcao(code) => {
                Message::new("problem.weather.not_icao").with("code", code)
            }
            WeatherError::Unreachable { icao, detail } => {
                Message::new("problem.weather.unreachable")
                    .with("icao", icao)
                    .with("detail", detail)
            }
            WeatherError::Certificate { icao, detail } => {
                Message::new("problem.weather.certificate")
                    .with("icao", icao)
                    .with("detail", detail)
            }
            WeatherError::Missing(icao) => {
                Message::new("problem.weather.missing").with("icao", icao)
            }
        }
    }
}

/// 四位字母才是 ICAO 代码。
///
/// 数字不算：`ZS9D` 这种写法在气象源上永远查不到，而**先挡下来**比让用户等
/// 两次重试再看一条网络错误要好得多——那条错误会把他引去查网络。
pub fn is_icao(code: &str) -> bool {
    let code = code.trim();
    code.len() == 4 && code.chars().all(|c| c.is_ascii_alphabetic())
}

/// 去掉 `METAR` / `SPECI` 前缀，确认这确实是这个机场的报文。
///
/// **答错机场比答不出来更危险**：那份报文会被当成本场的播出去，而界面上
/// 一切正常。
pub fn normalize(line: &str, icao: &str) -> Option<String> {
    let mut line = line.trim();
    for prefix in ["METAR ", "SPECI "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            line = rest.trim();
        }
    }
    line.starts_with(&icao.trim().to_uppercase())
        .then(|| line.to_string())
}

/// 从一份回文里挑出这个机场的报文。
pub fn pick(body: &str, icao: &str) -> Option<String> {
    body.lines().find_map(|line| normalize(line, icao))
}

/// 把底层错误归到能照着查的那一种。
///
/// 证书校验失败尤其容易被误读成"服务器坏了"——实测遇到过一次
/// `certificate has expired`，而服务器证书本身好好的，几分钟后自己就恢复了。
/// 真要排查，能动的只有本机时间和系统根证书。**该查什么的那句话在字典里**
/// （`problem.weather.certificate`），这里只分出是哪一种；底层错误原样带着。
pub fn unreachable(icao: String, detail: String) -> WeatherError {
    if detail.contains("CERTIFICATE_VERIFY_FAILED") || detail.contains("certificate") {
        WeatherError::Certificate { icao, detail }
    } else {
        WeatherError::Unreachable { icao, detail }
    }
}

/// 取一份原始 METAR 电码。
///
/// 网络和 TLS 层面的失败会重试 `retries` 次——ICAO 写错这种不会，那重试多少遍
/// 都是一样的结果。
pub async fn fetch(
    client: &reqwest::Client,
    base: &str,
    icao: &str,
    retries: u32,
) -> Result<String, WeatherError> {
    let icao = icao.trim().to_uppercase();
    if !is_icao(&icao) {
        return Err(WeatherError::NotIcao(icao));
    }
    let target = format!("{base}{icao}");

    let mut last = String::new();
    for attempt in 0..=retries {
        match client
            .get(&target)
            .header("User-Agent", USER_AGENT)
            .send()
            .await
        {
            Ok(response) => match response.text().await {
                Ok(body) => {
                    return pick(&body, &icao).ok_or(WeatherError::Missing(icao));
                }
                Err(e) => last = e.to_string(),
            },
            Err(e) => last = e.to_string(),
        }
        // 日志里留下地址：换过气象源之后，"取不到天气"到底是谁的问题，
        // 光看报错文字是分不出来的。
        tracing::warn!(
            icao = %icao, url = %target, attempt = attempt + 1, total = retries + 1,
            error = %last, "fetching the METAR failed"
        );
        if attempt < retries {
            tokio::time::sleep(RETRY_DELAY).await;
        }
    }
    Err(unreachable(icao, last))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_metar_prefix_is_not_part_of_the_report() {
        assert_eq!(
            normalize("METAR ZSPD 251300Z 09004MPS", "ZSPD").as_deref(),
            Some("ZSPD 251300Z 09004MPS")
        );
    }

    #[test]
    fn a_speci_prefix_is_stripped_the_same_way() {
        assert_eq!(
            normalize("SPECI ZSPD 251300Z 09004MPS", "ZSPD").as_deref(),
            Some("ZSPD 251300Z 09004MPS")
        );
    }

    /// 气象源答错机场比答不出来更危险：那份报文会被当成本场的播出去。
    #[test]
    fn a_line_for_another_airport_is_not_this_airports_report() {
        assert!(normalize("ZBAA 251300Z 09004MPS", "ZSPD").is_none());
    }

    #[test]
    fn the_first_line_that_is_this_airport_wins() {
        let body = "\n没有报文的一行\nZSPD 251300Z 09004MPS\nZSPD 251400Z 10005MPS\n";
        assert_eq!(pick(body, "ZSPD").as_deref(), Some("ZSPD 251300Z 09004MPS"));
    }

    #[test]
    fn a_body_that_never_mentions_this_airport_has_no_report() {
        assert!(pick("ZBAA 251300Z 09004MPS\n", "ZSPD").is_none());
    }

    #[test]
    fn four_letters_is_an_icao_code_and_three_is_not() {
        assert!(is_icao("zspd"));
        assert!(!is_icao("ZSP"));
        assert!(!is_icao("ZSPDX"));
        assert!(!is_icao("ZS9D"));
        assert!(!is_icao(""));
    }

    /// 证书校验失败最容易被读成"服务器坏了"，而能动的其实是本机。
    ///
    /// 该查什么的那句话在字典的 `problem.weather.certificate` 里；这里钉的是它被
    /// 认出来、而底层那句原样带到了界面上。
    #[test]
    fn a_certificate_failure_says_what_to_check() {
        let said = unreachable(
            "ZSPD".into(),
            "CERTIFICATE_VERIFY_FAILED: certificate has expired".into(),
        )
        .message();
        assert_eq!(said.key, "problem.weather.certificate");
        assert_eq!(
            said.values["detail"],
            "CERTIFICATE_VERIFY_FAILED: certificate has expired"
        );
    }

    #[test]
    fn an_ordinary_failure_is_passed_through_unchanged() {
        assert_eq!(
            unreachable("ZSPD".into(), "connection refused".into()),
            WeatherError::Unreachable {
                icao: "ZSPD".into(),
                detail: "connection refused".into()
            }
        );
    }

    // ——— 真的去问一次 ———

    /// 起一个 HTTP 服务。`drop_first` 为真时第一条连接接下就断，用来演一次抖动。
    ///
    /// 手写而不是拉一个 mock 库，理由和 `can-voice-update` 那份一样：要断言的是
    /// "问对了地址、带对了头、答案挑得出来"，那只需要一条真的 HTTP 交互。
    async fn serve(
        body: &'static str,
        status: &'static str,
        drop_first: bool,
    ) -> (String, tokio::sync::oneshot::Receiver<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            if drop_first {
                let (sock, _) = listener.accept().await.expect("accept");
                drop(sock);
            }
            let (mut sock, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 2048];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).to_string());
            let resp = format!(
                "HTTP/1.1 {status}\r\ncontent-type: text/plain\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
        (format!("http://{addr}/metar.php?id="), rx)
    }

    #[tokio::test]
    async fn fetching_asks_the_source_for_this_airport_and_says_who_it_is() {
        let (base, req) = serve("ZSPD 251300Z 09004MPS 9999 NOSIG", "200 OK", false).await;
        let client = reqwest::Client::new();
        let report = fetch(&client, &base, "zspd", 0).await.expect("report");
        assert_eq!(report, "ZSPD 251300Z 09004MPS 9999 NOSIG");

        let asked = req.await.expect("request");
        assert!(asked.contains("id=ZSPD"), "{asked}");
        assert!(asked.contains(USER_AGENT), "{asked}");
    }

    /// 一次抖动不该让这个席位整整一个刷新周期都没有天气。
    #[tokio::test]
    async fn one_shaky_connection_is_retried() {
        let (base, _req) = serve("ZSPD 251300Z 09004MPS", "200 OK", true).await;
        let client = reqwest::Client::new();
        assert_eq!(
            fetch(&client, &base, "ZSPD", 1).await.expect("report"),
            "ZSPD 251300Z 09004MPS"
        );
    }

    #[tokio::test]
    async fn a_source_that_has_no_report_for_this_airport_is_not_a_network_failure() {
        let (base, _req) = serve("", "200 OK", false).await;
        let client = reqwest::Client::new();
        assert_eq!(
            fetch(&client, &base, "ZSPD", 0).await,
            Err(WeatherError::Missing("ZSPD".into()))
        );
    }

    /// ICAO 写错重试多少遍都是一样的结果，所以一次都不问。
    #[tokio::test]
    async fn a_code_that_is_not_an_icao_is_refused_without_asking_anyone() {
        let client = reqwest::Client::new();
        assert_eq!(
            fetch(&client, "http://127.0.0.1:1/?id=", "ZSP", 3).await,
            Err(WeatherError::NotIcao("ZSP".into()))
        );
    }
}
