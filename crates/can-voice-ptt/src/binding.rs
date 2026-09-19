//! 一个 PTT 绑定，以及它在设置文件里的样子。
//!
//! # 这个 crate 不产生任何界面文字
//!
//! [`Binding::token`] 返回的是 `"V"` / `"X1"` / `"3"` 这样的**短标识**，措辞由上层的
//! i18n 决定。共用文件里带中文会让同一句话出现在四个地方，而翻译时会漏掉两个——
//! can-audio 为此写了一条扫 AST 的测试，这里的对应物是 `no_ui_strings`。

use serde::{Deserialize, Serialize};

/// 本平台能不能用鼠标侧键做 PTT。
///
/// **macOS 上不能。** rdev 0.5.3 的 `macos/common.rs` 只处理 `LeftMouseDown/Up` 与
/// `RightMouseDown/Up`，`OtherMouseDown`/`OtherMouseUp` 一处都没有——连中键都报不出来，
/// `simulate.rs` 里还留着一句 `// ignored because we don't use OtherMouse EventType`。
///
/// 界面要靠这个常量把话说在前面。**一个绑好了、显示正常、却从来不响的 PTT，
/// 正是这个项目反复要躲开的那类故障。**
pub fn mouse_supported() -> bool {
    !cfg!(target_os = "macos")
}

/// 本平台能不能全局监听键盘。
///
/// **Wayland 下不能。** 它不允许一个普通程序监听全局按键，而 rdev 走的是 Xlib。
/// 后果和 macOS 上的鼠标侧键一模一样：绑好了、界面显示正常、按下去从来不响。
/// README 和 release notes 都写了这条限制，程序里一直没写——而用户读的是程序。
pub fn keyboard_supported() -> bool {
    keyboard_supported_on(
        cfg!(target_os = "linux"),
        std::env::var("WAYLAND_DISPLAY").ok().as_deref(),
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
    )
}

/// [`keyboard_supported`] 的纯函数部分。
///
/// 两个变量都要看：只看 `XDG_SESSION_TYPE` 的话，没设它的合成器漏网；
/// 只看 `WAYLAND_DISPLAY` 的话，一个从 X11 会话里启动的 Wayland 应用会误判。
fn keyboard_supported_on(
    linux: bool,
    wayland_display: Option<&str>,
    session_type: Option<&str>,
) -> bool {
    if !linux {
        return true;
    }
    let wayland = wayland_display.is_some_and(|v| !v.is_empty())
        || session_type.is_some_and(|v| v.eq_ignore_ascii_case("wayland"));
    !wayland
}

/// 可以绑定的鼠标按钮。**只有侧键。**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    X1,
    X2,
}

impl MouseButton {
    /// 把 rdev 给的按钮规范化。不是侧键、或者认不出来的编号，返回 `None`。
    ///
    /// rdev 只给 `Button::Unknown(u8)`，而那个数**每个平台不一样**：
    /// Windows 走 `WM_XBUTTONDOWN` 取 `HIWORD(mouseData)`（XBUTTON1=1、XBUTTON2=2），
    /// X11 直接给按钮号（4–7 是滚轮，rdev 已滤掉）。存进设置的必须是规范化之后的
    /// X1/X2，否则同一份设置换个系统就静默失灵。
    ///
    /// 认不出来的编号返回 `None` 而不是猜一个：猜错会让另一个键变成 PTT。
    pub fn from_rdev(b: rdev::Button) -> Option<Self> {
        match b {
            // 左/右/中键一律不可绑：绑左键意味着在任何窗口里点任何东西都会发话，
            // 而 TX 指示灯还被挡在他刚点的那个东西后面。
            rdev::Button::Left | rdev::Button::Right | rdev::Button::Middle => None,
            rdev::Button::Unknown(n) => match (cfg!(target_os = "windows"), n) {
                (true, 1) => Some(Self::X1),
                (true, 2) => Some(Self::X2),
                (false, 8) => Some(Self::X1),
                (false, 9) => Some(Self::X2),
                _ => None,
            },
        }
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::X1 => "X1",
            Self::X2 => "X2",
        }
    }
}

