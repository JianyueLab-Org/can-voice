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
//!
//! # 观察员是例外：只连语音，频率可以手输
//!
//! 双人机组的右座（#36，设计 §7.3）不开 FSD 连接——开了网络上就多出一架和机长
//! 叠在一起的飞机——所以他没有"座舱里的 COM1 就是真相"这回事：他未必开着
//! 模拟器，开着的那台也未必调在机长那个频率上。他的频率手输优先、没填才跟本机
//! COM1，位置借机长那架的（`HELLO.follow`）。规则在 [`can_voice_app::observer`]。

mod install;

use can_voice_app::Bridge;
use can_voice_fsd::pilot::{FlightPlan, PilotIdentity, PilotPosition};
use can_voice_fsd::pilot_client::{self, PilotConfig, PilotEvent, PilotHandle};
use can_voice_i18n::Message;
use can_voice_sim::chat::{ChatLog, ChatMessage};
use can_voice_sim::controllers::{ControllerEntry, ControllerTable};
use can_voice_sim::csl::ModelSet;
use can_voice_sim::traffic::{Entry, TrafficTable};
use can_voice_sim::{bridge, xplane, Snapshot};
use can_voice_token::TokenSource;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::Manager;

/// 位置上报的节奏。和 [`pilot_client::POSITION_INTERVAL`] 同一个值——
/// 这里只是把模拟器那一帧喂过去，真正决定发不发的是那边。
const PUMP_INTERVAL: Duration = Duration::from_millis(200);
/// 往插件推一帧的节奏。比位置上报快，插值才有意义。
const PLUGIN_INTERVAL: Duration = Duration::from_millis(50);
/// TCAS 只有 64 个位置。
const MAX_TRAFFIC: usize = 64;
/// 他机显示距离的默认值。超出这个距离的不往插件送——画不出来的飞机白占
/// TCAS 的位置。**可配**：旧版就有 `traffic_range_nm`，帧数紧张的人要能调小。
const DEFAULT_RANGE_NM: u32 = 200;
/// 夹住。填 0 的人看到一片空天会以为程序坏了；填得极大的人会把仅有的 64 个
/// TCAS 位置浪费在屏幕外面的飞机上，近处真正要看的那几架反而被挤掉。
const MIN_RANGE_NM: u32 = 5;
const MAX_RANGE_NM: u32 = 500;
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
    #[serde(default)]
    pub mic_volume: can_voice_settings::VolumePercent,
    #[serde(default)]
    pub speaker_volume: can_voice_settings::VolumePercent,
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
    /// 用户说过"这一版不用再问我"的那个版本号。**跳过的是那一个版本，
    /// 不是从此闭嘴**——下一版照样提示。
    #[serde(default)]
    pub skipped_update: String,
    /// 上次装插件用的 X-Plane 目录。记着是因为自动探测**经常什么也探不到**
    /// （绿色版、搬过目录、装在另一块盘上），那种人每次开窗口都要重填一遍。
    #[serde(default)]
    pub xplane_root: String,
    /// 他机显示距离（海里）。
    ///
    /// **默认值走 `default_range` 而不是 `0`**：老的设置文件里没有这一项，
    /// 反序列化拿到 0 会被夹成最小值，等于一次升级把所有人的可见范围砍到 5 海里。
    #[serde(default = "default_range")]
    pub traffic_range_nm: u32,
    /// CSL 包放在哪。空的表示跟着 X-Plane 目录走。
    #[serde(default)]
    pub csl_dir: String,
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
    /// 观察员模式（双人机组的右座）：只连语音，不上 FSD。**连着的时候改不了**。
    #[serde(default)]
    pub observer: bool,
    /// 观察员跟随的呼号——机长那架飞机的。和 `callsign` 分开存：同一个人换回
    /// 自己飞的时候，呼号框里不该预填着别人的呼号。
    #[serde(default)]
    pub follow: String,
    /// 观察员手输的频率（kHz）。`None` = 跟随本机 COM1。
    #[serde(default)]
    pub observer_frequency: Option<u32>,
}

fn default_range() -> u32 {
    DEFAULT_RANGE_NM
}

/// 把界面上填的数夹进讲得通的范围。
fn clamp_range(nm: u32) -> u32 {
    nm.clamp(MIN_RANGE_NM, MAX_RANGE_NM)
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
    /// 他机显示距离（海里）。和 `inject` 一样是给那条 50 ms 的循环读的，
    /// 所以是原子量不是锁——那条循环不该为了一个数去抢设置的锁。
    traffic_range: Arc<std::sync::atomic::AtomicU32>,
    traffic: Arc<Mutex<TrafficTable>>,
    /// 收发过的文字消息。**攒在这里而不是靠事件推**：窗口重开之前
    /// 管制员说过的话，靠事件流是收不到的。
    chat: Arc<Mutex<ChatLog>>,
    chime: Arc<Chimer>,
    /// 在线管制席位。
    controllers: Arc<Mutex<ControllerTable>>,
    csl: Arc<Mutex<ModelSet>>,
    /// 还在扫 CSL。几个 GB 的包要几十秒，这段时间里"扫到 0 个"不是结论。
    csl_loading: Arc<std::sync::atomic::AtomicBool>,
    http: reqwest::Client,
    ptt: Mutex<Option<can_voice_ptt::PttWatcher>>,
    /// 以观察员身份连着时是跟随的那个呼号；没连、或者正常上着网是 `None`。
    ///
    /// **界面靠它判断观察员"上线了没有"**：观察员没有 FSD，`link` 永远是 `None`，
    /// 只看 `link` 的话连上之后登录表单不消失。
    observing: Mutex<Option<String>>,
    /// 观察员手输的频率（kHz），0 = 没填。给频率那条循环每一拍读，所以是原子量。
    manual_frequency: Arc<std::sync::atomic::AtomicU32>,
    /// 让订阅跟上频率的那条循环。**下线时停掉**：留着的话，换个身份再连上来时
    /// 新旧两条一起改台面——一条跟手输的频率，一条跟 COM1——台面上就有两个
    /// 都开着发射的频率。
    pump: Mutex<Option<tokio::task::AbortHandle>>,
}

