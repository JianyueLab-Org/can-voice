//! `msfs-for-can` —— MSFS 飞行客户端的 Rust 侧。
//!
//! 和 `xpc-for-can` 是同一套接线，换掉两处：
//!
//! ```text
//! 模拟器链路   SimConnect（本机，一问一答）  ← X-Plane 是 UDP 多播 + 订阅
//! 他机呈现     SimConnect 注入 AI 机          ← X-Plane 是 XPPython3 插件
//! ```
//!
//! 别的——FSD 飞行员会话、他机表与插值、语音跟着 COM1、PTT——都是同一份代码，
//! 住在 `can-voice-fsd` 和 `can-voice-sim` 里。`can-audio` 那边这两支客户端是
//! 两份近似副本，改一处要改两遍。
//!
//! # SimConnect 那一半只在 Windows 上编译，但确实编
//!
//! 开发机是 macOS，而 `SimConnect.dll` 只有 Windows 有。DLL 是**运行时**加载的
//! （见 `can_voice_sim::msfs`），所以编译不需要那份不能随仓库分发的 SDK ——
//! **CI 的 Windows job 会真的编这一段**。
//!
//! 但"编得过"不等于"对"：函数签名、结构体布局、常量取值这些只有连上一次真实
//! 模拟器才验得了。纯逻辑那一半（SimVar 表、单位换算、快照、注入账本、机模
//! 候选表）到处都能测，测试也都在。非 Windows 上
//! [`can_voice_sim::msfs::available`] 返回 false，界面会直说。

use can_voice_app::Bridge;
use can_voice_fsd::pilot::{FlightPlan, PilotIdentity, PilotPosition};
use can_voice_fsd::pilot_client::{self, PilotConfig, PilotEvent, PilotHandle};
use can_voice_sim::msfs::{SimConnectSource, SimConnectTraffic, SimVarSource, TrafficSink};
use can_voice_sim::chat::{ChatLog, ChatMessage};
use can_voice_sim::controllers::{ControllerEntry, ControllerTable};
use can_voice_sim::traffic::{Entry, TrafficTable};
use can_voice_sim::Snapshot;
use can_voice_token::TokenSource;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// SimConnect 是一问一答，自己拿一个线程去轮。
///
/// **和 X-Plane 那条不一样**：那边是订阅、模拟器往这边推；这边没有推送，也
/// 没有发现过程——SimConnect 只连本机，模拟器没开就是打不开。
#[derive(Debug, Clone)]
pub struct SimLink {
    state: Arc<Mutex<SimState>>,
}

#[derive(Debug, Default)]
struct SimState {
    connected: bool,
    snapshot: Option<Snapshot>,
    /// 打不开时的原因，界面要说得出来。
    problem: Option<String>,
}

impl SimLink {
    pub fn spawn() -> Self {
        let link = Self {
            state: Arc::new(Mutex::new(SimState::default())),
        };
        let state = link.state.clone();
        // 专门一个线程而不是 tokio 任务：SimConnect 的调用是阻塞的 C 函数，
        // 放在异步运行时里会把整个执行器卡住。
        std::thread::spawn(move || sim_loop(state));
        link
    }

    pub fn connected(&self) -> bool {
        self.state.lock().expect("sim").connected
    }

    pub fn problem(&self) -> Option<String> {
        self.state.lock().expect("sim").problem.clone()
    }

    pub fn snapshot(&self) -> Option<Snapshot> {
        self.state.lock().expect("sim").snapshot.clone()
    }
}

