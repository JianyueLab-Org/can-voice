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

use can_voice_atis::datafeed::Online;
use can_voice_atis::metar::Metar;
use can_voice_atis::netconfig::{self, Comparison, Merged, NetworkConfig};
use can_voice_atis::profile::{
    Preset, Profile, ProfileError, ProfileSet, Station, DEFAULT_PROFILE_PATH,
};
use can_voice_atis::script::{self, Rendered};
use can_voice_atis::{vatis, weather};
use can_voice_fsd::client::{self, Config, FsdEvent, FsdHandle, FsdState, Reason};
use can_voice_fsd::packet::{self, Identity, Position, FACILITY_ATIS, RATING_OBSERVER};
use can_voice_i18n::Message;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Manager;

/// 多久问一次报文，默认值。
///
/// METAR 半小时一份（特报不定时），五分钟一问既不会漏掉特报，也不会把服务端
/// 的气象缓存问穿。**可配**：旧版就能调，不同机场的特报节奏差别不小。
const DEFAULT_REFRESH_SECS: u32 = 300;
/// 夹住。填 5 秒的人会每五秒去问一次服务端的气象缓存；填 0 的那个更糟——
/// `tokio::time::interval` 的周期是 0 会直接 panic，而那会把整个席位的循环打死，
/// 界面上看到的是"上线之后再也没有报文"。
const MIN_REFRESH_SECS: u32 = 60;
const MAX_REFRESH_SECS: u32 = 3600;
const METAR_TIMEOUT: Duration = Duration::from_secs(20);
/// HTTP 那几条路（气象兜底、网络配置、数据源）的超时。
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// 气象源地址。环境变量 > 设置 > 默认。
fn metar_url(endpoints: &can_voice_settings::Endpoints) -> String {
    can_voice_settings::endpoints::endpoint(
        "CAN_METAR_URL",
        &endpoints.metar_url,
        weather::DEFAULT_URL,
    )
}

fn clamp_refresh(secs: u32) -> u32 {
    secs.clamp(MIN_REFRESH_SECS, MAX_REFRESH_SECS)
}

fn default_refresh() -> u32 {
    DEFAULT_REFRESH_SECS
}

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
    /// 这个席位此刻在播的东西。**界面上的动作改的就是它。**
    airing: Arc<Mutex<Airing>>,
    task: tokio::task::JoinHandle<()>,
    /// 按"现在就更新"时敲它一下，盯报文那条循环立刻去问一次。
    wake: Arc<tokio::sync::Notify>,
}

/// 一个在播席位的可变部分。
///
/// **跑道构型和情报字母都在这里而不是那条循环的局部变量里**，因为它们要能被界面
/// 改：此前换一套跑道构型必须把席位停掉重上，而重上的那几十秒里飞行员查不到通播
/// ——恰恰是管制员正在换跑道、最不该让通播消失的时刻。
struct Airing {
    station: Station,
    preset: Preset,
    letter: char,
    /// 最近一次拿到的报文原文。换构型、推字母时照着它重渲染，不必再问一次 FSD。
    last_metar: String,
}

/// 按当前这一份在播状态渲染一遍。
fn render_airing(a: &Airing) -> Rendered {
    script::render(
        &a.station,
        &a.preset,
        &Metar::parse(&a.last_metar),
        a.letter,
    )
}

/// 重渲染、送上线、更新界面读的那一份。
///
/// 报文还没拿到时**什么都不做**：拿一份空报文渲染出来的稿子会把"还没有天气"
/// 播成一份看起来正常的通播。
fn publish(airing: &Airing, fsd: &FsdHandle, live: &Arc<Mutex<Live>>) -> bool {
    if airing.last_metar.is_empty() {
        return false;
    }
    let rendered = render_airing(airing);
    fsd.set_atis_lines(packet::wrap_atis_text(&rendered.wire));
    let mut l = live.lock().expect("live");
    l.letter = airing.letter;
    l.preset = airing.preset.name.clone();
    l.rendered = rendered;
    true
}

