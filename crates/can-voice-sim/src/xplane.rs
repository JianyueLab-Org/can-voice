//! X-Plane 数据链路。UDP，两个协议：BECN 信标发现 + RREF 订阅。
//!
//! ```text
//! 发现   监听多播 239.255.1.1:49707 的 BECN 信标，拿到 X-Plane 的地址和端口
//! 订阅   RREF 请求，freq 指定每秒推送次数，index 是我们自己编的号
//! 接收   RREF 回包里是 (index, f32) 对，按 index 对回 dataref
//! ```
//!
//! 是**订阅**不是一问一答：飞行员客户端每秒要发好几次位置包，一次往返扛不住。

use crate::Snapshot;
use std::collections::HashMap;

pub const MCAST_GROUP: &str = "239.255.1.1";
pub const MCAST_PORT: u16 = 49707;
pub const DEFAULT_PORT: u16 = 49000;
/// 每个 dataref 每秒推送次数。
pub const UPDATE_RATE: i32 = 5;
/// RREF 请求包的固定长度。**X-Plane 只认这个长度**，短了直接丢。
pub const RREF_PACKET_LEN: usize = 413;

/// 要订阅的 dataref。**顺序就是 index**，改顺序等于改协议编号。
pub const DATAREFS: [(&str, &str); 18] = [
    ("latitude", "sim/flightmodel/position/latitude"),
    ("longitude", "sim/flightmodel/position/longitude"),
    // 真高（米），FSD 要英尺。
    ("elevation", "sim/flightmodel/position/elevation"),
    ("agl", "sim/flightmodel/position/y_agl"),
    // 这两个只用来算气压修正量，不参与"飞机实际在哪儿"。
    (
        "indicated_altitude",
        "sim/cockpit2/gauges/indicators/altitude_ft_pilot",
    ),
    (
        "baro_setting",
        "sim/cockpit2/gauges/actuators/barometer_setting_in_hg_pilot",
    ),
    ("groundspeed", "sim/flightmodel/position/groundspeed"),
    ("pitch", "sim/flightmodel/position/theta"),
    ("bank", "sim/flightmodel/position/phi"),
    ("heading_true", "sim/flightmodel/position/psi"),
    ("squawk", "sim/cockpit/radios/transponder_code"),
    ("xpdr_mode", "sim/cockpit/radios/transponder_mode"),
    // 0.001 MHz 精度，支持 8.33 kHz 间隔。X-Plane 11.30 起才有。
    (
        "com1",
        "sim/cockpit2/radios/actuators/com1_frequency_hz_833",
    ),
    (
        "com2",
        "sim/cockpit2/radios/actuators/com2_frequency_hz_833",
    ),
    // 老的，0.01 MHz 精度。两个一起订，谁回就用谁——**不存在的 dataref
    // X-Plane 只是不推送，不会报错**，所以不需要按版本分支。
    ("com1_legacy", "sim/cockpit/radios/com1_freq_hz"),
    ("com2_legacy", "sim/cockpit/radios/com2_freq_hz"),
    ("com1_power", "sim/cockpit2/radios/actuators/com1_power"),
    ("on_ground", "sim/flightmodel/failures/onground_any"),
];

/// 这些网段几乎都是虚拟网卡：VPN、WSL、Hyper-V、Docker。
///
/// 信标会从它们身上也回来一份，但往那边发 RREF **收不到任何数据**。
/// 实测里 `198.18.0.1` 就是这么混进来的，选中它之后一个 dataref 都收不到。
const VIRTUAL_PREFIXES: [&str; 9] = [
    "198.18.", "198.19.", "172.17.", "172.18.", "172.19.", "172.20.", "169.254.", "10.211.",
    "10.37.",
];

