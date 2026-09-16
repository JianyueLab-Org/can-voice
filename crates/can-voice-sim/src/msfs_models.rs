//! 机型码 → MSFS 机模标题。
//!
//! X-Plane 那边是 CSL 包（见 [`crate::csl`]），按目录里的 `xsb_aircraft.txt`
//! 找模型文件。**MSFS 没有这一套**：`SimConnect_AICreateNonATCAircraft` 要的是
//! 一个 **container title**——`aircraft.cfg` 里 `title=` 那一行的字符串，例如
//! `"Airbus A320 Neo Asobo"`。它不是机型码，也没有任何地方把两者对起来。
//!
//! # 为什么是一串候选而不是一个答案
//!
//! 标题存不存在取决于**这台机器装了什么**：版本（2020 / 2024）、版本等级
//! （Standard / Deluxe / Premium）、买没买 DLC、装没装第三方 AI 机模包。
//! 同一份表在两台机器上命中率不一样，而 `AICreateNonATCAircraft` 对一个不存在
//! 的标题是**直接失败**，不会自己找替身。
//!
//! 所以这里不返回"那个标题"，返回**一串按可信度排好的候选**：
//!
//! ```text
//! 1. 机型本身              A20N → "Airbus A320 Neo Asobo"
//! 2. 同族机型              A21N 没有就退到 A20N 的标题
//! 3. 同类机身              宽体顶宽体、支线顶支线
//! 4. 兜底                  一定存在的那一个
//! ```
//!
//! 调用方第 n 次重试就用第 n 个候选（见 [`crate::inject::Injector::attempt`]）。
//! **一架画不出来的飞机比一架涂装不对的飞机危险得多**，所以这串一定以一个
//! 兜底结尾，而不是以"没有"结尾。
//!
//! # 用户可以自己换一张表
//!
//! 装了 FSLTL、AIG 这类 AI 机模包的人，机库里的标题比这张表全得多。
//! [`load_overrides`] 读一个 JSON，键是机型码，值是标题或者标题数组；
//! 读到的候选**排在内置表前面**，而不是替换掉它——第三方包也会缺机型，
//! 缺的时候还得落回内置的兜底。

use std::collections::HashMap;
use std::path::Path;

/// 一定存在的那一个。**MSFS 的每个版本、每个等级都带 C172。**
///
/// 拿它顶一架 B777 视觉上很荒唐，但这是候选链的最后一节——走到这里说明前面
/// 每一个都没装。荒唐的大小总好过天上是空的。
pub const FALLBACK: &str = "Cessna Skyhawk G1000 Asobo";

/// 内置的机型码 → 标题表。
///
/// **只收第一方（Asobo）机模**，因为只有它们的标题在不同机器上是同一个字符串。
/// 第三方机模的标题带作者前缀、带涂装名，同一架飞机在两个人的机库里是两个
/// 字符串——那种东西写进内置表只会制造"在我这儿是好的"。
///
/// 收得不全是故意的：宁可让一个机型顺着同族退到隔壁，也不要一行猜出来的标题。
pub const TITLES: [(&str, &str); 18] = [
    ("A20N", "Airbus A320 Neo Asobo"),
    ("A310", "Airbus A310-300 Asobo"),
    ("B748", "Boeing 747-8i Asobo"),
    ("B78X", "Boeing 787-10 Asobo"),
    ("B38M", "Boeing 737 MAX 8 Asobo"),
    ("AT76", "ATR 72-600 Asobo"),
    ("C152", "Cessna 152 Asobo"),
    ("C172", "Cessna Skyhawk G1000 Asobo"),
    ("C25C", "Cessna CJ4 Citation Asobo"),
    ("C700", "Cessna Citation Longitude Asobo"),
    ("TBM9", "Daher TBM 930 Asobo"),
    ("BE20", "Beechcraft King Air 350i Asobo"),
    ("DA40", "Diamond DA40 NG Asobo"),
    ("DA62", "Diamond DA62 Asobo"),
    ("SR22", "Cirrus SR22 Asobo"),
    ("P28A", "Diamond DA40 NG Asobo"),
    ("DH8D", "ATR 72-600 Asobo"),
    ("E75L", "Cessna Citation Longitude Asobo"),
];

/// 按类别的兜底：同族和同类都没命中时，至少让大小对得上。
///
/// 键是 [`crate::csl::category_of`] 的返回值。
const BY_CATEGORY: [(&str, &str); 4] = [
    ("宽体", "Boeing 747-8i Asobo"),
    ("窄体", "Airbus A320 Neo Asobo"),
    ("支线", "ATR 72-600 Asobo"),
    ("通航", FALLBACK),
];

fn builtin(icao: &str) -> Option<&'static str> {
    TITLES
        .iter()
        .find(|(code, _)| code.eq_ignore_ascii_case(icao))
        .map(|(_, title)| *title)
}

fn by_category(icao: &str) -> &'static str {
    let category = crate::csl::category_of(icao);
    BY_CATEGORY
        .iter()
        .find(|(name, _)| *name == category)
        .map(|(_, title)| *title)
        .unwrap_or(FALLBACK)
}

