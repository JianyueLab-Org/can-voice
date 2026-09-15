//! 真正的监听：键盘/鼠标（rdev）与手柄（gilrs）。
//!
//! # rdev 的监听**停不下来**，这决定了整个结构
//!
//! `rdev::listen` 没有 stop、没有 unsubscribe、没有 `CFRunLoopStop`——核过源码，
//! 一处都没有。它跑到进程结束为止。三条后果：
//!
//! 1. **一个进程最多起一次**，而且是**懒起**：只有真的绑了键盘或鼠标才起。
//!    macOS 上创建全局键盘监听会触发辅助功能授权弹窗，而一个只绑了手柄的用户
//!    被要求授权键盘监控，读起来像恶意软件。
//! 2. **换绑定不重启监听**，只改共享状态。所以"取消了所有键盘绑定"之后监听还在
//!    ——这是 rdev 的限制，不是疏忽，写在这里免得下一个人去找那个不存在的 stop。
//! 3. **捕获共用这条监听**，不另起一条。那反倒把 Python 版的两个 bug 结构性地消掉了：
//!    两个线程同泵一个事件队列（不是线程安全的），以及正在录的那一下被播出去。
//!
//! # 绝不拦截
//!
//! 用的是 `rdev::listen`（只读）而不是任何会消费事件的接口。吞掉事件意味着那个键
//! 在**其它所有程序里**都失灵，而它在模拟器里通常还有别的用途。

use crate::binding::{Binding, MouseButton};
use crate::state::{DeviceReporter, Router, Sources};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// 手柄的轮询间隔。gilrs 没有阻塞式的等事件，只能轮询。
const JOYSTICK_POLL: std::time::Duration = std::time::Duration::from_millis(20);

/// 监听一组绑定，随时可以问"现在该不该发话"。
pub struct PttWatcher {
    router: Arc<Mutex<Router>>,
    transmitting: Arc<AtomicBool>,
    stop_joystick: Arc<AtomicBool>,
    rdev_started: Arc<AtomicBool>,
}

impl PttWatcher {
    /// 建一个监听器并按当前绑定启动需要的来源。
    pub fn new(bindings: Vec<Binding>) -> Self {
        let w = Self {
            router: Arc::new(Mutex::new(Router::new(bindings.clone()))),
            transmitting: Arc::new(AtomicBool::new(false)),
            stop_joystick: Arc::new(AtomicBool::new(false)),
            rdev_started: Arc::new(AtomicBool::new(false)),
        };
        w.ensure_sources(&bindings);
        w
    }

    /// 现在该不该发话。
    pub fn transmitting(&self) -> bool {
        self.transmitting.load(Ordering::Relaxed)
    }

    /// 把那一位借出去，让上层用自己的节奏去读。
    ///
    /// 借的是**同一个**原子量而不是一份拷贝：拷贝会让"松手"这件事在两处各判一次，
    /// 而它们迟早会不一致——一次没对上就是麦克风常开。
    pub fn transmitting_flag(&self) -> Arc<AtomicBool> {
        self.transmitting.clone()
    }

    /// 换一组绑定。**不会停掉任何已经起来的监听**（rdev 停不了），
    /// 但会把新需要的那些起起来。
    pub fn set_bindings(&self, bindings: Vec<Binding>) {
        if let Ok(mut r) = self.router.lock() {
            r.set_bindings(bindings.clone());
        }
        self.publish();
        self.ensure_sources(&bindings);
    }

    /// 开始"按一下你要的键"。捕获期间事件不驱动 PTT。
    pub fn begin_capture(&self) {
        if let Ok(mut r) = self.router.lock() {
            r.begin_capture();
        }
        self.publish();
    }

    pub fn cancel_capture(&self) {
        if let Ok(mut r) = self.router.lock() {
            r.cancel_capture();
        }
    }

    /// 取走捕获到的绑定（还没按下时返回 `None`）。
    pub fn take_captured(&self) -> Option<Binding> {
        self.router.lock().ok().and_then(|mut r| r.take_captured())
    }

    fn publish(&self) {
        if let Ok(r) = self.router.lock() {
            self.transmitting.store(r.transmitting(), Ordering::Relaxed);
        }
    }