/// 给发现到的地址排个优先级，**小的优先**。
///
/// 本机最优（同机跑 X-Plane 是最常见的情形，而且必然通）；其次是普通局域网
/// 地址；已知的虚拟网卡段排最后。
pub fn address_rank(ip: &str) -> u8 {
    if ip == "127.0.0.1" || ip == "localhost" {
        return 0;
    }
    if VIRTUAL_PREFIXES.iter().any(|p| ip.starts_with(p)) {
        return 2;
    }
    1
}

/// 从一份 BECN 信标里取出 X-Plane 在听的端口。
///
/// 布局是 `BECN\0` + 两个字节的**信标协议版本**（不是 X-Plane 的版本号——
/// 早先按 "X-Plane v1.2" 打进日志是错的，会让人以为装了个远古版本）+
/// 两个 i32 + 一个 u32 + 端口 u16，全部小端。
pub fn parse_beacon(data: &[u8]) -> Option<u16> {
    if data.len() < 21 || &data[..5] != b"BECN\0" {
        return None;
    }
    // 5 魔数 + 1 major + 1 minor + 4 + 4 + 4 = 19，端口在 19..21。
    Some(u16::from_le_bytes([data[19], data[20]]))
}

/// 一条 RREF 订阅请求。`rate = 0` 表示退订。
pub fn subscribe_packet(rate: i32, index: i32, dataref: &str) -> Vec<u8> {
    let mut p = Vec::with_capacity(RREF_PACKET_LEN);
    p.extend_from_slice(b"RREF\0");
    p.extend_from_slice(&rate.to_le_bytes());
    p.extend_from_slice(&index.to_le_bytes());
    p.extend_from_slice(dataref.as_bytes());
    p.push(0);
    p.resize(RREF_PACKET_LEN, 0);
    p
}

/// 所有 dataref 的订阅请求。
pub fn subscribe_all(rate: i32) -> Vec<Vec<u8>> {
    DATAREFS
        .iter()
        .enumerate()
        .map(|(i, (_, dataref))| subscribe_packet(rate, i as i32, dataref))
        .collect()
}

/// 解析一个 RREF 回包，返回 `(名字, 值)` 对。
///
/// 认不出的 index 跳过而不是整包作废：X-Plane 会把上一次订阅残留的 index
/// 一起推过来。
pub fn parse_values(data: &[u8]) -> Vec<(&'static str, f32)> {
    if data.len() < 13 || &data[..4] != b"RREF" {
        return Vec::new();
    }
    let body = data.len() - 5;
    if body % 8 != 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(body / 8);
    for i in 0..body / 8 {
        let at = 5 + i * 8;
        let index = i32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        let value = f32::from_le_bytes([data[at + 4], data[at + 5], data[at + 6], data[at + 7]]);
        if let Some((name, _)) = DATAREFS.get(index as usize) {
            out.push((*name, value));
        }
    }
    out
}

/// COM 频率（MHz）。**优先 8.33 那个**，没有就用老的。
///
/// `_833` 的单位是 kHz，除以 1000 得兆赫，能表示 8.33 间隔（132.005）。
/// 老的那个单位是 10 kHz，除以 100 得兆赫，只有 0.01 MHz 精度——X-Plane 11.30
/// 以前只有它，8.33 的频道会被舍到最近的 25 kHz。
pub fn frequency(precise: Option<f64>, legacy: Option<f64>) -> Option<f64> {
    let round3 = |v: f64| (v * 1000.0).round() / 1000.0;
    if let Some(p) = precise {
        if p > 0.0 {
            return Some(round3(p / 1000.0));
        }
    }
    if let Some(l) = legacy {
        if l > 0.0 {
            return Some(round3(l / 100.0));
        }
    }
    None
}

