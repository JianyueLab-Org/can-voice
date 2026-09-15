//! `audio-for-can` —— 管制语音客户端的 Rust 侧。
//!
//! # 这一层只做桥接
//!
//! 全部逻辑在 `can-voice-app` 的 [`Bridge`] 和它下面的核心库里：耦合规则、订阅
//! 状态机、抖动缓冲、混音、PTT。这里只有 Tauri 命令和一个状态容器。
//!
//! **前端能做的事，就是核心库公开的那几件。** 没有 `join`、没有 `leave`、
//! 没有 channel id——`can-voice-app` 有一条扫源码的测试钉住这件事，而它扫的正是
//! 这一层要遵守的那条线。
//!
//! # 产品名指的是管制端
//!
//! `audio-for-can` 听起来像飞行员端，它不是。这四个产品名是 can-api 的固定白名单，
//! 历史错位，**沿用**。

use can_voice_app::{Bridge, Snapshot};
use can_voice_client::stack::Radio;
use can_voice_client::Config;
use can_voice_token::TokenSource;
use std::sync::Arc;

/// 应用的运行时状态。
pub struct App {
    bridge: Arc<Bridge>,
    http: reqwest::Client,
    /// PTT 监听。**懒起**：只有真的绑了键盘或鼠标才会去要辅助功能授权。
    ptt: std::sync::Mutex<Option<can_voice_ptt::PttWatcher>>,
}

