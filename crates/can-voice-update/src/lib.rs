//! 更新检查：问 can-api 有没有新版，**只报告，不动手**。
//!
//! 四条规矩照搬 `can-audio/*/update.py`，每一条都是踩出来的：
//!
//! - **走 can-api，不走 GitHub。** 这不是偏好而是连通性：从大陆拉一个 60 MB 的
//!   GitHub 资产经常卡死，而 api.ceruleanavi.net 本来就通。
//! - **失败要安静。** 每一条错误路径都返回"没有更新"并记一条 INFO。
//!   连不上更新服务不值得一个对话框，更不该拖慢或者卡住启动。
//! - **绝不自动更新。** 这里只**报告**，下不下载是人决定的——一个正在值班的
//!   管制员不需要一个自作主张重启自己的程序。
//! - **不要打断正在工作的人**，而且**记住被跳过的版本**。
//!
//! # 回包有两层，而 Python 版两层都读错了
//!
//! `update.py` 判的是一个顶层的 `update_available`，而 can-api 从来没发过那个
//! 字段——真的在 `update.available` 里。于是检查**永远**返回"没有更新"，
//! 四个客户端都一样，而它看起来和"确实没有更新"一模一样。
//!
//! 后面还埋着第二个：顶层的 `client` 是**包名字符串**，每个构建在
//! `clients[<name>]` 里。只修第一层的话，下一步就是在一个 `String` 上调 `.get()`。

use serde_json::Value;

/// 一个可下载的新版本。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Latest {
    pub version: String,
    /// 下载地址。**指向 can-api 的中转**，不是 GitHub。
    pub download: String,
    pub notes: String,
    pub size: u64,
}

/// 从 `/api/v1/clients/latest?client=…&version=…` 的回包里取出该提示的版本。
///
/// 任何一步对不上都返回 `None`——"没有更新"是这条路径上唯一安全的默认值。
pub fn parse(body: &Value, client: &str) -> Option<Latest> {
    // 第一层：结论在 `update.available` 里，不在顶层。
    if !body.get("update")?.get("available")?.as_bool()? {
        return None;
    }
    // 第二层：构建在 `clients[<name>]` 里；顶层的 `client` 只是包名。
    //
    // 四个构建里坏了一个时 can-api **不把它列出来**，而不是给一个会 404 的
    // 地址。所以这里取不到就是"没有更新"。
    let build = body.get("clients")?.get(client)?;
    Some(Latest {
        version: build.get("version")?.as_str()?.to_string(),
        download: build.get("download")?.as_str()?.to_string(),
        notes: body
            .get("notes")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        size: build
            .get("size")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    })
}

/// 查更新要等多久。
///
/// **有上界，而且短。** 这条请求在启动路径上，超时不设的话一个连得上但不回话
/// 的中间设备能让客户端一直"正在启动"。查不到新版是小事。
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// can-api 前面挡着 Cloudflare。实测 `/api/*` 不会被挑战（HTML 页面会），
/// 但 UA 照着 can-audio 已有的写法来——那一行是踩出来的。
const USER_AGENT: &str = "Mozilla/5.0 (compatible; CanClient/1.0)";

/// 去问一次 can-api 有没有新版。
///
/// `client` 是产品名（`audio-for-can` / `atis-for-can` / `xpc-for-can` /
/// `msfs-for-can`，can-api 那边是一张固定白名单），`version` 是本机这一版。
///
/// **任何一步出错都返回 `None` 并记一条 INFO。** 连不上更新服务不值得一个
/// 对话框，更不该拖慢或者卡住启动——这是这个模块四条规矩里的第二条。
pub async fn check(
    http: &reqwest::Client,
    api_origin: &str,
    client: &str,
    version: &str,
) -> Option<Latest> {
    let url = format!("{}/api/v1/clients/latest", api_origin.trim_end_matches('/'));
    let resp = match http
        .get(&url)
        .query(&[("client", client), ("version", version)])
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(TIMEOUT)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            tracing::info!(status = %r.status(), "update check refused; assuming no update");
            return None;
        }
        Err(e) => {
            tracing::info!(error = %e, "update check unreachable; assuming no update");
            return None;
        }
    };
    let body: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::info!(error = %e, "update check returned something that is not JSON");
            return None;
        }
    };
    let latest = parse(&body, client)?;
    // can-api 已经比过一次（`update.available`），这里再比一次是因为**两边的
    // 比较必须一致**：服务端说有、而本机版本号其实更新（本地构建、回滚过），
    // 提示"请更新到一个更旧的版本"比不提示糟得多。
    is_newer(&latest.version, version).then_some(latest)
}

