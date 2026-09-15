//! 飞行员那一侧的角色，跑在 [`crate::session`] 的骨架上。
//!
//! 和通播席位共用连接、登录、重连、超时那一套；这里只有"飞行员是什么样"：
//! `#AP` 登录、每秒 5 次位置包（停在地面上降到 5 秒一次）、把别人的位置包
//! 攒成他机表。

use crate::packet::{CallsignProblem, Incoming};
use crate::pilot::{
    self, unpack_pbh, Attitude, FlightPlan, PilotIdentity, PilotPosition, XpdrMode,
};
use crate::session::{self, Role, SessionHandle};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// 每秒 5 次，和 VATSIM 客户端一致。
pub const POSITION_INTERVAL: Duration = Duration::from_millis(200);
/// 停在地面上没动时降到这个频率，省得刷屏。
pub const SLOW_POSITION_INTERVAL: Duration = Duration::from_secs(5);
/// 按一下识别亮多久。
pub const IDENT_DURATION: Duration = Duration::from_secs(8);

pub use crate::session::{FsdEvent, FsdState, Reason, RECONNECT_LIMIT};

/// 网上另一架飞机。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Traffic {
    pub callsign: String,
    pub squawk: u16,
    pub latitude: f64,
    pub longitude: f64,
    pub altitude: i32,
    pub groundspeed: i32,
    pub attitude: Attitude,
}

/// 网上一个管制席位。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Controller {
    pub callsign: String,
    /// MHz。协议里频率压成五位（`18000` = 118.000），这里还原成兆赫。
    pub frequency: f64,
    pub facility: u32,
    pub rating: u32,
    pub latitude: f64,
    pub longitude: f64,
    pub vis_range: u32,
}

