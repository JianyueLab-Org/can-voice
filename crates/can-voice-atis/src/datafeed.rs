//! 从 can-fsd 的 datafeed 取正在播的 ATIS。
//!
//! 消费的是 `atis[].callsign`、`.frequency` 和 `.text_atis` 三个字段。
//! can-fsd 保证 `text_atis` 是一个字符串数组、永不为 null，并且只有呼号以
//! `_ATIS` 结尾的席位才会出现在 `atis[]` 里——但这一侧照样自己判一次，
//! 因为判错的后果是在一个管制席位的频率上播通播。

use crate::profile::AtisType;
use serde_json::Value;

/// 取 datafeed 时用的 User-Agent。
///
/// **数据源前面挡着 Cloudflare，非浏览器形态的 UA 一律 403。** Python 版为此写了
/// 一整段注释：`requests` 的默认 UA（`python-requests/x.y`）就在被拒之列。
/// reqwest 的默认 UA 是同一类东西，所以这个头是必须的而不是礼貌——
/// 而 403 看起来像"datafeed 挂了"。
pub const USER_AGENT: &str = "Mozilla/5.0 (compatible; CanATIS/3.0)";

/// `frequency` 为这个值时表示"**没设频率**"，不是一个频率。
///
/// 拿它去订阅会在一个谁也不在的频率上播一整天，而日志里一切正常。
const NO_FREQUENCY_MHZ: f64 = 199.998;

/// 浮点比较的容差。上游给的可能是 `199.998`、`199.9980` 或 `199.998000`。
const FREQ_EPSILON: f64 = 0.001;

/// 一个正在播的通播席位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Station {
    pub callsign: String,
    pub freq_khz: u32,
    /// 整段报文，行与行之间用**一个空格**连接——换行读出来是一次停顿，
    /// 而报文的断行只是终端宽度，不是句读。
    pub text: String,
}

/// 从一份 datafeed 文档里取出所有该播的席位。
///
/// **单条坏数据只跳过那一条。** 一个缺字段的上游不该让整个机队崩掉——
/// 那会让全网的 ATIS 一起下线，而这正是这支机队存在的理由。
///
/// # 缺 `atis` 字段答 `None`，不是一个空列表
///
/// 两者对调用方是两件事：空列表的意思是"此刻没有人在播"，对账据此把所有正在
/// 播的席位停掉；而一份 200 但形状不对的文档说明不了这件事，它比一次网络错误
/// 更不可能代表"全网的 ATIS 都下线了"。答空列表的那一版会让整队停播，30 秒后
/// 字段回来了再全部重连、重新合成一遍。旧版有这道保护：
/// `if data and 'atis' in data`（`can-audio/server/ATIS/mumble.py:460`）。
pub fn stations_from(feed: &Value) -> Option<Vec<Station>> {
    let list = feed.get("atis")?.as_array()?;
    Some(list.iter().filter_map(station_from).collect())
}

fn station_from(entry: &Value) -> Option<Station> {
    let callsign = entry.get("callsign")?.as_str()?.trim();
    if !callsign.ends_with("_ATIS") {
        return None;
    }
    let mhz: f64 = entry.get("frequency")?.as_str()?.trim().parse().ok()?;
    if (mhz - NO_FREQUENCY_MHZ).abs() < FREQ_EPSILON {
        tracing::debug!(callsign, "skipping the no-frequency placeholder");
        return None;
    }
    let lines = entry.get("text_atis")?.as_array()?;
    let text = lines
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        return None;
    }
    Some(Station {
        callsign: callsign.to_string(),
        freq_khz: (mhz * 1000.0).round() as u32,
        text,
    })
}

/// 数据源上此刻在线的一个通播席位，**给桌面端挑用**。
///
/// 和上面那个 [`Station`] 不是一回事：那一个是给机队播的，带整段报文；
/// 这一个只有机场、呼号、频率和类型——够建一个席位，不够播。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Online {
    pub icao: String,
    pub callsign: String,
    /// 十进制兆赫字符串，和 [`crate::profile::Station::frequency`] 同形状，
    /// 所以可以直接填进去。
    pub frequency: String,
    pub atis_type: AtisType,
}

