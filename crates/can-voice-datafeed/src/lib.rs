//! can-fsd 的 datafeed：谁在管制、在哪个频率、哪个 CAN 号是谁。
//!
//! # 为什么客户端要读它
//!
//! 语音那条链路只认一张票，票里只有 CAN 号和 `max_tx`。管制员**在哪个席位上、
//! 守哪个频率**这件事，语音服务端不知道，也不该知道——那是 FSD 的事实。所以
//! 席位频率只能从这里查：查到了就自动加进电台栈，查不到就说明这个人此刻没在
//! 管制，不该发射。
//!
//! # 认 CAN 号，不认呼号
//!
//! 客户端手里只有用户填的那个号；呼号恰恰是要从这里查出来的东西。拿呼号去认
//! 等于要求用户先把自己的席位名一字不差地敲一遍，而敲错的表现是"自动加频率
//! 不工作"。
//!
//! # 两个会咬人的取值
//!
//! `facility == 0` 是观察员——挂着观察员不算在管制。`frequency == "199.998"`
//! 是"没设频率"的占位值，不是一个频率；拿它去订阅，人会在一个谁也不在的频率上
//! 守一整天而界面上一切正常。

use std::collections::HashMap;

use serde_json::Value;

/// 取 datafeed 时用的 User-Agent。
///
/// **数据源前面挡着 Cloudflare，非浏览器形态的 UA 一律 403。** reqwest 的默认
/// UA 就在被拒之列，所以这个头是必须的而不是礼貌——而 403 看起来像
/// "datafeed 挂了"。
pub const USER_AGENT: &str = "Mozilla/5.0 (compatible; CanVoice/1.0)";

/// datafeed 在哪。
pub const DEFAULT_URL: &str = "https://data.ceruleanavi.net/v1/data.json";

/// `frequency` 为这个值时表示"**没设频率**"，不是一个频率。
const NO_FREQUENCY_MHZ: f64 = 199.998;

/// 浮点比较的容差。上游给的可能是 `199.998`、`199.9980` 或 `199.998000`。
const FREQ_EPSILON: f64 = 0.001;

/// `facility` 为这个值表示观察员。**挂着观察员不算在管制。**
pub const FACILITY_OBSERVER: i64 = 0;

/// 一个在线席位。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Position {
    pub cid: String,
    pub callsign: String,
    pub freq_khz: u32,
    pub facility: i64,
}

/// 取一份 datafeed。
///
/// **失败返回 `None`，不报错。** 上游抖一下不该让客户端把已经在守的频率丢掉：
/// 调用方拿到 `None` 就什么都不做，保留上一次的结论等下一轮。
pub async fn fetch(client: &reqwest::Client, url: &str) -> Option<Value> {
    let response = client
        .get(url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .send()
        .await;
    match response {
        Ok(r) if r.status().is_success() => match r.json::<Value>().await {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(error = %e, "the datafeed was not valid json");
                None
            }
        },
        Ok(r) => {
            tracing::warn!(status = %r.status(), "could not read the datafeed");
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "could not read the datafeed");
            None
        }
    }
}

/// 一条记录上的频率（kHz）。没设频率、解不出来都是 `None`。
pub fn frequency_khz(entry: &Value) -> Option<u32> {
    let freq = entry.get("frequency")?;
    // 上游一直给的是字符串，但数字也收着：判错的代价是一个能守的频率被丢掉。
    let mhz: f64 = match freq {
        Value::String(s) => s.trim().parse().ok()?,
        other => other.as_f64()?,
    };
    if (mhz - NO_FREQUENCY_MHZ).abs() < FREQ_EPSILON {
        return None;
    }
    let khz = (mhz * 1000.0).round();
    if !khz.is_finite() || !(118_000.0..=136_975.0).contains(&khz) {
        return None;
    }
    let khz = khz as u32;
    (khz % 5 == 0).then_some(khz)
}

fn position_from(entry: &Value) -> Option<Position> {
    let callsign = entry.get("callsign")?.as_str()?.trim();
    if callsign.is_empty() {
        return None;
    }
    let facility = entry.get("facility").and_then(Value::as_i64)?;
    if facility <= FACILITY_OBSERVER {
        return None;
    }
    Some(Position {
        cid: cid_of(entry)?,
        callsign: callsign.to_string(),
        freq_khz: frequency_khz(entry)?,
        facility,
    })
}

/// `cid` 两边都当字符串比：datafeed 给的是字符串，而用户填的也是。
fn cid_of(entry: &Value) -> Option<String> {
    match entry.get("cid")? {
        Value::String(s) => Some(s.trim().to_string()),
        other => Some(other.as_i64()?.to_string()),
    }
}

