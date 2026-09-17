//! 各服务的地址。
//!
//! # 为什么要能在界面上改
//!
//! 语音服务器、FSD、can-api、数据源的地址此前全是环境变量——一个装了 msi 的人
//! 没有地方改它们（#45）。测试服、临时切机房、排障时指到本机，都要改。
//!
//! # 谁说了算：环境变量 > 设置 > 默认
//!
//! 环境变量是给开发和排障用的，**不该被设置文件盖掉**：一个在命令行上指到本机
//! 起客户端的人，不该因为设置里存着线上地址就连到线上去。反过来，设置里留空就是
//! "用默认"，而不是"用空串"。
//!
//! 界面要能说出某一项此刻被环境变量盖着（[`fields`]）——否则改了没反应，
//! 看起来就是设置坏了。

use can_voice_i18n::Message;
use serde::{Deserialize, Serialize};

/// can-api。
pub const API_ORIGIN: &str = "https://api.ceruleanavi.net";
/// 语音服务器，`主机:端口`。
pub const VOICE_SERVER: &str = "audio.ceruleanavi.net:64738";
/// FSD，`主机:端口`。
pub const FSD_SERVER: &str = "fsd.ceruleanavi.net:6809";

/// 存下来的地址。**空串表示用默认。**
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Endpoints {
    #[serde(default)]
    pub api_origin: String,
    #[serde(default)]
    pub voice_server: String,
    #[serde(default)]
    pub fsd_server: String,
    #[serde(default)]
    pub datafeed_url: String,
    #[serde(default)]
    pub metar_url: String,
    #[serde(default)]
    pub atis_config_url: String,
}

/// 每一项被哪几个环境变量盖着。
const ENV_KEYS: &[(&str, &[&str])] = &[
    ("api_origin", &["CAN_API_ORIGIN"]),
    (
        "voice_server",
        &["CAN_VOICE_SERVER", "CAN_VOICE_SERVER_NAME"],
    ),
    ("fsd_server", &["CAN_FSD_HOST", "CAN_FSD_PORT"]),
    ("datafeed_url", &["CAN_FSD_DATAFEED"]),
    ("metar_url", &["CAN_METAR_URL"]),
    ("atis_config_url", &["CAN_ATIS_CONFIG_URL"]),
];

/// 读一个环境变量。**设成空串等于没设**——`CAN_API_ORIGIN= ./app` 这种写法
/// 常见于想"清掉"它的时候。
pub fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// 环境变量 > 设置 > 默认。
pub fn pick(env: Option<String>, saved: &str, default: &str) -> String {
    if let Some(v) = env {
        return v.trim().to_string();
    }
    match saved.trim() {
        "" => default.to_string(),
        saved => saved.to_string(),
    }
}

/// [`pick`]，环境变量从进程里读。
pub fn endpoint(key: &str, saved: &str, default: &str) -> String {
    pick(env(key), saved, default)
}

/// `主机:端口` → `(主机, 端口)`。没写端口、或端口不是数字，用 `default_port`。
///
/// 认 IPv6 的方括号写法（`[::1]:6809`）。
pub fn host_port(address: &str, default_port: u16) -> (String, u16) {
    let address = address.trim();
    let (host, port) = match address.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((host, tail)) => (host, tail.strip_prefix(':')),
            None => (rest, None),
        },
        None => match address.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (address, None),
        },
    };
    let port = port
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(default_port);
    (host.to_string(), port)
}

/// 语音服务器：`(地址, TLS 校验用的主机名)`。
///
/// 主机名默认取地址里的主机那一段；`CAN_VOICE_SERVER_NAME` 单独给的时候用它
/// ——指到一个 IP 上排障时，证书上写的还是域名。
pub fn voice_target(
    env_server: Option<String>,
    env_name: Option<String>,
    saved: &str,
) -> (String, String) {
    let server = pick(env_server, saved, VOICE_SERVER);
    let name = env_name.unwrap_or_else(|| host_port(&server, 0).0);
    (server, name)
}

/// FSD：`(主机, 端口)`。环境变量是**分开的两个**（`CAN_FSD_HOST` / `CAN_FSD_PORT`），
/// 设置里是一格 `主机:端口`，各自按优先级取。
pub fn fsd_target(
    env_host: Option<String>,
    env_port: Option<String>,
    saved: &str,
) -> (String, u16) {
    let (default_host, default_port) = host_port(FSD_SERVER, 0);
    let (saved_host, saved_port) = match saved.trim() {
        "" => (default_host, default_port),
        saved => host_port(saved, default_port),
    };
    let host = env_host.map(|h| h.trim().to_string()).unwrap_or(saved_host);
    let port = env_port
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(saved_port);
    (host, port)
}

