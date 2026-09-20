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
use can_voice_client::stack::{Radio, RadioStack};
use can_voice_client::Config;
use can_voice_datafeed::Position;
use can_voice_i18n::Message;
use can_voice_token::TokenSource;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;

/// 多久去问一次 datafeed。和旧版一样 60 秒。
///
/// 上席位、下席位都不是每秒会变的事，而这是一个挡着 Cloudflare 的公共端点：
/// 问得太密对谁都没有好处。
const FEED_INTERVAL: Duration = Duration::from_secs(60);

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
    /// 麦克风总音量，0–200，100 是原声。
    #[serde(default)]
    pub mic_volume: can_voice_settings::VolumePercent,
    /// 喇叭总音量，0–200，100 是原声。
    #[serde(default)]
    pub speaker_volume: can_voice_settings::VolumePercent,
    /// 用户说过"这一版不用再问我"的那个版本号。**跳过的是那一个版本，
    /// 不是从此闭嘴**——下一版照样提示。
    #[serde(default)]
    pub skipped_update: String,
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

/// 应用的运行时状态。
pub struct App {
    bridge: Arc<Bridge>,
    http: reqwest::Client,
    /// PTT 监听。**懒起**：只有真的绑了键盘或鼠标才会去要辅助功能授权。
    ptt: std::sync::Mutex<Option<can_voice_ptt::PttWatcher>>,
    store: can_voice_app::Store,
    settings: std::sync::Mutex<Settings>,
    /// 数据源上的最新结论。界面读的就是它。
    feed: Arc<std::sync::Mutex<FeedView>>,
    /// 用户**手工删掉过**的频率。它们不会被自动加回来。
    user_removed: Arc<std::sync::Mutex<HashSet<u32>>>,
    /// 那条 60 秒的轮询。上线时起，下线时停。
    feed_task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl App {
    pub fn new() -> Self {
        // 产品名就是设置目录名。四个产品名是 can-api 的固定白名单，
        // 拿它当目录名等于不再多发明一个。
        let store = can_voice_app::Store::for_product("audio-for-can");
        let settings: Settings = store.load();
        let bridge = Arc::new(Bridge::new());
        // 台面不从磁盘恢复。原来 voice 每次启动都是空的：频率从数据源来，
        // 上一场的临时频道多半已经没人，留着看起来一切正常。
        bridge.set_master_volume(settings.mic_volume.get(), settings.speaker_volume.get());
        Self {
            bridge,
            http: reqwest::Client::builder()
                .user_agent(concat!("audio-for-can/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            ptt: std::sync::Mutex::new(None), // setup 里按已存绑定拉起来，和原来 voice 一样
            store,
            settings: std::sync::Mutex::new(settings),
            feed: Arc::new(std::sync::Mutex::new(FeedView::default())),
            user_removed: Arc::new(std::sync::Mutex::new(HashSet::new())),
            feed_task: std::sync::Mutex::new(None),
        }
    }

    /// 按当前绑定起 PTT 监听。启动时就要拉起来：原来 voice 一开窗口就听绑定，
    /// 不是进一次设置才生效。
    fn install_ptt(&self, bindings: Vec<can_voice_ptt::Binding>) {
        let mut slot = match self.ptt.lock() {
            Ok(s) => s,
            Err(p) => p.into_inner(),
        };
        match slot.as_ref() {
            Some(w) => w.set_bindings(bindings),
            None => {
                let watcher = can_voice_ptt::PttWatcher::new(bindings);
                spawn_ptt_pump(self.bridge.clone(), watcher.transmitting_flag());
                *slot = Some(watcher);
            }
        }
    }

    /// 停掉那条轮询。
    ///
    /// **下线时必须停**：不停的话，下了线的客户端每 60 秒还在替一个已经不在的
    /// 会话改台面，而用户看到的是"我明明下线了，频率还在自己动"。
    fn stop_feed(&self) {
        if let Some(task) = locked(&self.feed_task).take() {
            task.abort();
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
///
/// 运行时不再恢复上一场的台面（和原来 voice 一样，启动是空栈）。测试还走这里。
#[cfg(test)]
fn restore_stack(stack: &mut can_voice_client::stack::RadioStack, saved: &[Radio]) {
    for r in saved {
        stack.add_named(r.freq_khz, &r.callsign);
        stack.set_rx(r.freq_khz, r.rx);
        stack.set_tx(r.freq_khz, r.tx);
        stack.set_xc(r.freq_khz, r.xc);
        stack.set_gain(r.freq_khz, r.gain);
        stack.set_muted(r.freq_khz, r.muted);
        if r.selected {
            stack.set_selected(r.freq_khz);
        }
    }
}

/// 拿一把锁，中毒了也照用。
///
/// 这里面装的都是纯数据（一组频率号、一份快照），没有"改到一半"的不变量；
/// 放弃它只会让一个已经出过错的程序连界面都不再更新。
fn locked<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

// ——— 数据源 ———
//
// **语音服务端不知道谁在管哪个席位，也不该知道。** 那是 FSD 的事实，只在
// datafeed 上。所以"本席频率"这件事只能在客户端查出来：查到了就自动加进台面，
// 查不到就说明这个人此刻没在管制，不该发射。
//
// 旧版（`can-audio/controller/gui.py`）就是这么做的，新版一条都没有——症状是
// 管制员要自己记住并手敲本席频率，忘了加就是"我在 121.8 守着"而实际没订阅，
// 而且谁都可以在任何频率上发射。

/// 数据源上关于"我"的结论。界面直接照着画。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct DutyView {
    /// 在管的席位呼号。空 = 此刻不在管制。
    pub callsign: String,
    /// 那个席位的频率。
    pub freq_khz: Option<u32>,
    /// 这一轮有没有真的把发射关掉过。
    ///
    /// 只有真关掉了才该跟用户说"你已经不在席位上了，发射已关闭"——一句没有
    /// 对应事实的警告，下一次就没人看了。
    pub dropped_tx: bool,
}

/// 界面读的那一份数据源快照。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct FeedView {
    pub duty: DutyView,
    /// 此刻在线、守得住的席位，按频率排。
    pub online: Vec<Position>,
    /// CAN 号 → 呼号。
    ///
    /// 语音那一侧下发的"谁在说话"是一个 CAN 号；电台行上要显示的是呼号。
    pub roster: HashMap<String, String>,
    /// 此刻允不允许发射。
    ///
    /// **由 [`feed`] 命令在读的那一刻填，不是轮询存进来的**：它是台面自己的状态，
    /// 存一份副本就会有两个真相，而对不上的那一刻界面画的是灰的、实际却发得出去。
    pub transmit_allowed: bool,
    /// 最近一次取 datafeed 成功了没有。
    ///
    /// **取不到不等于下席位**，界面要把这两件事分开说：前者是"查不到"，
    /// 后者是"确实没在管制"，而处理方法完全不同。
    pub reachable: bool,
}

/// 把一次 datafeed 的结论落到台面上。
///
/// `feed` 是 `None` 表示**这一轮没取到**——那就什么都不动，返回 `None`。上游抖
/// 一下就把一个正在管制的人的发射权收走，比"自动加频率不工作"糟得多：他还坐在
/// 席位上，飞行员还在那个频率上叫他。
///
/// `user_removed` 是用户手工删掉过的频率。**它们不会被加回来**——每 60 秒跟用户
/// 抢一次是最招人烦的那种智能。
fn apply_feed(
    stack: &mut RadioStack,
    cid: &str,
    feed: Option<&serde_json::Value>,
    user_removed: &HashSet<u32>,
) -> Option<FeedView> {
    let feed = feed?;
    let mine = can_voice_datafeed::controller_for(cid, feed);

    // 顺序是承重的：先放行再开 TX，否则 `set_tx` 会被发射权那道闸拦掉。
    let dropped_tx = stack.set_transmit_allowed(mine.is_some());
    stack.set_locked(mine.as_ref().map(|p| p.freq_khz));

    if let Some(p) = &mine {
        if !user_removed.contains(&p.freq_khz) {
            let known = stack.radios().iter().any(|r| r.freq_khz == p.freq_khz);
            // 呼号每一轮都补：频率常常先被手工加进来，过一轮才知道那上面是谁。
            stack.add_named(p.freq_khz, &p.callsign);
            if !known {
                // **只有第一次才动开关。** 管制员可能刚刚有意把 TX 关掉（换班
                // 交接、跨席位借用）；自动加是为了不让人忘了加，不是不让人改。
                stack.set_tx(p.freq_khz, true);
                stack.set_selected(p.freq_khz);
                tracing::info!(freq_khz = p.freq_khz, callsign = %p.callsign,
                    "adopted the frequency of the position being staffed");
            }
        }
    }

    Some(FeedView {
        duty: DutyView {
            callsign: mine
                .as_ref()
                .map(|p| p.callsign.clone())
                .unwrap_or_default(),
            freq_khz: mine.as_ref().map(|p| p.freq_khz),
            dropped_tx,
        },
        online: can_voice_datafeed::online_positions(feed),
        roster: can_voice_datafeed::roster(feed),
        // 读的那一刻才填，见 `FeedView::transmit_allowed`。
        transmit_allowed: false,
        reachable: true,
    })
}

/// 每 60 秒问一次数据源，把结论落到台面上。
///
/// 只在上着线的时候跑。第一轮**立刻**跑：刚上线的人正等着他的席位频率出现，
/// 让他先等满一分钟是那种"看起来没反应"的故障。
fn spawn_feed(app: &App, cid: String) {
    let bridge = app.bridge.clone();
    let url = can_voice_settings::endpoints::endpoint(
        "CAN_FSD_DATAFEED",
        &app.settings().endpoints.datafeed_url,
        can_voice_datafeed::DEFAULT_URL,
    );
    let http = app.http.clone();
    let slot = app.feed.clone();
    let removed = app.user_removed.clone();
    let task = tokio::spawn(async move {
        let mut tick = tokio::time::interval(FEED_INTERVAL);
        loop {
            tick.tick().await;
            let feed = can_voice_datafeed::fetch(&http, &url).await;
            let removed = locked(&removed).clone();
            let view = bridge.with_stack(|s| apply_feed(s, &cid, feed.as_ref(), &removed));
            match view {
                Some(v) => *locked(&slot) = v,
                // 取不到就只把"查不到"这件事说出来，别的一律不动。
                None => locked(&slot).reachable = false,
            }
        }
    });
    if let Some(old) = locked(&app.feed_task).replace(task) {
        old.abort();
    }
}

/// 界面读的那一份数据源快照。
#[tauri::command]
fn feed(state: tauri::State<'_, App>) -> FeedView {
    let mut v = locked(&state.feed).clone();
    v.transmit_allowed = state.bridge.transmit_allowed();
    v
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
) -> Result<(), Message> {
    let remembered = cid.clone();
    let tokens = TokenSource::new(
        &state.settings().endpoints.api_origin(),
        cid,
        password,
        state.http.clone(),
    );
    let (server, server_name) = state.settings().endpoints.voice();
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
        .map_err(|e| e.message())?;
    // 连上了才记住这个号——连不上的那个多半是打错了。
    state.update_settings(|s| s.cid = remembered.clone());
    // 席位频率只有数据源知道。这条起来之前，界面上那句"你不在席位上"是"还没查"
    // 而不是结论——`FeedView::reachable` 就是用来分开这两件事的。
    spawn_feed(&state, remembered);
    Ok(())
}

#[tauri::command]
async fn disconnect(state: tauri::State<'_, App>) -> Result<(), String> {
    state.stop_feed();
    // 下线就不再"在席位上"了，那道闸也就没有意义：它拦的是"上着线却不在席位上
    // 发射"。**下了线反而要放开**——不放的话，一个下线的人连自己的台面都摆不了，
    // 而他多半正是为了下一次上线在摆。锁也解开：留着的话那个频率删不掉。
    state.bridge.with_stack(|s| {
        s.set_transmit_allowed(true);
        s.set_locked(None);
    });
    *locked(&state.feed) = FeedView::default();
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

/// 加一个频率。`callsign` 是从在线一览里点过来时带上的，手敲的那条是空的。
///
/// 加回来就把"用户删过它"那条记录清掉：不然自动加频率对这个频率永久失效，
/// 而那只有下一次重开程序才会恢复。
#[tauri::command]
fn add_frequency(state: tauri::State<'_, App>, freq_khz: u32, callsign: Option<String>) {
    let callsign = callsign.unwrap_or_default();
    state
        .bridge
        .with_stack(|s| s.add_named(freq_khz, &callsign));
    locked(&state.user_removed).remove(&freq_khz);
    state.update_settings(|_| {});
}

/// 删一个频率。
///
/// **正在管的那个席位频率删不掉**，返回 `false` 而不是报错——界面上那个按钮本来
/// 就该是灰的，走到这里多半是热键或者别的路子。
///
/// 删掉的记下来：**不会每 60 秒被自动加回来**。每一分钟跟用户抢一次是最招人烦的
/// 那种智能。
#[tauri::command]
fn remove_frequency(state: tauri::State<'_, App>, freq_khz: u32) -> bool {
    let removed = state.bridge.with_stack(|s| s.remove(freq_khz));
    if removed {
        locked(&state.user_removed).insert(freq_khz);
    }
    state.update_settings(|_| {});
    removed
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

/// 单频静音。**和关 RX 是两件事**：静音只是不播出来，包照收、灯照亮；
/// 关 RX 是退订，下一次有人叫你时连灯都不亮。
#[tauri::command]
fn set_muted(state: tauri::State<'_, App>, freq_khz: u32, on: bool) {
    state.bridge.set_muted(freq_khz, on);
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

#[tauri::command]
fn set_master_volume(state: tauri::State<'_, App>, mic: u32, speaker: u32) {
    state.update_settings(|s| {
        s.mic_volume = can_voice_settings::VolumePercent::new(mic);
        s.speaker_volume = can_voice_settings::VolumePercent::new(speaker);
    });
    state.bridge.set_master_volume(mic, speaker);
}

/// 换一组 PTT 绑定。
///
/// **监听是懒起的，而且起了就停不掉**——`rdev::listen` 没有 stop。所以只有真的绑了
/// 键盘或鼠标才会去要辅助功能授权（macOS 上那是一个弹窗，而一个只绑了手柄的用户
/// 被要求授权键盘监控，读起来像恶意软件）。
#[tauri::command]
fn set_ptt_bindings(state: tauri::State<'_, App>, bindings: Vec<can_voice_ptt::Binding>) {
    state.update_settings(|s| s.ptt = bindings.clone());
    state.install_ptt(bindings);
}

/// 发话灯。看的是交给语音层的 PTT，屏幕按钮和硬件绑定都算。
#[tauri::command]
fn ptt_pressed(state: tauri::State<'_, App>) -> bool {
    state.bridge.transmitting()
}

/// "按一下你要的键"。捕获期间事件**不驱动 PTT**——正在录的那一下不能被播出去。
#[tauri::command]
fn begin_ptt_capture(state: tauri::State<'_, App>) {
    state.install_ptt(state.settings().ptt);
    if let Ok(s) = state.ptt.lock() {
        if let Some(w) = s.as_ref() {
            w.begin_capture();
        }
    }
}

#[tauri::command]
fn take_captured_binding(state: tauri::State<'_, App>) -> Option<can_voice_ptt::Binding> {
    state
        .ptt
        .lock()
        .ok()
        .and_then(|s| s.as_ref().and_then(|w| w.take_captured()))
}

#[tauri::command]
fn cancel_ptt_capture(state: tauri::State<'_, App>) {
    if let Ok(s) = state.ptt.lock() {
        if let Some(w) = s.as_ref() {
            w.cancel_capture();
        }
    }
}

/// 窗口内的按键。绑定时不走全局钩子：macOS 没辅助功能时 rdev 会静默丢事件。
#[tauri::command]
fn ptt_ui_key(state: tauri::State<'_, App>, code: String, pressed: bool) {
    if let Ok(s) = state.ptt.lock() {
        if let Some(w) = s.as_ref() {
            w.handle_ui_key(&code, pressed);
        }
    }
}

/// 把 PTT 的按下状态泵给语音层。
///
/// 一帧一拍（20 毫秒）：比帧还快没有意义，慢了会让发话的头尾被切掉。
fn spawn_ptt_pump(
    bridge: std::sync::Arc<Bridge>,
    flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    tauri::async_runtime::spawn(async move {
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
fn set_audio_devices(state: tauri::State<'_, App>, input: Option<String>, output: Option<String>) {
    state
        .bridge
        .set_audio_devices(input.clone(), output.clone());
    state.update_settings(|s| {
        s.input_device = input;
        s.output_device = output;
    });
}

#[tauri::command]
async fn test_speaker(state: tauri::State<'_, App>) -> Result<(), String> {
    let s = state.settings();
    let output = s.output_device.clone();
    let gain = s.speaker_volume.get() as f32 / 100.0;
    tauri::async_runtime::spawn_blocking(move || {
        can_voice_client::audio::speaker_test(output.as_deref(), gain).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn test_mic(state: tauri::State<'_, App>) -> Result<(), String> {
    let s = state.settings();
    let input = s.input_device.clone();
    let output = s.output_device.clone();
    let mic = s.mic_volume.get() as f32 / 100.0;
    let spk = s.speaker_volume.get() as f32 / 100.0;
    tauri::async_runtime::spawn_blocking(move || {
        can_voice_client::audio::mic_test(input.as_deref(), output.as_deref(), mic, spk)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
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
    let (skipped, busy) = {
        let s = match app.settings.lock() {
            Ok(s) => s.clone(),
            Err(p) => p.into_inner().clone(),
        };
        // 连着的时候就是"正在工作"：PTT 是全局热键，弹窗抢了焦点就按不出去了。
        let busy = matches!(
            app.bridge.snapshot().link,
            can_voice_client::LinkState::Online
        );
        (s.skipped_update, busy)
    };
    let origin = app.settings().endpoints.api_origin();
    let Some(latest) = can_voice_update::check(
        &app.http,
        &origin,
        "audio-for-can",
        env!("CARGO_PKG_VERSION"),
    )
    .await
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
) -> Result<(), Message> {
    let origin = app.settings().endpoints.api_origin();
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

// ——— 设置对话框、置顶、精简（#45）———

/// 精简模式下窗口最小能缩到多小。
const COMPACT_MIN: (f64, f64) = (440.0, 180.0);
/// 按下"精简"那一刻缩成多大。**不缩的话**，东西藏起来了窗口却还是那么大，
/// 人还得自己去拖——而这个开关存在的全部理由就是一下子压到雷达屏的角落里。
const COMPACT_SIZE: (f64, f64) = (460.0, 320.0);

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
    let was_compact = app.settings().appearance.compact;
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
        ("datafeed_url", can_voice_datafeed::DEFAULT_URL),
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
    let saved: Settings = can_voice_settings::Store::for_product("audio-for-can").load();
    can_voice_log::init(
        "audio-for-can",
        env!("CARGO_PKG_VERSION"),
        std::env::args().any(|a| a == "--debug") || saved.debug_log,
    );

    tauri::Builder::default()
        .manage(App::new())
        // 置顶和精简在窗口一出来就还原。压在雷达屏上用的人不该每次启动都再点一遍。
        .setup(|handle| {
            let app = handle.state::<App>();
            app.install_ptt(app.settings().ptt);
            let appearance = app.settings().appearance;
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
            connect,
            disconnect,
            snapshot,
            radios,
            feed,
            add_frequency,
            remove_frequency,
            set_switch,
            set_volume,
            set_muted,
            set_selected,
            set_transmitting,
            set_master_volume,
            set_ptt_bindings,
            ptt_pressed,
            begin_ptt_capture,
            cancel_ptt_capture,
            ptt_ui_key,
            take_captured_binding,
            mouse_ptt_supported,
            audio_devices,
            settings,
            set_audio_devices,
            test_speaker,
            test_mic,
            ptt_bindings,
            keyboard_ptt_supported,
        ])
        .run(tauri::generate_context!())
        .expect("error while running audio-for-can");
}

#[cfg(test)]
mod tests {
    use super::*;

    use can_voice_client::stack::RadioStack;
    use serde_json::json;

    fn one_controller(
        cid: &str,
        callsign: &str,
        frequency: &str,
        facility: i64,
    ) -> serde_json::Value {
        json!({
            "controllers": [{
                "cid": cid, "callsign": callsign,
                "frequency": frequency, "facility": facility,
            }],
            "pilots": [], "atis": [],
        })
    }

    /// 上了席位，那个频率自动加进台面并且键到发射上。
    ///
    /// 不自动加的话，管制员要自己记住并手敲本席频率；忘了加就是"我在 121.8
    /// 守着"而实际上根本没订阅——两边都以为对方在。
    #[test]
    fn the_frequency_i_am_staffing_is_added_and_keyed_for_transmit() {
        let mut stack = RadioStack::new();
        let feed = one_controller("1000", "ZSPD_TWR", "118.350", 4);

        let v = apply_feed(&mut stack, "1000", Some(&feed), &HashSet::new()).expect("a feed");

        assert_eq!(v.duty.callsign, "ZSPD_TWR");
        assert_eq!(v.duty.freq_khz, Some(118_350));
        let r = &stack.radios()[0];
        assert_eq!(r.freq_khz, 118_350);
        assert!(r.tx, "the position frequency has to be keyed for transmit");
        assert!(r.selected);
        assert_eq!(r.callsign, "ZSPD_TWR");
        // 而且它删不掉。
        assert!(!stack.remove(118_350));
    }

    /// **取不到 datafeed 不等于下了席位。**
    ///
    /// 上游抖一下就把一个正在管制的人的发射权收走，比"自动加频率不工作"糟得多：
    /// 他还坐在席位上，飞行员还在那个频率上叫他。取不到就什么都不动，等下一轮。
    #[test]
    fn a_datafeed_that_did_not_come_back_changes_nothing() {
        let mut stack = RadioStack::new();
        let mine = one_controller("1000", "ZSPD_TWR", "118.350", 4);
        apply_feed(&mut stack, "1000", Some(&mine), &HashSet::new());

        assert!(apply_feed(&mut stack, "1000", None, &HashSet::new()).is_none());

        assert!(stack.transmit_allowed());
        assert!(stack.radios()[0].tx);
        assert!(stack.is_locked(118_350));
    }

    /// **手工删掉的频率不会每 60 秒被加回来。**
    ///
    /// 每一分钟跟用户抢一次是最招人烦的那种智能。
    #[test]
    fn a_frequency_i_removed_by_hand_is_not_added_back() {
        let mut stack = RadioStack::new();
        let feed = one_controller("1000", "ZSPD_TWR", "118.350", 4);
        let removed = HashSet::from([118_350]);

        apply_feed(&mut stack, "1000", Some(&feed), &removed);

        assert!(stack.radios().is_empty());
    }

    /// 下了席位，发射就得真的关掉。
    ///
    /// 只把按钮画灰不够：一个下了席位却还标着 TX 的电台，在下一次声明里照样把
    /// TX 发上去。挂观察员（`facility == 0`）算下席位。
    #[test]
    fn stepping_off_the_position_takes_transmit_away() {
        let mut stack = RadioStack::new();
        let mine = one_controller("1000", "ZSPD_TWR", "118.350", 4);
        let observing = one_controller("1000", "ZSPD_OBS", "118.350", 0);
        apply_feed(&mut stack, "1000", Some(&mine), &HashSet::new());

        let v = apply_feed(&mut stack, "1000", Some(&observing), &HashSet::new()).expect("a feed");

        assert!(v.duty.callsign.is_empty());
        assert!(
            v.duty.dropped_tx,
            "the user has to be told his transmit just went away"
        );
        assert!(!stack.transmit_allowed());
        assert!(!stack.radios()[0].tx);
        // 但还听得见，而且现在删得掉了。
        assert!(stack.radios()[0].rx);
        assert!(stack.remove(118_350));
    }

    /// 已经在台面上的席位频率，**不每 60 秒把 TX 抢回来**。
    ///
    /// 管制员可能刚刚有意把它关掉（换班交接、跨席位借用）。自动加是为了不让人
    /// 忘了加，不是为了不让人改。
    #[test]
    fn an_adopted_frequency_keeps_the_switches_i_left_it_with() {
        let mut stack = RadioStack::new();
        let feed = one_controller("1000", "ZSPD_TWR", "118.350", 4);
        apply_feed(&mut stack, "1000", Some(&feed), &HashSet::new());
        stack.set_tx(118_350, false);

        apply_feed(&mut stack, "1000", Some(&feed), &HashSet::new());

        assert!(!stack.radios()[0].tx);
    }

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
                muted: false,
                callsign: String::new(),
            },
            Radio {
                freq_khz: 121_800,
                rx: true,
                tx: true,
                xc: false,
                gain: 1.0,
                selected: true,
                muted: true,
                callsign: "ZSPD_TWR".into(),
            },
        ];

        let mut stack = can_voice_client::stack::RadioStack::new();
        restore_stack(&mut stack, &saved);

        assert_eq!(stack.radios(), saved.as_slice());
    }
}
