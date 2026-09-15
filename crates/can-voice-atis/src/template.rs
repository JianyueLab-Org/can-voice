//! ATIS 模板渲染。
//!
//! 变量名和写法照搬 vATIS（`vatis.app/docs/client/atis-configuration/presets`）：
//! 模板里写 `[WIND]`、`[CLOUDS]` 这类占位符，生成时替换成实际数据。变量名后面加
//! `:VOX` 表示取**语音形态**而不是文本形态——文字通播要紧凑（照抄电码），
//! 语音要能听懂。
//!
//! ```text
//! 模板   "[FACILITY] ATIS [ATIS_LETTER] [OBS_TIME]. [WX]. [ARPT_COND]"
//! 文字   "ZSPD ATIS A 251300Z. 09004MPS 9999 FEW030 25/18 Q1013. 跑道 35L"
//! 语音   同一份模板渲染两遍，气象要素取 voice 形态
//! ```
//!
//! **同一个模板渲染两次**：一次给文字通播，一次给语音。没有显式写 `:VOX` 的变量，
//! 在语音那一遍里也会自动取 voice 形态——否则每个模板都得写满 `:VOX`，太啰嗦。

use crate::metar::{spell_letter, Element, Metar};
use crate::voicefix;
use regex::{Captures, Regex};
use std::collections::BTreeMap;
use std::sync::LazyLock;

static VARIABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([A-Z_]+)(:VOX)?\]").expect("VARIABLE"));
/// 缩略语在模板里写成 `@变量名`，和 vATIS 一致。
static CONTRACTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@([A-Za-z_][A-Za-z0-9_]*)").expect("CONTRACTION"));

/// 变量名 → 取值函数。一个含义有多个别名的，vATIS 里也是这样。
pub const ALIASES: &[(&str, &str)] = &[
    ("FACILITY", "facility"),
    ("ARPT", "facility"),
    ("ATIS_CODE", "letter"),
    ("ATIS_LETTER", "letter"),
    ("LETTER", "letter"),
    ("ID", "letter"),
    ("WX", "full_wx"),
    ("FULL_WX_STRING", "full_wx"),
    ("OBS_TIME", "observation_time"),
    ("TIME", "observation_time"),
    ("WIND", "wind"),
    ("SURFACE_WIND", "wind"),
    ("RVR", "rvr"),
    ("VIS", "visibility"),
    ("PREVAILING_VISIBILITY", "visibility"),
    ("PRESENT_WX", "present_weather"),
    ("PRESENT_WEATHER", "present_weather"),
    ("CLOUDS", "clouds"),
    ("TEMP", "temperature"),
    ("DEW", "dew_point"),
    ("PRESSURE", "pressure"),
    ("TREND", "trend"),
    ("RECENT_WX", "recent_weather"),
    ("ARPT_COND", "airport_conditions"),
    ("NOTAMS", "notams"),
    ("TL", "transition_level"),
    ("CLOSING", "closing"),
];

fn alias(name: &str) -> Option<&'static str> {
    ALIASES.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
}

pub const DEFAULT_TEMPLATE: &str =
    "[FACILITY] ATIS [ATIS_LETTER] [OBS_TIME]. [WX]. [ARPT_COND] [NOTAMS] [CLOSING]";

pub const DEFAULT_CLOSING: &str = "advise on initial contact you have information [ATIS_LETTER]";

/// 模板变量表。值统一是 [`Element`]（文字 + 语音两种形态）。
pub type Context = BTreeMap<&'static str, Element>;

/// 席位上那些自由文本字段。
#[derive(Debug, Clone, Default)]
pub struct FreeText {
    pub facility: String,
    /// 语音稿里念的机场名。空的话退回 `facility`。
    pub facility_voice: String,
    pub letter: String,
    pub airport_conditions: String,
    pub notams: String,
    pub transition_level: String,
    /// 收尾语。空串表示用 [`DEFAULT_CLOSING`]。
    pub closing: Option<String>,
}

/// 自由文本（机场条件、NOTAM）。
///
/// 文字通播**保留缩写原样**——那份是给人看的，`RWY34L APCH` 正是它该有的样子。
/// 语音稿必须展开，否则 TTS 会把 `APCH` 念成"埃屁西埃奇"、
/// 把 `RWY34L_R` 念成"阿威三十四艾尔下划线啊"。
fn free(value: &str) -> Element {
    let v = value.trim();
    Element::new(v, voicefix::expand_free_text(v))
}

