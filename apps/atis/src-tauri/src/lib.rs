//! `atis-for-can` —— 通播制作客户端的 Rust 侧。
//!
//! # 这一支**只做稿子，不出声**
//!
//! 它连 FSD、把席位挂上网、答飞行员的通播查询、按报文变化推进情报字母。
//! 声音归服务端机队（`can-voice-atis` 那个可执行文件）。
//!
//! **这不是一条已经拍板的设计**，代价写在 `can_voice_atis` 的模块头上：机队念的
//! 是电码原文的读法，而这里算出来的 `voice_en` / `voice_zh` 是真正给人听的稿子。
//! 两者差得不小。渲染照样算、照样回给前端，所以哪天改成本地出声都不必重写。
//!
//! # 全部逻辑在 `can-voice-atis` 和 `can-voice-fsd` 里
//!
//! 这一层只有 Tauri 命令、一个状态容器，和每个在播席位那条盯报文的循环。

use can_voice_atis::metar::Metar;
use can_voice_atis::profile::{Preset, Profile, ProfileSet, Station, DEFAULT_PROFILE_PATH};
use can_voice_atis::script::{self, Rendered};
use can_voice_fsd::client::{self, Config, FsdEvent, FsdHandle, FsdState, Reason};
use can_voice_fsd::packet::{self, Identity, Position, FACILITY_ATIS, RATING_OBSERVER};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 多久问一次报文。
///
/// METAR 半小时一份（特报不定时），五分钟一问既不会漏掉特报，也不会把服务端
/// 的气象缓存问穿。
const METAR_POLL: Duration = Duration::from_secs(300);
const METAR_TIMEOUT: Duration = Duration::from_secs(20);

/// 一个在播席位此刻的样子。前端读它。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Live {
    pub state: Option<FsdState>,
    pub reason: Option<Reason>,
    pub letter: char,
    pub preset: String,
    /// 最近一次拿到的报文原文。空的表示还没拿到。
    pub metar: String,
    #[serde(flatten)]
    pub rendered: Rendered,
}

struct Running {
    fsd: FsdHandle,
    live: Arc<Mutex<Live>>,
    task: tokio::task::JoinHandle<()>,
    /// 按"现在就更新"时敲它一下，盯报文那条循环立刻去问一次。
    wake: Arc<tokio::sync::Notify>,
}

pub struct App {
    profiles: Mutex<ProfileSet>,
    running: Mutex<HashMap<String, Running>>,
}

