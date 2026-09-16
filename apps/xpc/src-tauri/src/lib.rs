//! `xpc-for-can` —— X-Plane 飞行客户端的 Rust 侧。
//!
//! # 这一层把五条线接起来，逻辑都不在这儿
//!
//! ```text
//! can-voice-sim::xplane       X-Plane 的 UDP 链路 → Snapshot
//! can-voice-fsd::pilot_client FSD 飞行员会话（位置、他机、文字）
//! can-voice-sim::traffic      他机表与插值
//! can-voice-sim::bridge       发给 XPPython3 插件的本地通道
//! can-voice-app::Bridge       语音（订阅跟着 COM1 走）
//! ```
//!
//! # 频率跟着 COM1 走，不是让人在界面上再填一遍
//!
//! 飞行员调的是座舱里的 COM1，客户端上再有一个频率框就会有两个真相。
//! 这里每一帧读 [`Snapshot::com1`]，变了就换订阅。**COM1 电门关着时退订**
//! ——关了电门还在频率上说话，对管制来说是个幽灵。

use can_voice_app::Bridge;
use can_voice_fsd::pilot::{FlightPlan, PilotIdentity, PilotPosition};
use can_voice_fsd::pilot_client::{self, PilotConfig, PilotEvent, PilotHandle};
use can_voice_sim::chat::{ChatLog, ChatMessage};
use can_voice_sim::controllers::{ControllerEntry, ControllerTable};
use can_voice_sim::csl::ModelSet;
use can_voice_sim::traffic::{Entry, TrafficTable};
use can_voice_sim::{bridge, xplane, Snapshot};
use can_voice_token::TokenSource;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 位置上报的节奏。和 [`pilot_client::POSITION_INTERVAL`] 同一个值——
/// 这里只是把模拟器那一帧喂过去，真正决定发不发的是那边。
const PUMP_INTERVAL: Duration = Duration::from_millis(200);
/// 往插件推一帧的节奏。比位置上报快，插值才有意义。
const PLUGIN_INTERVAL: Duration = Duration::from_millis(50);
/// TCAS 只有 64 个位置。
const MAX_TRAFFIC: usize = 64;
/// 超出这个距离的不往插件送——画不出来的飞机白占 TCAS 的位置。
const MAX_RANGE_NM: f64 = 200.0;
/// 多久去问一轮机型和配置。
const ASK_INTERVAL: Duration = Duration::from_secs(2);
/// 同一架飞机的配置多久重问一次。灯和襟翼一直在变。
const CONFIG_REFRESH: f64 = 10.0;

/// 存下来的设置。
///
/// **密码不在里面。** 它只换一张短寿命的票，把长期凭据留在磁盘上买不到东西。
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub cid: String,
    #[serde(default)]
    pub callsign: String,
    #[serde(default)]
    pub aircraft: String,
    #[serde(default)]
    pub real_name: String,
    #[serde(default)]
    pub ptt: Vec<can_voice_ptt::Binding>,
    #[serde(default)]
    pub input_device: Option<String>,
    #[serde(default)]
    pub output_device: Option<String>,
    /// 往模拟器里注入他机。**默认开**，但要能关：想只用语音不看他机的人
    /// 现在关不掉，而注入是最吃帧数的那一部分。
    #[serde(default = "yes")]
    pub inject: bool,
    /// 用户说过"这一版不用再问我"的那个版本号。**跳过的是那一个版本，
    /// 不是从此闭嘴**——下一版照样提示。
    #[serde(default)]
    pub skipped_update: String,
}

fn yes() -> bool {
    true
}

pub struct App {
    voice: Arc<Bridge>,
    sim: xplane::Link,
    fsd: Mutex<Option<PilotHandle>>,
    /// FSD 链路最近一条事件。**界面靠它判断"上线了没有"**。
    link: Arc<Mutex<Option<can_voice_fsd::session::FsdEvent>>>,
    /// 插件最近一次回报。`None` = 从没听到过。
    plugin: Arc<Mutex<Option<(std::time::Instant, bridge::Status)>>>,
    store: can_voice_settings::Store,
    settings: Mutex<Settings>,
    /// 注入开关。注入的那几条循环每一拍读它。
    inject: Arc<std::sync::atomic::AtomicBool>,
    traffic: Arc<Mutex<TrafficTable>>,
    /// 收发过的文字消息。**攒在这里而不是靠事件推**：窗口重开之前
    /// 管制员说过的话，靠事件流是收不到的。
    chat: Arc<Mutex<ChatLog>>,
    /// 在线管制席位。
    controllers: Arc<Mutex<ControllerTable>>,
    csl: Arc<Mutex<ModelSet>>,
    http: reqwest::Client,
    ptt: Mutex<Option<can_voice_ptt::PttWatcher>>,
}