/// 把 METAR 和席位上的自由文本组装成变量表。
pub fn build_context(metar: &Metar, fields: &FreeText) -> Context {
    let facility = if fields.facility.is_empty() {
        metar.station.clone()
    } else {
        fields.facility.clone()
    };
    let facility_voice = if fields.facility_voice.is_empty() {
        facility.clone()
    } else {
        fields.facility_voice.clone()
    };

    let mut ctx: Context = BTreeMap::new();
    // 文字稿写 ICAO（ZSPD），语音念全名（Shanghai Pudong International Airport）
    // ——真实通播念的是机场名，念四个字母听着像在拼写。
    ctx.insert(
        "facility",
        Element::new(facility, voicefix::expand_free_text(&facility_voice)),
    );
    // 文字稿留字母本身（A），语音稿念通话字母（Alpha）。
    ctx.insert(
        "letter",
        Element::new(fields.letter.clone(), spell_letter(&fields.letter)),
    );
    ctx.insert("full_wx", metar.full_wx());
    ctx.insert("observation_time", metar.observation_time.clone());
    ctx.insert("wind", metar.wind.clone());
    ctx.insert("rvr", metar.rvr.clone());
    ctx.insert("visibility", metar.visibility.clone());
    ctx.insert("present_weather", metar.present_weather.clone());
    ctx.insert("clouds", metar.clouds.clone());
    ctx.insert("temperature", metar.temperature.clone());
    ctx.insert("dew_point", metar.dew_point.clone());
    ctx.insert("pressure", metar.pressure.clone());
    ctx.insert("trend", metar.trend.clone());
    ctx.insert("recent_weather", metar.recent_weather.clone());
    ctx.insert("airport_conditions", free(&fields.airport_conditions));
    ctx.insert("notams", free(&fields.notams));
    ctx.insert("transition_level", free(&fields.transition_level));

    // 收尾语本身还能引用 [ATIS_LETTER]，所以要先渲染一遍。
    //
    // **两遍都要渲染。** 只做文字那一遍的话，收尾语里的 [ATIS_LETTER] 永远是
    // "F"，于是同一句通播开头念 "INFORMATION Foxtrot"、结尾念 "information F"
    // ——听上去像两份稿子。
    let closing_text = match &fields.closing {
        Some(c) => c.as_str(),
        None => DEFAULT_CLOSING,
    };
    let closing = Element::new(
        substitute(closing_text, &ctx, false),
        voicefix::expand_free_text(&substitute(closing_text, &ctx, true)),
    );
    ctx.insert("closing", closing);
    ctx
}

fn substitute(template: &str, ctx: &Context, voice: bool) -> String {
    VARIABLE
        .replace_all(template, |c: &Captures| {
            let name = &c[1];
            let vox = c.get(2).is_some();
            // 认不出的变量原样留着，方便发现拼错。
            let Some(key) = alias(name) else {
                return c[0].to_string();
            };
            match ctx.get(key) {
                Some(e) => {
                    // 语音那一遍默认就取 voice 形态；文字那一遍只有显式写了
                    // `:VOX` 才取。
                    if voice || vox {
                        e.voice.clone()
                    } else {
                        e.text.clone()
                    }
                }
                None => String::new(),
            }
        })
        .into_owned()
}

static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("WS"));
static SPACE_BEFORE_PUNCT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+([.,。，])").expect("SPACE_BEFORE_PUNCT"));
static LEADING_PUNCT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[.,。，]\s*").expect("LEADING_PUNCT"));

/// 折叠连续**同一个**标点。
///
/// Python 写的是 `([.,。，])\1+` —— 反向引用，Rust 的 `regex` 不支持，
/// 所以手写。注意它只折叠重复的同一个符号：`,.` 两个不同的原样留着。
fn collapse_repeats(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last: Option<char> = None;
    for c in text.chars() {
        if matches!(c, '.' | ',' | '。' | '，') && last == Some(c) {
            continue;
        }
        out.push(c);
        last = Some(c);
    }
    out
}

/// 收拾掉变量为空留下的多余空格和标点。
pub fn tidy(text: &str) -> String {
    let t = WS.replace_all(text, " ").trim().to_string();
    let t = SPACE_BEFORE_PUNCT.replace_all(&t, "$1").into_owned();
    let t = collapse_repeats(&t);
    LEADING_PUNCT.replace(&t, "").trim().to_string()
}

/// 缩略语表：`@名字` → （文字形态, 语音形态）。
pub type Contractions = BTreeMap<String, (String, String)>;

/// 把 `@变量名` 换成缩略语的文字或语音形态。
///
/// 认不出的 `@xxx` **原样留着**——多半是拼错了，留着才看得见。
fn expand_contractions(text: &str, contractions: &Contractions, voice: bool) -> String {
    if contractions.is_empty() {
        return text.to_string();
    }
    CONTRACTION
        .replace_all(text, |c: &Captures| match contractions.get(&c[1]) {
            Some((written, spoken)) => {
                let (first, second) = if voice {
                    (spoken, written)
                } else {
                    (written, spoken)
                };
                if first.is_empty() {
                    second.clone()
                } else {
                    first.clone()
                }
            }
            None => c[0].to_string(),
        })
        .into_owned()
}

