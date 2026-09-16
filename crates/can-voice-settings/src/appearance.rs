//! 窗口外观：主题、置顶、精简。四个桌面端共用。
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

/// 窗口外观。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Appearance {
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub always_on_top: bool,
    #[serde(default)]
    pub compact: bool,
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
            }
        );
    }
}