impl App {
    pub fn new() -> Self {
        let store = can_voice_settings::Store::for_product("xpc-for-can");
        let settings: Settings = store.load();
        Self {
            voice: Arc::new(Bridge::new()),
            sim: xplane::Link::spawn(),
            fsd: Mutex::new(None),
            link: Arc::new(Mutex::new(None)),
            plugin: {
                let slot = Arc::new(Mutex::new(None));
                spawn_plugin_status_reader(slot.clone());
                slot
            },
            store,
            inject: Arc::new(std::sync::atomic::AtomicBool::new(settings.inject)),
            settings: Mutex::new(settings),
            traffic: Arc::new(Mutex::new(TrafficTable::new())),
            chat: Arc::new(Mutex::new(ChatLog::default())),
            controllers: Arc::new(Mutex::new(ControllerTable::default())),
            // CSL 在**后台**加载：几个 GB 的包扫一遍要几十秒，放在这里会让
            // 窗口几十秒打不开，而用户看到的是程序卡死。
            csl: Arc::new(Mutex::new(ModelSet::default())),
            http: reqwest::Client::builder()
                .user_agent(concat!("xpc-for-can/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            ptt: Mutex::new(None),
        }
    }

    fn settings_snapshot(&self) -> Settings {
        match self.settings.lock() {
            Ok(s) => s.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    /// 改一份设置并存下去。每次改动都写：改动都是用户动作，一次几百字节。
    fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        let mut s = match self.settings.lock() {
            Ok(s) => s,
            Err(p) => p.into_inner(),
        };
        f(&mut s);
        if let Err(e) = self.store.save(&*s) {
            tracing::warn!(error = %e, "could not save the settings");
        }
    }
}

/// CSL 包放在哪儿。
///
/// 默认是 X-Plane 那套插件的老地方；装在别处（几个 GB 的包常常在另一块盘上）
/// 就用 `CAN_XPC_CSL_DIR` 指过去。
fn csl_root() -> std::path::PathBuf {
    std::env::var_os("CAN_XPC_CSL_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("Resources/plugins/CSL"))
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// 插件那一侧的状况。
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct PluginView {
    /// 它报的协议版本。
    pub version: u32,
    /// 它此刻画着几架。
    pub drawn: usize,
    /// 版本对不对得上。**对不上时插件静默丢弃每一帧**，症状是"完全没有交通"
    /// 而两边日志都干净——这是最难查的一种，所以要单独说。
    pub version_ok: bool,
}

/// 多久没听到就算它不在了。插件每秒回报一次。
const PLUGIN_STALE: Duration = Duration::from_secs(5);

/// 界面读的一份快照。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct View {
    /// X-Plane 连上了没有。
    pub sim_connected: bool,
    pub sim: Option<Snapshot>,
    pub link: Option<can_voice_fsd::session::FsdState>,
    pub reason: Option<can_voice_fsd::session::Reason>,
    pub traffic: Vec<Entry>,
    pub voice: Option<can_voice_app::Snapshot>,
    /// 收发过的文字消息，最旧的在前。
    pub messages: Vec<ChatMessage>,
    /// 在线管制席位，按呼号排序。
    pub controllers: Vec<ControllerEntry>,
    /// X-Plane 插件。`None` = 没听到过它——没装、没启用，或者 X-Plane 没开。
    pub plugin: Option<PluginView>,
}

/// 把模拟器那一帧变成一次位置上报。
fn to_position(s: &Snapshot, rating: u32) -> PilotPosition {
    PilotPosition {
        mode: s.xpdr_mode,
        squawk: s.squawk,
        rating,
        latitude: s.latitude,
        longitude: s.longitude,
        altitude: s.altitude,
        groundspeed: s.groundspeed,
        pitch: s.pitch,
        bank: s.bank,
        heading: s.heading,
        on_ground: s.on_ground,
        pressure_delta: s.pressure_delta,
    }
}

/// COM1（MHz）→ 频率（kHz）。电门关着或者读不到就是 `None`。
fn voice_frequency(s: &Snapshot) -> Option<u32> {
    if !s.com1_power {
        return None;
    }
    let mhz = s.com1?;
    let khz = (mhz * 1000.0).round();
    // 夹在 VHF 波段里。模拟器初始化过程中 COM1 会短暂读出 0 或者别的怪值，
    // 照单订阅会让人落在一个谁也不在的频率上，而界面看着一切正常。
    (118_000.0..=136_975.0).contains(&khz).then_some(khz as u32)
}

// ——— 命令 ———

#[tauri::command]
async fn connect(
    app: tauri::State<'_, App>,
    cid: String,
    password: String,
    callsign: String,
    aircraft: String,
    real_name: String,
) -> Result<(), String> {
    can_voice_fsd::pilot::check_pilot_callsign(&callsign).map_err(|e| e.to_string())?;
    let saved = app.settings_snapshot();

    // 语音先连。凭据只在这里出现一次，换成一张短期票之后就不再需要——
    // 重连带的是票不是密码，所以一个卡在重连里的客户端不会把账号锁出语音。
    let tokens = TokenSource::new(
        &env_or("CAN_API_ORIGIN", "https://api.ceruleanavi.net"),
        cid.clone(),
        password.clone(),
        app.http.clone(),
    );
    let voice_cfg = can_voice_client::Config {
        server: env_or("CAN_VOICE_SERVER", "audio.ceruleanavi.net:64738"),
        server_name: env_or("CAN_VOICE_SERVER_NAME", "audio.ceruleanavi.net"),
        token: String::new(),
        client_id: concat!("xpc-for-can/", env!("CARGO_PKG_VERSION")).into(),
        follow: String::new(),
        // 一人一个账号，没有席位标记：同一个成员号第二次登录顶掉第一条。
        station: String::new(),
        input_device: saved.input_device.clone(),
        output_device: saved.output_device.clone(),
        audio_devices: true,
        extra_roots: Vec::new(),
    };
    app.voice
        .connect(voice_cfg, &tokens)
        .await
        .map_err(|e| e.to_string())?;

    let fsd = pilot_client::connect(PilotConfig {
        host: env_or("CAN_FSD_HOST", "fsd.ceruleanavi.net"),
        port: env_or("CAN_FSD_PORT", "6809")
            .parse()
            .unwrap_or(can_voice_fsd::packet::DEFAULT_PORT),
        identity: PilotIdentity::new(
            &callsign,
            &cid,
            &password,
            &real_name,
            can_voice_fsd::packet::RATING_OBSERVER,
            can_voice_fsd::pilot::SIMULATOR_XPLANE_12,
            concat!("XPC for CAN ", env!("CARGO_PKG_VERSION")),
        ),
        reconnect_limit: pilot_client::RECONNECT_LIMIT,
        // 别人问起时答这个。不答的话对方只能拿通用模型画我们——
        // 一架 A320 在别人屏幕上是 737。
        aircraft: aircraft.trim().to_uppercase(),
        airline: callsign
            .trim()
            .to_uppercase()
            .chars()
            .take(3)
            .filter(|c| c.is_ascii_alphabetic())
            .collect(),
    });

    spawn_link_reader(fsd.link(), app.link.clone());
    spawn_traffic_reader(
        fsd.traffic(),
        app.traffic.clone(),
        app.chat.clone(),
        app.controllers.clone(),
    );
    spawn_asking(fsd.clone(), app.traffic.clone());
    spawn_pump(fsd.clone(), app.sim.clone(), app.voice.clone());
    spawn_plugin_feed(
        app.sim.clone(),
        app.traffic.clone(),
        app.csl.clone(),
        app.inject.clone(),
    );
    *app.fsd.lock().expect("fsd") = Some(fsd);
    // 上线成功才记住这一组：连不上的那一组多半有一项是打错的。
    app.update_settings(|s| {
        s.cid = cid;
        s.callsign = callsign;
        s.aircraft = aircraft;
        s.real_name = real_name;
    });
    Ok(())
}

#[tauri::command]
async fn disconnect(app: tauri::State<'_, App>) -> Result<(), String> {
    if let Some(fsd) = app.fsd.lock().expect("fsd").take() {
        fsd.stop();
    }
    *app.link.lock().expect("link") = None;
    app.voice.disconnect().await;
    app.traffic.lock().expect("traffic").prune(f64::MAX);
    Ok(())
}

/// 存下来的设置。**前端一挂上就读它**：CAN 号、呼号、机型、姓名都预填回去。
#[tauri::command]
fn settings(app: tauri::State<'_, App>) -> Settings {
    app.settings_snapshot()
}

/// 开 / 关他机注入。
///
/// 关掉时**喂一份空的他机表**而不是停掉那条循环：停掉的话已经画出来的飞机会
/// 留在天上不动，而空表会让它们按正常的消失路径被撤掉。
#[tauri::command]
fn set_injection(app: tauri::State<'_, App>, on: bool) {
    app.inject.store(on, std::sync::atomic::Ordering::Relaxed);
    app.update_settings(|s| s.inject = on);
}

/// 换录音 / 播放设备。`None` 是跟系统默认。
///
/// **立刻生效**，不必重新上线：核心库在音频线程上重建两条流。
#[tauri::command]
fn set_audio_devices(app: tauri::State<'_, App>, input: Option<String>, output: Option<String>) {
    app.voice.set_audio_devices(input.clone(), output.clone());
    app.update_settings(|s| {
        s.input_device = input;
        s.output_device = output;
    });
}

/// 一个绑定加上它给界面看的短标识。`token()` 只有 Rust 侧一份。
#[derive(Debug, serde::Serialize)]
pub struct BindingView {
    pub token: String,
    /// 读进来但认不出来的（换了平台的扫描码）。界面要说"它失效了"。
    pub unresolved: bool,
    pub binding: can_voice_ptt::Binding,
}

/// 当前的 PTT 绑定。
#[tauri::command]
fn ptt_bindings(app: tauri::State<'_, App>) -> Vec<BindingView> {
    app.settings_snapshot()
        .ptt
        .into_iter()
        .map(|b| BindingView {
            token: b.token(),
            unresolved: matches!(b, can_voice_ptt::Binding::Unresolved { .. }),
            binding: b,
        })
        .collect()
}

/// 可用的录音 / 播放设备。
#[tauri::command]
fn audio_devices() -> serde_json::Value {
    serde_json::json!({
        "input": can_voice_client::audio::input_devices(),
        "output": can_voice_client::audio::output_devices(),
    })
}

/// 本系统能不能全局监听键盘。**Wayland 下不能。**
#[tauri::command]
fn keyboard_ptt_supported() -> bool {
    can_voice_ptt::keyboard_supported()
}

#[tauri::command]
fn view(app: tauri::State<'_, App>) -> View {
    build_view(&app)
}

/// 拼一份界面快照。
///
/// 和命令分开是为了可测：`link` 曾经在这里写死 `None`，而前端的
/// `online` / `connected` 全由它推导，于是连上之后界面整个锁死在登录态。
fn build_view(app: &App) -> View {
    let sim = app.sim.snapshot();
    let now = monotonic();
    let origin = sim.as_ref().map(|s| (s.latitude, s.longitude));
    let last_link = app.link.lock().expect("link").clone();
    View {
        sim_connected: app.sim.connected(),
        traffic: app.traffic.lock().expect("traffic").snapshot(
            now,
            origin,
            Some(MAX_TRAFFIC),
            Some(MAX_RANGE_NM),
        ),
        sim,
        link: last_link.as_ref().map(|e| e.state),
        reason: last_link.map(|e| e.reason),
        voice: Some(app.voice.snapshot()),
        messages: app.chat.lock().expect("chat").snapshot(),
        controllers: app
            .controllers
            .lock()
            .expect("controllers")
            .snapshot(now, origin),
        plugin: app
            .plugin
            .lock()
            .expect("plugin")
            .filter(|(at, _)| at.elapsed() < PLUGIN_STALE)
            .map(|(_, s)| PluginView {
                version: s.v,
                drawn: s.drawn,
                version_ok: bridge::version_matches(&s),
            }),
    }
}

/// 听插件的回报。
///
/// **在连接之前就开始听**：用户要在上线之前就知道插件装没装，而不是上线之后
/// 对着一片空白的天空猜。
fn spawn_plugin_status_reader(slot: Arc<Mutex<Option<(std::time::Instant, bridge::Status)>>>) {
    tokio::spawn(async move {
        let addr = format!("{}:{}", bridge::HOST, bridge::CLIENT_PORT);
        let Ok(socket) = tokio::net::UdpSocket::bind(&addr).await else {
            // 端口被占是可能的（开了两个客户端）。第二个听不到插件，
            // 但别的都照常——所以这里只记一行，不影响启动。
            tracing::warn!(%addr, "could not listen for the plugin; its status will show as absent");
            return;
        };
        let mut buf = [0u8; 2048];
        loop {
            let Ok((n, _)) = socket.recv_from(&mut buf).await else {
                return;
            };
            if let Some(status) = bridge::decode_status(&buf[..n]) {
                *slot.lock().expect("plugin") = Some((std::time::Instant::now(), status));
            }
        }
    });
}

/// 发一条文字消息。
///
/// 返回 `Err` 而不是 `false`：发不出去有三种不同的原因（没上线、没写正文、
/// 既没填收件人又没有 COM1 频率），而一个 `false` 让界面只能说"发送失败"。
#[tauri::command]
fn send_text(app: tauri::State<'_, App>, recipient: String, message: String) -> Result<(), String> {
    let com1 = app.sim.snapshot().as_ref().and_then(voice_frequency);
    // 收件人和正文在**这里**定下来，然后原样交给 FSD 那一侧——聊天记录里
    // 那一行必须和真正发出去的那一包是同一个答案。
    let out = can_voice_sim::chat::outgoing(&recipient, &message, com1)
        .ok_or_else(|| "没有可发的内容：正文是空的，或者既没填收件人也没有 COM1 频率".to_string())?;
    match app.fsd.lock().expect("fsd").as_ref() {
        Some(fsd) => fsd.send_text(out.to.clone(), out.text.clone()),
        // **没发出去就不记**：记了的话聊天区里那句话看起来发出去了。
        None => return Err("还没上线".into()),
    }
    record_sent(&app, out.to, out.text);
    Ok(())
}

/// 把自己发出去的一条记进聊天记录。
///
/// 发信人写自己的呼号而不是留空：界面按呼号分组显示，留空的话自己那半边
/// 全挤在一个没有名字的分组里。
fn record_sent(app: &App, recipient: String, message: String) {
    let from = app.settings.lock().expect("settings").callsign.clone();
    app.chat.lock().expect("chat").record(ChatMessage {
        from,
        to: recipient,
        text: message,
        outbound: true,
        at: monotonic(),
    });
}

#[tauri::command]
fn ident(app: tauri::State<'_, App>) -> bool {
    match app.fsd.lock().expect("fsd").as_ref() {
        Some(fsd) => {
            fsd.ident();
            true
        }
        None => false,
    }
}

#[tauri::command]
fn file_flight_plan(app: tauri::State<'_, App>, plan: FlightPlan) -> bool {
    match app.fsd.lock().expect("fsd").as_ref() {
        Some(fsd) => {
            fsd.file_flight_plan(plan);
            true
        }
        None => false,
    }
}

#[tauri::command]
fn set_transmitting(app: tauri::State<'_, App>, on: bool) {
    app.voice.set_transmitting(on);
}

/// 换一组 PTT 绑定。
///
/// **监听是懒起的，而且起了就停不掉**（`rdev::listen` 没有 stop）。所以只有
/// 真的绑了键盘或鼠标才会去要辅助功能授权——一个只绑了手柄的用户被要求授权
/// 键盘监控，读起来像恶意软件。
#[tauri::command]
fn set_ptt_bindings(app: tauri::State<'_, App>, bindings: Vec<can_voice_ptt::Binding>) {
    app.update_settings(|s| s.ptt = bindings.clone());
    let mut slot = match app.ptt.lock() {
        Ok(s) => s,
        Err(p) => p.into_inner(),
    };
    match slot.as_ref() {
        Some(w) => w.set_bindings(bindings),
        None => {
            let watcher = can_voice_ptt::PttWatcher::new(bindings);
            spawn_ptt_pump(app.voice.clone(), watcher.transmitting_flag());
            *slot = Some(watcher);
        }
    }
}

#[tauri::command]
fn ptt_pressed(app: tauri::State<'_, App>) -> bool {
    app.ptt
        .lock()
        .ok()
        .and_then(|s| s.as_ref().map(|w| w.transmitting()))
        .unwrap_or(false)
}

/// "按一下你要的键"。捕获期间事件**不驱动 PTT**——正在录的那一下不能被播出去。
#[tauri::command]
fn begin_ptt_capture(app: tauri::State<'_, App>) {
    if let Ok(s) = app.ptt.lock() {
        if let Some(w) = s.as_ref() {
            w.begin_capture();
        }
    }
}

#[tauri::command]
fn take_captured_binding(app: tauri::State<'_, App>) -> Option<can_voice_ptt::Binding> {
    app.ptt
        .lock()
        .ok()
        .and_then(|s| s.as_ref().and_then(|w| w.take_captured()))
}

/// 本系统能不能用鼠标侧键做 PTT。**macOS 上不能**，界面要把这件事说在前面。
#[tauri::command]
fn mouse_ptt_supported() -> bool {
    can_voice_ptt::mouse_supported()
}

/// 把 PTT 的按下状态泵给语音层。
///
/// 一帧一拍（20 毫秒）：比帧还快没有意义，慢了会让发话的头尾被切掉。
fn spawn_ptt_pump(voice: Arc<Bridge>, flag: Arc<std::sync::atomic::AtomicBool>) {
    tokio::spawn(async move {
        use std::sync::atomic::Ordering;
        let mut last = false;
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        loop {
            tick.tick().await;
            let now = flag.load(Ordering::Relaxed);
            if now != last {
                last = now;
                voice.set_transmitting(now);
            }
        }
    });
}

// ——— 后台 ———

/// 单调秒。他机表不读时钟，时间由这里给。
fn monotonic() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
}

/// 把 FSD 链路事件记进 `App.link`——界面读的就是它。
///
/// 没有这一路的时候 `link` 恒为 `None`，而前端的 `online` / `connected`
/// 全由它推导，于是上线成功也看不出来。
fn spawn_link_reader(
    mut events: tokio::sync::broadcast::Receiver<can_voice_fsd::session::FsdEvent>,
    slot: Arc<Mutex<Option<can_voice_fsd::session::FsdEvent>>>,
) {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(e) => *slot.lock().expect("link") = Some(e),
                // 跟不上就继续：界面只关心最新那一条。
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => return,
            }
        }
    });
}

