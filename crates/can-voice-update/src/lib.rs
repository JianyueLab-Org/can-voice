//! 更新检查：问 can-api 有没有新版，**只报告，不动手**。
//!
//! 四条规矩照搬 `can-audio/*/update.py`，每一条都是踩出来的：
//!
//! - **走 can-api，不走 GitHub。** 这不是偏好而是连通性：从大陆拉一个 60 MB 的
//!   GitHub 资产经常卡死，而 api.ceruleanavi.net 本来就通。
//! - **失败要安静。** 每一条错误路径都返回"没有更新"并记一条 INFO。
//!   连不上更新服务不值得一个对话框，更不该拖慢或者卡住启动。
//! - **只有能就地替换自己的包才自动更新。** AppImage、NSIS、MSI 可以；deb 和 rpm
//!   要提权，而这套更新不问人，所以它们退回横幅提示。判据是
//!   `self_replaceable`，取值来自 `tauri_utils::platform::bundle_type()`——
//!   和插件 `install_inner` 用的是同一个，两处判据不同就会出现「我们以为是
//!   AppImage 而插件以为不是」。
//! - **不要打断正在工作的人**，而且**记住被跳过的版本**。
//!
//! # 清单是平的，而且问的是 can-voice 那条路
//!
//! 这个模块问的是 `/api/v1/voice/update/{client}/{target}/{arch}/{bundle}`。
//! 路径里已经写明了是哪一个平台，所以回包里没有可以用来分支的东西：
//! `{version, notes, pub_date, url, signature}`，就是 tauri-plugin-updater
//! 读的那一份。
//!
//! 它此前问的是 `/api/v1/clients/latest`，而那条路解析的是 **can-audio** 的
//! 发行——一个 can-voice 客户端拿 `27.0.3` 去比 can-audio 的 `2.x`，结论永远
//! 是"没有更新"，而它看起来和"确实没有更新"一模一样。
//!
//! "没什么可装的"一律是 **204**：can-api 那边每一种普通的落空都答 204，
//! 而插件把 204 读成 `Ok(None)`、把其它非 2xx 读成要重试的错误。

use can_voice_i18n::Message;
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

/// 这一种包能不能就地把自己换掉。
///
/// 参数是 `tauri_utils::platform::bundle_type()` 的名字，插件也是照它分支的。
/// deb 和 rpm 插件装得了，这里故意不放行：`dpkg -i` 和 `rpm -U` 要走 pkexec
/// 或者图形 sudo，和"不问人"相冲；而 `/usr` 是包管理器的地盘，下一次
/// `apt upgrade` 会把版本退回去。
pub fn self_replaceable(bundle: Option<&str>) -> bool {
    matches!(bundle, Some("appimage") | Some("nsis") | Some("msi"))
}

/// 当前这一份构建该问哪个地址。
pub fn manifest_url(
    api_origin: &str,
    client: &str,
    target: &str,
    arch: &str,
    bundle: &str,
    current: &str,
) -> String {
    format!(
        "{}/api/v1/voice/update/{client}/{target}/{arch}/{bundle}?current={current}",
        api_origin.trim_end_matches('/')
    )
}