/// 当前平台的短名，用来给认不出名字的键打标记。见 [`Binding::Key`]。
pub fn current_platform() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// 一个 PTT 绑定。设置里存的是**一个列表**——按下任意一个即发话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Binding {
    /// 键盘。`code` 是 rdev `Key` 的规范名（`KeyV`、`Space`、`F12`……）。
    ///
    /// **rdev 只认到 F12**，再往上（F13–F24，而那恰恰是常用的 PTT 键）走
    /// `Key::Unknown(扫描码)`，而扫描码是**平台相关**的。所以这类绑定要带上
    /// 记录它的平台：换个系统读到时认不出来，那就明说"这个绑定失效了"，
    /// 而不是留一个永远不响的键。
    Key {
        code: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        platform: String,
    },
    /// 鼠标侧键。
    Mouse { button: MouseButton },
    /// 手柄按钮。
    Joystick { device: u32, button: u32 },
    /// 读进来但认不出来的绑定。**留着而不是丢掉**，界面才说得出"它失效了"。
    Unresolved { token: String },
}

impl Binding {
    /// 造一个键盘绑定，带上当前平台。
    pub fn key(code: impl Into<String>) -> Self {
        let code = code.into();
        // 有名字的键是跨平台的，不必打标记；只有 `Unknown(扫描码)` 才要。
        let platform = if code.starts_with("Unknown") {
            current_platform().to_string()
        } else {
            String::new()
        };
        Self::Key { code, platform }
    }

    /// 从 rdev 的按键造一个绑定。
    pub fn from_rdev_key(k: rdev::Key) -> Self {
        Self::key(format!("{k:?}"))
    }

    /// 网页 `KeyboardEvent.code` → 和 rdev `Key` 的 Debug 名同一套。
    ///
    /// macOS 上没辅助功能时 rdev `listen` **不报错，只是收不到键**，录制会空等。
    /// 绑定时走窗口内的 keydown，名字必须和之后 rdev 按下时的一样，否则录上了也不响。
    pub fn from_event_code(code: &str) -> Option<Self> {
        event_code_to_rdev(code).map(Self::key)
    }

    /// 这个绑定现在还能匹配到东西吗。
    pub fn is_live(&self) -> bool {
        match self {
            Self::Unresolved { .. } => false,
            // 带平台标记的键只在本平台有效：换个系统那个扫描码指向的是别的键。
            Self::Key { platform, .. } => platform.is_empty() || platform == current_platform(),
            Self::Mouse { .. } => mouse_supported(),
            Self::Joystick { .. } => true,
        }
    }

    /// 给界面拼文案用的短标识。**不是文案本身。**
    pub fn token(&self) -> String {
        match self {
            Self::Key { code, .. } => code.strip_prefix("Key").unwrap_or(code).to_string(),
            Self::Mouse { button } => button.token().to_string(),
            Self::Joystick { button, .. } => button.to_string(),
            Self::Unresolved { token } => token.clone(),
        }
    }
}

