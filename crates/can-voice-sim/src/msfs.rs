//! MSFS 数据链路（SimConnect）。
//!
//! 作用和 [`crate::xplane`] 完全一样：把机位、姿态、速度、无线电和应答机取出来，
//! 填进同一个 [`Snapshot`]。差别只在怎么取。
//!
//! ```text
//! X-Plane   UDP 多播发现 + RREF 订阅，推过来
//! MSFS      SimConnect（本机命名管道/共享内存），一问一答取 SimVar
//! ```
//!
//! SimConnect 只能连**本机**的模拟器，没有发现过程；模拟器没开时打开就直接失败。
//!
//! # 这一层分成两半，只有一半是这台机器上验得了的
//!
//! [`snapshot`]、[`SIMVARS`]、单位换算都是纯函数，有测试。真正调 SimConnect 的
//! 那一半是 **Windows-only 的 C API**，`#[cfg(windows)]`，在别的平台上
//! [`available`] 返回 false——上层因此只有一份代码，不用把整条链路切成两份。

use crate::{bcd_to_squawk, Snapshot};
use std::collections::HashMap;

/// 这个平台上有没有 SimConnect。
pub const fn available() -> bool {
    cfg!(windows)
}

/// 要取的 SimVar。
///
/// # 单位不是猜的，而且有几个名字是骗人的
///
/// - `PLANE_LATITUDE` / `PLANE_LONGITUDE` 取**度**，取回来就是度，不要再转
///   一次——转了 31.14 会变成 1784.2，服务端每个位置包都回
///   "Invalid latitude/longitude"，90 秒后把连接掐掉。
/// - `PLANE_PITCH_DEGREES` / `PLANE_BANK_DEGREES` / `PLANE_HEADING_DEGREES_TRUE`
///   名字里带 DEGREES，取回来是**弧度**。
/// - 俯仰和坡度还要**取负**：SimConnect 的正方向和 FSD 相反。
/// - 别用 `PRESSURE_ALTITUDE`：常见绑定库给它写的单位是**米**，这一堆高度里
///   就它一个不是英尺，当英尺用会差 3.28 倍。
/// - `TRANSPONDER_CODE:1` 是 **BCD**。
pub const SIMVARS: [(&str, &str, &str); 22] = [
    ("latitude", "PLANE LATITUDE", "Degrees"),
    ("longitude", "PLANE LONGITUDE", "Degrees"),
    ("altitude", "PLANE ALTITUDE", "Feet"),
    ("agl", "PLANE ALT ABOVE GROUND", "Feet"),
    ("indicated_altitude", "INDICATED ALTITUDE", "Feet"),
    ("baro_setting", "KOHLSMAN SETTING HG", "inHg"),
    ("groundspeed", "GROUND VELOCITY", "Knots"),
    ("pitch", "PLANE PITCH DEGREES", "Radians"),
    ("bank", "PLANE BANK DEGREES", "Radians"),
    ("heading", "PLANE HEADING DEGREES TRUE", "Radians"),
    ("squawk", "TRANSPONDER CODE:1", "Number"),
    ("xpdr_state", "TRANSPONDER STATE:1", "Number"),
    ("com1", "COM ACTIVE FREQUENCY:1", "MHz"),
    ("com2", "COM ACTIVE FREQUENCY:2", "MHz"),
    ("on_ground", "SIM ON GROUND", "Bool"),
    ("engine_on", "GENERAL ENG COMBUSTION:1", "Bool"),
    ("gear", "GEAR HANDLE POSITION", "Percent"),
    ("flaps", "TRAILING EDGE FLAPS LEFT PERCENT", "Percent"),
    ("spoilers", "SPOILERS HANDLE POSITION", "Percent"),
    ("light_beacon", "LIGHT BEACON", "Bool"),
    ("light_landing", "LIGHT LANDING", "Bool"),
    ("light_taxi", "LIGHT TAXI", "Bool"),
];

/// `LIGHT NAV` 和 `LIGHT STROBE` 单独列出来——[`SIMVARS`] 的长度是写死的，
/// 加一项要改两处，这里的注释就是提醒。
pub const EXTRA_LIGHTS: [(&str, &str, &str); 2] = [
    ("light_strobe", "LIGHT STROBE", "Bool"),
    ("light_nav", "LIGHT NAV", "Bool"),
];

/// 全部要订的 SimVar。
pub fn all_simvars() -> Vec<(&'static str, &'static str, &'static str)> {
    SIMVARS.iter().chain(EXTRA_LIGHTS.iter()).copied().collect()
}

/// 把取回来的 SimVar 换算成一份 [`Snapshot`]。
pub fn snapshot(raw: &HashMap<&str, f64>) -> Option<Snapshot> {
    if raw.is_empty() {
        return None;
    }
    let get = |k: &str| raw.get(k).copied();
    let altitude = get("altitude").unwrap_or(0.0).round() as i32;
    let groundspeed = get("groundspeed").unwrap_or(0.0).round() as i32;
    let on_ground = get("on_ground").unwrap_or(0.0) != 0.0;
    let flag = |k: &str| get(k).map(|v| v != 0.0);
    Some(Snapshot {
        // **已经是度。** 再 to_degrees 一次的话 31.14 会变成 1784.2，
        // 服务端每个位置包都回 "Invalid latitude/longitude"。
        latitude: get("latitude").unwrap_or(0.0),
        longitude: get("longitude").unwrap_or(0.0),
        altitude,
        pressure_delta: crate::pressure_delta(
            get("indicated_altitude"),
            get("baro_setting"),
            altitude,
        ),
        agl: get("agl").unwrap_or(0.0).round() as i32,
        groundspeed,
        // 弧度转度，**而且要取负**：SimConnect 的正方向和 FSD 相反。
        pitch: -get("pitch").unwrap_or(0.0).to_degrees(),
        bank: -get("bank").unwrap_or(0.0).to_degrees(),
        heading: get("heading").unwrap_or(0.0).to_degrees().rem_euclid(360.0),
        squawk: bcd_to_squawk(get("squawk").unwrap_or(0x2000_u32 as f64) as u32),
        xpdr_mode: crate::xpdr_mode(
            get("xpdr_state").map(|v| v as i32),
            crate::XPDR_ONLINE_FROM_MSFS,
            on_ground,
            groundspeed,
        ),
        com1: frequency(get("com1")),
        com2: frequency(get("com2")),
        // MSFS 没有"COM 电门"这个 SimVar，按开着算。
        com1_power: true,
        on_ground,
        animation: Some(crate::Animation {
            gear_down: get("gear").unwrap_or(100.0) > 50.0,
            flaps: (get("flaps").unwrap_or(0.0) / 100.0).clamp(0.0, 1.0),
            spoilers: get("spoilers").unwrap_or(0.0) > 0.0,
            engines_on: flag("engine_on").unwrap_or(true),
            beacon_on: flag("light_beacon").unwrap_or(false),
            landing_on: flag("light_landing").unwrap_or(false),
            taxi_on: flag("light_taxi").unwrap_or(false),
            strobe_on: flag("light_strobe").unwrap_or(false),
            nav_on: flag("light_nav").unwrap_or(false),
        }),
    })
}

