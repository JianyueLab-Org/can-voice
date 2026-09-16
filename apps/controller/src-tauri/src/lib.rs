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

/// 存下来的设置。
///
/// **密码不在里面。** 它只用来换一张 60 秒的票，之后重连带的是票；
/// 把一份长期凭据留在磁盘上买不到任何东西。
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    /// 上次用的 CAN 号，下次启动预填。
    #[serde(default)]
    pub cid: String,
    /// 频率台面。
    #[serde(default)]
    pub radios: Vec<Radio>,
    /// PTT 绑定。
    #[serde(default)]
    pub ptt: Vec<can_voice_ptt::Binding>,
    /// 录音设备。`None` 是系统默认。
    #[serde(default)]
    pub input_device: Option<String>,
    /// 播放设备。`None` 是系统默认。
    #[serde(default)]
    pub output_device: Option<String>,
    /// 用户说过"这一版不用再问我"的那个版本号。**跳过的是那一个版本，
    /// 不是从此闭嘴**——下一版照样提示。
    #[serde(default)]
    pub skipped_update: String,
}

/// 应用的运行时状态。
pub struct App {
    bridge: Arc<Bridge>,
    http: reqwest::Client,
    /// PTT 监听。**懒起**：只有真的绑了键盘或鼠标才会去要辅助功能授权。
    ptt: std::sync::Mutex<Option<can_voice_ptt::PttWatcher>>,
    store: can_voice_app::Store,
    settings: std::sync::Mutex<Settings>,
}