/// 听 FSD 那条链路上发生的一切。
///
/// **三张表一起喂**：他机、文字消息、在线席位。前一版只认他机那四条事件，
/// 其余的落在 `Ok(_) => {}` 上，于是飞行员只能发不能收、看不到谁在线。
fn spawn_traffic_reader(
    mut events: tokio::sync::broadcast::Receiver<PilotEvent>,
    table: Arc<Mutex<TrafficTable>>,
    chat: Arc<Mutex<ChatLog>>,
    controllers: Arc<Mutex<ControllerTable>>,
) {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(e) => can_voice_sim::feed::absorb(
                    monotonic(),
                    e,
                    &mut table.lock().expect("traffic"),
                    &mut chat.lock().expect("chat"),
                    &mut controllers.lock().expect("controllers"),
                ),
                // 跟不上就丢了几条；下一条会补上，不必重来。
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            }
        }
    });
}

/// 定期去问别人的机型和配置。
///
/// **只能轮询，没有主动推送。** 不问的话所有他机永远是通用模型、全程关灯、
/// 光杆落地——而那看起来像"模型匹配坏了"，不像"没人问过"。
fn spawn_asking(fsd: PilotHandle, table: Arc<Mutex<TrafficTable>>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(ASK_INTERVAL);
        loop {
            tick.tick().await;
            let (types, configs) = {
                let mut table = table.lock().expect("traffic");
                let now = monotonic();
                (
                    table.missing_plane_info(),
                    table.due_for_config(now, CONFIG_REFRESH),
                )
            };
            for callsign in types {
                fsd.request_plane_info(callsign);
            }
            for callsign in configs {
                fsd.request_config(callsign);
            }
        }
    });
}

