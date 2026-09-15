//! 中文通播稿。
//!
//! 英文那套走 vATIS 的模板变量（[`crate::metar`] 里每个元素都带 text 和 voice
//! 两种形态），中文这边**不是逐词翻译**——民航中文通播有自己的语序和读法，
//! 所以单独渲染一遍。
//!
//! 数字读法和 [`crate::readback`] 一致，全网统一：
//!
//! ```text
//! 0 洞   1 幺   2 两   3 三   4 四   5 五   6 六   7 拐   8 八   9 九
//! ```
//!
//! 拐和洞这些不是方言而是无线电通话规范，为的是在嘈杂信道里不会把"七"听成
//! "一"、"零"听成"六"。**改这张表会让管制员听到的和他们受训时的不一样。**
//!
//! 高度在中文通播里念**米**，而 METAR 的云高是百英尺，所以要换算——念英尺的
//! 中文通播在国内是不存在的。
//!
//! 照着 `can-audio/atis/chinese.py` 移植，逐条对照金文件。

use crate::metar::Metar;
use crate::readback::{chinese_letter_word, spell_chinese};
use regex::Regex;
use std::sync::LazyLock;

/// 云高折算。国内通播惯例是 **100 英尺算 30 米**（精确值 30.48），这样
/// `FEW030` 念"九百米"而不是"九百一十米"。
pub const METRES_PER_HUNDRED_FEET: i64 = 30;

fn cloud_amount(code: &str) -> Option<&'static str> {
    Some(match code {
        "FEW" => "少云",
        "SCT" => "疏云",
        "BKN" => "多云",
        "OVC" => "阴天",
        "NSC" => "无重要云",
        "NCD" => "未探测到云",
        "SKC" | "CLR" => "碧空",
        _ => return None,
    })
}

fn cloud_type(code: &str) -> Option<&'static str> {
    Some(match code {
        "CB" => "积雨云",
        "TCU" => "浓积云",
        _ => return None,
    })
}

fn weather_word(code: &str) -> Option<&'static str> {
    Some(match code {
        "RA" => "雨",
        "SN" => "雪",
        "DZ" => "毛毛雨",
        "SG" => "米雪",
        "GR" => "冰雹",
        "GS" => "霰",
        "FG" => "雾",
        "BR" => "轻雾",
        "HZ" => "霾",
        "FU" => "烟",
        "SA" => "沙",
        "DU" => "浮尘",
        "PO" => "尘卷风",
        "SQ" => "飑",
        "FC" => "漏斗云",
        "TS" => "雷暴",
        "SH" => "阵性",
        "FZ" => "冻",
        "BL" => "吹",
        "DR" => "低吹",
        "MI" => "浅",
        "BC" => "散片",
        "PR" => "部分",
        _ => return None,
    })
}

/// 情报字母念成通话字母：`J` → 朱丽叶。认不出的原样返回。
pub fn spell_letter(letter: &str) -> String {
    let t = letter.trim();
    let mut chars = t.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => chinese_letter_word(c)
            .map(str::to_string)
            .unwrap_or_else(|| t.to_uppercase()),
        _ => t.to_uppercase(),
    }
}

/// 念成整数而不是逐位。温度、米数这些用它。
///
/// 只处理 0–9999，通播里不会出现更大的数。
pub fn spell_count(value: i64) -> String {
    if value < 0 {
        return format!("零下 {}", spell_count(-value));
    }
    if value == 0 {
        return "零".to_string();
    }

    const UNITS: [&str; 4] = ["", "十", "百", "千"];
    const DIGITS: [&str; 10] = ["零", "一", "二", "三", "四", "五", "六", "七", "八", "九"];

    let string = value.to_string();
    let length = string.len();
    let mut text = String::new();
    for (index, ch) in string.chars().enumerate() {
        let digit = ch.to_digit(10).unwrap_or(0) as usize;
        let position = length - index - 1;
        if digit != 0 {
            // 一十五说成十五，但一百一十五的"一百"要留。
            if digit == 1 && position == 1 && index == 0 {
                text.push_str(UNITS[position]);
            } else {
                text.push_str(DIGITS[digit]);
                text.push_str(UNITS[position]);
            }
        } else if !text.ends_with('零') && index != length - 1 {
            text.push('零');
        }
    }
    let trimmed = text.trim_end_matches('零');
    if trimmed.is_empty() {
        "零".to_string()
    } else {
        trimmed.to_string()
    }
}

