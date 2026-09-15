//! 绑定的聚合、来源选择，以及打不开的设备怎么报。

use crate::binding::Binding;
use std::collections::HashSet;

/// 这组绑定需要启动哪些输入来源。
///
/// **只启动被绑定的那些。** macOS 上创建全局键盘监听会触发辅助功能授权弹窗，
/// 而一个只绑了手柄的用户被要求授权键盘监控，读起来像恶意软件。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sources {
    pub keyboard: bool,
    pub mouse: bool,
    pub joystick: bool,
}

impl Sources {
    pub fn needed(bindings: &[Binding]) -> Self {
        let mut s = Self::default();
        for b in bindings.iter().filter(|b| b.is_live()) {
            match b {
                Binding::Key { .. } => s.keyboard = true,
                Binding::Mouse { .. } => s.mouse = true,
                Binding::Joystick { .. } => s.joystick = true,
                Binding::Unresolved { .. } => {}
            }
        }
        s
    }

    /// 键盘和鼠标走**同一个** rdev 监听，所以绑了其中任何一个都要起它。
    /// 写成一个方法而不是让调用方自己 `||`，是为了让"只起鼠标那一半"这个
    /// 不存在的选项不会被想出来。
    pub fn rdev_listener(&self) -> bool {
        self.keyboard || self.mouse
    }
}

/// 当前按下了哪些绑定。
#[derive(Debug, Default)]
pub struct PttState {
    bindings: Vec<Binding>,
    held: HashSet<String>,
}

impl PttState {
    pub fn new(bindings: Vec<Binding>) -> Self {
        Self {
            bindings,
            held: HashSet::new(),
        }
    }

    /// 换一组绑定。
    ///
    /// **按下状态一并清掉。** 不清的话，一个正按着 V 的人把绑定改成 B，
    /// 松手之后 V 的释放事件找不到对应的绑定，于是 `held` 里永远留着它——
    /// 麦克风从此常开，而界面上 PTT 是松开的。
    pub fn set_bindings(&mut self, bindings: Vec<Binding>) {
        self.bindings = bindings;
        self.held.clear();
    }

    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    /// 按下。不在绑定表里、或者已经失效的，什么都不做。
    pub fn press(&mut self, b: &Binding) {
        if self.is_bound(b) {
            self.held.insert(identity(b));
        }
    }

    /// 松开。
    pub fn release(&mut self, b: &Binding) {
        self.held.remove(&identity(b));
    }

    /// 现在该不该发话：**任意一个绑定按着就算**。
    pub fn transmitting(&self) -> bool {
        !self.held.is_empty()
    }

    fn is_bound(&self, b: &Binding) -> bool {
        b.is_live() && self.bindings.iter().any(|x| x == b && x.is_live())
    }
}

/// 一个绑定在"按下集合"里的身份。
fn identity(b: &Binding) -> String {
    format!("{b:?}")
}

/// 打不开的设备只报第一次。
///
/// 打不开的手柄会一直打不开：Python 版那条日志填满了整整一轮轮转，
/// 一次真实记录里是每 3 秒一条，持续到进程退出。
#[derive(Debug, Default)]
pub struct DeviceReporter {
    warned: HashSet<u32>,
}

impl DeviceReporter {
    /// 这一次失败值不值得记一条 WARN。
    pub fn should_warn(&mut self, device: u32) -> bool {
        self.warned.insert(device)
    }

    /// 这个设备打开成功了：下一次失败重新值得报一条。拔了再插是一次新的故障。
    pub fn opened(&mut self, device: u32) {
        self.warned.remove(&device);
    }
}

/// 事件的分发：同一条监听既要驱动 PTT，又要在"按一下你要的键"时录一个绑定。
///
/// **两件事必须由同一个地方裁决，而且互斥。** Python 版把捕获做成另起一个监听，
/// 于是两个线程同时泵一个事件队列（不是线程安全的），而且**正在录的那一下会被
/// 播出去**。这里只有一条监听——rdev 的 `listen` 本来也停不下来，见
/// [`crate::watcher`]——捕获期间事件只进捕获，不驱动 PTT。
#[derive(Debug, Default)]
pub struct Router {
    state: PttState,
    capturing: bool,
    captured: Option<Binding>,
}

impl Router {
    pub fn new(bindings: Vec<Binding>) -> Self {
        Self {
            state: PttState::new(bindings),
            capturing: false,
            captured: None,
        }
    }

    pub fn set_bindings(&mut self, bindings: Vec<Binding>) {
        self.state.set_bindings(bindings);
    }

    pub fn press(&mut self, b: Binding) {
        if self.capturing {
            // 录下来就停：一次捕获只要一个绑定，而且这一下不能出声。
            self.captured = Some(b);
            self.capturing = false;
            return;
        }
        self.state.press(&b);
    }

    pub fn release(&mut self, b: &Binding) {
        self.state.release(b);
    }