fn controllers(feed: &Value) -> &[Value] {
    feed.get("controllers")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// 这个 CAN 号此刻在管的席位。不在管制就是 `None`。
pub fn controller_for(cid: &str, feed: &Value) -> Option<Position> {
    let cid = cid.trim();
    controllers(feed)
        .iter()
        .filter_map(position_from)
        .find(|p| p.cid == cid)
}

/// 此刻在线、且守得住的席位，按频率排，同频率按呼号排。
///
/// **同频率的多个席位不合并**：ZSPD_1_TWR 和 ZSPD_2_TWR 可以在同一个频率上，
/// 合起来会让人以为只有一个人在。
pub fn online_positions(feed: &Value) -> Vec<Position> {
    let mut out: Vec<Position> = controllers(feed).iter().filter_map(position_from).collect();
    out.sort_by(|a, b| {
        a.freq_khz
            .cmp(&b.freq_khz)
            .then_with(|| a.callsign.cmp(&b.callsign))
    });
    out
}

/// 这个 CAN 号此刻的等级。查不到就是 `None`。
///
/// **给通播登录用的。** 写死观察员的话，一个 C1 管制员开的通播在雷达图上显示成
/// 观察员，而管制席位上的同一个人是 C1。三组都找：开通播的那个人此刻多半正以
/// 管制身份连着，而他的等级在 `controllers[]` 里。
pub fn rating_for(cid: &str, feed: &Value) -> Option<u32> {
    let cid = cid.trim();
    for group in ["controllers", "pilots", "atis"] {
        let Some(list) = feed.get(group).and_then(Value::as_array) else {
            continue;
        };
        for entry in list {
            if cid_of(entry).as_deref() == Some(cid) {
                if let Some(r) = entry.get("rating").and_then(Value::as_u64) {
                    return Some(r as u32);
                }
            }
        }
    }
    None
}

/// 取一份 datafeed，自带一个一次性的 HTTP 客户端。
///
/// 给**手上没有共享客户端**的调用方用：通播制作端只在按下"上线"的那一刻查一次
/// 等级，为这一次给整个应用挂一个 `reqwest::Client` 不值得。
///
/// 超时**五秒**而不是十几秒：它用在"按下上线"那条路径上，而在那里等下去不如先
/// 上去——查不到等级只是显示成观察员，等不到才是真的上不了线。
pub async fn fetch_once(url: &str) -> Option<Value> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .ok()?;
    fetch(&client, url).await
}

