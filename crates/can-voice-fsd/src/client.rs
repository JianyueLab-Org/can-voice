//! 通播席位那一侧的角色，跑在 [`crate::session`] 的骨架上。
//!
//! 连接、登录、重连、超时那一套在骨架里，两个角色共用；这里只有"通播席位是
//! 什么样"：`#AA` 登录、每 15 秒一个位置包、答飞行员和服务端的通播查询。

use crate::packet::{self, CallsignProblem, Identity, Incoming, Position};
use crate::session::{self, Role, SessionHandle};
use std::time::Duration;

pub use crate::session::{
    FsdEvent, FsdState, Reason, CONNECT_TIMEOUT, LOGIN_TIMEOUT, RECONNECT_DELAY, RECONNECT_LIMIT,
};

/// 服务端 150 秒收不到位置包就断线。
pub const POSITION_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub identity: Identity,
    pub position: Position,
    pub atis_lines: Vec<String>,
    pub reconnect_limit: u32,
}

/// 通播席位这个角色。
struct AtisRole {
    identity: Identity,
    position: Position,
    atis_lines: Vec<String>,
}

enum AtisCommand {
    SetAtisLines(Vec<String>),
    SetFrequency(String),
}

impl Role for AtisRole {
    type Command = AtisCommand;

    fn callsign(&self) -> &str {
        &self.identity.callsign
    }

    fn validated_callsign(&self) -> Result<String, CallsignProblem> {
        packet::check_atis_callsign(&self.identity.callsign)
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
            self.identity.caps_query(),
        ]
    }

    fn logoff_packet(&self) -> String {
        self.identity.logoff_packet()
    }

    fn tick(&mut self) -> Result<Vec<String>, Reason> {
        match self.position.packet(&self.identity.callsign) {
            Some(p) => Ok(vec![p]),
            None => Err(Reason::BadFrequency(self.position.frequency.clone())),
        }
    }

    fn tick_interval(&self) -> Duration {
        POSITION_INTERVAL
    }

    fn on_packet(&mut self, incoming: &Incoming, _raw: &str) -> Vec<String> {
        match incoming {
            Incoming::AtisQuery { sender } => {
                packet::atis_reply(&self.identity.callsign, sender, &self.atis_lines)
            }
            Incoming::CapsQuery { sender } => {
                vec![format!(
                    "$CR{}:{sender}:CAPS:ATCINFO=1",
                    self.identity.callsign
                )]
            }
            _ => Vec::new(),
        }
    }

    fn on_command(&mut self, command: AtisCommand) -> Vec<String> {
        match command {
            AtisCommand::SetAtisLines(lines) => {
                self.atis_lines = lines;
                Vec::new()
            }
            AtisCommand::SetFrequency(f) => {
                self.position.frequency = f;
                // 频率变了立刻补一个位置包，别等下一个 15 秒——在线列表里
                // 那 15 秒显示的是旧频率。
                //
                // 认不出的频率这里安静地不发；下一个 tick 会报 BadFrequency
                // 并把这条连接收掉。**不在这里立刻掐**：打字打到一半的
                // "12" 不该把已经在播的席位踢下线。
                self.position
                    .packet(&self.identity.callsign)
                    .into_iter()
                    .collect()
            }
        }
    }
}

/// 对外的把手。
#[derive(Debug, Clone)]
pub struct FsdHandle {
    inner: SessionHandle<AtisCommand>,
}

impl FsdHandle {
    pub fn events(&self) -> tokio::sync::broadcast::Receiver<FsdEvent> {
        self.inner.events()
    }

    /// 换一份通播文字。**超过 64 行的部分丢掉**——服务端只收这么多。
    pub fn set_atis_lines(&self, mut lines: Vec<String>) {
        lines.truncate(packet::MAX_ATIS_LINES);
        self.inner.send(AtisCommand::SetAtisLines(lines));
    }

    pub fn set_frequency(&self, frequency: impl Into<String>) {
        self.inner.send(AtisCommand::SetFrequency(frequency.into()));
    }

    /// 向服务端要一份 METAR。拿不到返回 `None`。
    pub async fn request_metar(&self, icao: &str, timeout: Duration) -> Option<String> {
        self.inner.request_metar(icao, timeout).await
    }

    pub fn stop(&self) {
        self.inner.stop();
    }
}

/// 起一条通播席位的连接。
pub fn connect(config: Config) -> FsdHandle {
    let role = AtisRole {
        identity: config.identity,
        position: config.position,
        atis_lines: config.atis_lines,
    };
    FsdHandle {
        inner: session::spawn(config.host, config.port, role, config.reconnect_limit),
    }
}
