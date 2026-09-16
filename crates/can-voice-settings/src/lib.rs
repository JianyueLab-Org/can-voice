//! 设置的落盘。四个桌面端共用。
//!
//! # 为什么它自己是一个 crate
//!
//! 它只要 serde。放在 `can-voice-app` 里的话，`atis-for-can` 为了读一个 JSON
//! 文件就得把 `can-voice-ptt` 拖进来——那意味着 rdev 和 gilrs，也就是 Linux 上的
//! libx11 与 libudev。而通播制作端根本没有 PTT。
//!
//! # 读不出来不是错误
//!
//! 第一次启动没有文件；一份被写坏的文件如果让程序起不来，用户就只剩重装这一条
//! 路。两种情况都回到默认值，并在日志里留一行。
//!
//! # 写是先写临时文件再改名
//!
//! 直接写目标文件的话，一次崩溃或断电会留下半份 JSON，而下次启动读到的是
//! "设置全没了"——那正是这个模块存在的理由的反面。

pub mod appearance;
pub mod endpoints;

pub use appearance::{Appearance, Language, Theme};
pub use endpoints::Endpoints;

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// 一份存在磁盘上的设置。
///
/// 存什么由各个客户端自己定（它们要存的东西不一样），这里只管**存在哪、
/// 怎么存、存坏了怎么办**。
pub struct Store {
    path: PathBuf,
}

impl Store {
    /// 某个产品自己那一份。`product` 是四个产品名之一（`audio-for-can` 等）。
    pub fn for_product(product: &str) -> Self {
        Self {
            path: config_dir(product).join("settings.json"),
        }
    }

    /// 指定路径。测试用。
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 读一份设置。读不出来就是默认值。
    pub fn load<T: DeserializeOwned + Default>(&self) -> T {
        let raw = match std::fs::read(&self.path) {
            Ok(raw) => raw,
            // 没有文件是第一次启动，连日志都不值得写一行。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return T::default(),
            Err(e) => {
                tracing::warn!(path = %self.path.display(), error = %e, "could not read the settings");
                return T::default();
            }
        };
        match serde_json::from_slice(&raw) {
            Ok(v) => v,
            Err(e) => {
                // 坏文件留在原地不删：用户可能想看看里面是什么，
                // 而下一次 save 会盖掉它。
                tracing::warn!(path = %self.path.display(), error = %e,
                    "the settings file is not valid json; falling back to defaults");
                T::default()
            }
        }
    }

    /// 写一份设置。先写临时文件再改名。
    pub fn save<T: Serialize>(&self, value: &T) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, body)?;
        // rename 在同一个文件系统里是原子的：要么是旧的那一份，要么是新的，
        // 不会是半份。
        std::fs::rename(&tmp, &self.path)
    }
}

/// 设置目录。
fn config_dir(product: &str) -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("HOME").map(|h| {
                let h = PathBuf::from(h);
                if cfg!(target_os = "macos") {
                    h.join("Library").join("Application Support")
                } else {
                    h.join(".config")
                }
            })
        })
        .unwrap_or_default();
    base.join(product)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Example {
        cid: String,
        freqs: Vec<u32>,
    }

    /// 临时目录放在 `target/` 下面，不用系统 temp：它是这个仓库自己的
    /// 构建产物目录，已经被忽略，而且跟着 `cargo clean` 一起走。
    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-settings")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch");
        dir
    }

    /// **存了就要能读回来。** 没有这一条，CAN 号、频率台面、音量和 PTT 绑定
    /// 每次启动归零——管制员每次上线都要重建整个频率台面。
    #[test]
    fn a_saved_value_comes_back() {
        let store = Store::at(scratch("roundtrip").join("settings.json"));
        let want = Example {
            cid: "1001".into(),
            freqs: vec![118_000, 121_800],
        };

        store.save(&want).expect("save");

        assert_eq!(store.load::<Example>(), want);
    }

    /// 第一次启动没有文件。那不是错误，是默认值。
    #[test]
    fn a_missing_file_is_the_default_not_an_error() {
        let store = Store::at(scratch("missing").join("settings.json"));
        assert_eq!(store.load::<Example>(), Example::default());
    }

    /// **一份写坏的文件不能让程序起不来。** 那会把用户逼到重装，
    /// 而他丢掉的正是这个模块要保住的东西。
    #[test]
    fn a_corrupt_file_is_the_default_not_a_brick() {
        let path = scratch("corrupt").join("settings.json");
        std::fs::write(&path, b"{ this is not json").expect("write");
        let store = Store::at(&path);
        assert_eq!(store.load::<Example>(), Example::default());
    }

    /// 写完不留临时文件——留下来的话，下一次 `save` 会以为上一次崩在半路。
    #[test]
    fn no_temporary_file_is_left_behind() {
        let dir = scratch("tidy");
        let store = Store::at(dir.join("settings.json"));
        store.save(&Example::default()).expect("save");

        let left: Vec<_> = std::fs::read_dir(&dir)
            .expect("read_dir")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n != "settings.json")
            .collect();
        assert!(left.is_empty(), "留下了 {left:?}");
    }
}
