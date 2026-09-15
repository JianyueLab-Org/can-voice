//! 把 ATIS 文本换成无线电读法：数字逐位念，孤立的大写字母念它的字母词。
//!
//! 照搬 `can-audio/server/ATIS/process.py`。**这一整块是领域知识，不是可以重写的
//! 东西**——大陆的无线电数字读法（洞幺两……拐）和普通中文数字不一样，而
//! `niner` 之所以不是 `nine`，是因为无线电上 nine 和 five 太像。
//!
//! # 中文那一半里的字母仍然念英文，这是已知的缺口
//!
//! Python 版的 `replace_letter` 不分语言，一律用英文 NATO 词，所以中文那一半里
//! 一个孤立的 `A` 会被念成 "Alpha"。这里照搬了那个行为。
//!
//! **它多半是错的**：`can-audio` 的客户端侧（`chinese.py`）为此专门做过研究，
//! 结论是中文播报要念中文字母词（`J` → 朱丽叶），理由正是"念拉丁字母会让 TTS 在
//! 中文句子中间蹦出一个英文字符"。但那张表 `CLAUDE.md` 只给了 J 一个字母，
//! 26 个凑不齐，**猜出来的表比照搬更糟**。
//!
//! 实际影响有限：`text_atis` 的中文那一半多半是 `atis-for-can` 渲染的，
//! 它已经把字母换成中文词了，所以这里的替换根本不会触发。会触发的是
//! vATIS/EuroScope 之类别的来源。补齐那张表之后再改这里。

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

fn nato() -> &'static HashMap<char, &'static str> {
    static M: OnceLock<HashMap<char, &'static str>> = OnceLock::new();
    M.get_or_init(|| NATO.iter().copied().collect())
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

    for token in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        // 孤立的大写字母 → 字母词。
        //
        // **只换孤立的。** `RWY 18L` 里的 L 贴着数字，换掉它会念成
        // "one eight Lima"，而那不是跑道号的读法。
        if let Some(word) = lone_capital(token) {
            out.push_str(word);
            continue;
        }
        expand_digits(token, digits, &mut out);
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
fn lone_capital(token: &str) -> Option<&'static str> {
    let mut chars = token.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    nato().get(&c).copied()
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

    /// 孤立大写字母在**中文那一半**也换成英文 NATO 词，这是照搬 Python 版的行为。
    /// 见 `nato_in_the_chinese_half_is_a_known_gap` 上面那段注释。
    #[test]
    fn nato_in_the_chinese_half_is_a_known_gap() {
        assert!(single("通播 A", true).contains("Alpha"));
    }
    // ——— 真实报文 ———

    /// 拿金文件里那段真实 ZSSS ATIS 过一遍，当回归锚点。
    ///
    /// **它同时钉住一个已知的毛病：`/LEVEL 3600 M` 里的 `M` 是"米"，
    /// 却被念成 "Mike"。** Python 版一模一样（`\s([A-Z])\s` 照样匹配 " M "），
    /// 所以这不是移植引入的，是搬过来的。
    ///
    /// 没有顺手改掉，是因为改对需要一张"哪些孤立大写字母是单位"的表
    /// （M=米、FT=英尺……），而 `M` 在别的位置确实可能就是字母 M。
    /// 拍脑袋加一条规则，风险是把另一处读对的地方读错。先让它可见。
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

        // 已知毛病，见上。哪天修了，改这一行而不是删掉它。
        assert!(
            got.contains("three six zero zero Mike"),
            "the metres M is still read as the letter Mike; if this changed, the fix landed: {got}"
        );
    }
}
