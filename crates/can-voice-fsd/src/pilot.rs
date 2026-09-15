//! FSD 的**飞行员**那一侧。
//!
//! 逐条对照 can-fsd 的解析代码（`internal/fsd/conn.go`、`handler.go`、
//! `packet.go`）和 `docs/protocol.md`：
//!
//! ```text
//! 登录   $ID{呼号}:SERVER:{客户端ID}:{客户端名}:{主}:{次}:{CID}:{机器码}
//!        #AP{呼号}:SERVER:{CID}:{密码}:{等级}:{协议版本}:{模拟器}:{真实姓名}
//! 位置   @{应答机模式}:{呼号}:{squawk}:{等级}:{纬度}:{经度}:{高度}:{地速}:{PBH}:{气压差}
//! 计划   $FP{呼号}:SERVER:{规则}:{机型}:{真空速}:{起飞地}:{预计起飞}:{实际起飞}
//!        :{巡航高度}:{目的地}:{航路小时}:{航路分钟}:{燃油小时}:{燃油分钟}
//!        :{备降场}:{备注}:{航路}          —— 一共 17 段，少一段整包被拒
//! 文字   #TM{呼号}:{收件人}:{正文}
//! 下线   #DP{呼号}:{CID}
//! ```
//!
//! 通播那一侧在 [`crate::packet`]，两边共用呼号校验、`$ID`、`sanitize_line`
//! 和 `redact`。

use crate::packet::{self, machine_id, sanitize_line, CallsignProblem, CLIENT_ID, PROTO_REVISION};

/// 模拟器编号，取自 can-fsd 的 `docs/enumerations.md`。
///
/// **`8` 是 "Microsoft Flight Simulator 2004"**，而 `can-audio` 的 X-Plane
/// 客户端一直报的就是 8 ——它谎报了自己是哪个模拟器。这里按实际的报。
pub const SIMULATOR_XPLANE_11: u32 = 15;
pub const SIMULATOR_XPLANE_12: u32 = 16;
pub const SIMULATOR_MSFS: u32 = 25;

/// 应答机模式，对应位置包的第一个字符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum XpdrMode {
    /// 待机 / 仅 mode A。
    #[default]
    Standby,
    /// 正常（mode C）。
    ModeC,
    /// 识别。
    Ident,
}

impl XpdrMode {
    pub fn letter(self) -> char {
        match self {
            XpdrMode::Standby => 'S',
            XpdrMode::ModeC => 'N',
            XpdrMode::Ident => 'Y',
        }
    }
}

/// 一架飞机的姿态。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Attitude {
    pub pitch: f64,
    pub bank: f64,
    pub heading: f64,
    pub on_ground: bool,
}

/// 把俯仰/坡度/航向压成 32 位整数。
///
/// can-fsd 的 `PitchBankHeading` 这样拆（`internal/fsd/packet.go`）：
///
/// ```text
/// pitch   = 位 22-31，乘 360/1024，再折到 -180..180
/// bank    = 位 12-21，同上
/// heading = 位 2-11，乘 360/1024
/// 位 1    = 是否在地面
/// ```
///
/// 角度先折到 0..360 再量化，否则负角度会溢出。
///
/// **不取负。** can-fsd 那边曾经在 `normaliseSigned` 里写过一句 `v = -v`，
/// 于是 datafeed（以及网站雷达）上每架飞机的俯仰和坡度都是反的；三份独立实现
/// （openfsd、Vatsim.Network、这一份）没有一个取负。
pub fn pack_pbh(pitch: f64, bank: f64, heading: f64, on_ground: bool) -> u32 {
    const RATIO: f64 = 1024.0 / 360.0;
    let quantise =
        |v: f64| -> u32 { ((v.rem_euclid(360.0) * RATIO).round() as i64 & 0x3FF) as u32 };
    let mut packed = (quantise(pitch) << 22) | (quantise(bank) << 12) | (quantise(heading) << 2);
    if on_ground {
        packed |= 0x2;
    }
    packed
}

