//! 语音稿的收尾处理。
//!
//! 通播稿里有两类东西是 TTS 念不好的，而它们都来自**自由文本**（机场条件、
//! NOTAM）——那部分是管制员照着真实通播抄进来的，全是缩写和数字：
//!
//! ```text
//! ILS ZULU RWY34L APCH AND ILS ZULU RWY34R APCH. LDG RWY 34 LEFT AND
//! 34 RIGHT. DEP RWY 05 AND 34 RIGHT. DEP FREQ 126.0, SIMUL PARL ILS
//! APCHS TO RWY34L_R ARE INPR
//! ```
//!
//! TTS 会把 `APCH` 念成"埃屁西埃奇"、`RWY34L_R` 念成"阿威三十四艾尔下划线啊"、
//! `126.0` 念成"一百二十六点零"。真实通播里这些都是要展开的：approach、runway
//! three four left and right、one two six decimal zero。
//!
//! 另一类是**元素之间粘住**，比如 `broken niner thousandtemperature two five`
//! 和 `hectopascals2992`。这类是拼接时少了分隔符，念出来是一个怪词。
//!
//! 这一层只动语音稿，文字通播照旧保留电码原样——文字那份是给人看的，缩写正是
//! 它该有的样子。
//!
//! # 这是一份移植，照着 `can-audio/atis/voicefix.py` 一条一条搬过来的
//!
//! 每一条规则的顺序都是有理由的，注释里写着；打乱顺序不会让任何测试报错，
//! 只会让某一类稿子念错，而那要等到有人在频率上听见才发现。
//!
//! 两处和 Python 不一样，都是因为 Rust 的 `regex` **没有环视**：
//! 零宽插入（`(?<=[a-z])(?=[A-Z][a-z])` 一类）改成了手写扫描。改写成"吃掉
//! 前后字符再吐回去"是不对的——那会让紧挨着的第二处插入被吃掉的字符盖住，
//! `aBcDe` 只切一刀而不是两刀。

use regex::{Captures, Regex};
use std::sync::LazyLock;

/// 航空缩写。键必须是完整单词（按词边界匹配），否则 `DEP` 会把 `DEPARTURE`
/// 里的前三个字母也换掉。**长的写在前面**，让 `APCHS` 先于 `APCH` 命中。
pub const ABBREVIATIONS: &[(&str, &str)] = &[
    ("APCHS", "approaches"),
    ("APCH", "approach"),
    ("ARR", "arrival"),
    ("DEP", "departure"),
    ("DEPS", "departures"),
    ("LDG", "landing"),
    ("TKOF", "takeoff"),
    ("RWY", "runway"),
    ("RWYS", "runways"),
    ("TWY", "taxiway"),
    ("FREQ", "frequency"),
    ("SIMUL", "simultaneous"),
    ("PARL", "parallel"),
    ("INPR", "in progress"),
    ("INOP", "inoperative"),
    ("UNAVBL", "unavailable"),
    ("AVBL", "available"),
    ("CLSD", "closed"),
    ("TFC", "traffic"),
    ("ACFT", "aircraft"),
    ("CTC", "contact"),
    ("EXP", "expect"),
    ("MAINT", "maintenance"),
    ("CONST", "construction"),
    ("WIP", "work in progress"),
    ("BTN", "between"),
    ("TWR", "tower"),
    ("GND", "ground"),
    ("APP", "approach"),
    ("DME", "D M E"),
    ("VOR", "V O R"),
    ("NDB", "N D B"),
    ("ILS", "I L S"),
    ("RVR", "R V R"),
    ("CAT", "category"),
    ("MET", "met"),
    ("SFC", "surface"),
    ("BLW", "below"),
    ("ABV", "above"),
    ("TRL", "transition level"),
    ("TA", "transition altitude"),
    ("QNH", "Q N H"),
    ("STP", "standard time procedure"),
    // 这两个不展开的话 TTS 会当成单词念（"atk"、"arnav"）。放在最后，前面的
    // 规则都跑完了才轮到它们，不会被 CAT / TA 这些短缩写切到。
    ("ATC", "A T C"),
    ("RNAV", "R NAV"),
];

