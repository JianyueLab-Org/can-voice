//! 把他机插件装进 X-Plane。
//!
//! 只有这一支需要：MSFS 那边靠 SimConnect 直接建 AI 飞机，一个插件都不用装。
//!
//! # X-Plane 装在哪，只能另找来源
//!
//! UDP 那条链路给不出来——BECN 信标里只有地址和端口，客户端连模拟器装在哪个盘
//! 都不知道。所以两条路：先读 X-Plane 安装器自己写的那份记录
//! （`x-plane_install_12.txt`），读不到就让用户自己填目录。自动探测是省事用的，
//! **自己填那条路必须一直留着**：那份记录的位置各平台不同，用绿色版或者搬过
//! 目录的人根本没有它。
//!
//! # 这里不装 XPPython3
//!
//! 那是编译出来的二进制，版本还跟模拟器绑（X-Plane 12 要 v4.x，11.52 要
//! v3.1.5——v4 是拿 SDK 420 编的，装到 XP11 上是静默不工作）。替用户下载解压
//! 第三方二进制是另一个风险级别。这里只**检测**它在不在，不在就把话说清楚。
//!
//! # 新旧按内容比，不按版本号
//!
//! 插件是一个平铺的源文件，跑在 X-Plane 自己的 Python 里，import 不到任何版本
//! 模块；写个版本常量就得手动维护，迟早忘。内容一样就是最新，不一样就该更新，
//! 顺带把协议号变化也覆盖了。
//!
//! 协议号那件事还要单独报：对不上时插件**静默丢弃每一帧**（`header.get("v")`
//! 不等就 return，不记日志），症状是"他机一架都不出现"而两端日志都干干净净。
//! 这是最难自查的一类故障，所以 [`inspect`] 把装好的那份的协议号也解出来单独回报，
//! 而不是只说一句"版本旧"。

use can_voice_i18n::Message;
use std::path::{Path, PathBuf};

/// 插件源码相对 `src-tauri/` 的位置。
///
/// **只写这一处。** 插件在包里有两份：[`BUNDLED`] 编进二进制，给应用内安装用；
/// `tauri.conf.json` 的 `bundle.resources` 再放一份进安装目录，给装不进去、只能
/// 自己拷的人用。两份各指一个源文件的话迟早一份更新了另一份没有，所以两边都从
/// 这里取，测试钉着后者。写成宏而不是常量，是因为 `include_str!` 只收字面量。
macro_rules! plugin_source {
    () => {
        "../plugin/PI_XpcTraffic.py"
    };
}

/// 随包带的那份插件源码。
///
/// `include_str!` 编进二进制，不从磁盘找：打包之后当前目录是用户双击时所在的
/// 目录，不是程序目录，相对路径取不到——can-audio 那边为此专门处理了
/// PyInstaller 的 `sys._MEIPASS`，而这里一开始就不需要那一步。
pub const BUNDLED: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", plugin_source!()));

pub const PLUGIN_NAME: &str = "PI_XpcTraffic.py";

/// 安装目录里那一份所在的文件夹名，相对 Tauri 的资源目录。
///
/// 和 XPPython3 那个文件夹同名，是为了让人拷的时候形状就是对的；发布页那个
/// zip 里也是这个文件夹。
pub const RESOURCE_DIR: &str = "PythonPlugins";

/// 探测的结果。界面直接照着它画。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum State {
    /// 没找到 X-Plane 目录。
    NoRoot,
    /// 指的那个目录不像 X-Plane。
    NotXplane,
    /// 没装过。
    Missing,
    /// 装过，但和随包这份不一样。
    Outdated,
    /// 已是最新。
    Current,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Status {
    pub root: String,
    pub state: State,
    /// XPPython3 在不在。**不在的话插件装了也不会跑**，而那是另一个包。
    pub xppython3: bool,
    /// 装着的那份声明的协议号。没装、或者读不出来时是 `None`。
    pub installed_protocol: Option<u32>,
    /// 这个客户端说的协议号。
    pub bundled_protocol: u32,
    /// 插件该落在哪。
    pub path: String,
    /// 装着的那份和这个客户端不是同一种话。见模块文档。
    pub protocol_mismatch: bool,
    /// 现在点"安装"有没有意义。
    pub can_install: bool,
}

/// 这个目录看着像不像 X-Plane 装的地方。
///
/// 认 `Resources/plugins`：每个 X-Plane 安装都有它。`PythonPlugins` 不一定在
/// （没装 XPPython3 就没有），拿后者判断会把好目录判成坏的。
pub fn is_xplane_root(root: &Path) -> bool {
    !root.as_os_str().is_empty() && root.join("Resources").join("plugins").is_dir()
}