impl App {
    pub fn new() -> Self {
        Self {
            bridge: Arc::new(Bridge::new()),
            http: reqwest::Client::builder()
                .user_agent(concat!("audio-for-can/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            ptt: std::sync::Mutex::new(None),
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

// ——— 命令 ———

/// 连上语音网。
///
/// 凭据只在这里出现一次，换成一张 60 秒的票之后就不再需要——**重连带的是票不是
/// 密码**，所以一个卡在重连里的客户端不会把账号锁出语音。
#[tauri::command]
async fn connect(
    state: tauri::State<'_, App>,
    cid: String,
    password: String,
) -> Result<(), String> {
    let tokens = TokenSource::new(
        &env_or("CAN_API_ORIGIN", "https://api.ceruleanavi.net"),
        cid,
        password,
        state.http.clone(),
    );
    let server = env_or("CAN_VOICE_SERVER", "audio.ceruleanavi.net:64738");
    let server_name = server.split(':').next().unwrap_or("localhost").to_string();

    state
        .bridge
        .connect(
            Config {
                server,
                server_name,
                // 由 `can_voice_token::connect` 填；票只从 TokenSource 来。
                token: String::new(),
                client_id: concat!("audio-for-can/", env!("CARGO_PKG_VERSION")).into(),
                follow: String::new(),
                input_device: None,
                output_device: None,
                audio_devices: true,
                extra_roots: Vec::new(),
            },
            &tokens,
        )
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn disconnect(state: tauri::State<'_, App>) -> Result<(), String> {
    state.bridge.disconnect().await;
    Ok(())
}

/// 当前状态。**前端挂上就读它**，不要从事件流拼——事件是广播，
/// 窗口重开之前发生的事收不到。
#[tauri::command]
fn snapshot(state: tauri::State<'_, App>) -> Snapshot {
    state.bridge.snapshot()
}

/// 当前台面。
#[tauri::command]
fn radios(state: tauri::State<'_, App>) -> Vec<Radio> {
    state.bridge.radios()
}

#[tauri::command]
fn add_frequency(state: tauri::State<'_, App>, freq_khz: u32) {
    state.bridge.with_stack(|s| s.add(freq_khz));
}

#[tauri::command]
fn remove_frequency(state: tauri::State<'_, App>, freq_khz: u32) {
    state.bridge.with_stack(|s| s.remove(freq_khz));
}

/// 三个开关。**耦合规则在核心库里，前端不要自己实现一遍**：关 RX 清 TX/XC、
/// 开 TX 强制 RX、开 XC 强制 RX+TX。
#[tauri::command]
fn set_switch(state: tauri::State<'_, App>, freq_khz: u32, switch: String, on: bool) {
    state.bridge.with_stack(|s| match switch.as_str() {
        "rx" => s.set_rx(freq_khz, on),
        "tx" => s.set_tx(freq_khz, on),
        "xc" => s.set_xc(freq_khz, on),
        other => tracing::warn!(other, "unknown switch"),
    });
}

#[tauri::command]
fn set_volume(state: tauri::State<'_, App>, freq_khz: u32, gain: f32) {
    state.bridge.set_volume(freq_khz, gain);
}

/// 界面上选中的那一行。**不发给服务端**——它和服务端的"主频率"是两件毫不相干
/// 的事，字段因此叫 `selected`。
#[tauri::command]
fn set_selected(state: tauri::State<'_, App>, freq_khz: u32) {
    state.bridge.with_stack(|s| s.set_selected(freq_khz));
}

/// 手动按下 / 松开（界面上那个按钮）。绑定的 PTT 走 [`set_ptt_bindings`]。
#[tauri::command]
fn set_transmitting(state: tauri::State<'_, App>, on: bool) {
    state.bridge.set_transmitting(on);
}

/// 换一组 PTT 绑定。
///
/// **监听是懒起的，而且起了就停不掉**——`rdev::listen` 没有 stop。所以只有真的绑了
/// 键盘或鼠标才会去要辅助功能授权（macOS 上那是一个弹窗，而一个只绑了手柄的用户
/// 被要求授权键盘监控，读起来像恶意软件）。
#[tauri::command]
fn set_ptt_bindings(state: tauri::State<'_, App>, bindings: Vec<can_voice_ptt::Binding>) {
    let mut slot = match state.ptt.lock() {
        Ok(s) => s,
        Err(p) => p.into_inner(),
    };
    match slot.as_ref() {
        Some(w) => w.set_bindings(bindings),
        None => {
            let watcher = can_voice_ptt::PttWatcher::new(bindings);
            spawn_ptt_pump(state.bridge.clone(), watcher.transmitting_flag());
            *slot = Some(watcher);
        }
    }
}

/// 读一次当前绑定按下与否，给界面点灯用。
#[tauri::command]
fn ptt_pressed(state: tauri::State<'_, App>) -> bool {
    state.ptt.lock().ok().and_then(|s| s.as_ref().map(|w| w.transmitting())).unwrap_or(false)
}

/// "按一下你要的键"。捕获期间事件**不驱动 PTT**——正在录的那一下不能被播出去。
#[tauri::command]
fn begin_ptt_capture(state: tauri::State<'_, App>) {
    if let Ok(s) = state.ptt.lock() {
        if let Some(w) = s.as_ref() {
            w.begin_capture();
        }
    }
}

#[tauri::command]
fn take_captured_binding(state: tauri::State<'_, App>) -> Option<can_voice_ptt::Binding> {
    state.ptt.lock().ok().and_then(|s| s.as_ref().and_then(|w| w.take_captured()))
}

/// 把 PTT 的按下状态泵给语音层。
///
/// 一帧一拍（20 毫秒）：比帧还快没有意义，慢了会让发话的头尾被切掉。
fn spawn_ptt_pump(bridge: std::sync::Arc<Bridge>, flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    tokio::spawn(async move {
        use std::sync::atomic::Ordering;
        let mut last = false;
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(20));
        loop {
            tick.tick().await;
            let now = flag.load(Ordering::Relaxed);
            if now != last {
                last = now;
                bridge.set_transmitting(now);
            }
        }
    });
}

/// 本系统能不能用鼠标侧键做 PTT。
///
/// **macOS 上不能**，而界面要把这件事说在前面：一个绑好了、显示正常、
/// 却从来不响的 PTT，正是这个项目反复要躲开的那类故障。
#[tauri::command]
fn mouse_ptt_supported() -> bool {
    can_voice_ptt::mouse_supported()
}

#[tauri::command]
fn audio_devices() -> serde_json::Value {
    serde_json::json!({
        "input": can_voice_client::audio::input_devices(),
        "output": can_voice_client::audio::output_devices(),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt().with_env_filter(env_or("RUST_LOG", "info")).init();

    tauri::Builder::default()
        .manage(App::new())
        .invoke_handler(tauri::generate_handler![
            connect,
            disconnect,
            snapshot,
            radios,
            add_frequency,
            remove_frequency,
            set_switch,
            set_volume,
            set_selected,
            set_transmitting,
            set_ptt_bindings,
            ptt_pressed,
            begin_ptt_capture,
            take_captured_binding,
            mouse_ptt_supported,
            audio_devices,
        ])
        .run(tauri::generate_context!())
        .expect("error while running audio-for-can");
}