/// 把收到的原始 dataref 值换算成一份 [`Snapshot`]。
pub fn snapshot(raw: &HashMap<&str, f32>) -> Option<Snapshot> {
    if raw.is_empty() {
        return None;
    }
    let get = |k: &str| raw.get(k).map(|v| f64::from(*v));
    let altitude = (get("elevation").unwrap_or(0.0) / crate::METRES_PER_FOOT).round() as i32;
    let groundspeed = (get("groundspeed").unwrap_or(0.0) * crate::KNOTS_PER_MPS).round() as i32;
    let on_ground = get("on_ground").unwrap_or(0.0) != 0.0;
    Some(Snapshot {
        latitude: get("latitude").unwrap_or(0.0),
        longitude: get("longitude").unwrap_or(0.0),
        altitude,
        pressure_delta: crate::pressure_delta(
            get("indicated_altitude"),
            get("baro_setting"),
            altitude,
        ),
        agl: (get("agl").unwrap_or(0.0) / crate::METRES_PER_FOOT).round() as i32,
        groundspeed,
        pitch: get("pitch").unwrap_or(0.0),
        bank: get("bank").unwrap_or(0.0),
        heading: get("heading_true").unwrap_or(0.0).rem_euclid(360.0),
        squawk: get("squawk").unwrap_or(2000.0) as u16,
        xpdr_mode: crate::xpdr_mode(get("xpdr_mode").map(|v| v as i32), on_ground, groundspeed),
        com1: frequency(get("com1"), get("com1_legacy")),
        com2: frequency(get("com2"), get("com2_legacy")),
        com1_power: get("com1_power").unwrap_or(1.0) != 0.0,
        on_ground,
        animation: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_voice_fsd::pilot::XpdrMode;

    /// X-Plane 只认 413 字节的 RREF 请求，短了直接丢——而"丢了"的表现是
    /// 一个 dataref 都不推送，看起来和"X-Plane 没开"一样。
    #[test]
    fn a_subscribe_packet_is_exactly_the_length_x_plane_accepts() {
        let p = subscribe_packet(UPDATE_RATE, 3, "sim/flightmodel/position/latitude");
        assert_eq!(p.len(), RREF_PACKET_LEN);
        assert_eq!(&p[..5], b"RREF\0");
        assert_eq!(i32::from_le_bytes(p[5..9].try_into().unwrap()), UPDATE_RATE);
        assert_eq!(i32::from_le_bytes(p[9..13].try_into().unwrap()), 3);
        assert!(p[13..].starts_with(b"sim/flightmodel/position/latitude\0"));
        // 尾部是零填充，不是垃圾。
        assert!(p[13 + 34..].iter().all(|b| *b == 0));
    }

    #[test]
    fn unsubscribing_is_the_same_packet_with_rate_zero() {
        let p = subscribe_packet(0, 0, "x");
        assert_eq!(i32::from_le_bytes(p[5..9].try_into().unwrap()), 0);
        assert_eq!(subscribe_all(UPDATE_RATE).len(), DATAREFS.len());
    }

    /// index 就是 [`DATAREFS`] 里的下标，所以**改顺序等于改协议编号**。
    #[test]
    fn the_index_is_the_position_in_the_table() {
        let packets = subscribe_all(UPDATE_RATE);
        for (i, p) in packets.iter().enumerate() {
            assert_eq!(i32::from_le_bytes(p[9..13].try_into().unwrap()), i as i32);
            assert!(p[13..].starts_with(DATAREFS[i].1.as_bytes()));
        }
    }

    fn rref_reply(pairs: &[(i32, f32)]) -> Vec<u8> {
        let mut d = Vec::from(&b"RREF,"[..]);
        for (i, v) in pairs {
            d.extend_from_slice(&i.to_le_bytes());
            d.extend_from_slice(&v.to_le_bytes());
        }
        d
    }

    #[test]
    fn a_reply_decodes_back_to_named_datarefs() {
        let got = parse_values(&rref_reply(&[(0, 31.5), (1, 121.5)]));
        assert_eq!(got, vec![("latitude", 31.5), ("longitude", 121.5)]);
    }

    /// **认不出的 index 跳过，不让整包作废。** X-Plane 会把上一次订阅残留的
    /// index 一起推过来，整包丢掉的话所有数据一起没了。
    #[test]
    fn an_unknown_index_does_not_throw_away_the_whole_packet() {
        let got = parse_values(&rref_reply(&[(999, 1.0), (0, 31.5)]));
        assert_eq!(got, vec![("latitude", 31.5)]);
    }

    #[test]
    fn a_malformed_reply_yields_nothing() {
        assert!(parse_values(b"NOPE").is_empty());
        assert!(parse_values(b"RREF,\x00\x00\x00").is_empty());
    }

    /// 信标里那两个字节是**信标协议**的版本，不是 X-Plane 的版本号。
    #[test]
    fn the_beacon_gives_up_the_port() {
        let mut b = Vec::from(&b"BECN\0"[..]);
        b.push(1); // 信标协议 major
        b.push(2); // minor
        b.extend_from_slice(&1i32.to_le_bytes());
        b.extend_from_slice(&11i32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&49000u16.to_le_bytes());
        assert_eq!(parse_beacon(&b), Some(49000));
        assert_eq!(parse_beacon(b"XXXX\0"), None);
        assert_eq!(parse_beacon(&b[..10]), None);
    }

    /// 虚拟网卡排最后。实测里 `198.18.0.1` 混进来过，选中它之后一个
    /// dataref 都收不到。
    #[test]
    fn a_virtual_adapter_sorts_behind_a_real_one() {
        assert!(address_rank("127.0.0.1") < address_rank("192.168.1.5"));
        assert!(address_rank("192.168.1.5") < address_rank("198.18.0.1"));
        assert_eq!(address_rank("172.17.0.1"), 2);
    }

    /// 8.33 那个优先。老的只有 0.01 MHz 精度，132.005 会被舍成 132.00。
    #[test]
    fn the_precise_frequency_wins_over_the_legacy_one() {
        assert_eq!(frequency(Some(132_005.0), Some(13200.0)), Some(132.005));
        assert_eq!(frequency(None, Some(12180.0)), Some(121.8));
        assert_eq!(frequency(Some(0.0), Some(12180.0)), Some(121.8));
        // 两个都没有就是 None，**不是 0**。
        assert_eq!(frequency(None, None), None);
        assert_eq!(frequency(Some(0.0), Some(0.0)), None);
    }

    #[test]
    fn a_snapshot_comes_out_in_the_units_fsd_wants() {
        let raw: HashMap<&str, f32> = [
            ("latitude", 31.142_33),
            ("longitude", 121.790_84),
            // 米 → 英尺
            ("elevation", 10_668.0),
            ("agl", 3048.0),
            // 米每秒 → 节
            ("groundspeed", 100.0),
            ("heading_true", 725.0),
            ("squawk", 2000.0),
            ("com1", 132_005.0),
        ]
        .into_iter()
        .collect();
        let s = snapshot(&raw).expect("snapshot");
        assert_eq!(s.altitude, 35_000);
        assert_eq!(s.agl, 10_000);
        assert_eq!(s.groundspeed, 194);
        // 航向折回 0..360，而不是原样送出去。
        assert!((s.heading - 5.0).abs() < 1e-9, "{}", s.heading);
        assert_eq!(s.com1, Some(132.005));
        assert_eq!(s.com2, None);
        // 读不到应答机档位时按在线。
        assert_eq!(s.xpdr_mode, XpdrMode::ModeC);
    }

    /// **`snapshot()` 读的每一个名字都必须在 [`DATAREFS`] 里订过。**
    ///
    /// 漏订一个的表现不是报错，是那一项永远取默认值——第一版就漏了
    /// `on_ground`，于是每架飞机在雷达上都是"在空中"，连停在机坪上的也是，
    /// 而所有测试都是绿的。
    #[test]
    fn every_field_the_snapshot_reads_is_actually_subscribed() {
        let subscribed: Vec<&str> = DATAREFS.iter().map(|(name, _)| *name).collect();
        for name in [
            "latitude",
            "longitude",
            "elevation",
            "agl",
            "indicated_altitude",
            "baro_setting",
            "groundspeed",
            "pitch",
            "bank",
            "heading_true",
            "squawk",
            "xpdr_mode",
            "com1",
            "com2",
            "com1_legacy",
            "com2_legacy",
            "com1_power",
            "on_ground",
        ] {
            assert!(
                subscribed.contains(&name),
                "{name} is read but never subscribed"
            );
        }
        // 反过来也钉一下：订了却没人读的，要么是忘了用，要么该删。
        assert_eq!(subscribed.len(), 18);
    }

    /// 停在机坪上就该报在地面。漏订 `on_ground` 的话这条会红。
    #[test]
    fn a_parked_aircraft_reports_being_on_the_ground() {
        let raw: HashMap<&str, f32> = [("latitude", 31.0), ("on_ground", 1.0)]
            .into_iter()
            .collect();
        let s = snapshot(&raw).expect("snapshot");
        assert!(s.on_ground);
        assert_eq!(s.xpdr_mode, XpdrMode::ModeC);
    }

    #[test]
    fn no_data_yet_is_none_rather_than_a_zeroed_aircraft() {
        assert!(snapshot(&HashMap::new()).is_none());
    }
}

// ——— 真正的链路 ———

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::UdpSocket;

/// 超过这么久没有新数据就认为断了。
pub const STALE_AFTER: Duration = Duration::from_secs(3);
/// 还是没有的话，重新去找一次 X-Plane。
pub const REDISCOVER_AFTER: Duration = Duration::from_secs(15);
/// 收到第一份信标后再等这么久，收齐其他网卡的。
pub const BEACON_GATHER: Duration = Duration::from_secs(1);
pub const DISCOVER_TIMEOUT: Duration = Duration::from_secs(10);

/// 一条 X-Plane 链路。
#[derive(Debug, Clone)]
pub struct Link {
    state: Arc<Mutex<LinkState>>,
}

#[derive(Debug, Default)]
struct LinkState {
    values: HashMap<&'static str, f32>,
    connected: bool,
    address: Option<SocketAddr>,
}

impl Link {
    /// 起一条链路。它自己找 X-Plane、订阅、掉了再找。
    pub fn spawn() -> Self {
        let link = Self {
            state: Arc::new(Mutex::new(LinkState::default())),
        };
        let state = link.state.clone();
        tokio::spawn(async move { run(state).await });
        link
    }

    pub fn connected(&self) -> bool {
        self.state.lock().expect("link").connected
    }

    pub fn address(&self) -> Option<SocketAddr> {
        self.state.lock().expect("link").address
    }

    /// 当前这一帧，已经换算成 FSD 要的单位。没数据返回 `None`。
    pub fn snapshot(&self) -> Option<Snapshot> {
        let state = self.state.lock().expect("link");
        snapshot(&state.values)
    }
}

/// 听多播信标，找出 X-Plane 在哪儿。
///
/// **收到第一份之后再等一会儿。** 一台装了 VPN/WSL/Docker 的机器会从每块网卡
/// 各回一份，而第一份到的往往就是虚拟网卡那份——往它发 RREF 收不到任何数据。
/// 收齐了按 [`address_rank`] 挑。
pub async fn discover(timeout: Duration) -> Option<SocketAddr> {
    let socket = bind_beacon_socket()?;
    let group: Ipv4Addr = MCAST_GROUP.parse().ok()?;
    socket
        .join_multicast_v4(group, Ipv4Addr::UNSPECIFIED)
        .ok()?;

    let deadline = tokio::time::Instant::now() + timeout;
    let mut found: Vec<SocketAddr> = Vec::new();
    let mut gather_until: Option<tokio::time::Instant> = None;
    let mut buf = [0u8; 1024];

    loop {
        let until = match gather_until {
            Some(g) => g.min(deadline),
            None => deadline,
        };
        let left = until.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match tokio::time::timeout(left, socket.recv_from(&mut buf)).await {
            Ok(Ok((n, from))) => {
                if let Some(port) = parse_beacon(&buf[..n]) {
                    let addr = SocketAddr::new(from.ip(), port);
                    if !found.contains(&addr) {
                        found.push(addr);
                    }
                    gather_until.get_or_insert(tokio::time::Instant::now() + BEACON_GATHER);
                }
            }
            Ok(Err(_)) | Err(_) => break,
        }
    }

    found.sort_by_key(|a| address_rank(&a.ip().to_string()));
    found.into_iter().next()
}

/// 绑信标端口，**必须开地址复用**。
///
/// 49707 上常常已经有别人在听：LiveTraffic、swift、另一个我们自己的实例，
/// 甚至 X-Plane 自己。不开复用的话 bind 直接失败，而失败的表现是"永远找不到
/// X-Plane"——和没开模拟器一模一样，没有任何线索指向端口被占。
fn bind_beacon_socket() -> Option<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).ok()?;
    socket.set_reuse_address(true).ok()?;
    // macOS/BSD 还要 SO_REUSEPORT 才允许两个进程同时收同一份多播。
    // socket2 只在这些平台上给这个方法，所以 cfg 要按它的条件写，
    // 不能简单地"非 Windows"。
    #[cfg(all(unix, not(any(target_os = "solaris", target_os = "illumos"))))]
    socket.set_reuse_port(true).ok()?;
    socket.set_nonblocking(true).ok()?;
    socket
        .bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, MCAST_PORT).into())
        .ok()?;
    UdpSocket::from_std(socket.into()).ok()
}

