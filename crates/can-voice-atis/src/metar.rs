//! METAR 解析：把电码拆成一个个气象要素。
//!
//! 按 vATIS 的做法，每个要素同时有两种形态：
//!
//! ```text
//! text   照抄电码，给文字通播看的      "09004MPS"
//! voice  念出来的样子，给语音合成用的  "wind zero niner zero degrees four meters per second"
//! ```
//!
//! 模板里 `[WIND]` 取 text、`[WIND:VOX]` 取 voice。两者分开是因为文字通播要紧凑、
//! 语音要能听懂——这也是 vATIS 模板变量 `:VOX` 后缀的由来。
//!
//! 只解析 ATIS 用得上的组，**认不出来的组直接跳过**，不会因为一个奇怪的组就让
//! 整份报文解析失败。一份读不出来的通播比一份少一个要素的通播糟得多。
//!
//! 照着 `can-audio/atis/metar.py` 移植。度数和风速**逐位念**（`one four zero`），
//! 不能交给 TTS 去读 `140`——那会念成 "one hundred forty"，不是航空用语。

use crate::readback;
use crate::voicefix::spell_digits;
use regex::Regex;
use std::sync::LazyLock;

/// 云量。
fn cloud_cover(code: &str) -> Option<&'static str> {
    Some(match code {
        "FEW" => "few",
        "SCT" => "scattered",
        "BKN" => "broken",
        "OVC" => "overcast",
        "VV" => "vertical visibility",
        _ => return None,
    })
}

fn cloud_type(code: &str) -> Option<&'static str> {
    Some(match code {
        "CB" => "cumulonimbus",
        "TCU" => "towering cumulus",
        _ => return None,
    })
}

/// 天气现象电码。强度前缀和现象本身共用一张表，和 Python 版一致。
fn weather_code(code: &str) -> Option<&'static str> {
    Some(match code {
        "-" => "light",
        "+" => "heavy",
        "VC" => "in the vicinity",
        "MI" => "shallow",
        "BC" => "patches of",
        "PR" => "partial",
        "DR" => "low drifting",
        "BL" => "blowing",
        "SH" => "showers of",
        "TS" => "thunderstorm",
        "FZ" => "freezing",
        "DZ" => "drizzle",
        "RA" => "rain",
        "SN" => "snow",
        "SG" => "snow grains",
        "PL" => "ice pellets",
        "GR" => "hail",
        "GS" => "small hail",
        "BR" => "mist",
        "FG" => "fog",
        "FU" => "smoke",
        "HZ" => "haze",
        "DU" => "dust",
        "SA" => "sand",
        "SQ" => "squalls",
        "VA" => "volcanic ash",
        "PO" => "dust whirls",
        "FC" => "funnel cloud",
        "SS" => "sandstorm",
        "DS" => "duststorm",
        _ => return None,
    })
}

static WIND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d{3}|VRB)(\d{2,3})(?:G(\d{2,3}))?(KT|MPS|KMH)$").expect("WIND")
});
static WIND_VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{3})V(\d{3})$").expect("WIND_VAR"));
static CLOUD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(FEW|SCT|BKN|OVC|VV)(\d{3}|///)(CB|TCU)?$").expect("CLOUD"));
static TEMP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(M?\d{2})/(M?\d{2})$").expect("TEMP"));
static TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{2})(\d{2})(\d{2})Z$").expect("TIME"));
static RVR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^R(\d{2}[LCR]?)/([PM]?)(\d{4})(?:V([PM]?)(\d{4}))?([UDN])?(FT)?$").expect("RVR")
});
static WEATHER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(-|\+|VC)?([A-Z]{2,8})$").expect("WEATHER"));
static VIS_METRES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{4}$").expect("VIS_METRES"));
static HPA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Q\d{4}$").expect("HPA"));
static INHG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^A\d{4}$").expect("INHG"));

const TREND_STARTS: [&str; 4] = ["NOSIG", "TEMPO", "BECMG", "RMK"];

/// 情报字母 → 通话字母表的词。不认识的原样返回。
///
/// 表在 [`crate::readback`]，通播念的是 "INFORMATION ALPHA"——直接给 TTS 一个
/// 孤零零的 `A`，念出来是"诶"。
pub fn spell_letter(letter: &str) -> String {
    let t = letter.trim();
    let mut chars = t.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => readback::nato_word(c)
            .map(str::to_string)
            .unwrap_or_else(|| t.to_string()),
        _ => t.to_string(),
    }
}