/// 把模拟器读到的外形换成答 `$CQ ACC` 用的那一份。
///
/// **只在模拟器真的报了外形时才调**：`Snapshot::animation` 是 `None` 时不发，
/// 而不是发一份全 false 的。一架名字打错了 dataref 的飞机和一架真的收着起落架、
/// 灯全灭的飞机在管制屏上一模一样，只有前者是我们的 bug。
fn to_aircraft_config(a: &can_voice_sim::Animation) -> can_voice_fsd::pilot::AircraftConfig {
    can_voice_fsd::pilot::AircraftConfig {
        gear_down: Some(a.gear_down),
        flaps: Some(a.flaps),
        spoilers: Some(a.spoilers),
        engines_on: Some(a.engines_on),
        taxi_on: Some(a.taxi_on),
        landing_on: Some(a.landing_on),
        beacon_on: Some(a.beacon_on),
        strobe_on: Some(a.strobe_on),
        nav_on: Some(a.nav_on),
    }
}

/// 每 200 ms 把模拟器那一帧喂给 FSD，并让语音订阅跟上 COM1。
fn spawn_pump(fsd: PilotHandle, sim: xplane::Link, voice: Arc<Bridge>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PUMP_INTERVAL);
        let mut current_freq: Option<u32> = None;
        // 外形变了才发：它是本地状态，别人问 `$CQ ACC` 时才用得上，
        // 每秒五次重复同一份没有意义。
        let mut last_config: Option<can_voice_sim::Animation> = None;
        loop {
            tick.tick().await;
            let Some(snapshot) = sim.snapshot() else {
                continue;
            };
            // 别人看我们的起落架和灯，靠的就是这一条。不发的话我们在所有人
            // 的屏幕上永远是收着轮子、灯全灭。
            if let Some(a) = snapshot.animation {
                if last_config != Some(a) {
                    last_config = Some(a);
                    fsd.set_own_config(to_aircraft_config(&a));
                }
            }
            fsd.update_position(to_position(
                &snapshot,
                can_voice_fsd::packet::RATING_OBSERVER,
            ));

            let wanted = voice_frequency(&snapshot);
            if wanted != current_freq {
                let previous = current_freq;
                current_freq = wanted;
                voice.with_stack(|stack| {
                    // 换频率就是把旧的撤掉、新的加上。飞行员端的台面永远只有
                    // 一个频率——COM1 就是那一个。
                    if let Some(old) = previous {
                        stack.remove(old);
                    }
                    if let Some(khz) = wanted {
                        stack.add(khz);
                        stack.set_rx(khz, true);
                        stack.set_tx(khz, true);
                        stack.set_selected(khz);
                    }
                });
            }
        }
    });
}

