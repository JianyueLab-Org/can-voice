//! 机场坐标表。
//!
//! FSD 的位置包（`%`）要经纬度，否则席位会落在 0/0 ——**几内亚湾外海**。
//! vATIS 是从它自己的 NavData 仓库拿坐标的；我们直接用本网站已经在用的那份：
//! can-web 的 `public/airports.json`，格式是 `{"RJAA": [纬度, 经度]}`，
//! 一万七千多个机场，源头是 VATSpy 数据。
//!
//! **这一份不是 AIRAC 数据**，所以不受 `CLAUDE.md` 里"导航数据不外流"那条约束
//! ——它是 VATSpy 的公开数据，can-web 本来就当静态资源发给浏览器。
//!
//! 表随程序一起编进二进制，查表不联网。用户在席位里手填的坐标优先——
//! 机场基准点未必是塔台位置，想精确定位时可以覆盖。

use std::collections::HashMap;
use std::sync::LazyLock;

const RAW: &str = include_str!("../data/airports.json");

static TABLE: LazyLock<HashMap<String, (f64, f64)>> = LazyLock::new(|| {
    match serde_json::from_str::<HashMap<String, Vec<f64>>>(RAW) {
        Ok(raw) => raw
            .into_iter()
            .filter_map(|(k, v)| {
                // 少于两项的条目跳过，而不是补 0 —— 补出来的是几内亚湾。
                match (v.first(), v.get(1)) {
                    (Some(lat), Some(lon)) => Some((k, (*lat, *lon))),
                    _ => None,
                }
            })
            .collect(),
        Err(e) => {
            tracing::warn!(error = %e, "the built-in airport table is not valid json");
            HashMap::new()
        }
    }
});

/// 查机场坐标，返回 `(纬度, 经度)`；查不到返回 `None`。
pub fn coordinates(icao: &str) -> Option<(f64, f64)> {
    TABLE.get(icao.trim().to_uppercase().as_str()).copied()
}

/// 表里有多少个机场。启动时打一行日志用。
pub fn len() -> usize {
    TABLE.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_loads_and_is_not_tiny() {
        assert!(len() > 17_000, "only {} airports loaded", len());
    }

    #[test]
    fn known_airports_are_found() {
        let (lat, lon) = coordinates("ZSPD").expect("ZSPD");
        assert!((lat - 31.14).abs() < 0.5, "ZSPD latitude {lat}");
        assert!((lon - 121.8).abs() < 0.5, "ZSPD longitude {lon}");
        // 小写和空白都要认。
        assert_eq!(coordinates(" zspd "), coordinates("ZSPD"));
    }

    /// 查不到返回 `None`，**不是 (0, 0)**。
    ///
    /// 0/0 是几内亚湾外海，而一个落在那里的席位在雷达上看着像一个真实的席位
    /// ——没有任何东西会报错。
    #[test]
    fn an_unknown_airport_is_none_not_the_gulf_of_guinea() {
        assert_eq!(coordinates("ZZZZ"), None);
        assert_eq!(coordinates(""), None);
    }
}
