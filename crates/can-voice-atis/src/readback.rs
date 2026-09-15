//! 把 ATIS 文本换成无线电读法：数字逐位念，孤立的大写字母念它的字母词。
//!
//! 照搬 `can-audio/server/ATIS/process.py`。**这一整块是领域知识，不是可以重写的
//! 东西**——大陆的无线电数字读法（洞幺两……拐）和普通中文数字不一样，而
//! `niner` 之所以不是 `nine`，是因为无线电上 nine 和 five 太像。
//!
//! # 中文那一半念中文字母词，这一条和服务端的 Python 版不一样
//!
//! Python 版的 `replace_letter` 不分语言，一律用英文 NATO 词，于是中文那一半里
//! 一个孤立的 `A` 会被念成 "Alpha"——TTS 在一串汉字中间蹦出一个英文字符。
//! 这里**没有**照搬那个行为。
//!
//! 曾经照搬过，理由是"26 个字母的中文表凑不齐，猜出来的比照搬更糟"。那个理由
//! 是错的：表一直在 `can-audio/atis/chinese.py` 的 `LETTERS` 里，客户端侧为此
//! 专门研究过。整张表搬了过来，见 [`CHINESE_LETTERS`]。

use std::collections::HashMap;
use std::sync::OnceLock;

/// 中英分隔符。**英文在前，中文在后。** 这是 can-audio 这边的约定，
/// 不是 datafeed 的——datafeed 只保证 `text_atis` 是一个字符串数组。
pub const SEPARATOR: char = '|';

/// 大陆无线电的中文数字读法。不是普通中文数字：0 念洞、1 念幺、2 念两、7 念拐。
const CHINESE_DIGITS: [&str; 10] = ["洞", "幺", "两", "三", "四", "五", "六", "拐", "八", "九"];

/// 英文数字读法。**9 是 niner**——无线电上 nine 和 five 太像。
const ENGLISH_DIGITS: [&str; 10] = [
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "niner",
];

const NATO: [(char, &str); 26] = [
    ('A', "Alpha"),
    ('B', "Bravo"),
    ('C', "Charlie"),
    ('D', "Delta"),
    ('E', "Echo"),
    ('F', "Foxtrot"),
    ('G', "Golf"),
    ('H', "Hotel"),
    ('I', "India"),
    ('J', "Juliett"),
    ('K', "Kilo"),
    ('L', "Lima"),
    ('M', "Mike"),
    ('N', "November"),
    ('O', "Oscar"),
    ('P', "Papa"),
    ('Q', "Quebec"),
    ('R', "Romeo"),
    ('S', "Sierra"),
    ('T', "Tango"),
    ('U', "Uniform"),
    ('V', "Victor"),
    ('W', "Whiskey"),
    ('X', "X-ray"),
    ('Y', "Yankee"),
    ('Z', "Zulu"),
];

/// 通话字母的中文读法，照搬 `can-audio/atis/chinese.py` 的 `LETTERS`。
///
/// 中文通播念的是"情报通播 朱丽叶"，不是拉丁字母 J。
/// **和 [`NATO`] 一一对应，顺序相同**——两张表要一起改。
const CHINESE_LETTERS: [&str; 26] = [
    "阿尔法",
    "布拉沃",
    "查理",
    "德尔塔",
    "埃科",
    "福克斯特罗",
    "高尔夫",
    "霍特尔",
    "印地亚",
    "朱丽叶",
    "基洛",
    "利马",
    "迈克",
    "诺文贝",
    "奥斯卡",
    "帕帕",
    "魁北克",
    "罗米欧",
    "塞拉",
    "探戈",
    "尤尼佛",
    "维克多",
    "威士忌",
    "爱克斯瑞",
    "洋基",
    "祖鲁",
];

fn nato() -> &'static HashMap<char, &'static str> {
    static M: OnceLock<HashMap<char, &'static str>> = OnceLock::new();
    M.get_or_init(|| NATO.iter().copied().collect())
}

/// 一个字母的通话字母表词。`metar` 那边的情报字母也念这一张表——
/// **不要再抄一份**：两张 NATO 表迟早会有一张被改。
pub fn nato_word(letter: char) -> Option<&'static str> {
    nato().get(&letter.to_ascii_uppercase()).copied()
}

/// 逐位念（中文无线电读法）：`350` → `三 五 洞`。非数字原样保留。
///
/// 和 [`crate::voicefix::spell_digits`] 是同一件事的另一半语言，
/// **表在这里，不要再抄一份**。
pub fn spell_chinese(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if !out.is_empty() {
            out.push(' ');
        }
        match c.to_digit(10) {
            Some(d) => out.push_str(CHINESE_DIGITS[d as usize]),
            None => out.push(c),
        }
    }
    out
}

