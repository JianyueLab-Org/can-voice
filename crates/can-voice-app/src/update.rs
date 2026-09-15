//! 更新检查。
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
