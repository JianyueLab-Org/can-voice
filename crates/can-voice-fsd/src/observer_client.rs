//! Observer FSD session. The observer publishes only their own simulator position.

use crate::packet::{self, CallsignProblem, Identity, Incoming, Position, RATING_OBSERVER};
use crate::session::{self, FsdEvent, Reason, Role, SessionHandle};
use std::time::Duration;
use tokio::sync::broadcast;

const POSITION_INTERVAL: Duration = Duration::from_secs(15);
const OBSERVER_FREQUENCY: &str = "199.998";
const OBSERVER_RANGE_NM: u32 = 100;

#[derive(Debug, Clone)]
pub struct ObserverConfig {
    pub host: String,
    pub port: u16,
    pub identity: Identity,
    pub reconnect_limit: u32,
}

enum ObserverCommand {
    Position { latitude: f64, longitude: f64 },
}

#[derive(Clone)]
pub struct ObserverHandle {
    inner: SessionHandle<ObserverCommand>,
}

impl ObserverHandle {
    pub fn events(&self) -> broadcast::Receiver<FsdEvent> {
        self.inner.events()
    }

    pub fn update_position(&self, latitude: f64, longitude: f64) {
        if latitude.is_finite()
            && (-90.0..=90.0).contains(&latitude)
            && longitude.is_finite()
            && (-180.0..=180.0).contains(&longitude)
        {
            self.inner.send(ObserverCommand::Position {
                latitude,
                longitude,
            });
        }
    }

    pub async fn request_metar(&self, icao: &str, timeout: Duration) -> Option<String> {
        self.inner.request_metar(icao, timeout).await
    }

    pub fn stop(&self) {
        self.inner.stop();
    }
}

struct ObserverRole {
    identity: Identity,
    position: Option<(f64, f64)>,
}

impl Role for ObserverRole {
    type Command = ObserverCommand;

    fn callsign(&self) -> &str {
        &self.identity.callsign
    }

    fn validated_callsign(&self) -> Result<String, CallsignProblem> {
        packet::check_callsign(&self.identity.callsign)
    }

    fn adopt_callsign(&mut self, callsign: String) {
        self.identity.callsign = callsign;
    }

    fn rating(&self) -> u32 {
        RATING_OBSERVER
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
        let Some((latitude, longitude)) = self.position else {
            return Ok(Vec::new());
        };
        let position = Position {
            frequency: OBSERVER_FREQUENCY.into(),
            facility: 0,
            vis_range: OBSERVER_RANGE_NM,
            rating: RATING_OBSERVER,
            latitude,
            longitude,
        };
        Ok(position
            .packet(&self.identity.callsign)
            .into_iter()
            .collect())
    }

    fn tick_interval(&self) -> Duration {
        POSITION_INTERVAL
    }

    fn on_packet(&mut self, _incoming: &Incoming, _raw: &str) -> Vec<String> {
        Vec::new()
    }

    fn on_command(&mut self, command: Self::Command) -> Vec<String> {
        match command {
            ObserverCommand::Position {
                latitude,
                longitude,
            } => {
                let first = self.position.is_none();
                self.position = Some((latitude, longitude));
                if first {
                    return self.tick().unwrap_or_default();
                }
            }
        }
        Vec::new()
    }
}

pub fn connect(mut config: ObserverConfig) -> ObserverHandle {
    config.identity.rating = RATING_OBSERVER;
    ObserverHandle {
        inner: session::spawn(
            config.host,
            config.port,
            ObserverRole {
                identity: config.identity,
                position: None,
            },
            config.reconnect_limit,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_position_after_login_is_published_immediately() {
        let mut role = ObserverRole {
            identity: Identity::new("ZSPD_OBS", "1234", "pw", "Observer", RATING_OBSERVER),
            position: None,
        };
        assert_eq!(
            role.on_command(ObserverCommand::Position {
                latitude: 31.0,
                longitude: 121.0
            }),
            vec!["%ZSPD_OBS:99998:0:100:1:31.00000:121.00000:0"]
        );
        assert!(role
            .on_command(ObserverCommand::Position {
                latitude: 31.1,
                longitude: 121.1
            })
            .is_empty());
    }
}
