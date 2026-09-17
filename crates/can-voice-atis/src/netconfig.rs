//! 从 can-api 取全网通播配置，并进本地那一份。
//!
//! ```text
//! GET https://api.ceruleanavi.net/api/v1/atis/config
//! ```
//!
//! # 这和「取在线席位」不是一回事
//!
//! 数据源（`data.ceruleanavi.net` 的 `atis[]`，见 [`crate::datafeed`]）给的是
//! **此刻谁在播**，只有机场和频率，是运行状态。这个接口给的是**配置本身**
//! ——席位、频率、跑道构型预设、模板、中文播报用词。以前网上没有这种东西，
//! 每个人都得把同样的模板和中文跑道词手打一遍，改了也传不到别人那里。
//!
//! 回的文档就是本客户端自己的 JSON 形状（[`Station`] 的 snake_case 字段）：
//!
//! ```text
//! {
//!   "version": "3f6d746b8451",   // 内容哈希，服务端算的，不是手填的版本号
//!   "updated": "2026-07-30",     // 给人看的日期
//!   "notes": "……",               // 一行说明，界面上显示
//!   "stations": [ {…}, … ]
//! }
//! ```
//!
//! # 三条原则，都是踩过的
//!
//! - **不认识的字段忽略，认识的字段照单全收。** 客户端版本比配置旧时，多出来的
//!   键直接跳过，而不是整份读不进来。
//! - **默认只补缺，不覆盖。** 本地那份可能是值班时手改过的（临时构型、NOTAM），
//!   网络版一律盖掉等于把人家的活删了。要覆盖必须调用方明确要求。
//! - **正在播出的席位一律不动。** 换掉它只会让稿子和实际在播的内容对不上。
//!
//! 版本号是服务端算的内容哈希（连 notes 改了都会变），所以"已经是最新"这个
//! 判断不会因为谁忘了手动进位而失效。

use crate::profile::{Profile, Station};
use can_voice_i18n::Message;
use serde_json::Value;

/// 配置在哪。
///
/// **是 can-api 自己的主机，不是 `ceruleanavi.net`。** 后者那条也答，但那是
/// can-web 给编译死了老地址的 Python 客户端留的反向代理；新客户端没有那份历史
/// 包袱，没理由多绕一跳。
pub const DEFAULT_URL: &str = "https://api.ceruleanavi.net/api/v1/atis/config";

/// 和 [`crate::datafeed`] 同一个原因：`ceruleanavi.net` 前面挡着 Cloudflare，
/// 非浏览器形态的 User-Agent 一律 403——而 403 看起来像"接口挂了"。
pub const USER_AGENT: &str = "Mozilla/5.0 (compatible; CanATIS/3.0)";

/// 甚高频航空频段，单位千赫。频率算不出来的席位留着只会在开播时才炸。
const VHF_KHZ: std::ops::RangeInclusive<u32> = 100_000..=200_000;

/// `Display` 是给日志的英文；界面上的那几句走 [`NetConfigError::messages`]（#29）。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NetConfigError {
    #[error("rate limited by the server")]
    RateLimited,
    #[error("the server answered {status} ({url})")]
    Status { status: u16, url: String },
    #[error("could not reach {url}: {detail}")]
    Unreachable { url: String, detail: String },
    #[error("the response is not valid JSON: {0}")]
    NotJson(String),
    #[error("the response is not a configuration document")]
    NotADocument,
    #[error("the configuration has no stations")]
    NoStations,
    /// 装的是前三个席位各自的问题。
    #[error("the configuration has no usable station: {}", joined(.0))]
    NothingUsable(Vec<Message>),
}