/// 飞行员这条链路上发生的事，和 [`FsdEvent`] 分开走。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum PilotEvent {
    Traffic(Box<Traffic>),
    /// 一架飞机下线了。
    TrafficGone(String),
    Controller(Box<Controller>),
    ControllerGone(String),
    Text {
        sender: String,
        recipient: String,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct PilotConfig {
    pub host: String,
    pub port: u16,
    pub identity: PilotIdentity,
    pub reconnect_limit: u32,
}

pub enum PilotCommand {
    Position(Box<PilotPosition>),
    Text { recipient: String, message: String },
    FilePlan(Box<FlightPlan>),
    Ident,
    RequestAtis(String),
}

struct PilotRole {
    identity: PilotIdentity,
    /// 最近一帧位置。**没有就不发包**——一个全零的位置包会把飞机放在
    /// 几内亚湾外海，而那在雷达上看着像一架真飞机。
    position: Option<PilotPosition>,
    ident_until: Option<Instant>,
    events: broadcast::Sender<PilotEvent>,
    /// 别人的呼号 → 最近一次收到的位置。只用来判断"这架还在不在"。
    seen: HashMap<String, Instant>,
}

impl Role for PilotRole {
    type Command = PilotCommand;

    fn callsign(&self) -> &str {
        &self.identity.callsign
    }

    fn validated_callsign(&self) -> Result<String, CallsignProblem> {
        pilot::check_pilot_callsign(&self.identity.callsign)
    }

    fn adopt_callsign(&mut self, callsign: String) {
        self.identity.callsign = callsign;
    }

    fn rating(&self) -> u32 {
        self.identity.rating
    }

    fn handshake_packets(&self) -> Vec<String> {
        vec![
            self.identity.id_packet(),
            self.identity.login_packet(),
            format!("$CQ{}:SERVER:CAPS", self.identity.callsign),
        ]
    }

    fn logoff_packet(&self) -> String {
        self.identity.logoff_packet()
    }

    fn tick(&mut self) -> Result<Vec<String>, Reason> {
        let Some(mut position) = self.position else {
            // 模拟器还没连上。不发包比发一个假位置好。
            return Ok(Vec::new());
        };
        if self.ident_until.is_some_and(|until| Instant::now() < until) {
            position.mode = XpdrMode::Ident;
        } else {
            self.ident_until = None;
        }
        Ok(vec![position.packet(&self.identity.callsign)])
    }

    fn tick_interval(&self) -> Duration {
        match self.position {
            Some(p) if p.on_ground && p.groundspeed < 1 => SLOW_POSITION_INTERVAL,
            _ => POSITION_INTERVAL,
        }
    }

    fn on_packet(&mut self, _incoming: &Incoming, raw: &str) -> Vec<String> {
        // 他机和管制席位不走 `packet::parse`：那一份只认这一侧**要回话**的包，
        // 而这两种是纯粹的通告。
        if let Some(t) = parse_traffic(raw) {
            if t.callsign != self.identity.callsign {
                self.seen.insert(t.callsign.clone(), Instant::now());
                let _ = self.events.send(PilotEvent::Traffic(Box::new(t)));
            }
        } else if let Some(c) = parse_controller(raw) {
            let _ = self.events.send(PilotEvent::Controller(Box::new(c)));
        } else if let Some(cs) = raw.strip_prefix("#DP").and_then(|r| r.split(':').next()) {
            self.seen.remove(cs);
            let _ = self.events.send(PilotEvent::TrafficGone(cs.to_string()));
        } else if let Some(cs) = raw.strip_prefix("#DA").and_then(|r| r.split(':').next()) {
            let _ = self.events.send(PilotEvent::ControllerGone(cs.to_string()));
        } else if let Some(rest) = raw.strip_prefix("#TM") {
            let mut parts = rest.splitn(3, ':');
            if let (Some(sender), Some(recipient), Some(message)) =
                (parts.next(), parts.next(), parts.next())
            {
                let _ = self.events.send(PilotEvent::Text {
                    sender: sender.to_string(),
                    recipient: recipient.to_string(),
                    message: message.to_string(),
                });
            }
        }
        Vec::new()
    }

    fn on_command(&mut self, command: PilotCommand) -> Vec<String> {
        match command {
            // 位置只存不发：发是 tick 的事，每秒 5 次。模拟器推得比这快，
            // 照单转发会把服务端刷爆。
            PilotCommand::Position(p) => {
                self.position = Some(*p);
                Vec::new()
            }
            PilotCommand::Text { recipient, message } => {
                // `.wallop` 在**这一步**翻成发往 `*S` 的消息。不翻的话它会被
                // 当成普通正文发到频率上，督导一个字都收不到。
                let (dot, body) = pilot::parse_dot_command(&message);
                let to = dot.unwrap_or(&recipient);
                pilot::text_message(&self.identity.callsign, to, &body)
                    .into_iter()
                    .collect()
            }
            PilotCommand::FilePlan(plan) => vec![plan.packet(&self.identity.callsign)],
            PilotCommand::Ident => {
                self.ident_until = Some(Instant::now() + IDENT_DURATION);
                // 立刻发一个亮着的位置包，不等下一个 tick——按下识别到雷达上
                // 亮起来之间的那 200 毫秒，管制正盯着屏幕等。
                self.tick().unwrap_or_default()
            }
            PilotCommand::RequestAtis(target) => {
                vec![pilot::request_atis(&self.identity.callsign, &target)]
            }
        }
    }
}

/// `@{模式}:{呼号}:{squawk}:{等级}:{纬度}:{经度}:{高度}:{地速}:{PBH}:{气压差}`
pub fn parse_traffic(raw: &str) -> Option<Traffic> {
    let rest = raw.strip_prefix('@')?;
    let f: Vec<&str> = rest.split(':').collect();
    if f.len() < 9 {
        return None;
    }
    Some(Traffic {
        callsign: f[1].to_string(),
        squawk: f[2].parse().unwrap_or(0),
        latitude: f[4].parse().ok()?,
        longitude: f[5].parse().ok()?,
        altitude: f[6].parse().unwrap_or(0),
        groundspeed: f[7].parse().unwrap_or(0),
        attitude: unpack_pbh(f[8].parse().unwrap_or(0)),
    })
}

/// `%{呼号}:{频率}:{席位类型}:{可视范围}:{等级}:{纬度}:{经度}:0`
pub fn parse_controller(raw: &str) -> Option<Controller> {
    let rest = raw.strip_prefix('%')?;
    let f: Vec<&str> = rest.split(':').collect();
    if f.len() < 7 {
        return None;
    }
    // 协议里频率压成五位，开头的 1 和小数点是隐含的：`18000` = 118.000。
    let khz: f64 = format!("1{}", f[1]).parse().ok()?;
    Some(Controller {
        callsign: f[0].to_string(),
        frequency: khz / 1000.0,
        facility: f[2].parse().unwrap_or(0),
        vis_range: f[3].parse().unwrap_or(0),
        rating: f[4].parse().unwrap_or(0),
        latitude: f[5].parse().ok()?,
        longitude: f[6].parse().ok()?,
    })
}

/// 对外的把手。
#[derive(Debug, Clone)]
pub struct PilotHandle {
    inner: SessionHandle<PilotCommand>,
    events: broadcast::Sender<PilotEvent>,
}

impl PilotHandle {
    /// 连接状态。
    pub fn link(&self) -> broadcast::Receiver<FsdEvent> {
        self.inner.events()
    }

    /// 他机、管制席位、文字消息。
    pub fn traffic(&self) -> broadcast::Receiver<PilotEvent> {
        self.events.subscribe()
    }

    pub fn update_position(&self, position: PilotPosition) {
        self.inner.send(PilotCommand::Position(Box::new(position)));
    }

    pub fn send_text(&self, recipient: impl Into<String>, message: impl Into<String>) {
        self.inner.send(PilotCommand::Text {
            recipient: recipient.into(),
            message: message.into(),
        });
    }

    pub fn file_flight_plan(&self, plan: FlightPlan) {
        self.inner.send(PilotCommand::FilePlan(Box::new(plan)));
    }

    pub fn ident(&self) {
        self.inner.send(PilotCommand::Ident);
    }

    pub fn request_atis(&self, callsign: impl Into<String>) {
        self.inner.send(PilotCommand::RequestAtis(callsign.into()));
    }

    pub async fn request_metar(&self, icao: &str, timeout: Duration) -> Option<String> {
        self.inner.request_metar(icao, timeout).await
    }

    pub fn stop(&self) {
        self.inner.stop();
    }
}

/// 起一条飞行员连接。
pub fn connect(config: PilotConfig) -> PilotHandle {
    let (events, _) = broadcast::channel(256);
    let role = PilotRole {
        identity: config.identity,
        position: None,
        ident_until: None,
        events: events.clone(),
        seen: HashMap::new(),
    };
    PilotHandle {
        inner: session::spawn(config.host, config.port, role, config.reconnect_limit),
        events,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_traffic_packet_becomes_another_aircraft() {
        let pbh = pilot::pack_pbh(5.0, -10.0, 271.0, false);
        let raw = format!("@N:CES123:2000:1:31.14233:121.79084:35000:450:{pbh}:-120");
        let t = parse_traffic(&raw).expect("traffic");
        assert_eq!(t.callsign, "CES123");
        assert_eq!(t.squawk, 2000);
        assert_eq!(t.altitude, 35_000);
        assert_eq!(t.groundspeed, 450);
        assert!((t.latitude - 31.142_33).abs() < 1e-9);
        // 姿态是从 PBH 还原的，不是另算的。
        assert!(
            t.attitude.pitch > 0.0 && t.attitude.bank < 0.0,
            "{:?}",
            t.attitude
        );
    }

    /// 字段少了就当认不出，**不要拿默认值凑一架飞机出来**——那会在雷达上
    /// 多一个谁也不认识的目标。
    #[test]
    fn a_short_traffic_packet_is_not_an_aircraft() {
        assert!(parse_traffic("@N:CES123:2000:1").is_none());
        assert!(parse_traffic("%ZSPD_TWR:27850:5:50:1:31.1:121.7:0").is_none());
    }

    /// 管制席位的频率在协议里压成五位，开头的 1 和小数点是隐含的。
    #[test]
    fn a_controller_frequency_gets_its_leading_one_back() {
        let c =
            parse_controller("%ZSPD_TWR:27850:5:50:1:31.14233:121.79084:0").expect("controller");
        assert_eq!(c.callsign, "ZSPD_TWR");
        assert!((c.frequency - 127.850).abs() < 1e-9, "{}", c.frequency);
        assert_eq!(c.facility, 5);
        assert_eq!(c.vis_range, 50);
        assert_eq!(c.rating, 1);
    }

    #[test]
    fn a_short_controller_packet_is_not_a_station() {
        assert!(parse_controller("%ZSPD_TWR:27850:5").is_none());
    }

    fn role() -> PilotRole {
        let (events, _) = broadcast::channel(16);
        PilotRole {
            identity: PilotIdentity::new("CES123", "1234", "pw", "R N", 1, 25, "XPC"),
            position: None,
            ident_until: None,
            events,
            seen: HashMap::new(),
        }
    }

    fn a_position() -> PilotPosition {
        PilotPosition {
            mode: XpdrMode::ModeC,
            squawk: 2000,
            rating: 1,
            latitude: 31.0,
            longitude: 121.0,
            altitude: 0,
            groundspeed: 0,
            pitch: 0.0,
            bank: 0.0,
            heading: 0.0,
            on_ground: true,
            pressure_delta: 0,
        }
    }

    /// **模拟器还没连上就不发包。** 一个全零的位置包会把飞机放在几内亚湾
    /// 外海，而那在雷达上看着像一架真飞机。
    #[test]
    fn nothing_is_sent_before_the_simulator_says_where_we_are() {
        let mut r = role();
        assert!(r.tick().expect("tick").is_empty());
    }

    /// 停在地面上降频，一动就回到 5 Hz。
    #[test]
    fn a_parked_aircraft_reports_less_often() {
        let mut r = role();
        assert_eq!(r.tick_interval(), POSITION_INTERVAL, "还没有位置时用快的");

        r.on_command(PilotCommand::Position(Box::new(a_position())));
        assert_eq!(r.tick_interval(), SLOW_POSITION_INTERVAL);

        let mut rolling = a_position();
        rolling.groundspeed = 12;
        r.on_command(PilotCommand::Position(Box::new(rolling)));
        assert_eq!(r.tick_interval(), POSITION_INTERVAL);

        let mut airborne = a_position();
        airborne.on_ground = false;
        r.on_command(PilotCommand::Position(Box::new(airborne)));
        assert_eq!(r.tick_interval(), POSITION_INTERVAL);
    }

    /// 位置**只存不发**：发是 tick 的事。模拟器推得比 5 Hz 快，照单转发会把
    /// 服务端刷爆。
    #[test]
    fn a_position_update_does_not_itself_send_a_packet() {
        let mut r = role();
        assert!(r
            .on_command(PilotCommand::Position(Box::new(a_position())))
            .is_empty());
        assert_eq!(r.tick().expect("tick").len(), 1);
    }

    /// 按下识别**立刻**发一个亮着的包，不等下一个 tick——按下到雷达上亮起来
    /// 之间那 200 毫秒，管制正盯着屏幕等。
    #[test]
    fn ident_lights_up_at_once_and_goes_out_by_itself() {
        let mut r = role();
        r.on_command(PilotCommand::Position(Box::new(a_position())));
        let sent = r.on_command(PilotCommand::Ident);
        assert_eq!(sent.len(), 1);
        assert!(sent[0].starts_with("@Y:CES123:"), "{}", sent[0]);
        // 还在亮着。
        assert!(r.tick().expect("tick")[0].starts_with("@Y:"));
        // 时间过了就灭。
        r.ident_until = Some(Instant::now() - Duration::from_secs(1));
        assert!(r.tick().expect("tick")[0].starts_with("@N:"));
        assert!(
            r.ident_until.is_none(),
            "过期之后要清掉，不然每次都要再判一遍"
        );
    }

    /// `.wallop` 在发消息这一步翻成发往 `*S` 的，**不跟着收件人框走**。
    #[test]
    fn a_wallop_goes_to_the_supervisors_not_to_the_frequency() {
        let mut r = role();
        let sent = r.on_command(PilotCommand::Text {
            recipient: "121.800".into(),
            message: ".wallop 有人抢频率".into(),
        });
        assert_eq!(sent, vec!["#TMCES123:*S:有人抢频率"]);

        let sent = r.on_command(PilotCommand::Text {
            recipient: "ZSPD_TWR".into(),
            message: "request push".into(),
        });
        assert_eq!(sent, vec!["#TMCES123:ZSPD_TWR:request push"]);
    }

    /// 自己的位置包会被服务端回显；**不能把自己当成他机**，否则雷达上会多出
    /// 一架跟着自己走的飞机。
    #[test]
    fn our_own_position_echo_is_not_another_aircraft() {
        let mut r = role();
        let mut rx = r.events.subscribe();
        r.on_packet(&Incoming::Other, "@N:CES123:2000:1:31.0:121.0:0:0:0:0");
        assert!(rx.try_recv().is_err(), "自己的回显不该进他机表");

        r.on_packet(&Incoming::Other, "@N:CCA101:2000:1:31.0:121.0:0:0:0:0");
        assert!(matches!(rx.try_recv(), Ok(PilotEvent::Traffic(_))));
    }

    #[test]
    fn a_logoff_takes_the_aircraft_off_the_list() {
        let mut r = role();
        let mut rx = r.events.subscribe();
        r.on_packet(&Incoming::Other, "#DPCCA101:1234");
        assert_eq!(rx.try_recv(), Ok(PilotEvent::TrafficGone("CCA101".into())));
        r.on_packet(&Incoming::Other, "#DAZSPD_TWR:1234");
        assert_eq!(
            rx.try_recv(),
            Ok(PilotEvent::ControllerGone("ZSPD_TWR".into()))
        );
    }

    #[test]
    fn a_text_message_reaches_the_listener() {
        let mut r = role();
        let mut rx = r.events.subscribe();
        r.on_packet(&Incoming::Other, "#TMZSPD_TWR:CES123:cleared to push");
        assert_eq!(
            rx.try_recv(),
            Ok(PilotEvent::Text {
                sender: "ZSPD_TWR".into(),
                recipient: "CES123".into(),
                message: "cleared to push".into()
            })
        );
    }

    /// 正文里的冒号不能把消息切碎——`splitn(3)` 保证第三段是整条正文。
    #[test]
    fn a_colon_in_a_message_stays_in_the_message() {
        let mut r = role();
        let mut rx = r.events.subscribe();
        r.on_packet(
            &Incoming::Other,
            "#TMZSPD_TWR:CES123:climb FL350 time 12:30",
        );
        match rx.try_recv() {
            Ok(PilotEvent::Text { message, .. }) => assert_eq!(message, "climb FL350 time 12:30"),
            other => panic!("got {other:?}"),
        }
    }
}