/// [`pack_pbh`] 的逆运算，用来还原别人的姿态。
pub fn unpack_pbh(packed: u32) -> Attitude {
    const RATIO: f64 = 360.0 / 1024.0;
    const MASK: u32 = 0x3FF;
    let signed = |v: f64| if v > 180.0 { v - 360.0 } else { v };
    Attitude {
        pitch: signed(f64::from((packed >> 22) & MASK) * RATIO),
        bank: signed(f64::from((packed >> 12) & MASK) * RATIO),
        heading: f64::from((packed >> 2) & MASK) * RATIO,
        on_ground: packed & 0x2 != 0,
    }
}

/// 呼号的飞行员版：长度、字符集、保留名，**没有** `_ATIS` 那条。
pub fn check_pilot_callsign(callsign: &str) -> Result<String, CallsignProblem> {
    packet::check_callsign(callsign)
}

/// 一条飞行员连接的身份。
#[derive(Debug, Clone)]
pub struct PilotIdentity {
    pub callsign: String,
    pub cid: String,
    pub password: String,
    pub real_name: String,
    pub rating: u32,
    pub simulator: u32,
    /// 客户端名，进 `$ID`。
    pub client_name: String,
}

impl PilotIdentity {
    /// 每一格都过 [`sanitize_line`]，**密码也不例外**——理由和通播那边同一条：
    /// 带冒号的密码会让后面的字段全部错位，而打码按固定下标做，错位之后冒号后
    /// 那一截密码会原样进日志。
    pub fn new(
        callsign: &str,
        cid: &str,
        password: &str,
        real_name: &str,
        rating: u32,
        simulator: u32,
        client_name: &str,
    ) -> Self {
        Self {
            callsign: callsign.trim().to_uppercase(),
            cid: sanitize_line(cid),
            password: sanitize_line(password),
            real_name: sanitize_line(real_name),
            rating,
            simulator,
            client_name: sanitize_line(client_name),
        }
    }

    /// `$ID`。第 9 格（challenge）**故意留空**，服务端就不会发起 VATSIM
    /// 客户端质询——那套算法只有官方客户端有密钥表。
    pub fn id_packet(&self) -> String {
        format!(
            "$ID{}:SERVER:{CLIENT_ID}:{}:{}:{}:{}:{}",
            self.callsign,
            self.client_name,
            packet::CLIENT_MAJOR,
            packet::CLIENT_MINOR,
            self.cid,
            machine_id(&self.callsign)
        )
    }

    /// `#AP` —— 飞行员登录。**和管制端的 `#AA` 字段顺序不同**：
    /// 这边是 CID/密码在前、真实姓名在最后。
    pub fn login_packet(&self) -> String {
        format!(
            "#AP{}:SERVER:{}:{}:{}:{PROTO_REVISION}:{}:{}",
            self.callsign, self.cid, self.password, self.rating, self.simulator, self.real_name
        )
    }

    /// `#DP` —— 下线。管制端是 `#DA`。
    pub fn logoff_packet(&self) -> String {
        format!("#DP{}:{}", self.callsign, self.cid)
    }
}

/// 一次位置上报。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PilotPosition {
    pub mode: XpdrMode,
    pub squawk: u16,
    pub rating: u32,
    pub latitude: f64,
    pub longitude: f64,
    /// 真高（英尺）。
    pub altitude: i32,
    pub groundspeed: i32,
    pub pitch: f64,
    pub bank: f64,
    pub heading: f64,
    pub on_ground: bool,
    /// 气压修正量。**高度字段报的是真高，加上它才是应答机报的气压高度。**
    /// 写死 0 的话管制端看到的高度和座舱高度表能差一千英尺。
    pub pressure_delta: i32,
}

impl PilotPosition {
    pub fn packet(&self, callsign: &str) -> String {
        format!(
            "@{}:{callsign}:{:04}:{}:{:.5}:{:.5}:{}:{}:{}:{}",
            self.mode.letter(),
            self.squawk,
            self.rating,
            self.latitude,
            self.longitude,
            self.altitude,
            self.groundspeed,
            pack_pbh(self.pitch, self.bank, self.heading, self.on_ground),
            self.pressure_delta
        )
    }
}