static WIND_VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d{3})V(\d{3})$").expect("WIND_VAR"));
static WIND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(VRB|\d{3})(\d{2,3})(?:G(\d{2,3}))?(MPS|KT)$").expect("WIND"));
static CLOUD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(FEW|SCT|BKN|OVC)(\d{3}|///)(CB|TCU)?$").expect("CLOUD"));
static HPA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Q(\d{3,4})$").expect("HPA"));
static INHG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^A(\d{4})$").expect("INHG"));
static OBS_TIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{2}(\d{4})Z$").expect("OBS_TIME"));

/// `09004MPS` / `VRB02MPS` / `27010G18MPS` / `09004MPS 350V050`
fn wind(token: &str) -> String {
    if token.is_empty() {
        return String::new();
    }
    // [`crate::metar`] 会把风向变化组拼在后面（`09004MPS 350V050`）。这里按空格
    // 拆开分别念——**整串去匹配一个单段的正则，匹配不上就整组静默丢掉**，
    // 带变化组的 METAR 播出来的中文通播完全没有风。Python 版曾经就是这样。
    let pieces: Vec<&str> = token.split_whitespace().collect();
    let Some(head_token) = pieces.first() else {
        return String::new();
    };
    let mut variation = String::new();
    for extra in &pieces[1..] {
        if let Some(c) = WIND_VAR.captures(extra) {
            variation = format!(
                " 风向在 {} 度 和 {} 度 之间变化",
                spell_chinese(&c[1]),
                spell_chinese(&c[2])
            );
        }
    }
    let Some(c) = WIND.captures(head_token) else {
        return String::new();
    };
    let direction = &c[1];
    let speed = &c[2];
    let gust = c.get(3).map(|m| m.as_str());
    let unit = &c[4];

    let head = if direction == "VRB" {
        "风向不定".to_string()
    } else if direction == "000" && speed == "00" {
        return "静风".to_string();
    } else {
        // 真实通播念的是"风向 三洞洞 度，风速 拐 米每秒"，风向和风速各带自己的
        // 名头；只说"风 三洞洞 度 拐 米每秒"是听不出哪个数是什么的。
        format!("风向 {} 度", spell_chinese(direction))
    };

    let measure = if unit == "MPS" {
        "米每秒"
    } else {
        "海里每小时"
    };
    let mut parts = vec![format!(
        "{head} 风速 {} {measure}",
        spell_count(speed.parse().unwrap_or(0))
    )];
    if let Some(g) = gust {
        parts.push(format!(
            "阵风 {} {measure}",
            spell_count(g.parse().unwrap_or(0))
        ));
    }
    format!("{}{variation}", parts.join(" "))
}

/// `9999` / `5000` / `CAVOK`
fn visibility(token: &str) -> String {
    let token = token.trim().to_uppercase();
    if token.is_empty() {
        return String::new();
    }
    if token == "CAVOK" {
        return "能见度 幺洞 公里 以上 云高 幺五洞洞 米 以上".to_string();
    }
    if token == "9999" {
        return "能见度 幺洞 公里 以上".to_string();
    }
    if !token.is_empty() && token.chars().all(|c| c.is_ascii_digit()) {
        let metres: i64 = token.parse().unwrap_or(0);
        if metres >= 1000 && metres % 1000 == 0 {
            return format!("能见度 {} 公里", spell_count(metres / 1000));
        }
        return format!("能见度 {} 米", spell_count(metres));
    }
    String::new()
}

/// `FEW030 SCT100 BKN020CB` —— 云高换算成米。
fn clouds(text: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for token in text.split_whitespace() {
        let token = token.trim().to_uppercase();
        if let Some(word) = cloud_amount(&token) {
            parts.push(word.to_string());
            continue;
        }
        let Some(c) = CLOUD.captures(&token) else {
            continue;
        };
        let Some(amount) = cloud_amount(&c[1]) else {
            continue;
        };
        let mut piece = amount.to_string();
        let height = &c[2];
        if height != "///" {
            let feet: i64 = height.parse().unwrap_or(0);
            piece.push_str(&format!(
                " {} 米",
                spell_count(feet * METRES_PER_HUNDRED_FEET)
            ));
        }
        if let Some(kind) = c.get(3) {
            piece.push(' ');
            piece.push_str(cloud_type(kind.as_str()).unwrap_or(kind.as_str()));
        }
        parts.push(piece);
    }
    parts.join(" ")
}

/// `-RA` / `+TSRA` / `VCSH` —— 拆成强度和现象。
fn weather(text: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for token in text.split_whitespace() {
        let mut token = token.trim().to_uppercase();
        let mut prefix = "";
        if let Some(rest) = token.strip_prefix('-') {
            prefix = "小";
            token = rest.to_string();
        } else if let Some(rest) = token.strip_prefix('+') {
            prefix = "大";
            token = rest.to_string();
        } else if let Some(rest) = token.strip_prefix("VC") {
            prefix = "附近有";
            token = rest.to_string();
        }

        let mut words = String::new();
        while token.len() >= 2 {
            match weather_word(&token[..2]) {
                Some(w) => {
                    words.push_str(w);
                    token = token[2..].to_string();
                }
                None => break,
            }
        }
        if !words.is_empty() {
            parts.push(format!("{prefix}{words}"));
        }
    }
    parts.join(" ")
}

/// `25` → 气温 二十五 摄氏度；`M03` → 露点负 三 摄氏度
///
/// 负号紧跟在名头后面（"露点负 八"），单位要念出来——真实通播就是这么播的。
/// 写成"露点 零下 八"也听得懂，但和实际播出的不是一个说法。
fn temperature(label: &str, text: &str) -> String {
    let text = text.trim().to_uppercase();
    if text.is_empty() {
        return String::new();
    }
    let negative = text.starts_with('M') || text.starts_with('-');
    let digits = text.trim_start_matches(['M', '-']);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return String::new();
    }
    format!(
        "{label}{} {} 摄氏度",
        if negative { "负" } else { "" },
        spell_count(digits.parse().unwrap_or(0))
    )
}