/// 渲染模板，返回（文字通播, 语音稿）。
pub fn render(template: &str, ctx: &Context, contractions: &Contractions) -> (String, String) {
    let text = expand_contractions(&substitute(template, ctx, false), contractions, false);
    let voice = expand_contractions(&substitute(template, ctx, true), contractions, true);
    // 语音稿整体再过一遍缩写展开：模板里**手写的字面量**（`TRL [TL]` 里的 TRL、
    // `EXPECT ILS APPROACH`）不属于任何自由文本字段，原来谁都不管它，TTS 就照
    // 字母念。已经展开过的部分是幂等的——展开后的文字里既没有缩写词也没有裸数字，
    // 各条规则都匹配不上。
    //
    // `polish` 是最后一道防线：上游哪一环少了分隔符，也不该让用户听到一个怪词。
    (
        tidy(&text),
        voicefix::polish(&voicefix::expand_free_text(&tidy(&voice))),
    )
}

/// 模板里认不出来的变量，配置界面用来提示拼写错误。
pub fn unknown_variables(template: &str) -> Vec<String> {
    let mut names: Vec<String> = VARIABLE
        .captures_iter(template)
        .map(|c| c[1].to_string())
        .filter(|n| alias(n).is_none())
        .collect();
    names.sort();
    names.dedup();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZSPD: &str = "ZSPD 251300Z 09004MPS 9999 FEW030 SCT100 25/18 Q1013 NOSIG";

    fn ctx_with(fields: FreeText) -> Context {
        build_context(&Metar::parse(ZSPD), &fields)
    }

    fn ctx() -> Context {
        ctx_with(FreeText {
            facility: "ZSPD".into(),
            letter: "B".into(),
            airport_conditions: "跑道 35L 使用中".into(),
            notams: "无".into(),
            ..Default::default()
        })
    }

    fn render_with(ctx: &Context, tpl: &str) -> (String, String) {
        render(tpl, ctx, &Contractions::new())
    }

    #[test]
    fn text_uses_raw_and_voice_uses_spoken() {
        let (text, voice) = render_with(&ctx(), "[WIND]");
        assert_eq!(text, "09004MPS");
        assert_eq!(voice, "wind zero niner zero degrees four meters per second");
    }

    /// `:VOX` 让文字那一遍也取语音形态。整个后缀的存在理由就是这一条。
    #[test]
    fn vox_suffix_forces_spoken_form_in_text() {
        let (text, _) = render_with(&ctx(), "[WIND:VOX]");
        assert_eq!(text, "wind zero niner zero degrees four meters per second");
    }

    #[test]
    fn aliases_point_at_the_same_value() {
        let c = ctx();
        for name in ["[ATIS_LETTER]", "[ATIS_CODE]", "[LETTER]", "[ID]"] {
            assert_eq!(render_with(&c, name).0, "B", "{name}");
        }
        assert_eq!(
            render_with(&c, "[VIS]").0,
            render_with(&c, "[PREVAILING_VISIBILITY]").0
        );
    }

    #[test]
    fn free_text_variables() {
        let c = ctx();
        assert_eq!(render_with(&c, "[ARPT_COND]").0, "跑道 35L 使用中");
        assert_eq!(render_with(&c, "[NOTAMS]").0, "无");
    }

    /// 认不出的变量**要留着**，方便发现拼错。悄悄删掉的话，一份少了一整段的
    /// 通播看上去和正常的一模一样。
    #[test]
    fn unknown_variable_is_left_alone() {
        assert_eq!(render_with(&ctx(), "[NOPE]").0, "[NOPE]");
        assert_eq!(unknown_variables("[NOPE] [WIND]"), vec!["NOPE"]);
        assert!(unknown_variables("[WIND] [CLOUDS]").is_empty());
    }

    /// 没有 RVR 时不该留下多余的空格和标点。
    #[test]
    fn empty_variables_do_not_leave_debris() {
        let (text, _) = render_with(&ctx(), "[WIND]. [RVR]. [PRESSURE]");
        assert!(!text.contains("  "), "{text}");
        assert!(!text.contains(".."), "{text}");
    }

    #[test]
    fn default_template_renders() {
        let (text, voice) = render_with(&ctx(), DEFAULT_TEMPLATE);
        assert!(text.starts_with("ZSPD ATIS B"), "{text}");
        assert!(text.contains("跑道 35L 使用中"), "{text}");
        assert!(voice.contains("wind zero niner zero"), "{voice}");
        assert!(voice.contains("information Bravo"), "{voice}");
    }

    /// **收尾语里的字母也要念通话字母。**
    ///
    /// 收尾语本身能引用 `[ATIS_LETTER]`，所以它要先渲染一遍——而且**两遍都要**。
    /// 只做文字那一遍的话，同一句通播开头念 "INFORMATION Foxtrot"、
    /// 结尾念 "information F"，听上去像两份稿子。
    #[test]
    fn the_closing_letter_is_spoken_too() {
        let c = ctx_with(FreeText {
            facility: "ZSPD".into(),
            letter: "F".into(),
            ..Default::default()
        });
        let (_, voice) = render_with(&c, "[CLOSING]");
        assert!(voice.contains("information Foxtrot"), "{voice}");
        assert!(!format!("{voice} ").contains("information F "), "{voice}");
    }

    /// 语音念机场全名，文字稿留 ICAO。念 "Z S P D" 听着像在拼写，
    /// 真实通播念的是机场名。
    #[test]
    fn the_facility_is_spoken_as_the_airport_name() {
        let c = ctx_with(FreeText {
            facility: "ZSPD".into(),
            facility_voice: "Shanghai Pudong International Airport".into(),
            letter: "F".into(),
            ..Default::default()
        });
        let (text, voice) = render_with(&c, "[FACILITY]");
        assert_eq!(text, "ZSPD");
        assert_eq!(voice, "Shanghai Pudong International Airport");
    }

    #[test]
    fn without_a_name_it_falls_back_to_the_code() {
        let c = ctx_with(FreeText {
            facility: "ZSPD".into(),
            letter: "F".into(),
            ..Default::default()
        });
        let (text, voice) = render_with(&c, "[FACILITY]");
        assert_eq!(text, "ZSPD");
        assert_eq!(voice, "ZSPD");
    }

    /// 席位没填机场代码时用报文里的电台代号，不是留空。
    #[test]
    fn an_empty_facility_falls_back_to_the_station() {
        let c = ctx_with(FreeText {
            letter: "A".into(),
            ..Default::default()
        });
        assert_eq!(render_with(&c, "[FACILITY]").0, "ZSPD");
    }

    /// 预设可以换掉默认收尾语；空的退回内置那句，**不能变成不说收尾语**。
    #[test]
    fn a_preset_closing_replaces_the_default() {
        let c = ctx_with(FreeText {
            facility: "ZBAA".into(),
            letter: "J".into(),
            closing: Some("advise you have information [ATIS_LETTER] and RNAV".into()),
            ..Default::default()
        });
        let (_, voice) = render_with(&c, "[CLOSING]");
        // RNAV 在语音稿里会被展开成 R NAV，否则 TTS 当成单词念。
        assert!(voice.contains("information Juliett and R NAV"), "{voice}");

        let c = ctx_with(FreeText {
            facility: "ZBAA".into(),
            letter: "J".into(),
            closing: None,
            ..Default::default()
        });
        assert!(
            render_with(&c, "[CLOSING]")
                .1
                .contains("advise on initial contact"),
            "the built-in closing must come back when the preset has none"
        );
    }

    /// `@缩略语` 两种形态分开；认不出的 `@xxx` 原样留着。
    #[test]
    fn contractions_have_a_written_and_a_spoken_form() {
        let mut c = Contractions::new();
        c.insert("ils".into(), ("ILS".into(), "I L S".into()));
        let (text, voice) = render("@ils @nope", &ctx(), &c);
        assert_eq!(text, "ILS @nope");
        assert!(voice.starts_with("I L S"), "{voice}");
        assert!(voice.contains("@nope"), "{voice}");
    }

    /// 只有语音形态的缩略语，文字那一遍退回语音形态而不是留空。
    #[test]
    fn a_contraction_with_only_one_form_uses_it_for_both() {
        let mut c = Contractions::new();
        c.insert("vor".into(), ("VOR".into(), String::new()));
        let (text, voice) = render("@vor", &ctx(), &c);
        assert_eq!(text, "VOR");
        assert!(!voice.is_empty(), "the voice form must not vanish");
    }

    /// `tidy` 只折叠**重复的同一个**标点。Python 那条用的是反向引用
    /// （`([.,。，])\1+`），换成"任意标点连着就折叠"会把 `,.` 也吃掉一个。
    #[test]
    fn only_repeats_of_the_same_punctuation_collapse() {
        // 注意这里**不补空格**——那是 `voicefix::tidy` 的活，两个 tidy 不一样。
        assert_eq!(tidy("a,,,b"), "a,b");
        assert_eq!(tidy("a , b"), "a, b");
        assert_eq!(tidy("a,.b"), "a,.b");
        assert_eq!(tidy("a 。。 b"), "a。 b");
        // 开头的标点整个去掉——变量为空时留下的就是这种。
        assert_eq!(tidy(". [x]"), "[x]");
    }
}