/// 跑道后缀。L/R/C 在跑道号后面读成方位词，别的地方不动。
fn runway_side(c: char) -> Option<&'static str> {
    match c.to_ascii_uppercase() {
        'L' => Some("left"),
        'R' => Some("right"),
        'C' => Some("center"),
        _ => None,
    }
}

/// 逐位念。通播里的数字基本都这么念。
///
/// `9` 念 `niner`——航空惯例，和 [`crate::metar`] 里那份数字表一致。
pub fn spell_digits(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(match c {
            '0' => "zero",
            '1' => "one",
            '2' => "two",
            '3' => "three",
            '4' => "four",
            '5' => "five",
            '6' => "six",
            '7' => "seven",
            '8' => "eight",
            '9' => "niner",
            other => {
                out.push(other);
                continue;
            }
        });
    }
    out
}

/// `RWY34L` / `34L_R` / `05` —— 跑道号逐位念，L/R/C 念成方位。
///
/// 下划线是日本通播里"和"的写法：`RWY34L_R` 是 34 左**和** 34 右。
fn runway(number: &str, sides: &str) -> String {
    let mut spoken = spell_digits(number);
    let words: Vec<&str> = sides.chars().filter_map(runway_side).collect();
    if !words.is_empty() {
        spoken.push(' ');
        spoken.push_str(&words.join(" and "));
    }
    spoken
}

static RWY_PREFIXED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bRWY\s*(\d{2})((?:[LRC](?:_[LRC])*)?)\b").expect("RWY_PREFIXED")
});
static RWY_BARE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(\d{2})([LRC](?:_[LRC])*)\b").expect("RWY_BARE"));
static FREQUENCY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{2,3})\.(\d{1,3})\b").expect("FREQUENCY"));
static INTEGER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d+\b").expect("INTEGER"));
static ABBREV: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    ABBREVIATIONS
        .iter()
        .map(|(short, long)| {
            (
                Regex::new(&format!(r"(?i)\b{short}\b")).expect("abbreviation"),
                *long,
            )
        })
        .collect()
});

/// 把自由文本里的缩写和数字展开成能念的形式。
///
/// **顺序有讲究**：先处理跑道（它含数字和字母），再处理剩下的小数和整数，最后
/// 才展开纯字母缩写——反过来的话 `RWY` 已经变成 `runway`，跑道号就认不出来了。
pub fn expand_free_text(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }

    // 跑道：`RWY34L_R` / `RWY 34L`。
    //
    // Python 这里还有一条带后顾断言的 `(?<=\bRWY )\d{2}…`，它在任何输入上都是
    // 死规则：能匹配它的位置前面必然有 `RWY `，而上面这一条从 `RWY` 起就把整段
    // 吃掉了，轮到它时数字已经是 `three four` 了。所以没有搬过来。
    let mut result = RWY_PREFIXED
        .replace_all(text, |c: &Captures| format!("RWY {}", runway(&c[1], &c[2])))
        .into_owned();

    // 列表里的跑道：`ARR RWY 16L, 17R, DEP RWY 16R, 17L` —— 第二个之后不再紧跟
    // `RWY`，上面那条够不着，实测 `17R` 原样留下，TTS 念成"十七阿"。
    // 真实通播的进离场跑道全是这种列表写法。带方位字母的两位数在通播自由文本里
    // 只可能是跑道号，所以单独成一条；不带字母的两位数不管（那可能是别的数）。
    result = RWY_BARE
        .replace_all(&result, |c: &Captures| runway(&c[1], &c[2]))
        .into_owned();

    // 频率一类的小数。
    result = FREQUENCY
        .replace_all(&result, |c: &Captures| {
            format!("{} decimal {}", spell_digits(&c[1]), spell_digits(&c[2]))
        })
        .into_owned();

    // 展开缩写。词边界匹配，避免切到别的单词里。
    for (re, long) in ABBREV.iter() {
        result = re.replace_all(&result, *long).into_owned();
    }

    // 剩下的整数逐位念。四位以上多半是年份或编号，也一样逐位。
    result = INTEGER
        .replace_all(&result, |c: &Captures| spell_digits(&c[0]))
        .into_owned();

    // 下划线和斜杠念不出来，换成词。
    result = result.replace('_', " and ").replace('/', " ");
    tidy(&result)
}