/// `Q1013` / `A2992`
fn pressure(text: &str) -> String {
    let text = text.trim().to_uppercase();
    if let Some(c) = HPA.captures(&text) {
        return format!("修正海压 {} 百帕", spell_chinese(&c[1]));
    }
    if let Some(c) = INHG.captures(&text) {
        let value = &c[1];
        return format!(
            "修正海压 {} 点 {} 英寸汞柱",
            spell_chinese(&value[..2]),
            spell_chinese(&value[2..])
        );
    }
    String::new()
}

/// `251300Z` → 幺三洞洞 世界协调时
///
/// 真实通播报的是"世界协调时"，只说"时"听起来像本地时间。
fn observation_time(text: &str) -> String {
    match OBS_TIME.captures(text.trim()) {
        Some(c) => format!("{} 世界协调时", spell_chinese(&c[1])),
        None => String::new(),
    }
}

/// 一段中文通播稿要说的那几格。
#[derive(Debug, Clone, Default)]
pub struct Script<'a> {
    /// 席位名，比如"上海浦东"。
    pub facility: &'a str,
    /// 情报字母，会念成对应的汉字（A 阿尔法 …）。
    pub letter: &'a str,
    /// 使用跑道，比如"三六左"，调用方自己写好。
    pub runway: &'a str,
    /// 额外说明，接在最后。
    pub extra: &'a str,
}