/// 一个字母的中文通话词。中文通播念它，不念拉丁字母。
pub fn chinese_letter_word(letter: char) -> Option<&'static str> {
    let c = letter.to_ascii_uppercase();
    c.is_ascii_uppercase()
        .then(|| CHINESE_LETTERS[(c as u8 - b'A') as usize])
}

/// 一个字母在这一半语言里该念的词。
fn letter_word(letter: char, chinese: bool) -> Option<&'static str> {
    if chinese {
        chinese_letter_word(letter)
    } else {
        nato_word(letter)
    }
}

/// 紧跟在数字后面的孤立大写字母，是**单位**而不是字母。
///
/// `/LEVEL 3600 M` 里的 `M` 是"米"。不看位置的话它会被念成 "Mike"，
/// 而听的人得到的是一个不存在的情报字母。
///
/// 表只有一行，这是有意的：判据是"**前一个 token 以数字收尾**"，
/// 那才是真正做事的部分；再出现一个单位时往表里加一行就是了。
/// 换个位置的 `M`（`ATIS M` 的情报字母）前面不是数字，照旧念 Mike。
const UNITS_AFTER_A_NUMBER: [(char, &str, &str); 1] = [('M', "meters", "米")];

fn unit_word(letter: char, chinese: bool) -> Option<&'static str> {
    UNITS_AFTER_A_NUMBER
        .iter()
        .find(|(c, _, _)| *c == letter)
        .map(|(_, en, zh)| if chinese { *zh } else { *en })
}

/// 处理一整段 ATIS 文本，自动识别中英混合。
///
/// 恰好一个 [`SEPARATOR`] 才算中英混合；零个当纯英文，两个以上也当纯英文
/// ——猜它想表达什么只会把一段读不通的东西播出去。
pub fn process(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let parts: Vec<&str> = text.split(SEPARATOR).collect();
    if parts.len() == 2 {
        format!(
            "{}{}{}",
            single(parts[0].trim(), false),
            SEPARATOR,
            single(parts[1].trim(), true)
        )
    } else {
        single(text, false)
    }
}

/// 处理单一语言的一段文本。
///
/// 顺序要紧：**先换字母、再换数字**。反过来的话，数字换出来的
/// "one eight" 里那些孤立的大写字母……其实不会有，但顺序照 Python 版保持一致，
/// 免得哪天有人加一条规则时两边行为分岔。
pub fn single(text: &str, chinese: bool) -> String {
    let digits = if chinese {
        &CHINESE_DIGITS
    } else {
        &ENGLISH_DIGITS
    };
    let mut out = String::with_capacity(text.len() * 2);

    // 前一个 token 是不是以数字收尾。单位（`3600 M`）靠它和情报字母分开。
    let mut after_a_number = false;
    for token in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        // 孤立的大写字母 → 字母词。
        //
        // **只换孤立的。** `RWY 18L` 里的 L 贴着数字，换掉它会念成
        // "one eight Lima"，而那不是跑道号的读法。
        if let Some(c) = lone_capital(token) {
            match unit_word(c, chinese).filter(|_| after_a_number) {
                Some(unit) => out.push_str(unit),
                None => {
                    out.push_str(letter_word(c, chinese).expect("lone_capital only yields A-Z"))
                }
            }
            after_a_number = false;
            continue;
        }
        expand_digits(token, digits, &mut out);
        after_a_number = token
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_digit());
    }
    out
}

/// 这个 token 是不是一个孤立的大写字母。
///
/// **与 Python 版的一处有意差异。** 那边用 `re.sub(r'\s([A-Z])\s', …)`，而
/// `re.sub` 不重叠：匹配会把后面那个空格一起吃掉，于是 "A B C" 里只有 B 被换，
/// A（开头没有前导空格）和 C（空格被上一个匹配吃了）都漏掉。那是正则的副作用
/// 不是约定——漏掉的字母会被 TTS 念成一个孤零零的英文字母，而听的人根本不知道
/// 少了什么。按 token 切就没有这个问题。
fn lone_capital(token: &str) -> Option<char> {
    let mut chars = token.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    nato().contains_key(&c).then_some(c)
}