/// 每 50 ms 往插件推一帧。比位置上报快，插值才有意义。
fn spawn_plugin_feed(
    sim: xplane::Link,
    table: Arc<Mutex<TrafficTable>>,
    csl: Arc<Mutex<ModelSet>>,
    inject: Arc<std::sync::atomic::AtomicBool>,
) {
    tokio::spawn(async move {
        let Ok(socket) = tokio::net::UdpSocket::bind("127.0.0.1:0").await else {
            tracing::warn!("could not open the plugin socket; traffic will not be drawn");
            return;
        };
        let target = format!("{}:{}", bridge::HOST, bridge::PLUGIN_PORT);
        let mut tick = tokio::time::interval(PLUGIN_INTERVAL);
        let mut seq: u16 = 0;
        loop {
            tick.tick().await;
            let now = monotonic();
            let origin = sim.snapshot().map(|s| (s.latitude, s.longitude));
            let entries = {
                let mut table = table.lock().expect("traffic");
                table.prune(now);
                let entries = table.snapshot(now, origin, Some(MAX_TRAFFIC), Some(MAX_RANGE_NM));
                // 匹配还没匹配过的。**放在这里而不是收到机型那一刻**：
                // 只给要画的那几架匹配，一屏之外的不花这个钱。
                let models = csl.lock().expect("csl");
                for e in entries.iter().filter(|e| e.model_dirty) {
                    if let Some((m, level)) = models.match_model(&e.equipment, &e.airline, &e.csl) {
                        tracing::debug!(callsign = %e.callsign, ?level, model = %m.name, "matched");
                        table.set_model(
                            &e.callsign,
                            &e.equipment,
                            &e.airline,
                            &m.path.to_string_lossy(),
                        );
                    }
                }
                // 匹配之后再取一次，这一份才带得上刚填好的模型路径。
                drop(models);
                table.snapshot(now, origin, Some(MAX_TRAFFIC), Some(MAX_RANGE_NM))
            };
            // 关掉注入送的是**一份空表**，不是停掉这条循环：停掉的话已经画出来的
            // 飞机会留在天上不动，而空表让它们按正常的消失路径被撤掉。
            let entries = if inject.load(std::sync::atomic::Ordering::Relaxed) {
                entries
            } else {
                Vec::new()
            };
            let message = serde_json::json!({ "traffic": entries });
            seq = seq.wrapping_add(1);
            for packet in bridge::encode(&message, seq) {
                let _ = socket.send_to(&packet, &target).await;
            }
        }
    });
}