    /// 开始捕获。**先把按下状态清掉**：正按着 PTT 的人去改绑定，
    /// 松手事件会落在捕获模式里，`held` 就永远留着它了。
    pub fn begin_capture(&mut self) {
        self.capturing = true;
        self.captured = None;
        self.state.set_bindings(self.state.bindings().to_vec());
    }

    pub fn cancel_capture(&mut self) {
        self.capturing = false;
        self.captured = None;
    }

    pub fn take_captured(&mut self) -> Option<Binding> {
        self.captured.take()
    }

    pub fn capturing(&self) -> bool {
        self.capturing
    }

    pub fn transmitting(&self) -> bool {
        self.state.transmitting()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{mouse_supported, Binding, MouseButton};

    fn key(c: &str) -> Binding {
        Binding::key(c)
    }

    // ——— 按下任意一个即发话 ———

    #[test]
    fn nothing_held_means_not_transmitting() {
        let s = PttState::new(vec![key("KeyV")]);
        assert!(!s.transmitting());
    }

    #[test]
    fn any_bound_source_held_transmits() {
        let mut s = PttState::new(vec![
            key("KeyV"),
            Binding::Joystick {
                device: 0,
                button: 3,
            },
        ]);
        s.press(&Binding::Joystick {
            device: 0,
            button: 3,
        });
        assert!(s.transmitting(), "a bound joystick button must transmit");
    }

    #[test]
    fn releasing_one_of_two_held_bindings_keeps_transmitting() {
        // 两个都按着的时候松开一个不该断话——这在"键盘 + 手柄都绑了"的人身上
        // 是日常，而断在半句话上是能听出来的。
        let a = key("KeyV");
        let b = Binding::Joystick {
            device: 0,
            button: 3,
        };
        let mut s = PttState::new(vec![a.clone(), b.clone()]);
        s.press(&a);
        s.press(&b);
        s.release(&a);
        assert!(s.transmitting());
        s.release(&b);
        assert!(!s.transmitting());
    }

    #[test]
    fn an_unbound_key_does_nothing() {
        let mut s = PttState::new(vec![key("KeyV")]);
        s.press(&key("KeyB"));
        assert!(!s.transmitting());
    }

    /// 改绑定时**必须把按下状态清掉**。不清的话，一个正按着 V 的人把绑定改成 B，
    /// 松手之后 V 的释放事件找不到对应的绑定，于是 `held` 里永远留着它——
    /// 麦克风从此常开，而界面上 PTT 是松开的。
    #[test]
    fn rebinding_clears_whatever_was_held() {
        let mut s = PttState::new(vec![key("KeyV")]);
        s.press(&key("KeyV"));
        assert!(s.transmitting());
        s.set_bindings(vec![key("KeyB")]);
        assert!(
            !s.transmitting(),
            "a stale held key must not leave the microphone open"
        );
    }

    /// 失效的绑定**永远不匹配**，即使事件长得一样。
    #[test]
    fn an_unresolved_binding_never_transmits() {
        let dead = Binding::Unresolved {
            token: "F13".into(),
        };
        let mut s = PttState::new(vec![dead.clone()]);
        s.press(&dead);
        assert!(!s.transmitting());
    }

    // ——— 只启动被绑定的来源 ———

    /// macOS 上创建全局键盘监听会触发辅助功能授权弹窗。
    /// **用户只绑了手柄却被要求授权键盘监控，读起来像恶意软件。**
    #[test]
    fn only_the_bound_sources_are_started() {
        let only_joy = Sources::needed(&[Binding::Joystick {
            device: 0,
            button: 1,
        }]);
        assert!(
            !only_joy.keyboard,
            "do not ask for accessibility permission nobody needs"
        );
        assert!(!only_joy.mouse);
        assert!(only_joy.joystick);

        let only_key = Sources::needed(&[Binding::key("KeyV")]);
        assert!(only_key.keyboard && !only_key.joystick);
    }

    /// 键盘和鼠标走同一个 rdev 监听，所以绑了鼠标就得起那个监听——
    /// 这一点要写下来，否则下一个人会以为可以只起"鼠标"那一半。
    #[test]
    fn keyboard_and_mouse_share_one_listener() {
        let s = Sources::needed(&[Binding::key("KeyV")]);
        assert!(s.keyboard);
        assert!(s.rdev_listener());
        assert_eq!(s.rdev_listener(), s.keyboard || s.mouse);
    }

    /// **macOS 上鼠标绑定不算生效，所以它一个来源都不会拉起来。**
    ///
    /// 这不是退化，是那条"绑好了却从来不响"的故障被提前挡住：`is_live()` 为假，
    /// 界面据此说"这个绑定在本系统上不可用"，而不是起一个永远收不到侧键事件的监听。
    #[test]
    fn a_mouse_binding_follows_what_the_platform_can_actually_do() {
        let s = Sources::needed(&[Binding::Mouse {
            button: MouseButton::X1,
        }]);
        assert_eq!(s.mouse, mouse_supported());
        assert_eq!(s.rdev_listener(), mouse_supported());
    }

    #[test]
    fn no_bindings_starts_nothing() {
        let s = Sources::needed(&[]);
        assert!(!s.keyboard && !s.mouse && !s.joystick && !s.rdev_listener());
    }

    /// 失效的绑定不该把它那一类来源拉起来：一个从别的系统带过来的键盘绑定
    /// 不该在 macOS 上换来一个授权弹窗。
    #[test]
    fn a_dead_binding_does_not_start_its_source() {
        let s = Sources::needed(&[Binding::Unresolved { token: "x".into() }]);
        assert!(!s.keyboard && !s.mouse && !s.joystick);
    }

    // ——— 打不开的设备只报一次 ———

    /// 打不开的手柄会一直打不开。Python 版那条日志填满了整整一轮轮转：
    /// 一次真实记录里是每 3 秒一条 `could not open joystick 0`，持续到进程退出。
    #[test]
    fn a_device_that_will_not_open_is_reported_once() {
        let mut r = DeviceReporter::default();
        assert!(r.should_warn(0), "the first failure is worth a line");
        for _ in 0..100 {
            assert!(!r.should_warn(0), "the same failure is not worth 100 lines");
        }
    }

    #[test]
    fn each_device_gets_its_own_first_line() {
        let mut r = DeviceReporter::default();
        assert!(r.should_warn(0));
        assert!(
            r.should_warn(1),
            "a different device is a different problem"
        );
    }

    /// 打开成功之后要清零：拔了再插是一次新的故障，值得再报一次。
    #[test]
    fn a_successful_open_rearms_the_warning() {
        let mut r = DeviceReporter::default();
        assert!(r.should_warn(0));
        assert!(!r.should_warn(0));
        r.opened(0);
        assert!(r.should_warn(0), "a replug is a new problem");
    }

    // ——— 捕获与 PTT 互斥 ———

    /// **正在录的那一下不能被播出去。** Python 版把捕获做成另起一个监听，
    /// 于是按下"要绑的那个键"时 PTT 也跟着触发了。
    #[test]
    fn the_key_being_captured_does_not_go_on_the_air() {
        let mut r = Router::new(vec![key("KeyV")]);
        r.begin_capture();
        r.press(key("KeyV"));
        assert!(
            !r.transmitting(),
            "the key being bound must not key the microphone"
        );
        assert_eq!(r.take_captured(), Some(key("KeyV")));
    }

    #[test]
    fn capture_ends_after_one_binding() {
        let mut r = Router::new(vec![]);
        r.begin_capture();
        assert!(r.capturing());
        r.press(key("KeyB"));
        assert!(!r.capturing(), "one capture takes one binding");
    }

    #[test]
    fn events_drive_ptt_again_once_capture_is_done() {
        let mut r = Router::new(vec![key("KeyV")]);
        r.begin_capture();
        r.press(key("KeyB"));
        r.take_captured();
        r.press(key("KeyV"));
        assert!(r.transmitting());
    }

    /// 正按着 PTT 的人去改绑定：松手事件会落在捕获模式里，
    /// 不清的话 `held` 永远留着它——麦克风从此常开。
    #[test]
    fn beginning_a_capture_releases_whatever_was_held() {
        let mut r = Router::new(vec![key("KeyV")]);
        r.press(key("KeyV"));
        assert!(r.transmitting());
        r.begin_capture();
        assert!(
            !r.transmitting(),
            "a held key must not survive into capture mode"
        );
    }

    #[test]
    fn a_cancelled_capture_yields_nothing() {
        let mut r = Router::new(vec![]);
        r.begin_capture();
        r.cancel_capture();
        r.press(key("KeyB"));
        assert_eq!(r.take_captured(), None);
    }

    // ——— 这个 crate 不产生任何界面文字 ———

    /// can-audio 用一条扫 AST 的测试钉住 `ptt.py` 里不得有界面文字：共用文件里带中文
    /// 会让同一句话出现在四个地方，而翻译时会漏掉两个。这里扫**字符串字面量**
    /// 所在的行（注释不算——注释是写给读代码的人的，本来就该是中文）。
    #[test]
    fn no_ui_strings() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for entry in std::fs::read_dir(&dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            if !matches!(path.extension().and_then(|e| e.to_str()), Some("rs")) {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("read");
            let mut in_tests = false;
            for (n, line) in src.lines().enumerate() {
                if line.trim_start().starts_with("mod tests") {
                    in_tests = true;
                }
                if in_tests {
                    continue; // 测试里的断言消息不上界面
                }
                let t = line.trim_start();
                if t.starts_with("//") || t.starts_with("///") || t.starts_with("//!") {
                    continue;
                }
                if !t.contains('"') {
                    continue;
                }
                assert!(
                    !t.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                    "{}:{} has a CJK string literal — wording belongs to the i18n layer: {t}",
                    path.display(),
                    n + 1
                );
            }
        }
    }
}
