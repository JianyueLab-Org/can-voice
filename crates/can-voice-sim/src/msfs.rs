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

#[cfg(windows)]
pub use ffi::SimConnectSource;

#[cfg(not(windows))]
pub type SimConnectSource = Unavailable;

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

    use super::{all_simvars, SimVarSource};
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::os::raw::{c_char, c_double, c_int, c_ulong};

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

    #[repr(C)]
    struct Recv {
        size: c_ulong,
        version: c_ulong,
        id: c_ulong,
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
                        u32::MAX,
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
                unsafe {
                    if (*data).id != RECV_ID_SIMOBJECT_DATA {
                        continue;
                    }
                    let payload = data as *const RecvSimObjectData;
                    let count = (*payload).define_count as usize;
                    // **只认数量对得上的那一包。** 对不上说明定义和回包错位了，
                    // 而错位之后每一个值单独看都是合法的数字。
                    if count != names.len() {
                        continue;
                    }
                    let values = (payload as *const u8)
                        .add(std::mem::size_of::<RecvSimObjectData>())
                        as *const c_double;
                    for (i, name) in names.iter().enumerate() {
                        out.insert(*name, *values.add(i));
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
}