fn sim_loop(state: Arc<Mutex<SimState>>) {
    let mut raw: std::collections::HashMap<&'static str, f64> = std::collections::HashMap::new();
    loop {
        let mut source = SimConnectSource::default();
        if let Err(e) = source.open() {
            {
                let mut state = state.lock().expect("sim");
                state.connected = false;
                state.problem = Some(e);
            }
            // 模拟器没开是常态，不是错误。隔几秒再试一次。
            std::thread::sleep(Duration::from_secs(5));
            continue;
        }
        state.lock().expect("sim").problem = None;
        loop {
            match source.poll() {
                Ok(values) => {
                    if !values.is_empty() {
                        raw.extend(values);
                        let mut state = state.lock().expect("sim");
                        state.connected = true;
                        state.snapshot = can_voice_sim::msfs::snapshot(&raw);
                    }
                }
                Err(e) => {
                    let mut state = state.lock().expect("sim");
                    state.connected = false;
                    state.problem = Some(e);
                    break;
                }
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        source.close();
    }
}

/// 轮 SimConnect 的节奏。比位置上报快一点，插值才有东西可插。
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// 位置上报的节奏。和 [`pilot_client::POSITION_INTERVAL`] 同一个值——
/// 这里只是把模拟器那一帧喂过去，真正决定发不发的是那边。
const PUMP_INTERVAL: Duration = Duration::from_millis(200);
/// 同时注入多少架。MSFS 的 AI 机再多会明显掉帧。
const MAX_TRAFFIC: usize = 64;
/// 超出这个距离的不注入——看不见的飞机白占 AI 机的位置。
const MAX_RANGE_NM: f64 = 200.0;
/// 对一次账的节奏。
const INJECT_INTERVAL: Duration = Duration::from_millis(500);
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
    /// 收到管制消息时播放提示音。**默认开**：飞行员盯着的是窗外，
    /// 消息区多出来的那一行谁也看不见。
    #[serde(default = "yes")]
    pub message_sound: bool,
    /// 频率上的**每一条**都提示。默认关——默认只有点到你呼号的才响。
    #[serde(default)]
    pub message_sound_all: bool,
    /// 提示音音量，百分比，和上面那两根滑条同一个量纲（0–200）。
    ///
    /// **默认值要用具名函数**，不能写裸的 `#[serde(default)]`：老的设置文件里
    /// 没有这一项，反序列化拿到 0 等于一次升级把所有人的提示音静音。而 0 本身
    /// 是合法取值（用户明确要静音），事后分不出是"没设过"还是"设成了 0"。
    #[serde(default = "default_alert_volume")]
    pub message_sound_volume: u32,
    /// MSFS 的包目录。空的表示自己去找（先读 `UserCfg.opt`，再退到猜路径）。
    #[serde(default)]
    pub packages_dir: String,
    /// 用户说过"这一版不用再问我"的那个版本号。**跳过的是那一个版本，
    /// 不是从此闭嘴**——下一版照样提示。
    #[serde(default)]
    pub skipped_update: String,
}

fn yes() -> bool {
    true
}

fn default_alert_volume() -> u32 {
    100
}

/// 和麦克风那两根滑条同一个量纲。**0 是合法的**（静音），所以只夹上界。
fn clamp_alert_volume(percent: u32) -> u32 {
    percent.min(200)
}

/// 提示音要用到的那几个值。
///
/// 和 `inject` / `traffic_range` 一样做成原子量：收包那条路径不该为了三个开关
/// 去抢设置的锁。
#[derive(Default)]
struct Chimer {
    enabled: std::sync::atomic::AtomicBool,
    every: std::sync::atomic::AtomicBool,
    volume: std::sync::atomic::AtomicU32,
    device: Mutex<Option<String>>,
    /// 正在连着的那条 FSD 的呼号。**设置里存的那份可能是上一次连的**，
    /// 拿它去判"有没有点到我"会在刚换过呼号的那一次响错。
    callsign: Mutex<String>,
    gate: Mutex<can_voice_chime::Gate>,
}

impl Chimer {
    fn new(s: &Settings) -> Self {
        use std::sync::atomic::{AtomicBool, AtomicU32};
        Self {
            enabled: AtomicBool::new(s.message_sound),
            every: AtomicBool::new(s.message_sound_all),
            volume: AtomicU32::new(clamp_alert_volume(s.message_sound_volume)),
            device: Mutex::new(s.output_device.clone()),
            callsign: Mutex::new(String::new()),
            gate: Mutex::new(can_voice_chime::Gate::new()),
        }
    }

    fn set_callsign(&self, callsign: &str) {
        if let Ok(mut c) = self.callsign.lock() {
            callsign.clone_into(&mut c);
        }
    }

    /// 来了一条文字消息。
    fn on_text(self: &Arc<Self>, sender: &str, recipient: &str, body: &str) {
        use std::sync::atomic::Ordering::Relaxed;
        let callsign = match self.callsign.lock() {
            Ok(c) => c.clone(),
            Err(p) => p.into_inner().clone(),
        };
        if can_voice_sim::chat::wants_alert(
            &callsign,
            sender,
            recipient,
            body,
            self.every.load(Relaxed),
        ) {
            self.fire(false);
        }
    }

    /// 试听：用户自己点的，不看开关也不受最短间隔限制。
    fn preview(self: &Arc<Self>) {
        self.fire(true);
    }

    fn fire(self: &Arc<Self>, force: bool) {
        use std::sync::atomic::Ordering::Relaxed;
        let volume = self.volume.load(Relaxed);
        let enabled = self.enabled.load(Relaxed);
        {
            let mut gate = match self.gate.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            if !gate.allow(monotonic(), force, enabled, volume) {
                return;
            }
        }
        let device = match self.device.lock() {
            Ok(d) => d.clone(),
            Err(p) => p.into_inner().clone(),
        };
        let me = Arc::clone(self);
        // **自己的 OS 线程，从开流到放完都在上面**：`cpal::Stream` 在 macOS 上是
        // `!Send`。放不出来也绝不能影响收消息本身，所以这里不等它、不看返回值。
        std::thread::spawn(move || {
            can_voice_chime::play_blocking(device.as_deref(), volume);
            if let Ok(mut g) = me.gate.lock() {
                g.finished();
            }
        });
    }
}

pub struct App {
    voice: Arc<Bridge>,
    sim: SimLink,
    fsd: Mutex<Option<PilotHandle>>,
    /// FSD 链路最近一条事件。**界面靠它判断"上线了没有"**。
    link: Arc<Mutex<Option<can_voice_fsd::session::FsdEvent>>>,
    store: can_voice_settings::Store,
    settings: Mutex<Settings>,
    /// 注入开关。注入的那几条循环每一拍读它。
    inject: Arc<std::sync::atomic::AtomicBool>,
    traffic: Arc<Mutex<TrafficTable>>,
    /// 收发过的文字消息。**攒在这里而不是靠事件推**：窗口重开之前
    /// 管制员说过的话，靠事件流是收不到的。
    chat: Arc<Mutex<ChatLog>>,
    chime: Arc<Chimer>,
    /// 在线管制席位。
    controllers: Arc<Mutex<ControllerTable>>,
    http: reqwest::Client,
    ptt: Mutex<Option<can_voice_ptt::PttWatcher>>,
    /// 本机机库。**扫出来的是本机的表**，不进内置表——内置表只收第一方，
    /// 因为只有它们的标题在不同机器上是同一个字符串。
    hangar: Arc<Mutex<can_voice_sim::msfs_hangar::Hangar>>,
    hangar_loading: Arc<std::sync::atomic::AtomicBool>,
}

impl App {
    pub fn new() -> Self {
        let store = can_voice_settings::Store::for_product("msfs-for-can");
        let settings: Settings = store.load();
        // 在 settings 被移进结构体之前建好。
        let chime = Arc::new(Chimer::new(&settings));
        Self {
            voice: Arc::new(Bridge::new()),
            sim: SimLink::spawn(),
            fsd: Mutex::new(None),
            link: Arc::new(Mutex::new(None)),
            store,
            inject: Arc::new(std::sync::atomic::AtomicBool::new(settings.inject)),
            settings: Mutex::new(settings),
            traffic: Arc::new(Mutex::new(TrafficTable::new())),
            chat: Arc::new(Mutex::new(ChatLog::default())),
            chime,
            hangar: Arc::new(Mutex::new(can_voice_sim::msfs_hangar::Hangar::default())),
            hangar_loading: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            controllers: Arc::new(Mutex::new(ControllerTable::default())),
            http: reqwest::Client::builder()
                .user_agent(concat!("msfs-for-can/", env!("CARGO_PKG_VERSION")))
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

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// 界面读的一份快照。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct View {
    /// MSFS 连上了没有。
    pub sim_connected: bool,
    /// 连不上时的原因。**非 Windows 上就是"这个系统没有 SimConnect"**，
    /// 界面要直说，而不是让人对着一个永远灰着的灯猜。
    pub sim_problem: Option<String>,
    pub sim: Option<Snapshot>,
    pub link: Option<can_voice_fsd::session::FsdState>,
    pub reason: Option<can_voice_fsd::session::Reason>,
    pub traffic: Vec<Entry>,
    pub voice: Option<can_voice_app::Snapshot>,
    /// 收发过的文字消息，最旧的在前。
    pub messages: Vec<ChatMessage>,
    /// 在线管制席位，按呼号排序。
    pub controllers: Vec<ControllerEntry>,
    /// 本机机库扫的结果。
    pub hangar: HangarView,
}

/// 机库扫到了什么，给界面看的。
///
/// **三个数都要给**：涂装几百个而机型是 0，说明读到的全是附加件那类没有机型码
/// 的配置——那和"目录指错了"是两回事，光看一个总数分不出来。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct HangarView {
    /// 正在扫。界面靠它区分"还在扫"和"扫完了，没有"。
    pub loading: bool,
    /// 读了多少个 `aircraft.cfg`。
    pub files: usize,
    /// 认出多少个涂装。
    pub liveries: usize,
    /// 其中有多少种机型码。
    pub types: usize,
    /// 正在用的包目录；空的表示自己去找。
    pub dir: String,
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
        client_id: concat!("msfs-for-can/", env!("CARGO_PKG_VERSION")).into(),
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
            can_voice_fsd::pilot::SIMULATOR_MSFS,
            concat!("MSFS for CAN ", env!("CARGO_PKG_VERSION")),
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
        app.chime.clone(),
    );
    spawn_asking(fsd.clone(), app.traffic.clone());
    spawn_pump(fsd.clone(), app.sim.clone(), app.voice.clone());
    spawn_ai_injection(
        app.sim.clone(),
        app.traffic.clone(),
        app.inject.clone(),
        app.hangar.clone(),
    );
    *app.fsd.lock().expect("fsd") = Some(fsd);
    // 判"有没有点到我"要用正在连着的这个呼号，不是设置里存的那个。
    app.chime.set_callsign(&callsign);
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
    app.chime.set_callsign("");
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
/// 改完**立刻重扫**。不重扫的话，填对了路径的人做的这件事看起来毫无反应。
#[tauri::command]
fn set_packages_dir(app: tauri::State<'_, App>, dir: String) {
    app.update_settings(|s| s.packages_dir.clone_from(&dir));
    spawn_hangar_scan(
        app.hangar.clone(),
        app.hangar_loading.clone(),
        hangar_roots(&dir),
    );
}

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
    // 提示音也走这块设备——它存在的全部理由就是不响在系统默认输出上。
    if let Ok(mut d) = app.chime.device.lock() {
        d.clone_from(&output);
    }
    app.update_settings(|s| {
        s.input_device = input;
        s.output_device = output;
    });
}

#[tauri::command]
fn set_message_sound(app: tauri::State<'_, App>, on: bool) {
    app.chime
        .enabled
        .store(on, std::sync::atomic::Ordering::Relaxed);
    app.update_settings(|s| s.message_sound = on);
}

#[tauri::command]
fn set_message_sound_all(app: tauri::State<'_, App>, on: bool) {
    app.chime
        .every
        .store(on, std::sync::atomic::Ordering::Relaxed);
    app.update_settings(|s| s.message_sound_all = on);
}

/// 返回夹过的那个数：界面上填 9999 之后该看到 200。
#[tauri::command]
fn set_message_sound_volume(app: tauri::State<'_, App>, percent: u32) -> u32 {
    let percent = clamp_alert_volume(percent);
    app.chime
        .volume
        .store(percent, std::sync::atomic::Ordering::Relaxed);
    app.update_settings(|s| s.message_sound_volume = percent);
    percent
}

/// 试听。**用当前选着的设备和音量**，不是已经存下来的那份——用户多半正是
/// 刚换了耳机才来点这一下的。点了没声音就说明设备选错了，这正是这个按钮的意义。
#[tauri::command]
fn preview_chime(app: tauri::State<'_, App>) {
    app.chime.preview();
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
    // **机库锁只拿一次，拿完就放。** 在下面那个结构体字面量里连写三次
    // `app.hangar.lock()` 的话，前一次的守卫是一个临时值，要活到整条语句结束——
    // 于是第二次 `lock()` 等的正是自己手里那把锁。std 的 Mutex 不可重入，
    // 表现是界面第一次轮询 `view` 就永远不回，整个客户端卡死。
    let hangar = {
        let h = app.hangar.lock().expect("hangar");
        HangarView {
            loading: app
                .hangar_loading
                .load(std::sync::atomic::Ordering::Relaxed),
            files: h.files,
            liveries: h.liveries,
            types: h.by_icao.len(),
            dir: app.settings_snapshot().packages_dir,
        }
    };
    View {
        sim_connected: app.sim.connected(),
        sim_problem: app.sim.problem(),
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
        hangar,
        controllers: app
            .controllers
            .lock()
            .expect("controllers")
            .snapshot(now, origin),
    }
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
    chime: Arc<Chimer>,
) {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(e) => {
                    // 提示音在 absorb 之前判：那一步会把事件吃掉。
                    if let PilotEvent::Text {
                        sender,
                        recipient,
                        message,
                    } = &e
                    {
                        chime.on_text(sender, recipient, message);
                    }
                    can_voice_sim::feed::absorb(
                        monotonic(),
                        e,
                        &mut table.lock().expect("traffic"),
                        &mut chat.lock().expect("chat"),
                        &mut controllers.lock().expect("controllers"),
                    );
                }
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
fn spawn_pump(fsd: PilotHandle, sim: SimLink, voice: Arc<Bridge>) {
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

/// 用户自己那张机模表的位置。没有就用内置的。
/// 要扫哪几个目录：用户指定的优先，否则自己去找。
fn hangar_roots(dir: &str) -> Vec<std::path::PathBuf> {
    let dir = dir.trim();
    if !dir.is_empty() {
        return vec![std::path::PathBuf::from(dir)];
    }
    can_voice_sim::msfs_hangar::default_roots()
}

/// 后台扫机库。
///
/// **放后台**：社区包多的话上万个文件，扫一遍要几秒到几十秒，放在启动路径上会让
/// 窗口迟迟打不开，而用户看到的是程序卡死。扫完（哪怕一个也没扫到）都要把
/// `loading` 放下来：界面靠它区分"还在扫"和"扫完了，没有"。
fn spawn_hangar_scan(
    hangar: Arc<Mutex<can_voice_sim::msfs_hangar::Hangar>>,
    loading: Arc<std::sync::atomic::AtomicBool>,
    roots: Vec<std::path::PathBuf>,
) {
    loading.store(true, std::sync::atomic::Ordering::Relaxed);
    std::thread::spawn(move || {
        let found = can_voice_sim::msfs_hangar::scan(&roots);
        tracing::info!(
            files = found.files,
            liveries = found.liveries,
            types = found.by_icao.len(),
            "scanned the local hangar"
        );
        if found.by_icao.is_empty() {
            tracing::warn!(
                roots = ?roots,
                "no aircraft with a type code were found; set the packages directory in settings"
            );
        }
        *hangar.lock().expect("hangar") = found;
        loading.store(false, std::sync::atomic::Ordering::Relaxed);
    });
}

fn titles_path() -> std::path::PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
        .unwrap_or_default();
    base.join("msfs-for-can").join("titles.json")
}

/// 把他机表对到模拟器的 AI 机上。
///
/// **账在 [`can_voice_sim::inject`] 里、机模候选在
/// [`can_voice_sim::msfs_models`] 里，两边都有测试**；这里只负责把算出来的
/// 动作发下去，再把模拟器的回音喂回账本。
///
/// # 为什么是一条专门的线程，而且是第二条 SimConnect 连接
///
/// SimConnect 的调用是阻塞的 C 函数，放在 tokio 任务里会把执行器卡住——
/// 读自机那条线程为此存在，这条同理。而 handle 不是线程安全的，两条线程
/// 共用一个就得加锁，等于把注入和轮询串起来。SimConnect 本来就允许一个进程
/// 开多条连接，开两条更简单。
fn spawn_ai_injection(
    sim: SimLink,
    table: Arc<Mutex<TrafficTable>>,
    inject: Arc<std::sync::atomic::AtomicBool>,
    hangar: Arc<Mutex<can_voice_sim::msfs_hangar::Hangar>>,
) {
    // 不在 Windows 上就没有 SimConnect，起个线程每 5 秒失败一次没有意义。
    if !can_voice_sim::msfs::available() {
        return;
    }
    std::thread::spawn(move || {
        // 手写的 `titles.json` 在前，本机扫出来的在后：前者是用户明确说过的，
        // 后者是推断的。两者都排在内置表前面，而内置表仍然兜底。
        let mut overrides = can_voice_sim::msfs_models::load_overrides(&titles_path());
        {
            let found = hangar.lock().expect("hangar");
            overrides.merge(can_voice_sim::msfs_models::Overrides::from_pairs(
                found.by_icao.clone(),
            ));
        }
        if !overrides.is_empty() {
            tracing::info!(types = overrides.len(), "loaded model title overrides");
        }
        loop {
            let mut sink = SimConnectTraffic::default();
            if let Err(e) = sink.open() {
                tracing::debug!(error = %e, "traffic link");
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
            tracing::info!("traffic link open");
            let mut injector = can_voice_sim::inject::Injector::new();
            loop {
                // 先收回音：建成的登记 id，失败的记一笔好换下一个机模。
                for (callsign, result) in sink.pump() {
                    match result {
                        Some(object_id) => injector.created(&callsign, object_id),
                        None => {
                            injector.failed(&callsign);
                            tracing::debug!(%callsign, attempt = injector.attempt(&callsign),
                                "could not create; trying the next model");
                        }
                    }
                }

                let now = monotonic();
                // **自机位置是距离过滤的前提。** 不给的话 MAX_RANGE_NM 不起
                // 作用，两百海里外的飞机会白占 AI 机的名额。
                let origin = sim.snapshot().map(|s| (s.latitude, s.longitude));
                let entries = {
                    let mut table = table.lock().expect("traffic");
                    table.prune(now);
                    table.snapshot(now, origin, Some(MAX_TRAFFIC), Some(MAX_RANGE_NM))
                };
                // 关掉注入喂的是**一份空表**，不是停掉这条循环：停掉的话已经建出来
                // 的 AI 机会留在天上不动，而空表让 reconcile 把它们撤掉。
                let entries = if inject.load(std::sync::atomic::Ordering::Relaxed) {
                    entries
                } else {
                    Vec::new()
                };

                // **机型码在这里换成机模标题**，因为 SimConnect 要的是后者。
                // 第 n 次重试用第 n 个候选；候选用完就放弃这架，否则它会每
                // INJECT_INTERVAL 重试一次，永远。
                let mut resolved = Vec::new();
                let mut give_up = Vec::new();
                for action in injector.reconcile(&entries) {
                    match action {
                        can_voice_sim::inject::Action::Create {
                            callsign,
                            equipment,
                            entry,
                        } => {
                            let list =
                                can_voice_sim::msfs_models::candidates(&equipment, &overrides);
                            match list.get(injector.attempt(&callsign) as usize) {
                                Some(title) => {
                                    resolved.push(can_voice_sim::inject::Action::Create {
                                        callsign,
                                        equipment: title.clone(),
                                        entry,
                                    })
                                }
                                None => {
                                    tracing::warn!(%callsign, %equipment,
                                        "no model could be created for this type; giving up");
                                    give_up.push((callsign, equipment));
                                }
                            }
                        }
                        other => resolved.push(other),
                    }
                }
                for (callsign, equipment) in give_up {
                    injector.give_up(&callsign, &equipment);
                }

                if let Err(e) = sink.apply(&resolved) {
                    tracing::warn!(error = %e, "traffic link lost");
                    break;
                }
                std::thread::sleep(INJECT_INTERVAL);
            }
            sink.close();
        }
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
    let Some(latest) = can_voice_update::check(&app.http, &origin, "msfs-for-can", env!("CARGO_PKG_VERSION")).await else {
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
        "msfs-for-can",
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
    can_voice_log::init("msfs-for-can", std::env::args().any(|a| a == "--debug"));

    let app = App::new();
    // 启动就扫一遍。慢，所以在后台；界面上有 loading。
    spawn_hangar_scan(
        app.hangar.clone(),
        app.hangar_loading.clone(),
        hangar_roots(&app.settings_snapshot().packages_dir),
    );
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
            set_packages_dir,
            set_audio_devices,
            set_message_sound,
            set_message_sound_all,
            set_message_sound_volume,
            preview_chime,
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
    use can_voice_fsd::pilot::XpdrMode;

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
