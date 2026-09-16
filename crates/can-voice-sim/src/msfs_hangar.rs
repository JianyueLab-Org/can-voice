//! 扫本机的 MSFS 机库，认出装着哪些机模。
//!
//! 内置表只有 18 条第一方标题（见 [`crate::msfs_models`]），**装了 FSLTL 或者 AIG
//! 的人一个机模都不会被发现**，只能一条条手写 `titles.json`。这个模块补的是另一半。
//!
//! 扫出来的表是**本机的**，不进内置表——内置表只收第一方，因为只有它们的标题在
//! 不同机器上是同一个字符串。两者合起来用：本机的排在前面，缺的落回内置的兜底。
//!
//! 结构照 [`crate::csl`]（X-Plane 那一侧）来：**"找"和"解"分开**，解析只吃文本，
//! 所以在任何平台上都测得到——而找安装只在 Windows 上有意义。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// 一个涂装。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Livery {
    /// `AICreateNonATCAircraft` 要的那个字符串，必须逐字一致。
    pub title: String,
    /// `[GENERAL]` 里的机型码，清洗过；没有就是空的。
    pub icao: String,
    /// `icao_airline`，多半是空的。
    pub airline: String,
}

/// 扫出来的机库。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Hangar {
    /// 机型码 → 标题，按扫到的顺序。
    pub by_icao: BTreeMap<String, Vec<String>>,
    /// 一共认出多少个涂装（含没有机型码的）。
    pub liveries: usize,
    /// 读了多少个 `aircraft.cfg`。
    pub files: usize,
}

/// 走到 `aircraft.cfg` 时不再往下走的目录名。
pub const PRUNE: [&str; 6] = [
    "texture",
    "sound",
    "panel",
    "model",
    "attachments",
    "contentinfo",
];

