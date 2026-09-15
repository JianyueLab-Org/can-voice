//! 他机表：把 FSD 收到的位置包攒起来，插值成渲染端每一帧要画的样子。
//!
//! 两个模拟器共用。位置包大约每秒 5 个，而模拟器要每帧一个位置——中间这段
//! 是插值和外推的事，和"是 X-Plane 还是 MSFS"无关。

use std::collections::HashMap;

/// 多久没消息就当它走了。
pub const STALE_AFTER: f64 = 15.0;
/// 最多往后外推这么多秒。**对方掉线时飞机应当停在原地，不是一直飞下去。**
pub const MAX_EXTRAPOLATE: f64 = 2.0;
pub const NM_PER_DEGREE: f64 = 60.0;

/// 一个时刻的位置和姿态。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct Sample {
    /// 单调秒。**由调用方给**，这一层不读时钟——不然就没法测。
    pub time: f64,
    pub latitude: f64,
    pub longitude: f64,
    /// 英尺。
    pub altitude: f64,
    pub pitch: f64,
    pub bank: f64,
    pub heading: f64,
    pub on_ground: bool,
    /// 节。
    pub groundspeed: f64,
}

/// 按**最短弧**插值。359° 到 1° 应当往前走 2°，而不是倒着走 358°。
pub fn interpolate_angle(a: f64, b: f64, ratio: f64) -> f64 {
    let difference = (b - a + 180.0).rem_euclid(360.0) - 180.0;
    (a + difference * ratio).rem_euclid(360.0)
}

/// 两点距离（海里）。平面近似，几百海里内够用，比 haversine 便宜。
pub fn distance_nm(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let mean_lat = ((lat1 + lat2) / 2.0).to_radians();
    let dy = (lat2 - lat1) * NM_PER_DEGREE;
    // 经度差按最短弧算，**跨 180° 经线时才不会得出"绕地球一圈"的距离**。
    let dlon = (lon2 - lon1 + 180.0).rem_euclid(360.0) - 180.0;
    let dx = dlon * NM_PER_DEGREE * mean_lat.cos();
    dx.hypot(dy)
}

/// 网上的一架飞机。
#[derive(Debug, Clone, Default)]
pub struct Aircraft {
    pub callsign: String,
    pub squawk: u16,
    pub previous: Option<Sample>,
    pub latest: Option<Sample>,
    /// 建表时刻。位置还没到的那些靠它拿同样的宽限。
    pub created: f64,
    pub equipment: String,
    pub airline: String,
    pub livery: String,
    pub csl: String,
    /// 渲染端还没为这架匹配过模型。
    pub model_dirty: bool,
}

impl Aircraft {
    pub fn new(callsign: &str, now: f64) -> Self {
        Self {
            callsign: callsign.to_string(),
            created: now,
            model_dirty: true,
            ..Default::default()
        }
    }

    pub fn update(&mut self, sample: Sample) {
        // 乱序的包丢掉：往回走一步会让飞机在屏幕上抖一下。
        if self.latest.is_some_and(|l| sample.time < l.time) {
            return;
        }
        self.previous = self.latest;
        self.latest = Some(sample);
    }

    /// 英尺每分钟。只有两个采样才算得出来。
    pub fn vertical_speed(&self) -> f64 {
        let (Some(previous), Some(latest)) = (self.previous, self.latest) else {
            return 0.0;
        };
        let span = latest.time - previous.time;
        if span <= 0.0 {
            return 0.0;
        }
        (latest.altitude - previous.altitude) / span * 60.0
    }

    /// 插值出 `now` 时刻的位置。没有数据返回 `None`。
    ///
    /// 两个采样之间线性插值；超过最后一个采样按最后的速度外推，但最多
    /// [`MAX_EXTRAPOLATE`] 秒。
    pub fn position_at(&self, now: f64) -> Option<Sample> {
        let latest = self.latest?;
        let Some(previous) = self.previous else {
            return Some(latest);
        };
        let span = latest.time - previous.time;
        if span <= 0.0 {
            return Some(latest);
        }

        let mut ratio = (now - previous.time) / span;
        // 往前不外推（收到乱序包时会出现），往后最多外推固定秒数。
        let limit = 1.0 + MAX_EXTRAPOLATE.min(span * 2.0) / span;
        ratio = ratio.clamp(0.0, limit);

        // **经度也要按最短弧走**：179.98°E 到 -179.98° 是往前 0.04°，
        // 线性差值会让飞机横穿整个地球再回来。
        let lon_step = (latest.longitude - previous.longitude + 180.0).rem_euclid(360.0) - 180.0;
        let mut longitude = previous.longitude + lon_step * ratio;
        if longitude > 180.0 {
            longitude -= 360.0;
        } else if longitude < -180.0 {
            longitude += 360.0;
        }

        Some(Sample {
            time: now,
            latitude: previous.latitude + (latest.latitude - previous.latitude) * ratio,
            longitude,
            altitude: previous.altitude + (latest.altitude - previous.altitude) * ratio,
            pitch: previous.pitch + (latest.pitch - previous.pitch) * ratio,
            bank: previous.bank + (latest.bank - previous.bank) * ratio,
            heading: interpolate_angle(previous.heading, latest.heading, ratio),
            on_ground: latest.on_ground,
            groundspeed: latest.groundspeed,
        })
    }
}

