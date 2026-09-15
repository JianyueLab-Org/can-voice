//! CSL 模型匹配。
//!
//! CSL 是 X-Plane 多人机模型的通用格式（Bluebell、X-CSL 等），xPilot、
//! LiveTraffic、swift 都用它，所以这里跟着它的约定走，用户装哪个包都能认。
//!
//! 一个包的目录里有 `xsb_aircraft.txt`：
//!
//! ```text
//! EXPORT_NAME BB_Airbus
//! OBJ8_AIRCRAFT A320_CCA
//! OBJ8 SOLID YES A320/A320_CCA.obj
//! ICAO A320
//! AIRLINE A320 CCA
//! ```
//!
//! 匹配就是拿 FSD 问来的机型和航司去这张表里找，找不到一级级往下退：
//!
//! ```text
//! 1. 机型 + 航司     波音 738 的国航涂装
//! 2. 机型            波音 738，随便什么涂装
//! 3. 同族近似机型    B739 顶替 B738
//! 4. 同类机身        宽体顶宽体、支线顶支线
//! 5. 同类别通用      按前缀猜一个
//! 6. 兜底            包里的第一个模型
//! ```
//!
//! **退化必须一直有结果。** 宁可画一架涂装不对的飞机，也不能因为匹配不到就让
//! 天上空着——飞行员看不见的飞机比看错涂装的飞机危险得多。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 同族机型。匹配不到精确型号时按这里找替身，都是外形接近的。
/// 不求全，只覆盖网络上常见的；查不到就落到按类别的通用匹配。
pub const FAMILIES: [&[&str]; 17] = [
    &["A318", "A319", "A320", "A321", "A19N", "A20N", "A21N"],
    &["A332", "A333", "A338", "A339"],
    &["A343", "A345", "A346"],
    &["A359", "A35K"],
    &["A388"],
    &[
        "B731", "B732", "B733", "B734", "B735", "B736", "B737", "B738", "B739", "B37M", "B38M",
        "B39M",
    ],
    &["B741", "B742", "B743", "B744", "B748"],
    &["B752", "B753"],
    &["B762", "B763", "B764"],
    &["B772", "B773", "B77L", "B77W"],
    &["B788", "B789", "B78X"],
    &["E170", "E75L", "E75S", "E190", "E195"],
    &["CRJ2", "CRJ7", "CRJ9", "CRJX"],
    &["C919", "AR21"],
    &["MD82", "MD83", "MD88", "MD90"],
    &["C172", "C182", "C152", "P28A", "SR22"],
    &["AT45", "AT72", "AT76"],
];

/// 机身类别。同族找不到时按这个找替身——**拿一架 A319 去顶 B777 视觉上差得
/// 离谱**，而宽体顶宽体、支线顶支线至少大小对得上。
pub const CATEGORIES: [(&str, &[&str]); 4] = [
    (
        "宽体",
        &[
            "B77W", "B77L", "B772", "B773", "B788", "B789", "B78X", "A332", "A333", "A338", "A339",
            "A359", "A35K", "B742", "B743", "B744", "B748", "A388", "B762", "B763", "B764", "MD11",
            "A306", "A310",
        ],
    ),
    (
        "窄体",
        &[
            "A319", "A320", "A321", "A318", "A19N", "A20N", "A21N", "B737", "B738", "B739", "B736",
            "B735", "B734", "B733", "B37M", "B38M", "B39M", "B752", "B753", "C919", "MD82", "MD83",
            "MD88", "MD90", "B712",
        ],
    ),
    (
        "支线",
        &[
            "E170", "E75L", "E75S", "E190", "E195", "E145", "E135", "CRJ2", "CRJ7", "CRJ9", "CRJX",
            "AR21", "AT45", "AT72", "AT76", "DH8A", "DH8B", "DH8C", "DH8D", "SF34", "J328",
        ],
    ),
    (
        "通航",
        &[
            "C172", "C182", "C152", "C208", "C25C", "C700", "P28A", "SR22", "S22T", "DA40", "DA62",
            "BE36", "BE58", "B350", "TBM9", "PC12", "PC6", "DHC2", "DV20", "DR40", "MXS", "VL3",
            "A5",
        ],
    ),
];

