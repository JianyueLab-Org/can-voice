//! 把一条 FSD 事件记进它该进的那张派生表。
//!
//! 两个飞行员客户端逐字相同的一段 `match`，所以它在这里而不是各抄一遍——
//! 抄两遍的后果这个 crate 的文档开头就写着。`PilotEvent::Text` 和
//! 两条席位事件此前都落在 `Ok(_) => {}` 上，两边一起漏。

use crate::chat::{ChatLog, ChatMessage};
use crate::controllers::ControllerTable;
use crate::traffic::{Sample, TrafficTable};
use can_voice_fsd::pilot_client::PilotEvent;

/// 记一条事件。`now` 是单调秒，由调用方给。
pub fn absorb(
    now: f64,
    event: PilotEvent,
    traffic: &mut TrafficTable,
    chat: &mut ChatLog,
    controllers: &mut ControllerTable,
) {
    match event {
        PilotEvent::Traffic(t) => {
            traffic.update_position(
                &t.callsign,
                t.squawk,
                Sample {
                    time: now,
                    latitude: t.latitude,
                    longitude: t.longitude,
                    altitude: f64::from(t.altitude),
                    pitch: t.attitude.pitch,
                    bank: t.attitude.bank,
                    heading: t.attitude.heading,
                    on_ground: t.attitude.on_ground,
                    groundspeed: f64::from(t.groundspeed),
                },
            );
        }
        PilotEvent::TrafficGone(callsign) => {
            traffic.remove(&callsign);
        }
        PilotEvent::PlaneInfo {
            callsign,
            equipment,
            airline,
            ..
        } => traffic.set_plane_info(&callsign, now, &equipment, &airline),
        PilotEvent::Config { callsign, config } => traffic.set_config(&callsign, now, *config),
        PilotEvent::Controller(c) => controllers.update(now, *c),
        PilotEvent::ControllerGone(callsign) => controllers.remove(&callsign),
        PilotEvent::Text {
            sender,
            recipient,
            message,
        } => chat.record(ChatMessage {
            from: sender,
            to: recipient,
            text: message,
            outbound: false,
            at: now,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_voice_fsd::pilot::Attitude;
    use can_voice_fsd::pilot_client::{Controller, Traffic};

    fn traffic_of(callsign: &str) -> Traffic {
        Traffic {
            callsign: callsign.into(),
            squawk: 2000,
            latitude: 31.14,
            longitude: 121.8,
            altitude: 3000,
            groundspeed: 250,
            attitude: Attitude {
                pitch: 0.0,
                bank: 0.0,
                heading: 90.0,
                on_ground: false,
            },
        }
    }

    /// 这一条就是这个 bug 本身：`PilotEvent::Text` 曾经落在 `Ok(_) => {}` 上，
    /// 于是飞行员只能发不能收，管制员打字他看不见。
    #[test]
    fn a_text_message_from_a_controller_is_recorded() {
        let (mut traffic, mut chat, mut controllers) = tables();
        absorb(
            1.0,
            PilotEvent::Text {
                sender: "ZSPD_TWR".into(),
                recipient: "CCA1501".into(),
                message: "pushback approved".into(),
            },
            &mut traffic,
            &mut chat,
            &mut controllers,
        );
        let got = chat.snapshot();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].from, "ZSPD_TWR");
        assert_eq!(got[0].text, "pushback approved");
        assert!(!got[0].outbound, "a received message is not outbound");
        assert_eq!(got[0].at, 1.0);
    }

    /// 席位上线进列表，下线出列表。同样是被 `Ok(_) => {}` 丢掉的那两条。
    #[test]
    fn a_controller_goes_into_the_list_and_comes_out_on_logoff() {
        let (mut traffic, mut chat, mut controllers) = tables();
        let c = Controller {
            callsign: "ZSPD_TWR".into(),
            frequency: 118.35,
            facility: 4,
            rating: 5,
            latitude: 31.14,
            longitude: 121.8,
            vis_range: 150,
        };
        absorb(
            0.0,
            PilotEvent::Controller(Box::new(c)),
            &mut traffic,
            &mut chat,
            &mut controllers,
        );
        assert_eq!(controllers.snapshot(0.0, None).len(), 1);

        absorb(
            1.0,
            PilotEvent::ControllerGone("ZSPD_TWR".into()),
            &mut traffic,
            &mut chat,
            &mut controllers,
        );
        assert!(controllers.snapshot(1.0, None).is_empty());
    }

    /// 他机那几条照旧。这一条钉着"搬家的时候没有把原来能用的弄丢"。
    #[test]
    fn a_position_report_still_reaches_the_traffic_table() {
        let (mut traffic, mut chat, mut controllers) = tables();
        absorb(
            0.0,
            PilotEvent::Traffic(Box::new(traffic_of("CES2345"))),
            &mut traffic,
            &mut chat,
            &mut controllers,
        );
        assert_eq!(traffic.snapshot(0.0, None, None, None).len(), 1);

        absorb(
            0.5,
            PilotEvent::TrafficGone("CES2345".into()),
            &mut traffic,
            &mut chat,
            &mut controllers,
        );
        assert!(traffic.snapshot(0.5, None, None, None).is_empty());
    }

    fn tables() -> (TrafficTable, ChatLog, ControllerTable) {
        (
            TrafficTable::new(),
            ChatLog::default(),
            ControllerTable::default(),
        )
    }
}
