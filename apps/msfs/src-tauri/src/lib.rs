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
//! # SimConnect 那一半在这个仓库里编译不了
//!
//! CI 跑在 Linux 上、开发机是 macOS，而 `SimConnect.dll` 只有 Windows 有。
//! 纯逻辑那一半（SimVar 表、单位换算、快照）到处都能测；连模拟器那一半
//! **必须在 Windows 上编一次、连一次真实模拟器才算数**。非 Windows 上
//! [`can_voice_sim::msfs::available`] 返回 false，界面会直说。

use can_voice_app::Bridge;
use can_voice_fsd::pilot::{FlightPlan, PilotIdentity, PilotPosition};
use can_voice_fsd::pilot_client::{self, PilotConfig, PilotEvent, PilotHandle};
use can_voice_sim::msfs::{SimConnectSource, SimVarSource};
use can_voice_sim::traffic::{Entry, Sample, TrafficTable};
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

pub struct App {
    voice: Arc<Bridge>,
    sim: SimLink,
    fsd: Mutex<Option<PilotHandle>>,
    traffic: Arc<Mutex<TrafficTable>>,
    http: reqwest::Client,
    ptt: Mutex<Option<can_voice_ptt::PttWatcher>>,
}

impl App {
    pub fn new() -> Self {
        Self {
            voice: Arc::new(Bridge::new()),
            sim: SimLink::spawn(),
            fsd: Mutex::new(None),
            traffic: Arc::new(Mutex::new(TrafficTable::new())),
            http: reqwest::Client::builder()
                .user_agent(concat!("msfs-for-can/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            ptt: Mutex::new(None),
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
        input_device: None,
        output_device: None,
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

    spawn_traffic_reader(fsd.traffic(), app.traffic.clone());
    spawn_asking(fsd.clone(), app.traffic.clone());
    spawn_pump(fsd.clone(), app.sim.clone(), app.voice.clone());
    spawn_ai_injection(app.sim.clone(), app.traffic.clone());
    *app.fsd.lock().expect("fsd") = Some(fsd);
    Ok(())
}

#[tauri::command]
async fn disconnect(app: tauri::State<'_, App>) -> Result<(), String> {
    if let Some(fsd) = app.fsd.lock().expect("fsd").take() {
        fsd.stop();
    }
    app.voice.disconnect().await;
    app.traffic.lock().expect("traffic").prune(f64::MAX);
    Ok(())
}

#[tauri::command]
fn view(app: tauri::State<'_, App>) -> View {
    let sim = app.sim.snapshot();
    let now = monotonic();
    let origin = sim.as_ref().map(|s| (s.latitude, s.longitude));
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
        link: None,
        reason: None,
        voice: Some(app.voice.snapshot()),
    }
}

#[tauri::command]
fn send_text(app: tauri::State<'_, App>, recipient: String, message: String) -> bool {
    match app.fsd.lock().expect("fsd").as_ref() {
        Some(fsd) => {
            fsd.send_text(recipient, message);
            true
        }
        None => false,
    }
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

fn spawn_traffic_reader(
    mut events: tokio::sync::broadcast::Receiver<PilotEvent>,
    table: Arc<Mutex<TrafficTable>>,
) {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(PilotEvent::Traffic(t)) => {
                    let now = monotonic();
                    table.lock().expect("traffic").update_position(
                        &t.callsign,
                        t.squawk,
                        Sample {
                            time: now,
                            latitude: t.latitude,
                            longitude: t.longitude,
                            altitude: f64::from(t.altitude),
                            pitch: t.attitude.pitch,
                            bank: t.attitude.bank,
                            heading: t.attitude.heading,
                            on_ground: t.attitude.on_ground,
                            groundspeed: f64::from(t.groundspeed),
                        },
                    );
                }
                Ok(PilotEvent::TrafficGone(callsign)) => {
                    table.lock().expect("traffic").remove(&callsign);
                }
                Ok(PilotEvent::PlaneInfo {
                    callsign,
                    equipment,
                    airline,
                    ..
                }) => {
                    table.lock().expect("traffic").set_plane_info(
                        &callsign,
                        monotonic(),
                        &equipment,
                        &airline,
                    );
                }
                Ok(PilotEvent::Config { callsign, config }) => {
                    table
                        .lock()
                        .expect("traffic")
                        .set_config(&callsign, monotonic(), *config);
                }
                Ok(_) => {}
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

/// 每 200 ms 把模拟器那一帧喂给 FSD，并让语音订阅跟上 COM1。
fn spawn_pump(fsd: PilotHandle, sim: SimLink, voice: Arc<Bridge>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PUMP_INTERVAL);
        let mut current_freq: Option<u32> = None;
        loop {
            tick.tick().await;
            let Some(snapshot) = sim.snapshot() else {
                continue;
            };
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

/// 每 50 ms 把他机表对到模拟器的 AI 机上。
///
/// **账在 [`can_voice_sim::inject`] 里，有测试**；这里只是把算出来的动作发下去。
/// 真正调 SimConnect 的那几下是 Windows-only 的。
fn spawn_ai_injection(sim: SimLink, table: Arc<Mutex<TrafficTable>>) {
    tokio::spawn(async move {
        let mut injector = can_voice_sim::inject::Injector::new();
        let mut tick = tokio::time::interval(INJECT_INTERVAL);
        loop {
            tick.tick().await;
            let now = monotonic();
            let origin = sim.snapshot().map(|s| (s.latitude, s.longitude));
            let entries = {
                let mut table = table.lock().expect("traffic");
                table.prune(now);
                table.snapshot(now, origin, Some(MAX_TRAFFIC), Some(MAX_RANGE_NM))
            };
            for action in injector.reconcile(&entries) {
                tracing::debug!(?action, "ai");
                // TODO(Windows)：接上 SimConnect 的 AICreateNonATCAircraft /
                // SetDataOnSimObject / AIRemoveObject。账已经算好了，缺的只是
                // 那三个调用，而它们在这台机器上编译不了。
            }
        }
    });
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(env_or("RUST_LOG", "info"))
        .init();

    tauri::Builder::default()
        .manage(App::new())
        .invoke_handler(tauri::generate_handler![
            connect,
            disconnect,
            view,
            send_text,
            ident,
            file_flight_plan,
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