/// 一份飞行计划。字段顺序和数量抄自 can-fsd 的 `docs/protocol.md`。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FlightPlan {
    /// `I` / `V` / `Y` / `Z`，只取一个字符。
    pub rules: String,
    pub aircraft: String,
    pub cruise_speed: String,
    pub departure: String,
    pub departure_time: String,
    pub actual_time: String,
    pub cruise_altitude: String,
    pub arrival: String,
    pub enroute_hours: String,
    pub enroute_minutes: String,
    pub fuel_hours: String,
    pub fuel_minutes: String,
    pub alternate: String,
    pub remarks: String,
    pub route: String,
}

/// `$FP` 一共 **17 段**。
///
/// 少一段服务端回 "Too few fields for $FP"——`can-audio` 那边真实日志里每次
/// 提交都是这个：漏了燃油小时/分钟两段，还把航路时间那两段当成了备降时间。
pub const FLIGHT_PLAN_FIELDS: usize = 17;

impl FlightPlan {
    pub fn packet(&self, callsign: &str) -> String {
        let up = |s: &str| sanitize_line(s).to_uppercase();
        let or_zero = |s: &str| {
            let v = sanitize_line(s);
            if v.is_empty() {
                "0".to_string()
            } else {
                v
            }
        };
        let rules = {
            let r = sanitize_line(&self.rules);
            r.chars().next().unwrap_or('I').to_string()
        };
        let fields = [
            rules,
            sanitize_line(&self.aircraft),
            sanitize_line(&self.cruise_speed),
            up(&self.departure),
            sanitize_line(&self.departure_time),
            sanitize_line(&self.actual_time),
            sanitize_line(&self.cruise_altitude),
            up(&self.arrival),
            or_zero(&self.enroute_hours),
            or_zero(&self.enroute_minutes),
            or_zero(&self.fuel_hours),
            or_zero(&self.fuel_minutes),
            up(&self.alternate),
            sanitize_line(&self.remarks),
            up(&self.route),
        ];
        // 收件人按文档是 SERVER；`*A` 是服务端转发给管制时用的，不是填报用的。
        format!("$FP{callsign}:SERVER:{}", fields.join(":"))
    }
}

/// 督导频道。FSD 里 `*S` 是一个**收件人**而不是一条命令：客户端把 `.wallop`
/// 翻成发往这个地址的普通 `#TM`，服务端认出它再转给所有在线督导。
pub const WALLOP_RECIPIENT: &str = "*S";

/// 认识的点命令。
const DOT_COMMANDS: [(&str, &str); 1] = [(".wallop", WALLOP_RECIPIENT)];

/// 把用户输入的一行翻成 `(收件人, 正文)`。
///
/// **不认识的点命令原样发出去**，既不报错也不吞掉：猜不出用户是想打一条命令
/// 还是真要发一句以点开头的话，而吞掉一条本该发出去的消息，比把一句奇怪的话
/// 发到频率上更糟。
///
/// EuroScope 和 CRC 是在**客户端**把 `.wallop` 翻成正确的包的。`can-audio` 的
/// 飞行员端原来没有这一步：用户打的 `.wallop 求助` 被当成普通正文，跟着收件人
/// 框（空的时候是 COM1 频率）发到频率上去了——督导收不到，而界面还回一行
/// "已发送"。
pub fn parse_dot_command(text: &str) -> (Option<&'static str>, String) {
    let stripped = text.trim();
    if !stripped.starts_with('.') {
        return (None, stripped.to_string());
    }
    let mut parts = stripped.splitn(2, char::is_whitespace);
    let head = parts.next().unwrap_or("").to_lowercase();
    match DOT_COMMANDS.iter().find(|(name, _)| *name == head) {
        // 正文一个字都不动——大小写、标点和内部空格都是用户写给督导的原话。
        Some((_, recipient)) => (
            Some(recipient),
            parts.next().unwrap_or("").trim().to_string(),
        ),
        None => (None, stripped.to_string()),
    }
}