impl App {
    pub fn new() -> Self {
        let store = can_voice_settings::Store::for_product("xpc-for-can");
        let settings: Settings = store.load();
        // 在 settings 被移进结构体之前建好。
        let chime = Arc::new(Chimer::new(&settings));
        Self {
            voice: Arc::new(Bridge::new()),
            sim: xplane::Link::spawn(),
            fsd: Mutex::new(None),
            link: Arc::new(Mutex::new(None)),
            observing: Mutex::new(None),
            manual_frequency: Arc::new(std::sync::atomic::AtomicU32::new(saved_manual_frequency(
                &settings,
            ))),
            pump: Mutex::new(None),
            plugin: {
                let slot = Arc::new(Mutex::new(None));
                spawn_plugin_status_reader(slot.clone());
                slot
            },
            store,
            inject: Arc::new(std::sync::atomic::AtomicBool::new(settings.inject)),
            traffic_range: Arc::new(std::sync::atomic::AtomicU32::new(clamp_range(
                settings.traffic_range_nm,
            ))),
            settings: Mutex::new(settings),
            traffic: Arc::new(Mutex::new(TrafficTable::new())),
            chat: Arc::new(Mutex::new(ChatLog::default())),
            chime,
            controllers: Arc::new(Mutex::new(ControllerTable::default())),
            // CSL 在**后台**加载：几个 GB 的包扫一遍要几十秒，放在这里会让
            // 窗口几十秒打不开，而用户看到的是程序卡死。
            csl: Arc::new(Mutex::new(ModelSet::default())),
            // 从第一帧起就是"正在扫"：`run()` 紧接着就会 spawn，而扫之前的 0
            // 不是"一个都没有"。
            csl_loading: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            http: reqwest::Client::builder()
                .user_agent(concat!("xpc-for-can/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            ptt: Mutex::new(None),
        }
    }

    /// 他机显示距离（海里）。给界面和那条 50 ms 的循环共用。
    fn range_nm(&self) -> f64 {
        self.traffic_range
            .load(std::sync::atomic::Ordering::Relaxed)
            .into()
    }

    fn settings_snapshot(&self) -> Settings {
        match self.settings.lock() {
            Ok(s) => s.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    /// 按当前绑定起 PTT 监听。启动时就要拉起来：原来 voice 一开窗口就听绑定。
    fn install_ptt(&self, bindings: Vec<can_voice_ptt::Binding>) {
        let mut slot = match self.ptt.lock() {
            Ok(s) => s,
            Err(p) => p.into_inner(),
        };
        match slot.as_ref() {
            Some(w) => w.set_bindings(bindings),
            None => {
                let watcher = can_voice_ptt::PttWatcher::new(bindings);
                spawn_ptt_pump(self.voice.clone(), watcher.transmitting_flag());
                *slot = Some(watcher);
            }
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

    /// 以观察员身份连着的话，跟随的是谁。
    fn observing(&self) -> Option<String> {
        match self.observing.lock() {
            Ok(o) => o.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    /// 连着没有，不论是哪种身份。**不看 `link`**：它要等 FSD 的第一条事件才有值，
    /// 而观察员压根没有 FSD。
    fn is_online(&self) -> bool {
        self.fsd.lock().map(|f| f.is_some()).unwrap_or(true) || self.observing().is_some()
    }

    /// 观察员手输的频率，没填是 `None`。
    fn manual_frequency(&self) -> Option<u32> {
        Some(
            self.manual_frequency
                .load(std::sync::atomic::Ordering::Relaxed),
        )
        .filter(|&k| k != 0)
    }

    /// 换一条频率循环上来，旧的先停掉。
    fn replace_pump(&self, pump: Option<tokio::task::AbortHandle>) {
        let mut slot = match self.pump.lock() {
            Ok(s) => s,
            Err(p) => p.into_inner(),
        };
        if let Some(old) = std::mem::replace(&mut *slot, pump) {
            old.abort();
        }
    }
}

/// 设置文件里存的手输频率，0 表示没有。**再过一遍波段**：设置文件是人能手改的，
/// 一个波段外的数进了循环，语音会落在谁也不在的频率上。
fn saved_manual_frequency(s: &Settings) -> u32 {
    s.observer_frequency
        .filter(|k| can_voice_app::observer::BAND_KHZ.contains(k))
        .unwrap_or(0)
}

/// CSL 包放在哪儿。环境变量最大——它是给开发和排障用的，不该被设置文件盖掉。
fn csl_root(s: &Settings) -> std::path::PathBuf {
    std::env::var_os("CAN_XPC_CSL_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| csl_dir(&s.csl_dir, &s.xplane_root))
}

/// 设置里填的优先，其次跟着 X-Plane 目录走，都没有才是那条相对路径。
///
/// **跟着 X-Plane 目录走这一步很要紧。** 只剩那条相对路径的话，打包出来的程序
/// 的当前目录是用户双击时所在的目录，几乎注定扫不到；而扫不到的表现是"天上是
/// 空的"，和没装插件、和 UDP 不通在界面上长得一模一样。装插件那一步已经问出了
/// X-Plane 装在哪，不该再让人填第二遍。
fn csl_dir(typed: &str, xplane_root: &str) -> std::path::PathBuf {
    if !typed.trim().is_empty() {
        return std::path::PathBuf::from(typed.trim());
    }
    if !xplane_root.trim().is_empty() {
        return std::path::Path::new(xplane_root.trim())
            .join("Resources")
            .join("plugins")
            .join("CSL");
    }
    std::path::PathBuf::from("Resources/plugins/CSL")
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
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
    /// CSL 扫到了什么。
    pub csl: CslView,
    /// 以观察员身份连着时的状况；没连、或者正常上着网是 `None`。
    pub observer: Option<ObserverView>,
}

/// 观察员那一侧的状况。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ObserverView {
    /// 跟随的呼号。
    pub follow: String,
    /// 语音此刻该在的频率（kHz）。`None` = 还没有频率：没手输，COM1 也读不到。
    ///
    /// **要报出来**：没有频率的观察员连得上、灯是绿的，却什么也听不见。
    pub frequency: Option<u32>,
    /// 这个频率是手输的，不是跟着 COM1 来的。
    pub manual: bool,
}

/// CSL 那一侧的状况。
///
/// **要报出来**：扫不到模型的表现是"天上是空的"，和没装插件、和 UDP 不通在界面
/// 上长得一模一样，而三者要做的事完全不同。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CslView {
    /// 扫的是哪个目录。填错路径的人得看得见自己填的是什么。
    pub root: String,
    /// 扫到几个模型。
    pub models: usize,
    /// 还在扫。几个 GB 的包要几十秒，这段时间里的 0 不是"一个都没有"。
    pub loading: bool,
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

/// 语音连接的配置。`follow` 只有观察员填。
fn voice_config(saved: &Settings, follow: String) -> can_voice_client::Config {
    let (voice_server, voice_name) = saved.endpoints.voice();
    can_voice_client::Config {
        server: voice_server,
        server_name: voice_name,
        token: String::new(),
        client_id: concat!("xpc-for-can/", env!("CARGO_PKG_VERSION")).into(),
        follow,
        // 一人一个账号，没有席位标记：同一个成员号第二次登录顶掉第一条。
        station: String::new(),
        input_device: saved.input_device.clone(),
        output_device: saved.output_device.clone(),
        audio_devices: true,
        extra_roots: Vec::new(),
    }
}

/// 清空台面。**每次上线之前做**：台面在桥里，下线不清——上一次以观察员身份
/// 手输的频率留在上面的话，这一次正常上网络时 COM1 那个频率加上去，台面上
/// 就是两个都开着发射的频率。
fn clear_radios(voice: &Bridge) {
    voice.with_stack(|stack| {
        let tuned: Vec<u32> = stack.radios().iter().map(|r| r.freq_khz).collect();
        for khz in tuned {
            stack.remove(khz);
        }
    });
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
    follow: String,
) -> Result<(), Message> {
    let saved = app.settings_snapshot();
    if saved.observer {
        return connect_observer(&app, &saved, cid, password, follow).await;
    }
    can_voice_fsd::pilot::check_pilot_callsign(&callsign).map_err(|e| e.message())?;

    // 语音先连。凭据只在这里出现一次，换成一张短期票之后就不再需要——
    // 重连带的是票不是密码，所以一个卡在重连里的客户端不会把账号锁出语音。
    let tokens = TokenSource::new(
        &saved.endpoints.api_origin(),
        cid.clone(),
        password.clone(),
        app.http.clone(),
    );
    app.replace_pump(None);
    clear_radios(&app.voice);
    app.voice
        .connect(voice_config(&saved, String::new()), &tokens)
        .await
        .map_err(|e| e.message())?;

    let (fsd_host, fsd_port) = saved.endpoints.fsd();
    let fsd = pilot_client::connect(PilotConfig {
        host: fsd_host,
        port: fsd_port,
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
        app.chime.clone(),
    );
    spawn_asking(fsd.clone(), app.traffic.clone());
    app.replace_pump(Some(spawn_pump(
        fsd.clone(),
        app.sim.clone(),
        app.voice.clone(),
    )));
    spawn_plugin_feed(
        app.sim.clone(),
        app.traffic.clone(),
        app.csl.clone(),
        app.inject.clone(),
        app.traffic_range.clone(),
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

/// 观察员上线：只连语音。
///
/// **不开 FSD 连接**，所以没有他机、没有文字消息、不能拍发计划也不能识别——
/// 那些都是机长那条连接的事。位置借机长那架飞机的：服务端拿 `follow` 去
/// datafeed 里查。
async fn connect_observer(
    app: &App,
    saved: &Settings,
    cid: String,
    password: String,
    follow: String,
) -> Result<(), Message> {
    use can_voice_app::observer;
    let follow = observer::follow_callsign(&follow).map_err(|e| e.message())?;
    let tokens = TokenSource::new(
        &saved.endpoints.api_origin(),
        cid.clone(),
        password,
        app.http.clone(),
    );
    app.replace_pump(None);
    clear_radios(&app.voice);
    app.voice
        .connect(voice_config(saved, follow.clone()), &tokens)
        .await
        .map_err(|e| e.message())?;
    app.replace_pump(Some(spawn_observer_pump(
        app.sim.clone(),
        app.voice.clone(),
        app.manual_frequency.clone(),
    )));
    *app.observing.lock().expect("observing") = Some(follow.clone());
    app.update_settings(|s| {
        s.cid = cid;
        s.follow = follow;
    });
    Ok(())
}

#[tauri::command]
async fn disconnect(app: tauri::State<'_, App>) -> Result<(), String> {
    app.replace_pump(None);
    *app.observing.lock().expect("observing") = None;
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

/// 开 / 关观察员模式。**连着的时候不许改**：身份是上线那一刻定的，一架正在网上
/// 的飞机半路变成观察员，FSD 那条连接断不断都说不通。
#[tauri::command]
fn set_observer(app: tauri::State<'_, App>, on: bool) -> Result<(), Message> {
    set_observer_mode(&app, on)
}

fn set_observer_mode(app: &App, on: bool) -> Result<(), Message> {
    if app.is_online() {
        return Err(Message::new("problem.observer_locked"));
    }
    app.update_settings(|s| s.observer = on);
    Ok(())
}

/// 观察员手输的频率。空的 = 回到跟随 COM1。
///
/// **连着也能改，立刻生效**：频率那条循环每一拍读它。读不出来就不存，把哪里
/// 不对说出来——存进去一个错的数，语音会落在谁也不在的频率上，而界面看着
/// 一切正常。回的是真正存下的那一份，界面照它回填。
#[tauri::command]
fn set_observer_frequency(
    app: tauri::State<'_, App>,
    text: String,
) -> Result<Option<u32>, Message> {
    set_manual_frequency(&app, &text)
}

fn set_manual_frequency(app: &App, text: &str) -> Result<Option<u32>, Message> {
    let khz = can_voice_app::observer::parse_frequency(text).map_err(|e| e.message())?;
    app.manual_frequency
        .store(khz.unwrap_or(0), std::sync::atomic::Ordering::Relaxed);
    app.update_settings(|s| s.observer_frequency = khz);
    Ok(khz)
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
/// 改他机显示距离。**立刻生效**：那条循环每一拍读这个原子量。
#[tauri::command]
fn set_traffic_range(app: tauri::State<'_, App>, nm: u32) -> u32 {
    let nm = clamp_range(nm);
    app.traffic_range
        .store(nm, std::sync::atomic::Ordering::Relaxed);
    app.update_settings(|s| s.traffic_range_nm = nm);
    // 把夹过的那个数还回去：界面上填 9999 之后该看到 500，而不是自己填的那个。
    nm
}

/// 改 CSL 目录，并**立刻重扫**。
///
/// 不重扫的话，填对了路径的人要关掉程序再开一次才看得到飞机，而他刚刚做的
/// 那件事看起来毫无反应。
#[tauri::command]
fn set_csl_dir(app: tauri::State<'_, App>, dir: String) {
    app.update_settings(|s| s.csl_dir = dir);
    spawn_csl_load(
        app.csl.clone(),
        app.csl_loading.clone(),
        csl_root(&app.settings_snapshot()),
    );
}

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
    let com1 = sim.as_ref().and_then(voice_frequency);
    let now = monotonic();
    let origin = sim.as_ref().map(|s| (s.latitude, s.longitude));
    let last_link = app.link.lock().expect("link").clone();
    View {
        sim_connected: app.sim.connected(),
        traffic: app.traffic.lock().expect("traffic").snapshot(
            now,
            origin,
            Some(MAX_TRAFFIC),
            Some(app.range_nm()),
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
        csl: CslView {
            root: csl_root(&app.settings_snapshot()).display().to_string(),
            models: app.csl.lock().expect("csl").len(),
            loading: app.csl_loading.load(std::sync::atomic::Ordering::Relaxed),
        },
        // 和频率循环走的是同一条规则，所以这里算出来的就是那条循环 200 ms 内会收敛到的。
        observer: app.observing().map(|follow| {
            let manual = app.manual_frequency();
            ObserverView {
                follow,
                frequency: can_voice_app::observer::frequency_for(manual, com1),
                manual: manual.is_some(),
            }
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
            let n = match socket.recv_from(&mut buf).await {
                Ok((n, _)) => n,
                // Windows 的 ICMP 回声（见 `can_voice_sim::udp`）不说明插件或这个
                // 端口出了什么事。这条循环没有超时，"超时"在这里就是接着听——
                // 返回的话，这个客户端开着的整段时间里插件都显示成没装。
                Err(e) if can_voice_sim::udp::counts_as_timeout(&e) => continue,
                Err(_) => return,
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
fn send_text(
    app: tauri::State<'_, App>,
    recipient: String,
    message: String,
) -> Result<(), Message> {
    let com1 = app.sim.snapshot().as_ref().and_then(voice_frequency);
    // 收件人和正文在**这里**定下来，然后原样交给 FSD 那一侧——聊天记录里
    // 那一行必须和真正发出去的那一包是同一个答案。
    let out = can_voice_sim::chat::outgoing(&recipient, &message, com1)
        .ok_or_else(|| Message::new("problem.nothing_to_send"))?;
    match app.fsd.lock().expect("fsd").as_ref() {
        Some(fsd) => fsd.send_text(out.to.clone(), out.text.clone()),
        // **没发出去就不记**：记了的话聊天区里那句话看起来发出去了。
        None => return Err(Message::new("problem.offline")),
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

#[tauri::command]
fn set_master_volume(app: tauri::State<'_, App>, mic: u32, speaker: u32) {
    app.update_settings(|s| {
        s.mic_volume = can_voice_settings::VolumePercent::new(mic);
        s.speaker_volume = can_voice_settings::VolumePercent::new(speaker);
    });
    app.voice.set_master_volume(mic, speaker);
}

/// 换一组 PTT 绑定。
///
/// **监听是懒起的，而且起了就停不掉**（`rdev::listen` 没有 stop）。所以只有
/// 真的绑了键盘或鼠标才会去要辅助功能授权——一个只绑了手柄的用户被要求授权
/// 键盘监控，读起来像恶意软件。
#[tauri::command]
fn set_ptt_bindings(app: tauri::State<'_, App>, bindings: Vec<can_voice_ptt::Binding>) {
    app.update_settings(|s| s.ptt = bindings.clone());
    app.install_ptt(bindings);
}

/// 发话灯。看的是交给语音层的 PTT，屏幕按钮和硬件绑定都算。
#[tauri::command]
fn ptt_pressed(app: tauri::State<'_, App>) -> bool {
    app.voice.transmitting()
}

/// "按一下你要的键"。捕获期间事件**不驱动 PTT**——正在录的那一下不能被播出去。
#[tauri::command]
fn begin_ptt_capture(app: tauri::State<'_, App>) {
    app.install_ptt(app.settings_snapshot().ptt);
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

#[tauri::command]
fn cancel_ptt_capture(app: tauri::State<'_, App>) {
    if let Ok(s) = app.ptt.lock() {
        if let Some(w) = s.as_ref() {
            w.cancel_capture();
        }
    }
}

#[tauri::command]
fn ptt_ui_key(app: tauri::State<'_, App>, code: String, pressed: bool) {
    if let Ok(s) = app.ptt.lock() {
        if let Some(w) = s.as_ref() {
            w.handle_ui_key(&code, pressed);
        }
    }
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
    tauri::async_runtime::spawn(async move {
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
fn spawn_pump(fsd: PilotHandle, sim: xplane::Link, voice: Arc<Bridge>) -> tokio::task::AbortHandle {
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

            retune(&voice, &mut current_freq, voice_frequency(&snapshot));
        }
    })
    .abort_handle()
}

/// 让台面落在 `wanted` 上。没变就什么也不做——每一拍都改的话，每一拍都往服务端
/// 推一份全量声明。
///
/// 换频率就是把旧的撤掉、新的加上。飞行员端的台面永远只有一个频率：正常上网络
/// 时是 COM1，观察员是手输的那个或者 COM1。
fn retune(voice: &Bridge, current: &mut Option<u32>, wanted: Option<u32>) {
    if wanted == *current {
        return;
    }
    let previous = std::mem::replace(current, wanted);
    voice.with_stack(|stack| {
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

/// 观察员的频率循环：每 200 ms 看一眼该在哪个频率上。
///
/// **读不到模拟器也照转**，这是和 [`spawn_pump`] 不一样的地方：手输了频率的
/// 观察员可以根本不开模拟器，照搬那条"没有模拟器那一帧就跳过"的话，这种人
/// 永远订阅不上任何频率。
fn spawn_observer_pump(
    sim: xplane::Link,
    voice: Arc<Bridge>,
    manual: Arc<std::sync::atomic::AtomicU32>,
) -> tokio::task::AbortHandle {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PUMP_INTERVAL);
        let mut current: Option<u32> = None;
        loop {
            tick.tick().await;
            let typed = Some(manual.load(std::sync::atomic::Ordering::Relaxed)).filter(|&k| k != 0);
            let com1 = sim.snapshot().as_ref().and_then(voice_frequency);
            retune(
                &voice,
                &mut current,
                can_voice_app::observer::frequency_for(typed, com1),
            );
        }
    })
    .abort_handle()
}

/// 每 50 ms 往插件推一帧。比位置上报快，插值才有意义。
fn spawn_plugin_feed(
    sim: xplane::Link,
    table: Arc<Mutex<TrafficTable>>,
    csl: Arc<Mutex<ModelSet>>,
    inject: Arc<std::sync::atomic::AtomicBool>,
    range: Arc<std::sync::atomic::AtomicU32>,
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
            // 每一拍读一次：改了距离要立刻生效，而不是等下次开程序。
            let max_range = f64::from(range.load(std::sync::atomic::Ordering::Relaxed));
            let entries = {
                let mut table = table.lock().expect("traffic");
                table.prune(now);
                let entries = table.snapshot(now, origin, Some(MAX_TRAFFIC), Some(max_range));
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
                table.snapshot(now, origin, Some(MAX_TRAFFIC), Some(max_range))
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
///
/// 扫完（哪怕一个也没扫到）都要把 `loading` 放下来：界面靠它区分"还在扫"
/// 和"扫完了，没有"。
fn spawn_csl_load(
    csl: Arc<Mutex<ModelSet>>,
    loading: Arc<std::sync::atomic::AtomicBool>,
    root: std::path::PathBuf,
) {
    loading.store(true, std::sync::atomic::Ordering::Relaxed);
    std::thread::spawn(move || {
        let loaded = can_voice_sim::csl::load(&root);
        if loaded.is_empty() {
            tracing::warn!(
                root = %root.display(),
                "no CSL models found; other aircraft will not be drawn.                  set CAN_XPC_CSL_DIR if the packages live elsewhere"
            );
        }
        *csl.lock().expect("csl") = loaded;
        loading.store(false, std::sync::atomic::Ordering::Relaxed);
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
    let (skipped, busy) = {
        let s = match app.settings.lock() {
            Ok(s) => s.clone(),
            Err(p) => p.into_inner().clone(),
        };
        // 上着网就是"正在工作"。观察员没有 FSD 链路，但他同样戴着耳机在听。
        let busy = app.link.lock().expect("link").is_some() || app.observing().is_some();
        (s.skipped_update, busy)
    };
    let origin = app.settings_snapshot().endpoints.api_origin();
    let Some(latest) =
        can_voice_update::check(&app.http, &origin, "xpc-for-can", env!("CARGO_PKG_VERSION")).await
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
    app.update_settings(|s| s.skipped_update = version);
}

/// 用系统浏览器打开下载页。**绝不自动更新**：装不装、什么时候装是人决定的。
#[tauri::command]
fn open_download(url: String) -> Result<(), Message> {
    can_voice_update::open_in_browser(&url)
}

// ——— X-Plane 插件 ———

/// 看哪个目录：界面上填的优先，其次是上次记住的，都没有才去自动探测。
///
/// 空白当作没填——一个被清空的输入框不该把人锁在旧目录上。
fn chosen_root(typed: Option<String>, remembered: &str) -> Option<String> {
    typed
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .or_else(|| (!remembered.is_empty()).then(|| remembered.to_string()))
}

/// 自动探测到的 X-Plane 目录。可能一个都没有，那时界面靠手填那一栏。
#[tauri::command]
fn xplane_installs() -> Vec<String> {
    install::find_installs()
}

/// 插件装了没有、是不是最新、协议号对不对得上。
#[tauri::command]
fn plugin_install_status(app: tauri::State<'_, App>, root: Option<String>) -> install::Status {
    let remembered = app.settings_snapshot().xplane_root;
    let root = chosen_root(root, &remembered);
    install::inspect(root.as_deref().map(std::path::Path::new))
}

/// 装（或者覆盖）插件，返回它落在哪。
///
/// 装成功才记住这个目录：填错了路径的人不该在下次开窗口时还看着那一条。
#[tauri::command]
fn install_plugin(app: tauri::State<'_, App>, root: String) -> Result<String, Message> {
    let path = install::install(std::path::Path::new(&root))?;
    app.update_settings(|s| s.xplane_root = root);
    Ok(path.display().to_string())
}

/// 安装目录里带着的那份插件在哪个文件夹。没有就是 `None`。
///
/// 应用内安装写不进去时界面拿它给人指路。**位置在运行时问 Tauri**，不写死：
/// Windows 是程序目录，deb/rpm 是 `/usr/lib/xpc-for-can`，AppImage 是运行时才挂上的
/// 那个镜像，开发时又是 `target/debug`。
#[tauri::command]
fn bundled_plugin_dir(handle: tauri::AppHandle) -> Option<String> {
    let resources = handle.path().resource_dir().ok()?;
    install::bundled_copy_dir(&resources).map(|p| p.display().to_string())
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

// ——— 设置对话框、置顶、精简（#45）———

/// 精简模式下窗口最小能缩到多小。
const COMPACT_MIN: (f64, f64) = (320.0, 220.0);
/// 按下"精简"那一刻缩成多大。**不缩的话**，东西藏起来了窗口却还是那么大，
/// 人还得自己去拖——而这个开关存在的全部理由就是一下子压到雷达屏的角落里。
const COMPACT_SIZE: (f64, f64) = (460.0, 340.0);

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
        ("voice_server", can_voice_settings::endpoints::VOICE_SERVER),
        ("fsd_server", can_voice_settings::endpoints::FSD_SERVER),
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
    let saved: Settings = can_voice_settings::Store::for_product("xpc-for-can").load();
    can_voice_log::init(
        "xpc-for-can",
        env!("CARGO_PKG_VERSION"),
        std::env::args().any(|a| a == "--debug") || saved.debug_log,
    );

    let app = App::new();
    spawn_csl_load(
        app.csl.clone(),
        app.csl_loading.clone(),
        csl_root(&app.settings_snapshot()),
    );
    tauri::Builder::default()
        .manage(app)
        // 置顶和精简在窗口一出来就还原。压在雷达屏上用的人不该每次启动都再点一遍。
        .setup(|handle| {
            let app = handle.state::<App>();
            let saved = app.settings_snapshot();
            app.voice
                .set_master_volume(saved.mic_volume.get(), saved.speaker_volume.get());
            app.install_ptt(saved.ptt);
            let appearance = app.settings_snapshot().appearance;
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
            xplane_installs,
            plugin_install_status,
            install_plugin,
            bundled_plugin_dir,
            connect,
            disconnect,
            set_observer,
            set_observer_frequency,
            view,
            send_text,
            ident,
            file_flight_plan,
            settings,
            set_injection,
            set_traffic_range,
            set_csl_dir,
            set_audio_devices,
            set_message_sound,
            set_message_sound_all,
            set_message_sound_volume,
            preview_chime,
            keyboard_ptt_supported,
            ptt_bindings,
            audio_devices,
            set_transmitting,
            set_master_volume,
            set_ptt_bindings,
            ptt_pressed,
            begin_ptt_capture,
            cancel_ptt_capture,
            ptt_ui_key,
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

    /// 看哪个目录：界面上填的优先，其次是上次记住的，都没有才去自动探测。
    ///
    /// **记住的那个必须能被覆盖**：搬过目录、装了第二份 X-Plane 的人，界面上填了
    /// 新路径却还在看旧的，是那种"点了没反应"的故障。空白当作没填——一个被清空
    /// 的输入框不该把人锁在旧目录上。
    #[test]
    fn the_typed_root_wins_over_the_remembered_one() {
        assert_eq!(
            chosen_root(Some("/games/XP12".into()), "/old"),
            Some("/games/XP12".to_string())
        );
        assert_eq!(
            chosen_root(Some("   ".into()), "/old"),
            Some("/old".to_string())
        );
        assert_eq!(chosen_root(None, "/old"), Some("/old".to_string()));
        // 两个都没有：交给自动探测，而不是拿一个空路径去看。
        assert_eq!(chosen_root(None, ""), None);
    }

    /// CSL 包在哪：设置里填的优先，其次跟着 X-Plane 目录走，都没有才是那条相对路径。
    ///
    /// **跟着 X-Plane 目录走这一步是新的。** 默认值原来只有那条相对路径，而打包
    /// 出来的程序的当前目录是用户双击时所在的目录，几乎注定扫不到；扫不到的表现
    /// 是"天上是空的"——和没装插件、和 UDP 不通在界面上长得一模一样。既然装插件
    /// 那一步已经问出了 X-Plane 装在哪，就不该再让人填第二遍。
    #[test]
    fn the_csl_directory_follows_the_x_plane_root_when_nobody_typed_one() {
        assert_eq!(
            csl_dir("/disk2/CSL", "/games/XP12"),
            std::path::PathBuf::from("/disk2/CSL")
        );
        assert_eq!(
            csl_dir("", "/games/XP12"),
            std::path::Path::new("/games/XP12")
                .join("Resources")
                .join("plugins")
                .join("CSL")
        );
        assert_eq!(
            csl_dir("", ""),
            std::path::PathBuf::from("Resources/plugins/CSL")
        );
    }

    /// 他机显示距离夹在讲得通的范围里。
    ///
    /// 填 0 的人看到一片空天会以为程序坏了；填 9999 的人会把 TCAS 仅有的 64 个
    /// 位置浪费在屏幕外面的飞机上，近处真正要看的那几架反而被挤掉。
    #[test]
    fn a_traffic_range_outside_what_makes_sense_is_clamped() {
        assert_eq!(clamp_range(0), MIN_RANGE_NM);
        assert_eq!(clamp_range(9999), MAX_RANGE_NM);
        assert_eq!(clamp_range(80), 80);
    }

    /// 扫到了几个 CSL 模型要报出来。
    ///
    /// 扫不到的表现是"天上是空的"——和没装插件、和 UDP 不通在界面上长得一模一样，
    /// 而三者要做的事完全不同。
    #[test]
    fn the_view_says_how_many_csl_models_were_found() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let _guard = rt.enter();
        let app = App::new();
        *app.csl.lock().expect("csl") =
            can_voice_sim::csl::ModelSet::new(vec![can_voice_sim::csl::Model {
                name: "B738".into(),
                icao: "B738".into(),
                ..Default::default()
            }]);

        assert_eq!(build_view(&app).csl.models, 1);
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

    // ——— 观察员（#36）———

    /// 一个设置写到仓库 `.temp/` 里的 App。**不能用真的那份**：这几条测试会改设置，
    /// 而 `for_product` 指的是开发机上真在用的那个设置文件。
    fn app_with_scratch_settings(name: &str) -> (App, std::path::PathBuf) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../.temp/xpc-tests")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        let mut app = App::new();
        app.store = can_voice_settings::Store::at(dir.join("settings.json"));
        (app, dir)
    }

    /// 老的设置文件里没有这三项。读进来必须是"不是观察员"，而不是读失败退回默认值
    /// 把别的设置一起丢掉。
    #[test]
    fn a_settings_file_from_before_observer_mode_still_loads() {
        let s: Settings =
            serde_json::from_str(r#"{"cid":"1234567","callsign":"CES123"}"#).expect("parse");
        assert_eq!(s.cid, "1234567");
        assert!(!s.observer);
        assert_eq!(s.follow, "");
        assert_eq!(s.observer_frequency, None);
    }

    /// **观察员没有 FSD，`link` 永远是 `None`**，而前端的 `online` 原本全由它推导。
    /// 不单独报出来的话，观察员连上之后登录表单不消失。
    #[test]
    fn an_observer_shows_as_online_without_an_fsd_link() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let _guard = rt.enter();
        let (app, dir) = app_with_scratch_settings("online");
        assert!(build_view(&app).observer.is_none());
        assert!(!app.is_online());

        *app.observing.lock().expect("observing") = Some("CCA1501".into());

        let v = build_view(&app);
        assert_eq!(v.link, None);
        assert_eq!(v.observer.expect("observer").follow, "CCA1501");
        assert!(app.is_online());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 身份是上线那一刻定的：连着的时候切换观察员模式要被拒绝，而且不能存进去。
    #[test]
    fn observer_mode_cannot_be_switched_while_connected() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let _guard = rt.enter();
        let (app, dir) = app_with_scratch_settings("switch");
        assert_eq!(set_observer_mode(&app, true), Ok(()));
        assert!(app.settings_snapshot().observer);

        *app.observing.lock().expect("observing") = Some("CCA1501".into());

        assert!(set_observer_mode(&app, false).is_err());
        assert!(app.settings_snapshot().observer);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 手输的频率立刻进快照（频率循环读的是同一个原子量），清空就回到跟随 COM1。
    /// 没开模拟器、也没手输的观察员要报"没有频率"，而不是报一个它不在的频率。
    #[test]
    fn a_typed_frequency_takes_effect_and_clearing_it_follows_com1_again() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let _guard = rt.enter();
        let (app, dir) = app_with_scratch_settings("typed");
        *app.observing.lock().expect("observing") = Some("CCA1501".into());

        assert_eq!(set_manual_frequency(&app, "121.8"), Ok(Some(121_800)));
        let o = build_view(&app).observer.expect("observer");
        assert_eq!(o.frequency, Some(121_800));
        assert!(o.manual);
        assert_eq!(app.settings_snapshot().observer_frequency, Some(121_800));

        assert_eq!(set_manual_frequency(&app, ""), Ok(None));
        let o = build_view(&app).observer.expect("observer");
        assert!(!o.manual);
        // 测试里没有 X-Plane，COM1 读不到。
        assert_eq!(o.frequency, None);
        assert_eq!(app.settings_snapshot().observer_frequency, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 打错的字不存，已经生效的那个频率也不动：存进去的话语音会落在谁也不在的频率上。
    #[test]
    fn a_typo_does_not_replace_the_frequency_in_use() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let _guard = rt.enter();
        let (app, dir) = app_with_scratch_settings("typo");
        assert_eq!(set_manual_frequency(&app, "124.350"), Ok(Some(124_350)));

        assert!(set_manual_frequency(&app, "124,35").is_err());
        assert!(set_manual_frequency(&app, "140.000").is_err());

        assert_eq!(app.manual_frequency(), Some(124_350));
        assert_eq!(app.settings_snapshot().observer_frequency, Some(124_350));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// 设置文件是人能手改的，波段外的数不进频率循环。
    #[test]
    fn a_saved_frequency_outside_the_band_is_ignored() {
        let with = |khz| Settings {
            observer_frequency: khz,
            ..Default::default()
        };
        assert_eq!(saved_manual_frequency(&with(Some(121_800))), 121_800);
        assert_eq!(saved_manual_frequency(&with(Some(99_500))), 0);
        assert_eq!(saved_manual_frequency(&with(None)), 0);
    }

    /// **飞行员端的台面永远只有一个频率。** 换频率时旧的必须撤掉——留着的话两个
    /// 频率都开着发射，一按 PTT 两边一起说。
    #[test]
    fn retuning_leaves_exactly_one_frequency() {
        let voice = Bridge::new();
        let mut current = None;

        retune(&voice, &mut current, Some(121_800));
        retune(&voice, &mut current, Some(124_350));

        let radios = voice.radios();
        assert_eq!(radios.len(), 1);
        assert_eq!(radios[0].freq_khz, 124_350);
        assert!(radios[0].rx && radios[0].tx);

        retune(&voice, &mut current, None);
        assert!(voice.radios().is_empty());
    }

    /// 台面在桥里，下线不清。上一次以观察员身份手输的频率留着的话，这一次正常
    /// 上网络时 COM1 加上去，台面上就是两个频率。
    #[test]
    fn going_online_again_starts_from_an_empty_stack() {
        let voice = Bridge::new();
        let mut observer_session = None;
        retune(&voice, &mut observer_session, Some(121_800));

        clear_radios(&voice);
        let mut pilot_session = None;
        retune(&voice, &mut pilot_session, Some(124_350));

        let radios = voice.radios();
        assert_eq!(radios.len(), 1);
        assert_eq!(radios[0].freq_khz, 124_350);
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