static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("WS"));
static SPACE_BEFORE_PUNCT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s+([,.])").expect("SPACE_BEFORE_PUNCT"));
static DOUBLE_DOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.\s*\.+").expect("DOUBLE_DOT"));
static DOUBLE_COMMA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r",\s*,+").expect("DOUBLE_COMMA"));

/// 标点后面补一个空格。
///
/// Python 写的是 `([,.])(?=\S)` —— 前看断言，只看不吃。改写成 `([,.])(\S)`
/// 是不行的：`,,x` 里第二个逗号会被第一次匹配吃掉，于是少补一个空格，
/// 而后面那条折叠连续逗号的规则会把差别放大成一个真实的不同结果。
fn space_after_punct(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if matches!(c, ',' | '.') {
            if let Some(next) = chars.peek() {
                if !next.is_whitespace() {
                    out.push(' ');
                }
            }
        }
    }
    out
}

/// 收拾空白和标点，让 TTS 断句正常。
fn tidy(text: &str) -> String {
    let t = WS.replace_all(text, " ").into_owned();
    let t = SPACE_BEFORE_PUNCT.replace_all(&t, "$1").into_owned();
    let t = space_after_punct(&t);
    let t = DOUBLE_DOT.replace_all(&t, ".").into_owned();
    let t = DOUBLE_COMMA.replace_all(&t, ",").into_owned();
    t.trim().to_string()
}

/// 把各段拼起来，保证段与段之间断得开。
///
/// `broken niner thousandtemperature two five` 就是这一步少了分隔符——两段
/// 直接首尾相接，TTS 当成一个词念。
pub fn join_elements<I, S>(parts: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let cleaned: Vec<String> = parts
        .into_iter()
        .filter_map(|p| {
            let t = p.as_ref().trim().trim_matches(',').to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        })
        .collect();
    tidy(&cleaned.join(", "))
}

/// 通播里每一段的起始词。上游少了分隔符时，这些词会和前一段的末尾粘成一个
/// 怪词（真实案例：`broken niner thousandtemperature two five`）。**全小写的
/// 粘连没有任何模式能识别**，只能靠"我知道通播里有哪些段"来切。
pub const PHRASE_STARTS: &[&str] = &[
    "temperature",
    "dew point",
    "dewpoint",
    "wind",
    "visibility",
    "altimeter",
    "QNH",
    "clouds",
    "few",
    "scattered",
    "broken",
    "overcast",
    "vertical visibility",
    "runway",
    "transition level",
    "remarks",
    "advise you have information",
];

static PHRASE_RES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    PHRASE_STARTS
        .iter()
        .map(|w| {
            (
                // Python 那条是 `(?<=[a-z])<word>\b`，且带 IGNORECASE ——
                // **忽略大小写作用在整条模式上，后顾里的 `[a-z]` 也跟着大小写不敏感**，
                // 所以这里是 `[A-Za-z]`。
                Regex::new(&format!(r"(?i)([A-Za-z])({})\b", regex::escape(w))).expect("phrase"),
                *w,
            )
        })
        .collect()
});

static LETTER_THEN_DIGITS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([a-z])(\d+)").expect("LETTER_THEN_DIGITS"));