impl App {
    pub fn new() -> Self {
        Self {
            profiles: Mutex::new(ProfileSet::load(profile_path())),
            running: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// 配置文件放在系统给的配置目录里，**不是当前目录**。
///
/// Python 版用的是相对路径，所以客户端必须在自己那个目录里跑；一个从开始菜单
/// 启动的程序当前目录是什么全看是谁启动的，于是配置会写到一个谁也找不到的地方。
fn profile_path() -> std::path::PathBuf {
    match dirs_config() {
        Some(dir) => {
            let _ = std::fs::create_dir_all(&dir);
            dir.join(DEFAULT_PROFILE_PATH)
        }
        None => std::path::PathBuf::from(DEFAULT_PROFILE_PATH),
    }
}

fn dirs_config() -> Option<std::path::PathBuf> {
    // 不引一个只为这一件事的依赖。三个平台各自的老地方。
    if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| {
            std::path::PathBuf::from(h)
                .join("Library/Application Support/net.ceruleanavi.atis-for-can")
        })
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA")
            .map(|a| std::path::PathBuf::from(a).join("net.ceruleanavi.atis-for-can"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
            })
            .map(|c| c.join("atis-for-can"))
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

// ——— 配置：读 ———

#[derive(serde::Serialize)]
pub struct Profiles {
    names: Vec<String>,
    active: String,
}

#[tauri::command]
fn profiles(app: tauri::State<'_, App>) -> Profiles {
    let mut set = app.profiles.lock().expect("profiles");
    set.active();
    Profiles {
        names: set.names(),
        active: set.active_name().to_string(),
    }
}

#[tauri::command]
fn stations(app: tauri::State<'_, App>) -> Vec<Station> {
    app.profiles
        .lock()
        .expect("profiles")
        .active()
        .stations
        .clone()
}

// ——— 配置：写 ———
//
// 每一次写都落盘。一份改了却没存的配置，在下一次崩溃或者关机时就没了，
// 而用户以为它存着——比没有这个功能更糟。

fn save(set: &ProfileSet) {
    if let Err(e) = set.save() {
        tracing::warn!(error = %e, "could not save the profile file");
    }
}

#[tauri::command]
fn add_profile(app: tauri::State<'_, App>, name: String) -> Result<(), String> {
    let mut set = app.profiles.lock().expect("profiles");
    set.add(&name).map_err(|e| e.to_string())?;
    save(&set);
    Ok(())
}

#[tauri::command]
fn rename_profile(app: tauri::State<'_, App>, old: String, new: String) -> Result<(), String> {
    let mut set = app.profiles.lock().expect("profiles");
    set.rename(&old, &new).map_err(|e| e.to_string())?;
    save(&set);
    Ok(())
}

#[tauri::command]
fn remove_profile(app: tauri::State<'_, App>, name: String) -> Result<bool, String> {
    let mut set = app.profiles.lock().expect("profiles");
    let gone = set.remove(&name).map_err(|e| e.to_string())?;
    save(&set);
    Ok(gone)
}

#[tauri::command]
fn select_profile(app: tauri::State<'_, App>, name: String) -> bool {
    let mut set = app.profiles.lock().expect("profiles");
    let ok = set.select(&name);
    if ok {
        save(&set);
    }
    ok
}

#[tauri::command]
fn add_station(app: tauri::State<'_, App>, identifier: String) -> Result<Station, String> {
    let station = Station::new(&identifier);
    // 呼号不合规就**在这里**说，而不是等连上去被服务端拒。
    packet::check_atis_callsign(&station.callsign()).map_err(|e| e.to_string())?;
    let mut set = app.profiles.lock().expect("profiles");
    set.active()
        .add(station.clone())
        .map_err(|e| e.to_string())?;
    save(&set);
    Ok(station)
}

#[tauri::command]
fn remove_station(app: tauri::State<'_, App>, callsign: String) -> bool {
    let mut set = app.profiles.lock().expect("profiles");
    let gone = set.active().remove(&callsign);
    if gone {
        save(&set);
    }
    gone
}

/// 整席位覆盖。前端编辑的是一份完整的 [`Station`]，换掉比逐字段发命令简单，
/// 也不会出现"改了一半"的中间状态。
#[tauri::command]
fn save_station(
    app: tauri::State<'_, App>,
    callsign: String,
    mut station: Station,
) -> Result<(), String> {
    station.normalise();
    packet::check_atis_callsign(&station.callsign()).map_err(|e| e.to_string())?;
    let mut set = app.profiles.lock().expect("profiles");
    let profile: &mut Profile = set.active();
    // 换了呼号就等于换了一个席位，重名要挡住。
    if station.callsign() != callsign && profile.get(&station.callsign()).is_some() {
        return Err(format!("{} already exists", station.callsign()));
    }
    profile.remove(&callsign);
    profile.add(station).map_err(|e| e.to_string())?;
    save(&set);
    Ok(())
}

// ——— 渲染预览 ———

/// 拿一份报文渲染一次，**不连网也能看**。
///
/// 配模板的时候要能立刻看到结果；要求先连上 FSD 才能预览，会让人对着一份猜出来
/// 的稿子调格式。
#[tauri::command]
fn preview(
    app: tauri::State<'_, App>,
    callsign: String,
    preset: String,
    metar: String,
    letter: String,
) -> Result<Rendered, String> {
    let mut set = app.profiles.lock().expect("profiles");
    let station = set
        .active()
        .get(&callsign)
        .ok_or_else(|| format!("no station {callsign}"))?
        .clone();
    let chosen: Preset = station
        .preset(&preset)
        .cloned()
        .ok_or_else(|| "this station has no presets".to_string())?;
    let letter = letter.chars().next().unwrap_or(station.letter);
    Ok(script::render(
        &station,
        &chosen,
        &Metar::parse(&metar),
        letter,
    ))
}

// ——— 上线 / 下线 ———

#[tauri::command]
fn start(
    app: tauri::State<'_, App>,
    callsign: String,
    preset: String,
    cid: String,
    password: String,
) -> Result<(), String> {
    let station = {
        let mut set = app.profiles.lock().expect("profiles");
        set.active()
            .get(&callsign)
            .ok_or_else(|| format!("no station {callsign}"))?
            .clone()
    };
    let chosen = station
        .preset(&preset)
        .cloned()
        .ok_or_else(|| "this station has no presets".to_string())?;
    // 频率在这里就要能解析。等连上去才发现的话，席位已经挂在网上了，
    // 而位置包会一直发不出去。
    station
        .frequency_khz()
        .ok_or_else(|| format!("{} is not a frequency", station.frequency))?;

    let mut running = app.running.lock().expect("running");
    if running.contains_key(&callsign) {
        return Err(format!("{callsign} is already on the air"));
    }

    let config = Config {
        host: env_or("CAN_FSD_HOST", "fsd.ceruleanavi.net"),
        port: env_or("CAN_FSD_PORT", "6809")
            .parse()
            .unwrap_or(packet::DEFAULT_PORT),
        identity: Identity::new(
            &station.callsign(),
            &cid,
            &password,
            // 真实姓名那一格放机场名，在线列表里比一个 "ATIS" 有用。
            if station.name.is_empty() {
                &station.identifier
            } else {
                &station.name
            },
            RATING_OBSERVER,
        ),
        position: Position {
            frequency: station.frequency.clone(),
            facility: FACILITY_ATIS,
            vis_range: 50,
            rating: RATING_OBSERVER,
            latitude: station.latitude,
            longitude: station.longitude,
        },
        atis_lines: Vec::new(),
        reconnect_limit: client::RECONNECT_LIMIT,
    };

    let live = Arc::new(Mutex::new(Live {
        letter: station.letter,
        preset: chosen.name.clone(),
        ..Default::default()
    }));
    let wake = Arc::new(tokio::sync::Notify::new());
    let fsd = client::connect(config);
    let task = tokio::spawn(watch(
        station,
        chosen,
        fsd.clone(),
        live.clone(),
        wake.clone(),
    ));
    running.insert(
        callsign,
        Running {
            fsd,
            live,
            task,
            wake,
        },
    );
    Ok(())
}

#[tauri::command]
fn stop(app: tauri::State<'_, App>, callsign: String) {
    if let Some(r) = app.running.lock().expect("running").remove(&callsign) {
        r.fsd.stop();
        r.task.abort();
    }
}

#[tauri::command]
fn live(app: tauri::State<'_, App>) -> HashMap<String, Live> {
    app.running
        .lock()
        .expect("running")
        .iter()
        .map(|(k, r)| (k.clone(), r.live.lock().expect("live").clone()))
        .collect()
}

/// 现在就去问一次报文，不等下一个五分钟。
///
/// 特报（SPECI）不按半小时来，而值班的人往往比轮询先知道天气变了。
#[tauri::command]
fn refresh(app: tauri::State<'_, App>, callsign: String) -> bool {
    match app.running.lock().expect("running").get(&callsign) {
        Some(r) => {
            r.wake.notify_one();
            true
        }
        None => false,
    }
}

/// 一个席位的后台循环：盯 FSD 的状态，按时问报文，报文变了就推进字母、重渲染、
/// 把新文字送上线。
async fn watch(
    station: Station,
    preset: Preset,
    fsd: FsdHandle,
    live: Arc<Mutex<Live>>,
    wake: Arc<tokio::sync::Notify>,
) {
    let mut events = fsd.events();
    let mut ticker = tokio::time::interval(METAR_POLL);
    let mut online = false;
    let mut letter = station.letter;
    let mut last_metar = String::new();

    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(FsdEvent { state, reason }) => {
                    {
                        let mut l = live.lock().expect("live");
                        l.state = Some(state);
                        l.reason = Some(reason);
                    }
                    let was_online = online;
                    online = state == FsdState::Online;
                    if matches!(state, FsdState::Offline | FsdState::Stopped) {
                        return;
                    }
                    // 刚上线就立刻要一份报文，不等第一个五分钟——否则席位
                    // 挂在网上却一句通播都没有。
                    if online && !was_online {
                        poll(&station, &preset, &fsd, &live, &mut letter, &mut last_metar, true).await;
                    }
                }
                // 事件流跟不上就丢了几条状态；下一条会补上，不必重来。
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            },
            _ = ticker.tick() => {
                if online {
                    poll(&station, &preset, &fsd, &live, &mut letter, &mut last_metar, false).await;
                }
            }
            _ = wake.notified() => {
                if online {
                    poll(&station, &preset, &fsd, &live, &mut letter, &mut last_metar, false).await;
                }
            }
        }
    }
}

