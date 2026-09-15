//! 跨实现金文件：Rust 的 METAR / 语音修整 / 模板渲染要和 `can-audio/atis`
//! 的 Python 实现**一字不差**。
//!
//! # 为什么值得一个金文件
//!
//! 这三个模块是一次移植。移植的风险不在"写不出来"，而在**写出一个差一点点的
//! 版本**：少一个逗号、`Q0995` 少念一位、列表里第二个跑道没展开。这些在单元
//! 测试里每一条都得有人先想到才会被写下来，而通播是播给人听的——错了要等到
//! 有人在频率上听见才知道。
//!
//! 金文件换一个方向：拿 15 份真实报文、12 段真实自由文本、8 个模板交叉出
//! 120 次渲染，全部由旧实现生成一遍，新实现必须逐字符对上。没有人需要事先
//! 想到"`RERA` 要念成 recent rain"。
//!
//! 重新生成：`cd crates/can-voice-atis/testdata && python3 gen_atis_golden.py > atis-golden.json`
//! ——**只在 Python 那边确实改了行为时才重新生成**。移植出了分歧就去改 Rust；
//! 重新生成金文件只会把分歧盖掉，而那正是它要防住的事。

use can_voice_atis::chinese;
use can_voice_atis::metar::Metar;
use can_voice_atis::template::{self, Contractions, FreeText};
use can_voice_atis::voicefix;
use serde_json::Value;

fn golden() -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/atis-golden.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    serde_json::from_str(&raw).expect("the golden file is not valid json")
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

#[test]
fn every_metar_parses_exactly_as_the_python_one_did() {
    let g = golden();
    let cases = g["metars"].as_array().expect("metars");
    assert!(cases.len() >= 15, "the corpus shrank: {}", cases.len());

    for case in cases {
        let raw = s(&case["raw"]);
        let m = Metar::parse(raw);
        assert_eq!(m.station, s(&case["station"]), "station of {raw:?}");
        assert_eq!(m.is_valid(), case["valid"], "is_valid of {raw:?}");
        assert_eq!(m.cavok, case["cavok"], "cavok of {raw:?}");
        assert_eq!(m.auto, case["auto"], "auto of {raw:?}");

        for (name, want) in case["elements"].as_object().expect("elements") {
            let got = match name.as_str() {
                "observation_time" => &m.observation_time,
                "wind" => &m.wind,
                "visibility" => &m.visibility,
                "rvr" => &m.rvr,
                "present_weather" => &m.present_weather,
                "clouds" => &m.clouds,
                "temperature" => &m.temperature,
                "dew_point" => &m.dew_point,
                "pressure" => &m.pressure,
                "trend" => &m.trend,
                "recent_weather" => &m.recent_weather,
                other => panic!("the golden file names an element that does not exist: {other}"),
            };
            assert_eq!(got.text, s(&want["text"]), "{name}.text of {raw:?}");
            assert_eq!(got.voice, s(&want["voice"]), "{name}.voice of {raw:?}");
        }

        let full = m.full_wx();
        assert_eq!(
            full.text,
            s(&case["full_wx"]["text"]),
            "full_wx.text of {raw:?}"
        );
        assert_eq!(
            full.voice,
            s(&case["full_wx"]["voice"]),
            "full_wx.voice of {raw:?}"
        );
    }
}

#[test]
fn free_text_is_expanded_and_polished_exactly_as_before() {
    let g = golden();
    let cases = g["free_text"].as_array().expect("free_text");
    assert!(cases.len() >= 12, "the corpus shrank: {}", cases.len());
    for case in cases {
        let input = s(&case["input"]);
        assert_eq!(
            voicefix::expand_free_text(input),
            s(&case["expand"]),
            "expand_free_text({input:?})"
        );
        assert_eq!(
            voicefix::polish(input),
            s(&case["polish"]),
            "polish({input:?})"
        );
    }
}

#[test]
fn every_render_comes_out_word_for_word() {
    let g = golden();
    let cases = g["renders"].as_array().expect("renders");
    assert!(cases.len() >= 120, "the corpus shrank: {}", cases.len());

    // 和 `testdata/gen_atis_golden.py` 里那份必须一致。
    let mut contractions = Contractions::new();
    contractions.insert("ils".into(), ("ILS".into(), "I L S".into()));
    contractions.insert("vor".into(), ("VOR".into(), String::new()));

    let fields = FreeText {
        facility: "ZSPD".into(),
        facility_voice: "Shanghai Pudong International Airport".into(),
        letter: "F".into(),
        airport_conditions: "ARR RWY 16L, 17R. ILS APCH IN USE".into(),
        notams: "TWY A CLSD DUE MAINT".into(),
        transition_level: "3600".into(),
        closing: None,
    };

    for case in cases {
        let raw = s(&case["metar"]);
        let tpl = s(&case["template"]);
        let m = Metar::parse(raw);
        let ctx = template::build_context(&m, &fields);
        let (text, voice) = template::render(tpl, &ctx, &contractions);
        assert_eq!(text, s(&case["text"]), "text of {tpl:?} on {raw:?}");
        assert_eq!(voice, s(&case["voice"]), "voice of {tpl:?} on {raw:?}");

        let unknown: Vec<String> = case["unknown"]
            .as_array()
            .expect("unknown")
            .iter()
            .map(|v| s(v).to_string())
            .collect();
        assert_eq!(
            template::unknown_variables(tpl),
            unknown,
            "unknown of {tpl:?}"
        );
    }
}

/// 中文通播稿不是英文稿的逐词翻译，所以它也要自己的一份金文件。
///
/// 这一份钉住的东西里有两条是**领域知识而不是实现细节**：云高按
/// 100 英尺 = 30 米折算（不是精确的 30.48，那会念出"九百一十米"），
/// 温度按整数念（二十五）而风向按无线电逐位念（洞 九 洞）。
#[test]
fn the_chinese_script_reads_word_for_word_as_before() {
    let g = golden();
    let cases = g["chinese"].as_array().expect("chinese");
    assert!(cases.len() >= 64, "the corpus shrank: {}", cases.len());
    for case in cases {
        let script = chinese::Script {
            facility: s(&case["facility"]),
            letter: s(&case["letter"]),
            runway: s(&case["runway"]),
            extra: s(&case["extra"]),
        };
        // `metar: null` 和空串都是"**还没有报文**"那一路：只念台名和字母，
        // 不编气象，也不念收尾语——没有内容可收。
        let parsed = case["metar"]
            .as_str()
            .filter(|r| !r.is_empty())
            .map(Metar::parse);
        let got = chinese::render(parsed.as_ref(), &script);
        assert_eq!(
            got,
            s(&case["script"]),
            "{:?} on {:?}",
            case["metar"],
            script.facility
        );
    }
}

/// 中文的整数念法。`spell_count` 有一条真正的规则藏在里面：
/// **一十五说成十五，但一百一十五的"一百"要留**——所以 10/15 和 110/115
/// 必须都在语料里，只测一边看不出这条。
#[test]
fn chinese_numbers_are_spoken_as_numbers_not_digit_by_digit() {
    let g = golden();
    let cases = g["counts"].as_array().expect("counts");
    assert!(cases.len() >= 25, "the corpus shrank: {}", cases.len());
    for case in cases {
        let value = case["value"].as_i64().expect("value");
        assert_eq!(
            chinese::spell_count(value),
            s(&case["spoken"]),
            "spell_count({value})"
        );
    }
}
