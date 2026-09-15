//! 模拟器数据链路：把机位、姿态、速度、无线电和应答机从模拟器里取出来。
//!
//! # 为什么两个模拟器共用一个 crate
//!
//! `can-audio` 的 `xpc/` 和 `msfs/` 是两份近似副本——`fsdpilot.py`、
//! `traffic.py`、`observer.py` 逐个文件对应，连 `snapshot()` 的字段都
//! **刻意保持一致**，而那一致性靠的是人每次改两遍。改漏一边的后果是两个
//! 模拟器的用户在网上看到的是两套不同的飞机。
//!
//! 这里把那份"一致"变成类型：[`Snapshot`] 只有一个定义，两条链路都往它里面填。
//! 换算、应答机模式判定、气压修正量也只有一份。
//!
//! [`xplane`] 是 UDP（BECN 信标 + RREF 订阅），纯字节活，可以完整测试。
//! [`msfs`] 是 SimConnect，**Windows-only 的 C API**。

pub mod bridge;
pub mod xplane;

pub mod msfs;

use can_voice_fsd::pilot::XpdrMode;

pub const METRES_PER_FOOT: f64 = 0.3048;
pub const KNOTS_PER_MPS: f64 = 1.943_844_492_440_6;
/// 标准气压，inHg。
pub const STANDARD_PRESSURE_INHG: f64 = 29.92;
/// 低于这个地速算"停着"。
pub const PARKED_SPEED_KT: i32 = 1;

/// 高度表拨得出来的气压范围，inHg。
///
/// 超出这个范围的读数是模拟器还没初始化完时的垃圾值（见过 0 和 1013）。
/// **拿 0 去算修正量会把飞机在雷达上挪三万英尺**，所以宁可不修正。
pub const BARO_RANGE: (f64, f64) = (25.0, 32.0);

/// 一帧模拟器数据，已经换算成 FSD 要的单位。
///
/// **两条链路填的是同一个结构**，不是两个"字段碰巧一样"的结构。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Snapshot {
    pub latitude: f64,
    pub longitude: f64,
    /// 真高，英尺。
    pub altitude: i32,
    /// 气压高度减真高，英尺。位置包的最后一个字段。
    pub pressure_delta: i32,
    pub agl: i32,
    pub groundspeed: i32,
    pub pitch: f64,
    pub bank: f64,
    pub heading: f64,
    pub squawk: u16,
    pub xpdr_mode: XpdrMode,
    /// COM 频率，MHz。读不到是 `None`——**不是 0**：0 是一个合法的路由键。
    pub com1: Option<f64>,
    pub com2: Option<f64>,
    pub com1_power: bool,
    pub on_ground: bool,
    /// 报给别人做动画用的。X-Plane 那条链路目前不填。
    pub animation: Option<Animation>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            latitude: 0.0,
            longitude: 0.0,
            altitude: 0,
            pressure_delta: 0,
            agl: 0,
            groundspeed: 0,
            pitch: 0.0,
            bank: 0.0,
            heading: 0.0,
            squawk: 2000,
            xpdr_mode: XpdrMode::ModeC,
            com1: None,
            com2: None,
            com1_power: true,
            on_ground: false,
            animation: None,
        }
    }
}

/// 别人画这架飞机时要的姿态之外的东西。
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize)]
pub struct Animation {
    pub gear_down: bool,
    /// 0.0–1.0。
    pub flaps: f64,
    pub spoilers: bool,
    pub engines_on: bool,
    pub beacon_on: bool,
    pub landing_on: bool,
    pub taxi_on: bool,
    pub strobe_on: bool,
}

/// 位置包最后一个字段：气压高度减真高，英尺。
///
/// 应答机报的是**气压高度**（高度表拨 29.92 时读到的数），而位置包第 7 个字段
/// 报的是**真高**——两者在巡航高度上能差一千英尺，这就是"座舱里 35000、雷达上
/// 34000"的由来。协议把差值单独放在最后一个字段，正是为了让画他机的客户端拿
/// 真高摆飞机、让管制端拿真高加修正量当高度显示。
///
/// 温度偏差带来的误差不需要另算：指示高度本身就带着它，真高不带，相减自然就有。
///
/// 读不到就返回 0，也就是退回不修正——**宁可不修正，不能瞎修正**。
pub fn pressure_delta(
    indicated_ft: Option<f64>,
    baro_inhg: Option<f64>,
    true_altitude_ft: i32,
) -> i32 {
    let (Some(indicated), Some(baro)) = (indicated_ft, baro_inhg) else {
        return 0;
    };
    if !(BARO_RANGE.0..=BARO_RANGE.1).contains(&baro) {
        return 0;
    }
    let pressure_altitude = indicated + (STANDARD_PRESSURE_INHG - baro) * 1000.0;
    (pressure_altitude - f64::from(true_altitude_ft)).round() as i32
}