/// 机型码猜类别，用于最后一级通用匹配。
const GENERIC_BY_PREFIX: [(&str, &str); 13] = [
    ("A3", "A320"),
    ("A2", "A320"),
    ("B7", "B738"),
    ("B3", "B738"),
    ("E1", "E190"),
    ("E7", "E190"),
    ("CRJ", "CRJ7"),
    ("MD", "MD82"),
    ("DH", "DH8D"),
    ("AT", "AT76"),
    ("C1", "C172"),
    ("P2", "C172"),
    ("SR", "C172"),
];

pub const DEFAULT_TYPE: &str = "B738";

/// CSL 包里的一个模型。
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub struct Model {
    pub name: String,
    pub path: PathBuf,
    pub package: String,
    pub icao: String,
    pub airline: String,
    pub livery: String,
}

/// 机型所属的同族，不在表里就是空。
pub fn family_of(icao: &str) -> &'static [&'static str] {
    let icao = icao.to_uppercase();
    FAMILIES
        .iter()
        .find(|f| f.contains(&icao.as_str()))
        .copied()
        .unwrap_or(&[])
}

/// 机型属于哪一类机身。认不出返回空。
pub fn category_of(icao: &str) -> &'static str {
    let icao = icao.to_uppercase();
    CATEGORIES
        .iter()
        .find(|(_, types)| types.contains(&icao.as_str()))
        .map(|(name, _)| *name)
        .unwrap_or("")
}

/// 猜一个同类别的通用机型码。
pub fn generic_for(icao: &str) -> &'static str {
    let icao = icao.to_uppercase();
    GENERIC_BY_PREFIX
        .iter()
        .find(|(prefix, _)| icao.starts_with(prefix))
        .map(|(_, generic)| *generic)
        .unwrap_or(DEFAULT_TYPE)
}

/// 匹配到第几级。写进日志，用户报"我看到的飞机长得不对"时能直接看出来。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum MatchLevel {
    /// 对方直接给了 CSL 名字。
    ByName,
    TypeAndAirline,
    /// 机型对，涂装不对。
    TypeOnly,
    /// 用同族的顶替。
    Family {
        used: String,
        airline_kept: bool,
    },
    /// 同类机身顶替。
    Category {
        category: String,
        used: String,
    },
    Generic {
        used: String,
    },
    /// 没有近似机型，用了包里的第一个。
    Fallback,
}

/// 所有装好的 CSL 模型，以及匹配逻辑。
#[derive(Debug, Default)]
pub struct ModelSet {
    models: Vec<Model>,
    by_icao: HashMap<String, Vec<usize>>,
    by_icao_airline: HashMap<(String, String), Vec<usize>>,
}