/// 从 `UserCfg.opt` 里取包目录。
///
/// 这是**唯一靠谱的来源**：包目录常被搬到另一块盘，只靠猜路径会安静地漏掉
/// 整个安装。格式是一行一个 `键 "值"`。
pub fn parse_user_cfg(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if !line.to_lowercase().starts_with("installedpackagespath") {
            continue;
        }
        // **按空白切，不是按一个空格切**：见过用制表符分隔的写法，
        // 只认空格的话整个安装会被安静地漏掉。路径自己带空格没关系，只切第一段。
        let value = line.split_once(char::is_whitespace)?.1.trim();
        let value = value.trim_matches('"').trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

/// 去掉行尾注释和引号。这些文件是人手写的，格式相当随意。
fn clean(value: &str) -> String {
    let mut v = value;
    if let Some(i) = v.find(';') {
        v = &v[..i];
    }
    if let Some(i) = v.find("//") {
        v = &v[..i];
    }
    v.trim()
        .trim_matches(|c| c == '"' || c == '\'')
        .trim()
        .to_string()
}

/// 清洗机型码。
///
/// 真实安装里这个字段并不干净——见过 `"A359 ULR"` 这种带后缀的写法。取第一段，
/// 大写，长度 2–4 且全是字母数字才算数。**对不上就当没有**：宁可退到同族匹配，
/// 也不要让一个假的机型码进索引，那会让别的飞机永远匹配不到它。
fn clean_icao(value: &str) -> String {
    let first = value.split_whitespace().next().unwrap_or("").to_uppercase();
    let len = first.chars().count();
    if (2..=4).contains(&len) && first.chars().all(char::is_alphanumeric) {
        first
    } else {
        String::new()
    }
}

/// 解一个 `aircraft.cfg`。
///
/// 只读两样：`[GENERAL]` 的 `icao_type_designator`，和每一个 `[FLTSIM.n]` 的
/// `title`（必需）与 `icao_airline`（可选）。那一个机型码**抄到这个文件的每一个
/// 涂装上**。读不出来的跳过就好——一个装得不干净的飞机不该让整张表建不起来。
pub fn parse_aircraft_cfg(text: &str) -> Vec<Livery> {
    let mut general_icao = String::new();
    let mut out: Vec<Livery> = Vec::new();
    let mut section = String::new();
    let mut title = String::new();
    let mut airline = String::new();
    let mut in_fltsim = false;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with("//") {
            continue;
        }
        if line.starts_with('[') {
            if in_fltsim && !title.is_empty() {
                out.push(Livery {
                    title: std::mem::take(&mut title),
                    icao: String::new(),
                    airline: std::mem::take(&mut airline),
                });
            }
            title.clear();
            airline.clear();
            section = line.trim_matches(|c| c == '[' || c == ']').to_lowercase();
            in_fltsim = section.starts_with("fltsim.");
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_lowercase();
        let value = clean(value);
        if section == "general" && key == "icao_type_designator" && general_icao.is_empty() {
            general_icao = clean_icao(&value);
        } else if in_fltsim {
            // 重复的键取第一个：这些文件里重复键是常态。
            if key == "title" && title.is_empty() {
                title = value;
            } else if key == "icao_airline" && airline.is_empty() {
                airline = value.to_uppercase();
            }
        }
    }
    if in_fltsim && !title.is_empty() {
        out.push(Livery {
            title,
            icao: String::new(),
            airline,
        });
    }
    for livery in &mut out {
        livery.icao.clone_from(&general_icao);
    }
    out
}

/// 把一批涂装攒成机库。
pub fn collect(liveries: Vec<Livery>, files: usize) -> Hangar {
    let mut by_icao: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let counted = liveries.len();
    for livery in liveries {
        // 没有机型码的进不了索引，但**数得上**：涂装几百个而机型是 0，
        // 正是"读到的全是附加件那类没有机型码的配置"的信号。
        if livery.icao.is_empty() {
            continue;
        }
        if seen
            .entry(livery.icao.clone())
            .or_default()
            .insert(livery.title.clone())
        {
            by_icao.entry(livery.icao).or_default().push(livery.title);
        }
    }
    Hangar {
        by_icao,
        liveries: counted,
        files,
    }
}

/// 在一个包目录底下找所有 `aircraft.cfg`。
///
/// **跟着链接走。** Windows 的目录联接（junction）在这里等价于符号链接，而
/// "把包目录搬到另一块盘、原地留个 junction"是社区里最普遍的做法之一，商店版
/// 还常把 `Official` 做成 junction——不跟着走的话整个 `Official` 会被安静跳过，
/// 表现是只扫到 Community 里那几个零星附加件。
///
/// 代价是要自己防环：跟着链接走可能绕回上层目录。按规范化后的路径记账。
pub fn find_aircraft_cfgs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    walk(root, &mut seen, &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, seen: &mut BTreeSet<PathBuf>, out: &mut Vec<PathBuf>) {
    let real = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    if !seen.insert(real) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        // `metadata` 跟着链接走，`symlink_metadata` 不跟——这里要的是前者。
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        if meta.is_dir() {
            // `attachments/` 尤其要紧：Fenix 之类的插件机在那底下放了几十个
            // 部件配置，每个都有 `[GENERAL]` 和 title 却没有机型码——当成飞机
            // 会污染匹配表，还可能被当成兜底顶到别人头上。
            if PRUNE.contains(&name.as_str()) || name.starts_with("texture.") {
                continue;
            }
            subdirs.push(path);
        } else if name == "aircraft.cfg" {
            out.push(path);
        }
    }
    for dir in subdirs {
        walk(&dir, seen, out);
    }
}

/// 从一份 `UserCfg.opt` 里读出存在的包目录。
fn root_from_user_cfg(cfg: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(cfg).ok()?;
    let dir = PathBuf::from(parse_user_cfg(&text)?);
    dir.is_dir().then_some(dir)
}