/// 在后台把 CSL 扫进来。几个 GB 的包要几十秒，不能挡着窗口。
fn spawn_csl_load(csl: Arc<Mutex<ModelSet>>) {
    std::thread::spawn(move || {
        let root = csl_root();
        let loaded = can_voice_sim::csl::load(&root);
        if loaded.is_empty() {
            tracing::warn!(
                root = %root.display(),
                "no CSL models found; other aircraft will not be drawn.                  set CAN_XPC_CSL_DIR if the packages live elsewhere"
            );
        }
        *csl.lock().expect("csl") = loaded;
    });
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
        // 上着网就是"正在工作"。
        let busy = app.link.lock().expect("link").is_some();
        (s.skipped_update, busy) };
    let origin = env_or("CAN_API_ORIGIN", "https://api.ceruleanavi.net");
    let Some(latest) = can_voice_update::check(&app.http, &origin, "xpc-for-can", env!("CARGO_PKG_VERSION")).await else {
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
        "xpc-for-can",
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
    can_voice_log::init("xpc-for-can", std::env::args().any(|a| a == "--debug"));

    let app = App::new();
    spawn_csl_load(app.csl.clone());
    tauri::Builder::default()
        .manage(app)
        .invoke_handler(tauri::generate_handler![
            log_file,
            send_log,
            check_update,
            skip_update,
            open_download,
            connect,
            disconnect,
            view,
            send_text,
            ident,
            file_flight_plan,
            settings,
            set_injection,
            set_audio_devices,
            keyboard_ptt_supported,
            ptt_bindings,
            audio_devices,
            set_transmitting,
            set_ptt_bindings,
            ptt_pressed,
            begin_ptt_capture,
            take_captured_binding,
            mouse_ptt_supported,
        ])
        .run(tauri::generate_context!())
        .expect("tauri failed to start");
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_voice_fsd::pilot::XpdrMode;


    /// **界面靠 `link` 判断"上线了没有"**：`App.vue` 里 `online` 就是
    /// `link != null`，`connected` 是 `link === "Online"`。它写死 `None` 的时候，
    /// FSD 上线成功之后登录表单不消失、下线和识别按钮永不出现、
    /// 文字消息框永久禁用——连上了却什么都干不了。
    #[test]
    fn the_view_reports_the_link_state_the_session_last_emitted() {
        use can_voice_fsd::session::{FsdEvent, FsdState, Reason};

        let rt = tokio::runtime::Runtime::new().expect("rt");
        let _guard = rt.enter();
        let app = App::new();
        *app.link.lock().expect("link") = Some(FsdEvent {
            state: FsdState::Online,
            reason: Reason::Online,
        });

        let v = build_view(&app);

        assert_eq!(v.link, Some(FsdState::Online));
        assert_eq!(v.reason, Some(Reason::Online));
    }

    /// 管制员打的字要能到界面上，在线席位也是。
    ///
    /// 这两样此前在事件循环里落在 `Ok(_) => {}` 上，而快照里根本没有装它们的
    /// 字段——症状是飞行员只能发不能收，且不知道谁在线、该叫哪个频率。
    #[test]
    fn the_view_carries_the_messages_and_the_online_controllers() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let _guard = rt.enter();
        let app = App::new();
        can_voice_sim::feed::absorb(
            monotonic(),
            PilotEvent::Text {
                sender: "ZSPD_TWR".into(),
                recipient: "CCA1501".into(),
                message: "pushback approved".into(),
            },
            &mut app.traffic.lock().expect("traffic"),
            &mut app.chat.lock().expect("chat"),
            &mut app.controllers.lock().expect("controllers"),
        );
        can_voice_sim::feed::absorb(
            monotonic(),
            PilotEvent::Controller(Box::new(can_voice_fsd::pilot_client::Controller {
                callsign: "ZSPD_TWR".into(),
                frequency: 118.35,
                facility: 4,
                rating: 5,
                latitude: 31.14,
                longitude: 121.8,
                vis_range: 150,
            })),
            &mut app.traffic.lock().expect("traffic"),
            &mut app.chat.lock().expect("chat"),
            &mut app.controllers.lock().expect("controllers"),
        );

        let v = build_view(&app);

        assert_eq!(v.messages.len(), 1);
        assert_eq!(v.messages[0].text, "pushback approved");
        assert_eq!(v.controllers.len(), 1);
        assert_eq!(v.controllers[0].callsign, "ZSPD_TWR");
    }

    /// 自己发出去的那条也要进记录。
    ///
    /// 只记收到的话，聊天区里只剩对方的半边对话——而"洛杉矶，明白"是接在
    /// 自己刚发的那句后面才读得懂的。
    #[test]
    fn a_message_i_send_is_in_my_own_log() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let _guard = rt.enter();
        let app = App::new();
        app.settings.lock().expect("settings").callsign = "CCA1501".into();

        record_sent(&app, "ZSPD_TWR".into(), "request pushback".into());

        let v = build_view(&app);
        assert_eq!(v.messages.len(), 1);
        assert!(v.messages[0].outbound);
        assert_eq!(v.messages[0].from, "CCA1501");
        assert_eq!(v.messages[0].to, "ZSPD_TWR");
        assert_eq!(v.messages[0].text, "request pushback");
    }

    /// **COM1 电门关着就退订。** 关了电门还在频率上说话，对管制来说是个幽灵。
    #[test]
    fn a_dead_com1_means_no_frequency() {
        let mut s = Snapshot {
            com1: Some(121.8),
            com1_power: false,
            ..Default::default()
        };
        assert_eq!(voice_frequency(&s), None);
        s.com1_power = true;
        assert_eq!(voice_frequency(&s), Some(121_800));
    }

    /// 模拟器初始化过程中 COM1 会短暂读出 0 或者别的怪值。照单订阅会让人落在
    /// 一个谁也不在的频率上，而界面看着一切正常。
    #[test]
    fn a_frequency_outside_the_band_is_refused() {
        let out_of_band = |mhz| {
            voice_frequency(&Snapshot {
                com1: Some(mhz),
                com1_power: true,
                ..Default::default()
            })
        };
        assert_eq!(out_of_band(0.0), None);
        assert_eq!(out_of_band(99.5), None);
        assert_eq!(out_of_band(140.0), None);
        assert_eq!(out_of_band(118.0), Some(118_000));
        assert_eq!(out_of_band(136.975), Some(136_975));
    }

    /// 位置上报原样带上模拟器给的应答机模式和气压修正量——这两项在这一层
    /// 不该被重新判断，[`can_voice_sim`] 已经判过了。
    #[test]
    fn the_position_carries_the_simulator_values_through() {
        let s = Snapshot {
            squawk: 7700,
            xpdr_mode: XpdrMode::Standby,
            pressure_delta: -120,
            altitude: 35_000,
            ..Default::default()
        };
        let p = to_position(&s, 1);
        assert_eq!(p.squawk, 7700);
        assert_eq!(p.mode, XpdrMode::Standby);
        assert_eq!(p.pressure_delta, -120);
        assert_eq!(p.altitude, 35_000);
    }
}