/// CAN 号 → 呼号。三组人都在里面。
///
/// 语音那一侧认的是 CAN 号——服务端下发的"谁在说话"就是一个号。这张表是用来把
/// 电台行上那个"最后通话：1005"翻译成"最后通话：CES2345"的，所以飞行员和通播
/// 也要收：只收管制的话，频率上说话的那个人显示的永远是一个数字。
pub fn roster(feed: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for group in ["controllers", "pilots", "atis"] {
        let Some(list) = feed.get(group).and_then(Value::as_array) else {
            continue;
        };
        for entry in list {
            let (Some(cid), Some(callsign)) =
                (cid_of(entry), entry.get("callsign").and_then(Value::as_str))
            else {
                continue;
            };
            let callsign = callsign.trim();
            if !cid.is_empty() && !callsign.is_empty() {
                out.insert(cid, callsign.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn feed(controllers: serde_json::Value) -> serde_json::Value {
        json!({ "controllers": controllers, "pilots": [], "atis": [] })
    }

    fn atc(cid: &str, callsign: &str, frequency: &str, facility: i64) -> serde_json::Value {
        json!({
            "cid": cid,
            "callsign": callsign,
            "frequency": frequency,
            "facility": facility,
        })
    }

    /// 认的是 **CAN 号**，不是呼号。
    ///
    /// 客户端手里只有用户填的那个号，呼号恰恰是要从这里查出来的东西；拿呼号去认
    /// 等于要求用户先把自己的席位名一字不差地敲一遍。
    #[test]
    fn my_position_is_the_one_whose_cid_matches() {
        let f = feed(json!([
            atc("1000", "ZSPD_TWR", "118.350", 4),
            atc("2000", "ZBAA_APP", "120.500", 5),
        ]));

        let me = controller_for("2000", &f).expect("found");
        assert_eq!(me.callsign, "ZBAA_APP");
        assert_eq!(me.freq_khz, 120_500);
        assert_eq!(controller_for("3000", &f), None);
    }

    /// 挂着观察员不算在管制。
    ///
    /// `facility == 0` 是观察员。把他当成在席位上，等于让一个只是来看看的人
    /// 获得发射权，而他的"频率"多半还是那个没设频率的占位值。
    #[test]
    fn an_observer_is_not_staffing_a_position() {
        let f = feed(json!([atc("1000", "ZSPD_OBS", "118.350", 0)]));
        assert_eq!(controller_for("1000", &f), None);
    }

    /// `199.998` 是"**没设频率**"，不是一个频率。
    ///
    /// 拿它去订阅，人会在一个谁也不在的频率上守一整天，而界面上一切正常。
    #[test]
    fn the_no_frequency_placeholder_is_not_a_frequency() {
        assert_eq!(
            frequency_khz(&json!({ "frequency": "118.350" })),
            Some(118_350)
        );
        assert_eq!(frequency_khz(&json!({ "frequency": "199.998" })), None);
        // 上游给的可能是 `199.9980`，也可能干脆是个数字而不是字符串。
        assert_eq!(frequency_khz(&json!({ "frequency": "199.9980" })), None);
        assert_eq!(
            frequency_khz(&json!({ "frequency": 118.35 })),
            Some(118_350)
        );
        assert_eq!(frequency_khz(&json!({})), None);
    }

    #[test]
    fn off_raster_controller_frequency_is_not_a_staffed_voice_position() {
        let entry = json!({ "frequency": "118.501" });
        assert_eq!(frequency_khz(&entry), None);
        let f = feed(json!([atc("1000", "ZSPD_TWR", "118.501", 4)]));
        assert_eq!(controller_for("1000", &f), None);
    }

    /// 在线一览按频率排，同频率按呼号排。
    ///
    /// **同频率的多个席位不合并**：ZSPD_1_TWR 和 ZSPD_2_TWR 可以在同一个频率上，
    /// 合起来会让人以为只有一个人在。观察员和没设频率的不进这张表——它们在
    /// 这张表上没有任何用处，点一下只会订阅到一个空频率。
    #[test]
    fn the_online_list_is_sorted_and_leaves_out_what_cannot_be_tuned() {
        let f = feed(json!([
            atc("3", "ZSPD_2_TWR", "118.350", 4),
            atc("4", "ZGGG_OBS", "121.800", 0),
            atc("1", "ZBAA_APP", "120.500", 5),
            atc("2", "ZSPD_1_TWR", "118.350", 4),
            atc("5", "ZULS_CTR", "199.998", 6),
            atc("6", "", "127.500", 6),
        ]));

        let names: Vec<String> = online_positions(&f)
            .into_iter()
            .map(|p| p.callsign)
            .collect();
        assert_eq!(names, ["ZSPD_1_TWR", "ZSPD_2_TWR", "ZBAA_APP"]);
    }

    /// 等级跟着本人，不是一个常量。
    ///
    /// 写死观察员的话，一个 C1 管制员开的通播在雷达图上显示成观察员，而管制席位
    /// 上的同一个人是 C1。三组都找：开通播的那个人此刻多半正以管制身份连着。
    #[test]
    fn the_rating_follows_the_member() {
        let f = json!({
            "controllers": [{ "cid": "1000", "callsign": "ZSPD_TWR", "rating": 5 }],
            "pilots": [{ "cid": "2000", "callsign": "CES2345", "rating": 2 }],
            "atis": [],
        });

        assert_eq!(rating_for("1000", &f), Some(5));
        assert_eq!(rating_for("2000", &f), Some(2));
        // 没连着就查不到。调用方自己决定回落到什么。
        assert_eq!(rating_for("3000", &f), None);
    }

    /// CAN 号 → 呼号，三组人都要进去。
    ///
    /// 这张表是用来把电台行上那个"最后通话：1005"翻译成"最后通话：CES2345"的。
    /// 只收管制的话，飞行员在频率上说话时显示的永远是一个数字。
    #[test]
    fn the_roster_covers_pilots_controllers_and_atis() {
        let f = json!({
            "controllers": [atc("1000", "ZSPD_TWR", "118.350", 4)],
            "pilots": [{ "cid": "2000", "callsign": "CES2345" }],
            "atis": [{ "cid": "3000", "callsign": "ZSPD_ATIS" }],
        });

        let r = roster(&f);
        assert_eq!(r.get("1000").map(String::as_str), Some("ZSPD_TWR"));
        assert_eq!(r.get("2000").map(String::as_str), Some("CES2345"));
        assert_eq!(r.get("3000").map(String::as_str), Some("ZSPD_ATIS"));
    }

    /// 上面几条吃的都是自己写的 JSON，只证明这个 crate 和自己一致。这一条吃
    /// can-fsd 的**黄金文件**（逐字节副本，每天和上游对一次账，见
    /// `.github/workflows/datafeed-golden.yml`）：can-fsd 把 `frequency`、`facility`、
    /// `cid`、`rating` 改了名或改了类型，副本一同步这里就红。
    #[test]
    fn can_fsds_golden_datafeed_reads_the_way_the_client_expects() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../server/testdata/datafeed_golden.json"
        );
        let raw = std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let feed: Value = serde_json::from_slice(&raw).expect("parse the golden datafeed");

        let lax = Position {
            cid: "5158".into(),
            callsign: "LAX_25_CTR".into(),
            freq_khz: 126_525,
            facility: 6,
        };
        assert_eq!(controller_for("5158", &feed), Some(lax.clone()));
        let callsigns: Vec<_> = online_positions(&feed)
            .into_iter()
            .map(|p| p.callsign)
            .collect();
        assert_eq!(callsigns, ["ZSHA_CTR", "LAX_25_CTR"], "sorted by frequency");

        assert_eq!(rating_for("5158", &feed), Some(10));
        assert_eq!(rating_for("1000", &feed), Some(1));

        let r = roster(&feed);
        assert_eq!(r.get("5158").map(String::as_str), Some("LAX_25_CTR"));
        assert_eq!(r.get("1012").map(String::as_str), Some("CCA5852"));
    }
}