/// 猜这台机器上的包目录。Windows 之外一律空。
///
/// **`UserCfg.opt` 优先，猜路径垫底**：包目录常被搬到另一块盘，只猜路径会安静地
/// 漏掉整个安装。
pub fn default_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        for name in [
            "Microsoft Flight Simulator",
            "Microsoft Flight Simulator 2024",
        ] {
            let dir = PathBuf::from(&appdata).join(name);
            let found = root_from_user_cfg(&dir.join("UserCfg.opt"))
                .or_else(|| dir.join("Packages").is_dir().then(|| dir.join("Packages")));
            if let Some(found) = found {
                if !roots.contains(&found) {
                    roots.push(found);
                }
            }
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        // **2024 版在商店里的包名不是 FlightSimulator 而是 Limitless**，
        // 只认前者会整个漏掉商店版 2024。
        for pkg in [
            "Microsoft.FlightSimulator_8wekyb3d8bbwe",
            "Microsoft.Limitless_8wekyb3d8bbwe",
        ] {
            let cfg = PathBuf::from(&local)
                .join("Packages")
                .join(pkg)
                .join("LocalCache")
                .join("UserCfg.opt");
            if let Some(found) = root_from_user_cfg(&cfg) {
                if !roots.contains(&found) {
                    roots.push(found);
                }
            }
        }
    }
    roots
}