fn joined(problems: &[Message]) -> String {
    problems
        .iter()
        .map(Message::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

impl NetConfigError {
    /// 给人看的那一句。"没有能用的席位"后面跟着是哪几个、为什么，挂在
    /// [`Message::details`] 上——前端一条条翻、按当前语言的标点连起来。
    pub fn message(&self) -> Message {
        match self {
            NetConfigError::RateLimited => Message::new("problem.netconfig.rate_limited"),
            NetConfigError::Status { status, url } => Message::new("problem.netconfig.status")
                .with("status", status)
                .with("url", url),
            NetConfigError::Unreachable { url, detail } => {
                Message::new("problem.netconfig.unreachable")
                    .with("url", url)
                    .with("detail", detail)
            }
            NetConfigError::NotJson(detail) => {
                Message::new("problem.netconfig.not_json").with("detail", detail)
            }
            NetConfigError::NotADocument => Message::new("problem.netconfig.not_a_document"),
            NetConfigError::NoStations => Message::new("problem.netconfig.no_stations"),
            NetConfigError::NothingUsable(problems) => {
                Message::new("problem.netconfig.nothing_usable")
                    .with_details(problems.iter().cloned())
            }
        }
    }
}

/// 解析好的一份网络配置。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct NetworkConfig {
    pub version: String,
    pub updated: String,
    pub notes: String,
    pub stations: Vec<Station>,
    /// 单个读不进来的席位在这里报出来，别让人以为全都拿到了。
    pub problems: Vec<Message>,
}

impl NetworkConfig {
    /// 界面上显示的版本说明。括号是哪一种、"未知版本"怎么说，归字典（#29）。
    pub fn label(&self) -> Message {
        match (self.updated.is_empty(), self.version.is_empty()) {
            (false, false) => Message::new("network.label.both")
                .with("updated", &self.updated)
                .with("version", &self.version),
            (false, true) => Message::new("network.label.one").with("value", &self.updated),
            (true, false) => Message::new("network.label.one").with("value", &self.version),
            (true, true) => Message::new("network.label.unknown"),
        }
    }

    pub fn len(&self) -> usize {
        self.stations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stations.is_empty()
    }
}

/// 和本地那份比一比的结果。三个列表装的都是**网络版**的席位。
///
/// 界面先把这个结果给用户看，再决定要不要动他的配置——"按一下就变了"在值班时
/// 是很难接受的。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Comparison {
    pub missing: Vec<Station>,
    pub differing: Vec<Station>,
    pub same: Vec<Station>,
}

/// 并完之后发生了什么。装的是呼号。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Merged {
    pub added: Vec<String>,
    pub replaced: Vec<String>,
    pub kept: Vec<String>,
    /// 因为正在播出而跳过的。
    pub skipped: Vec<String>,
}

/// 文档里的席位列表。
///
/// 正常是 `stations`。也认 `profiles`——本客户端存盘用的就是那个形状，于是把
/// 配置地址指向自己导出的那份 json（或分区自建的一份）也能用。
fn station_entries(document: &Value) -> Vec<&Value> {
    if let Some(list) = document.get("stations").and_then(Value::as_array) {
        return list.iter().collect();
    }
    document
        .get("profiles")
        .and_then(Value::as_array)
        .map(|profiles| {
            profiles
                .iter()
                .filter_map(|p| p.get("stations").and_then(Value::as_array))
                .flatten()
                .collect()
        })
        .unwrap_or_default()
}