impl Online {
    /// 建一个可以加进配置的席位。
    ///
    /// **只有机场、频率和类型**——模板、预设、跑道构型它给不了，所以建出来的是
    /// 默认模板的席位，不是一个导完就能播的席位。界面要把这一点说清楚。
    pub fn to_station(&self) -> crate::profile::Station {
        let mut station = crate::profile::Station::new(&self.icao);
        station.frequency = self.frequency.clone();
        station.atis_type = self.atis_type;
        station
    }
}

/// 呼号后缀 → 通播类型。**长的在前**：`_ATIS` 会把 `ZSPD_D_ATIS` 也匹配上，
/// 于是机场变成 `ZSPD_D`，而那不是一个四位代码。
const SUFFIXES: &[(&str, AtisType)] = &[
    ("_D_ATIS", AtisType::Departure),
    ("_A_ATIS", AtisType::Arrival),
    ("_ATIS", AtisType::Combined),
];

/// 数据源上此刻在线的通播席位。
///
/// **这不是"从网上取配置"。** 配置在 can-api 的 `/api/v1/atis/config`，由
/// [`crate::netconfig`] 取（席位、频率、跑道构型预设、模板、中文用词）。这里读
/// 的是运行状态，所以它能省掉的只是查机场和频率这一步。
pub fn online_stations(feed: &Value) -> Vec<Online> {
    let Some(list) = feed.get("atis").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out: Vec<Online> = list.iter().filter_map(online_from).collect();
    out.sort_by(|a, b| a.callsign.cmp(&b.callsign));
    out
}