/// 把数字逐位念出来：`090` → `zero niner zero`。非数字原样保留。
///
/// 就是 [`crate::voicefix::spell_digits`]——**同一张表只该有一份**，
/// Python 那边两个模块各写了一份一模一样的 `DIGITS`。
pub fn spell(text: &str) -> String {
    spell_digits(text)
}

/// 带正负号的整数：`-3` → `minus three`，`25` → `two five`。
pub fn spell_number(value: i32) -> String {
    if value < 0 {
        format!("minus {}", spell(&value.abs().to_string()))
    } else {
        spell(&value.to_string())
    }
}

/// 高度按航空习惯念：`3000` → `three thousand`，`10000` → `one zero thousand`。
///
/// 逐位念成 `three zero zero zero` 是听不出高度的。
pub fn spell_altitude(feet: i32) -> String {
    let thousands = feet / 1000;
    let hundreds = (feet % 1000) / 100;
    let mut parts: Vec<String> = Vec::new();
    if thousands != 0 {
        parts.push(format!("{} thousand", spell(&thousands.to_string())));
    }
    if hundreds != 0 {
        parts.push(format!("{} hundred", spell(&hundreds.to_string())));
    }
    if parts.is_empty() {
        spell(&feet.to_string())
    } else {
        parts.join(" ")
    }
}

/// 一个要素的文本形态和语音形态。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Element {
    pub text: String,
    pub voice: String,
}

impl Element {
    /// 语音留空时就取文本形态——照搬 Python 的 `voice or text`。
    pub fn new(text: impl Into<String>, voice: impl Into<String>) -> Self {
        let text = text.into();
        let voice = voice.into();
        let voice = if voice.is_empty() {
            text.clone()
        } else {
            voice
        };
        Self { text, voice }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::new(text, "")
    }

    /// 空要素。Python 那边靠 `__bool__`，这里显式一点。
    pub fn is_empty(&self) -> bool {
        self.text.is_empty() && self.voice.is_empty()
    }
}