fn string_at(document: &Value, key: &str) -> String {
    document
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// 文档 → [`NetworkConfig`]。单个席位坏掉不连累整份。
pub fn parse(document: &Value) -> Result<NetworkConfig, NetConfigError> {
    if !document.is_object() {
        return Err(NetConfigError::NotADocument);
    }
    let entries = station_entries(document);
    if entries.is_empty() {
        return Err(NetConfigError::NoStations);
    }

    let mut stations = Vec::new();
    let mut problems = Vec::new();
    for entry in entries {
        // 报错时要说出是哪个席位。解不出来的那些连 identifier 都未必有，
        // 所以这一行不能等 Station 解出来再取。
        let named = entry
            .get("identifier")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .trim()
            .to_uppercase();
        // **不认识的字段忽略。** serde 默认就是跳过多余的键，而这正是要的：
        // 客户端版本比配置旧时，多出来的键不该让整份读不进来。
        let mut station: Station = match serde_json::from_value(entry.clone()) {
            Ok(s) => s,
            Err(e) => {
                // serde 的报错是英文原文，当作数据交出去。
                problems.push(
                    Message::new("problem.netconfig.station_unreadable")
                        .with("station", &named)
                        .with("detail", e),
                );
                continue;
            }
        };
        station.normalise();
        match station.frequency_khz() {
            Some(khz) if VHF_KHZ.contains(&khz) => stations.push(station),
            Some(_) => problems.push(
                Message::new("problem.netconfig.out_of_band")
                    .with("station", &named)
                    .with("frequency", &station.frequency),
            ),
            None => problems.push(
                Message::new("problem.netconfig.bad_frequency")
                    .with("station", &named)
                    .with("frequency", &station.frequency),
            ),
        }
    }

    if stations.is_empty() {
        // 报前三条。全列出来的话一份坏文件会刷满整块提示。
        problems.truncate(3);
        return Err(NetConfigError::NothingUsable(problems));
    }
    stations.sort_by_key(Station::callsign);
    Ok(NetworkConfig {
        version: string_at(document, "version"),
        updated: string_at(document, "updated"),
        notes: string_at(document, "notes"),
        stations,
        problems,
    })
}

/// 两个席位实质上是否不同。
///
/// 比的是**配置**，所以先把运行状态摘掉：情报字母每几分钟就推进一格，带着它比
/// 的话每个席位永远都"和网络版不一样"。
pub fn differs(local: &Station, remote: &Station) -> bool {
    let config_only = |station: &Station| {
        let mut s = station.clone();
        s.letter = '\0';
        s
    };
    config_only(local) != config_only(remote)
}

/// 和本地那份比一比。
pub fn compare(profile: &Profile, stations: &[Station]) -> Comparison {
    let mut out = Comparison::default();
    for station in stations {
        match profile.get(&station.callsign()) {
            None => out.missing.push(station.clone()),
            Some(here) if differs(here, station) => out.differing.push(station.clone()),
            Some(_) => out.same.push(station.clone()),
        }
    }
    out
}

/// 用户在差异面前勾了什么，就只并那些。
///
/// 补缺和覆盖是**分开问**的：补缺几乎总是想要的，覆盖却可能删掉值班时手改过
/// 的临时构型和 NOTAM，所以两件事不能被一个"确定"一起点掉。
pub fn chosen(comparison: &Comparison, add_missing: bool, overwrite: bool) -> Vec<Station> {
    let mut out = Vec::new();
    if add_missing {
        out.extend(comparison.missing.iter().cloned());
    }
    if overwrite {
        out.extend(comparison.differing.iter().cloned());
    }
    out
}

impl Merged {
    /// 这一次是不是把整份网络配置都并进来了。
    ///
    /// 只有是，才该记住"已经更新到这一版"——否则下次点开，被跳过或没勾的那些
    /// 差异就再也不会出现在人面前。
    pub fn settles(&self, comparison: &Comparison, add_missing: bool, overwrite: bool) -> bool {
        self.skipped.is_empty()
            && (comparison.missing.is_empty() || add_missing)
            && (comparison.differing.is_empty() || overwrite)
    }
}

/// 把网络配置并进 profile。不存盘——存盘走调用方那份 `ProfileSet`。
pub fn merge(
    profile: &mut Profile,
    stations: &[Station],
    overwrite: bool,
    protected: &[String],
) -> Merged {
    let mut done = Merged::default();
    for station in stations {
        let callsign = station.callsign();
        let Some(here) = profile.get(&callsign) else {
            // add 只会因为呼号重复而失败，而这一支恰恰是"本地没有"。
            if profile.add(station.clone()).is_ok() {
                done.added.push(callsign);
            }
            continue;
        };
        if protected.contains(&callsign) {
            done.skipped.push(callsign);
            continue;
        }
        if !overwrite || !differs(here, station) {
            done.kept.push(callsign);
            continue;
        }
        // 情报字母跟着本地走：播了一半把字母退回 A，飞行员报的和听到的就对不上。
        let letter = here.letter;
        let mut replacement = station.clone();
        replacement.set_letter(letter);
        profile.remove(&callsign);
        let _ = profile.add(replacement);
        done.replaced.push(callsign);
    }

    if !done.added.is_empty() || !done.replaced.is_empty() {
        tracing::info!(
            added = done.added.len(),
            overwritten = done.replaced.len(),
            kept = done.kept.len(),
            skipped = done.skipped.len(),
            "merged the network configuration"
        );
    }
    done
}

/// 取一份配置文档。
///
/// **失败一律说出为什么**，不像 [`crate::datafeed`] 那边可以静默回 `None`：
/// 查等级失败不该影响播出，但用户明确按了"更新配置"，失败就必须告诉他原因，
/// 否则他只得到一个什么都没发生的按钮。
pub async fn fetch(client: &reqwest::Client, url: &str) -> Result<Value, NetConfigError> {
    let response = client
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| NetConfigError::Unreachable {
            url: url.to_string(),
            detail: e.to_string(),
        })?;

    let status = response.status();
    if status.as_u16() == 429 {
        return Err(NetConfigError::RateLimited);
    }
    if !status.is_success() {
        return Err(NetConfigError::Status {
            status: status.as_u16(),
            url: url.to_string(),
        });
    }
    let body = response
        .text()
        .await
        .map_err(|e| NetConfigError::Unreachable {
            url: url.to_string(),
            detail: e.to_string(),
        })?;
    serde_json::from_str(&body).map_err(|e| NetConfigError::NotJson(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::AtisType;
    use serde_json::json;

    fn wire(identifier: &str, frequency: &str) -> Value {
        json!({
            "identifier": identifier,
            "name": identifier,
            "frequency": frequency,
            "atis_type": "combined",
            "presets": [{"name": "默认", "template": "[WIND]"}],
        })
    }

    fn document(stations: Value) -> Value {
        json!({
            "version": "3f6d746b8451",
            "updated": "2026-07-30",
            "notes": "全网统一模板",
            "stations": stations,
        })
    }

    fn local(identifier: &str, frequency: &str) -> Station {
        let mut s = Station::new(identifier);
        s.frequency = frequency.into();
        s.atis_type = AtisType::Combined;
        s
    }

    #[test]
    fn a_document_with_no_stations_is_refused() {
        assert_eq!(parse(&document(json!([]))), Err(NetConfigError::NoStations));
    }

    #[test]
    fn something_that_is_not_a_document_is_refused_before_anything_else() {
        assert_eq!(parse(&json!([1, 2, 3])), Err(NetConfigError::NotADocument));
    }

    #[test]
    fn the_version_and_the_date_both_show_up_in_the_label() {
        let config = parse(&document(json!([wire("ZSPD", "127.850")]))).unwrap();
        assert_eq!(
            config.label(),
            Message::new("network.label.both")
                .with("updated", "2026-07-30")
                .with("version", "3f6d746b8451")
        );
    }

    #[test]
    fn a_config_that_names_neither_is_still_labelled() {
        let config = parse(&json!({"stations": [wire("ZSPD", "127.850")]})).unwrap();
        assert_eq!(config.label(), Message::new("network.label.unknown"));
    }

    /// 频率算不出来的席位留着只会在开播时才炸，那时候人已经在台上了。
    #[test]
    fn a_frequency_outside_the_vhf_band_is_left_out_and_said_so() {
        let config = parse(&document(json!([
            wire("ZSPD", "127.850"),
            wire("ZBAA", "8.500"),
        ])))
        .unwrap();
        assert_eq!(config.stations.len(), 1);
        assert_eq!(
            config.problems,
            [Message::new("problem.netconfig.out_of_band")
                .with("station", "ZBAA")
                .with("frequency", "8.500")]
        );
    }

    #[test]
    fn a_station_that_cannot_be_read_at_all_does_not_sink_the_rest() {
        let config = parse(&document(json!([
            {"name": "没有识别码"},
            wire("ZSPD", "127.850"),
        ])))
        .unwrap();
        assert_eq!(config.stations.len(), 1);
        assert_eq!(config.problems.len(), 1);
        assert_eq!(
            config.problems[0].key,
            "problem.netconfig.station_unreadable"
        );
        assert_eq!(config.problems[0].values["station"], "?");
    }

    /// 一个能用的都没有时，界面上要说出是哪几个、为什么——不是一句光秃秃的"没有"。
    #[test]
    fn nothing_usable_names_the_first_few_stations_and_why() {
        let err = parse(&document(json!([
            wire("ZBAA", "8.500"),
            wire("ZGGG", "x"),
            wire("ZSSS", "1.0"),
            wire("ZUUU", "2.0"),
        ])))
        .expect_err("nothing usable");
        let said = err.message();
        assert_eq!(said.key, "problem.netconfig.nothing_usable");
        assert_eq!(said.details.len(), 3, "the first three: {said:?}");
        assert_eq!(said.details[1].key, "problem.netconfig.bad_frequency");
    }

    #[test]
    fn stations_come_back_in_callsign_order() {
        let config = parse(&document(json!([
            wire("ZSPD", "127.850"),
            wire("ZBAA", "126.800"),
        ])))
        .unwrap();
        let callsigns: Vec<String> = config.stations.iter().map(Station::callsign).collect();
        assert_eq!(callsigns, ["ZBAA_ATIS", "ZSPD_ATIS"]);
    }

    /// 客户端版本比配置旧时，多出来的键该直接跳过，而不是整份读不进来。
    #[test]
    fn a_key_this_version_has_never_heard_of_is_ignored_rather_than_fatal() {
        let mut entry = wire("ZSPD", "127.850");
        entry["some_future_field"] = json!({"nested": true});
        let config = parse(&document(json!([entry]))).unwrap();
        assert_eq!(config.stations.len(), 1);
    }

    /// 把 config_url 指向自己导出的那份 json 也该能用。
    #[test]
    fn the_shape_this_client_exports_is_read_too() {
        let config = parse(&json!({
            "profiles": [{"name": "默认", "stations": [wire("ZSPD", "127.850")]}]
        }))
        .unwrap();
        assert_eq!(config.stations.len(), 1);
    }

    /// **跨仓契约。** 夹具是 can-api 线上那份配置里的一个席位原样拷过来的。
    ///
    /// 这里要防的是字段改名：serde 对缺失的键给默认值而不是报错，所以 can-api
    /// 把 `chinese_extra` 改个名、或者这边把 serde 字段改个名，读出来的都是一个
    /// "空的"中文附加——解析照样成功，播出来少一句。只有逐字段断言才看得见。
    #[test]
    fn the_shape_can_api_serves_today_reads_back_field_for_field() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata/atis-config-can-api.json");
        let raw = std::fs::read_to_string(&path).expect("fixture");
        let config = parse(&serde_json::from_str(&raw).expect("json")).expect("config");

        assert_eq!(config.version, "f9b4e50ead40");
        assert!(config.problems.is_empty(), "{:?}", config.problems);
        let zbaa = &config.stations[0];
        assert_eq!(zbaa.callsign(), "ZBAA_ATIS");
        assert_eq!(zbaa.frequency_khz(), Some(127_000));
        assert_eq!(zbaa.chinese_name, "北京首都国际机场");
        assert_eq!(zbaa.voice_language, crate::profile::VoiceLanguage::Both);
        assert!((zbaa.latitude - 40.07407).abs() < 1e-6);
        assert_eq!(zbaa.code_range, ('A', 'Z'));
        // 线上那份不带字母：没有字母就从范围的起点开始，而不是一个 '\0'。
        assert_eq!(zbaa.letter, 'A');

        let south = zbaa.preset("南向").expect("preset");
        assert_eq!(south.name, "南向");
        assert!(south.airport_conditions.starts_with("ARR RWY 19"));
        assert!(south.notams.starts_with("Delivery frequency"));
        assert!(south.chinese_runway.starts_with("跑道 幺九"));
        assert!(south.closing.starts_with("advise on initial contact"));
        assert!(south.chinese_extra.starts_with("放行频率"));
        assert_eq!(zbaa.presets.len(), 2);
    }

    // ——— 比对 ———

    #[test]
    fn comparing_sorts_the_network_stations_into_missing_differing_and_same() {
        let mut profile = Profile::named("默认");
        profile.add(local("ZSPD", "127.850")).unwrap();
        profile.add(local("ZBAA", "126.800")).unwrap();

        let network = vec![
            local("ZSPD", "127.850"),
            local("ZBAA", "128.000"),
            local("ZGGG", "126.500"),
        ];
        let c = compare(&profile, &network);
        assert_eq!(
            c.same.iter().map(Station::callsign).collect::<Vec<_>>(),
            ["ZSPD_ATIS"]
        );
        assert_eq!(
            c.differing
                .iter()
                .map(Station::callsign)
                .collect::<Vec<_>>(),
            ["ZBAA_ATIS"]
        );
        assert_eq!(
            c.missing.iter().map(Station::callsign).collect::<Vec<_>>(),
            ["ZGGG_ATIS"]
        );
    }

    /// 情报字母每几分钟就推进一格，带着它比的话每个席位永远都"和网络版不一样"。
    #[test]
    fn the_information_letter_is_not_part_of_what_makes_two_stations_differ() {
        let mut here = local("ZSPD", "127.850");
        let there = local("ZSPD", "127.850");
        here.set_letter('Q');
        assert!(!differs(&here, &there));
    }

    // ——— 并进去 ———

    #[test]
    fn merging_adds_the_stations_this_profile_does_not_have() {
        let mut profile = Profile::named("默认");
        let done = merge(&mut profile, &[local("ZSPD", "127.850")], false, &[]);
        assert_eq!(done.added, ["ZSPD_ATIS"]);
        assert!(profile.get("ZSPD_ATIS").is_some());
    }

    /// 本地那份可能是值班时手改过的，一律盖掉等于把人家的活删了。
    #[test]
    fn an_existing_station_is_kept_unless_overwriting_was_asked_for() {
        let mut profile = Profile::named("默认");
        profile.add(local("ZSPD", "127.850")).unwrap();

        let done = merge(&mut profile, &[local("ZSPD", "128.000")], false, &[]);
        assert_eq!(done.kept, ["ZSPD_ATIS"]);
        assert_eq!(profile.get("ZSPD_ATIS").unwrap().frequency, "127.850");

        let done = merge(&mut profile, &[local("ZSPD", "128.000")], true, &[]);
        assert_eq!(done.replaced, ["ZSPD_ATIS"]);
        assert_eq!(profile.get("ZSPD_ATIS").unwrap().frequency, "128.000");
    }

    /// 播了一半把字母退回 A，飞行员报的和听到的就对不上了。
    #[test]
    fn overwriting_keeps_the_local_information_letter() {
        let mut profile = Profile::named("默认");
        let mut here = local("ZSPD", "127.850");
        here.set_letter('Q');
        profile.add(here).unwrap();

        merge(&mut profile, &[local("ZSPD", "128.000")], true, &[]);
        assert_eq!(profile.get("ZSPD_ATIS").unwrap().letter, 'Q');
    }

    /// 播出中的席位被 FSD 那条连接拿着，换掉它只会让在播内容和界面显示的稿子
    /// 对不上。
    #[test]
    fn a_station_that_is_on_the_air_is_never_touched_even_when_overwriting() {
        let mut profile = Profile::named("默认");
        profile.add(local("ZSPD", "127.850")).unwrap();

        let done = merge(
            &mut profile,
            &[local("ZSPD", "128.000")],
            true,
            &["ZSPD_ATIS".to_string()],
        );
        assert_eq!(done.skipped, ["ZSPD_ATIS"]);
        assert_eq!(profile.get("ZSPD_ATIS").unwrap().frequency, "127.850");
    }

    fn three_way() -> Comparison {
        Comparison {
            missing: vec![local("ZGGG", "126.500")],
            differing: vec![local("ZBAA", "128.000")],
            same: vec![local("ZSPD", "127.850")],
        }
    }

    fn callsigns(stations: &[Station]) -> Vec<String> {
        stations.iter().map(Station::callsign).collect()
    }

    #[test]
    fn only_what_was_ticked_is_merged() {
        let c = three_way();
        assert_eq!(callsigns(&chosen(&c, true, false)), ["ZGGG_ATIS"]);
        assert_eq!(callsigns(&chosen(&c, false, true)), ["ZBAA_ATIS"]);
        assert_eq!(
            callsigns(&chosen(&c, true, true)),
            ["ZGGG_ATIS", "ZBAA_ATIS"]
        );
        assert!(chosen(&c, false, false).is_empty());
    }

    #[test]
    fn taking_everything_offered_settles_the_version() {
        let c = three_way();
        let done = Merged {
            added: vec!["ZGGG_ATIS".into()],
            replaced: vec!["ZBAA_ATIS".into()],
            ..Merged::default()
        };
        assert!(done.settles(&c, true, true));
    }

    /// 没勾覆盖的话，那份差异下次还要让人再看一遍。
    #[test]
    fn declining_to_overwrite_leaves_the_version_unsettled() {
        let c = three_way();
        let done = Merged {
            added: vec!["ZGGG_ATIS".into()],
            ..Merged::default()
        };
        assert!(!done.settles(&c, true, false));
    }

    /// 旧版漏了这一条：没勾补缺也记下了版本号，于是缺的那几个席位再也不会被提起。
    #[test]
    fn declining_to_add_what_is_missing_leaves_it_unsettled_too() {
        let c = Comparison {
            missing: vec![local("ZGGG", "126.500")],
            ..Comparison::default()
        };
        assert!(!Merged::default().settles(&c, false, false));
    }

    #[test]
    fn a_station_skipped_because_it_was_on_the_air_leaves_it_unsettled() {
        let c = three_way();
        let done = Merged {
            added: vec!["ZGGG_ATIS".into()],
            skipped: vec!["ZBAA_ATIS".into()],
            ..Merged::default()
        };
        assert!(!done.settles(&c, true, true));
    }

    #[test]
    fn nothing_to_do_at_all_is_already_settled() {
        let c = Comparison {
            same: vec![local("ZSPD", "127.850")],
            ..Comparison::default()
        };
        assert!(Merged::default().settles(&c, false, false));
    }

    // ——— 真的去问一次 ———

    async fn serve(body: &'static str, status: &'static str) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
        format!("http://{addr}/api/v1/atis/config")
    }

    #[tokio::test]
    async fn a_document_that_comes_back_is_handed_over_whole() {
        let url = serve(r#"{"version":"abc","stations":[]}"#, "200 OK").await;
        let got = fetch(&reqwest::Client::new(), &url)
            .await
            .expect("document");
        assert_eq!(got["version"], json!("abc"));
    }

    /// 被限流和"服务器坏了"要分开说：前者过一会儿再按一次就好。
    #[tokio::test]
    async fn being_rate_limited_says_so_rather_than_naming_a_status_code() {
        let url = serve("{}", "429 Too Many Requests").await;
        assert_eq!(
            fetch(&reqwest::Client::new(), &url).await,
            Err(NetConfigError::RateLimited)
        );
    }

    #[tokio::test]
    async fn any_other_bad_status_names_the_code_and_the_address() {
        let url = serve("{}", "503 Service Unavailable").await;
        assert!(matches!(
            fetch(&reqwest::Client::new(), &url).await,
            Err(NetConfigError::Status { status: 503, .. })
        ));
    }

    /// 用户明确按了"更新配置"，失败就必须说出为什么——否则他只得到一个什么都
    /// 没发生的按钮。查等级那条路可以静默失败，这条不行。
    #[tokio::test]
    async fn a_source_that_cannot_be_reached_says_which_address_it_tried() {
        let err = fetch(&reqwest::Client::new(), "http://127.0.0.1:1/atis")
            .await
            .expect_err("unreachable");
        assert!(format!("{err}").contains("127.0.0.1:1"), "{err}");
        assert_eq!(err.message().values["url"], "http://127.0.0.1:1/atis");
    }

    #[tokio::test]
    async fn something_that_is_not_json_says_that_rather_than_a_parse_trace() {
        let url = serve("<html>404</html>", "200 OK").await;
        assert!(matches!(
            fetch(&reqwest::Client::new(), &url).await,
            Err(NetConfigError::NotJson(_))
        ));
    }
}