/// 这个平台用哪个命令开浏览器。
///
/// 拆成一个按名字查表的纯函数，是为了三个平台都能在**一台机器上**测到——
/// Windows 那条 `""` 少了就是"浏览器没反应"，而那种 bug 只有 Windows 用户碰得到。
fn opener(target_os: &str) -> Option<(&'static str, &'static [&'static str])> {
    match target_os {
        "macos" => Some(("open", &[])),
        "linux" => Some(("xdg-open", &[])),
        // `start` 把第一个带引号的参数当窗口标题，所以要先喂它一个空标题。
        "windows" => Some(("cmd", &["/C", "start", ""])),
        _ => None,
    }
}

/// 这个地址能不能交给系统去打开。
///
/// **只放行 https。** 它来自 can-api 回包里的一个字符串，而这一步是把外部数据
/// 交给操作系统——`file://` 和自定义协议处理器都不该从这里进去。
fn is_openable(url: &str) -> bool {
    url.starts_with("https://")
}

/// 用系统默认浏览器打开下载地址。
///
/// **绝不自动更新**（这个模块的第三条规矩）：这里只是把人送到下载页，
/// 装不装、什么时候装是他的事。
pub fn open_in_browser(url: &str) -> Result<(), String> {
    if !is_openable(url) {
        return Err("下载地址不是一个 https 地址，没有打开".into());
    }
    let (program, args) = opener(std::env::consts::OS).ok_or("不知道这个系统怎么开浏览器")?;
    std::process::Command::new(program)
        .args(args)
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打不开浏览器：{e}"))
}

/// 自带 HTTP 客户端的版本，给手上没有现成 client 的调用方用。
///
/// 通播制作客户端就是这一种：它本来不需要 reqwest，而"为查一次更新拉一个
/// HTTP 栈的依赖"和当初把 `can-voice-settings` 拆出来要躲的是同一件事。
pub async fn check_once(api_origin: &str, client: &str, version: &str) -> Option<Latest> {
    check(&reqwest::Client::new(), api_origin, client, version).await
}

/// `latest` 比 `current` 新吗。
///
/// **按数值比，不按字符串。** 按字符串比 `2.0.10` 排在 `2.0.9` 前面，
/// 于是客户端要么永远提示、要么永远不提示。can-api 两侧都是数值的
/// （`release.CompareVersions`），这边必须跟上。
///
/// 非数字一律剥掉：release tag 带 `v` 而客户端自报不带。
pub fn is_newer(latest: &str, current: &str) -> bool {
    let a = numbers(latest);
    let b = numbers(current);
    if a.is_empty() {
        return false;
    }
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

fn numbers(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// 现在该不该把这个新版本弹给用户。
///
/// - **`busy` 时不弹。** 管制员连着、ATIS 在播的时候，一个模态框盖在无线电台面上
///   比晚一次更新糟得多——而 PTT 是全局热键，弹窗抢了焦点就按不出去了。
///   那种时候在状态栏留一行。
/// - **跳过的那个版本不再问。** 每次启动都再问一遍，和自动更新一样烦、只是更频繁。
///   但跳过的是**那一个版本**，不是从此闭嘴。
pub fn should_prompt(latest: &str, current: &str, skipped: Option<&str>, busy: bool) -> bool {
    if busy || !is_newer(latest, current) {
        return false;
    }
    !matches!(skipped, Some(s) if numbers(s) == numbers(latest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reply(latest: &str, available: bool, with_build: bool) -> serde_json::Value {
        let mut clients = serde_json::Map::new();
        if with_build {
            clients.insert(
                "audio-for-can".into(),
                json!({
                    "name": "audio-for-can",
                    "version": latest,
                    "size": 62_000_000u64,
                    "download": "https://api.ceruleanavi.net/api/v1/clients/download/audio-for-can?v=x",
                    "origin": "https://github.com/…"
                }),
            );
        }
        json!({
            "version": latest,
            "notes": "https://github.com/…/releases/tag/x",
            "clients": clients,
            "client": "audio-for-can",
            "update": { "available": available, "current": "2.2.4", "latest": latest }
        })
    }

    // ——— 真的去问一次 ———

    /// 起一个只答一次的 HTTP 服务，返回 `(origin, 收到的请求行)`。
    ///
    /// 手写而不是拉一个 mock 库：要断言的是"问对了地址、带对了参数、
    /// 答案解得出来"，那只需要一条真的 HTTP 交互。
    async fn serve_once(
        status_line: &'static str,
        body: String,
    ) -> (String, tokio::sync::oneshot::Receiver<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 2048];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let _ = tx.send(req);
            let resp = format!(
                "HTTP/1.1 {status_line}\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
        (format!("http://{addr}"), rx)
    }

    /// 这条链路此前**根本不存在**：三个纯函数、二十条测试，没有一次 HTTP 调用，
    /// 四端零引用。于是旧版本用户永远不知道有新版，而 `proto_unsupported` 的
    /// "请更新客户端"指向一条走不通的路。
    #[tokio::test]
    async fn a_newer_build_comes_back_as_an_update() {
        let (origin, req) = serve_once("200 OK", reply("27.1.0", true, true).to_string()).await;

        let got = check(&reqwest::Client::new(), &origin, "audio-for-can", "27.0.3")
            .await
            .expect("a newer build must be reported");

        assert_eq!(got.version, "27.1.0");
        assert!(
            got.download.contains("clients/download"),
            "{}",
            got.download
        );

        // 问的是 can-api 那条已发布的路径，而且带上了自己是谁、现在是哪一版：
        // 少了 client 参数的话 can-api 不知道该拿哪个包的版本来比。
        let line = req.await.expect("the server saw a request");
        assert!(line.contains("/api/v1/clients/latest"), "{line}");
        assert!(line.contains("client=audio-for-can"), "{line}");
        assert!(line.contains("version=27.0.3"), "{line}");
    }

    /// 自带客户端那条路也要真的走通——四个客户端里有一个走的是它。
    #[tokio::test]
    async fn the_self_contained_check_reaches_the_server_too() {
        let (origin, _req) = serve_once("200 OK", reply("27.1.0", true, true).to_string()).await;
        let got = check_once(&origin, "audio-for-can", "27.0.3")
            .await
            .expect("a newer build must be reported");
        assert_eq!(got.version, "27.1.0");
    }

    /// **失败要安静。** 更新服务出错不值得一个对话框，更不该拖慢启动。
    #[tokio::test]
    async fn a_server_error_is_not_an_update() {
        let (origin, _req) = serve_once("500 Internal Server Error", "nope".into()).await;
        assert_eq!(
            check(&reqwest::Client::new(), &origin, "audio-for-can", "27.0.3").await,
            None
        );
    }

    /// 连不上同理——这是大陆网络上最常见的一种，而它绝不能变成一句报错。
    #[tokio::test]
    async fn an_unreachable_server_is_not_an_update() {
        assert_eq!(
            check(
                &reqwest::Client::new(),
                "http://127.0.0.1:1",
                "audio-for-can",
                "27.0.3",
            )
            .await,
            None
        );
    }

    /// 同一版不提示。回包里 `update.available` 为 false 时更不提示——
    /// 那是 can-api 已经比过一次的结论。
    #[tokio::test]
    async fn the_same_version_is_not_an_update() {
        let (origin, _req) = serve_once("200 OK", reply("27.0.3", false, true).to_string()).await;
        assert_eq!(
            check(&reqwest::Client::new(), &origin, "audio-for-can", "27.0.3").await,
            None
        );
    }

    // ——— 下载靠系统浏览器，不靠这个程序 ———

    /// 三个平台各有各的开法。Windows 那条 `""` 不是笔误：`start` 把第一个带引号
    /// 的参数当**窗口标题**，少了它地址就成了标题，浏览器一个字都收不到。
    #[test]
    fn each_platform_gets_its_own_opener() {
        assert_eq!(opener("macos"), Some(("open", &[][..])));
        assert_eq!(opener("linux"), Some(("xdg-open", &[][..])));
        assert_eq!(opener("windows"), Some(("cmd", &["/C", "start", ""][..])));
        assert_eq!(opener("plan9"), None);
    }

    /// **只开 https。** 下载地址是 can-api 回包里的一个字符串——一个被改过的
    /// 回包不该能让客户端去打开一个本地文件或者一个自定义协议处理器。
    #[test]
    fn only_an_https_url_is_opened() {
        assert!(is_openable(
            "https://api.ceruleanavi.net/api/v1/clients/download/xpc-for-can"
        ));
        assert!(!is_openable("http://api.ceruleanavi.net/x"));
        assert!(!is_openable("file:///etc/passwd"));
        assert!(!is_openable("javascript:alert(1)"));
        assert!(!is_openable(""));
    }

    // ——— 版本比较是数值的，不是字符串的 ———

    /// **按字符串比，`2.0.10` 排在 `2.0.9` 前面。** 那会让客户端要么永远提示、
    /// 要么永远不提示——can-api 两侧都是数值比较，这边必须跟上。
    #[test]
    fn ten_is_newer_than_nine() {
        assert!(is_newer("2.0.10", "2.0.9"));
        assert!(!is_newer("2.0.9", "2.0.10"));
    }

    /// tag 带 `v` 而客户端自报不带。两边都把非数字剥掉再比。
    #[test]
    fn a_leading_v_does_not_make_it_a_different_version() {
        assert!(!is_newer("v2.2.4", "2.2.4"));
        assert!(is_newer("v2.2.5", "2.2.4"));
    }

    #[test]
    fn a_shorter_version_is_not_automatically_older() {
        assert!(!is_newer("2.2", "2.2.0"));
        assert!(is_newer("2.3", "2.2.9"));
    }

    #[test]
    fn nonsense_is_not_newer_than_anything() {
        assert!(!is_newer("", "2.2.4"));
        assert!(!is_newer("不是版本号", "2.2.4"));
    }

    // ——— 回包有两层，而这正是 Python 版读错的地方 ———

    /// `update.py` 判的是一个**顶层的 `update_available`**，而 can-api 从来没有
    /// 发过那个字段——真的在 `update.available` 里。于是检查永远返回"没有更新"，
    /// 而它看起来和"确实没有更新"一模一样。
    #[test]
    fn the_verdict_lives_under_update_not_at_the_top() {
        let r = reply("2.2.5", true, true);
        assert!(
            r.get("update_available").is_none(),
            "the fixture must match reality"
        );
        assert!(parse(&r, "audio-for-can").is_some());
    }

    /// 另一半：顶层的 `client` 是**包名字符串**，每个构建在 `clients[<name>]` 里。
    /// 只修第一层的话，下一步就是在一个 `String` 上调 `.get()`。
    #[test]
    fn the_build_lives_under_clients_keyed_by_name() {
        let r = reply("2.2.5", true, true);
        assert!(r["client"].is_string(), "the fixture must match reality");
        let got = parse(&r, "audio-for-can").expect("a build");
        assert_eq!(got.version, "2.2.5");
        assert!(got.download.contains("/clients/download/audio-for-can"));
        assert_eq!(got.size, 62_000_000);
    }

    #[test]
    fn no_update_available_means_nothing_to_offer() {
        assert!(parse(&reply("2.2.4", false, true), "audio-for-can").is_none());
    }

    /// 四个构建里坏了一个时 can-api **不把它列出来**，而不是给一个会 404 的地址。
    /// 这边照样要当成"没有更新"。
    #[test]
    fn a_missing_build_is_not_an_update() {
        assert!(parse(&reply("2.2.5", true, false), "audio-for-can").is_none());
    }

    #[test]
    fn a_malformed_reply_is_not_an_update() {
        assert!(parse(&json!("nonsense"), "audio-for-can").is_none());
        assert!(parse(&json!({}), "audio-for-can").is_none());
    }

    // ——— 什么时候才弹 ———

    #[test]
    fn a_newer_version_is_offered() {
        assert!(should_prompt("2.2.5", "2.2.4", None, false));
    }

    /// **不要打断正在工作的人。** 管制员连着的时候、ATIS 在播的时候，
    /// 一个模态框盖在无线电台面上比晚一次更新糟得多——而 PTT 是全局热键。
    #[test]
    fn nobody_is_interrupted_while_working() {
        assert!(!should_prompt("2.2.5", "2.2.4", None, true));
    }

    /// **记住被跳过的版本。** 每次启动都再问一遍，和自动更新一样烦，只是更频繁。
    #[test]
    fn a_skipped_version_is_not_asked_about_again() {
        assert!(!should_prompt("2.2.5", "2.2.4", Some("2.2.5"), false));
    }

    /// 但跳过的是**那一个版本**，不是从此不再提示。
    #[test]
    fn skipping_one_version_does_not_silence_the_next() {
        assert!(should_prompt("2.2.6", "2.2.4", Some("2.2.5"), false));
    }

    #[test]
    fn the_current_version_is_never_offered() {
        assert!(!should_prompt("2.2.4", "2.2.4", None, false));
        assert!(!should_prompt("2.2.3", "2.2.4", None, false));
    }
}