/// 用户自己那张表。键是机型码，值是一个标题或者一串标题。
#[derive(Debug, Default, Clone)]
pub struct Overrides {
    by_icao: HashMap<String, Vec<String>>,
}

impl Overrides {
    /// 用一张现成的表建。给本机扫出来的机库用（见 [`crate::msfs_hangar`]）。
    pub fn from_pairs(pairs: impl IntoIterator<Item = (String, Vec<String>)>) -> Self {
        let mut by_icao: HashMap<String, Vec<String>> = HashMap::new();
        for (icao, titles) in pairs {
            let icao = icao.trim().to_uppercase();
            let titles: Vec<String> = titles.into_iter().filter(|t| !t.is_empty()).collect();
            if icao.is_empty() || titles.is_empty() {
                continue;
            }
            by_icao.entry(icao).or_default().extend(titles);
        }
        Self { by_icao }
    }

    /// 把另一张表并进来。**自己已有的候选排在前面。**
    ///
    /// 用在"手写的 `titles.json` 并上本机扫出来的机库"这一处，次序是有讲究的：
    /// 手写的是用户明确说过的，扫出来的是推断的。
    pub fn merge(&mut self, other: Self) {
        for (icao, titles) in other.by_icao {
            let slot = self.by_icao.entry(icao).or_default();
            for title in titles {
                if !slot.contains(&title) {
                    slot.push(title);
                }
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.by_icao.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_icao.len()
    }

    /// 从 JSON 文本读。**读不懂就当没有**——一份写坏的覆盖表不该让他机全部
    /// 消失，那比涂装不对严重得多。
    pub fn parse(text: &str) -> Self {
        let Ok(raw) = serde_json::from_str::<HashMap<String, serde_json::Value>>(text) else {
            return Self::default();
        };
        let mut by_icao = HashMap::new();
        for (icao, value) in raw {
            let titles: Vec<String> = match value {
                serde_json::Value::String(s) => vec![s],
                serde_json::Value::Array(items) => items
                    .into_iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
                _ => continue,
            };
            let titles: Vec<String> = titles.into_iter().filter(|t| !t.is_empty()).collect();
            if titles.is_empty() {
                continue;
            }
            by_icao.insert(icao.to_ascii_uppercase(), titles);
        }
        Self { by_icao }
    }

    fn titles(&self, icao: &str) -> &[String] {
        self.by_icao
            .get(&icao.to_ascii_uppercase())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// 读用户那张表。文件不在、读不了、写坏了，都返回一张空表。
pub fn load_overrides(path: &Path) -> Overrides {
    match std::fs::read_to_string(path) {
        Ok(text) => Overrides::parse(&text),
        Err(_) => Overrides::default(),
    }
}

/// 一个机型码的候选标题，按可信度排好。
///
/// **一定非空，且一定含 [`FALLBACK`]**——也就是说调用方一路试下去，总会试到
/// 一个这台机器上确实有的机模。试完了就该放弃这架，而不是掉进"还有没有下一个"
/// 的循环。
///
/// 注意 [`FALLBACK`] 不保证在**最后**：C172 自己的标题就是它，那时它排在第一。
pub fn candidates(icao: &str, overrides: &Overrides) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push = |title: &str| {
        if !title.is_empty() && !out.iter().any(|t| t == title) {
            out.push(title.to_string());
        }
    };

    // 1. 用户表里这个机型的，整串照抄，顺序是用户写的顺序。
    for title in overrides.titles(icao) {
        push(title);
    }
    // 2. 内置表里这个机型的。
    if let Some(title) = builtin(icao) {
        push(title);
    }
    // 3. 同族。用户表和内置表都试——装了第三方包的人，同族里更可能有货。
    for sibling in crate::csl::family_of(icao) {
        if sibling.eq_ignore_ascii_case(icao) {
            continue;
        }
        for title in overrides.titles(sibling) {
            push(title);
        }
        if let Some(title) = builtin(sibling) {
            push(title);
        }
    }
    // 4. 同类机身。
    push(by_category(icao));
    // 5. 兜底。**这一步保证了返回值非空。**
    push(FALLBACK);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手写的排在扫出来的前面：前者是用户明确说过的。
    #[test]
    fn the_hand_written_table_wins_over_the_scanned_one() {
        let mut hand = Overrides::parse(r#"{"B738": ["手写的"]}"#);
        hand.merge(Overrides::from_pairs([(
            "b738".to_string(),
            vec!["扫出来的".to_string()],
        )]));
        let got = candidates("B738", &hand);
        assert_eq!(got[0], "手写的");
        assert_eq!(got[1], "扫出来的");
    }

    /// 扫出来的表照样要落回内置的兜底：第三方包也会缺机型。
    #[test]
    fn a_scanned_table_still_falls_back() {
        let scanned = Overrides::from_pairs([("B738".to_string(), vec!["某个 738".to_string()])]);
        let got = candidates("A320", &scanned);
        assert!(got.contains(&FALLBACK.to_string()));
    }

    /// **一定非空，而且一定含兜底。** 候选链要是可能为空，调用方就得在每一处
    /// 判空；要是可能不含兜底，就会有机型一路试到底也画不出来。
    ///
    /// 不要求兜底在**最后**——C172 自己的标题就是兜底，那时它排第一。
    #[test]
    fn every_type_reaches_something_that_exists() {
        let none = Overrides::default();
        for icao in [
            "A20N",
            "B738",
            "B77W",
            "A388",
            "C172",
            "CRJ9",
            "ZZZZ",
            "",
            "不是机型",
        ] {
            let list = candidates(icao, &none);
            assert!(!list.is_empty(), "{icao} 的候选是空的");
            assert!(list.iter().any(|t| t == FALLBACK), "{icao}: {list:?}");
        }
    }

    #[test]
    fn a_type_we_know_comes_first() {
        let list = candidates("A20N", &Overrides::default());
        assert_eq!(list[0], "Airbus A320 Neo Asobo");
    }

    /// 机型码不区分大小写——FSD 上什么都收得到。
    #[test]
    fn the_type_code_is_case_insensitive() {
        let none = Overrides::default();
        assert_eq!(candidates("a20n", &none)[0], candidates("A20N", &none)[0]);
    }

    /// 同族顶替：A21N 内置表里没有，应当退到同族的 A20N。
    #[test]
    fn an_unknown_type_falls_back_to_its_family() {
        let list = candidates("A21N", &Overrides::default());
        assert_eq!(list[0], "Airbus A320 Neo Asobo", "{list:?}");
    }

    /// **宽体顶宽体。** 同族里一个都没有的时候，至少别拿 C172 去顶 B777。
    #[test]
    fn a_widebody_is_replaced_by_a_widebody_before_the_fallback() {
        let list = candidates("B77W", &Overrides::default());
        let fallback_at = list.iter().position(|t| t == FALLBACK).expect("兜底");
        let heavy_at = list
            .iter()
            .position(|t| t == "Boeing 747-8i Asobo")
            .expect("宽体");
        assert!(heavy_at < fallback_at, "{list:?}");
    }

    /// 候选不重复——同族里两个机型指向同一个标题时会撞上。重复的候选等于
    /// 白白多试一次同样会失败的东西。
    #[test]
    fn a_title_is_never_offered_twice() {
        let list = candidates("B739", &Overrides::default());
        let mut sorted = list.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), list.len(), "{list:?}");
    }

    #[test]
    fn a_users_own_table_wins() {
        let o = Overrides::parse(r#"{"A20N": "FSLTL A320neo CCA"}"#);
        let list = candidates("A20N", &o);
        assert_eq!(list[0], "FSLTL A320neo CCA");
        // 内置的那个还在后面——第三方包也可能没装全。
        assert!(
            list.contains(&"Airbus A320 Neo Asobo".to_string()),
            "{list:?}"
        );
    }

    #[test]
    fn a_user_can_give_several_in_order() {
        let o = Overrides::parse(r#"{"B738": ["AIG B738 CSN", "AIG B738 generic"]}"#);
        let list = candidates("B738", &o);
        assert_eq!(&list[..2], &["AIG B738 CSN", "AIG B738 generic"]);
    }

    /// **写坏的覆盖表当没有。** 让他机全部消失比涂装不对严重得多。
    #[test]
    fn a_broken_override_file_is_ignored_rather_than_fatal() {
        assert!(Overrides::parse("{ 这不是 json").is_empty());
        assert!(Overrides::parse("").is_empty());
        // 类型不对的那几个条目跳过，别的照收。
        let o = Overrides::parse(r#"{"A20N": 42, "B738": "good", "C172": [], "B744": [7]}"#);
        assert_eq!(o.len(), 1);
        assert_eq!(candidates("B738", &o)[0], "good");
    }

    /// 文件不在就是一张空表，不是错误——绝大多数人不会有这个文件。
    #[test]
    fn a_missing_override_file_is_not_an_error() {
        assert!(load_overrides(Path::new("/nonexistent/titles.json")).is_empty());
    }

    /// 内置表里的机型码都是四位——写错一个就是永远命不中，而且看不出来。
    #[test]
    fn the_builtin_table_is_well_formed() {
        for (icao, title) in TITLES {
            assert_eq!(icao.len(), 4, "{icao}");
            assert!(
                icao.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
                "{icao}"
            );
            assert!(!title.is_empty(), "{icao}");
        }
    }

    /// 类别兜底表要盖住 [`crate::csl::category_of`] 会返回的每一个值，
    /// 否则那一类会直接跳到 C172。
    #[test]
    fn every_category_has_a_title() {
        for icao in ["B77W", "A320", "CRJ9", "C172"] {
            let category = crate::csl::category_of(icao);
            assert!(
                BY_CATEGORY.iter().any(|(name, _)| *name == category),
                "类别 {category}（{icao}）没有兜底"
            );
        }
    }
}