/// 模拟器的应答机档位 → 位置包要的模式。
///
/// 模拟器那边的取值是 0 关 / 1 待机 / 2 开 / 3 测试或 C，所以 `>= 2` 算在线。
/// **读不到当在线**：默认"关"等于在拿不准的时候主动把自己从管制端的标牌上抹掉，
/// 方向反了。
///
/// **待机和关只在飞机确实停着的时候才当真。** 冷舱的飞机不该在雷达上是个亮着的
/// C 模式目标，而冷舱恰恰就是"停在机坪上没动"这一种；一架已经在滑行或者已经
/// 离地的飞机还报待机，对管制没有任何好处——待机在位置包里是包头的 `@S`，
/// EuroScope 收到就当成没有 C 模式的目标，标牌上的**高度和地速会一起空掉**。
pub fn xpdr_mode(raw: Option<i32>, on_ground: bool, groundspeed_kt: i32) -> XpdrMode {
    let Some(raw) = raw else {
        return XpdrMode::ModeC;
    };
    if raw >= 2 {
        return XpdrMode::ModeC;
    }
    if on_ground && groundspeed_kt < PARKED_SPEED_KT {
        return XpdrMode::Standby;
    }
    XpdrMode::ModeC
}

/// 把 BCD 编码的 squawk 转成十进制。MSFS 的 `TRANSPONDER CODE:1` 是 BCD。
pub fn bcd_to_squawk(bcd: u32) -> u16 {
    let mut out = 0u16;
    for shift in [12, 8, 4, 0] {
        let digit = ((bcd >> shift) & 0xF) as u16;
        // 八进制的应答机码里不该出现 8/9；出现了就是垃圾值，整码作废。
        if digit > 7 {
            return 2000;
        }
        out = out * 10 + digit;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 巡航高度上气压高度和真高能差一千英尺。
    #[test]
    fn the_pressure_delta_is_what_makes_the_label_match_the_cockpit() {
        // 高度表拨 29.92 时两者相等，差值只剩指示与真高的差。
        assert_eq!(pressure_delta(Some(35_000.0), Some(29.92), 35_000), 0);
        // 拨 30.92（比标准高一寸）时，指示高度比气压高度低一千英尺。
        assert_eq!(pressure_delta(Some(34_000.0), Some(30.92), 34_000), -1000);
    }

    /// **宁可不修正，不能瞎修正。** 模拟器还没初始化完时高度表会读出 0 或者
    /// 1013，拿它去算会把飞机在雷达上挪三万英尺。
    #[test]
    fn a_nonsense_barometer_reading_is_ignored_rather_than_used() {
        assert_eq!(pressure_delta(Some(35_000.0), Some(0.0), 35_000), 0);
        assert_eq!(pressure_delta(Some(35_000.0), Some(1013.0), 35_000), 0);
        assert_eq!(pressure_delta(Some(35_000.0), None, 35_000), 0);
        assert_eq!(pressure_delta(None, Some(29.92), 35_000), 0);
    }

    /// 读不到当**在线**。默认"关"等于在拿不准的时候主动把自己从管制端的标牌上
    /// 抹掉，方向反了。
    #[test]
    fn an_unknown_transponder_state_counts_as_on() {
        assert_eq!(xpdr_mode(None, false, 0), XpdrMode::ModeC);
    }

    /// 待机只在飞机**确实停着**的时候当真。
    #[test]
    fn standby_is_only_believed_when_the_aircraft_is_parked() {
        assert_eq!(xpdr_mode(Some(1), true, 0), XpdrMode::Standby);
        // 已经在滑行：报待机对管制没有好处，EuroScope 会把高度和地速一起空掉。
        assert_eq!(xpdr_mode(Some(1), true, 12), XpdrMode::ModeC);
        // 已经离地。
        assert_eq!(xpdr_mode(Some(0), false, 250), XpdrMode::ModeC);
        // 开着就是开着。
        assert_eq!(xpdr_mode(Some(2), true, 0), XpdrMode::ModeC);
        assert_eq!(xpdr_mode(Some(3), true, 0), XpdrMode::ModeC);
    }

    #[test]
    fn a_bcd_squawk_becomes_the_number_the_pilot_dialled() {
        assert_eq!(bcd_to_squawk(0x2000), 2000);
        assert_eq!(bcd_to_squawk(0x7700), 7700);
        assert_eq!(bcd_to_squawk(0x1234), 1234);
        // 八进制里没有 8 和 9——出现了就是垃圾值，整码作废回到 2000。
        assert_eq!(bcd_to_squawk(0x1284), 2000);
    }
}