async fn run(state: Arc<Mutex<LinkState>>) {
    let mut known_good: Option<SocketAddr> = None;
    let mut last_discovered: Option<SocketAddr> = None;

    loop {
        let address = match discover(DISCOVER_TIMEOUT).await {
            Some(a) => {
                last_discovered = Some(a);
                a
            }
            // 这一轮没收到信标。**不要无脑退回本机**：明明发现过
            // 192.168.31.231，等 15 秒没数据（X-Plane 还在读盘）就把它扔了，
            // 下一轮退回 127.0.0.1，再等 15 秒，来回折腾。
            // 收过数据的地址最可信，其次是上一次发现到的。
            None => known_good
                .or(last_discovered)
                .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], DEFAULT_PORT))),
        };

        if let Some(good) = serve(&state, address).await {
            known_good = Some(good);
        }
        state.lock().expect("link").connected = false;
    }
}

/// 订上、收数据，直到太久没动静。返回"这个地址确实给过数据"。
async fn serve(state: &Arc<Mutex<LinkState>>, address: SocketAddr) -> Option<SocketAddr> {
    let Ok(socket) = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)).await else {
        return None;
    };
    for packet in subscribe_all(UPDATE_RATE) {
        if socket.send_to(&packet, address).await.is_err() {
            return None;
        }
    }
    state.lock().expect("link").address = Some(address);

    let mut ever = false;
    let mut last = tokio::time::Instant::now();
    let mut buf = [0u8; 8192];
    loop {
        let quiet = tokio::time::Instant::now().duration_since(last);
        let give_up = if ever {
            REDISCOVER_AFTER
        } else {
            DISCOVER_TIMEOUT
        };
        if quiet > give_up {
            // 退订一下再走，免得 X-Plane 一直往一个没人听的端口推。
            for packet in subscribe_all(0) {
                let _ = socket.send_to(&packet, address).await;
            }
            return ever.then_some(address);
        }
        match tokio::time::timeout(STALE_AFTER, socket.recv(&mut buf)).await {
            Ok(Ok(n)) => {
                let values = parse_values(&buf[..n]);
                if values.is_empty() {
                    continue;
                }
                ever = true;
                last = tokio::time::Instant::now();
                let mut state = state.lock().expect("link");
                state.connected = true;
                for (name, value) in values {
                    state.values.insert(name, value);
                }
            }
            Ok(Err(_)) => return ever.then_some(address),
            Err(_) => {
                state.lock().expect("link").connected = false;
            }
        }
    }
}
