//! 四个客户端的字典和界面代码对得上（#29）。
//!
//! **放在 workspace 里的一个 crate 下面，是因为只有这里 CI 真的跑。** 前端没有
//! 测试框架，`apps/` 也不在 workspace 里——应用自己的单元测试 CI 只编不跑。
//!
//! 旧版 can-audio 的 `test_i18n.py` 钉的几件事，这里一件不少：
//!
//! - 两种语言 key 一样多、没有空的；
//! - 占位符两边一致（英文少一个 `{frequency}`，界面上就少一个频率）；
//! - 英文里没有汉字（半翻译比不翻还糟）；
//! - 代码里用到的 key 字典里都有（一次批量编辑悄悄删掉一段 key 的事，真出过）；
//! - 界面代码里没有写死的中文。
//!
//! 外加两件这个结构才有的：`common` 字典四个客户端逐字节相同，以及 Rust 侧
//! `Message::new("…")` 发出去的 key 在会显示它的那几个客户端的字典里都有。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const APPS: [&str; 4] = ["xpc", "msfs", "controller", "atis"];

/// 英文字典里允许带汉字的 key。**每一条都要说得出为什么。**
const ENGLISH_MAY_CONTAIN_CHINESE: &[(&str, &str)] = &[
    // 语言选择器里每种语言用它自己的名字：看不懂中文的人也认得出"中文"两个字
    // 是一个选项，而一个叫 "Chinese" 的选项对只读中文的人没有用。
    ("*", "language.zh"),
    // 通播客户端"中文台名""中文跑道"两格的示例：填进去的是中文通播要念的内容，
    // 示例只能是中文，写成 "Shanghai Pudong" 反倒教人填错。旧版 can-audio 的
    // station.chinese_name_hint / chinese_runway_hint 也是这么豁免的。
    ("atis", "station.chinese_name_hint"),
    ("atis", "station.chinese_runway_hint"),
];

/// 界面代码里允许出现汉字的文件。**每一条都要说得出为什么。**
const UI_FILES_MAY_CONTAIN_CHINESE: &[&str] = &[];

/// Rust 源码里允许有带汉字的字符串字面量的文件：它们不是界面文字。
const RUST_FILES_MAY_CONTAIN_CHINESE: &[&str] = &[
    // 中文通播的播报内容本身：云量、读法、句读。和界面语言无关——切到英文界面
    // 的管制员照样要播中文通播。
    "crates/can-voice-atis/src/chinese.rs",
    "crates/can-voice-atis/src/readback.rs",
    "crates/can-voice-atis/src/template.rs",
    // 写进通播配置文件的默认名字（"默认""未命名"）：存下来就是那份配置、那份构型
    // 的名字，界面按它选中和改名，是数据不是界面文字；旧版存的也是这几个字。
    // 单独一个文件，profile.rs / vatis.rs 里的报错照样被扫。
    "crates/can-voice-atis/src/default_names.rs",
    // CSL / MSFS 机模的类别名是匹配用的内部键，从来不上界面。
    "crates/can-voice-sim/src/csl.rs",
    "crates/can-voice-sim/src/msfs_models.rs",
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn relative(path: &Path) -> String {
    let root = repo().canonicalize().expect("repo root");
    let full = path.canonicalize().expect("path");
    full.strip_prefix(&root)
        .expect("inside the repo")
        .to_string_lossy()
        .replace('\\', "/")
}

fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{303f}'   // 中文标点
        | '\u{3400}'..='\u{4dbf}'
        | '\u{4e00}'..='\u{9fff}'
        | '\u{ff00}'..='\u{ffef}' // 全角
    )
}

fn has_cjk(s: &str) -> bool {
    s.chars().any(is_cjk)
}

/// 一份字典，拍平成 `a.b.c → 文字`。
type Dict = BTreeMap<String, String>;

fn flatten(prefix: &str, value: &serde_json::Value, out: &mut Dict, file: &Path) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v, out, file);
            }
        }
        serde_json::Value::String(s) => {
            out.insert(prefix.to_string(), s.clone());
        }
        other => panic!(
            "{}: {prefix} is {other}, but every leaf must be a string",
            file.display()
        ),
    }
}

fn load(app: &str, part: &str, lang: &str) -> Dict {
    let file = repo()
        .join("apps")
        .join(app)
        .join("src/locales")
        .join(format!("{part}.{lang}.json"));
    let raw = std::fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let value: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let mut out = Dict::new();
    flatten("", &value, &mut out, &file);
    out
}