/// 一个 URL 缺不缺协议头。
fn url_lacks_scheme(value: &str) -> bool {
    let v = value.trim();
    !(v.is_empty() || v.starts_with("https://") || v.starts_with("http://"))
}

/// 一个 `主机:端口` 哪里不对。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServerShape {
    /// 写成了 URL。
    HasScheme,
    /// 主机是空的、带空白，或者端口不是个数。
    NotHostPort,
}

fn server_problem(value: &str) -> Option<ServerShape> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    if v.contains("://") {
        return Some(ServerShape::HasScheme);
    }
    let (host, _) = host_port(v, 0);
    // 端口那一段：没有就行，有就得是个数。**不按字节切**——粘进来的东西后面
    // 可能跟着一个多字节字符，切在它中间是 panic。
    let port_fits = |tail: Option<&str>| tail.map_or(true, |p| p.parse::<u16>().is_ok());
    let port_ok = match v.strip_prefix('[') {
        Some(rest) => rest.split_once(']').is_some_and(|(_, tail)| {
            tail.is_empty() || tail.strip_prefix(':').is_some_and(|p| port_fits(Some(p)))
        }),
        None => port_fits(v.rsplit_once(':').map(|(_, p)| p)),
    };
    if host.is_empty() || host.contains(char::is_whitespace) || !port_ok {
        return Some(ServerShape::NotHostPort);
    }
    None
}

/// 设置对话框里的一格。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Field {
    /// [`Endpoints`] 里的字段名。
    pub key: &'static str,
    /// 留空时用的那个。界面拿它当占位提示。
    pub default: String,
    /// 此刻被环境变量盖着：这一格改了也不会生效。
    pub overridden: bool,
}

