//! 导入 vATIS 的配置文件。
//!
//! 模板语法本来就是照搬 vATIS 的（见 [`crate::template`]），**导入的价值正在
//! 这里**：一个已经在别处把预设和缩略语调顺的人，不该为了换一个客户端再手打
//! 一遍。
//!
//! vATIS 的 profile 是 JSON，字段名 camelCase、枚举序列化成字符串
//! （`vATIS.Desktop/SourceGenerationContext.cs` 的 `JsonSourceGenerationOptions`）：
//!
//! ```text
//! {
//!   "name": "配置名",
//!   "stations": [                     // 旧版叫 composites
//!     {
//!       "identifier": "KLAX",
//!       "name": "Los Angeles",
//!       "atisType": "Combined",       // Combined / Departure / Arrival
//!       "codeRange": {"low": "A", "high": "Z"},
//!       "frequency": 135700000,       // uint，单位是赫兹
//!       "presets":      [{"name": …, "template": …, "airportConditions": …, "notams": …}],
//!       "contractions": [{"variableName": …, "text": …, "voice": …}]
//!     }
//!   ]
//! }
//! ```
//!
//! # 频率要换算，不是照抄
//!
//! vATIS 存的是**赫兹的整数**（`135700000`），而 [`Station::frequency`] 是一个
//! 十进制兆赫的**字符串**（`"135.700"`）——`the_frequency_is_a_number_of_kilohertz_and_nothing_else`
//! 钉着这件事。照抄过来的话 `frequency_khz()` 会算出 135_700_000_000，那是一个
//! 合法的路由键，于是席位安静地挂在一个谁也不在的频率上。
//!
//! # 没有对应实现的东西要报出来
//!
//! vATIS 有而这边没有的（细粒度格式设置、IDS 端点、语音录制……）原样跳过，并在
//! 结果里列出来——免得用户以为全都导进来了，到了台上才发现少一半。

use crate::profile::{AtisType, Preset, Station};
use crate::template::Contractions;
use serde_json::Value;

/// vATIS 里有、这边没有对应实现的字段。导入时跳过并提示。
const UNSUPPORTED: &[(&str, &str)] = &[
    (
        "atisFormat",
        "细粒度的格式设置（风、能见度等各自的读法选项）",
    ),
    ("idsEndpoint", "IDS 推送端点"),
    ("airportConditionDefinitions", "预置的机场条件短语库"),
    ("notamDefinitions", "预置的 NOTAM 短语库"),
    ("atisVoice", "语音录制设置"),
    ("externalGenerator", "外部生成器"),
];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ImportError {
    #[error("打不开文件：{0}")]
    Unreadable(String),
    #[error("不是合法的 JSON：{0}")]
    NotJson(String),
    #[error("这个文件看起来不是 vATIS 的配置")]
    NotAProfile,
    #[error("配置里没有任何席位（stations / composites 都是空的）")]
    NoStations,
    #[error("席位缺少 identifier")]
    NoIdentifier,
    #[error("频率 {0} 无法识别")]
    NotAFrequency(String),
    #[error("频率 {0} 超出甚高频范围")]
    OutOfBand(String),
    #[error("没有能导入的席位：{0}")]
    NothingUsable(String),
}

/// 导进来的一份配置。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Imported {
    /// vATIS 那份配置自己的名字。
    pub name: String,
    pub stations: Vec<Station>,
    /// 给用户看的提示：哪些席位没进来、哪些设置这边没有对应功能。
    pub notes: Vec<String>,
}

/// vATIS 的频率 → 这边的十进制兆赫字符串。
///
/// vATIS 存的是赫兹。千赫和兆赫的写法也认——手改过的文件里三种都见过，而认错
/// 单位的后果是席位挂在一个谁也不在的频率上。
pub fn frequency_from(value: &Value) -> Result<String, ImportError> {
    let number = value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .ok_or_else(|| ImportError::NotAFrequency(value.to_string()))?;

    let mhz = if number >= 1_000_000.0 {
        number / 1_000_000.0
    } else if number >= 100_000.0 {
        number / 1_000.0
    } else {
        number
    };
    let spelled = format!("{mhz:.3}");
    if !(100.0..=200.0).contains(&mhz) {
        return Err(ImportError::OutOfBand(spelled));
    }
    Ok(spelled)
}