/// 把 token 里的每一段数字逐位展开，其余原样保留。
fn expand_digits(token: &str, digits: &[&str; 10], out: &mut String) {
    let mut run = false;
    for c in token.chars() {
        match c.to_digit(10) {
            Some(d) => {
                // 数字段两侧加空格，让 TTS 把它和相邻的字母断开。
                if !run && !out.ends_with(' ') && !out.is_empty() {
                    out.push(' ');
                }
                if run {
                    out.push(' ');
                }
                out.push_str(digits[d as usize]);
                run = true;
            }
            None => {
                if run && !out.ends_with(' ') {
                    out.push(' ');
                }
                run = false;
                out.push(c);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_are_spoken_one_at_a_time_in_english() {
        assert_eq!(
            single("QNH 1007", false)
                .split_whitespace()
                .collect::<Vec<_>>(),
            vec!["QNH", "one", "zero", "zero", "seven"]
        );
    }

    /// 九念 niner，不是 nine —— 无线电上 nine 和 five 太像。
    #[test]
    fn nine_is_niner() {
        assert!(single("T 29", false).contains("niner"));
    }

    #[test]
    fn digits_are_spoken_one_at_a_time_in_chinese() {
        assert_eq!(
            single("QNH 1007", true)
                .split_whitespace()
                .collect::<Vec<_>>(),
            vec!["QNH", "幺", "洞", "洞", "拐"]
        );
    }

    /// 洞幺两三四五六拐八九 —— 大陆的无线电读法，和普通中文数字不一样：
    /// 0 念洞、1 念幺、2 念两、7 念拐。
    #[test]
    fn the_chinese_digits_are_the_radio_ones_not_the_ordinary_ones() {
        let got = single("0123456789", true);
        let digits: String = got.split_whitespace().collect();
        assert_eq!(digits, "洞幺两三四五六拐八九");
    }

    #[test]
    fn a_standalone_capital_becomes_its_nato_word() {
        assert!(single("ZSSS ATIS A 1200Z", false).contains("Alpha"));
        assert!(single("T 29 /DP 24", false).contains("Tango"));
    }

    /// 只换**孤立**的大写字母。`RWY 18L` 里的 L 是贴着数字的，
    /// 换掉它会念成"one eight Lima"，而那不是跑道号的读法。
    #[test]
    fn a_letter_attached_to_something_else_is_left_alone() {
        let got = single("DEP RWY 18L & 18R", false);
        assert!(!got.contains("Lima"), "got {got}");
        assert!(!got.contains("Romeo"), "got {got}");
        // ATIS / DEP / RWY 这些词也不能被拆
        assert!(got.contains("RWY"), "got {got}");
    }

    /// **与 Python 版的一处有意差异。** 那边用的正则是
    /// `\s([A-Z])\s`，而 `re.sub` 不重叠：匹配会把后面那个空格一起吃掉，
    /// 于是 "A B C" 里只有 B 被换，A（开头，前面没空格）和 C（空格被吃了）都漏掉。
    /// 那是正则的副作用，不是约定——漏掉的字母会被 TTS 念成一个孤零零的英文字母，
    /// 而听的人根本不知道少了什么。
    #[test]
    fn consecutive_letters_are_all_converted_unlike_the_python_regex() {
        let got = single("A B C", false);
        for want in ["Alpha", "Bravo", "Charlie"] {
            assert!(got.contains(want), "{want} missing from {got}");
        }
    }

    #[test]
    fn a_letter_at_either_end_is_converted_too() {
        assert!(single("A 1200Z", false).contains("Alpha"));
        assert!(single("YOU HAVE INFO A", false).contains("Alpha"));
    }

    // ——— 中英分隔 ———

    /// 上游用 `|` 分隔，**英文在前中文在后**。这是 can-audio 这边的约定，
    /// 不是 datafeed 的——datafeed 只保证 `text_atis` 是一个字符串数组。
    #[test]
    fn a_pipe_splits_english_from_chinese() {
        let out = process("QNH 1007|修正海压 1007");
        let (en, zh) = out.split_once('|').expect("the separator survives");
        assert!(en.contains("one"), "english half: {en}");
        assert!(zh.contains("幺"), "chinese half: {zh}");
        assert!(
            !zh.contains("one"),
            "the chinese half must not get english digits: {zh}"
        );
    }

    #[test]
    fn text_without_a_pipe_is_treated_as_english() {
        let out = process("QNH 1007");
        assert!(out.contains("one"));
        assert!(!out.contains('|'));
    }

    /// 两个以上的 `|` 不是"中英混合"，按原样当英文处理——猜它想表达什么
    /// 只会把一段读不通的东西播出去。
    #[test]
    fn more_than_one_separator_is_not_a_language_split() {
        let out = process("A|B|C");
        assert!(!out.contains("Alpha") || out.matches('|').count() == 2);
    }

    #[test]
    fn empty_text_stays_empty() {
        assert_eq!(process(""), "");
    }

    /// 中文那一半念中文字母词。**这是和服务端 Python 版有意不同的一条**：
    /// 那边不分语言一律念 NATO，于是一串汉字中间蹦出一个 "Alpha"。
    #[test]
    fn the_chinese_half_speaks_chinese_letter_words() {
        let got = single("通播 A", true);
        assert!(got.contains("阿尔法"), "{got}");
        assert!(!got.contains("Alpha"), "{got}");
        assert_eq!(single("通播 J", true), "通播 朱丽叶");
        // 英文那一半不受影响。
        assert!(single("INFO J", false).contains("Juliett"));
    }

    /// 两张字母表必须一一对应——中文那张是按 A..Z 的下标取的，
    /// 顺序错一位，整个表就偏了一格而每一条单独看都像对的。
    #[test]
    fn the_two_letter_tables_line_up() {
        assert_eq!(NATO.len(), CHINESE_LETTERS.len());
        for (i, (c, _)) in NATO.iter().enumerate() {
            assert_eq!(*c, (b'A' + i as u8) as char, "NATO is out of order at {i}");
            assert_eq!(chinese_letter_word(*c), Some(CHINESE_LETTERS[i]));
        }
        assert_eq!(chinese_letter_word('A'), Some("阿尔法"));
        assert_eq!(chinese_letter_word('Z'), Some("祖鲁"));
    }
    // ——— 真实报文 ———

    /// 拿金文件里那段真实 ZSSS ATIS 过一遍，当回归锚点。
    ///
    /// 它曾经钉着一个毛病：`/LEVEL 3600 M` 里的 `M` 是"米"，却被念成 "Mike"
    /// （Python 版一模一样，`\s([A-Z])\s` 照样匹配 " M "，所以那不是移植引入的）。
    /// **现在修好了**，靠的是位置而不是一张更长的表——见 [`UNITS_AFTER_A_NUMBER`]。
    #[test]
    fn a_real_atis_report_reads_sensibly() {
        let real = "ZSSS ATIS A 1200Z DEP RWY 18L & 18R EXP ILS APCH LDG RWY 18L & \
18R WIND 160 DEG 5 MPS CAVOK T 29 /DP 24 QNH 1007 HPA QNH OF \
SHANGHAI TERMINAL CONTROL AREA 1008 TRANSITION ALTITUDE 3000 \
/LEVEL 3600 M ADZ YOU HAVE INFO A";
        let got = process(real);

        assert!(got.contains("ATIS Alpha one two zero zero Z"), "{got}");
        assert!(
            got.contains("RWY one eight L"),
            "runway designators keep their suffix: {got}"
        );
        assert!(got.contains("CAVOK Tango two niner"), "{got}");
        assert!(got.contains("QNH one zero zero seven HPA"), "{got}");
        assert!(got.contains("YOU HAVE INFO Alpha"), "{got}");

        assert!(
            got.contains("three six zero zero meters"),
            "the metres M must not be read as the letter Mike: {got}"
        );
        assert!(!got.contains("Mike"), "{got}");
    }

    /// 单位靠**位置**和情报字母分开，不靠一张更长的字母表。
    ///
    /// `3600 M` 的 M 是米；`ATIS M` 的 M 是情报字母。两者是同一个 token，
    /// 区别只在前面那个词——这就是为什么判据是"前一个 token 以数字收尾"。
    #[test]
    fn a_unit_after_a_number_is_not_an_information_letter() {
        assert!(single("LEVEL 3600 M", false).contains("meters"));
        assert!(single("ATIS M 1200Z", false).contains("Mike"));
        assert!(!single("ATIS M 1200Z", false).contains("meters"));
        // 中文那一半念中文单位。
        assert!(single("高度 3600 M", true).contains("米"));
    }

    /// 单位只在紧跟数字时算数——隔一个词就不算。
    ///
    /// 这条钉住的是"前一个 token"而不是"这一行里出现过数字"：后者会把
    /// `QNH 1007 HPA M` 这种写法里的 M 也吃掉。
    #[test]
    fn the_number_has_to_be_the_token_right_before() {
        let got = single("3600 FT M", false);
        assert!(got.contains("Mike"), "{got}");
        assert!(!got.contains("meters"), "{got}");
    }
}