/// FSD 约定的俯仰坡度换成 SimConnect 约定，注入他机时用。
///
/// [`snapshot`] 读自机时取了一次负（`PLANE PITCH DEGREES` 名字里写着度、
/// 实际是弧度，而且正方向和 FSD 相反），写回去自然要再取一次。
///
/// **它单独成一个函数，是因为这条约定唯一出过的错就是两端各做各的假设。**
/// 注入那一半曾经把 `unpack_pbh` 出来的 FSD 值原样送进 `PLANE PITCH DEGREES`：
/// 别人爬升时机头朝下、左转时向右压坡度，而自机上网的姿态是对的——
/// 单机自测看不出来，要两个人对飞才看得见。往返测试 `reading_then_injecting_round_trips`
/// 把两端钉在一起，改一边就红。
pub fn attitude_for_injection(pitch: f64, bank: f64) -> (f64, f64) {
    (-pitch, -bank)
}

/// COM 频率（MHz）。SimConnect 直接给兆赫。
fn frequency(mhz: Option<f64>) -> Option<f64> {
    let mhz = mhz?;
    (mhz > 0.0).then(|| (mhz * 1000.0).round() / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_voice_fsd::pilot::XpdrMode;

    fn raw(pairs: &[(&'static str, f64)]) -> HashMap<&'static str, f64> {
        pairs.iter().copied().collect()
    }

    /// **经纬度已经是度，不要再转一次。**
    ///
    /// can-audio 那边犯过：又 `math.degrees` 了一遍，31.14 变成 1784.2，
    /// 服务端每个位置包都回 "Invalid latitude/longitude"，90 秒后把连接掐掉。
    #[test]
    fn the_position_is_already_in_degrees() {
        let s = snapshot(&raw(&[("latitude", 31.142_33), ("longitude", 121.790_84)]))
            .expect("snapshot");
        assert!((s.latitude - 31.142_33).abs() < 1e-9);
        assert!((s.longitude - 121.790_84).abs() < 1e-9);
    }

    /// 姿态那几个名字里带 DEGREES，取回来是**弧度**，而且正方向和 FSD 相反。
    #[test]
    fn the_attitude_is_radians_and_the_sign_is_flipped() {
        let s = snapshot(&raw(&[
            ("pitch", 10.0_f64.to_radians()),
            ("bank", 25.0_f64.to_radians()),
            ("heading", 270.0_f64.to_radians()),
        ]))
        .expect("snapshot");
        // 机头朝上在 SimConnect 里是负的俯仰角。
        assert!((s.pitch + 10.0).abs() < 1e-6, "{}", s.pitch);
        assert!((s.bank + 25.0).abs() < 1e-6, "{}", s.bank);
        // 航向不取负，只转成度。
        assert!((s.heading - 270.0).abs() < 1e-6, "{}", s.heading);
    }

    #[test]
    fn the_heading_wraps_into_zero_to_three_sixty() {
        let s = snapshot(&raw(&[("heading", 725.0_f64.to_radians())])).expect("snapshot");
        assert!((s.heading - 5.0).abs() < 1e-6, "{}", s.heading);
    }

    /// 注入他机时要把 FSD 约定换回 SimConnect 约定——和
    /// [`the_attitude_is_radians_and_the_sign_is_flipped`] 那一次取负配对。
    #[test]
    fn injecting_flips_the_attitude_back() {
        let (pitch, bank) = attitude_for_injection(10.0, 25.0);
        assert!((pitch + 10.0).abs() < 1e-9, "{pitch}");
        assert!((bank + 25.0).abs() < 1e-9, "{bank}");
    }

    /// **读进来再写回去必须回到原值。** 这一条钉的是接口两端用同一个约定：
    /// 读那一半取了负而写那一半没取，别人爬升时机头朝下、左转时向右压坡度，
    /// 而自机上网的姿态是对的——单机自测看不出来，要两个人对飞才看得见。
    #[test]
    fn reading_then_injecting_round_trips() {
        let s = snapshot(&raw(&[
            ("pitch", 10.0_f64.to_radians()),
            ("bank", 25.0_f64.to_radians()),
        ]))
        .expect("snapshot");
        let (pitch, bank) = attitude_for_injection(s.pitch, s.bank);
        assert!((pitch - 10.0).abs() < 1e-6, "{pitch}");
        assert!((bank - 25.0).abs() < 1e-6, "{bank}");
    }

    /// 应答机码是 BCD。
    #[test]
    fn the_squawk_comes_out_of_bcd() {
        let s = snapshot(&raw(&[("squawk", f64::from(0x7700_u32))])).expect("snapshot");
        assert_eq!(s.squawk, 7700);
    }

    /// **MSFS 的 2 是"测试"，不是"开"。** 用 X-Plane 那个门槛的话，一架在做
    /// 应答机自检的飞机会被当成在线报出去。
    #[test]
    fn a_transponder_under_test_is_not_online() {
        let s = snapshot(&raw(&[("xpdr_state", 2.0), ("on_ground", 1.0)])).expect("snapshot");
        assert_eq!(s.xpdr_mode, XpdrMode::Standby);
        let s = snapshot(&raw(&[("xpdr_state", 3.0), ("on_ground", 1.0)])).expect("snapshot");
        assert_eq!(s.xpdr_mode, XpdrMode::ModeC);
    }

    /// 动画状态这一侧有，X-Plane 那一侧没有——所以 [`Snapshot::animation`]
    /// 是 `Option`，而不是两边都填一份默认值。
    #[test]
    fn the_animation_state_comes_through() {
        let s = snapshot(&raw(&[
            ("gear", 0.0),
            ("flaps", 50.0),
            ("spoilers", 1.0),
            ("light_beacon", 1.0),
            ("light_strobe", 0.0),
        ]))
        .expect("snapshot");
        let a = s.animation.expect("animation");
        assert!(!a.gear_down);
        assert!((a.flaps - 0.5).abs() < 1e-9);
        assert!(a.spoilers);
        assert!(a.beacon_on);
        assert!(!a.strobe_on);
        // 读不到的按开着算：发动机停了却报着在转，比反过来无害。
        assert!(a.engines_on);
    }

    /// 起落架手柄是百分比，一半以上算放下。
    #[test]
    fn the_gear_handle_is_a_percentage() {
        let down = snapshot(&raw(&[("gear", 100.0)])).expect("snapshot");
        assert!(down.animation.expect("animation").gear_down);
        let up = snapshot(&raw(&[("gear", 0.0)])).expect("snapshot");
        assert!(!up.animation.expect("animation").gear_down);
        // 读不到时按放下——地面上看到一架收着起落架的飞机更可疑。
        let unknown = snapshot(&raw(&[("latitude", 31.0)])).expect("snapshot");
        assert!(unknown.animation.expect("animation").gear_down);
    }

    #[test]
    fn a_zero_frequency_is_none_rather_than_zero() {
        let s = snapshot(&raw(&[("com1", 0.0), ("com2", 121.8)])).expect("snapshot");
        assert_eq!(s.com1, None);
        assert_eq!(s.com2, Some(121.8));
    }

    #[test]
    fn no_data_yet_is_none() {
        assert!(snapshot(&HashMap::new()).is_none());
    }

    /// 每一个 [`snapshot`] 读的名字都必须在要订的表里——漏订一个的表现不是
    /// 报错，是那一项永远取默认值。X-Plane 那边就漏过 `on_ground`。
    #[test]
    fn every_field_the_snapshot_reads_is_actually_requested() {
        let requested: Vec<&str> = all_simvars().into_iter().map(|(name, _, _)| name).collect();
        for name in [
            "latitude",
            "longitude",
            "altitude",
            "agl",
            "indicated_altitude",
            "baro_setting",
            "groundspeed",
            "pitch",
            "bank",
            "heading",
            "squawk",
            "xpdr_state",
            "com1",
            "com2",
            "on_ground",
            "engine_on",
            "gear",
            "flaps",
            "spoilers",
            "light_beacon",
            "light_landing",
            "light_taxi",
            "light_strobe",
            "light_nav",
        ] {
            assert!(
                requested.contains(&name),
                "{name} is read but never requested"
            );
        }
        assert_eq!(requested.len(), 24);
    }

    #[test]
    fn simconnect_is_windows_only() {
        assert_eq!(available(), cfg!(windows));
    }
}

// ——— 真正的链路 ———

/// 一个能取 SimVar 的东西。
///
/// **抽成 trait 是为了让上面那一整层在这台机器上也能编译和测试。** 真正调
/// SimConnect 的实现是 Windows-only 的 C API；把它藏在这个 trait 后面，
/// `apps/msfs` 就只有一份代码，而不是用 `#[cfg]` 把整个客户端切成两份。
pub trait SimVarSource: Send + 'static {
    /// 连上模拟器。**没有发现过程**——SimConnect 只连本机，模拟器没开就直接失败。
    fn open(&mut self) -> Result<(), String>;
    /// 取一轮。返回这一轮读到的值；模拟器断了返回 `Err`。
    fn poll(&mut self) -> Result<HashMap<&'static str, f64>, String>;
    fn close(&mut self);
}

/// 非 Windows 上的实现：永远打不开。
///
/// 存在的意义是让 `apps/msfs` 在这台机器上编得过、跑得起来（界面会说
/// "这个系统上没有 SimConnect"），而不是让开发时少一整条编译路径。
#[derive(Debug, Default)]
pub struct Unavailable;

impl SimVarSource for Unavailable {
    fn open(&mut self) -> Result<(), String> {
        Err("SimConnect is only available on Windows".to_string())
    }
    fn poll(&mut self) -> Result<HashMap<&'static str, f64>, String> {
        Err("not connected".to_string())
    }
    fn close(&mut self) {}
}

/// 往模拟器里放他机的那一头。
///
/// 和 [`SimVarSource`] 分开，是因为**它们各自拿一条 SimConnect 连接**。
/// SimConnect 的 handle 不是线程安全的，而读数据那条跑在自己的阻塞线程上
/// （C 函数会把 tokio 执行器卡住）；共用一个 handle 就得加锁，等于把注入
/// 和轮询串起来。SimConnect 本来就允许一个进程开多条连接，用两条更简单。
pub trait TrafficSink: Send + 'static {
    /// 连上。模拟器没开就是打不开——那是常态，不是错误。
    fn open(&mut self) -> Result<(), String>;

    /// 把这一批动作发下去。
    ///
    /// 返回**这一批之后收到的新建回音**：`Ok` 里是 `(呼号, object_id)`，
    /// `Err` 里是建失败的呼号。新建是异步的——发下去不等于建成了，回音要靠
    /// 后面的 [`Self::pump`] 收。
    fn apply(&mut self, actions: &[crate::inject::Action]) -> Result<(), String>;

    /// 收新建的回音。返回 `(呼号, 结果)`，`None` 表示这一架建失败了。
    fn pump(&mut self) -> Vec<(String, Option<u32>)>;

    fn close(&mut self);
}

/// 不在 Windows 上时的那一份：什么都不做。
#[derive(Debug, Default)]
pub struct NoTraffic;

impl TrafficSink for NoTraffic {
    fn open(&mut self) -> Result<(), String> {
        Err("SimConnect is only available on Windows".into())
    }
    fn apply(&mut self, _actions: &[crate::inject::Action]) -> Result<(), String> {
        Err("SimConnect is only available on Windows".into())
    }
    fn pump(&mut self) -> Vec<(String, Option<u32>)> {
        Vec::new()
    }
    fn close(&mut self) {}
}

#[cfg(windows)]
pub use ffi::SimConnectTraffic;
#[cfg(not(windows))]
pub type SimConnectTraffic = NoTraffic;

#[cfg(windows)]
pub use ffi::SimConnectSource;

#[cfg(not(windows))]
pub type SimConnectSource = Unavailable;

#[cfg(any(windows, test))]
fn dispatch_has_bytes(size: usize, required: usize) -> bool {
    size >= required
}

#[cfg(any(windows, test))]
fn dispatch_has_array(size: usize, header: usize, count: usize, element: usize) -> bool {
    count
        .checked_mul(element)
        .and_then(|bytes| header.checked_add(bytes))
        .is_some_and(|required| dispatch_has_bytes(size, required))
}

#[cfg(any(windows, test))]
fn decode_f64_payload(bytes: &[u8], header: usize, count: usize) -> Option<Vec<f64>> {
    if !dispatch_has_array(bytes.len(), header, count, std::mem::size_of::<f64>()) {
        return None;
    }
    let end = header.checked_add(count.checked_mul(std::mem::size_of::<f64>())?)?;
    bytes
        .get(header..end)?
        .chunks_exact(8)
        .map(|chunk| Some(f64::from_ne_bytes(chunk.try_into().ok()?)))
        .collect()
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;

    #[test]
    fn dispatch_requires_the_full_fixed_header() {
        assert!(dispatch_has_bytes(24, 24));
        assert!(!dispatch_has_bytes(23, 24));
    }

    #[test]
    fn dispatch_rejects_truncated_and_overflowing_variable_payloads() {
        assert!(dispatch_has_array(64, 32, 4, 8));
        assert!(!dispatch_has_array(63, 32, 4, 8));
        assert!(!dispatch_has_array(usize::MAX, 32, usize::MAX, 8));
    }

    #[test]
    fn complete_simvar_payload_decodes_only_validated_values() {
        let mut packet = vec![0; 4];
        packet.extend_from_slice(&1.5f64.to_ne_bytes());
        packet.extend_from_slice(&(-2.0f64).to_ne_bytes());
        assert_eq!(decode_f64_payload(&packet, 4, 2), Some(vec![1.5, -2.0]));
        assert_eq!(decode_f64_payload(&packet[..packet.len() - 1], 4, 2), None);
    }
}

#[cfg(windows)]
mod ffi {
    //! SimConnect 的 C API 绑定。
    //!
    //! # 编译期不依赖 SimConnect SDK
    //!
    //! DLL 是**运行时**加载的，所以编译这一段不需要那份不能随仓库分发的 SDK
    //! ——CI 的 Windows runner 因此能真的编译它。**但"编得过"不等于"对"**：
    //! 函数签名、结构体布局、常量取值这些只有连上一次真实模拟器才验得了。
    //!
    //! 调用顺序照微软的文档：
    //!
    //! ```text
    //! SimConnect_Open
    //! SimConnect_AddToDataDefinition   每个 SimVar 一次，顺序就是结构体里的顺序
    //! SimConnect_RequestDataOnSimObject  period = SIM_FRAME，只在变化时回
    //! SimConnect_GetNextDispatch       轮询，拿 SIMOBJECT_DATA
    //! SimConnect_Close
    //! ```
    //!
    //! **`AddToDataDefinition` 的顺序就是回包里 f64 的顺序**，和
    //! [`super::all_simvars`] 一一对应。改那张表的顺序而不改这里，读出来的
    //! 每一个值都会串位——而每一个值单独看都是合法的数字。

    use super::{all_simvars, dispatch_has_array, dispatch_has_bytes, SimVarSource, TrafficSink};
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::os::raw::{c_char, c_double, c_int, c_ulong};
    use std::sync::atomic::{AtomicBool, Ordering};

    static SHORT_HEADER_LOGGED: AtomicBool = AtomicBool::new(false);
    static SHORT_SIMVARS_LOGGED: AtomicBool = AtomicBool::new(false);
    static SHORT_ASSIGNED_LOGGED: AtomicBool = AtomicBool::new(false);
    static SHORT_EXCEPTION_LOGGED: AtomicBool = AtomicBool::new(false);

    fn warn_short(flag: &AtomicBool, packet: &'static str) {
        if !flag.swap(true, Ordering::Relaxed) {
            tracing::warn!(packet, "truncated SimConnect dispatch ignored");
        }
    }

    type Handle = *mut c_void;
    const DEF_ID: c_ulong = 1;
    const REQ_ID: c_ulong = 1;
    /// `SIMCONNECT_DATATYPE_FLOAT64`
    const DATATYPE_FLOAT64: c_int = 4;
    /// `SIMCONNECT_PERIOD_SIM_FRAME`
    const PERIOD_SIM_FRAME: c_ulong = 4;
    /// `SIMCONNECT_OBJECT_ID_USER`
    const OBJECT_ID_USER: c_ulong = 0;
    /// `SIMCONNECT_RECV_ID_SIMOBJECT_DATA`
    const RECV_ID_SIMOBJECT_DATA: c_ulong = 8;
    /// `SIMCONNECT_RECV_ID_ASSIGNED_OBJECT_ID`——新建 AI 机的回音。
    const RECV_ID_ASSIGNED_OBJECT_ID: c_ulong = 12;
    /// `SIMCONNECT_RECV_ID_EXCEPTION`——**新建失败走的是这条**，不是返回值。
    ///
    /// `AICreateNonATCAircraft` 是异步的：标题不存在时它照样返回成功，失败是
    /// 过一会儿以一个 EXCEPTION 回来的，里面带着当初那个 request id。所以
    /// request id 必须能反查回呼号，否则失败了也不知道是哪一架。
    const RECV_ID_EXCEPTION: c_ulong = 1;
    /// 他机位置的数据定义 id。和读自机的 [`DEF_ID`] 分属两条连接，不会撞。
    const TRAFFIC_DEF_ID: c_ulong = 2;
    /// `SIMCONNECT_DATA_SET_FLAG_DEFAULT`
    const SET_FLAG_DEFAULT: c_ulong = 0;

    #[repr(C)]
    struct Recv {
        size: c_ulong,
        version: c_ulong,
        id: c_ulong,
    }

    /// `SIMCONNECT_RECV_ASSIGNED_OBJECT_ID`
    #[repr(C)]
    struct RecvAssignedObjectId {
        base: Recv,
        request_id: c_ulong,
        object_id: c_ulong,
    }

    /// `SIMCONNECT_RECV_EXCEPTION`
    #[repr(C)]
    struct RecvException {
        base: Recv,
        exception: c_ulong,
        /// 出问题的那个包的序号。**不是 request id**——要靠它反查得先记下
        /// 每次调用的 send id，而 `SimConnect_GetLastSentPacketID` 才给得出。
        send_id: c_ulong,
        index: c_ulong,
    }

    /// `SIMCONNECT_DATA_INITPOSITION`
    ///
    /// 布局是六个 f64 加两个 DWORD，56 字节。**顺序和类型都不能动**——
    /// 它是直接按字节传给 SimConnect 的，错一个字段就是飞机出现在地球另一边。
    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default, PartialEq)]
    struct InitPosition {
        latitude: c_double,
        longitude: c_double,
        /// 英尺。
        altitude: c_double,
        pitch: c_double,
        bank: c_double,
        heading: c_double,
        on_ground: c_ulong,
        /// 节。
        airspeed: c_ulong,
    }

    /// 更新他机位置用的数据定义。**六个 f64 加两个 f64 的开关**——
    /// `SetDataOnSimObject` 只认 FLOAT64，所以 `SIM ON GROUND` 也是 f64。
    ///
    /// 这张表的顺序就是 `AddToDataDefinition` 的调用顺序，也就是结构体里
    /// 字段的顺序。**改一个而不改另一个，飞机的每一个值都会串位**，而串位
    /// 之后每一个值单独看都是合法的数字。
    const TRAFFIC_VARS: [(&str, &str); 8] = [
        ("PLANE LATITUDE", "degrees"),
        ("PLANE LONGITUDE", "degrees"),
        ("PLANE ALTITUDE", "feet"),
        ("PLANE PITCH DEGREES", "degrees"),
        ("PLANE BANK DEGREES", "degrees"),
        ("PLANE HEADING DEGREES TRUE", "degrees"),
        ("SIM ON GROUND", "bool"),
        ("AIRSPEED TRUE", "knots"),
    ];

    /// 和 [`TRAFFIC_VARS`] 一一对应。
    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default, PartialEq)]
    struct TrafficPosition {
        latitude: c_double,
        longitude: c_double,
        altitude: c_double,
        pitch: c_double,
        bank: c_double,
        heading: c_double,
        on_ground: c_double,
        airspeed: c_double,
    }

    #[repr(C)]
    struct RecvSimObjectData {
        base: Recv,
        request_id: c_ulong,
        object_id: c_ulong,
        define_id: c_ulong,
        flags: c_ulong,
        entry_number: c_ulong,
        out_of: c_ulong,
        define_count: c_ulong,
        // 之后紧跟着 define_count 个 f64。
    }

    // **运行时加载 SimConnect.dll，不在链接期依赖它。**
    //
    // 写成 `#[link(name = "SimConnect")]` 的话，编译这个 crate 就需要
    // SimConnect SDK 的 .lib —— 而那份 SDK 不能随仓库分发，于是 CI 编不了，
    // 于是这一整段代码在合并之前没有任何东西看过它。运行时加载把编译期依赖
    // 变成零：**CI 的 Windows runner 现在会真的编译这一段**，而 DLL 由用户
    // 机器上的 MSFS 提供（模拟器装好就有）。
    //
    // 代价是每个函数要自己声明一次类型并 GetProcAddress 一次。名字取不到时
    // 说得出是哪一个，而不是笼统的"打不开"。

    type FnOpen = unsafe extern "system" fn(
        *mut Handle,
        *const c_char,
        *mut c_void,
        c_ulong,
        *mut c_void,
        c_ulong,
    ) -> c_int;
    type FnClose = unsafe extern "system" fn(Handle) -> c_int;
    type FnAddToDataDefinition = unsafe extern "system" fn(
        Handle,
        c_ulong,
        *const c_char,
        *const c_char,
        c_int,
        f32,
        c_ulong,
    ) -> c_int;
    type FnRequestDataOnSimObject = unsafe extern "system" fn(
        Handle,
        c_ulong,
        c_ulong,
        c_ulong,
        c_ulong,
        c_ulong,
        c_ulong,
        c_ulong,
        c_ulong,
    ) -> c_int;
    type FnGetNextDispatch =
        unsafe extern "system" fn(Handle, *mut *mut Recv, *mut c_ulong) -> c_int;
    type FnAICreateNonATCAircraft = unsafe extern "system" fn(
        Handle,
        *const c_char,
        *const c_char,
        InitPosition,
        c_ulong,
    ) -> c_int;
    type FnAIRemoveObject = unsafe extern "system" fn(Handle, c_ulong, c_ulong) -> c_int;
    type FnGetLastSentPacketID = unsafe extern "system" fn(Handle, *mut c_ulong) -> c_int;
    type FnSetDataOnSimObject = unsafe extern "system" fn(
        Handle,
        c_ulong,
        c_ulong,
        c_ulong,
        c_ulong,
        c_ulong,
        *mut c_void,
    ) -> c_int;

    extern "system" {
        fn LoadLibraryA(name: *const c_char) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    }

    /// SimConnect.dll 里我们用到的那几个入口。
    struct Api {
        open: FnOpen,
        close: FnClose,
        add_to_data_definition: FnAddToDataDefinition,
        request_data_on_sim_object: FnRequestDataOnSimObject,
        get_next_dispatch: FnGetNextDispatch,
        ai_create_non_atc_aircraft: FnAICreateNonATCAircraft,
        ai_remove_object: FnAIRemoveObject,
        set_data_on_sim_object: FnSetDataOnSimObject,
        get_last_sent_packet_id: FnGetLastSentPacketID,
    }

    impl Api {
        /// 加载一次。**DLL 由模拟器提供**：MSFS 装好就在 PATH 上找得到，
        /// 没装的话这里就是"找不到 SimConnect.dll"——那正是要对用户说的话。
        fn load() -> Result<&'static Api, String> {
            use std::sync::OnceLock;
            static API: OnceLock<Result<Api, String>> = OnceLock::new();
            match API.get_or_init(Api::load_once) {
                Ok(api) => Ok(api),
                Err(e) => Err(e.clone()),
            }
        }

        fn load_once() -> Result<Api, String> {
            let name = std::ffi::CString::new("SimConnect.dll").expect("static");
            let module = unsafe { LoadLibraryA(name.as_ptr()) };
            if module.is_null() {
                return Err(
                    "could not load SimConnect.dll; is Microsoft Flight Simulator installed?"
                        .into(),
                );
            }
            // 取一个入口。**取不到要说出是哪一个**——一个笼统的"打不开"会让人
            // 去查模拟器有没有开，而实际问题是 DLL 版本太老、少了这个导出。
            fn symbol(module: *mut c_void, name: &str) -> Result<*mut c_void, String> {
                let c = std::ffi::CString::new(name).map_err(|e| e.to_string())?;
                let address = unsafe { GetProcAddress(module, c.as_ptr()) };
                if address.is_null() {
                    return Err(format!("SimConnect.dll has no {name}; it is too old"));
                }
                Ok(address)
            }
            unsafe {
                Ok(Api {
                    open: std::mem::transmute::<*mut c_void, FnOpen>(symbol(
                        module,
                        "SimConnect_Open",
                    )?),
                    close: std::mem::transmute::<*mut c_void, FnClose>(symbol(
                        module,
                        "SimConnect_Close",
                    )?),
                    add_to_data_definition: std::mem::transmute::<*mut c_void, FnAddToDataDefinition>(
                        symbol(module, "SimConnect_AddToDataDefinition")?,
                    ),
                    request_data_on_sim_object: std::mem::transmute::<
                        *mut c_void,
                        FnRequestDataOnSimObject,
                    >(symbol(
                        module,
                        "SimConnect_RequestDataOnSimObject",
                    )?),
                    get_next_dispatch: std::mem::transmute::<*mut c_void, FnGetNextDispatch>(
                        symbol(module, "SimConnect_GetNextDispatch")?,
                    ),
                    ai_create_non_atc_aircraft: std::mem::transmute::<
                        *mut c_void,
                        FnAICreateNonATCAircraft,
                    >(symbol(
                        module,
                        "SimConnect_AICreateNonATCAircraft",
                    )?),
                    ai_remove_object: std::mem::transmute::<*mut c_void, FnAIRemoveObject>(symbol(
                        module,
                        "SimConnect_AIRemoveObject",
                    )?),
                    set_data_on_sim_object: std::mem::transmute::<*mut c_void, FnSetDataOnSimObject>(
                        symbol(module, "SimConnect_SetDataOnSimObject")?,
                    ),
                    get_last_sent_packet_id: std::mem::transmute::<
                        *mut c_void,
                        FnGetLastSentPacketID,
                    >(symbol(
                        module,
                        "SimConnect_GetLastSentPacketID",
                    )?),
                })
            }
        }
    }

    fn ok(result: c_int) -> bool {
        result >= 0
    }

    /// 真正连 SimConnect 的那一份。
    #[derive(Default)]
    pub struct SimConnectSource {
        handle: Handle,
    }

    // Handle 是一个只在这一个线程里用的不透明指针。
    unsafe impl Send for SimConnectSource {}

    impl SimVarSource for SimConnectSource {
        fn open(&mut self) -> Result<(), String> {
            let api = Api::load()?;
            let name = std::ffi::CString::new("msfs-for-can").map_err(|e| e.to_string())?;
            let mut handle: Handle = std::ptr::null_mut();
            unsafe {
                if !ok((api.open)(
                    &mut handle,
                    name.as_ptr(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )) {
                    return Err("could not open SimConnect; is the simulator running?".into());
                }
                // **顺序就是回包里 f64 的顺序。** 见模块头。
                for (_, simvar, units) in all_simvars() {
                    let name = std::ffi::CString::new(simvar).map_err(|e| e.to_string())?;
                    let units = std::ffi::CString::new(units).map_err(|e| e.to_string())?;
                    if !ok((api.add_to_data_definition)(
                        handle,
                        DEF_ID,
                        name.as_ptr(),
                        units.as_ptr(),
                        DATATYPE_FLOAT64,
                        0.0,
                        c_ulong::MAX,
                    )) {
                        (api.close)(handle);
                        return Err(format!("the simulator does not know the SimVar {simvar}"));
                    }
                }
                if !ok((api.request_data_on_sim_object)(
                    handle,
                    REQ_ID,
                    DEF_ID,
                    OBJECT_ID_USER,
                    PERIOD_SIM_FRAME,
                    0,
                    0,
                    0,
                    0,
                )) {
                    (api.close)(handle);
                    return Err("the simulator refused the data request".into());
                }
            }
            self.handle = handle;
            Ok(())
        }

        fn poll(&mut self) -> Result<HashMap<&'static str, f64>, String> {
            if self.handle.is_null() {
                return Err("not connected".into());
            }
            let api = Api::load()?;
            let names: Vec<&'static str> =
                all_simvars().into_iter().map(|(name, _, _)| name).collect();
            let mut out = HashMap::new();
            loop {
                let mut data: *mut Recv = std::ptr::null_mut();
                let mut size: c_ulong = 0;
                let result = unsafe { (api.get_next_dispatch)(self.handle, &mut data, &mut size) };
                if !ok(result) || data.is_null() {
                    // 队列空了就是空了，不是错误。
                    return Ok(out);
                }
                if !dispatch_has_bytes(size as usize, std::mem::size_of::<Recv>()) {
                    warn_short(&SHORT_HEADER_LOGGED, "header");
                    continue;
                }
                unsafe {
                    if (*data).id != RECV_ID_SIMOBJECT_DATA {
                        continue;
                    }
                    if !dispatch_has_bytes(size as usize, std::mem::size_of::<RecvSimObjectData>())
                    {
                        warn_short(&SHORT_SIMVARS_LOGGED, "simvar header");
                        continue;
                    }
                    let payload = data as *const RecvSimObjectData;
                    let count = (*payload).define_count as usize;
                    // **只认数量对得上的那一包。** 对不上说明定义和回包错位了，
                    // 而错位之后每一个值单独看都是合法的数字。
                    if count != names.len() {
                        continue;
                    }
                    if !dispatch_has_array(
                        size as usize,
                        std::mem::size_of::<RecvSimObjectData>(),
                        count,
                        std::mem::size_of::<c_double>(),
                    ) {
                        warn_short(&SHORT_SIMVARS_LOGGED, "simvar values");
                        continue;
                    }
                    let required = std::mem::size_of::<RecvSimObjectData>()
                        + count * std::mem::size_of::<c_double>();
                    let bytes = std::slice::from_raw_parts(data as *const u8, required);
                    if let Some(values) = super::decode_f64_payload(
                        bytes,
                        std::mem::size_of::<RecvSimObjectData>(),
                        count,
                    ) {
                        for (name, value) in names.iter().zip(values) {
                            out.insert(*name, value);
                        }
                    }
                }
            }
        }

        fn close(&mut self) {
            if self.handle.is_null() {
                return;
            }
            if let Ok(api) = Api::load() {
                unsafe { (api.close)(self.handle) };
            }
            self.handle = std::ptr::null_mut();
        }
    }

    impl Drop for SimConnectSource {
        fn drop(&mut self) {
            self.close();
        }
    }

    // 他机注入那条 SimConnect 连接。
    //
    // 三个调用：
    //
    // ```text
    // SimConnect_AICreateNonATCAircraft   建一架，异步，回音是 ASSIGNED_OBJECT_ID
    // SimConnect_SetDataOnSimObject       挪位置，同步
    // SimConnect_AIRemoveObject           撤掉
    // ```
    //
    // # 新建是异步的，而且失败不走返回值
    //
    // `AICreateNonATCAircraft` 对一个不存在的机模标题**照样返回成功**，失败是
    // 过一会儿以一个 `SIMCONNECT_RECV_EXCEPTION` 回来的。而 EXCEPTION 里带的是
    // `send_id` 而不是 `request_id`，所以要在每次调用之后立刻问一次
    // `GetLastSentPacketID` 把 send id 记下来，否则失败了也不知道是哪一架。
    //
    // 光靠 EXCEPTION 还不够：也可能什么都不回（模拟器正在加载、连接断了）。
    // 所以再加一道超时——[`CREATE_TIMEOUT`] 之内没有回音就当失败。**两道都要**，
    // 一架卡在"已经要过、还没回音"的飞机会永远不被重试。

    use crate::inject::Action;
    use std::time::{Duration, Instant};

    /// 多久没有回音就当这一架建失败了。
    ///
    /// 给得比较宽，因为模拟器在加载场景时能停好几秒；给得太紧会把"正在忙"
    /// 当成"机模不存在"，然后一路退化到 C172。
    const CREATE_TIMEOUT: Duration = Duration::from_secs(10);

    /// 新建请求的 id 从这里开始编号。
    const FIRST_REQUEST_ID: c_ulong = 100;

    struct Outstanding {
        callsign: String,
        asked: Instant,
    }

    /// 真正往模拟器里放他机的那一份。
    #[derive(Default)]
    pub struct SimConnectTraffic {
        handle: Handle,
        next_request: c_ulong,
        /// request id → 呼号。回音回来时靠它找回是谁。
        by_request: HashMap<c_ulong, Outstanding>,
        /// send id → request id。EXCEPTION 带的是 send id。
        by_send: HashMap<c_ulong, c_ulong>,
    }

    // Handle 是一个只在这一个线程里用的不透明指针。
    unsafe impl Send for SimConnectTraffic {}

    impl SimConnectTraffic {
        /// 记下刚发出去那一个包的 send id，好让 EXCEPTION 能反查回呼号。
        ///
        /// 问不到就算了——那只是少一条快速失败的路，超时那一道还在。
        fn remember_send(&mut self, api: &Api, request_id: c_ulong) {
            let mut send_id: c_ulong = 0;
            let got = unsafe { (api.get_last_sent_packet_id)(self.handle, &mut send_id) };
            if ok(got) {
                self.by_send.insert(send_id, request_id);
            }
        }

        fn create(
            &mut self,
            api: &Api,
            callsign: &str,
            title: &str,
            entry: &crate::traffic::Entry,
        ) -> Result<(), String> {
            // **建在它现在所在的位置**，不是 0°N 0°E。给零的话飞机会在几内亚湾
            // 外面出现半秒再跳过来，而且模拟器可能顺手去加载那一块地景。
            let (pitch, bank) =
                super::attitude_for_injection(entry.position.pitch, entry.position.bank);
            let position = InitPosition {
                latitude: entry.position.latitude,
                longitude: entry.position.longitude,
                altitude: entry.position.altitude,
                pitch,
                bank,
                heading: entry.position.heading,
                on_ground: u32::from(entry.position.on_ground) as c_ulong,
                airspeed: entry.position.groundspeed.max(0.0) as c_ulong,
            };
            let title = std::ffi::CString::new(title).map_err(|e| e.to_string())?;
            // 尾号就用呼号。MSFS 会把它画在机身上，也是在模拟器里认出这架的办法。
            let tail = std::ffi::CString::new(callsign).map_err(|e| e.to_string())?;
            let request_id = self.next_request;
            self.next_request += 1;
            let sent = unsafe {
                (api.ai_create_non_atc_aircraft)(
                    self.handle,
                    title.as_ptr(),
                    tail.as_ptr(),
                    position,
                    request_id,
                )
            };
            if !ok(sent) {
                return Err(format!("could not ask for {callsign}"));
            }
            self.remember_send(api, request_id);
            self.by_request.insert(
                request_id,
                Outstanding {
                    callsign: callsign.to_string(),
                    asked: Instant::now(),
                },
            );
            Ok(())
        }

        fn update(&mut self, api: &Api, object_id: u32, entry: &crate::traffic::Entry) {
            let (pitch, bank) =
                super::attitude_for_injection(entry.position.pitch, entry.position.bank);
            let mut data = TrafficPosition {
                latitude: entry.position.latitude,
                longitude: entry.position.longitude,
                altitude: entry.position.altitude,
                pitch,
                bank,
                heading: entry.position.heading,
                on_ground: if entry.position.on_ground { 1.0 } else { 0.0 },
                airspeed: entry.position.groundspeed,
            };
            unsafe {
                (api.set_data_on_sim_object)(
                    self.handle,
                    TRAFFIC_DEF_ID,
                    object_id as c_ulong,
                    SET_FLAG_DEFAULT,
                    0,
                    std::mem::size_of::<TrafficPosition>() as c_ulong,
                    &mut data as *mut TrafficPosition as *mut c_void,
                );
            }
        }

        fn remove(&mut self, api: &Api, object_id: u32) {
            let request_id = self.next_request;
            self.next_request += 1;
            unsafe {
                (api.ai_remove_object)(self.handle, object_id as c_ulong, request_id);
            }
        }

        /// 超时的那些当失败。
        fn expired(&mut self) -> Vec<String> {
            let now = Instant::now();
            let late: Vec<c_ulong> = self
                .by_request
                .iter()
                .filter(|(_, o)| now.duration_since(o.asked) > CREATE_TIMEOUT)
                .map(|(id, _)| *id)
                .collect();
            late.into_iter()
                .filter_map(|id| self.by_request.remove(&id).map(|o| o.callsign))
                .collect()
        }
    }

    impl TrafficSink for SimConnectTraffic {
        fn open(&mut self) -> Result<(), String> {
            let api = Api::load()?;
            let name = std::ffi::CString::new("msfs-for-can traffic").map_err(|e| e.to_string())?;
            let mut handle: Handle = std::ptr::null_mut();
            unsafe {
                if !ok((api.open)(
                    &mut handle,
                    name.as_ptr(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    0,
                )) {
                    return Err("could not open SimConnect; is the simulator running?".into());
                }
                // **顺序就是 TrafficPosition 里字段的顺序。** 见 TRAFFIC_VARS。
                for (simvar, units) in TRAFFIC_VARS {
                    let name = std::ffi::CString::new(simvar).map_err(|e| e.to_string())?;
                    let units = std::ffi::CString::new(units).map_err(|e| e.to_string())?;
                    if !ok((api.add_to_data_definition)(
                        handle,
                        TRAFFIC_DEF_ID,
                        name.as_ptr(),
                        units.as_ptr(),
                        DATATYPE_FLOAT64,
                        0.0,
                        c_ulong::MAX,
                    )) {
                        (api.close)(handle);
                        return Err(format!("the simulator does not know the SimVar {simvar}"));
                    }
                }
            }
            self.handle = handle;
            self.next_request = FIRST_REQUEST_ID;
            self.by_request.clear();
            self.by_send.clear();
            Ok(())
        }

        fn apply(&mut self, actions: &[Action]) -> Result<(), String> {
            if self.handle.is_null() {
                return Err("not connected".into());
            }
            let api = Api::load()?;
            for action in actions {
                match action {
                    Action::Create {
                        callsign,
                        equipment,
                        entry,
                    } => {
                        // `equipment` 这里已经是**机模标题**，不是机型码——
                        // 换算在调用方，因为候选表是可测的纯逻辑。
                        self.create(api, callsign, equipment, entry)?;
                    }
                    Action::Update {
                        object_id, entry, ..
                    } => self.update(api, *object_id, entry.as_ref()),
                    Action::Remove { object_id, .. } => self.remove(api, *object_id),
                }
            }
            Ok(())
        }

        fn pump(&mut self) -> Vec<(String, Option<u32>)> {
            let mut out = Vec::new();
            if self.handle.is_null() {
                return out;
            }
            let Ok(api) = Api::load() else {
                return out;
            };
            loop {
                let mut data: *mut Recv = std::ptr::null_mut();
                let mut size: c_ulong = 0;
                let result = unsafe { (api.get_next_dispatch)(self.handle, &mut data, &mut size) };
                if !ok(result) || data.is_null() {
                    break; // 队列空了
                }
                if !dispatch_has_bytes(size as usize, std::mem::size_of::<Recv>()) {
                    warn_short(&SHORT_HEADER_LOGGED, "header");
                    continue;
                }
                unsafe {
                    match (*data).id {
                        RECV_ID_ASSIGNED_OBJECT_ID => {
                            if !dispatch_has_bytes(
                                size as usize,
                                std::mem::size_of::<RecvAssignedObjectId>(),
                            ) {
                                warn_short(&SHORT_ASSIGNED_LOGGED, "assigned object");
                                continue;
                            }
                            let payload = data as *const RecvAssignedObjectId;
                            if let Some(o) = self.by_request.remove(&(*payload).request_id) {
                                out.push((o.callsign, Some((*payload).object_id as u32)));
                            }
                        }
                        RECV_ID_EXCEPTION => {
                            if !dispatch_has_bytes(
                                size as usize,
                                std::mem::size_of::<RecvException>(),
                            ) {
                                warn_short(&SHORT_EXCEPTION_LOGGED, "exception");
                                continue;
                            }
                            let payload = data as *const RecvException;
                            // EXCEPTION 带的是 send id，要经 by_send 转一道。
                            if let Some(request_id) = self.by_send.remove(&(*payload).send_id) {
                                if let Some(o) = self.by_request.remove(&request_id) {
                                    out.push((o.callsign, None));
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            // 什么都没回的那些也当失败，否则它们永远卡在"要过了、等回音"。
            out.extend(self.expired().into_iter().map(|c| (c, None)));
            out
        }

        fn close(&mut self) {
            if self.handle.is_null() {
                return;
            }
            if let Ok(api) = Api::load() {
                unsafe { (api.close)(self.handle) };
            }
            self.handle = std::ptr::null_mut();
            self.by_request.clear();
            self.by_send.clear();
        }
    }

    impl Drop for SimConnectTraffic {
        fn drop(&mut self) {
            self.close();
        }
    }
}