/// 收到一份报文之后该做什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    /// 报文没变，什么都不做。
    Nothing,
    /// 重新渲染并发上去；`advance` 表示同时推进情报字母。
    Publish { advance: bool },
}

/// **只有报文真的变了才推进情报字母。**
///
/// 字母是给飞行员回报用的凭据（"information Bravo"）；每五分钟跳一格的话，
/// 机组报的那个字母永远和当前那份对不上，而管制员没法判断他到底看没看过。
///
/// 第一次拿到报文是个例外：要发，但**不推进**——那一份就是当前这个字母的内容，
/// 推进一格等于宣布了一份从来没播过的通播。
fn decide(previous: &str, report: &str, first: bool) -> Next {
    if report == previous && !first {
        return Next::Nothing;
    }
    Next::Publish {
        advance: !previous.is_empty() && report != previous,
    }
}

/// 问一次报文，变了就推进字母并重发。
async fn poll(
    station: &Station,
    preset: &Preset,
    fsd: &FsdHandle,
    live: &Arc<Mutex<Live>>,
    letter: &mut char,
    last_metar: &mut String,
    first: bool,
) {
    let Some(report) = fsd.request_metar(&station.identifier, METAR_TIMEOUT).await else {
        // 问不到就**保持现状**，不清空也不换字母：一次网络抖动不该让一个席位
        // 的通播变成空的。
        tracing::info!(icao = %station.identifier, "no metar this round");
        return;
    };
    let Next::Publish { advance } = decide(last_metar, &report, first) else {
        return;
    };
    if advance {
        let mut s = station.clone();
        s.letter = *letter;
        *letter = s.advance_letter();
    }
    *last_metar = report.clone();

    let rendered = script::render(station, preset, &Metar::parse(&report), *letter);
    fsd.set_atis_lines(packet::wrap_atis_text(&rendered.wire));
    let mut l = live.lock().expect("live");
    l.letter = *letter;
    l.metar = report;
    l.rendered = rendered;
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(env_or("RUST_LOG", "info"))
        .init();

    tauri::Builder::default()
        .manage(App::new())
        .invoke_handler(tauri::generate_handler![
            profiles,
            stations,
            add_profile,
            rename_profile,
            remove_profile,
            select_profile,
            add_station,
            remove_station,
            save_station,
            preview,
            start,
            stop,
            live,
            refresh,
        ])
        .run(tauri::generate_context!())
        .expect("tauri failed to start");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 第一份报文要发，但**不推进字母**——那一份就是当前这个字母的内容。
    #[test]
    fn the_first_report_is_published_without_advancing() {
        assert_eq!(
            decide("", "ZSPD 251300Z", true),
            Next::Publish { advance: false }
        );
    }

    /// 报文没变就什么都不做。每五分钟重发一次没有坏处，但**每五分钟跳一格
    /// 字母**会让机组报的那个字母永远对不上。
    #[test]
    fn an_unchanged_report_changes_nothing() {
        assert_eq!(decide("ZSPD 251300Z", "ZSPD 251300Z", false), Next::Nothing);
    }

    #[test]
    fn a_changed_report_advances_the_letter() {
        assert_eq!(
            decide("ZSPD 251300Z", "ZSPD 251400Z", false),
            Next::Publish { advance: true }
        );
    }

    /// 刚上线时即使报文和上一轮一样也要发一次——**席位挂在网上却一句通播
    /// 都没有**比多发一次糟得多。但同样不推进字母。
    #[test]
    fn coming_back_online_republishes_without_advancing() {
        assert_eq!(
            decide("ZSPD 251300Z", "ZSPD 251300Z", true),
            Next::Publish { advance: false }
        );
    }
}