impl ModelSet {
    pub fn new(models: Vec<Model>) -> Self {
        let mut set = Self {
            models,
            ..Default::default()
        };
        for (i, m) in set.models.iter().enumerate() {
            set.by_icao.entry(m.icao.clone()).or_default().push(i);
            if !m.airline.is_empty() {
                set.by_icao_airline
                    .entry((m.icao.clone(), m.airline.clone()))
                    .or_default()
                    .push(i);
            }
        }
        set
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    fn first_with_icao(&self, icao: &str) -> Option<&Model> {
        self.by_icao.get(icao)?.first().map(|i| &self.models[*i])
    }

    fn first_with_both(&self, icao: &str, airline: &str) -> Option<&Model> {
        self.by_icao_airline
            .get(&(icao.to_string(), airline.to_string()))?
            .first()
            .map(|i| &self.models[*i])
    }

    /// 对方直接指定了 CSL 名字时按名字找。
    pub fn by_name(&self, name: &str) -> Option<&Model> {
        if name.is_empty() {
            return None;
        }
        let name = name.to_uppercase();
        self.models.iter().find(|m| m.name.to_uppercase() == name)
    }

    /// 挑一个模型。装了模型就**一定**有结果。
    pub fn match_model(
        &self,
        equipment: &str,
        airline: &str,
        csl: &str,
    ) -> Option<(&Model, MatchLevel)> {
        if self.models.is_empty() {
            return None;
        }
        if let Some(m) = self.by_name(csl) {
            return Some((m, MatchLevel::ByName));
        }
        let equipment = equipment.to_uppercase();
        let airline = airline.to_uppercase();

        if !equipment.is_empty() && !airline.is_empty() {
            if let Some(m) = self.first_with_both(&equipment, &airline) {
                return Some((m, MatchLevel::TypeAndAirline));
            }
        }
        if !equipment.is_empty() {
            if let Some(m) = self.first_with_icao(&equipment) {
                return Some((m, MatchLevel::TypeOnly));
            }
        }

        // 3. 同族近似机型；优先仍带正确航司的。
        for relative in family_of(&equipment) {
            if *relative == equipment {
                continue;
            }
            if !airline.is_empty() {
                if let Some(m) = self.first_with_both(relative, &airline) {
                    return Some((
                        m,
                        MatchLevel::Family {
                            used: (*relative).to_string(),
                            airline_kept: true,
                        },
                    ));
                }
            }
            if let Some(m) = self.first_with_icao(relative) {
                return Some((
                    m,
                    MatchLevel::Family {
                        used: (*relative).to_string(),
                        airline_kept: false,
                    },
                ));
            }
        }

        // 4. 同类机身。**这一级必须排在「通用机型」前面。**
        //
        // GENERIC_BY_PREFIX 是按两位前缀猜的，而 `A3` / `B7` 这样的前缀同时盖住
        // 窄体和宽体：B77W 会被猜成 B738、A359 会被猜成 A320。把通用那级放前面
        // 的话，只要装了 B738 或 A320（最普及的两个模型），**所有宽体都会退成
        // 窄体**，这一级永远轮不到——一架 777 在别人屏幕上变成 737，正是它本来
        // 要挡的那种情况。
        let category = category_of(&equipment);
        if !category.is_empty() {
            let types = CATEGORIES
                .iter()
                .find(|(n, _)| *n == category)
                .map(|(_, t)| *t)
                .unwrap_or(&[]);
            for candidate in types {
                if let Some(m) = self.first_with_icao(candidate) {
                    return Some((
                        m,
                        MatchLevel::Category {
                            category: category.to_string(),
                            used: (*candidate).to_string(),
                        },
                    ));
                }
            }
        }

        // 5. 同类别通用。走到这里说明机型码不在 CATEGORIES 里（新机型、打错的
        //    代码），只能按前缀猜。猜出来的通用机型本身也可能没装（比如猜出
        //    A320 但包里只有 A20N），所以这一级同样要走一遍同族。
        let generic = generic_for(&equipment);
        for candidate in std::iter::once(&generic).chain(family_of(generic).iter()) {
            if let Some(m) = self.first_with_icao(candidate) {
                return Some((
                    m,
                    MatchLevel::Generic {
                        used: (*candidate).to_string(),
                    },
                ));
            }
        }

        // 6. 兜底。**看不见的飞机比涂装错的飞机危险得多。**
        Some((&self.models[0], MatchLevel::Fallback))
    }
}

/// 读一个 CSL 包的 `xsb_aircraft.txt`。
///
/// 这个格式有几十年的历史，各家包写得并不一致：路径分隔符可能是 `/` 也可能是
/// `\`，`OBJ8` 行的字段数不固定，注释用 `#`。**宽松地读，认不出的行跳过**
/// ——一个包里一行有问题不该让整包用不了。
pub fn parse_manifest(directory: &Path, text: &str) -> Vec<Model> {
    let mut models: Vec<Model> = Vec::new();
    let mut package = String::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        let keyword = parts[0].to_uppercase();
        match keyword.as_str() {
            "EXPORT_NAME" if parts.len() > 1 => package = parts[1].to_string(),
            "OBJ8_AIRCRAFT" | "AIRCRAFT" if parts.len() > 1 => models.push(Model {
                name: parts[1].to_string(),
                package: package.clone(),
                ..Default::default()
            }),
            "OBJ8" if parts.len() >= 4 => {
                if let Some(current) = models.last_mut() {
                    // `OBJ8 <类型> <是否有动画> <路径>`；只要 SOLID 的那条。
                    if parts[1].eq_ignore_ascii_case("SOLID") && current.path.as_os_str().is_empty()
                    {
                        let relative = parts[3..].join(" ").replace('\\', "/");
                        current.path = directory.join(relative);
                    }
                }
            }
            "ICAO" if parts.len() > 1 => {
                if let Some(current) = models.last_mut() {
                    current.icao = parts[1].to_uppercase();
                }
            }
            "AIRLINE" if parts.len() > 2 => {
                if let Some(current) = models.last_mut() {
                    if current.icao.is_empty() {
                        current.icao = parts[1].to_uppercase();
                    }
                    current.airline = parts[2].to_uppercase();
                }
            }
            "LIVERY" if parts.len() > 3 => {
                if let Some(current) = models.last_mut() {
                    if current.icao.is_empty() {
                        current.icao = parts[1].to_uppercase();
                    }
                    if current.airline.is_empty() {
                        current.airline = parts[2].to_uppercase();
                    }
                    current.livery = parts[3].to_uppercase();
                }
            }
            _ => {}
        }
    }
    models.retain(|m| !m.path.as_os_str().is_empty() && !m.icao.is_empty());
    models
}