/// 一个客户端某种语言的整份字典：`common` 加上它自己的。
fn merged(app: &str, lang: &str) -> Dict {
    let mut d = load(app, "common", lang);
    d.extend(load(app, "app", lang));
    d
}

fn placeholders(s: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = s;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close)
                if close > 0
                    && after[..close]
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_') =>
            {
                out.insert(after[..close].to_string());
                rest = &after[close + 1..];
            }
            _ => rest = after,
        }
    }
    out
}

fn walk(dir: &Path, ext: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if matches!(name, "node_modules" | "dist" | "target" | "gen" | "locales") {
                continue;
            }
            walk(&path, ext, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| ext.contains(&e))
        {
            out.push(path);
        }
    }
}

fn files(dir: &Path, ext: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(dir, ext, &mut out);
    out.sort();
    out
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// 源码里 `t("a.b")` 用到的字面量 key。`t(\`reason.${x}\`)` 这类拼出来的跳过——
/// 那一种靠 TypeScript 的 `Key` 类型和人。
fn frontend_keys(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (n, line) in src.lines().enumerate() {
        let mut from = 0;
        while let Some(at) = line[from..].find("t(") {
            let start = from + at;
            from = start + 2;
            if line[..start].chars().next_back().is_some_and(is_ident) {
                continue; // `set(`、`split(` 之类
            }
            let rest = line[start + 2..].trim_start();
            let Some(quote) = rest
                .chars()
                .next()
                .filter(|c| matches!(c, '"' | '\'' | '`'))
            else {
                continue;
            };
            let body = &rest[1..];
            let Some(end) = body.find(quote) else {
                continue;
            };
            let key = &body[..end];
            if key.contains("${") {
                continue;
            }
            out.push((n + 1, key.to_string()));
        }
    }
    out
}

/// 源码里 `Message::new("…")` 的 key。
fn rust_keys(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (n, line) in src.lines().enumerate() {
        let line = cut_line_comment(line);
        let mut from = 0;
        while let Some(at) = line[from..].find("Message::new(") {
            let start = from + at + "Message::new(".len();
            from = start;
            let rest = line[start..].trim_start();
            let Some(body) = rest.strip_prefix('"') else {
                continue;
            };
            if let Some(end) = body.find('"') {
                out.push((n + 1, body[..end].to_string()));
            }
        }
    }
    out
}

/// 去掉 `.vue` / `.ts` 里的注释：`<!-- -->`、`/* */`、行尾的 `//`。
///
/// 行尾 `//` 前面紧挨着 `:` 的不算注释——那是字符串里的 `https://`。
fn strip_web_comments(src: &str) -> String {
    let mut s = src.to_string();
    for (open, close) in [("<!--", "-->"), ("/*", "*/")] {
        let mut out = String::with_capacity(s.len());
        let mut rest = s.as_str();
        while let Some(i) = rest.find(open) {
            out.push_str(&rest[..i]);
            let after = &rest[i + open.len()..];
            match after.find(close) {
                Some(j) => {
                    // 保留换行，行号才对得上。
                    out.extend(after[..j].chars().filter(|&c| c == '\n'));
                    rest = &after[j + close.len()..];
                }
                None => {
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        s = out;
    }
    s.lines()
        .map(cut_line_comment)
        .collect::<Vec<_>>()
        .join("\n")
}

/// 去掉行尾的 `//` 注释。前面紧挨着 `:` 的不算——那是字符串里的 `https://`。
fn cut_line_comment(line: &str) -> &str {
    let mut from = 0;
    while let Some(at) = line[from..].find("//") {
        let i = from + at;
        if i > 0 && line[..i].ends_with(':') {
            from = i + 2;
            continue;
        }
        return &line[..i];
    }
    line
}

/// 一行 Rust 里的普通字符串字面量（不认原始字符串；用到原始字符串装汉字的文件
/// 在豁免表里）。
fn rust_literals(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '/' && line[i..].starts_with("//") {
            break;
        }
        if c == '\'' {
            // 字符字面量 `'"'` 不能当成字符串的开头。
            if let Some(end) = line[i + 1..].find('\'') {
                if end <= 4 {
                    for _ in 0..=end {
                        chars.next();
                    }
                }
            }
            continue;
        }
        if c != '"' {
            continue;
        }
        let mut lit = String::new();
        while let Some((_, d)) = chars.next() {
            match d {
                '\\' => {
                    if let Some((_, escaped)) = chars.next() {
                        lit.push(escaped);
                    }
                }
                '"' => break,
                _ => lit.push(d),
            }
        }
        out.push(lit);
    }
    out
}

// ——— 字典本身 ———

#[test]
fn both_languages_have_the_same_keys_and_none_is_empty() {
    let mut problems = Vec::new();
    for app in APPS {
        for part in ["common", "app"] {
            let zh = load(app, part, "zh");
            let en = load(app, part, "en");
            let zk: BTreeSet<_> = zh.keys().collect();
            let ek: BTreeSet<_> = en.keys().collect();
            for k in zk.difference(&ek) {
                problems.push(format!("{app}/{part}: {k} is missing in en"));
            }
            for k in ek.difference(&zk) {
                problems.push(format!("{app}/{part}: {k} is missing in zh"));
            }
            for (lang, d) in [("zh", &zh), ("en", &en)] {
                for (k, v) in d {
                    if v.trim().is_empty() {
                        problems.push(format!("{app}/{part}.{lang}: {k} is empty"));
                    }
                }
            }
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// 英文少一个 `{frequency}`，界面上就少一个频率，而谁也不会发现。
#[test]
fn placeholders_agree_between_the_languages() {
    let mut problems = Vec::new();
    for app in APPS {
        let zh = merged(app, "zh");
        let en = merged(app, "en");
        for (k, z) in &zh {
            let Some(e) = en.get(k) else { continue };
            if placeholders(z) != placeholders(e) {
                problems.push(format!(
                    "{app}: {k}: zh has {:?}, en has {:?}",
                    placeholders(z),
                    placeholders(e)
                ));
            }
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// 半翻译比不翻还糟：一句英文里夹着半句中文，两种读者都读不懂。
#[test]
fn english_has_no_chinese_in_it() {
    let exempt = |app: &str, key: &str| {
        ENGLISH_MAY_CONTAIN_CHINESE
            .iter()
            .any(|(a, k)| (*a == "*" || *a == app) && *k == key)
    };
    let mut problems = Vec::new();
    for app in APPS {
        for (k, v) in merged(app, "en") {
            if has_cjk(&v) && !exempt(app, &k) {
                problems.push(format!("{app}: {k} = {v:?}"));
            }
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// 豁免表不许留着过期的条目：翻译好了还挂在表上，下一个人会以为那一条还没翻。
#[test]
fn every_english_exemption_is_still_needed() {
    for (app, key) in ENGLISH_MAY_CONTAIN_CHINESE {
        let apps: Vec<&str> = if *app == "*" {
            APPS.to_vec()
        } else {
            vec![app]
        };
        for a in apps {
            let en = merged(a, "en");
            let v = en
                .get(*key)
                .unwrap_or_else(|| panic!("{a}: exempted key {key} does not exist"));
            assert!(has_cjk(v), "{a}: {key} no longer needs its exemption");
        }
    }
}

/// `common` 是四个共用组件和共用 crate 的话，四个客户端必须是同一份——
/// 改了一个忘了另外三个，同一句话在四个客户端里就是四种说法。
#[test]
fn the_common_dictionaries_are_identical_in_every_app() {
    for lang in ["zh", "en"] {
        let read = |app: &str| {
            std::fs::read(repo().join(format!("apps/{app}/src/locales/common.{lang}.json")))
                .unwrap_or_else(|e| panic!("{app}: common.{lang}.json: {e}"))
        };
        let first = read(APPS[0]);
        for app in &APPS[1..] {
            assert!(
                read(app) == first,
                "apps/{app}/src/locales/common.{lang}.json differs from apps/{}'s",
                APPS[0]
            );
        }
    }
}

/// 两份字典在前端是一层浅合并：同名的顶层命名空间后一份会把前一份整个盖掉。
#[test]
fn an_app_dictionary_does_not_reuse_a_common_namespace() {
    let top = |d: &Dict| -> BTreeSet<String> {
        d.keys()
            .map(|k| k.split('.').next().unwrap_or("").to_string())
            .collect()
    };
    for app in APPS {
        let common = top(&load(app, "common", "zh"));
        let own = top(&load(app, "app", "zh"));
        let shared: Vec<_> = common.intersection(&own).collect();
        assert!(
            shared.is_empty(),
            "{app}: app.zh.json reuses common namespaces {shared:?}"
        );
    }
}

// ——— 代码和字典对得上 ———

#[test]
fn every_key_the_interface_asks_for_exists() {
    let mut problems = Vec::new();
    for app in APPS {
        let dict = merged(app, "zh");
        for file in files(&repo().join(format!("apps/{app}/src")), &["vue", "ts"]) {
            let src = std::fs::read_to_string(&file).expect("read");
            for (line, key) in frontend_keys(&src) {
                if !dict.contains_key(&key) {
                    problems.push(format!("{}:{line}: {key}", relative(&file)));
                }
            }
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// Rust 发出去的 key，会显示它的每一个客户端都得认得。
///
/// 共用 crate 的进 `common`（四个客户端都有）；`can-voice-atis` 只有通播用；
/// 应用自己的只看那一个应用。
#[test]
fn every_key_rust_sends_to_the_interface_exists() {
    let mut problems = Vec::new();
    let mut check = |dir: PathBuf, dict: &Dict| {
        for file in files(&dir, &["rs"]) {
            let src = std::fs::read_to_string(&file).expect("read");
            for (line, key) in rust_keys(&src) {
                if !dict.contains_key(&key) {
                    problems.push(format!("{}:{line}: {key}", relative(&file)));
                }
            }
        }
    };
    let common = load(APPS[0], "common", "zh");
    for entry in std::fs::read_dir(repo().join("crates")).expect("crates") {
        let dir = entry.expect("entry").path();
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if name == "can-voice-i18n" {
            continue;
        }
        let dict = if name == "can-voice-atis" {
            merged("atis", "zh")
        } else {
            common.clone()
        };
        check(dir.join("src"), &dict);
    }
    for app in APPS {
        check(
            repo().join(format!("apps/{app}/src-tauri/src")),
            &merged(app, "zh"),
        );
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// 界面会显示的每一个 NOTICE `kind`，`common` 里都要有一句话。
///
/// 这条链子的前两截已经有人守着：Go 的常量对 `notice_kind`（`control.rs` 的
/// `every_notice_kind_the_server_sends_has_a_constant_here`），`notice_kind` 对
/// `on_notice` 的分派。少的是最后一截——**一个 kind 到了界面上没有对应的话**，
/// 落到的是 `notice.other`，于是飞行员看到的是一行 `range_unavailable` 原文。
///
/// 不在这份名单里的三个 kind，每一个都说得出为什么：`tx_denied` 和 `talker` 被
/// `pump.rs` 的 `on_notice` 变成了别的 Event，从不进 `Snapshot.notices`；
/// `audio_restored` 进了 `snapshot.rs` 是**撤掉** `audio_unavailable` 那一条，
/// 不是再叠一句。
///
/// 放在 `common` 而不是某一个 `app.*.json`：这些 kind 是协议的一部分，四个客户端
/// 收到的是同一条通知，措辞分家只会让同一件事有四种说法。
#[test]
fn every_notice_kind_the_interface_shows_has_a_line_in_the_common_dictionary() {
    /// `on_notice` 分流走的，永远到不了界面。
    const DIVERTED: &[&str] = &["tx_denied", "talker"];
    /// 客户端自己造的本地通知，走同一条展示路径但从不上线
    /// （`crates/can-voice-app/src/snapshot.rs`）。
    const LOCAL: &[&str] = &["audio_unavailable"];
    /// 认不出的 kind 的兜底两句：一条带频率，一条不带。
    const FALLBACK: &[&str] = &["other", "other_on"];

    let src = std::fs::read_to_string(repo().join("crates/can-voice-proto/src/control.rs"))
        .expect("control.rs");
    let module = src
        .split_once("pub mod notice_kind {")
        .expect("notice_kind 模块不见了")
        .1
        .split_once("\n}")
        .expect("notice_kind 模块没有闭合")
        .0;
    let mut kinds: BTreeSet<String> = module
        .lines()
        .filter(|l| l.trim_start().starts_with("pub const"))
        .filter_map(|l| l.split('"').nth(1))
        .map(str::to_string)
        .filter(|k| !DIVERTED.contains(&k.as_str()))
        .collect();

    // 下界：常量的写法一改这条扫描就会一条都取不到，而空集合恒等于通过。
    assert!(
        kinds.len() >= 2,
        "只从 notice_kind 里认出 {} 个会上界面的 kind（{kinds:?}）——是不是常量的写法变了？",
        kinds.len()
    );
    kinds.extend(LOCAL.iter().map(|k| k.to_string()));
    kinds.extend(FALLBACK.iter().map(|k| k.to_string()));

    let mut problems = Vec::new();
    for lang in ["zh", "en"] {
        let common = load(APPS[0], "common", lang);
        for k in &kinds {
            let key = format!("notice.{k}");
            if !common.contains_key(&key) {
                problems.push(format!("common.{lang}.json: {key} 不在字典里"));
            }
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

/// 快照里带 `notices` 的客户端，界面上必须真的把它画出来（#91）。
///
/// 这条要单独钉，是因为**漏掉它的时候什么都不会报**：类型声明在、Rust 一路把
/// 通知填进快照、字典也齐，只是没有哪一行模板读它。症状于是是"能连上、状态绿、
/// 说话没人听见"，而两支飞行员端就这么过了很久。
///
/// 认的是 `.vue` 里有没有调 `noticeText(`——那是三个客户端共用的展示入口。
#[test]
fn every_app_whose_snapshot_carries_notices_renders_them() {
    let mut problems = Vec::new();
    for app in APPS {
        let all = files(&repo().join(format!("apps/{app}/src")), &["vue", "ts"]);
        let read = |f: &PathBuf| std::fs::read_to_string(f).expect("read");
        if !all.iter().any(|f| read(f).contains("notices:")) {
            continue; // 这个客户端的快照里本来就没有 notices
        }
        let rendered = all
            .iter()
            .filter(|f| f.extension().and_then(|e| e.to_str()) == Some("vue"))
            .any(|f| read(f).contains("noticeText("));
        if !rendered {
            problems.push(format!(
                "{app}: 快照里有 notices，却没有一个 .vue 调 noticeText() 把它画出来"
            ));
        }
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}

// ——— 没有写死的中文 ———

/// 界面代码里（注释除外）不许有汉字：每一句话都要进字典。
#[test]
fn the_interface_code_has_no_hardcoded_chinese() {
    let mut problems = Vec::new();
    for app in APPS {
        for file in files(&repo().join(format!("apps/{app}/src")), &["vue", "ts"]) {
            let rel = relative(&file);
            if UI_FILES_MAY_CONTAIN_CHINESE.contains(&rel.as_str()) {
                continue;
            }
            let src = std::fs::read_to_string(&file).expect("read");
            for (n, line) in strip_web_comments(&src).lines().enumerate() {
                if has_cjk(line) {
                    problems.push(format!("{rel}:{}: {}", n + 1, line.trim()));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "wording belongs in src/locales/*.json:\n{}",
        problems.join("\n")
    );
}

/// Rust 侧交给界面的也不许是拼好的中文：切到英文之后它还是中文。
///
/// 扫的是字符串字面量，注释不算（注释本来就该是中文），`mod tests` 以下不算
/// （断言消息不上界面）。
#[test]
fn rust_hands_the_interface_keys_not_chinese() {
    let mut roots = vec![];
    for entry in std::fs::read_dir(repo().join("crates")).expect("crates") {
        roots.push(entry.expect("entry").path().join("src"));
    }
    for app in APPS {
        roots.push(repo().join(format!("apps/{app}/src-tauri/src")));
    }
    let mut problems = Vec::new();
    for root in roots {
        for file in files(&root, &["rs"]) {
            let rel = relative(&file);
            if RUST_FILES_MAY_CONTAIN_CHINESE.contains(&rel.as_str()) {
                continue;
            }
            let src = std::fs::read_to_string(&file).expect("read");
            for (n, line) in src.lines().enumerate() {
                let t = line.trim_start();
                if t.starts_with("mod tests") {
                    break;
                }
                if t.starts_with("//") {
                    continue;
                }
                if rust_literals(line).iter().any(|l| has_cjk(l)) {
                    problems.push(format!("{rel}:{}: {}", n + 1, t));
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "return a can_voice_i18n::Message instead:\n{}",
        problems.join("\n")
    );
}

// ——— 这些扫描自己要对 ———

#[test]
fn the_scanners_find_what_they_should_and_nothing_else() {
    assert_eq!(
        placeholders("频率 {frequency} 不在 {low}–{high} 之间 {not a name}"),
        ["frequency", "high", "low"].map(String::from).into()
    );
    assert_eq!(
        frontend_keys(r#"{{ t("settings.title") }} set("x.y") t(`reason.${r}`) t('a.b', { n })"#),
        vec![(1, "settings.title".to_string()), (1, "a.b".to_string())]
    );
    assert_eq!(
        rust_keys(r#"Err(Message::new("error.log.no_file").with("x", 1)) // Message::new("no")"#),
        vec![(1, "error.log.no_file".to_string())]
    );
    assert_eq!(
        strip_web_comments(
            "a <!-- 中文\n注释 --> b\nconst u = \"https://x\"; // 中文\n/* 中\n文 */c"
        ),
        "a \n b\nconst u = \"https://x\"; \n\nc"
    );
    assert_eq!(
        rust_literals(r#"let q = '"'; f("中文", "x\"y"); // "注释""#),
        vec!["中文".to_string(), "x\"y".to_string()]
    );
}

// ——— 界面的颜色只有一处声明 ———

/// can-audio 那套语义色，对应它四个客户端逐字节相同的 `controller/theme.py`。
///
/// **不在组件里写十六进制字面量**：旧版只有一个 theme.py，而这边同一个颜色
/// 散在四个客户端的组件里，改一处就漏三处。
#[test]
fn every_app_declares_the_can_audio_palette() {
    const TOKENS: &[(&str, &str)] = &[
        ("--can-off", "#436384"),
        ("--can-on", "#28a745"),
        ("--can-active", "#c7861d"),
        ("--can-muted", "#dc3545"),
        ("--can-theme", "#5eb1bf"),
        ("--can-idle", "#8b90a4"),
        ("--can-surface", "#252839"),
        ("--can-window", "#2c2f45"),
    ];
    for app in APPS {
        let css = std::fs::read_to_string(repo().join(format!("apps/{app}/src/style.css")))
            .unwrap_or_else(|e| panic!("{app}: style.css: {e}"));
        for (name, value) in TOKENS {
            let decl = format!("{name}: {value}");
            assert!(
                css.contains(&decl),
                "{app}: style.css does not declare `{decl}`"
            );
        }
    }
}

// ——— 共用的前端文件是逐字节相同的副本 ———

/// 哪些前端文件是共用的，以及每个该出现在哪几个客户端里。
///
/// **这张表就是"共用"这件事的声明处。** 仓库里共用代码靠复制而不是抽包
/// （见 README 里桌面端不进 workspace 那一段），所以唯一的防线是断言副本相同：
/// 改了一个客户端而忘了其余三个，在这里失败；某个客户端多出一份没登记的同名
/// 文件，也在这里失败。没登记的文件按定义就是那个客户端自己的。
const SHARED_FRONTEND: &[(&str, &[&str])] = &[
    ("appearance.ts", &["controller", "atis", "xpc", "msfs"]),
    ("i18n.ts", &["controller", "atis", "xpc", "msfs"]),
    ("main.ts", &["controller", "atis", "xpc", "msfs"]),
    ("style.css", &["controller", "atis", "xpc", "msfs"]),
    (
        "components/LogPanel.vue",
        &["controller", "atis", "xpc", "msfs"],
    ),
    (
        "components/SettingsCommon.vue",
        &["controller", "atis", "xpc", "msfs"],
    ),
    (
        "components/StartupGate.vue",
        &["controller", "atis", "xpc", "msfs"],
    ),
    (
        "components/UpdateBanner.vue",
        &["controller", "atis", "xpc", "msfs"],
    ),
    (
        "components/WindowToggles.vue",
        &["controller", "atis", "xpc", "msfs"],
    ),
];

#[test]
fn shared_frontend_files_are_identical_in_every_app_that_carries_them() {
    for (file, owners) in SHARED_FRONTEND {
        let path = |app: &str| repo().join(format!("apps/{app}/src/{file}"));
        let first =
            std::fs::read(path(owners[0])).unwrap_or_else(|e| panic!("{}: {file}: {e}", owners[0]));
        for app in &owners[1..] {
            let other = std::fs::read(path(app)).unwrap_or_else(|e| panic!("{app}: {file}: {e}"));
            assert!(
                other == first,
                "apps/{app}/src/{file} differs from apps/{}'s",
                owners[0]
            );
        }
        for app in &APPS {
            assert!(
                owners.contains(app) || !path(app).exists(),
                "apps/{app}/src/{file} exists but is not registered for {app} in SHARED_FRONTEND"
            );
        }
    }
}