fn join(elements: &[Element]) -> Element {
    if elements.is_empty() {
        return Element::default();
    }
    Element::new(
        elements
            .iter()
            .map(|e| e.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        elements
            .iter()
            .map(|e| e.voice.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn wind_element(token: &str, variation: Option<(&str, &str)>) -> Option<Element> {
    let c = WIND.captures(token)?;
    let direction = c.get(1)?.as_str();
    let speed: u32 = c.get(2)?.as_str().parse().ok()?;
    let gust = c.get(3).map(|m| m.as_str());
    let unit_voice = match c.get(4)?.as_str() {
        "KT" => "knots",
        "MPS" => "meters per second",
        "KMH" => "kilometers per hour",
        _ => return None,
    };

    let mut text = token.to_string();
    let mut voice = if direction != "VRB" && speed == 0 {
        "wind calm".to_string()
    } else if direction == "VRB" {
        format!("wind variable {} {unit_voice}", spell(&speed.to_string()))
    } else {
        // 念法照本网通播的稿子：WIND 140 DEGREES 5 METRES PER SECOND。
        format!(
            "wind {} degrees {} {unit_voice}",
            spell(direction),
            spell(&speed.to_string())
        )
    };

    if let Some(g) = gust {
        let g: u32 = g.parse().ok()?;
        voice.push_str(&format!(" gusting {} {unit_voice}", spell(&g.to_string())));
    }

    if let Some((low, high)) = variation {
        text.push_str(&format!(" {low}V{high}"));
        voice.push_str(&format!(
            " variable between {} and {}",
            spell(low),
            spell(high)
        ));
    }

    Some(Element::new(text, voice))
}

fn visibility_element(token: &str) -> Option<Element> {
    if token == "9999" {
        return Some(Element::new(
            token,
            "visibility one zero kilometers or more",
        ));
    }
    if VIS_METRES.is_match(token) {
        let metres: u32 = token.parse().ok()?;
        return Some(if metres % 1000 == 0 {
            Element::new(
                token,
                format!(
                    "visibility {} kilometers",
                    spell(&(metres / 1000).to_string())
                ),
            )
        } else {
            Element::new(
                token,
                format!("visibility {} meters", spell(&metres.to_string())),
            )
        });
    }
    if let Some(miles) = token.strip_suffix("SM") {
        return Some(Element::new(
            token,
            format!("visibility {miles} statute miles"),
        ));
    }
    None
}

fn rvr_element(token: &str) -> Option<Element> {
    let c = RVR.captures(token)?;
    let runway = c.get(1)?.as_str();
    let prefix = c.get(2).map(|m| m.as_str()).unwrap_or("");
    let value: u32 = c.get(3)?.as_str().parse().ok()?;
    let trend = c.get(6).map(|m| m.as_str()).unwrap_or("");
    let unit = if c.get(7).is_some() { "feet" } else { "meters" };

    let mut voice = format!("runway {} visual range", spell(runway));
    match prefix {
        "P" => voice.push_str(" more than"),
        "M" => voice.push_str(" less than"),
        _ => {}
    }
    voice.push_str(&format!(" {} {unit}", spell(&value.to_string())));
    match trend {
        "U" => voice.push_str(" increasing"),
        "D" => voice.push_str(" decreasing"),
        _ => {}
    }
    Some(Element::new(token, voice))
}

fn weather_element(token: &str) -> Option<Element> {
    let c = WEATHER.captures(token)?;
    let intensity = c.get(1).map(|m| m.as_str());
    let body = c.get(2)?.as_str();
    if body.len() % 2 != 0 {
        return None;
    }
    let pairs: Vec<&str> = body
        .as_bytes()
        .chunks(2)
        .map(|p| std::str::from_utf8(p).unwrap_or(""))
        .collect();
    if !pairs.iter().all(|p| weather_code(p).is_some()) {
        return None;
    }
    let mut words: Vec<&str> = Vec::new();
    if let Some(i) = intensity {
        words.push(weather_code(i)?);
    }
    for p in &pairs {
        words.push(weather_code(p)?);
    }
    Some(Element::new(token, words.join(" ")))
}

fn cloud_element(token: &str) -> Option<Element> {
    if token == "NSC" || token == "NCD" {
        return Some(Element::new(token, "no significant clouds"));
    }
    if token == "SKC" || token == "CLR" {
        return Some(Element::new(token, "sky clear"));
    }
    let c = CLOUD.captures(token)?;
    let mut voice = cloud_cover(c.get(1)?.as_str())?.to_string();
    let height = c.get(2)?.as_str();
    if height != "///" {
        let feet: i32 = height.parse().ok()?;
        voice.push(' ');
        voice.push_str(&spell_altitude(feet * 100));
    }
    if let Some(t) = c.get(3) {
        voice.push(' ');
        voice.push_str(cloud_type(t.as_str())?);
    }
    Some(Element::new(token, voice))
}

fn signed(value: &str) -> i32 {
    match value.strip_prefix('M') {
        Some(rest) => -rest.parse::<i32>().unwrap_or(0),
        None => value.parse::<i32>().unwrap_or(0),
    }
}

/// 解析后的 METAR。每个字段都是 [`Element`]，缺失时是空 `Element`。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Metar {
    pub raw: String,
    pub station: String,
    pub observation_time: Element,
    pub wind: Element,
    pub visibility: Element,
    pub rvr: Element,
    pub present_weather: Element,
    pub clouds: Element,
    pub temperature: Element,
    pub dew_point: Element,
    pub pressure: Element,
    pub trend: Element,
    pub recent_weather: Element,
    pub cavok: bool,
    pub auto: bool,
}

impl Metar {
    pub fn parse(raw: &str) -> Self {
        let mut m = Metar {
            raw: raw.replace('=', "").trim().to_string(),
            ..Default::default()
        };
        m.fill();
        m
    }

    fn fill(&mut self) {
        let raw = self.raw.clone();
        let mut tokens: Vec<&str> = raw.split_whitespace().collect();
        // 有的源会带 METAR / SPECI 前缀。
        while matches!(tokens.first(), Some(&"METAR") | Some(&"SPECI")) {
            tokens.remove(0);
        }
        let Some((station, rest)) = tokens.split_first() else {
            return;
        };
        self.station = (*station).to_string();

        // 风向变化组在报文里跟在风组**后面**，但要并进风组里念，
        // 所以先扫一遍找出来。
        let variation = rest
            .iter()
            .filter_map(|t| WIND_VAR.captures(t))
            .next_back()
            .and_then(|c| {
                Some((
                    c.get(1)?.as_str().to_string(),
                    c.get(2)?.as_str().to_string(),
                ))
            });

        let (mut clouds, mut weather, mut rvrs, mut recent) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let mut trend_tokens: Vec<&str> = Vec::new();
        let mut in_trend = false;

        for token in rest {
            if TREND_STARTS.contains(token) {
                in_trend = true;
            }
            if in_trend {
                trend_tokens.push(token);
                continue;
            }

            if *token == "AUTO" {
                self.auto = true;
                continue;
            }
            if *token == "CAVOK" {
                self.cavok = true;
                self.visibility = Element::new("CAVOK", "cavok");
                continue;
            }
            if WIND_VAR.is_match(token) {
                continue; // 已经并进风组了
            }

            if self.observation_time.is_empty() {
                if let Some(c) = TIME.captures(token) {
                    let hhmm = format!("{}{}", &c[2], &c[3]);
                    self.observation_time = Element::new(*token, format!("time {}", spell(&hhmm)));
                    continue;
                }
            }

            if self.wind.is_empty() {
                if let Some(e) = wind_element(
                    token,
                    variation.as_ref().map(|(a, b)| (a.as_str(), b.as_str())),
                ) {
                    self.wind = e;
                    continue;
                }
            }

            if let Some(e) = rvr_element(token) {
                rvrs.push(e);
                continue;
            }

            if self.visibility.is_empty() && !self.cavok {
                if let Some(e) = visibility_element(token) {
                    self.visibility = e;
                    continue;
                }
            }

            if let Some(e) = cloud_element(token) {
                clouds.push(e);
                continue;
            }

            if let Some(c) = TEMP.captures(token) {
                let (temp, dew) = (c[1].to_string(), c[2].to_string());
                self.temperature = Element::new(
                    temp.clone(),
                    format!("temperature {}", spell_number(signed(&temp))),
                );
                self.dew_point = Element::new(
                    dew.clone(),
                    format!("dewpoint {}", spell_number(signed(&dew))),
                );
                continue;
            }

            if HPA.is_match(token) {
                // 保持字符串：解析成整数会把 Q0995 的前导零吃掉，念出来少一位。
                self.pressure =
                    Element::new(*token, format!("QNH {} hectopascals", spell(&token[1..])));
                continue;
            }
            if INHG.is_match(token) {
                let value = &token[1..];
                self.pressure = Element::new(
                    *token,
                    format!(
                        "altimeter {} point {}",
                        spell(&value[..2]),
                        spell(&value[2..])
                    ),
                );
                continue;
            }

            if let Some(rest) = token.strip_prefix("RE") {
                if let Some(e) = weather_element(rest) {
                    recent.push(Element::new(*token, format!("recent {}", e.voice)));
                    continue;
                }
            }

            if let Some(e) = weather_element(token) {
                weather.push(e);
            }
        }

        self.clouds = join(&clouds);
        self.present_weather = join(&weather);
        self.rvr = join(&rvrs);
        self.recent_weather = join(&recent);
        if !trend_tokens.is_empty() {
            self.trend = Element::new(
                trend_tokens.join(" "),
                if trend_tokens[0] == "NOSIG" {
                    "no significant change"
                } else {
                    ""
                },
            );
        }
    }

    /// 按标准顺序拼起来，对应 vATIS 的 `[WX]` / `[FULL_WX_STRING]`。
    pub fn full_wx(&self) -> Element {
        // 温度和露点在电码里本来就是 25/18 一组，文字形态要拼回去。
        let temp_dew = if !self.temperature.is_empty() && !self.dew_point.is_empty() {
            Element::new(
                format!("{}/{}", self.temperature.text, self.dew_point.text),
                format!("{}, {}", self.temperature.voice, self.dew_point.voice),
            )
        } else if !self.temperature.is_empty() {
            self.temperature.clone()
        } else {
            self.dew_point.clone()
        };

        let order = [
            &self.wind,
            &self.visibility,
            &self.rvr,
            &self.present_weather,
            &self.clouds,
            &temp_dew,
            &self.pressure,
        ];
        let present: Vec<&Element> = order.into_iter().filter(|e| !e.is_empty()).collect();
        Element::new(
            present
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            present
                .iter()
                .map(|e| e.voice.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        )
    }

    pub fn is_valid(&self) -> bool {
        !self.station.is_empty() && (!self.wind.is_empty() || !self.pressure.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一份真实的 ZSPD 报文。`can-audio/atis/test_atis.py` 用的也是这一份，
    /// 每一条断言都是从那边搬过来的——**移植的价值全在这里**：
    /// 两份实现在同一份电码上念出同一句话，才谈得上换掉旧的。
    const ZSPD: &str = "ZSPD 251300Z 09004MPS 9999 FEW030 SCT100 25/18 Q1013 NOSIG";

    #[test]
    fn station_and_time() {
        let m = Metar::parse(ZSPD);
        assert_eq!(m.station, "ZSPD");
        assert_eq!(m.observation_time.text, "251300Z");
        assert_eq!(m.observation_time.voice, "time one three zero zero");
    }

    /// 文字通播照抄电码，语音要念得出来——这就是 `:VOX` 的意义。
    #[test]
    fn wind_has_both_forms() {
        let m = Metar::parse(ZSPD);
        assert_eq!(m.wind.text, "09004MPS");
        assert_eq!(
            m.wind.voice,
            "wind zero niner zero degrees four meters per second"
        );
    }

    #[test]
    fn visibility_and_clouds() {
        let m = Metar::parse(ZSPD);
        assert_eq!(m.visibility.voice, "visibility one zero kilometers or more");
        assert_eq!(m.clouds.text, "FEW030 SCT100");
        assert_eq!(
            m.clouds.voice,
            "few three thousand, scattered one zero thousand"
        );
    }

    #[test]
    fn temperature_and_pressure() {
        let m = Metar::parse(ZSPD);
        assert_eq!(m.temperature.voice, "temperature two five");
        assert_eq!(m.dew_point.voice, "dewpoint one eight");
        assert_eq!(m.pressure.voice, "QNH one zero one three hectopascals");
    }

    #[test]
    fn calm_and_variable_wind() {
        assert_eq!(
            Metar::parse("ZBAA 251300Z 00000MPS 9999 25/18 Q1013")
                .wind
                .voice,
            "wind calm"
        );
        assert_eq!(
            Metar::parse("ZBAA 251300Z VRB02MPS 9999 25/18 Q1013")
                .wind
                .voice,
            "wind variable two meters per second"
        );
    }

    #[test]
    fn gust_and_variation() {
        let wind = Metar::parse("KLAX 251300Z 26015G25KT 220V300 10SM 25/18 A2992").wind;
        assert!(wind.voice.contains("gusting two five knots"), "{wind:?}");
        assert!(
            wind.voice
                .contains("variable between two two zero and three zero zero"),
            "{wind:?}"
        );
        // 变化组并进风组之后，文字形态也要带上它。
        assert_eq!(wind.text, "26015G25KT 220V300");
    }

    #[test]
    fn negative_temperature() {
        let m = Metar::parse("ZYTX 251300Z 09004MPS 9999 M03/M07 A2992");
        assert_eq!(m.temperature.voice, "temperature minus three");
        assert!(
            m.pressure
                .voice
                .contains("altimeter two niner point niner two"),
            "{:?}",
            m.pressure
        );
    }

    #[test]
    fn weather_and_rvr() {
        let m = Metar::parse("ZSPD 251300Z 09004MPS 3000 -SHRA R35L/1200 BKN010 25/18 Q1013");
        assert_eq!(m.present_weather.voice, "light showers of rain");
        assert!(m.rvr.voice.contains("runway three five"), "{:?}", m.rvr);
        assert_eq!(m.visibility.voice, "visibility three kilometers");
    }

    #[test]
    fn cavok_and_cb() {
        assert!(Metar::parse("ZSPD 251300Z 09004MPS CAVOK 25/18 Q1013").cavok);
        let m = Metar::parse("ZSPD 251300Z 09004MPS 9999 BKN020CB 25/18 Q1013");
        assert_eq!(m.clouds.voice, "broken two thousand cumulonimbus");
    }

    /// NOSIG 之后的内容不该混进气象要素里。
    #[test]
    fn trend_is_separated() {
        let m = Metar::parse(ZSPD);
        assert_eq!(m.trend.text, "NOSIG");
        assert!(!m.full_wx().text.contains("NOSIG"));
    }

    #[test]
    fn full_wx_order() {
        let m = Metar::parse(ZSPD);
        let full = m.full_wx();
        assert_eq!(full.text, "09004MPS 9999 FEW030 SCT100 25/18 Q1013");
        let wind = full.voice.find("wind").expect("wind");
        let vis = full.voice.find("visibility").expect("visibility");
        let qnh = full.voice.find("QNH").expect("QNH");
        assert!(wind < vis && vis < qnh, "{}", full.voice);
    }

    /// **认不出来的组不会让整份报文解析失败。**
    /// 一份读不出来的通播比一份少一个要素的通播糟得多。
    #[test]
    fn garbage_does_not_explode() {
        let m = Metar::parse("ZSPD 251300Z 09004MPS 9999 XYZZY123 25/18 Q1013");
        assert!(m.is_valid());
        assert_eq!(m.temperature.voice, "temperature two five");
    }

    #[test]
    fn metar_prefix_and_empty() {
        assert_eq!(
            Metar::parse("METAR ZSPD 251300Z 09004MPS Q1013").station,
            "ZSPD"
        );
        assert!(!Metar::parse("").is_valid());
    }

    /// 前导零不能被吃掉：`Q0995` 念出来必须是四位。
    ///
    /// 把气压当整数解析就会丢掉那个零，而念少一位的修正海压是一条会被照着
    /// 拨进高度表的错误数据。
    #[test]
    fn a_leading_zero_in_the_pressure_survives() {
        let m = Metar::parse("ZSPD 251300Z 09004MPS 9999 25/18 Q0995");
        assert_eq!(m.pressure.voice, "QNH zero niner niner five hectopascals");
    }

    /// 度数和风速逐位念，不能交给 TTS 去读 `140`
    /// ——那会念成 "one hundred forty"，不是航空用语。
    #[test]
    fn wind_says_degrees() {
        let m = Metar::parse("ZSPD 291130Z 14005MPS CAVOK 30/25 Q1010");
        assert_eq!(
            m.wind.voice,
            "wind one four zero degrees five meters per second"
        );
    }

    #[test]
    fn qnh_says_the_unit() {
        let m = Metar::parse("ZSPD 291130Z 14005MPS CAVOK 30/25 Q1010");
        assert_eq!(m.pressure.voice, "QNH one zero one zero hectopascals");
    }

    /// 通播念的是 INFORMATION ALPHA，不是 INFORMATION A。
    ///
    /// 直接把孤零零一个 `A` 交给 TTS，念出来是"诶"——听着不像通播，而且和
    /// 飞行员回报的 "information alpha" 对不上。
    #[test]
    fn every_letter_has_a_word() {
        for letter in "ABCDEFGHIJKLMNOPQRSTUVWXYZ".chars() {
            let s = letter.to_string();
            assert_ne!(spell_letter(&s), s, "{letter}");
        }
        assert_eq!(spell_letter("j"), "Juliett");
        // 认不出的原样返回，而不是消失。
        assert_eq!(spell_letter("42"), "42");
    }

    /// 高度按航空习惯念。逐位念成 `three zero zero zero` 是听不出高度的。
    #[test]
    fn altitudes_are_spoken_the_aviation_way() {
        assert_eq!(spell_altitude(3000), "three thousand");
        assert_eq!(spell_altitude(10000), "one zero thousand");
        assert_eq!(spell_altitude(1500), "one thousand five hundred");
        assert_eq!(spell_altitude(500), "five hundred");
        assert_eq!(spell_altitude(0), "zero");
    }

    /// 云高是**百英尺**，念的是英尺高度：`FEW030` 是三千英尺。
    #[test]
    fn cloud_height_is_hundreds_of_feet() {
        let m = Metar::parse("ZSPD 251300Z 09004MPS 9999 BKN008 25/18 Q1013");
        assert_eq!(m.clouds.voice, "broken eight hundred");
    }

    /// 云高缺测（`///`）时只念云量，不念一个瞎编的高度。
    #[test]
    fn a_missing_cloud_height_is_not_invented() {
        let m = Metar::parse("ZSPD 251300Z 09004MPS 9999 BKN/// 25/18 Q1013");
        assert_eq!(m.clouds.voice, "broken");
    }

    #[test]
    fn recent_weather_is_marked_as_recent() {
        let m = Metar::parse("ZSPD 251300Z 09004MPS 9999 RERA 25/18 Q1013");
        assert_eq!(m.recent_weather.voice, "recent rain");
        assert!(m.present_weather.is_empty(), "{:?}", m.present_weather);
    }

    /// 多个要素之间用逗号隔开，不是空格——TTS 靠标点断句，
    /// 少一个逗号就会把两段念成一个词。
    #[test]
    fn several_elements_of_a_kind_are_separated_by_commas() {
        let m = Metar::parse("ZSPD 251300Z 09004MPS 9999 FEW030 SCT100 BKN200 25/18 Q1013");
        assert_eq!(m.clouds.text, "FEW030 SCT100 BKN200");
        assert_eq!(m.clouds.voice.matches(", ").count(), 2, "{:?}", m.clouds);
    }
}