/// 存下来的设置。
///
/// **只有 CAN 号。** 密码不存——它换的是一张短寿命的票，把一份长期凭据留在
/// 磁盘上买不到任何东西；而挂五个席位要重复确认五次，那是 CAN 号的问题。
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub cid: String,
    /// 用户说过"这一版不用再问我"的那个版本号。**跳过的是那一个版本，
    /// 不是从此闭嘴**——下一版照样提示。
    #[serde(default)]
    pub skipped_update: String,
    /// 多久问一次报文（秒）。
    ///
    /// **默认值走 `default_refresh` 而不是 `0`**：老的设置文件里没有这一项，
    /// 反序列化拿到 0 会被夹成 60 秒，等于一次升级把所有人的轮询加密五倍。
    #[serde(default = "default_refresh")]
    pub metar_refresh_secs: u32,
    /// 登录用的等级。**0 表示自动**——跟着本人在数据源上的实际等级走。
    ///
    /// 写死观察员的话，一个 C1 管制员开的通播在雷达图上显示成观察员，而管制席位
    /// 上的同一个人是 C1。
    #[serde(default)]
    pub rating: u32,
    /// 上一次**整份**并进来的网络配置版本（服务端算的内容哈希）。
    ///
    /// 只在整份都并进来时才记：没勾覆盖、或有席位因为在播被跳过的话，下次点开
    /// 那些差异还要让人再看一遍。
    #[serde(default)]
    pub config_version: String,
    /// 主题、置顶、精简。
    #[serde(default)]
    pub appearance: can_voice_settings::Appearance,
    /// 各服务的地址。空的是默认；**环境变量仍然最大**，见 `can_voice_settings::endpoints`。
    #[serde(default)]
    pub endpoints: can_voice_settings::Endpoints,
    /// 调试级日志。**下次启动才生效**：日志订阅器在进程一开始就装好了，
    /// 半路换级别要一整套 reload 句柄，为一个排障开关不值得。
    #[serde(default)]
    pub debug_log: bool,
}

pub struct App {
    profiles: Mutex<ProfileSet>,
    running: Mutex<HashMap<String, Running>>,
    store: can_voice_settings::Store,
    settings: Mutex<Settings>,
    /// 报文刷新周期（秒）。每个在播席位那条循环读它，所以是原子量不是锁。
    refresh_secs: Arc<std::sync::atomic::AtomicU32>,
    /// 共用一个 HTTP 客户端：报文兜底每个在播席位每个周期都可能用一次。
    http: reqwest::Client,
    /// 最近一次取回来、给人看过差异的网络配置。
    ///
    /// **并的是给人看过的那一份**，不是按下"并进来"时再取一次——两次之间服务端
    /// 可能已经换了，而人点头的是他看到的那份差异。
    network: Mutex<Option<(NetworkConfig, Comparison)>>,
}

impl App {
    pub fn new() -> Self {
        let store = can_voice_settings::Store::for_product("atis-for-can");
        let settings: Settings = store.load();
        Self {
            profiles: Mutex::new(ProfileSet::load(profile_path())),
            running: Mutex::new(HashMap::new()),
            store,
            refresh_secs: Arc::new(std::sync::atomic::AtomicU32::new(clamp_refresh(
                settings.metar_refresh_secs,
            ))),
            settings: Mutex::new(settings),
            http: reqwest::Client::builder()
                .timeout(HTTP_TIMEOUT)
                .build()
                .unwrap_or_default(),
            network: Mutex::new(None),
        }
    }