/// 生成一段中文通播稿。
pub fn render(metar: Option<&Metar>, script: &Script) -> String {
    let spoken_letter = spell_letter(script.letter);
    let mut parts: Vec<String> = Vec::new();
    if !script.facility.is_empty() {
        parts.push(format!("{}情报通播", script.facility));
    }
    if !spoken_letter.is_empty() {
        parts.push(spoken_letter.clone());
    }
    let Some(m) = metar else {
        return parts.join(" ");
    };

    let time_text = observation_time(&m.observation_time.text);
    if !time_text.is_empty() {
        parts.push(time_text);
    }
    if !script.runway.is_empty() {
        // 这一格可以只填跑道号（"三六左"），也可以填一整段构型说明
        // （"跑道独立平行离场，跑道 三六左 起始高度 六百米……"）——真实通播里
        // 跑道构型就是这么一整段，而且位置在气象**之前**。后者自带"跑道"二字，
        // 再套一层"使用跑道"就成了"使用跑道 跑道独立平行离场"。
        parts.push(if script.runway.trim_start().starts_with("跑道") {
            script.runway.to_string()
        } else {
            format!("使用跑道 {}", script.runway)
        });
    }

    for value in [
        wind(&m.wind.text),
        visibility(&m.visibility.text),
        weather(&m.present_weather.text),
        clouds(&m.clouds.text),
    ] {
        if !value.is_empty() {
            parts.push(value);
        }
    }

    for phrase in [
        temperature("气温", &m.temperature.text),
        temperature("露点", &m.dew_point.text),
    ] {
        if !phrase.is_empty() {
            parts.push(phrase);
        }
    }

    let p = pressure(&m.pressure.text);
    if !p.is_empty() {
        parts.push(p);
    }

    if !script.extra.is_empty() {
        parts.push(script.extra.to_string());
    }
    if !spoken_letter.is_empty() {
        // 真实通播的收尾是让机组回报收到了哪一份，不是"完毕"。
        parts.push(format!(
            "首次与管制员联络时报告你已收到通播 {spoken_letter}"
        ));
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(raw: &str) -> Metar {
        Metar::parse(raw)
    }

    /// 云高按 **100 英尺 = 30 米**折算，不是精确的 30.48。
    ///
    /// 真实通播念的是"九百米"这种整数；按精确值算出来的"九百一十米"听着就不像话。
    /// 这是行业惯例而不是四舍五入的误差，所以它是一条定值而不是一个待修的近似。
    #[test]
    fn cloud_height_uses_the_thirty_metre_convention() {
        assert_eq!(clouds("FEW030"), "少云 九百 米");
        assert_eq!(clouds("SCT100"), "疏云 三千 米");
        // 30.48 会给出 914 米。
        assert_ne!(clouds("FEW030"), "少云 九百一十四 米");
    }

    /// **带风向变化组的报文不能整组丢掉风。**
    ///
    /// [`crate::metar`] 把变化组拼在风组文本后面（`09004MPS 350V050`）。拿整串去
    /// 匹配一个单段的正则会匹配不上，于是整段静默返回空——播出来的中文通播
    /// 完全没有风，而英文那份是好的，两种语言互相矛盾。
    #[test]
    fn a_wind_with_a_variation_group_still_speaks_the_wind() {
        let got = wind("09004MPS 350V050");
        assert!(got.contains("风向 洞 九 洞 度"), "{got}");
        assert!(got.contains("风速 四 米每秒"), "{got}");
        assert!(
            got.contains("风向在 三 五 洞 度 和 洞 五 洞 度 之间变化"),
            "{got}"
        );
    }

    #[test]
    fn calm_and_variable_and_gusting() {
        assert_eq!(wind("00000MPS"), "静风");
        assert!(wind("VRB02MPS").starts_with("风向不定"));
        assert!(wind("27010G18MPS").contains("阵风 十八 米每秒"));
        assert!(wind("27010KT").contains("海里每小时"));
        assert_eq!(wind(""), "");
        assert_eq!(wind("不是风组"), "");
    }

    /// 风向逐位念（无线电读法），风速念整数。
    ///
    /// 两个数紧挨着，念法却不同——这不是不一致，是通播的读法：
    /// 风向是三位方位，风速是一个量。
    #[test]
    fn direction_is_spelled_but_speed_is_counted() {
        let got = wind("09004MPS");
        assert!(got.contains("风向 洞 九 洞 度"), "{got}");
        assert!(got.contains("风速 四 米每秒"), "{got}");
    }

    /// 跑道那一格可以填一整段构型说明，**自带"跑道"二字时不要再套一层**。
    ///
    /// 否则念出来是"使用跑道 跑道独立平行离场"。
    #[test]
    fn a_runway_paragraph_is_not_prefixed_again() {
        let script = Script {
            facility: "北京首都",
            letter: "A",
            runway: "跑道独立平行离场，跑道 三六左",
            extra: "",
        };
        let got = render(
            Some(&m("ZBAA 291000Z 30007MPS CAVOK 24/M08 Q1003")),
            &script,
        );
        assert!(got.contains("跑道独立平行离场"), "{got}");
        assert!(!got.contains("使用跑道 跑道"), "{got}");

        let script = Script {
            runway: "三六左",
            ..script
        };
        let got = render(
            Some(&m("ZBAA 291000Z 30007MPS CAVOK 24/M08 Q1003")),
            &script,
        );
        assert!(got.contains("使用跑道 三六左"), "{got}");
    }

    /// 负温的负号紧跟名头（"露点负 八"），单位要念出来。
    /// 真实通播就是这么播的——写成"露点 零下 八"也听得懂，但不是那个说法。
    #[test]
    fn a_negative_temperature_keeps_the_sign_next_to_the_label() {
        assert_eq!(temperature("露点", "M08"), "露点负 八 摄氏度");
        assert_eq!(temperature("气温", "25"), "气温 二十五 摄氏度");
        assert_eq!(temperature("气温", ""), "");
        assert_eq!(temperature("气温", "XX"), "");
    }

    /// 一十五说成十五，**但一百一十五的"一百"要留**。
    ///
    /// 只看 10–19 会以为规则是"开头的一省掉"，那样 110 会念成"十一十"。
    #[test]
    fn the_leading_one_is_dropped_only_at_the_very_front() {
        assert_eq!(spell_count(15), "十五");
        assert_eq!(spell_count(10), "十");
        assert_eq!(spell_count(115), "一百一十五");
        assert_eq!(spell_count(110), "一百一十");
        assert_eq!(spell_count(105), "一百零五");
        assert_eq!(spell_count(1005), "一千零五");
        assert_eq!(spell_count(0), "零");
        assert_eq!(spell_count(-8), "零下 八");
    }

    /// 修正海压逐位念，`Q0995` 的前导零不能掉。
    #[test]
    fn the_pressure_is_spelled_digit_by_digit() {
        assert_eq!(pressure("Q1013"), "修正海压 幺 洞 幺 三 百帕");
        assert_eq!(pressure("Q0995"), "修正海压 洞 九 九 五 百帕");
        assert_eq!(pressure("A2992"), "修正海压 两 九 点 九 两 英寸汞柱");
        assert_eq!(pressure(""), "");
    }

    /// CAVOK 在中文通播里连云高一起交代，不只是能见度。
    #[test]
    fn cavok_says_the_cloud_base_too() {
        assert_eq!(
            visibility("CAVOK"),
            "能见度 幺洞 公里 以上 云高 幺五洞洞 米 以上"
        );
        assert_eq!(visibility("9999"), "能见度 幺洞 公里 以上");
        assert_eq!(visibility("5000"), "能见度 五 公里");
        assert_eq!(visibility("3600"), "能见度 三千六百 米");
    }

    /// 没有报文时只念台名和字母，**不念收尾语**——没有内容可收。
    #[test]
    fn without_a_report_it_says_only_who_and_which_letter() {
        let script = Script {
            facility: "上海浦东",
            letter: "A",
            runway: "三五左",
            extra: "无",
        };
        assert_eq!(render(None, &script), "上海浦东情报通播 阿尔法");
    }

    /// 情报字母念中文通话词。念拉丁字母会让 TTS 在一串汉字中间蹦出一个英文字符。
    #[test]
    fn the_information_letter_is_a_chinese_word() {
        assert_eq!(spell_letter("J"), "朱丽叶");
        assert_eq!(spell_letter("a"), "阿尔法");
        assert_eq!(spell_letter(""), "");
        // 认不出的原样返回，而不是消失。
        assert_eq!(spell_letter("AB"), "AB");
    }

    #[test]
    fn weather_codes_carry_their_intensity() {
        assert_eq!(weather("-RA"), "小雨");
        assert_eq!(weather("+TSRA"), "大雷暴雨");
        assert_eq!(weather("VCSH"), "附近有阵性");
        // 表里没有的电码整段跳过，而不是念出一个电码。
        assert_eq!(weather("PL"), "");
    }
}
