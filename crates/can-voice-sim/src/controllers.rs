//! 在线管制席位表。
//!
//! 和 [`crate::traffic`] 一样是从 FSD 事件里攒出来的派生状态，两个飞行员
//! 客户端共用。放在这里而不是各抄一遍，理由见 crate 文档。

use crate::traffic::distance_nm;
use can_voice_fsd::pilot_client::Controller;
use std::collections::HashMap;

/// 多久没报到就当它走了。
///
/// **比他机的 15 秒松得多**：席位位置包的间隔本来就长（EuroScope 大约 15–25
/// 秒一次），正常路径是下线时发 `#DA`，这个超时只是兜底。定得紧了的表现是
/// 一个正常在线的席位从列表里一闪一闪，而那比多留一会儿糟糕得多。
pub const STALE_AFTER: f64 = 90.0;

/// 列表里的一行。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ControllerEntry {
    pub callsign: String,
    /// MHz。
    pub frequency: f64,
    pub facility: u32,
    pub rating: u32,
    /// 离自己多远，海里。不知道自己在哪时是 `None`——**不编一个数出来**。
    pub range_nm: Option<f64>,
}

#[derive(Debug, Default)]
pub struct ControllerTable {
    /// 呼号 → (最后一次报到的单调秒, 席位)。
    seen: HashMap<String, (f64, Controller)>,
}

impl ControllerTable {
    /// 收到一条席位通告。
    pub fn update(&mut self, now: f64, c: Controller) {
        self.seen.insert(c.callsign.clone(), (now, c));
    }

    /// 席位下线（`#DA`）。
    pub fn remove(&mut self, callsign: &str) {
        self.seen.remove(callsign);
    }

    /// 现在在线的席位，按呼号排序。
    ///
    /// **顺手把过期的剔掉**：分成 `prune` 和 `snapshot` 两步的话，一个忘了调
    /// `prune` 的调用方看到的是一张永远不减的表，而那正是这里要防的那件事。
    pub fn snapshot(&mut self, now: f64, origin: Option<(f64, f64)>) -> Vec<ControllerEntry> {
        self.seen.retain(|_, (at, _)| now - *at <= STALE_AFTER);
        let mut out: Vec<ControllerEntry> = self
            .seen
            .values()
            .map(|(_, c)| ControllerEntry {
                callsign: c.callsign.clone(),
                frequency: c.frequency,
                facility: c.facility,
                rating: c.rating,
                range_nm: origin.map(|(lat, lon)| distance_nm(lat, lon, c.latitude, c.longitude)),
            })
            .collect();
        out.sort_by(|a, b| a.callsign.cmp(&b.callsign));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl(callsign: &str, freq: f64) -> Controller {
        Controller {
            callsign: callsign.into(),
            frequency: freq,
            facility: 4,
            rating: 5,
            latitude: 31.14,
            longitude: 121.8,
            vis_range: 150,
        }
    }

    /// 席位下线（`#DA`）当场消失，不等超时。
    #[test]
    fn a_controller_that_logs_off_is_gone_at_once() {
        let mut t = ControllerTable::default();
        t.update(0.0, ctrl("ZSPD_TWR", 118.35));
        t.remove("ZSPD_TWR");
        assert!(t.snapshot(0.0, None).is_empty());
    }

    /// 只认 `#DA` 的话，一个直接掉线的席位会**永远留在列表里**——
    /// 而飞行员会照着那条已经不存在的频率呼叫。
    #[test]
    fn a_controller_that_stops_reporting_falls_off_after_the_timeout() {
        let mut t = ControllerTable::default();
        t.update(0.0, ctrl("ZSPD_TWR", 118.35));
        assert_eq!(t.snapshot(STALE_AFTER - 1.0, None).len(), 1);
        assert!(t.snapshot(STALE_AFTER + 1.0, None).is_empty());
    }

    /// 再次报到就续上，不会因为"第一次是什么时候"被踢掉。
    #[test]
    fn a_controller_that_keeps_reporting_stays() {
        let mut t = ControllerTable::default();
        t.update(0.0, ctrl("ZSPD_TWR", 118.35));
        t.update(STALE_AFTER - 1.0, ctrl("ZSPD_TWR", 118.35));
        assert_eq!(t.snapshot(STALE_AFTER + 1.0, None).len(), 1);
    }

    /// 顺序是定的。HashMap 的遍历顺序是随机的，照它渲染的话列表每 200 毫秒
    /// 自己跳一次序，用户点不中一行。
    #[test]
    fn the_list_is_ordered_by_callsign() {
        let mut t = ControllerTable::default();
        for cs in ["ZSSS_APP", "ZSPD_TWR", "ZSPD_GND"] {
            t.update(0.0, ctrl(cs, 118.35));
        }
        let got: Vec<String> = t
            .snapshot(0.0, None)
            .into_iter()
            .map(|c| c.callsign)
            .collect();
        assert_eq!(got, ["ZSPD_GND", "ZSPD_TWR", "ZSSS_APP"]);
    }

    /// 知道自己在哪时报距离：飞行员要挑的是"离我最近的那个塔台"。
    #[test]
    fn the_distance_is_filled_in_when_the_position_is_known() {
        let mut t = ControllerTable::default();
        t.update(0.0, ctrl("ZSPD_TWR", 118.35));
        let got = t.snapshot(0.0, Some((32.14, 121.8)));
        assert_eq!(got.len(), 1);
        let range = got[0].range_nm.expect("a known position gives a distance");
        assert!((range - 60.0).abs() < 1.0, "{range}");
        // 不知道自己在哪时不编一个数出来。
        assert!(t.snapshot(0.0, None)[0].range_nm.is_none());
    }
}