    fn settings_snapshot(&self) -> Settings {
        match self.settings.lock() {
            Ok(s) => s.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

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

/// 去数据源上查这个 CAN 号此刻的等级。
///
/// 查不到返回 `None`，调用方回落到观察员——一次 datafeed 抖动不该让人登不上去。
async fn rating_lookup(cid: &str, endpoints: &can_voice_settings::Endpoints) -> Option<u32> {
    let url = can_voice_settings::endpoints::endpoint(
        "CAN_FSD_DATAFEED",
        &endpoints.datafeed_url,
        can_voice_datafeed::DEFAULT_URL,
    );
    let feed = can_voice_datafeed::fetch_once(&url).await?;
    can_voice_datafeed::rating_for(cid, &feed)
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
fn add_profile(app: tauri::State<'_, App>, name: String) -> Result<(), Message> {
    let mut set = app.profiles.lock().expect("profiles");
    set.add(&name).map_err(|e| e.message())?;
    save(&set);
    Ok(())
}

#[tauri::command]
fn rename_profile(app: tauri::State<'_, App>, old: String, new: String) -> Result<(), Message> {
    let mut set = app.profiles.lock().expect("profiles");
    set.rename(&old, &new).map_err(|e| e.message())?;
    save(&set);
    Ok(())
}

#[tauri::command]
fn remove_profile(app: tauri::State<'_, App>, name: String) -> Result<bool, Message> {
    let mut set = app.profiles.lock().expect("profiles");
    let gone = set.remove(&name).map_err(|e| e.message())?;
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
fn add_station(app: tauri::State<'_, App>, identifier: String) -> Result<Station, Message> {
    let station = Station::new(&identifier);
    // 呼号不合规就**在这里**说，而不是等连上去被服务端拒。
    packet::check_atis_callsign(&station.callsign()).map_err(|e| e.message())?;
    let mut set = app.profiles.lock().expect("profiles");
    set.active().add(station.clone()).map_err(|e| e.message())?;
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
) -> Result<(), Message> {
    station.normalise();
    packet::check_atis_callsign(&station.callsign()).map_err(|e| e.message())?;
    let mut set = app.profiles.lock().expect("profiles");
    let profile: &mut Profile = set.active();
    // 换了呼号就等于换了一个席位，重名要挡住。说法和 `Profile::add` 撞上重名时同一句。
    if station.callsign() != callsign && profile.get(&station.callsign()).is_some() {
        return Err(ProfileError::DuplicateStation(station.callsign()).message());
    }
    profile.remove(&callsign);
    profile.add(station).map_err(|e| e.message())?;
    save(&set);
    Ok(())
}

// ——— 从外面取 ———
//
// 四件事，旧版都有、新版此前一件都没有（#44）。**失败一律说出为什么**：这几条都是
// 人按了按钮才发生的，失败却什么都不说，他只得到一个没反应的按钮。

/// 在播的呼号。**并配置时这些一律不动**：换掉一个在播席位只会让稿子和实际在播
/// 的内容对不上。
fn on_air(app: &App) -> Vec<String> {
    match app.running.lock() {
        Ok(r) => r.keys().cloned().collect(),
        Err(p) => p.into_inner().keys().cloned().collect(),
    }
}

/// 取一份真实报文，**不上线也能取**。
///
/// 没有它的话，"先起客户端把稿子写好，再上线播"做不成：问报文的 `$AX` 要求已经
/// 连着 FSD，于是写模板的人只能对着一份编出来的电码调格式。
#[tauri::command]
async fn fetch_metar(app: tauri::State<'_, App>, icao: String) -> Result<String, Message> {
    let url = metar_url(&app.settings_snapshot().endpoints);
    weather::fetch(&app.http, &url, &icao, weather::RETRIES)
        .await
        .map_err(|e| e.message())
}

/// 导进来的结果：并了什么、跳过了什么、vATIS 那边有哪些这里没有对应功能。
///
/// 后两样是两张 [`Message`] 单子，前端按当前语言拼成提示（#29）。
#[derive(serde::Serialize)]
pub struct ImportReport {
    source: String,
    merged: Merged,
    failures: Vec<Message>,
    skipped: Vec<Message>,
}

/// 导入 vATIS 的配置。文件是界面那一侧读出来递过来的——为一个选文件的框引一个
/// 对话框插件、再开一条文件系统权限，不值得。
///
/// **只补缺，不覆盖**：同呼号的席位原样保留，和旧版一致。本地那一份多半是值班时
/// 调过的，别人的导出文件不该盖掉它。
#[tauri::command]
fn import_vatis(app: tauri::State<'_, App>, body: String) -> Result<ImportReport, Message> {
    let imported = vatis::from_text(&body).map_err(|e| e.message())?;
    let protected = on_air(&app);
    let mut set = app.profiles.lock().expect("profiles");
    let merged = netconfig::merge(set.active(), &imported.stations, false, &protected);
    save(&set);
    Ok(ImportReport {
        source: imported.name,
        merged,
        failures: imported.failures,
        skipped: imported.skipped,
    })
}

/// 把数据源上此刻在线的通播席位加进配置。
///
/// 这一步省掉的**只是查机场和频率**：模板、预设、跑道构型数据源给不了，要那些
/// 得走 [`check_network_config`]。只补缺，已有的呼号不动。
#[tauri::command]
async fn import_online(app: tauri::State<'_, App>) -> Result<Merged, Message> {
    let url = can_voice_settings::endpoints::endpoint(
        "CAN_FSD_DATAFEED",
        &app.settings_snapshot().endpoints.datafeed_url,
        can_voice_datafeed::DEFAULT_URL,
    );
    let feed = can_voice_datafeed::fetch(&app.http, &url)
        .await
        .ok_or_else(|| Message::new("problem.datafeed.unreachable").with("url", &url))?;
    let stations: Vec<Station> = can_voice_atis::datafeed::online_stations(&feed)
        .iter()
        .map(Online::to_station)
        .collect();
    let protected = on_air(&app);
    let mut set = app.profiles.lock().expect("profiles");
    let merged = netconfig::merge(set.active(), &stations, false, &protected);
    save(&set);
    Ok(merged)
}

/// 给人看的那份差异。装的是呼号，不是整个席位——界面只要列出来。
#[derive(serde::Serialize)]
pub struct NetworkPreview {
    label: Message,
    version: String,
    /// 服务端写的那一行说明，原样显示。
    notes: String,
    problems: Vec<Message>,
    /// 上一次整份并进来的版本。空的表示从没并过。
    previous: String,
    missing: Vec<String>,
    differing: Vec<String>,
    same: Vec<String>,
    /// 有差异、但正在播出的那些。并的时候会被跳过，先告诉人。
    on_air: Vec<String>,
}

/// 取全网通播配置，**只看差异，不动本地**。
///
/// 值班时"按一下就变了"很难接受，所以动手是另一个命令，而且只动人勾了的那些。
#[tauri::command]
async fn check_network_config(app: tauri::State<'_, App>) -> Result<NetworkPreview, Message> {
    let url = can_voice_settings::endpoints::endpoint(
        "CAN_ATIS_CONFIG_URL",
        &app.settings_snapshot().endpoints.atis_config_url,
        netconfig::DEFAULT_URL,
    );
    let document = netconfig::fetch(&app.http, &url)
        .await
        .map_err(|e| e.message())?;
    let config = netconfig::parse(&document).map_err(|e| e.message())?;
    let comparison = {
        let mut set = app.profiles.lock().expect("profiles");
        netconfig::compare(set.active(), &config.stations)
    };
    let live = on_air(&app);
    let callsigns = |list: &[Station]| list.iter().map(Station::callsign).collect::<Vec<_>>();
    let preview = NetworkPreview {
        label: config.label(),
        version: config.version.clone(),
        notes: config.notes.clone(),
        problems: config.problems.clone(),
        previous: app.settings_snapshot().config_version,
        missing: callsigns(&comparison.missing),
        differing: callsigns(&comparison.differing),
        same: callsigns(&comparison.same),
        on_air: callsigns(&comparison.differing)
            .into_iter()
            .filter(|c| live.contains(c))
            .collect(),
    };
    *app.network.lock().expect("network") = Some((config, comparison));
    Ok(preview)
}

/// 把上一次看过的那份网络配置并进来，只并勾了的。
#[tauri::command]
fn apply_network_config(
    app: tauri::State<'_, App>,
    add_missing: bool,
    overwrite: bool,
) -> Result<Merged, Message> {
    let Some((config, comparison)) = app.network.lock().expect("network").take() else {
        return Err(Message::new("problem.network.not_checked"));
    };
    let protected = on_air(&app);
    let merged = {
        let mut set = app.profiles.lock().expect("profiles");
        let chosen = netconfig::chosen(&comparison, add_missing, overwrite);
        let merged = netconfig::merge(set.active(), &chosen, overwrite, &protected);
        save(&set);
        merged
    };
    if merged.settles(&comparison, add_missing, overwrite) {
        app.update_settings(|s| s.config_version = config.version.clone());
    }
    Ok(merged)
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
) -> Result<Rendered, Message> {
    let mut set = app.profiles.lock().expect("profiles");
    let station = set
        .active()
        .get(&callsign)
        .ok_or_else(|| Message::new("problem.station.missing").with("callsign", &callsign))?
        .clone();
    let chosen: Preset = station
        .preset(&preset)
        .cloned()
        .ok_or_else(|| Message::new("problem.station.no_presets"))?;
    let letter = letter.chars().next().unwrap_or(station.letter);
    Ok(script::render(
        &station,
        &chosen,
        &Metar::parse(&metar),
        letter,
    ))
}

/// 存下来的设置。**前端一挂上就读它**，把 CAN 号填回去。
#[tauri::command]
fn settings(app: tauri::State<'_, App>) -> Settings {
    match app.settings.lock() {
        Ok(s) => s.clone(),
        Err(p) => p.into_inner().clone(),
    }
}

/// 记住这个 CAN 号。上线成功之后才叫，连不上的那个多半是打错了。
fn remember_cid(app: &App, cid: &str) {
    let mut s = match app.settings.lock() {
        Ok(s) => s,
        Err(p) => p.into_inner(),
    };
    if s.cid == cid {
        return;
    }
    s.cid = cid.to_string();
    if let Err(e) = app.store.save(&*s) {
        tracing::warn!(error = %e, "could not save the settings");
    }
}

// ——— 上线 / 下线 ———

/// 上线。
///
/// **是 async 的**：上线之前要去数据源查一次本人的等级，而那是一次 HTTP 往返。
/// 同步命令里等它，等于窗口在那几秒里点不动。
#[tauri::command]
async fn start(
    app: tauri::State<'_, App>,
    callsign: String,
    preset: String,
    cid: String,
    password: String,
) -> Result<(), Message> {
    let station = {
        let mut set = app.profiles.lock().expect("profiles");
        set.active()
            .get(&callsign)
            .ok_or_else(|| Message::new("problem.station.missing").with("callsign", &callsign))?
            .clone()
    };
    let chosen = station
        .preset(&preset)
        .cloned()
        .ok_or_else(|| Message::new("problem.station.no_presets"))?;
    // 频率在这里就要能解析。等连上去才发现的话，席位已经挂在网上了，
    // 而位置包会一直发不出去。
    station.frequency_khz().ok_or_else(|| {
        Message::new("problem.station.bad_frequency").with("frequency", &station.frequency)
    })?;

    // **等级跟着本人**：写死观察员的话，一个 C1 管制员开的通播在雷达图上显示成
    // 观察员，而管制席位上的同一个人是 C1。设置里指定了就用指定的；0 表示自动，
    // 去数据源上查一次。查不到就回落到观察员——一次 datafeed 抖动不该让人登不上。
    //
    // **在拿锁之前查。** 这一步要 await，而 `running` 是一把 std 的锁：攥着它
    // 跨 await 会把整张在播表挡住那几秒。
    let saved = app.settings_snapshot();
    let rating = match saved.rating {
        0 => rating_lookup(&cid, &saved.endpoints)
            .await
            .unwrap_or(RATING_OBSERVER),
        chosen => chosen,
    };

    let (fsd_host, fsd_port) = saved.endpoints.fsd();
    let mut running = app.running.lock().expect("running");
    if running.contains_key(&callsign) {
        return Err(Message::new("problem.station.already_on_air").with("callsign", &callsign));
    }

    let config = Config {
        host: fsd_host,
        port: fsd_port,
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
            rating,
        ),
        position: Position {
            frequency: station.frequency.clone(),
            facility: FACILITY_ATIS,
            vis_range: 50,
            rating,
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
    let airing = Arc::new(Mutex::new(Airing {
        letter: station.letter,
        station,
        preset: chosen,
        last_metar: String::new(),
    }));
    let wake = Arc::new(tokio::sync::Notify::new());
    let fsd = client::connect(config);
    let task = tokio::spawn(watch(
        airing.clone(),
        fsd.clone(),
        live.clone(),
        wake.clone(),
        app.refresh_secs.clone(),
        app.http.clone(),
        metar_url(&saved.endpoints),
    ));
    running.insert(
        callsign,
        Running {
            fsd,
            live,
            airing,
            task,
            wake,
        },
    );
    drop(running);
    remember_cid(&app, &cid);
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

/// 换一套跑道构型，**不必把席位停掉重上**。
///
/// 重上一次，飞行员在那几十秒里查不到通播，而管制员正在忙着换跑道——恰恰是最不
/// 该让通播消失的时刻。
///
/// **同时推进情报字母。** 跑道构型变了就是另一份通播；不推进的话，"information
/// Bravo" 这个名字底下的内容被悄悄换掉了，而手里拿着 Bravo 的机组无从知道。
/// 只想换个字母不动构型的，用 [`advance_letter`]。
#[tauri::command]
fn set_preset(app: tauri::State<'_, App>, callsign: String, preset: String) -> Result<(), Message> {
    let running = app.running.lock().expect("running");
    let r = running
        .get(&callsign)
        .ok_or_else(|| Message::new("problem.station.not_on_air").with("callsign", &callsign))?;
    let mut a = r.airing.lock().expect("airing");
    let chosen = a
        .station
        .preset(&preset)
        .cloned()
        .ok_or_else(|| Message::new("problem.station.no_preset").with("preset", &preset))?;
    a.preset = chosen;
    a.letter = next_letter(&a.station, a.letter);
    publish(&a, &r.fsd, &r.live);
    Ok(())
}

/// 手动推进一格情报字母并重发。
///
/// 播错了、或者报文没变但场面条件变了（跑道积水、某条滑行道关闭），都要靠它。
/// 自动推进只认报文变化，而那两件事报文里没有。
#[tauri::command]
fn advance_letter(app: tauri::State<'_, App>, callsign: String) -> Result<char, Message> {
    let running = app.running.lock().expect("running");
    let r = running
        .get(&callsign)
        .ok_or_else(|| Message::new("problem.station.not_on_air").with("callsign", &callsign))?;
    let mut a = r.airing.lock().expect("airing");
    a.letter = next_letter(&a.station, a.letter);
    publish(&a, &r.fsd, &r.live);
    Ok(a.letter)
}

/// 改报文刷新周期。**立刻生效**：每个席位那条循环下一拍就换 ticker。
#[tauri::command]
fn set_metar_refresh(app: tauri::State<'_, App>, secs: u32) -> u32 {
    let secs = clamp_refresh(secs);
    app.refresh_secs
        .store(secs, std::sync::atomic::Ordering::Relaxed);
    app.update_settings(|s| s.metar_refresh_secs = secs);
    // 把夹过的那个数还回去：填 5 之后界面上该看到 60。
    secs
}

/// 改登录等级。0 表示自动（跟着数据源上本人的实际等级）。
///
/// **下次上线才生效**：FSD 的等级是登录时声明的，改一个已经挂着的席位要重连。
#[tauri::command]
fn set_rating(app: tauri::State<'_, App>, rating: u32) {
    app.update_settings(|s| s.rating = rating);
}

/// 这份模板里有哪些认不出来的变量。
///
/// **认不出的变量是照字面念出去的**（`[RWY]` 打成 `[RUNWAY]`，飞行员听到的就是
/// 一句"runway"后面跟着中括号里那个词）。旧版每次重渲染都提示一次，新版这条检查
/// 写了却从没有人调用。
#[tauri::command]
fn template_problems(template: String) -> Vec<String> {
    can_voice_atis::template::unknown_variables(&template)
}

/// 一个席位的后台循环：盯 FSD 的状态，按时问报文，报文变了就推进字母、重渲染、
/// 把新文字送上线。
async fn watch(
    airing: Arc<Mutex<Airing>>,
    fsd: FsdHandle,
    live: Arc<Mutex<Live>>,
    wake: Arc<tokio::sync::Notify>,
    refresh: Arc<std::sync::atomic::AtomicU32>,
    http: reqwest::Client,
    // 上线那一刻定下来的气象源。和别的地址一样**下次上线才换**：
    // 播到一半换一个源，同一份报文可能因为格式差一点被当成变了。
    metar: String,
) {
    let mut events = fsd.events();
    let mut period = refresh_period(&refresh);
    let mut ticker = tokio::time::interval(period);
    let mut online = false;

    loop {
        // 周期改了就换一个 ticker：改完要立刻生效，而不是等这个席位下一次上线。
        let wanted = refresh_period(&refresh);
        if wanted != period {
            period = wanted;
            ticker = tokio::time::interval(wanted);
        }
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
                        poll(&airing, &fsd, &live, &http, &metar, true).await;
                    }
                }
                // 事件流跟不上就丢了几条状态；下一条会补上，不必重来。
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return,
            },
            _ = ticker.tick() => {
                if online {
                    poll(&airing, &fsd, &live, &http, &metar, false).await;
                }
            }
            _ = wake.notified() => {
                if online {
                    poll(&airing, &fsd, &live, &http, &metar, false).await;
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

fn refresh_period(refresh: &std::sync::atomic::AtomicU32) -> Duration {
    Duration::from_secs(clamp_refresh(refresh.load(std::sync::atomic::Ordering::Relaxed)).into())
}

/// 问一次报文，变了就推进字母并重发。
async fn poll(
    airing: &Arc<Mutex<Airing>>,
    fsd: &FsdHandle,
    live: &Arc<Mutex<Live>>,
    http: &reqwest::Client,
    metar: &str,
    first: bool,
) {
    // **锁不跨 await。** 问报文最长要二十秒，攥着锁的话这二十秒里换构型、
    // 推字母、读界面全都卡住。
    let (icao, previous) = {
        let a = airing.lock().expect("airing");
        (a.station.identifier.clone(), a.last_metar.clone())
    };
    let report = match fsd.request_metar(&icao, METAR_TIMEOUT).await {
        Some(report) => Some(report),
        // **FSD 没给就走 HTTP 兜底。** 服务端的气象源也会抖，一个在播席位不该
        // 因为那一侧抖了一下就整整一个周期没有新报文。
        //
        // 两条路来的报文可以直接比：can-fsd 问的是同一个气象源，也是 trim 之后
        // 取 ICAO 开头的那一行（`internal/fsd/metar.go` 的 `extractMetar`），所以
        // 换了来源不会让 `decide` 把同一份报文当成变了、白推一格字母。
        None => match weather::fetch(http, metar, &icao, weather::RETRIES).await {
            Ok(report) => {
                tracing::info!(icao = %icao, "fsd gave no metar; took it over http");
                Some(report)
            }
            Err(e) => {
                tracing::info!(icao = %icao, error = %e, "no metar this round");
                None
            }
        },
    };
    // 问不到就**保持现状**，不清空也不换字母：一次网络抖动不该让一个席位的通播
    // 变成空的。
    let Some(report) = report else {
        return;
    };
    let Next::Publish { advance } = decide(&previous, &report, first) else {
        return;
    };

    let mut a = airing.lock().expect("airing");
    if advance {
        a.letter = next_letter(&a.station, a.letter);
    }
    a.last_metar = report.clone();
    publish(&a, fsd, live);
    live.lock().expect("live").metar = report;
}

/// 下一个情报字母。
fn next_letter(station: &Station, current: char) -> char {
    let mut s = station.clone();
    s.letter = current;
    s.advance_letter()
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
    let (skipped, busy) = {
        let s = match app.settings.lock() {
            Ok(s) => s.clone(),
            Err(p) => p.into_inner().clone(),
        };
        // 有席位在播就是"正在工作"：换版本要停播，而停播是全网听得见的。
        let busy = !match app.running.lock() {
            Ok(r) => r.is_empty(),
            Err(p) => p.into_inner().is_empty(),
        };
        (s.skipped_update, busy)
    };
    let origin = app.settings_snapshot().endpoints.api_origin();
    let Some(latest) =
        can_voice_update::check_once(&origin, "atis-for-can", env!("CARGO_PKG_VERSION")).await
    else {
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
    let mut s = match app.settings.lock() {
        Ok(s) => s,
        Err(p) => p.into_inner(),
    };
    s.skipped_update = version;
    if let Err(e) = app.store.save(&*s) {
        tracing::warn!(error = %e, "could not save the settings");
    }
}

/// 用系统浏览器打开下载页。**绝不自动更新**：装不装、什么时候装是人决定的。
#[tauri::command]
fn open_download(url: String) -> Result<(), Message> {
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
) -> Result<(), Message> {
    let origin = app.settings_snapshot().endpoints.api_origin();
    let _ = &app;
    can_voice_log::upload_once(
        &origin,
        "atis-for-can",
        env!("CARGO_PKG_VERSION"),
        &cid,
        &password,
    )
    .await
}

// ——— 设置对话框、置顶、精简（#45）———

/// 精简模式下窗口最小能缩到多小。
const COMPACT_MIN: (f64, f64) = (260.0, 220.0);
/// 按下"精简"那一刻缩成多大。**不缩的话**，东西藏起来了窗口却还是那么大，
/// 人还得自己去拖——而这个开关存在的全部理由就是一下子压到雷达屏的角落里。
const COMPACT_SIZE: (f64, f64) = (320.0, 440.0);

/// 把外观里和窗口有关的两样落到窗口上。
///
/// 正常模式的最小尺寸**从 `tauri.conf.json` 读**，不在这里再抄一份：两处写同一
/// 对数字，改了一边，退出精简时窗口就还原到一个旧尺寸上。
///
/// `shrink` 为真时顺手把窗口缩到 [`COMPACT_SIZE`]：只在精简**刚打开**的那一刻、
/// 和启动时照着存下来的状态还原时才这么做——不然每改一次主题窗口都跳一下。
fn apply_window(
    window: &tauri::WebviewWindow,
    appearance: &can_voice_settings::Appearance,
    shrink: bool,
) {
    if let Err(e) = window.set_always_on_top(appearance.always_on_top) {
        tracing::warn!(error = %e, "could not change always-on-top");
    }
    let (w, h) = if appearance.compact {
        COMPACT_MIN
    } else {
        let config = window.app_handle().config();
        let conf = config.app.windows.first();
        (
            conf.and_then(|c| c.min_width).unwrap_or(COMPACT_MIN.0),
            conf.and_then(|c| c.min_height).unwrap_or(COMPACT_MIN.1),
        )
    };
    if let Err(e) = window.set_min_size(Some(tauri::LogicalSize::new(w, h))) {
        tracing::warn!(error = %e, "could not change the minimum window size");
    }
    if shrink && appearance.compact {
        let _ = window.set_size(tauri::LogicalSize::new(COMPACT_SIZE.0, COMPACT_SIZE.1));
    }
}

/// 换外观。主题归前端管，这里只存；置顶和精简要动窗口。
#[tauri::command]
fn set_appearance(
    window: tauri::WebviewWindow,
    app: tauri::State<'_, App>,
    appearance: can_voice_settings::Appearance,
) {
    let was_compact = app.settings_snapshot().appearance.compact;
    apply_window(&window, &appearance, !was_compact);
    app.update_settings(|s| s.appearance = appearance);
}

/// 存地址。**填得不对就不存**，并说出哪里不对：存进去一个连不上的地址，下次连接
/// 时报的是网络错误，指不到这里。回的是去掉首尾空白之后真正存下的那一份。
///
/// 下次连接才生效——正连着的那条链路不会被半路换掉。
#[tauri::command]
fn set_endpoints(
    app: tauri::State<'_, App>,
    endpoints: can_voice_settings::Endpoints,
) -> Result<can_voice_settings::Endpoints, Vec<Message>> {
    let endpoints = endpoints.trimmed();
    let problems = endpoints.problems();
    if !problems.is_empty() {
        // 整张清单交回去，前端按当前语言翻、按当前语言的句读连起来。
        return Err(problems);
    }
    app.update_settings(|s| s.endpoints = endpoints.clone());
    Ok(endpoints)
}

/// 这个客户端用得上的那几格地址：默认是什么、此刻是不是被环境变量盖着。
///
/// 被盖着的那几格界面要标出来——否则改了没反应，看起来就是设置坏了。
#[tauri::command]
fn endpoint_fields() -> Vec<can_voice_settings::endpoints::Field> {
    can_voice_settings::endpoints::fields(&[
        ("api_origin", can_voice_settings::endpoints::API_ORIGIN),
        ("fsd_server", can_voice_settings::endpoints::FSD_SERVER),
        ("datafeed_url", can_voice_datafeed::DEFAULT_URL),
        ("metar_url", weather::DEFAULT_URL),
        ("atis_config_url", netconfig::DEFAULT_URL),
    ])
}

#[tauri::command]
fn set_debug_log(app: tauri::State<'_, App>, on: bool) {
    app.update_settings(|s| s.debug_log = on);
}

pub fn run() {
    // **日志要落盘。** 打包出来的是一个没有控制台的 GUI 进程，`stdout` 写到哪里
    // 谁也看不见；用户报"连不上"的时候手里得有一份能发出来的东西。
    // 这一步同时装上 panic 钩子——崩溃不留记录的话，窗口没了、日志干净。
    // 设置里的调试开关要在装日志**之前**读。读设置本身失败时打的那条警告因此
    // 没有地方去——但读失败的结果是默认值，而默认值就是不开调试，不影响判断。
    let saved: Settings = can_voice_settings::Store::for_product("atis-for-can").load();
    can_voice_log::init(
        "atis-for-can",
        env!("CARGO_PKG_VERSION"),
        std::env::args().any(|a| a == "--debug") || saved.debug_log,
    );

    tauri::Builder::default()
        .manage(App::new())
        // 置顶和精简在窗口一出来就还原。压在雷达屏上用的人不该每次启动都再点一遍。
        .setup(|handle| {
            let appearance = handle.state::<App>().settings_snapshot().appearance;
            if let Some(window) = handle.get_webview_window("main") {
                apply_window(&window, &appearance, appearance.compact);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            log_file,
            set_appearance,
            set_endpoints,
            endpoint_fields,
            set_debug_log,
            send_log,
            check_update,
            skip_update,
            open_download,
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
            set_preset,
            advance_letter,
            set_metar_refresh,
            set_rating,
            template_problems,
            fetch_metar,
            import_vatis,
            import_online,
            check_network_config,
            apply_network_config,
            live,
            refresh,
            settings,
        ])
        .run(tauri::generate_context!())
        .expect("tauri failed to start");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn airing(preset: Preset) -> Airing {
        let mut station = Station::new("ZSPD");
        station.name = "Shanghai Pudong".into();
        Airing {
            station,
            preset,
            letter: 'A',
            last_metar: "ZSPD 251300Z 09004MPS 9999 FEW030 25/18 Q1013 NOSIG".into(),
        }
    }

    fn preset_using(runway_line: &str) -> Preset {
        Preset {
            name: "west".into(),
            airport_conditions: runway_line.into(),
            ..Default::default()
        }
    }

    /// **切跑道构型不必把席位停掉重上。**
    ///
    /// 重上一次，飞行员在那几十秒里查不到通播，而管制员正在忙着换跑道——
    /// 恰恰是最不该让通播消失的时刻。换完之后送上线的那一份必须是新构型的稿子。
    #[test]
    fn switching_the_configuration_re_renders_from_the_new_preset() {
        let mut a = airing(preset_using("DEPARTURE RUNWAY 16L"));
        let before = render_airing(&a);
        assert!(before.text.contains("16L"));

        a.preset = preset_using("DEPARTURE RUNWAY 34R");
        let after = render_airing(&a);

        assert!(after.text.contains("34R"), "got: {}", after.text);
        assert!(!after.text.contains("16L"));
    }

    /// **报文刷新周期夹在讲得通的范围里。**
    ///
    /// 旧版可调 60–3600 秒。填 5 秒的人会每五秒去问一次服务端的气象缓存；填 0 的
    /// 那个更糟——`tokio::time::interval` 的周期是 0 会直接 panic，而那会把整个
    /// 席位的循环打死，界面上看到的是"上线之后再也没有报文"。
    #[test]
    fn a_metar_refresh_period_outside_what_makes_sense_is_clamped() {
        assert_eq!(clamp_refresh(0), MIN_REFRESH_SECS);
        assert_eq!(clamp_refresh(5), MIN_REFRESH_SECS);
        assert_eq!(clamp_refresh(99_999), MAX_REFRESH_SECS);
        assert_eq!(clamp_refresh(600), 600);
    }

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