fn online_from(entry: &Value) -> Option<Online> {
    let callsign = entry.get("callsign")?.as_str()?.trim().to_uppercase();
    let frequency = entry.get("frequency")?.as_str()?.trim();
    let mhz: f64 = frequency.parse().ok()?;
    if (mhz - NO_FREQUENCY_MHZ).abs() < FREQ_EPSILON {
        return None;
    }
    let (icao, atis_type) = SUFFIXES
        .iter()
        .find_map(|(suffix, kind)| Some((callsign.strip_suffix(suffix)?.to_string(), *kind)))?;
    if icao.len() != 4 || !icao.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(Online {
        icao,
        callsign,
        frequency: frequency.to_string(),
        atis_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feed(atis: serde_json::Value) -> serde_json::Value {
        json!({ "atis": atis, "pilots": [], "controllers": [], "general": {} })
    }

    fn one(callsign: &str, frequency: &str, text: &[&str]) -> serde_json::Value {
        json!({ "callsign": callsign, "frequency": frequency, "text_atis": text })
    }

    /// 形状是对的那一份。缺字段的那一份归
    /// `a_feed_without_an_atis_array_is_not_the_same_as_nobody_broadcasting` 管。
    fn stations(feed: &serde_json::Value) -> Vec<Station> {
        stations_from(feed).expect("the feed carries an atis array")
    }

    #[test]
    fn a_station_comes_through_with_its_frequency_in_khz() {
        let s = stations(&feed(json!([one(
            "ZSSS_ATIS",
            "132.250",
            &["ZSSS ATIS A"]
        )])));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].callsign, "ZSSS_ATIS");
        assert_eq!(s[0].freq_khz, 132_250);
    }

    /// 行用**一个空格**连起来，不是换行。换行读出来是一次停顿，
    /// 而报文的断行只是终端宽度，不是句读。
    #[test]
    fn the_lines_are_joined_with_a_single_space() {
        let s = stations(&feed(json!([one("ZSSS_ATIS", "132.250", &["A B", "C D"])])));
        assert_eq!(s[0].text, "A B C D");
    }

    /// **`199.998` 的意思是"没设频率"，不是一个频率。** 拿它去订阅会在一个
    /// 谁也不在的频率上播一整天，而日志里一切正常。
    #[test]
    fn the_no_frequency_placeholder_is_skipped() {
        let s = stations(&feed(json!([
            one("ZSSS_ATIS", "199.998", &["x"]),
            one("ZBAA_ATIS", "127.800", &["y"]),
        ])));
        assert_eq!(s.len(), 1, "only the real frequency should survive: {s:?}");
        assert_eq!(s[0].callsign, "ZBAA_ATIS");
    }

    /// 判据是**近似相等**，不是字符串相等：上游给的可能是 `199.998`、
    /// `199.9980` 或者 `199.998000`。
    #[test]
    fn the_placeholder_is_matched_numerically_not_textually() {
        for raw in ["199.998", "199.9980", "199.998000"] {
            let s = stations(&feed(json!([one("ZSSS_ATIS", raw, &["x"])])));
            assert!(s.is_empty(), "{raw} should have been skipped");
        }
    }

    /// 只有 `_ATIS` 结尾的才是通播席位。can-fsd 保证了这一点，
    /// 但这一侧照样判——它是"要不要开一路去播"的判据，判错就是在一个
    /// 管制席位的频率上播通播。
    #[test]
    fn only_atis_callsigns_are_broadcast() {
        let s = stations(&feed(json!([
            one("ZSSS_TWR", "118.500", &["x"]),
            one("ZSSS_ATIS", "132.250", &["y"]),
        ])));
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].callsign, "ZSSS_ATIS");
    }

    #[test]
    fn a_station_with_no_text_is_skipped() {
        let s = stations(&feed(json!([one("ZSSS_ATIS", "132.250", &[])])));
        assert!(s.is_empty(), "there is nothing to say");
    }

    /// can-fsd 保证 `text_atis` 是数组、不是 null，但一个缺字段的上游
    /// **不该让整个机队崩掉**——那会让全网 ATIS 一起下线。
    #[test]
    fn a_malformed_entry_is_skipped_rather_than_fatal() {
        let s = stations(&feed(json!([
            json!({ "callsign": "ZSSS_ATIS" }),
            json!({ "frequency": "127.800", "text_atis": ["x"] }),
            json!("not even an object"),
            one("ZBAA_ATIS", "127.800", &["y"]),
        ])));
        assert_eq!(
            s.len(),
            1,
            "the one good entry must still come through: {s:?}"
        );
    }

    /// **缺字段不是"没人在播"。**
    ///
    /// 返回空列表的那一版会让对账把所有正在播的席位停掉，30 秒后字段回来了
    /// 再全部重连、重新合成一遍——而同一个循环的另一条规矩是"取不到 datafeed
    /// 不停播"。一份 200 但形状不对的文档，比一次网络错误更不可能说明
    /// "全网的 ATIS 都下线了"。旧版有这道保护：`if data and 'atis' in data`
    /// （`can-audio/server/ATIS/mumble.py:460`）。
    #[test]
    fn a_feed_without_an_atis_array_is_not_the_same_as_nobody_broadcasting() {
        assert!(stations_from(&json!({ "pilots": [] })).is_none());
        assert!(stations_from(&json!({ "atis": null })).is_none());
        assert!(stations_from(&json!({ "atis": "nonsense" })).is_none());
        assert!(stations_from(&json!("nonsense")).is_none());
        // 字段在、而且是个空数组：**这一条**才是"此刻没有人在播"。
        assert_eq!(stations_from(&feed(json!([]))), Some(Vec::new()));
    }

    #[test]
    fn a_frequency_that_is_not_a_number_is_skipped() {
        let s = stations(&feed(json!([one("ZSSS_ATIS", "N/A", &["x"])])));
        assert!(s.is_empty());
    }

    /// **数据源前面挡着 Cloudflare，非浏览器形态的 User-Agent 一律 403。**
    /// Python 版为此写了一整段注释：`requests` 的默认 UA
    /// （`python-requests/x.y`）就在被拒之列，不带这个头取不到任何数据——
    /// 而 403 看起来像"datafeed 挂了"。
    #[test]
    fn the_user_agent_does_not_look_like_a_library_default() {
        assert!(USER_AGENT.starts_with("Mozilla/"), "got {USER_AGENT}");
        for banned in ["reqwest", "python-requests", "curl", "Go-http-client"] {
            assert!(
                !USER_AGENT.contains(banned),
                "{banned} is on Cloudflare's reject list"
            );
        }
    }
    /// 拿 can-fsd 的 **datafeed 黄金文件**过一遍。
    ///
    /// `server/testdata/datafeed_golden.json` 是 can-fsd 那一份的逐字节副本，每天
    /// 对一次账（`.github/workflows/datafeed-golden.yml`），服务端的
    /// `internal/fsdfeed` 读的也是它——所以这条测试钉住的是上游的契约，而不是
    /// 我们以为的契约。自造的 JSON 证明不了这件事：它只证明我们和自己一致。
    #[test]
    fn can_fsds_golden_datafeed_parses() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../server/testdata/datafeed_golden.json"
        );
        let raw = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let feed: serde_json::Value =
            serde_json::from_slice(&raw).expect("parse the golden datafeed");

        let s = stations(&feed);
        assert_eq!(
            s.len(),
            1,
            "the golden datafeed carries one ATIS station: {s:?}"
        );
        assert_eq!(s[0].callsign, "ZSSS_ATIS");
        assert_eq!(s[0].freq_khz, 132_250);
        assert!(s[0].text.starts_with("ZSSS ATIS A 1200Z"), "{}", s[0].text);
        assert!(
            !s[0].text.contains('\n'),
            "lines are joined with a space, not a newline"
        );
    }
}