/// 某个客户端用得上的那几格。
///
/// **每个客户端自己报**，前端不抄默认值：默认地址分散在 `can-voice-datafeed`、
/// `can-voice-atis` 这几个 crate 里，前端再写一份迟早和真正生效的那个对不上，
/// 而占位提示上写着的正是人以为留空会连到的地方。
pub fn fields_with(
    wanted: &[(&'static str, &str)],
    lookup: impl Fn(&str) -> Option<String>,
) -> Vec<Field> {
    wanted
        .iter()
        .map(|&(key, default)| {
            let Some((_, vars)) = ENV_KEYS.iter().find(|(field, _)| *field == key) else {
                panic!("{key} is not a field of Endpoints");
            };
            Field {
                key,
                default: default.to_string(),
                overridden: vars.iter().any(|v| lookup(v).is_some()),
            }
        })
        .collect()
}

/// [`fields_with`]，环境变量从进程里读。
pub fn fields(wanted: &[(&'static str, &str)]) -> Vec<Field> {
    fields_with(wanted, env)
}

impl Endpoints {
    pub fn api_origin(&self) -> String {
        endpoint("CAN_API_ORIGIN", &self.api_origin, API_ORIGIN)
    }

    pub fn voice(&self) -> (String, String) {
        voice_target(
            env("CAN_VOICE_SERVER"),
            env("CAN_VOICE_SERVER_NAME"),
            &self.voice_server,
        )
    }

    pub fn fsd(&self) -> (String, u16) {
        fsd_target(env("CAN_FSD_HOST"), env("CAN_FSD_PORT"), &self.fsd_server)
    }

    /// 每一项去掉首尾空白。存之前跑一遍：粘贴进来的地址后面常常带着一个空格，
    /// 而它会让 DNS 解析失败，报的错指不到这里。
    pub fn trimmed(self) -> Self {
        let t = |s: String| s.trim().to_string();
        Self {
            api_origin: t(self.api_origin),
            voice_server: t(self.voice_server),
            fsd_server: t(self.fsd_server),
            datafeed_url: t(self.datafeed_url),
            metar_url: t(self.metar_url),
            atis_config_url: t(self.atis_config_url),
        }
    }

    /// 填得不对的地方。空的表示都对。
    ///
    /// **每一格、每一种错各有一个 key**，而不是一句"{格子}要……"加一个格子名：
    /// 格子名本身也要翻译，塞进占位符里的是一个已经定了语言的词。
    pub fn problems(&self) -> Vec<Message> {
        // key 都以 `Message::new("…")` 字面量写出来：字典测试扫的就是这个形状。
        let url = |message: Message, value: &str| {
            url_lacks_scheme(value).then(|| message.with("value", value.trim()))
        };
        let server = |scheme: Message, form: Message, value: &str| {
            server_problem(value).map(|shape| {
                match shape {
                    ServerShape::HasScheme => scheme,
                    ServerShape::NotHostPort => form,
                }
                .with("value", value.trim())
            })
        };
        [
            url(
                Message::new("error.endpoint.url_scheme.api_origin"),
                &self.api_origin,
            ),
            server(
                Message::new("error.endpoint.server_scheme.voice_server"),
                Message::new("error.endpoint.server_form.voice_server"),
                &self.voice_server,
            ),
            server(
                Message::new("error.endpoint.server_scheme.fsd_server"),
                Message::new("error.endpoint.server_form.fsd_server"),
                &self.fsd_server,
            ),
            url(
                Message::new("error.endpoint.url_scheme.datafeed_url"),
                &self.datafeed_url,
            ),
            url(
                Message::new("error.endpoint.url_scheme.metar_url"),
                &self.metar_url,
            ),
            url(
                Message::new("error.endpoint.url_scheme.atis_config_url"),
                &self.atis_config_url,
            ),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn an_empty_setting_means_the_default() {
        assert_eq!(pick(None, "", API_ORIGIN), API_ORIGIN);
        assert_eq!(pick(None, "   ", API_ORIGIN), API_ORIGIN);
    }

    #[test]
    fn a_saved_address_beats_the_default() {
        assert_eq!(
            pick(None, "https://test.example", API_ORIGIN),
            "https://test.example"
        );
    }

    /// 在命令行上指到本机起客户端的人，不该因为设置里存着线上地址就连到线上去。
    #[test]
    fn the_environment_beats_the_saved_address() {
        assert_eq!(
            pick(
                some("http://127.0.0.1:8080"),
                "https://test.example",
                API_ORIGIN
            ),
            "http://127.0.0.1:8080"
        );
    }

    #[test]
    fn a_saved_address_is_used_without_its_surrounding_whitespace() {
        assert_eq!(
            pick(None, " https://test.example ", API_ORIGIN),
            "https://test.example"
        );
    }

    #[test]
    fn a_host_and_port_split_where_you_would_expect() {
        assert_eq!(
            host_port("fsd.example:6810", 6809),
            ("fsd.example".into(), 6810)
        );
    }

    #[test]
    fn a_host_with_no_port_gets_the_default_one() {
        assert_eq!(host_port("fsd.example", 6809), ("fsd.example".into(), 6809));
    }

    #[test]
    fn a_port_that_is_not_a_number_gets_the_default_one() {
        assert_eq!(
            host_port("fsd.example:abc", 6809),
            ("fsd.example".into(), 6809)
        );
    }

    /// `split(':')` 会把 `[::1]:6809` 切成一地碎片，主机名变成 `[`。
    #[test]
    fn an_ipv6_address_in_brackets_keeps_its_colons() {
        assert_eq!(host_port("[::1]:6810", 6809), ("::1".into(), 6810));
        assert_eq!(host_port("[::1]", 6809), ("::1".into(), 6809));
    }

    #[test]
    fn the_voice_server_name_is_the_host_of_the_address_by_default() {
        assert_eq!(
            voice_target(None, None, ""),
            (
                VOICE_SERVER.to_string(),
                "audio.ceruleanavi.net".to_string()
            )
        );
        assert_eq!(
            voice_target(None, None, "voice.test.example:7000"),
            (
                "voice.test.example:7000".into(),
                "voice.test.example".into()
            )
        );
    }

    /// 指到一个 IP 上排障时，证书上写的还是域名。
    #[test]
    fn a_separate_server_name_from_the_environment_wins_for_tls() {
        assert_eq!(
            voice_target(some("10.0.0.5:64738"), some("audio.ceruleanavi.net"), ""),
            ("10.0.0.5:64738".into(), "audio.ceruleanavi.net".into())
        );
    }

    #[test]
    fn fsd_host_and_port_come_from_the_setting_when_nothing_overrides_them() {
        assert_eq!(
            fsd_target(None, None, ""),
            ("fsd.ceruleanavi.net".into(), 6809)
        );
        assert_eq!(
            fsd_target(None, None, "fsd.test.example:6810"),
            ("fsd.test.example".into(), 6810)
        );
    }

    /// 两个环境变量各管一半：只设了主机的人，端口还是设置里的那个。
    #[test]
    fn the_fsd_host_and_port_are_overridden_separately() {
        assert_eq!(
            fsd_target(some("127.0.0.1"), None, "fsd.test.example:6810"),
            ("127.0.0.1".into(), 6810)
        );
        assert_eq!(
            fsd_target(None, some("7000"), "fsd.test.example:6810"),
            ("fsd.test.example".into(), 7000)
        );
    }

    #[test]
    fn a_client_lists_its_own_fields_in_its_own_order_with_their_defaults() {
        let got = fields_with(
            &[("voice_server", VOICE_SERVER), ("api_origin", API_ORIGIN)],
            |_| None,
        );
        assert_eq!(
            got,
            [
                Field {
                    key: "voice_server",
                    default: VOICE_SERVER.into(),
                    overridden: false
                },
                Field {
                    key: "api_origin",
                    default: API_ORIGIN.into(),
                    overridden: false
                },
            ]
        );
    }

    #[test]
    fn a_field_under_an_environment_variable_says_so() {
        let got = fields_with(&[("fsd_server", FSD_SERVER)], |k| {
            (k == "CAN_FSD_PORT").then(|| "7000".to_string())
        });
        assert!(got[0].overridden);
    }

    /// 一个字段名拼错了，这一格在界面上就永远不会被标成"被盖着"——而那正是
    /// 人会困惑的时候。宁可当场响。
    #[test]
    #[should_panic(expected = "voice_sever")]
    fn a_field_name_nobody_knows_is_a_bug_not_a_blank_row() {
        fields_with(&[("voice_sever", VOICE_SERVER)], |_| None);
    }

    #[test]
    fn saving_trims_every_field() {
        let e = Endpoints {
            api_origin: " https://a.example ".into(),
            voice_server: "\tv.example:1\n".into(),
            ..Endpoints::default()
        }
        .trimmed();
        assert_eq!(e.api_origin, "https://a.example");
        assert_eq!(e.voice_server, "v.example:1");
    }

    #[test]
    fn all_empty_is_all_default_and_nothing_to_complain_about() {
        assert!(Endpoints::default().problems().is_empty());
    }

    /// 少了协议头的地址，reqwest 报的是 "builder error"，指不到这里。
    #[test]
    fn a_url_without_a_scheme_is_refused_before_it_is_saved() {
        let e = Endpoints {
            api_origin: "api.example".into(),
            ..Endpoints::default()
        };
        let problems = e.problems();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].key, "error.endpoint.url_scheme.api_origin");
        assert_eq!(problems[0].values["value"], "api.example");
    }

    #[test]
    fn a_server_address_with_a_port_that_is_not_a_number_is_refused() {
        let e = Endpoints {
            fsd_server: "fsd.example:port".into(),
            ..Endpoints::default()
        };
        assert_eq!(e.problems().len(), 1);
        assert_eq!(e.problems()[0].key, "error.endpoint.server_form.fsd_server");
    }

    /// 服务器地址不该带协议头——`https://audio.example:64738` 会被当成主机名
    /// `https` 端口 `//audio…`。
    #[test]
    fn a_server_address_written_like_a_url_is_refused() {
        let e = Endpoints {
            voice_server: "https://audio.example:64738".into(),
            ..Endpoints::default()
        };
        assert_eq!(e.problems().len(), 1);
        assert_eq!(
            e.problems()[0].key,
            "error.endpoint.server_scheme.voice_server"
        );
    }

    /// 设置对话框里什么都可能被粘进来。检查一个地址不能把程序检查崩了。
    #[test]
    fn garbage_after_an_ipv6_bracket_is_a_problem_rather_than_a_panic() {
        let e = Endpoints {
            voice_server: "[::1]é".into(),
            ..Endpoints::default()
        };
        assert_eq!(e.problems().len(), 1);
    }

    #[test]
    fn well_formed_addresses_pass() {
        let e = Endpoints {
            api_origin: "http://127.0.0.1:8080".into(),
            voice_server: "[::1]:64738".into(),
            fsd_server: "fsd.example".into(),
            datafeed_url: "https://data.example/v1/data.json".into(),
            metar_url: "https://metar.example/?id=".into(),
            atis_config_url: "https://api.example/api/v1/atis/config".into(),
        };
        assert!(e.problems().is_empty(), "{:?}", e.problems());
    }

    #[test]
    fn an_old_settings_file_with_no_endpoints_reads_as_all_default() {
        let got: Endpoints = serde_json::from_str("{}").unwrap();
        assert_eq!(got, Endpoints::default());
    }
}