/// 语音稿的最后一道：补上缺失的分隔，去掉念不出来的符号。
///
/// 这一层是防御性的——**就算上游拼错了，也不该让用户听到一个怪词。**
pub fn polish(voice: &str) -> String {
    if voice.is_empty() {
        return String::new();
    }

    let mut text = voice.to_string();
    // 已知的段起始词粘在前一个词尾巴上时，切开。
    //
    // 换成的是**表里那个写法**而不是匹配到的原文：`thousandTemperature` 切完
    // 念的是 `temperature`，大小写跟着表走，和 Python 一致。
    for (re, word) in PHRASE_RES.iter() {
        text = re
            .replace_all(&text, |c: &Captures| format!("{}, {}", &c[1], word))
            .into_owned();
    }

    // 字母紧跟数字（`hectopascals2992`）：切开，并且这种数字都是逐位念的。
    text = LETTER_THEN_DIGITS
        .replace_all(&text, |c: &Captures| {
            format!("{}, {}", &c[1], spell_digits(&c[2]))
        })
        .into_owned();
    // 数字紧跟字母、小写字母紧跟大写（`thousandTemperature`）也是粘住了。
    // 两条都是**零宽插入**，手写扫描，理由见模块头。
    text = split_digit_letter(&text);
    text = split_camel(&text);
    text = text.replace(['_', '/'], " ");
    tidy(&text)
}

/// `(?<=\d)(?=[A-Za-z])` → 插一个空格。
fn split_digit_letter(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && chars[i - 1].is_ascii_digit() && c.is_ascii_alphabetic() {
            out.push(' ');
        }
        out.push(*c);
    }
    out
}