/// 扫一批包目录。
pub fn scan(roots: &[PathBuf]) -> Hangar {
    let mut liveries = Vec::new();
    let mut files = 0;
    for root in roots {
        for path in find_aircraft_cfgs(root) {
            let Ok(raw) = std::fs::read(&path) else {
                continue;
            };
            files += 1;
            // BOM、各种编码都见得到；读不出 UTF-8 的字节按 lossy 处理，
            // 总比整个文件跳过强——要的只是 title 和机型码。
            let text =
                String::from_utf8_lossy(raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&raw));
            liveries.extend(parse_aircraft_cfg(&text));
        }
    }
    collect(liveries, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 值和键之间**按空白切，不能只按一个空格切**：见过用制表符分隔的写法，
    /// 那样整个安装会被安静地漏掉。
    #[test]
    fn a_tab_separates_the_key_from_the_path_just_as_a_space_does() {
        assert_eq!(
            parse_user_cfg("InstalledPackagesPath\t\"D:\\MSFS2022\"").as_deref(),
            Some("D:\\MSFS2022")
        );
    }

    /// 路径里带空格是常态，只切第一段。
    #[test]
    fn a_path_may_contain_spaces() {
        assert_eq!(
            parse_user_cfg("InstalledPackagesPath \"C:\\Flight Sim\\Packages\"").as_deref(),
            Some("C:\\Flight Sim\\Packages")
        );
    }

    /// 键名大小写不认死。
    #[test]
    fn the_key_is_matched_case_insensitively() {
        assert_eq!(
            parse_user_cfg("installedpackagespath \"D:\\p\"").as_deref(),
            Some("D:\\p")
        );
    }

    /// 别的行不管；没有这一项就是没有。
    #[test]
    fn without_the_key_there_is_nothing() {
        assert_eq!(parse_user_cfg("Version 12\nSomethingElse \"x\""), None);
    }

    /// 一个文件里可以有好几个 `[FLTSIM.n]`，机型码在 `[GENERAL]` 里，
    /// **要抄到每一个涂装上**。
    #[test]
    fn every_fltsim_block_inherits_the_general_icao() {
        let got = parse_aircraft_cfg(
            "[GENERAL]\nicao_type_designator = \"B738\"\n\
             [FLTSIM.0]\ntitle = \"Boeing 737-800 Air China\"\nicao_airline = \"CCA\"\n\
             [FLTSIM.1]\ntitle = \"Boeing 737-800 China Eastern\"\n",
        );
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|l| l.icao == "B738"));
        assert_eq!(got[0].airline, "CCA");
        assert_eq!(got[1].airline, "");
    }

    /// **没有 title 就不是一个涂装。** title 就是交给 SimConnect 的东西，
    /// 不能自己拼，只能读出来。
    #[test]
    fn a_block_without_a_title_is_not_a_livery() {
        let got =
            parse_aircraft_cfg("[GENERAL]\nicao_type_designator=B738\n[FLTSIM.0]\nui_type=x\n");
        assert!(got.is_empty());
    }

    /// 这些文件是人手写的：重复的键、注释、引号、没有值的行都见得到。
    /// **一个装得不干净的飞机不该让整张表建不起来。**
    #[test]
    fn a_messy_file_still_yields_what_it_can() {
        let got = parse_aircraft_cfg(
            "[GENERAL]\nicao_type_designator = A320 ; 注释\n\
             [FLTSIM.0]\ntitle = \"A320neo\" // 另一种注释\ntitle = \"A320neo\"\nbroken\n",
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].icao, "A320");
        assert_eq!(got[0].title, "A320neo");
    }

    /// 真实安装里这个字段并不干净：见过 `"A359 ULR"` 这种带后缀的写法。
    /// 取第一段；对不上就当没有——**宁可退到同族匹配，也不要让一个假的机型码进表**，
    /// 那会让别的飞机永远匹配不到它。
    #[test]
    fn the_type_code_is_cleaned_and_a_bad_one_is_dropped() {
        let ok = parse_aircraft_cfg(
            "[GENERAL]\nicao_type_designator=\"A359 ULR\"\n[FLTSIM.0]\ntitle=x\n",
        );
        assert_eq!(ok[0].icao, "A359");
        let bad =
            parse_aircraft_cfg("[GENERAL]\nicao_type_designator=\"?\"\n[FLTSIM.0]\ntitle=x\n");
        assert_eq!(bad[0].icao, "", "对不上的机型码要丢掉，不能进表");
        let long = parse_aircraft_cfg(
            "[GENERAL]\nicao_type_designator=\"TOOLONG\"\n[FLTSIM.0]\ntitle=x\n",
        );
        assert_eq!(long[0].icao, "");
    }

    /// 没有机型码的涂装仍然算数（进不了索引，但数得上），
    /// 因为"涂装几百个而机型是 0"正是"读到的全是附加件"的信号。
    #[test]
    fn liveries_without_a_type_are_counted_but_not_indexed() {
        let h = collect(
            vec![
                Livery {
                    title: "A".into(),
                    icao: "B738".into(),
                    airline: String::new(),
                },
                Livery {
                    title: "B".into(),
                    icao: String::new(),
                    airline: String::new(),
                },
            ],
            1,
        );
        assert_eq!(h.liveries, 2);
        assert_eq!(h.by_icao.len(), 1);
        assert_eq!(h.by_icao["B738"], vec!["A".to_string()]);
    }

    /// 同一个标题出现两次只留一个：同一个包被装了两遍是常事。
    #[test]
    fn a_duplicate_title_is_kept_once() {
        let h = collect(
            vec![
                Livery {
                    title: "A".into(),
                    icao: "B738".into(),
                    airline: String::new(),
                },
                Livery {
                    title: "A".into(),
                    icao: "B738".into(),
                    airline: String::new(),
                },
            ],
            2,
        );
        assert_eq!(h.by_icao["B738"], vec!["A".to_string()]);
    }

    /// `attachments/` 底下不是飞机。**Fenix 之类的插件机在那底下放了几十个部件配置**，
    /// 每个都有 `[GENERAL]` 和 title 却没有机型码——当成飞机会污染匹配表，
    /// 还可能被当成兜底顶到别人头上。
    #[test]
    fn attachments_are_not_aircraft() {
        let dir = tempdir();
        let deep = dir.join("FNX_32X/attachments/fnx/x/config");
        std::fs::create_dir_all(&deep).expect("mkdir");
        std::fs::write(deep.join("aircraft.cfg"), "[GENERAL]\n").expect("write");
        let real = dir.join("FNX_32X/SimObjects/Airplanes/FNX320");
        std::fs::create_dir_all(&real).expect("mkdir");
        std::fs::write(real.join("aircraft.cfg"), "[GENERAL]\n").expect("write");

        let got = find_aircraft_cfgs(&dir);
        assert_eq!(got.len(), 1, "attachments 底下那个不该被收进来：{got:?}");
        assert!(got[0].starts_with(&real));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 布局不写死：第三方包爱怎么套就怎么套，只认文件名。
    #[test]
    fn the_layout_is_not_hardcoded() {
        let dir = tempdir();
        let deep = dir.join("some/vendor/whatever/plane");
        std::fs::create_dir_all(&deep).expect("mkdir");
        std::fs::write(deep.join("aircraft.cfg"), "[GENERAL]\n").expect("write");
        assert_eq!(find_aircraft_cfgs(&dir).len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn tempdir() -> PathBuf {
        // 按项目规矩：临时文件放仓库自己的 .temp/，不去 /tmp。
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.temp/hangar-tests");
        let dir = base.join(format!(
            "{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }
}