/// 渲染端每一帧拿到的一条。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Entry {
    pub callsign: String,
    pub squawk: u16,
    #[serde(flatten)]
    pub position: Sample,
    pub vertical_speed: f64,
    pub equipment: String,
    pub airline: String,
    pub livery: String,
    pub csl: String,
    pub model_dirty: bool,
    pub range_nm: Option<f64>,
}

#[derive(Debug, Default)]
pub struct TrafficTable {
    aircraft: HashMap<String, Aircraft>,
}

impl TrafficTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.aircraft.len()
    }

    pub fn is_empty(&self) -> bool {
        self.aircraft.is_empty()
    }

    pub fn get(&self, callsign: &str) -> Option<&Aircraft> {
        self.aircraft.get(callsign)
    }

    pub fn update_position(&mut self, callsign: &str, squawk: u16, sample: Sample) {
        let entry = self
            .aircraft
            .entry(callsign.to_string())
            .or_insert_with(|| Aircraft::new(callsign, sample.time));
        entry.squawk = squawk;
        entry.update(sample);
    }

    /// 机型先到、位置未到的也要留住——立刻清掉的话，问来的机型白收了，
    /// 等位置到达时又得重新问一轮。
    pub fn set_plane_info(&mut self, callsign: &str, now: f64, equipment: &str, airline: &str) {
        let entry = self
            .aircraft
            .entry(callsign.to_string())
            .or_insert_with(|| Aircraft::new(callsign, now));
        if entry.equipment != equipment || entry.airline != airline {
            entry.equipment = equipment.to_string();
            entry.airline = airline.to_string();
            entry.model_dirty = true;
        }
    }

    pub fn remove(&mut self, callsign: &str) -> bool {
        self.aircraft.remove(callsign).is_some()
    }

    /// 清掉太久没消息的，返回被清掉的呼号。
    pub fn prune(&mut self, now: f64) -> Vec<String> {
        let gone: Vec<String> = self
            .aircraft
            .iter()
            .filter(|(_, a)| {
                let last = a.latest.map(|s| s.time).unwrap_or(a.created);
                now - last > STALE_AFTER
            })
            .map(|(k, _)| k.clone())
            .collect();
        for callsign in &gone {
            self.aircraft.remove(callsign);
            tracing::info!(%callsign, "dropped, no updates for too long");
        }
        gone
    }

    /// 给渲染端的一份数据，已经插值到 `now`。
    ///
    /// `origin` 是本机位置。给了就按距离排序并可以截断——**TCAS 只有 64 个
    /// 位置**，飞机比这多的时候必须先扔远的，不能随便扔。
    pub fn snapshot(
        &self,
        now: f64,
        origin: Option<(f64, f64)>,
        limit: Option<usize>,
        max_range_nm: Option<f64>,
    ) -> Vec<Entry> {
        let mut entries: Vec<Entry> = self
            .aircraft
            .values()
            .filter_map(|a| {
                let position = a.position_at(now)?;
                Some(Entry {
                    callsign: a.callsign.clone(),
                    squawk: a.squawk,
                    vertical_speed: a.vertical_speed(),
                    equipment: a.equipment.clone(),
                    airline: a.airline.clone(),
                    livery: a.livery.clone(),
                    csl: a.csl.clone(),
                    model_dirty: a.model_dirty,
                    range_nm: origin.map(|(lat, lon)| {
                        distance_nm(lat, lon, position.latitude, position.longitude)
                    }),
                    position,
                })
            })
            .collect();

        if origin.is_some() {
            entries.sort_by(|a, b| {
                a.range_nm
                    .unwrap_or(f64::MAX)
                    .total_cmp(&b.range_nm.unwrap_or(f64::MAX))
            });
            if let Some(max) = max_range_nm {
                entries.retain(|e| e.range_nm.is_some_and(|r| r <= max));
            }
        } else {
            // 没有本机位置时至少给一个稳定的顺序，否则每帧的排列都在变。
            entries.sort_by(|a, b| a.callsign.cmp(&b.callsign));
        }
        if let Some(limit) = limit {
            entries.truncate(limit);
        }
        entries
    }

    /// 渲染端匹配过模型之后回来清标记。
    ///
    /// **带上匹配时用的机型/航司。** 如果机型的回复恰好在快照和这里之间落地，
    /// 无条件清标记会把那次更新吞掉——飞机从此停在通用模型上，再也不重新匹配。
    /// 对不上就把标记留着，下一帧再匹配一次。
    pub fn mark_model_clean(&mut self, callsign: &str, equipment: &str, airline: &str) {
        let Some(a) = self.aircraft.get_mut(callsign) else {
            return;
        };
        if a.equipment != equipment || a.airline != airline {
            return;
        }
        a.model_dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(time: f64, lat: f64, lon: f64, heading: f64) -> Sample {
        Sample {
            time,
            latitude: lat,
            longitude: lon,
            altitude: 1000.0,
            pitch: 0.0,
            bank: 0.0,
            heading,
            on_ground: false,
            groundspeed: 250.0,
        }
    }

    /// **359° 到 1° 往前走 2°**，不是倒着走 358°。
    #[test]
    fn an_angle_takes_the_short_way_round() {
        assert!((interpolate_angle(359.0, 1.0, 0.5) - 0.0).abs() < 1e-9);
        assert!((interpolate_angle(10.0, 350.0, 0.5) - 0.0).abs() < 1e-9);
        assert!((interpolate_angle(90.0, 180.0, 0.5) - 135.0).abs() < 1e-9);
    }

    /// **经度也要按最短弧走。** 179.98°E 到 -179.98° 是往前 0.04°；线性差值
    /// 会让飞机横穿整个地球再回来。
    #[test]
    fn crossing_the_date_line_does_not_send_the_aircraft_round_the_world() {
        let mut a = Aircraft::new("CES123", 0.0);
        a.update(sample(0.0, 31.0, 179.98, 90.0));
        a.update(sample(1.0, 31.0, -179.98, 90.0));
        let mid = a.position_at(0.5).expect("position");
        // 中点应当落在 180 附近，而不是 0 附近。
        assert!(mid.longitude.abs() > 179.9, "got {}", mid.longitude);
    }

    /// 跨 180° 经线的距离也是一样的道理。
    #[test]
    fn the_distance_across_the_date_line_is_short() {
        let d = distance_nm(31.0, 179.98, 31.0, -179.98);
        assert!(d < 5.0, "got {d} nm");
    }

    #[test]
    fn two_samples_interpolate_linearly() {
        let mut a = Aircraft::new("CES123", 0.0);
        a.update(sample(0.0, 30.0, 120.0, 0.0));
        a.update(sample(2.0, 32.0, 122.0, 0.0));
        let p = a.position_at(1.0).expect("position");
        assert!((p.latitude - 31.0).abs() < 1e-9);
        assert!((p.longitude - 121.0).abs() < 1e-9);
    }

    /// **对方掉线时飞机应当停在原地，不是一直飞下去。**
    #[test]
    fn extrapolation_stops_after_a_couple_of_seconds() {
        let mut a = Aircraft::new("CES123", 0.0);
        a.update(sample(0.0, 30.0, 120.0, 0.0));
        a.update(sample(1.0, 31.0, 120.0, 0.0));
        // 一秒的间隔，最多再外推两秒 → ratio 封顶在 3。
        let far = a.position_at(1000.0).expect("position");
        assert!((far.latitude - 33.0).abs() < 1e-9, "got {}", far.latitude);
    }

    /// 乱序的包丢掉——往回走一步会让飞机在屏幕上抖一下。
    #[test]
    fn a_late_packet_does_not_drag_the_aircraft_backwards() {
        let mut a = Aircraft::new("CES123", 0.0);
        a.update(sample(0.0, 30.0, 120.0, 0.0));
        a.update(sample(2.0, 32.0, 120.0, 0.0));
        a.update(sample(1.0, 31.0, 120.0, 0.0));
        assert_eq!(a.latest.expect("latest").time, 2.0);
    }

    #[test]
    fn a_single_sample_is_reported_as_is() {
        let mut a = Aircraft::new("CES123", 0.0);
        a.update(sample(0.0, 30.0, 120.0, 45.0));
        let p = a.position_at(99.0).expect("position");
        assert_eq!(p.latitude, 30.0);
        assert_eq!(p.heading, 45.0);
    }

    #[test]
    fn the_vertical_speed_needs_two_samples() {
        let mut a = Aircraft::new("CES123", 0.0);
        assert_eq!(a.vertical_speed(), 0.0);
        a.update(sample(0.0, 30.0, 120.0, 0.0));
        assert_eq!(a.vertical_speed(), 0.0);
        let mut climbing = sample(1.0, 30.0, 120.0, 0.0);
        climbing.altitude = 1100.0;
        a.update(climbing);
        assert!((a.vertical_speed() - 6000.0).abs() < 1e-9);
    }

    /// **TCAS 只有 64 个位置**，多了必须先扔远的。
    #[test]
    fn the_nearest_aircraft_survive_the_cut() {
        let mut t = TrafficTable::new();
        for i in 0..10 {
            let s = sample(0.0, 30.0 + f64::from(i), 120.0, 0.0);
            t.update_position(&format!("CES{i:03}"), 2000, s);
        }
        let near = t.snapshot(0.0, Some((30.0, 120.0)), Some(3), None);
        assert_eq!(near.len(), 3);
        assert_eq!(near[0].callsign, "CES000");
        assert_eq!(near[2].callsign, "CES002");
        assert!(near[0].range_nm.expect("range") < near[2].range_nm.expect("range"));
    }

    #[test]
    fn a_range_limit_drops_the_far_ones() {
        let mut t = TrafficTable::new();
        t.update_position("NEAR", 2000, sample(0.0, 30.01, 120.0, 0.0));
        t.update_position("FAR", 2000, sample(0.0, 40.0, 120.0, 0.0));
        let near = t.snapshot(0.0, Some((30.0, 120.0)), None, Some(50.0));
        assert_eq!(near.len(), 1);
        assert_eq!(near[0].callsign, "NEAR");
    }

    /// 机型先到、位置未到的要留住——立刻清掉的话问来的机型白收了。
    #[test]
    fn an_aircraft_known_only_by_type_gets_the_same_grace_period() {
        let mut t = TrafficTable::new();
        t.set_plane_info("CES123", 0.0, "A320", "CES");
        assert_eq!(t.prune(STALE_AFTER - 1.0).len(), 0);
        assert_eq!(t.prune(STALE_AFTER + 1.0), vec!["CES123".to_string()]);
    }

    #[test]
    fn a_silent_aircraft_is_dropped() {
        let mut t = TrafficTable::new();
        t.update_position("CES123", 2000, sample(0.0, 30.0, 120.0, 0.0));
        assert!(t.prune(STALE_AFTER - 1.0).is_empty());
        assert_eq!(t.prune(STALE_AFTER + 1.0), vec!["CES123".to_string()]);
        assert!(t.is_empty());
    }

    /// **清标记要带上匹配时用的机型。** 机型的回复恰好在快照和清标记之间落地
    /// 时，无条件清会把那次更新吞掉，飞机从此停在通用模型上再也不重新匹配。
    #[test]
    fn a_stale_clean_does_not_swallow_a_type_that_arrived_meanwhile() {
        let mut t = TrafficTable::new();
        t.update_position("CES123", 2000, sample(0.0, 30.0, 120.0, 0.0));
        t.set_plane_info("CES123", 0.0, "A320", "CES");
        // 渲染端拿着 A320 去匹配的同时，B738 的回复到了。
        t.set_plane_info("CES123", 0.0, "B738", "CES");
        t.mark_model_clean("CES123", "A320", "CES");
        assert!(
            t.get("CES123").expect("aircraft").model_dirty,
            "标记不该被吞掉"
        );
        // 拿对的那一份来清就清得掉。
        t.mark_model_clean("CES123", "B738", "CES");
        assert!(!t.get("CES123").expect("aircraft").model_dirty);
    }

    /// 没有本机位置时也要有稳定顺序，否则每帧的排列都在变。
    #[test]
    fn the_order_is_stable_without_an_origin() {
        let mut t = TrafficTable::new();
        for cs in ["CCA101", "CES123", "CSN999"] {
            t.update_position(cs, 2000, sample(0.0, 30.0, 120.0, 0.0));
        }
        let names: Vec<String> = t
            .snapshot(0.0, None, None, None)
            .into_iter()
            .map(|e| e.callsign)
            .collect();
        assert_eq!(names, vec!["CCA101", "CES123", "CSN999"]);
    }
}