/// `(?<=[a-z])(?=[A-Z][a-z])` → 插一个逗号和空格。
fn split_camel(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, c) in chars.iter().enumerate() {
        if i > 0
            && chars[i - 1].is_ascii_lowercase()
            && c.is_ascii_uppercase()
            && chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase())
        {
            out.push_str(", ");
        }
        out.push(*c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ARR RWY 16L, 17R` —— 列表里第二个之后也要展开。
    ///
    /// Python 版原来只有紧跟 `RWY` 的那一个会展开，`17R` 原样留给 TTS，
    /// 念成"十七阿"。**真实通播的进离场跑道全是这种列表写法**，所以这条
    /// 几乎必然被撞上。整句照搬自 `can-audio/atis/test_atis.py`。
    #[test]
    fn runways_in_a_list_are_all_expanded() {
        assert_eq!(
            expand_free_text("ARR RWY 16L, 17R, DEP RWY 16R, 17L"),
            "arrival runway one six left, one seven right, \
             departure runway one six right, one seven left"
        );
    }

    #[test]
    fn runway_sides_are_words_not_letters() {
        assert!(expand_free_text("RWY 16L").contains("left"));
        assert!(expand_free_text("RWY 16R").contains("right"));
        assert!(expand_free_text("RWY 16C").contains("center"));
    }

    /// 下划线是日本通播里"和"的写法：`RWY34L_R` 是 34 左**和** 34 右。
    #[test]
    fn an_underscore_between_sides_means_and() {
        assert_eq!(
            expand_free_text("SIMUL PARL ILS APCHS TO RWY34L_R ARE INPR"),
            // `TO` 和 `ARE` 不在缩写表里，原样留着大写——和 Python 一字不差。
            "simultaneous parallel I L S approaches TO runway three four left and right \
             ARE in progress"
        );
    }

    /// 不展开的话 TTS 会把它们当成单词念（"atk"、"arnav"）。
    #[test]
    fn atc_and_rnav_are_spelled_out() {
        assert_eq!(
            expand_free_text("advise ATC when requesting clearance"),
            "advise A T C when requesting clearance"
        );
        assert_eq!(
            expand_free_text("RNAV departures available"),
            "R NAV departures available"
        );
    }

    /// **跑道必须写成 `36L`，不能写 `36Left`。**
    ///
    /// 展开靠的是"两位数字紧跟 L/R/C 再收尾"这个形状。写成 `36Left` 的话，
    /// L 后面还是字母、收不了尾，连"两位整数"那条兜底规则也匹配不上（6 后面
    /// 是 L，没有词边界），于是整串原样交给 TTS。真实通播里写错这一处，
    /// 念出来就不是 "three six left"。这条钉着这个坑，免得下次有人照着英文
    /// 单词去写。
    #[test]
    fn a_runway_written_as_36left_is_not_expanded() {
        assert_eq!(expand_free_text("RWY 36Left"), "runway 36Left");
        assert_eq!(expand_free_text("RWY 36L"), "runway three six left");
    }

    #[test]
    fn frequencies_are_read_as_decimals() {
        assert_eq!(
            expand_free_text("DEP FREQ 126.0"),
            "departure frequency one two six decimal zero"
        );
    }

    /// 长的缩写要先于短的命中，否则 `APCHS` 会被 `APCH` 切成 "approachS"。
    #[test]
    fn the_longer_abbreviation_wins() {
        assert_eq!(expand_free_text("APCHS"), "approaches");
        assert_eq!(expand_free_text("APCH"), "approach");
    }

    /// 词边界匹配：`DEP` 不该把 `DEPARTURE` 里的前三个字母也换掉。
    #[test]
    fn an_abbreviation_does_not_cut_into_a_longer_word() {
        assert_eq!(expand_free_text("DEPARTURE"), "DEPARTURE");
    }

    /// 段与段粘住时切开。真实案例就是
    /// `broken niner thousandtemperature two five` ——
    /// 上游少了一个分隔符，TTS 把它当成一个词念。
    #[test]
    fn a_phrase_stuck_to_the_previous_one_is_split() {
        assert_eq!(
            polish("broken niner thousandtemperature two five"),
            "broken niner thousand, temperature two five"
        );
    }

    /// 字母紧跟数字（`hectopascals2992`）：切开，而且这种数字是逐位念的。
    #[test]
    fn digits_stuck_to_a_word_are_split_and_spelled() {
        assert_eq!(
            polish("altimeter hectopascals2992"),
            "altimeter hectopascals, two niner niner two"
        );
    }

    /// `thousandTemperature` 这种驼峰粘连也是粘住了。
    ///
    /// **两处连着粘的都要切开。** Python 那条是零宽断言（只看不吃），
    /// 换成"吃掉前后字符"的写法只会切第一刀：`aBcDe` 会变成 `a, BcDe`
    /// 而不是 `a, Bc, De`。这就是这里手写扫描而不用正则的原因。
    #[test]
    fn two_adjacent_camel_joins_are_both_split() {
        assert_eq!(split_camel("aBcDe"), "a, Bc, De");
    }

    /// 数字和字母之间补一个空格，同样是零宽插入。
    #[test]
    fn a_letter_right_after_a_digit_gets_a_space() {
        assert_eq!(split_digit_letter("1a2b"), "1 a2 b");
    }

    /// 标点后面补空格用的也是前看断言。`,,x` 是那条规则和"吃掉下一个字符"
    /// 的写法分道扬镳的地方：吃掉的写法少补一个空格，而下一条折叠连续逗号的
    /// 规则会把那点差别放大成一个真实的不同结果。
    #[test]
    fn punctuation_spacing_matches_the_lookahead_not_a_consuming_match() {
        assert_eq!(tidy(",,x"), ", x");
        assert_eq!(tidy("a,b"), "a, b");
        assert_eq!(tidy("a , b"), "a, b");
    }

    #[test]
    fn empty_input_stays_empty() {
        assert_eq!(expand_free_text(""), "");
        assert_eq!(polish(""), "");
    }

    /// 段与段之间必须断得开——这就是 `join_elements` 存在的理由。
    #[test]
    fn joining_keeps_the_pieces_apart() {
        assert_eq!(
            join_elements(["broken niner thousand", "", "  ", "temperature two five"]),
            "broken niner thousand, temperature two five"
        );
    }
}
