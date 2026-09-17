//! 窗口外观：主题、置顶、精简、界面语言。四个桌面端共用。
//!
//! # 置顶和精简不是装饰
//!
//! 管制员把语音压在雷达屏上用，飞行员把它压在模拟器上用。被别的窗口盖住就等于
//! 看不见谁在呼叫；窗口大到盖住半个雷达，就只能把它拖到副屏上去——而副屏上的
//! 东西值班时没人看。旧版（`can-audio/controller/gui.py`）两个都是顶栏上的
//! 常驻开关，不是藏在设置对话框里的选项：它们是要频繁切的。

use serde::{Deserialize, Deserializer, Serialize};

/// 主题。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// 跟着系统走。默认。
    #[default]
    System,
    Light,
    Dark,
}

impl<'de> Deserialize<'de> for Theme {
    /// 手写而不是 derive：derive 出来的对认不出的值报错，而那会让整份设置
    /// 回默认（见 `a_theme_this_version_has_never_heard_of_is_the_system_one`）。
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(match value.as_str() {
            Some("light") => Theme::Light,
            Some("dark") => Theme::Dark,
            _ => Theme::System,
        })
    }
}

/// 界面语言（#29）。
///
/// **只管界面。** 通播播报用什么语言是通播自己的设置，和这一项无关——切到英文
/// 界面的管制员照样要播中文通播。
///
/// Rust 侧不翻译任何东西，这一项只是存着：措辞在前端的字典里，"跟随系统"也由
/// 前端按 webview 报的系统语言去解。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// 跟着系统走：中文系统是中文，英文系统是英文，别的系统是中文。默认。
    #[default]
    System,
    Zh,
    En,
}

impl<'de> Deserialize<'de> for Language {
    /// 手写，理由和 [`Theme`] 同一条：认不出的值不能让整份设置回默认。
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(match value.as_str() {
            Some("zh") => Language::Zh,
            Some("en") => Language::En,
            _ => Language::System,
        })
    }
}

/// 窗口外观。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Appearance {
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub always_on_top: bool,
    #[serde(default)]
    pub compact: bool,
    #[serde(default)]
    pub language: Language,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_theme_is_spelled_in_lowercase_on_disk() {
        assert_eq!(serde_json::to_string(&Theme::Dark).unwrap(), "\"dark\"");
        assert_eq!(
            serde_json::from_str::<Theme>("\"light\"").unwrap(),
            Theme::Light
        );
        assert_eq!(
            serde_json::from_str::<Theme>("\"system\"").unwrap(),
            Theme::System
        );
    }

    /// **一个认不出来的主题名不能让整份设置读不进来。**
    ///
    /// serde 对认不出的枚举值是报错，而 [`crate::Store::load`] 对解析失败的
    /// 处理是整份回默认——于是新版写进去一个 `"sepia"`、用户退回旧版，丢掉的
    /// 是 CAN 号、频率台面和 PTT 绑定。
    #[test]
    fn a_theme_this_version_has_never_heard_of_is_the_system_one() {
        assert_eq!(
            serde_json::from_str::<Theme>("\"sepia\"").unwrap(),
            Theme::System
        );
        assert_eq!(serde_json::from_str::<Theme>("42").unwrap(), Theme::System);
    }

    #[test]
    fn an_old_settings_file_with_no_appearance_at_all_reads_as_the_default() {
        let got: Appearance = serde_json::from_str("{}").unwrap();
        assert_eq!(got, Appearance::default());
        assert_eq!(got.theme, Theme::System);
        assert!(!got.always_on_top);
        assert!(!got.compact);
        assert_eq!(got.language, Language::System);
    }

    #[test]
    fn each_language_is_spelled_in_lowercase_on_disk() {
        assert_eq!(serde_json::to_string(&Language::En).unwrap(), "\"en\"");
        assert_eq!(
            serde_json::from_str::<Language>("\"zh\"").unwrap(),
            Language::Zh
        );
        assert_eq!(
            serde_json::from_str::<Language>("\"system\"").unwrap(),
            Language::System
        );
    }

    /// 新版加了一种语言、用户退回旧版：丢掉的只能是语言这一项，不能是整份设置。
    #[test]
    fn a_language_this_version_has_never_heard_of_is_the_system_one() {
        let got: Appearance = serde_json::from_str(r#"{"language":"ja","compact":true}"#).unwrap();
        assert_eq!(got.language, Language::System);
        assert!(got.compact);
    }

    #[test]
    fn a_bad_theme_does_not_take_the_other_switches_down_with_it() {
        let got: Appearance =
            serde_json::from_str(r#"{"theme":"sepia","always_on_top":true,"compact":true}"#)
                .unwrap();
        assert_eq!(
            got,
            Appearance {
                theme: Theme::System,
                always_on_top: true,
                compact: true,
                language: Language::System,
            }
        );
    }
}