/// 安装目录里带着的那一份在哪个文件夹。`resource_dir` 是 Tauri 的资源目录。
///
/// 应用内安装写不进去时（X-Plane 装在要管理员权限的地方）界面拿它给人指路：
/// 程序自己提不了权，人可以。文件不在就是 `None`——指着一个空文件夹叫人去拷，
/// 比不指更糟。
pub fn bundled_copy_dir(resource_dir: &Path) -> Option<PathBuf> {
    let dir = resource_dir.join(RESOURCE_DIR);
    dir.join(PLUGIN_NAME).is_file().then_some(dir)
}

/// 插件该落在哪：`Resources/plugins/PythonPlugins/`，XPPython3 只看这里。
///
/// 这里曾经少了 `plugins` 那一层，写到 `Resources/PythonPlugins/`。XPPython3 从来
/// 不去那里找，而 `inspect` 查的是同一个错地方，所以装完界面显示"最新"、天上
/// 一架他机都没有——两边都自洽，没有任何东西会红。
fn plugin_path(root: &Path) -> PathBuf {
    root.join("Resources")
        .join("plugins")
        .join("PythonPlugins")
        .join(PLUGIN_NAME)
}

fn has_xppython3(root: &Path) -> bool {
    root.join("Resources")
        .join("plugins")
        .join("XPPython3")
        .is_dir()
}

/// 从一份插件源码里把 `PROTOCOL_VERSION` 抠出来。
///
/// 只认**顶格**那一行：缩进的是别人的局部变量，不是这个文件声明的协议号。
/// 用读的而不是执行：用户手上那份可能是改过的、甚至是坏的，为了取一个常量去跑它
/// 没有必要。
pub fn protocol_version(source: &str) -> Option<u32> {
    for line in source.lines() {
        let rest = match line.strip_prefix("PROTOCOL_VERSION") {
            Some(r) => r,
            None => continue,
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let digits: String = rest
            .trim_start()
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !digits.is_empty() {
            return digits.parse().ok();
        }
    }
    None
}

/// 安装记录里的目录，按记录里的顺序，去重。
fn roots_from(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let path = line.trim();
        if !path.is_empty() && !out.iter().any(|p| p == path) {
            out.push(path.to_string());
        }
    }
    out
}