fn text(entry: &Value, key: &str) -> String {
    entry
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// vATIS 的枚举既可能是字符串也可能是数字（旧版本），两种都认。
fn atis_type_from(value: Option<&Value>) -> AtisType {
    let spelled = match value {
        Some(Value::String(s)) => s.trim().to_lowercase(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    };
    match spelled.as_str() {
        "departure" | "1" => AtisType::Departure,
        "arrival" | "2" => AtisType::Arrival,
        _ => AtisType::Combined,
    }
}

fn code_range_from(entry: &Value) -> (char, char) {
    let range = entry.get("codeRange");
    let end = |key: &str, fallback: char| {
        range
            .and_then(|r| r.get(key))
            .and_then(Value::as_str)
            .and_then(|s| s.trim().chars().next())
            .map(|c| c.to_ascii_uppercase())
            .unwrap_or(fallback)
    };
    (end("low", 'A'), end("high", 'Z'))
}

fn presets_from(entries: Option<&Value>) -> Vec<Preset> {
    let Some(list) = entries.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut ordered: Vec<&Value> = list.iter().collect();
    ordered.sort_by_key(|e| e.get("ordinal").and_then(Value::as_i64).unwrap_or(0));
    ordered
        .into_iter()
        .filter_map(|entry| {
            // 没有模板的预设导进来也没用——它渲染出来是一份空稿子。
            let template = entry.get("template").and_then(Value::as_str)?;
            if template.is_empty() {
                return None;
            }
            let name = text(entry, "name");
            Some(Preset {
                name: if name.is_empty() {
                    "未命名".into()
                } else {
                    name
                },
                template: template.to_string(),
                airport_conditions: text(entry, "airportConditions"),
                notams: text(entry, "notams"),
                ..Preset::default()
            })
        })
        .collect()
}

fn contractions_from(entries: Option<&Value>) -> Contractions {
    let mut out = Contractions::new();
    let Some(list) = entries.and_then(Value::as_array) else {
        return out;
    };
    for entry in list {
        let name = text(entry, "variableName");
        if name.is_empty() {
            continue;
        }
        // 旧版字段叫 string / spoken。
        let written = match text(entry, "text") {
            t if t.is_empty() => text(entry, "string"),
            t => t,
        };
        let spoken = match text(entry, "voice") {
            v if v.is_empty() => text(entry, "spoken"),
            v => v,
        };
        out.insert(name, (written, spoken));
    }
    out
}

/// 一个 vATIS 席位 → 一个 [`Station`]，外加这个席位上跳过了什么。
pub fn parse_station(entry: &Value) -> Result<(Station, Vec<&'static str>), ImportError> {
    let identifier = text(entry, "identifier");
    if identifier.is_empty() {
        return Err(ImportError::NoIdentifier);
    }

    let mut station = Station::new(&identifier);
    station.name = text(entry, "name");
    station.frequency = frequency_from(entry.get("frequency").unwrap_or(&Value::Null))?;
    station.atis_type = atis_type_from(entry.get("atisType"));
    station.code_range = code_range_from(entry);
    station.contractions = contractions_from(entry.get("contractions"));
    let presets = presets_from(entry.get("presets"));
    if !presets.is_empty() {
        station.presets = presets;
    }
    // 识别码大写、字母落回范围、坐标按 ICAO 补上——手写的 json 也要能用，
    // 别人导出的更是。
    station.normalise();

    let skipped = UNSUPPORTED
        .iter()
        .filter(|(key, _)| entry.get(*key).is_some_and(|v| !v.is_null()))
        .map(|(_, description)| *description)
        .collect();
    Ok((station, skipped))
}

/// 一份 vATIS 配置文档 → [`Imported`]。单个席位坏掉不连累整份。
pub fn parse(document: &Value) -> Result<Imported, ImportError> {
    if !document.is_object() {
        return Err(ImportError::NotAProfile);
    }
    // 旧版把席位叫 composites。
    let entries = document
        .get("stations")
        .or_else(|| document.get("composites"))
        .and_then(Value::as_array)
        .filter(|list| !list.is_empty())
        .ok_or(ImportError::NoStations)?;

    let mut stations = Vec::new();
    let mut failures = Vec::new();
    let mut skipped = std::collections::BTreeSet::new();
    for entry in entries {
        match parse_station(entry) {
            Ok((station, unsupported)) => {
                stations.push(station);
                skipped.extend(unsupported);
            }
            Err(e) => failures.push(e.to_string()),
        }
    }
    if stations.is_empty() {
        return Err(ImportError::NothingUsable(first_few(&failures)));
    }

    let mut notes = Vec::new();
    if !failures.is_empty() {
        notes.push(format!(
            "{} 个席位无法导入：{}",
            failures.len(),
            first_few(&failures)
        ));
    }
    if !skipped.is_empty() {
        notes.push(format!(
            "以下 vATIS 设置本客户端没有对应功能，已跳过：{}",
            skipped.into_iter().collect::<Vec<_>>().join("、")
        ));
    }
    Ok(Imported {
        name: text(document, "name"),
        stations,
        notes,
    })
}

/// 报前三条。全列出来的话一份坏文件会刷满整个对话框。
fn first_few(problems: &[String]) -> String {
    problems
        .iter()
        .take(3)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("；")
}

/// 一份 vATIS 配置的文本 → [`Imported`]。
///
/// 给桌面端用：文件是界面那一侧读出来再递过来的，没有路径。
pub fn from_text(body: &str) -> Result<Imported, ImportError> {
    // **vATIS 写的文件带 UTF-8 BOM。** 连着它交给 serde_json，报的是
    // "expected value at line 1 column 1"——看起来像文件本身坏了。
    let body = body.strip_prefix('\u{feff}').unwrap_or(body);
    let document: Value =
        serde_json::from_str(body).map_err(|e| ImportError::NotJson(e.to_string()))?;
    parse(&document)
}

/// 读一个 vATIS profile 文件。
pub fn load(path: impl AsRef<std::path::Path>) -> Result<Imported, ImportError> {
    let raw = std::fs::read(path).map_err(|e| ImportError::Unreadable(e.to_string()))?;
    from_text(&String::from_utf8_lossy(&raw))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn station(extra: Value) -> Value {
        let mut base = json!({
            "identifier": "KLAX",
            "name": "Los Angeles",
            "atisType": "Combined",
            "frequency": 135_700_000u64,
            "presets": [{"name": "默认", "template": "[WIND] [VIS]"}],
        });
        let (Value::Object(base_map), Value::Object(extra_map)) = (&mut base, extra) else {
            unreachable!()
        };
        base_map.extend(extra_map);
        base
    }

    #[test]
    fn a_vatis_frequency_is_hertz() {
        assert_eq!(frequency_from(&json!(135_700_000u64)).unwrap(), "135.700");
    }

    #[test]
    fn the_kilohertz_and_megahertz_spellings_are_tolerated_too() {
        assert_eq!(frequency_from(&json!(135_700)).unwrap(), "135.700");
        assert_eq!(frequency_from(&json!(135.7)).unwrap(), "135.700");
    }

    /// 这条是整个导入里最要紧的一行：照抄赫兹过来的话 `frequency_khz()` 算出
    /// 的是 135_700_000_000——一个合法的路由键，于是席位安静地挂在一个谁也不在
    /// 的频率上。
    #[test]
    fn the_imported_frequency_is_one_the_station_can_read_back_as_kilohertz() {
        let (station, _) = parse_station(&station(json!({}))).unwrap();
        assert_eq!(station.frequency_khz(), Some(135_700));
    }

    #[test]
    fn a_frequency_outside_the_vhf_band_is_refused() {
        assert_eq!(
            frequency_from(&json!(1_000)),
            Err(ImportError::OutOfBand("1000.000".into()))
        );
    }

    #[test]
    fn something_that_is_not_a_number_is_not_a_frequency() {
        assert_eq!(
            frequency_from(&json!("哪个频率")),
            Err(ImportError::NotAFrequency("\"哪个频率\"".into()))
        );
    }

    #[test]
    fn the_atis_type_is_read_from_either_spelling() {
        let (s, _) = parse_station(&station(json!({"atisType": "Departure"}))).unwrap();
        assert_eq!(s.atis_type, AtisType::Departure);
        let (s, _) = parse_station(&station(json!({"atisType": 2}))).unwrap();
        assert_eq!(s.atis_type, AtisType::Arrival);
    }

    #[test]
    fn an_atis_type_nobody_recognises_is_a_combined_one() {
        let (s, _) = parse_station(&station(json!({"atisType": "Whatever"}))).unwrap();
        assert_eq!(s.atis_type, AtisType::Combined);
    }

    #[test]
    fn presets_come_in_the_order_vatis_ordinals_put_them() {
        let (s, _) = parse_station(&station(json!({"presets": [
            {"name": "第二", "template": "b", "ordinal": 2},
            {"name": "第一", "template": "a", "ordinal": 1},
        ]})))
        .unwrap();
        let names: Vec<&str> = s.presets.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["第一", "第二"]);
    }

    /// 没有模板的预设导进来也没用——它渲染出来是一份空稿子。
    #[test]
    fn a_preset_with_no_template_is_not_worth_importing() {
        let (s, _) = parse_station(&station(json!({"presets": [
            {"name": "空的"},
            {"name": "有用的", "template": "[WIND]"},
        ]})))
        .unwrap();
        assert_eq!(s.presets.len(), 1);
        assert_eq!(s.presets[0].name, "有用的");
    }

    #[test]
    fn a_station_with_no_presets_at_all_still_gets_a_usable_one() {
        let (s, _) = parse_station(&station(json!({"presets": []}))).unwrap();
        assert_eq!(s.presets.len(), 1);
    }

    #[test]
    fn contractions_are_read_under_both_the_new_and_the_old_field_names() {
        let (s, _) = parse_station(&station(json!({"contractions": [
            {"variableName": "TL", "text": "TL 90", "voice": "transition level niner zero"},
            {"variableName": "OLD", "string": "老写法", "spoken": "the old spelling"},
        ]})))
        .unwrap();
        assert_eq!(
            s.contractions.get("TL"),
            Some(&("TL 90".into(), "transition level niner zero".into()))
        );
        assert_eq!(
            s.contractions.get("OLD"),
            Some(&("老写法".into(), "the old spelling".into()))
        );
    }

    #[test]
    fn a_code_range_that_is_not_given_is_a_through_z() {
        let (s, _) = parse_station(&station(json!({}))).unwrap();
        assert_eq!(s.code_range, ('A', 'Z'));
        let (s, _) =
            parse_station(&station(json!({"codeRange": {"low": "d", "high": "m"}}))).unwrap();
        assert_eq!(s.code_range, ('D', 'M'));
    }

    #[test]
    fn a_station_with_no_identifier_is_not_a_station() {
        assert_eq!(
            parse_station(&json!({"frequency": 135_700_000u64})),
            Err(ImportError::NoIdentifier)
        );
    }

    /// 用户以为全都导进来了、到了台上才发现少一半，比当场少导一半糟得多。
    #[test]
    fn what_this_client_has_no_answer_for_is_reported_rather_than_dropped_in_silence() {
        let (_, skipped) =
            parse_station(&station(json!({"idsEndpoint": "https://ids.example/"}))).unwrap();
        assert_eq!(skipped, ["IDS 推送端点"]);
    }

    #[test]
    fn the_old_composites_key_is_still_a_list_of_stations() {
        let imported = parse(&json!({"name": "旧版", "composites": [station(json!({}))]})).unwrap();
        assert_eq!(imported.name, "旧版");
        assert_eq!(imported.stations.len(), 1);
    }

    /// 一个坏席位不该让另外二十个也进不来。
    #[test]
    fn one_unusable_station_does_not_sink_the_other_ones() {
        let imported = parse(&json!({"stations": [
            {"name": "没有识别码"},
            station(json!({})),
        ]}))
        .unwrap();
        assert_eq!(imported.stations.len(), 1);
        assert!(
            imported.notes.iter().any(|n| n.contains("1")),
            "{:?}",
            imported.notes
        );
    }

    #[test]
    fn a_document_with_no_stations_at_all_is_refused() {
        assert_eq!(
            parse(&json!({"name": "空的"})),
            Err(ImportError::NoStations)
        );
    }

    #[test]
    fn a_document_that_is_not_an_object_is_not_a_vatis_profile() {
        assert_eq!(parse(&json!([1, 2, 3])), Err(ImportError::NotAProfile));
    }

    /// vATIS 写的文件带 UTF-8 BOM。连着 BOM 交给 serde_json 会当场说不是 JSON。
    #[test]
    fn a_file_with_a_byte_order_mark_still_reads() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.temp/vatis-tests");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("bom.json");
        let body =
            serde_json::to_string(&json!({"name": "带 BOM", "stations": [station(json!({}))]}))
                .expect("json");
        std::fs::write(&path, format!("\u{feff}{body}")).expect("write");

        let imported = load(&path).expect("imported");
        assert_eq!(imported.name, "带 BOM");
        let _ = std::fs::remove_file(&path);
    }

    /// 界面那一侧读文件时，BOM 可能已经被解码成一个 U+FEFF 字符留在开头，
    /// 也可能已经被剥掉。两种都要能读。
    #[test]
    fn text_handed_over_with_or_without_a_leading_byte_order_mark_reads() {
        let body = serde_json::to_string(&json!({"stations": [station(json!({}))]})).unwrap();
        assert_eq!(from_text(&body).unwrap().stations.len(), 1);
        assert_eq!(
            from_text(&format!("\u{feff}{body}"))
                .unwrap()
                .stations
                .len(),
            1
        );
    }

    #[test]
    fn text_that_is_not_json_says_so() {
        assert!(matches!(from_text("<xml/>"), Err(ImportError::NotJson(_))));
    }

    #[test]
    fn a_file_that_is_not_there_says_so_rather_than_panicking() {
        assert!(matches!(
            load("/no/such/vatis/profile.json"),
            Err(ImportError::Unreadable(_))
        ));
    }
}