/// `#TM` —— 一条文字消息。正文为空时不发。
pub fn text_message(callsign: &str, recipient: &str, message: &str) -> Option<String> {
    let message = sanitize_line(message);
    if message.is_empty() {
        return None;
    }
    Some(format!(
        "#TM{callsign}:{}:{message}",
        sanitize_line(recipient)
    ))
}

/// `$CQ…ATIS` —— 问某个管制席位要文字通播。
pub fn request_atis(callsign: &str, target: &str) -> String {
    format!(
        "$CQ{callsign}:{}:ATIS",
        sanitize_line(target).to_uppercase()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 从 can-fsd 的 `internal/fsd/packet.go` 逐行转写过来的参照实现。
    ///
    /// **它才是判定标准。** `unpack_pbh` 改了要能对上它——这一对独立实现是
    /// 唯一能发现"两边各自都自洽、但和服务端对不上"的东西，而那种错误在本地
    /// 怎么测都是绿的：原始包是被服务端**原样转发**给别的飞行员的，所以飞机
    /// 之间的渲染不受影响，只有这个服务自己发布的 datafeed（以及网站雷达）
    /// 会反过来。
    fn reference_unpack(packed: u32) -> (f64, f64, f64) {
        const RATIO: f64 = 360.0 / 1024.0;
        const MASK: u32 = 0x3FF;
        // normaliseSigned：把 0..360 折到 -180..180。**不取负。**
        fn normalise_signed(mut v: f64) -> f64 {
            if v > 180.0 {
                v -= 360.0;
            } else if v <= -180.0 {
                v += 360.0;
            }
            v
        }
        let pitch = normalise_signed(f64::from((packed >> 22) & MASK) * RATIO);
        let bank = normalise_signed(f64::from((packed >> 12) & MASK) * RATIO);
        let mut heading = f64::from((packed >> 2) & MASK) * RATIO;
        while heading < 0.0 {
            heading += 360.0;
        }
        while heading >= 360.0 {
            heading -= 360.0;
        }
        (pitch, bank, heading)
    }

    #[test]
    fn unpacking_agrees_with_the_server() {
        for packed in [
            0u32,
            0x2,
            0xFFFF_FFFF,
            pack_pbh(0.0, 0.0, 0.0, false),
            pack_pbh(10.0, -25.0, 140.0, false),
            pack_pbh(-3.5, 0.0, 359.9, true),
            pack_pbh(179.0, 179.0, 179.0, false),
            pack_pbh(-179.0, -179.0, 1.0, false),
        ] {
            let got = unpack_pbh(packed);
            let (p, b, h) = reference_unpack(packed);
            assert!((got.pitch - p).abs() < 1e-9, "pitch of {packed:#x}");
            assert!((got.bank - b).abs() < 1e-9, "bank of {packed:#x}");
            assert!((got.heading - h).abs() < 1e-9, "heading of {packed:#x}");
        }
    }

    /// **俯仰和坡度不取负。**
    ///
    /// can-fsd 那边曾经在 `normaliseSigned` 里写过一句 `v = -v`，于是
    /// datafeed 上每架飞机的姿态都是反的。抬头就该报正的俯仰。
    #[test]
    fn a_nose_up_attitude_stays_nose_up() {
        let got = unpack_pbh(pack_pbh(10.0, 0.0, 0.0, false));
        assert!(got.pitch > 0.0, "nose-up came back as {}", got.pitch);
        let got = unpack_pbh(pack_pbh(-10.0, 0.0, 0.0, false));
        assert!(got.pitch < 0.0, "nose-down came back as {}", got.pitch);
        // 水平时就是 0，不是 -0 也不是 360。
        assert_eq!(unpack_pbh(pack_pbh(0.0, 0.0, 0.0, false)).pitch, 0.0);
        // 右坡度同理。
        let right = unpack_pbh(pack_pbh(0.0, 25.0, 0.0, false));
        assert!(right.bank > 0.0, "right bank came back as {}", right.bank);
    }

    /// 量化到 1024 格，所以往返有约 0.35° 的台阶——这是协议的精度，不是 bug。
    #[test]
    fn a_round_trip_keeps_the_angles_within_one_step() {
        let step = 360.0 / 1024.0;
        for (pitch, bank, heading) in [
            (0.0, 0.0, 0.0),
            (7.5, -12.25, 271.0),
            (-4.0, 30.0, 89.9),
            (0.0, 0.0, 359.9),
        ] {
            let got = unpack_pbh(pack_pbh(pitch, bank, heading, false));
            assert!(
                (got.pitch - pitch).abs() <= step,
                "pitch {pitch} -> {}",
                got.pitch
            );
            assert!(
                (got.bank - bank).abs() <= step,
                "bank {bank} -> {}",
                got.bank
            );
            let dh = (got.heading - heading)
                .abs()
                .min(360.0 - (got.heading - heading).abs());
            assert!(dh <= step, "heading {heading} -> {}", got.heading);
        }
    }

    #[test]
    fn the_on_ground_bit_survives() {
        assert!(unpack_pbh(pack_pbh(0.0, 0.0, 0.0, true)).on_ground);
        assert!(!unpack_pbh(pack_pbh(0.0, 0.0, 0.0, false)).on_ground);
    }

    /// **`$FP` 一共 17 段。** 少一段服务端回 "Too few fields for $FP"，
    /// 整份计划被拒——而界面那边看起来只是"提交了没反应"。
    #[test]
    fn a_flight_plan_has_exactly_seventeen_fields() {
        let plan = FlightPlan {
            rules: "I".into(),
            aircraft: "A320/M-SDE2E3FGHIRWY/LB1".into(),
            cruise_speed: "N0450".into(),
            departure: "zspd".into(),
            departure_time: "1200".into(),
            actual_time: "0".into(),
            cruise_altitude: "FL350".into(),
            arrival: "zbaa".into(),
            enroute_hours: "2".into(),
            enroute_minutes: "10".into(),
            ..Default::default()
        };
        let p = plan.packet("CES123");
        assert_eq!(p.split(':').count(), FLIGHT_PLAN_FIELDS, "{p}");
        assert!(p.starts_with("$FPCES123:SERVER:I:"), "{p}");
        // 机场代码大写，空的时间字段补 0 而不是留空。
        assert!(p.contains(":ZSPD:"), "{p}");
        assert!(p.contains(":ZBAA:"), "{p}");
        assert!(
            p.ends_with(":0:0:::"),
            "fuel defaults to 0, the rest stay empty: {p}"
        );
    }

    /// 计划里的冒号会把后面每一段都挪一格——真实案例见 SweatBox 那批脚本里
    /// 那条多了一个冒号的 `$FP`。
    #[test]
    fn a_colon_in_the_remarks_cannot_shift_the_fields() {
        let plan = FlightPlan {
            remarks: "PBN/A1B1 RMK:/TCAS".into(),
            ..Default::default()
        };
        let p = plan.packet("CES123");
        assert_eq!(p.split(':').count(), FLIGHT_PLAN_FIELDS, "{p}");
        assert!(p.contains("RMK /TCAS"), "{p}");
    }

    /// 飞行员登录是 `#AP`，字段顺序和管制端的 `#AA` **不一样**：
    /// 这边 CID/密码在前、真实姓名在最后。
    #[test]
    fn the_pilot_login_is_not_the_controller_one() {
        let id = PilotIdentity::new(
            "CES123",
            "1234",
            "pw",
            "Zhang San",
            1,
            SIMULATOR_MSFS,
            "XPC",
        );
        let p = id.login_packet();
        assert_eq!(p, "#APCES123:SERVER:1234:pw:1:100:25:Zhang San");
        assert_eq!(id.logoff_packet(), "#DPCES123:1234");
    }

    #[test]
    fn a_colon_in_the_password_cannot_shift_the_fields_or_leak() {
        let id = PilotIdentity::new("CES123", "1234", "pa:ss", "R N", 1, SIMULATOR_MSFS, "XPC");
        let p = id.login_packet();
        assert_eq!(p.split(':').count(), 8, "{p}");
        assert!(p.contains("pa ss"), "{p}");
    }

    #[test]
    fn the_id_packet_leaves_the_challenge_field_empty() {
        let id = PilotIdentity::new("CES123", "1234", "pw", "R N", 1, SIMULATOR_XPLANE_12, "XPC");
        let p = id.id_packet();
        assert_eq!(
            p.split(':').count(),
            8,
            "a ninth field would be a challenge: {p}"
        );
        assert!(p.starts_with("$IDCES123:SERVER:0001:XPC:"), "{p}");
    }

    /// squawk 补到四位。`0` 发成 `0` 而不是 `0000` 的话服务端读出来是另一个码。
    #[test]
    fn the_position_packet_pads_the_squawk() {
        let pos = PilotPosition {
            mode: XpdrMode::ModeC,
            squawk: 20,
            rating: 1,
            latitude: 31.142_33,
            longitude: 121.790_84,
            altitude: 35_000,
            groundspeed: 450,
            pitch: 0.0,
            bank: 0.0,
            heading: 0.0,
            on_ground: false,
            pressure_delta: -120,
        };
        let p = pos.packet("CES123");
        assert!(
            p.starts_with("@N:CES123:0020:1:31.14233:121.79084:35000:450:"),
            "{p}"
        );
        assert!(
            p.ends_with(":-120"),
            "the pressure delta is the last field: {p}"
        );
        assert_eq!(p.split(':').count(), 10, "{p}");
    }

    #[test]
    fn the_transponder_letters_are_the_ones_the_server_reads() {
        assert_eq!(XpdrMode::Standby.letter(), 'S');
        assert_eq!(XpdrMode::ModeC.letter(), 'N');
        assert_eq!(XpdrMode::Ident.letter(), 'Y');
    }

    /// `.wallop` 要在**客户端**翻成发往 `*S` 的 `#TM`。不翻的话它会被当成
    /// 普通正文，跟着收件人框发到频率上，督导一个字都收不到，而界面还回一行
    /// "已发送"。
    #[test]
    fn wallop_is_translated_on_this_side() {
        assert_eq!(
            parse_dot_command(".wallop 有人抢频率"),
            (Some("*S"), "有人抢频率".to_string())
        );
        // 命令名不分大小写，正文一个字不动。
        assert_eq!(
            parse_dot_command(".WALLOP  Please Help.  "),
            (Some("*S"), "Please Help.".to_string())
        );
        assert_eq!(parse_dot_command(".wallop"), (Some("*S"), String::new()));
    }

    /// **不认识的点命令原样发出去**，既不报错也不吞掉：吞掉一条本该发出去的
    /// 消息，比把一句奇怪的话发到频率上更糟。
    #[test]
    fn an_unknown_dot_command_is_passed_through() {
        assert_eq!(
            parse_dot_command(".chat 你好"),
            (None, ".chat 你好".to_string())
        );
        assert_eq!(
            parse_dot_command("正常一句话"),
            (None, "正常一句话".to_string())
        );
    }

    #[test]
    fn an_empty_message_is_not_sent() {
        assert_eq!(text_message("CES123", "ZSPD_TWR", "   "), None);
        assert_eq!(
            text_message("CES123", "zspd_twr", "request push").as_deref(),
            Some("#TMCES123:zspd_twr:request push")
        );
    }

    /// 保留名不能当呼号。`FP` 尤其要挡——它是 `$FP` 的收件人形式。
    #[test]
    fn the_reserved_names_are_not_callsigns() {
        assert!(matches!(
            check_pilot_callsign("server"),
            Err(CallsignProblem::Reserved(_))
        ));
        assert!(matches!(
            check_pilot_callsign("FP"),
            Err(CallsignProblem::Reserved(_))
        ));
        // 飞行员呼号**没有** _ATIS 那条要求。
        assert_eq!(check_pilot_callsign("ces123").as_deref(), Ok("CES123"));
    }
}