    fn ensure_sources(&self, bindings: &[Binding]) {
        let need = Sources::needed(bindings);
        if need.rdev_listener() {
            self.ensure_rdev();
        }
        if need.joystick {
            self.ensure_joystick();
        }
    }

    /// 起 rdev 监听。**最多一次**，见模块文档。
    fn ensure_rdev(&self) {
        if self.rdev_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let router = self.router.clone();
        let flag = self.transmitting.clone();
        std::thread::Builder::new()
            .name("can-voice-ptt-rdev".into())
            .spawn(move || {
                let err = rdev::listen(move |event| {
                    let Some((binding, pressed)) = translate(&event.event_type) else {
                        return;
                    };
                    if let Ok(mut r) = router.lock() {
                        if pressed {
                            r.press(binding);
                        } else {
                            r.release(&binding);
                        }
                        flag.store(r.transmitting(), Ordering::Relaxed);
                    }
                });
                if let Err(e) = err {
                    // 起不来通常是权限：macOS 没给辅助功能、Linux 没有 X11。
                    tracing::warn!(error = ?e, "the keyboard/mouse listener could not start");
                }
            })
            .ok();
    }

    fn ensure_joystick(&self) {
        if self.stop_joystick.load(Ordering::Relaxed) {
            return;
        }
        let router = self.router.clone();
        let flag = self.transmitting.clone();
        let stop = self.stop_joystick.clone();
        std::thread::Builder::new()
            .name("can-voice-ptt-joystick".into())
            .spawn(move || {
                let mut reporter = DeviceReporter::default();
                let mut gilrs = match gilrs::Gilrs::new() {
                    Ok(g) => g,
                    Err(e) => {
                        tracing::warn!(error = %e, "no gamepad support on this system");
                        return;
                    }
                };
                while !stop.load(Ordering::Relaxed) {
                    while let Some(ev) = gilrs.next_event() {
                        let device = gamepad_index(&gilrs, ev.id);
                        let (button, pressed) = match ev.event {
                            gilrs::EventType::ButtonPressed(_, code) => (code.into_u32(), true),
                            gilrs::EventType::ButtonReleased(_, code) => (code.into_u32(), false),
                            gilrs::EventType::Connected => {
                                // 拔了再插是一次新的开始：下一次失败重新值得报一条。
                                reporter.opened(device);
                                continue;
                            }
                            gilrs::EventType::Disconnected => {
                                if reporter.should_warn(device) {
                                    tracing::warn!(device, "gamepad disconnected");
                                }
                                continue;
                            }
                            _ => continue,
                        };
                        let binding = Binding::Joystick { device, button };
                        if let Ok(mut r) = router.lock() {
                            if pressed {
                                r.press(binding);
                            } else {
                                r.release(&binding);
                            }
                            flag.store(r.transmitting(), Ordering::Relaxed);
                        }
                    }
                    std::thread::sleep(JOYSTICK_POLL);
                }
            })
            .ok();
    }
}

impl Drop for PttWatcher {
    fn drop(&mut self) {
        // 手柄那条停得掉；rdev 那条停不掉，见模块文档。
        self.stop_joystick.store(true, Ordering::Relaxed);
    }
}

fn gamepad_index(gilrs: &gilrs::Gilrs, id: gilrs::GamepadId) -> u32 {
    gilrs.gamepads().position(|(gid, _)| gid == id).unwrap_or(0) as u32
}

/// rdev 的事件 → 一个绑定加"按下还是松开"。不可绑的事件返回 `None`。
fn translate(e: &rdev::EventType) -> Option<(Binding, bool)> {
    match e {
        rdev::EventType::KeyPress(k) => Some((Binding::from_rdev_key(*k), true)),
        rdev::EventType::KeyRelease(k) => Some((Binding::from_rdev_key(*k), false)),
        rdev::EventType::ButtonPress(b) => {
            MouseButton::from_rdev(*b).map(|button| (Binding::Mouse { button }, true))
        }
        rdev::EventType::ButtonRelease(b) => {
            MouseButton::from_rdev(*b).map(|button| (Binding::Mouse { button }, false))
        }
        _ => None,
    }
}