/// 在一个目录树里找所有 CSL 包（含 `xsb_aircraft.txt` 的目录）。
///
/// **必须跟着符号链接走。** CSL 包动辄几个 GB，"放在另一块盘、在
/// `Resources/plugins` 下留个链接"是 X-Plane 这边最常见的安置方式——不跟，
/// 整套 CSL 一个包都扫不到，而现象只是他机不显示。Windows 的目录联接
/// （junction）也算链接。
///
/// 代价是要自己防环：跟着链接走可能绕回上层目录。按规范化路径记账，进过的
/// 目录不再进。
pub fn find_packages(root: &Path) -> Vec<PathBuf> {
    let mut packages = Vec::new();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut queue = vec![root.to_path_buf()];
    while let Some(dir) = queue.pop() {
        let real = std::fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
        if !seen.insert(real) {
            continue; // 绕回来了
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut children = Vec::new();
        let mut is_package = false;
        for entry in entries.flatten() {
            let path = entry.path();
            // `file_type()` 不跟链接走，`metadata()` 跟——这里要跟。
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.is_dir() {
                children.push(path);
            } else if path.file_name().is_some_and(|n| n == "xsb_aircraft.txt") {
                is_package = true;
            }
        }
        if is_package {
            // 包里面不会再套包，这一枝不用往下走。
            packages.push(dir);
        } else {
            queue.extend(children);
        }
    }
    packages.sort();
    packages
}

/// 把一个目录下所有 CSL 包读进来。
pub fn load(root: &Path) -> ModelSet {
    let mut models = Vec::new();
    for package in find_packages(root) {
        let manifest = package.join("xsb_aircraft.txt");
        match std::fs::read_to_string(&manifest) {
            Ok(text) => models.extend(parse_manifest(&package, &text)),
            Err(e) => tracing::warn!(path = %manifest.display(), error = %e, "cannot read"),
        }
    }
    tracing::info!(models = models.len(), root = %root.display(), "loaded CSL models");
    ModelSet::new(models)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(name: &str, icao: &str, airline: &str) -> Model {
        Model {
            name: name.into(),
            path: PathBuf::from(format!("{name}.obj")),
            package: "pkg".into(),
            icao: icao.into(),
            airline: airline.into(),
            livery: String::new(),
        }
    }

    #[test]
    fn an_exact_type_and_airline_wins() {
        let set = ModelSet::new(vec![
            model("A320_GENERIC", "A320", ""),
            model("A320_CCA", "A320", "CCA"),
        ]);
        let (m, level) = set.match_model("A320", "CCA", "").expect("match");
        assert_eq!(m.name, "A320_CCA");
        assert_eq!(level, MatchLevel::TypeAndAirline);
    }

    #[test]
    fn the_type_alone_is_next() {
        let set = ModelSet::new(vec![model("A320_GENERIC", "A320", "")]);
        let (m, level) = set.match_model("A320", "CCA", "").expect("match");
        assert_eq!(m.name, "A320_GENERIC");
        assert_eq!(level, MatchLevel::TypeOnly);
    }

    /// 同族顶替时**优先仍带正确航司的**。
    #[test]
    fn a_relative_with_the_right_airline_beats_a_relative_without() {
        let set = ModelSet::new(vec![
            model("A321_ANY", "A321", ""),
            model("A319_CCA", "A319", "CCA"),
        ]);
        let (m, level) = set.match_model("A320", "CCA", "").expect("match");
        assert_eq!(m.name, "A319_CCA");
        assert_eq!(
            level,
            MatchLevel::Family {
                used: "A319".into(),
                airline_kept: true
            }
        );
    }

    /// **同类机身必须排在按前缀猜的通用那一级前面。**
    ///
    /// `B7` 这个前缀同时盖住窄体和宽体，B77W 会被猜成 B738。顺序反了的话，
    /// 只要装了 B738（最普及的模型之一），**所有宽体都会退成窄体**——
    /// 一架 777 在别人屏幕上变成 737。
    #[test]
    fn a_widebody_does_not_degrade_into_a_narrowbody() {
        let set = ModelSet::new(vec![
            model("B738_ANY", "B738", ""),
            model("A333_ANY", "A333", ""),
        ]);
        let (m, level) = set.match_model("B77W", "CCA", "").expect("match");
        assert_eq!(m.name, "A333_ANY", "777 应当退到另一架宽体，而不是 737");
        assert!(
            matches!(level, MatchLevel::Category { ref category, .. } if category == "宽体"),
            "{level:?}"
        );
    }

    /// A359 同理：`A3` 前缀会把它猜成 A320。
    #[test]
    fn an_a350_does_not_degrade_into_an_a320() {
        let set = ModelSet::new(vec![
            model("A320_ANY", "A320", ""),
            model("B788_ANY", "B788", ""),
        ]);
        let (m, _) = set.match_model("A359", "", "").expect("match");
        assert_eq!(m.name, "B788_ANY");
    }

    /// 机型码不在任何表里（新机型、打错的代码）时才按前缀猜。
    #[test]
    fn an_unknown_type_falls_back_to_a_guess() {
        let set = ModelSet::new(vec![model("B738_ANY", "B738", "")]);
        let (m, level) = set.match_model("B7XX", "", "").expect("match");
        assert_eq!(m.name, "B738_ANY");
        assert_eq!(
            level,
            MatchLevel::Generic {
                used: "B738".into()
            }
        );
    }

    /// 猜出来的通用机型本身也可能没装，所以这一级要再走一遍同族。
    #[test]
    fn the_guess_itself_falls_through_its_own_family() {
        let set = ModelSet::new(vec![model("A20N_ANY", "A20N", "")]);
        let (m, level) = set.match_model("A2XX", "", "").expect("match");
        assert_eq!(m.name, "A20N_ANY");
        assert_eq!(
            level,
            MatchLevel::Generic {
                used: "A20N".into()
            }
        );
    }

    /// **退化必须一直有结果。** 看不见的飞机比涂装错的飞机危险得多。
    #[test]
    fn something_is_always_drawn_when_any_model_is_installed() {
        let set = ModelSet::new(vec![model("ONLY_ONE", "C172", "")]);
        let (m, level) = set.match_model("B77W", "CCA", "").expect("match");
        assert_eq!(m.name, "ONLY_ONE");
        assert_eq!(level, MatchLevel::Fallback);
    }

    /// 一个模型都没装才返回 None——那是"去装 CSL"，不是匹配失败。
    #[test]
    fn no_models_installed_is_the_only_no_result() {
        assert!(ModelSet::new(Vec::new())
            .match_model("A320", "CCA", "")
            .is_none());
    }

    #[test]
    fn an_explicit_csl_name_short_circuits_everything() {
        let set = ModelSet::new(vec![
            model("A320_CCA", "A320", "CCA"),
            model("B738_ANY", "B738", ""),
        ]);
        let (m, level) = set.match_model("B738", "", "a320_cca").expect("match");
        assert_eq!(m.name, "A320_CCA");
        assert_eq!(level, MatchLevel::ByName);
    }

    /// 宽松地读：注释、反斜杠路径、乱序字段、认不出的行都不该让整包报废。
    #[test]
    fn a_messy_manifest_still_yields_its_models() {
        let text = "\
EXPORT_NAME BB_Airbus
# 一行注释
OBJ8_AIRCRAFT A320_CCA
OBJ8 SOLID YES A320\\A320_CCA.obj
ICAO A320
AIRLINE A320 CCA

SOMETHING_WE_DO_NOT_KNOW 1 2 3
OBJ8_AIRCRAFT B738_NO_PATH
ICAO B738
OBJ8_AIRCRAFT B739_ANY
OBJ8 SOLID YES B739/x.obj
LIVERY B739 CSN WINGLETS
";
        let models = parse_manifest(Path::new("/csl/BB"), text);
        assert_eq!(models.len(), 2, "没路径的那个要被丢掉：{models:?}");
        assert_eq!(models[0].name, "A320_CCA");
        assert_eq!(models[0].icao, "A320");
        assert_eq!(models[0].airline, "CCA");
        assert_eq!(models[0].package, "BB_Airbus");
        // 反斜杠换成正斜杠，再拼到包目录下。
        assert_eq!(models[0].path, Path::new("/csl/BB/A320/A320_CCA.obj"));
        // LIVERY 那一行同时补上机型和航司。
        assert_eq!(models[1].icao, "B739");
        assert_eq!(models[1].airline, "CSN");
        assert_eq!(models[1].livery, "WINGLETS");
    }

    #[test]
    fn only_the_solid_object_becomes_the_path() {
        let text = "\
OBJ8_AIRCRAFT X
OBJ8 GLASS YES glass.obj
OBJ8 SOLID YES solid.obj
ICAO A320
";
        let models = parse_manifest(Path::new("/csl"), text);
        assert_eq!(models[0].path, Path::new("/csl/solid.obj"));
    }

    #[test]
    fn the_family_and_category_tables_agree_with_themselves() {
        assert!(family_of("B738").contains(&"B739"));
        assert!(family_of("ZZZZ").is_empty());
        assert_eq!(category_of("B77W"), "宽体");
        assert_eq!(category_of("A320"), "窄体");
        assert_eq!(category_of("ZZZZ"), "");
        assert_eq!(generic_for("B77W"), "B738");
        assert_eq!(generic_for("ZZZZ"), DEFAULT_TYPE);
    }
}

#[cfg(test)]
mod loading_tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("can-voice-csl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("mkdir");
        p
    }

    fn package(root: &Path, name: &str, icao: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join("xsb_aircraft.txt"),
            format!("EXPORT_NAME {name}\nOBJ8_AIRCRAFT {icao}_X\nOBJ8 SOLID YES {icao}.obj\nICAO {icao}\n"),
        )
        .expect("write");
    }

    #[test]
    fn packages_are_found_anywhere_under_the_root() {
        let root = temp_dir("nested");
        package(&root, "BB_Airbus", "A320");
        package(&root.join("deep").join("deeper"), "BB_Boeing", "B738");
        let found = find_packages(&root);
        assert_eq!(found.len(), 2, "{found:?}");

        let set = load(&root);
        assert_eq!(set.len(), 2);
        let (m, _) = set.match_model("A320", "", "").expect("match");
        assert_eq!(m.icao, "A320");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 包里面不会再套包，扫到一个就不往下走——一个 CSL 包里有几千个子目录。
    #[test]
    fn a_package_is_not_descended_into() {
        let root = temp_dir("nodescend");
        package(&root, "BB", "A320");
        package(&root.join("BB").join("inner"), "INNER", "B738");
        let found = find_packages(&root);
        assert_eq!(found.len(), 1, "{found:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **必须跟着符号链接走。** CSL 包动辄几个 GB，"放在另一块盘、留个链接"
    /// 是最常见的安置方式——不跟，整套 CSL 一个包都扫不到，而现象只是他机
    /// 不显示。
    #[cfg(unix)]
    #[test]
    fn a_symlinked_package_is_found() {
        let root = temp_dir("symlink");
        let elsewhere = temp_dir("symlink-target");
        package(&elsewhere, "BB_Far", "B77W");
        std::os::unix::fs::symlink(elsewhere.join("BB_Far"), root.join("BB_Far")).expect("symlink");
        assert_eq!(find_packages(&root).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&elsewhere);
    }

    /// 跟着链接走可能绕回上层目录。绕回来要停，不能无限转。
    #[cfg(unix)]
    #[test]
    fn a_loop_does_not_hang_the_scan() {
        let root = temp_dir("loop");
        package(&root, "BB", "A320");
        std::os::unix::fs::symlink(&root, root.join("back")).expect("symlink");
        assert_eq!(find_packages(&root).len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_root_is_empty_not_a_panic() {
        assert!(find_packages(Path::new("/definitely/not/here")).is_empty());
        assert!(load(Path::new("/definitely/not/here")).is_empty());
    }
}