/// 读 can-api 发的清单。平的，因为请求里已经写明了一个平台：
/// `{version, notes, pub_date, url, signature}`。
///
/// 任何一步对不上都返回 `None`——"没有更新"是这条路径上唯一安全的默认值。
pub fn parse(body: &Value) -> Option<Latest> {
    let version = body.get("version")?.as_str()?.trim_start_matches('v');
    let download = body.get("url")?.as_str()?;
    if version.is_empty() || download.is_empty() {
        return None;
    }
    Some(Latest {
        version: version.to_string(),
        download: download.to_string(),
        notes: body
            .get("notes")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        // 清单里没有大小。横幅把 0 显示成"0 MB"，所以这一位是 0 的时候
        // 调用方用不带大小的那句文案。
        size: 0,
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
    target: &str,
    arch: &str,
    bundle: &str,
    version: &str,
) -> Option<Latest> {
    let url = manifest_url(api_origin, client, target, arch, bundle, version);
    let resp = match http
        .get(&url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(TIMEOUT)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::info!(error = %e, "update check unreachable; assuming no update");
            return None;
        }
    };

    // 204 是"没什么可装的"，不是错误：can-api 那边每一种普通的落空都是它。
    if resp.status() == reqwest::StatusCode::NO_CONTENT {
        return None;
    }
    if !resp.status().is_success() {
        tracing::info!(status = %resp.status(), "update check refused; assuming no update");
        return None;
    }

    let body: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::info!(error = %e, "update check returned something that is not JSON");
            return None;
        }
    };
    parse(&body)
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
pub fn open_in_browser(url: &str) -> Result<(), Message> {
    if !is_openable(url) {
        return Err(Message::new("error.update.not_https"));
    }
    let (program, args) =
        opener(std::env::consts::OS).ok_or_else(|| Message::new("error.update.no_opener"))?;
    std::process::Command::new(program)
        .args(args)
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| Message::new("error.update.open_failed").with("detail", e))
}

/// 用系统默认的文件管理器打开一个目录。
///
/// **不收字符串，只收 `&Path`。** `open_in_browser` 只放行 https，注释写明的
/// 理由是这一步在把外部数据交给操作系统；一个能从前端传任意路径进来的命令会把
/// 那条理由作废。调用方自己算路径，这个签名让"从网页那一侧传一个路径进来"
/// 写不出来。
pub fn open_folder(path: &std::path::Path) -> Result<(), Message> {
    let (program, args) =
        opener(std::env::consts::OS).ok_or_else(|| Message::new("error.update.no_opener"))?;
    std::process::Command::new(program)
        .args(args)
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| Message::new("error.update.open_failed").with("detail", e))
}