/// `KeyboardEvent.code` → rdev `Key` 的 `{:?}`。对不上的键返回 `None`，不要猜。
fn event_code_to_rdev(code: &str) -> Option<String> {
    if let Some(rest) = code.strip_prefix("Key") {
        if rest.len() == 1 && rest.as_bytes()[0].is_ascii_uppercase() {
            return Some(code.to_string());
        }
    }
    if let Some(d) = code.strip_prefix("Digit") {
        if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() {
            return Some(format!("Num{d}"));
        }
    }
    if let Some(n) = code.strip_prefix('F') {
        if let Ok(i) = n.parse::<u32>() {
            if (1..=12).contains(&i) {
                return Some(format!("F{i}"));
            }
        }
    }
    let named = match code {
        "Space" => "Space",
        "Tab" => "Tab",
        "Enter" => "Return",
        "Backspace" => "Backspace",
        "Escape" => "Escape",
        "CapsLock" => "CapsLock",
        "ControlLeft" => "ControlLeft",
        "ControlRight" => "ControlRight",
        "ShiftLeft" => "ShiftLeft",
        "ShiftRight" => "ShiftRight",
        "AltLeft" => "Alt",
        "AltRight" => "AltGr",
        "MetaLeft" => "MetaLeft",
        "MetaRight" => "MetaRight",
        "ArrowUp" => "UpArrow",
        "ArrowDown" => "DownArrow",
        "ArrowLeft" => "LeftArrow",
        "ArrowRight" => "RightArrow",
        "Delete" => "Delete",
        "Home" => "Home",
        "End" => "End",
        "PageUp" => "PageUp",
        "PageDown" => "PageDown",
        "Insert" => "Insert",
        "Minus" => "Minus",
        "Equal" => "Equal",
        "BracketLeft" => "LeftBracket",
        "BracketRight" => "RightBracket",
        "Backslash" => "BackSlash",
        "Semicolon" => "SemiColon",
        "Quote" => "Quote",
        "Backquote" => "BackQuote",
        "Comma" => "Comma",
        "Period" => "Dot",
        "Slash" => "Slash",
        "PrintScreen" => "PrintScreen",
        "ScrollLock" => "ScrollLock",
        "Pause" => "Pause",
        "NumLock" => "NumLock",
        _ => return None,
    };
    Some(named.to_string())
}

/// can-audio 的旧设置里，PTT 是 `ptt_key` + `joystick_ptt` 两个字段；新版是一个列表。
///
/// **升级时悄悄丢掉某人的 PTT 键，看起来和麦克风坏了一模一样**，所以迁移是必须的。
/// 认不出来的旧键名变成 [`Binding::Unresolved`] 而不是被丢掉——界面才说得出话。
pub fn migrate_legacy(old: &serde_json::Value) -> Vec<Binding> {
    let mut out = Vec::new();
    if let Some(k) = old.get("ptt_key").and_then(|v| v.as_str()) {
        if !k.trim().is_empty() {
            out.push(match legacy_key_code(k.trim()) {
                Some(code) => Binding::key(code),
                None => Binding::Unresolved {
                    token: k.trim().to_string(),
                },
            });
        }
    }
    if let Some(b) = old.get("joystick_ptt").and_then(|v| v.as_u64()) {
        out.push(Binding::Joystick {
            device: 0,
            button: b as u32,
        });
    }
    out
}