#[cfg(test)]
mod online_tests {
    use super::*;
    use serde_json::json;

    fn feed(atis: Value) -> Value {
        json!({ "atis": atis, "pilots": [], "controllers": [], "general": {} })
    }

    fn one(callsign: &str, frequency: &str) -> Value {
        json!({ "callsign": callsign, "frequency": frequency, "text_atis": ["x"] })
    }

    #[test]
    fn an_online_position_gives_its_airport_and_its_frequency() {
        let found = online_stations(&feed(json!([one("ZSPD_ATIS", "127.850")])));
        assert_eq!(
            found,
            [Online {
                icao: "ZSPD".into(),
                callsign: "ZSPD_ATIS".into(),
                frequency: "127.850".into(),
                atis_type: AtisType::Combined,
            }]
        );
    }

    /// `_ATIS` 会把 `ZSPD_D_ATIS` 也匹配上，于是机场变成 `ZSPD_D`——一个四位
    /// 检查挡不住的五位串。长后缀必须先试。
    #[test]
    fn the_departure_suffix_is_matched_before_the_bare_one() {
        let found = online_stations(&feed(json!([one("ZSPD_D_ATIS", "126.500")])));
        assert_eq!(found[0].icao, "ZSPD");
        assert_eq!(found[0].atis_type, AtisType::Departure);
    }

    #[test]
    fn the_arrival_suffix_is_read_too() {
        let found = online_stations(&feed(json!([one("ZSPD_A_ATIS", "126.500")])));
        assert_eq!(found[0].atis_type, AtisType::Arrival);
    }

    /// 拿这个占位值去建席位，人会在一个谁也不在的频率上播一整天。
    #[test]
    fn the_no_frequency_placeholder_is_not_a_position_worth_offering() {
        assert!(online_stations(&feed(json!([one("ZSPD_ATIS", "199.998")]))).is_empty());
    }

    #[test]
    fn a_callsign_that_is_not_an_atis_one_is_skipped() {
        assert!(online_stations(&feed(json!([one("ZSPD_TWR", "118.000")]))).is_empty());
    }

    #[test]
    fn an_airport_code_that_is_not_four_characters_is_skipped() {
        assert!(online_stations(&feed(json!([one("ZS_ATIS", "118.000")]))).is_empty());
    }

    #[test]
    fn positions_come_back_in_callsign_order() {
        let found = online_stations(&feed(json!([
            one("ZSPD_ATIS", "127.850"),
            one("ZBAA_ATIS", "126.800"),
        ])));
        let callsigns: Vec<&str> = found.iter().map(|o| o.callsign.as_str()).collect();
        assert_eq!(callsigns, ["ZBAA_ATIS", "ZSPD_ATIS"]);
    }

    /// 取回来的频率要能**直接填进席位**，否则这一步省不掉什么。
    #[test]
    fn the_frequency_is_the_shape_a_station_stores() {
        let found = online_stations(&feed(json!([one("ZSPD_ATIS", "127.850")])));
        let mut station = crate::profile::Station::new(&found[0].icao);
        station.frequency = found[0].frequency.clone();
        assert_eq!(station.frequency_khz(), Some(127_850));
    }

    #[test]
    fn an_online_position_becomes_a_station_with_the_same_callsign_and_frequency() {
        let found = online_stations(&feed(json!([one("ZSPD_D_ATIS", "126.500")])));
        let station = found[0].to_station();
        assert_eq!(station.callsign(), "ZSPD_D_ATIS");
        assert_eq!(station.frequency_khz(), Some(126_500));
        // 坐标按 ICAO 补上：留成 0/0 的席位会显示在几内亚湾外海。
        assert_ne!((station.latitude, station.longitude), (0.0, 0.0));
    }

    #[test]
    fn a_feed_with_no_atis_array_at_all_is_simply_empty() {
        assert!(online_stations(&json!({"pilots": []})).is_empty());
    }
}