/// 自带 HTTP 客户端的版本，给手上没有现成 client 的调用方用。
///
/// 通播制作客户端就是这一种：它本来不需要 reqwest，而"为查一次更新拉一个
/// HTTP 栈的依赖"和当初把 `can-voice-settings` 拆出来要躲的是同一件事。
pub async fn check_once(
    api_origin: &str,
    client: &str,
    target: &str,
    arch: &str,
    bundle: &str,
    version: &str,
) -> Option<Latest> {
    check(
        &reqwest::Client::new(),
        api_origin,
        client,
        target,
        arch,
        bundle,
        version,
    )
    .await
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

    /// can-api 发的更新清单。**平的**：请求里已经写死了一个平台，
    /// 所以回包里没有可以用来分支的东西。
    fn manifest(latest: &str) -> serde_json::Value {
        json!({
            "version": latest,
            "notes": "https://github.com/…/releases/tag/x",
            "pub_date": "2026-09-22T00:00:00Z",
            "url": "https://api.ceruleanavi.net/api/v1/voice/download/audio-for-can/linux-x86_64-deb",
            "signature": "dW50cnVzdGVkIGNvbW1lbnQ6…",
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
        let (origin, req) = serve_once("200 OK", manifest("27.1.0").to_string()).await;

        let got = check(
            &reqwest::Client::new(),
            &origin,
            "audio-for-can",
            "linux",
            "x86_64",
            "deb",
            "27.0.3",
        )
        .await
        .expect("a newer build must be reported");

        assert_eq!(got.version, "27.1.0");
        assert!(got.download.contains("voice/download"), "{}", got.download);

        // 问的是 can-api 那条新路径，而且平台的每一段都在地址里：少了任何一段，
        // 一个 deb 和一个 AppImage 拿到的就是同一个清单。
        let line = req.await.expect("the server saw a request");
        assert!(
            line.contains("/api/v1/voice/update/audio-for-can/linux/x86_64/deb"),
            "{line}"
        );
        assert!(line.contains("current=27.0.3"), "{line}");
    }

    /// 自带客户端那条路也要真的走通——手上没有现成 client 的调用方走的是它。
    #[tokio::test]
    async fn the_self_contained_check_reaches_the_server_too() {
        let (origin, _req) = serve_once("200 OK", manifest("27.1.0").to_string()).await;
        let got = check_once(&origin, "audio-for-can", "linux", "x86_64", "deb", "27.0.3")
            .await
            .expect("a newer build must be reported");
        assert_eq!(got.version, "27.1.0");
    }

    /// can-api 的每一个"没什么可装的"都是 204。把它读成错误在这里无害，
    /// 但会把一个真的错误盖掉。
    #[tokio::test]
    async fn two_oh_four_is_not_an_update() {
        let (origin, _req) = serve_once("204 No Content", String::new()).await;
        let got = check(
            &reqwest::Client::new(),
            &origin,
            "audio-for-can",
            "linux",
            "x86_64",
            "deb",
            "27.0.3",
        )
        .await;
        assert!(got.is_none());
    }

    /// **失败要安静。** 更新服务出错不值得一个对话框，更不该拖慢启动。
    #[tokio::test]
    async fn a_server_error_is_not_an_update() {
        let (origin, _req) = serve_once("500 Internal Server Error", "nope".into()).await;
        assert_eq!(
            check(
                &reqwest::Client::new(),
                &origin,
                "audio-for-can",
                "linux",
                "x86_64",
                "deb",
                "27.0.3",
            )
            .await,
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
                "linux",
                "x86_64",
                "deb",
                "27.0.3",
            )
            .await,
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

    // ——— 清单是平的 ———

    /// 只有能就地替换自己的包才自动更新；deb 和 rpm 退回横幅。
    #[test]
    fn only_a_bundle_that_can_replace_itself_updates_itself() {
        // deb 和 rpm 插件装得了，但装它们要 pkexec 或者图形 sudo，而这套更新
        // 不问人。`/usr` 还是包管理器的地盘，下一次 `apt upgrade` 会把旧版本
        // 装回来。
        assert!(self_replaceable(Some("appimage")));
        assert!(self_replaceable(Some("nsis")));
        assert!(self_replaceable(Some("msi")));

        assert!(!self_replaceable(Some("deb")));
        assert!(!self_replaceable(Some("rpm")));
        assert!(!self_replaceable(Some("app")));

        // 没打包的构建——`cargo run`。没有东西可以替换。
        assert!(!self_replaceable(None));
        assert!(!self_replaceable(Some("unknown")));
    }

    #[test]
    fn the_manifest_url_carries_every_part_the_route_matches_on() {
        let url = manifest_url(
            "https://api.ceruleanavi.net/",
            "audio-for-can",
            "linux",
            "x86_64",
            "deb",
            "27.0.3",
        );
        assert_eq!(
            url,
            "https://api.ceruleanavi.net/api/v1/voice/update/audio-for-can/linux/x86_64/deb?current=27.0.3"
        );
    }

    /// 旧回包把每个构建塞在 `clients[<name>]` 底下。新的只有一个平台，
    /// 因为请求里已经写明了是哪一个。
    #[test]
    fn the_manifest_is_flat_not_keyed_by_client() {
        let body = json!({
            "version": "27.0.9",
            "notes": "有新版",
            "pub_date": "2026-09-22T00:00:00Z",
            "url": "https://api.ceruleanavi.net/api/v1/voice/download/audio-for-can/linux-x86_64-deb",
            "signature": "sig",
        });
        let latest = parse(&body).expect("a manifest is an update");
        assert_eq!(latest.version, "27.0.9");
        assert_eq!(
            latest.download,
            "https://api.ceruleanavi.net/api/v1/voice/download/audio-for-can/linux-x86_64-deb"
        );
        assert_eq!(latest.notes, "有新版");
    }

    #[test]
    fn a_manifest_without_a_url_is_not_an_update() {
        let body = json!({ "version": "27.0.9" });
        assert!(parse(&body).is_none());
    }

    #[test]
    fn a_malformed_reply_is_not_an_update() {
        assert!(parse(&json!("nonsense")).is_none());
        assert!(parse(&json!({})).is_none());
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