/// 旧键名 → rdev 的规范名。大小写不敏感。
///
/// 只覆盖**有名字**的键：旧配置里存不下平台相关的扫描码，所以认不出来的一律
/// 变成 `Unresolved` 让用户重绑，而不是猜一个。
fn legacy_key_code(name: &str) -> Option<String> {
    let n = name.to_ascii_lowercase();
    if n.len() == 1 {
        let c = n.chars().next()?;
        if c.is_ascii_alphabetic() {
            return Some(format!("Key{}", c.to_ascii_uppercase()));
        }
        if c.is_ascii_digit() {
            return Some(format!("Num{c}"));
        }
        return None;
    }
    // rdev 只到 F12。
    if let Some(num) = n.strip_prefix('f') {
        if let Ok(i) = num.parse::<u32>() {
            if (1..=12).contains(&i) {
                return Some(format!("F{i}"));
            }
            return None;
        }
    }
    let named = match n.as_str() {
        "space" => "Space",
        "tab" => "Tab",
        "enter" | "return" => "Return",
        "backspace" => "Backspace",
        "escape" | "esc" => "Escape",
        "capslock" => "CapsLock",
        "ctrl" | "control" | "ctrl_l" | "ctrl_left" => "ControlLeft",
        "ctrl_r" | "ctrl_right" => "ControlRight",
        "shift" | "shift_l" | "shift_left" => "ShiftLeft",
        "shift_r" | "shift_right" => "ShiftRight",
        "alt" => "Alt",
        "altgr" | "alt_gr" => "AltGr",
        "up" => "UpArrow",
        "down" => "DownArrow",
        "left" => "LeftArrow",
        "right" => "RightArrow",
        _ => return None,
    };
    Some(named.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_browser_event_code_matches_what_rdev_will_press() {
        assert_eq!(
            Binding::from_event_code("KeyV"),
            Some(Binding::from_rdev_key(rdev::Key::KeyV))
        );
        assert_eq!(
            Binding::from_event_code("Digit1"),
            Some(Binding::from_rdev_key(rdev::Key::Num1))
        );
        assert_eq!(
            Binding::from_event_code("Space"),
            Some(Binding::from_rdev_key(rdev::Key::Space))
        );
        assert_eq!(
            Binding::from_event_code("ControlLeft"),
            Some(Binding::from_rdev_key(rdev::Key::ControlLeft))
        );
        assert_eq!(
            Binding::from_event_code("Enter"),
            Some(Binding::from_rdev_key(rdev::Key::Return))
        );
        assert_eq!(Binding::from_event_code("Unidentified"), None);
        assert_eq!(Binding::from_event_code("F13"), None);
    }

    #[test]
    fn a_keyboard_binding_round_trips_through_settings() {
        let b = Binding::key("KeyV");
        let json = serde_json::to_string(&b).expect("serialise");
        assert_eq!(serde_json::from_str::<Binding>(&json).expect("parse"), b);
    }

    #[test]
    fn a_joystick_binding_round_trips_through_settings() {
        let b = Binding::Joystick {
            device: 0,
            button: 3,
        };
        let json = serde_json::to_string(&b).expect("serialise");
        assert_eq!(serde_json::from_str::<Binding>(&json).expect("parse"), b);
    }

    /// `token()` 是给界面拼文案用的**短标识**，不是文案本身。
    /// 措辞由上层的 i18n 决定——这个 crate 一个界面字符串都不产生。
    /// **Wayland 下键盘 PTT 不响，界面必须先说出来。**
    #[test]
    fn a_wayland_session_cannot_watch_the_keyboard() {
        assert!(!keyboard_supported_on(true, Some("wayland-0"), None));
        assert!(!keyboard_supported_on(true, None, Some("wayland")));
        // 大小写不该决定一个人能不能说话。
        assert!(!keyboard_supported_on(true, None, Some("Wayland")));
    }

    /// X11 可以，其余平台也可以——Wayland 是 Linux 独有的问题。
    #[test]
    fn x11_and_the_other_platforms_can() {
        assert!(keyboard_supported_on(true, None, Some("x11")));
        assert!(keyboard_supported_on(true, None, None));
        assert!(keyboard_supported_on(
            false,
            Some("wayland-0"),
            Some("wayland")
        ));
    }

    /// 空串等于没设。**照 `is_some` 判会把它当成 Wayland**，
    /// 于是一个 X11 用户被告知键盘 PTT 用不了。
    #[test]
    fn an_empty_wayland_display_is_not_a_wayland_session() {
        assert!(keyboard_supported_on(true, Some(""), None));
    }

    #[test]
    fn tokens_are_short_and_stable() {
        assert_eq!(Binding::key("KeyV").token(), "V");
        assert_eq!(Binding::key("ControlLeft").token(), "ControlLeft");
        assert_eq!(
            Binding::Mouse {
                button: MouseButton::X1
            }
            .token(),
            "X1"
        );
        assert_eq!(
            Binding::Joystick {
                device: 0,
                button: 3
            }
            .token(),
            "3"
        );
    }

    // ——— 鼠标只认侧键 ———

    /// **绑左键意味着在任何窗口里点任何东西都会发话**，而 TX 指示灯还被挡在
    /// 他刚点的那个东西后面。左/右/中键一律不可绑。
    #[test]
    fn the_main_mouse_buttons_are_not_bindable() {
        for raw in [
            rdev::Button::Left,
            rdev::Button::Right,
            rdev::Button::Middle,
        ] {
            assert_eq!(
                MouseButton::from_rdev(raw),
                None,
                "{raw:?} must not be bindable"
            );
        }
    }

    /// rdev 只给 `Unknown(u8)`，而那个数**每个平台不一样**。规范化成 X1/X2 存进设置，
    /// 否则同一份设置在 Windows 上是 X1、在 Linux 上什么都不是——
    /// 而症状是"我的 PTT 突然不灵了"，没有任何报错。
    ///
    /// 这张表的值是照 rdev 0.5.3 的源码定的：
    /// Windows 走 `WM_XBUTTONDOWN` 取 `HIWORD(mouseData)`（XBUTTON1=1、XBUTTON2=2），
    /// X11 直接给按钮号（4–7 是滚轮，rdev 已滤掉）。
    #[test]
    fn side_buttons_are_normalised_across_platforms() {
        #[cfg(target_os = "windows")]
        {
            assert_eq!(
                MouseButton::from_rdev(rdev::Button::Unknown(1)),
                Some(MouseButton::X1)
            );
            assert_eq!(
                MouseButton::from_rdev(rdev::Button::Unknown(2)),
                Some(MouseButton::X2)
            );
        }
        #[cfg(target_os = "linux")]
        {
            assert_eq!(
                MouseButton::from_rdev(rdev::Button::Unknown(8)),
                Some(MouseButton::X1)
            );
            assert_eq!(
                MouseButton::from_rdev(rdev::Button::Unknown(9)),
                Some(MouseButton::X2)
            );
        }
        // 认不出来的编号返回 None 而不是猜一个：猜错会让另一个键变成 PTT。
        assert_eq!(MouseButton::from_rdev(rdev::Button::Unknown(200)), None);
    }

    /// **macOS 上鼠标绑定不可用，而且这件事必须说得出口。**
    ///
    /// rdev 0.5.3 的 `macos/common.rs` 只处理 `LeftMouseDown/Up` 与 `RightMouseDown/Up`，
    /// `OtherMouseDown`/`OtherMouseUp` 一处都没有——连中键都报不出来。
    /// 一个绑好了、显示正常、却从来不响的 PTT，正是这个项目反复要躲开的那类故障，
    /// 所以界面要靠这个常量把话说在前面。
    #[test]
    fn the_platform_says_whether_mouse_bindings_work_at_all() {
        #[cfg(target_os = "macos")]
        assert!(
            !mouse_supported(),
            "rdev reports no OtherMouse events on macOS"
        );
        #[cfg(not(target_os = "macos"))]
        assert!(mouse_supported());
    }

    // ——— 从 Python 版的设置迁移 ———

    /// can-audio 存的是 `ptt_key` + `joystick_ptt` 两个字段，新版存的是一个列表。
    /// **升级时悄悄丢掉某人的 PTT 键，看起来和麦克风坏了一模一样**，
    /// 所以迁移是必须的，不是锦上添花。
    #[test]
    fn the_old_two_field_shape_migrates_into_a_list() {
        let old = serde_json::json!({ "ptt_key": "v", "joystick_ptt": 3 });
        let got = migrate_legacy(&old);
        assert_eq!(
            got,
            vec![
                Binding::key("KeyV"),
                Binding::Joystick {
                    device: 0,
                    button: 3
                },
            ]
        );
    }

    #[test]
    fn migrating_an_empty_legacy_config_yields_nothing() {
        assert!(migrate_legacy(&serde_json::json!({})).is_empty());
        assert!(migrate_legacy(&serde_json::json!({ "ptt_key": "" })).is_empty());
    }

    #[test]
    fn a_legacy_key_name_is_matched_case_insensitively() {
        assert_eq!(
            migrate_legacy(&serde_json::json!({ "ptt_key": "F12" })),
            vec![Binding::key("F12")]
        );
    }

    /// 认不出来的旧键名**不要默默丢掉**：报上去，让界面说"这个绑定失效了"。
    #[test]
    fn an_unrecognised_legacy_key_is_reported_not_dropped() {
        let got = migrate_legacy(&serde_json::json!({ "ptt_key": "不存在的键" }));
        assert_eq!(
            got,
            vec![Binding::Unresolved {
                token: "不存在的键".into()
            }]
        );
    }

    #[test]
    fn an_unresolved_binding_never_matches_anything() {
        let b = Binding::Unresolved {
            token: "whatever".into(),
        };
        assert!(
            !b.is_live(),
            "a binding that could not be resolved must not silently match"
        );
    }
}