impl App {
    pub fn new() -> Self {
        // 产品名就是设置目录名。四个产品名是 can-api 的固定白名单，
        // 拿它当目录名等于不再多发明一个。
        let store = can_voice_app::Store::for_product("audio-for-can");
        let settings: Settings = store.load();
        let bridge = Arc::new(Bridge::new());
        // 上次的台面先装回去，**在任何连接之前**：声明是幂等全量的，
        // 连上的那一刻会把它整份推出去。
        bridge.with_stack(|s| restore_stack(s, &settings.radios));
        Self {
            bridge,
            http: reqwest::Client::builder()
                .user_agent(concat!("audio-for-can/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            ptt: std::sync::Mutex::new(None),
            store,
            settings: std::sync::Mutex::new(settings),
        }
    }

    /// 改一份设置并存下去。
    ///
    /// **每次改动都写**：改动都是用户动作，一次几百字节，而"存得住"就是
    /// 这件事的全部意义。台面每次都从桥那边重新取——它才是那份真相。
    fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        let mut s = match self.settings.lock() {
            Ok(s) => s,
            Err(p) => p.into_inner(),
        };
        f(&mut s);
        s.radios = self.bridge.radios();
        if let Err(e) = self.store.save(&*s) {
            tracing::warn!(error = %e, "could not save the settings");
        }
    }

    fn settings(&self) -> Settings {
        match self.settings.lock() {
            Ok(s) => s.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// 把存下来的台面装回去。
///
/// **顺序是承重的**：重放要走核心库的耦合规则（关 RX 会清掉 TX/XC），
/// 所以先 RX 再 TX 再 XC。反过来的话，一个存着 TX 的频率装回来变成只能听不能发，
/// 而界面看起来完全正常——正是这个项目反复要躲开的那一类。
fn restore_stack(stack: &mut can_voice_client::stack::RadioStack, saved: &[Radio]) {
    for r in saved {
        stack.add(r.freq_khz);
        stack.set_rx(r.freq_khz, r.rx);
        stack.set_tx(r.freq_khz, r.tx);
        stack.set_xc(r.freq_khz, r.xc);
        stack.set_gain(r.freq_khz, r.gain);
        if r.selected {
            stack.set_selected(r.freq_khz);
        }
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
    let remembered = cid.clone();
    let tokens = TokenSource::new(
        &env_or("CAN_API_ORIGIN", "https://api.ceruleanavi.net"),
        cid,
        password,
        state.http.clone(),
    );
    let server = env_or("CAN_VOICE_SERVER", "audio.ceruleanavi.net:64738");
    let server_name = server.split(':').next().unwrap_or("localhost").to_string();
    let saved = state.settings();

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
                // 一人一个账号，没有席位标记：同一个成员号第二次登录顶掉第一条。
                station: String::new(),
                input_device: saved.input_device.clone(),
                output_device: saved.output_device.clone(),
                audio_devices: true,
                extra_roots: Vec::new(),
            },
            &tokens,
        )
        .await
        .map_err(|e| e.to_string())?;
    // 连上了才记住这个号——连不上的那个多半是打错了。
    state.update_settings(|s| s.cid = remembered);
    Ok(())
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
    state.update_settings(|_| {});
}

#[tauri::command]
fn remove_frequency(state: tauri::State<'_, App>, freq_khz: u32) {
    state.bridge.with_stack(|s| s.remove(freq_khz));
    state.update_settings(|_| {});
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
    state.update_settings(|_| {});
}

#[tauri::command]
fn set_volume(state: tauri::State<'_, App>, freq_khz: u32, gain: f32) {
    state.bridge.set_volume(freq_khz, gain);
    state.update_settings(|_| {});
}

/// 界面上选中的那一行。**不发给服务端**——它和服务端的"主频率"是两件毫不相干
/// 的事，字段因此叫 `selected`。
#[tauri::command]
fn set_selected(state: tauri::State<'_, App>, freq_khz: u32) {
    state.bridge.with_stack(|s| s.set_selected(freq_khz));
    state.update_settings(|_| {});
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
    state.update_settings(|s| s.ptt = bindings.clone());
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

/// 当前设置。**前端一挂上就读它**：CAN 号、设备、PTT 绑定都在里面。
#[tauri::command]
fn settings(state: tauri::State<'_, App>) -> Settings {
    state.settings()
}

/// 换录音 / 播放设备。`None` 是"跟系统默认"。
///
/// **立刻生效**，不必重连：核心库在音频线程上重建两条流。
#[tauri::command]
fn set_audio_devices(
    state: tauri::State<'_, App>,
    input: Option<String>,
    output: Option<String>,
) {
    state.bridge.set_audio_devices(input.clone(), output.clone());
    state.update_settings(|s| {
        s.input_device = input;
        s.output_device = output;
    });
}

/// 一个绑定加上它给界面看的短标识。
///
/// `token()` 在 Rust 侧，措辞和"认不出来"的判断都只有一份——前端照着 `kind`
/// 自己拼一遍的话，换平台失效的那种绑定会显示成一个正常的键。
#[derive(Debug, serde::Serialize)]
pub struct BindingView {
    pub token: String,
    /// 读进来但认不出来的（换了平台的扫描码）。界面要说"它失效了"。
    pub unresolved: bool,
    pub binding: can_voice_ptt::Binding,
}

/// 当前的 PTT 绑定。
#[tauri::command]
fn ptt_bindings(state: tauri::State<'_, App>) -> Vec<BindingView> {
    state
        .settings()
        .ptt
        .into_iter()
        .map(|b| BindingView {
            token: b.token(),
            unresolved: matches!(b, can_voice_ptt::Binding::Unresolved { .. }),
            binding: b,
        })
        .collect()
}

/// 本系统能不能全局监听键盘。**Wayland 下不能。**
///
/// 和 [`mouse_ptt_supported`] 同一个理由：一个绑好了、显示正常、却从来不响的
/// PTT，是这个项目反复要躲开的那类故障。
#[tauri::command]
fn keyboard_ptt_supported() -> bool {
    can_voice_ptt::keyboard_supported()
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

// ——— 更新检查 ———

/// 查一次有没有新版。
///
/// **失败一律当成"没有更新"**，而且不打断正在工作的人：连着的时候一个模态框
/// 盖在台面上比晚一次更新糟得多，跳过的那一版也不再问。
#[tauri::command]
async fn check_update(
    app: tauri::State<'_, App>,
) -> Result<Option<can_voice_update::Latest>, String> {
    let (skipped, busy) = { let s = match app.settings.lock() {
            Ok(s) => s.clone(),
            Err(p) => p.into_inner().clone(),
        };
        // 连着的时候就是"正在工作"：PTT 是全局热键，弹窗抢了焦点就按不出去了。
        let busy = matches!(app.bridge.snapshot().link, can_voice_client::LinkState::Online);
        (s.skipped_update, busy) };
    let origin = env_or("CAN_API_ORIGIN", "https://api.ceruleanavi.net");
    let Some(latest) = can_voice_update::check(
        &app.http,
        &origin,
        "audio-for-can",
        env!("CARGO_PKG_VERSION"),
    )
    .await else {
        return Ok(None);
    };
    let skipped = (!skipped.is_empty()).then_some(skipped);
    Ok(can_voice_update::should_prompt(
        &latest.version,
        env!("CARGO_PKG_VERSION"),
        skipped.as_deref(),
        busy,
    )
    .then_some(latest))
}

/// 记住"这一版不用再问我"。**跳过的是那一个版本，不是从此闭嘴。**
#[tauri::command]
fn skip_update(app: tauri::State<'_, App>, version: String) {
    app.update_settings(|s| s.skipped_update = version);
}

/// 用系统浏览器打开下载页。**绝不自动更新**：装不装、什么时候装是人决定的。
#[tauri::command]
fn open_download(url: String) -> Result<(), String> {
    can_voice_update::open_in_browser(&url)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
// ——— 日志 ———

/// 当前这份日志在哪。界面上显示给用户，让他知道要发的是哪个文件。
#[tauri::command]
fn log_file() -> Option<String> {
    can_voice_log::path().map(|p| p.display().to_string())
}

/// 把日志寄回去。
///
/// **要 CAN 号和密码**：can-api 的 `/api/v1/logs` 认的是这一对，不是会话。
/// 密码用完就丢，不进设置文件。
#[tauri::command]
async fn send_log(
    app: tauri::State<'_, App>,
    cid: String,
    password: String,
) -> Result<(), String> {
    let origin = env_or("CAN_API_ORIGIN", "https://api.ceruleanavi.net");
    can_voice_log::upload(
        &app.http,
        &origin,
        "audio-for-can",
        env!("CARGO_PKG_VERSION"),
        &cid,
        &password,
    )
    .await
}

pub fn run() {
    // **日志要落盘。** 打包出来的是一个没有控制台的 GUI 进程，`stdout` 写到哪里
    // 谁也看不见；用户报"连不上"的时候手里得有一份能发出来的东西。
    // 这一步同时装上 panic 钩子——崩溃不留记录的话，窗口没了、日志干净。
    can_voice_log::init("audio-for-can", std::env::args().any(|a| a == "--debug"));

    tauri::Builder::default()
        .manage(App::new())
        .invoke_handler(tauri::generate_handler![
            log_file,
            send_log,
            check_update,
            skip_update,
            open_download,
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
            settings,
            set_audio_devices,
            ptt_bindings,
            keyboard_ptt_supported,
        ])
        .run(tauri::generate_context!())
        .expect("error while running audio-for-can");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **存下来的台面要原样装回去。**
    ///
    /// 重放要走耦合规则（关 RX 会清掉 TX/XC），所以顺序是承重的：先 RX 再 TX
    /// 再 XC。顺序反了的话，一个存着 TX 的频率装回来是只能听不能发，
    /// 而界面看起来完全正常。
    #[test]
    fn a_saved_stack_is_restored_switch_for_switch() {
        let saved = vec![
            Radio {
                freq_khz: 118_000,
                rx: true,
                tx: false,
                xc: false,
                gain: 0.5,
                selected: false,
            },
            Radio {
                freq_khz: 121_800,
                rx: true,
                tx: true,
                xc: false,
                gain: 1.0,
                selected: true,
            },
        ];

        let mut stack = can_voice_client::stack::RadioStack::new();
        restore_stack(&mut stack, &saved);

        assert_eq!(stack.radios(), saved.as_slice());
    }
}