/// X-Plane 安装器写的那份记录可能在哪。
///
/// 每行一个安装目录。位置各平台不同，而且是**尽力而为**——界面上永远有"自己填
/// 目录"。12 排在 11 前面：两个都装着的时候，插件更可能是给 12 用的。
fn install_records() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let bases: Vec<PathBuf> = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join("AppData").join("Local")))
            .into_iter()
            .collect()
    } else if cfg!(target_os = "macos") {
        home.iter()
            .map(|h| h.join("Library").join("Preferences"))
            .collect()
    } else {
        home.iter().map(|h| h.join(".x-plane")).collect()
    };
    bases
        .iter()
        .flat_map(|b| {
            ["x-plane_install_12.txt", "x-plane_install_11.txt"]
                .iter()
                .map(|n| b.join(n))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// 自动探测到的 X-Plane 目录，只留还在的。
pub fn find_installs() -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for record in install_records() {
        let Ok(text) = std::fs::read_to_string(&record) else {
            continue;
        };
        for path in roots_from(&text) {
            if !found.contains(&path) && is_xplane_root(Path::new(&path)) {
                found.push(path);
            }
        }
    }
    found
}

/// 看一眼现状。`root` 给 `None` 就自动探测。
pub fn inspect(root: Option<&Path>) -> Status {
    let owned;
    let root = match root {
        Some(r) if !r.as_os_str().is_empty() => r,
        _ => {
            let Some(first) = find_installs().into_iter().next() else {
                return status(Path::new(""), State::NoRoot, false, None, PathBuf::new());
            };
            owned = PathBuf::from(first);
            &owned
        }
    };
    if !is_xplane_root(root) {
        return status(root, State::NotXplane, false, None, PathBuf::new());
    }

    let target = plugin_path(root);
    let xppython3 = has_xppython3(root);
    let Ok(installed) = std::fs::read_to_string(&target) else {
        return status(root, State::Missing, xppython3, None, target);
    };
    let version = protocol_version(&installed);
    let state = if installed == BUNDLED {
        State::Current
    } else {
        State::Outdated
    };
    status(root, state, xppython3, version, target)
}

fn status(
    root: &Path,
    state: State,
    xppython3: bool,
    installed_protocol: Option<u32>,
    path: PathBuf,
) -> Status {
    let bundled_protocol = can_voice_sim::bridge::PROTOCOL_VERSION;
    Status {
        root: root.display().to_string(),
        state,
        xppython3,
        installed_protocol,
        bundled_protocol,
        path: path.display().to_string(),
        protocol_mismatch: matches!(installed_protocol, Some(v) if v != bundled_protocol),
        can_install: matches!(state, State::Missing | State::Outdated | State::Current),
    }
}

/// 把插件写进去，返回落地的路径。
///
/// 错误原样往上抛（X-Plane 装在 `Program Files` 里就是这一类），让界面把话说给
/// 用户听——自己吞掉的话，界面只能说一句"失败了"。交出去的是字典 key 加上路径和
/// 系统给的原话，措辞在前端的字典里。
pub fn install(root: &Path) -> Result<PathBuf, Message> {
    if !is_xplane_root(root) {
        return Err(Message::new("problem.not_xplane"));
    }
    let target = plugin_path(root);
    let dir = target.parent().expect("plugin path has a parent");
    // 装了 XPPython3 也不一定已经有这个目录：它是第一次用时才建的。
    std::fs::create_dir_all(dir).map_err(|e| {
        Message::new("problem.plugin_dir")
            .with("path", dir.display())
            .with("detail", e)
    })?;
    std::fs::write(&target, BUNDLED).map_err(|e| {
        Message::new("problem.plugin_write")
            .with("path", target.display())
            .with("detail", e)
    })?;
    tracing::info!(path = %target.display(), "installed the traffic plugin");
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 临时目录在本 crate 的 `target/` 下：已经被忽略，而且跟着 `cargo clean` 走。
    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/test-install")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    /// 造一个看起来像 X-Plane 的目录。
    fn fake_xplane(name: &str) -> PathBuf {
        let root = scratch(name);
        std::fs::create_dir_all(root.join("Resources").join("plugins")).expect("plugins dir");
        root
    }

    /// **两份 `PROTOCOL_VERSION` 必须是同一个数。**
    ///
    /// 对不上时插件静默丢弃每一帧（它自己那句 `header.get("v") != PROTOCOL_VERSION`
    /// 收到就 return，不记日志），症状是"他机一架都不出现"而两端日志都干干净净。
    /// 随包这份是我们自己发的，所以这件事应该在编译期就被钉住，而不是等用户装完
    /// 才发现。
    #[test]
    fn the_bundled_plugin_speaks_the_version_this_client_speaks() {
        assert_eq!(
            protocol_version(BUNDLED),
            Some(can_voice_sim::bridge::PROTOCOL_VERSION),
        );
    }

    /// **安装目录里那一份和编进二进制的那一份必须是同一个文件。**
    ///
    /// 应用内安装写的是 `BUNDLED`；装不进去时界面叫人去拷的，是 `bundle.resources`
    /// 放进安装目录的那一份。两者一旦指向不同的源文件，手拷的那个人迟早拿到一份
    /// 协议号对不上的旧插件——而那种故障是静默丢帧、两端日志都干净。
    #[test]
    fn the_copy_in_the_install_directory_is_the_file_this_binary_embeds() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let conf: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(manifest.join("tauri.conf.json")).expect("tauri.conf.json"),
        )
        .expect("tauri.conf.json 不是合法的 JSON");
        let resources = conf["bundle"]["resources"]
            .as_object()
            .expect("bundle.resources 要写成「源 → 目标」的映射，不然文件落在 _up_/ 底下");

        let target = format!("{RESOURCE_DIR}/{PLUGIN_NAME}");
        let sources: Vec<&str> = resources
            .iter()
            .filter(|(_, t)| t.as_str() == Some(target.as_str()))
            .map(|(s, _)| s.as_str())
            .collect();
        assert_eq!(
            sources,
            vec![plugin_source!()],
            "{target} 必须恰好来自 include_str! 编进去的那个文件"
        );
        assert_eq!(
            std::fs::read_to_string(manifest.join(plugin_source!())).expect("read"),
            BUNDLED
        );
    }

    /// 资源目录里真有那个文件才给路径：界面不该叫人去一个空文件夹里拷东西。
    #[test]
    fn the_packaged_copy_is_pointed_at_only_when_it_is_there() {
        let dir = scratch("resources");
        assert_eq!(bundled_copy_dir(&dir), None);

        std::fs::create_dir_all(dir.join(RESOURCE_DIR)).expect("dir");
        assert_eq!(bundled_copy_dir(&dir), None, "只有文件夹、没有文件也不算");

        std::fs::write(dir.join(RESOURCE_DIR).join(PLUGIN_NAME), BUNDLED).expect("write");
        assert_eq!(bundled_copy_dir(&dir), Some(dir.join(RESOURCE_DIR)));
    }

    /// 版本号是**读**出来的，不是执行出来的：用户手上那份可能是改过的、甚至是坏的，
    /// 为了取一个常量去跑它没有必要。
    #[test]
    fn the_protocol_version_is_read_out_of_the_source_not_executed() {
        assert_eq!(protocol_version("PROTOCOL_VERSION = 7\n"), Some(7));
        assert_eq!(
            protocol_version("x = 1\nPROTOCOL_VERSION=12\ny = 2\n"),
            Some(12)
        );
        // 缩进的那一行是别人的局部变量，不是这个文件声明的协议号。
        assert_eq!(protocol_version("    PROTOCOL_VERSION = 3\n"), None);
        assert_eq!(protocol_version("nothing here\n"), None);
    }

    /// 认 `Resources/plugins`：每个 X-Plane 安装都有它。
    /// `PythonPlugins` 不一定在（没装 XPPython3 就没有），拿它判会把好目录判成坏的。
    #[test]
    fn an_xplane_root_is_recognised_by_resources_plugins() {
        let root = fake_xplane("is-root");
        assert!(is_xplane_root(&root));
        assert!(!is_xplane_root(&scratch("empty")));
    }

    /// 安装记录一行一个目录，按记录里的顺序来，空行和重复的不算。
    #[test]
    fn a_record_file_yields_its_directories_in_order() {
        let text = "/games/X-Plane 12\n\n/games/X-Plane 11\n/games/X-Plane 12\n";
        assert_eq!(
            roots_from(text),
            vec![
                "/games/X-Plane 12".to_string(),
                "/games/X-Plane 11".to_string()
            ]
        );
    }

    /// 没装过就说没装过，并且要能装。
    #[test]
    fn a_root_without_the_plugin_reports_missing() {
        let root = fake_xplane("missing");
        let s = inspect(Some(&root));
        assert_eq!(s.state, State::Missing);
        assert!(s.can_install);
        assert!(!s.xppython3, "这个假目录里没有 XPPython3");
        assert_eq!(s.installed_protocol, None);
    }

    /// 指的那个目录根本不是 X-Plane 时，要说的是这一句，而不是"没装过"——
    /// 后者会让用户点下"安装"，然后在一个随便什么目录里种下一个文件。
    #[test]
    fn a_directory_that_is_not_xplane_says_so() {
        let root = scratch("not-xplane");
        let s = inspect(Some(&root));
        assert_eq!(s.state, State::NotXplane);
        assert!(!s.can_install);
    }

    /// 往一个不是 X-Plane 的目录里装要拒绝，而且交给界面的是字典 key，不是拼好的一句话。
    #[test]
    fn installing_into_a_directory_that_is_not_xplane_is_refused() {
        let root = scratch("install-not-xplane");
        assert_eq!(install(&root), Err(Message::new("problem.not_xplane")));
        assert!(!plugin_path(&root).exists());
    }

    /// 装完之后那份文件要落在 XPPython3 真正会去看的地方，而且状态变成"最新"。
    #[test]
    fn installing_puts_the_file_where_xppython3_looks() {
        let root = fake_xplane("install");
        let path = install(&root).expect("install");
        assert!(
            path.ends_with("Resources/plugins/PythonPlugins/PI_XpcTraffic.py"),
            "{path:?}"
        );
        assert_eq!(std::fs::read_to_string(&path).expect("read"), BUNDLED);

        let s = inspect(Some(&root));
        assert_eq!(s.state, State::Current);
        assert_eq!(
            s.installed_protocol,
            Some(can_voice_sim::bridge::PROTOCOL_VERSION)
        );
        assert!(!s.protocol_mismatch);
    }

    /// **装着的那份协议号对不上，要单独报一句。**
    ///
    /// 只说"版本旧"是不够的：协议号对不上是唯一一种"装了、看起来正常、而天上
    /// 一架飞机都没有"的故障，而它两端日志都是干净的。
    #[test]
    fn an_older_protocol_version_is_reported_on_its_own() {
        let root = fake_xplane("mismatch");
        let dir = root.join("Resources").join("plugins").join("PythonPlugins");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join(PLUGIN_NAME), "PROTOCOL_VERSION = 1\n# 旧的\n").expect("write");

        let s = inspect(Some(&root));
        assert_eq!(s.state, State::Outdated);
        assert_eq!(s.installed_protocol, Some(1));
        assert!(s.protocol_mismatch, "协议号不一样就是不一样，必须报出来");
    }

    /// 没有目录可看的时候不要假装看过了。
    #[test]
    fn nothing_to_look_at_is_its_own_state() {
        let s = inspect(Some(std::path::Path::new("")));
        assert_eq!(s.state, State::NoRoot);
        assert!(!s.can_install);
    }
}
