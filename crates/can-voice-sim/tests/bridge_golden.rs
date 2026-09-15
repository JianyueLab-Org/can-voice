//! 生成插件那一侧要对的金文件。
//!
//! 分帧这件事两侧各有一份实现：Rust 的 `can_voice_sim::bridge` 和插件里的
//! `Reassembler`。**只有这一侧有测试**，而两边一旦对不上，表现是插件从某一帧
//! 起再也收不到任何数据、整个天空清空——`can-audio` 就是这么坏过一次（v1 按
//! 字符串切片，切口落在汉字中间）。
//!
//! 所以这里把几组真实形状的消息编成包写进金文件，
//! `apps/xpc/plugin/test_bridge.py` 拿它去喂插件的 `Reassembler`。

use can_voice_sim::bridge;
use serde_json::json;

#[test]
fn write_the_golden_file_the_plugin_checks_itself_against() {
    let cases = vec![
        ("small", json!({"traffic": []}), bridge::MAX_PAYLOAD),
        (
            "chinese-cut-in-half",
            json!({"traffic": [{"callsign": "国航一零一", "csl": "中文机型路径".repeat(20)}]}),
            8,
        ),
        (
            "sixty-four-aircraft",
            json!({"traffic": (0..64).map(|i| json!({
                "callsign": format!("CES{i:04}"),
                "latitude": 31.0 + f64::from(i) / 100.0,
                "longitude": 121.0,
                "altitude": 35_000,
                "gear_down": serde_json::Value::Null,
                "beacon_on": true,
            })).collect::<Vec<_>>()}),
            512,
        ),
    ];

    let mut out = serde_json::Map::new();
    for (name, message, payload) in cases {
        let packets: Vec<String> = bridge::encode_with(&message, 7, payload)
            .into_iter()
            .map(|p| String::from_utf8(p).expect("utf-8"))
            .collect();
        assert!(!packets.is_empty());
        out.insert(
            name.to_string(),
            json!({ "packets": packets, "message": message }),
        );
    }

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/bridge-golden.json");
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(out)).expect("json");
    // 只在内容真的变了时才写，免得每次跑测试都把工作树弄脏。
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        std::fs::write(&path, &text).expect("write the golden file");
    }
}
