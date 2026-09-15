//! 把"席位 + 预设 + 报文 + 情报字母"渲染成一份通播。
//!
//! 这是 [`crate::metar`]、[`crate::template`]、[`crate::chinese`] 三块的汇合点，
//! 也是**唯一**知道一份通播有几种形态的地方：
//!
//! ```text
//! text      文字通播。飞行员在客户端里读的就是它，所以保留电码原样。
//! voice_en  英文语音稿。模板的 voice 形态，给本地 TTS。
//! voice_zh  中文语音稿。不是英文的翻译，[`crate::chinese`] 从报文独立渲染。
//! wire      发上 FSD 的那一份（`text_atis`）。
//! ```
//!
//! # `wire` 为什么可能带一个 `|`
//!
//! `|` 是**语言的标记**，英文在前中文在后——[`crate::readback`] 按它决定哪一半
//! 用哪种嗓子。一个只播中文的席位因此只能写成 `|中文`：把英文那半留空是它表达
//! "这一份只有中文"的唯一写法。空的那一半不会被送去合成，见
//! [`crate::station`]。

use crate::chinese;
use crate::metar::Metar;
use crate::profile::{Preset, Station, VoiceLanguage};
use crate::template::{self, FreeText};

/// 一份渲染好的通播。
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Rendered {
    pub text: String,
    pub voice_en: String,
    pub voice_zh: String,
    pub wire: String,
}

/// 渲染一份通播。
pub fn render(station: &Station, preset: &Preset, metar: &Metar, letter: char) -> Rendered {
    let fields = FreeText {
        facility: station.identifier.clone(),
        facility_voice: station.name.clone(),
        letter: letter.to_string(),
        airport_conditions: preset.airport_conditions.clone(),
        notams: preset.notams.clone(),
        transition_level: preset.transition_level.clone(),
        // 空串表示"用内置那句"，不是"不说收尾语"。
        closing: if preset.closing.is_empty() {
            None
        } else {
            Some(preset.closing.clone())
        },
    };
    let ctx = template::build_context(metar, &fields);
    let (text, voice_en) = template::render(&preset.template, &ctx, &station.contractions);

    // 中文稿的跑道**跟着预设走**，预设没填才回退到席位上的那个——切到"北向"
    // 时英文稿的 ARR RWY 会变，中文稿要是还念着南向的跑道，同一份通播里两种
    // 语言互相矛盾。
    let runway = if preset.chinese_runway.is_empty() {
        station.chinese_runway.as_str()
    } else {
        preset.chinese_runway.as_str()
    };
    let facility = if station.chinese_name.is_empty() {
        station.identifier.as_str()
    } else {
        station.chinese_name.as_str()
    };
    let voice_zh = chinese::render(
        Some(metar),
        &chinese::Script {
            facility,
            letter: &letter.to_string(),
            runway,
            extra: &preset.chinese_extra,
        },
    );

    let wire = match station.voice_language {
        VoiceLanguage::En => text.clone(),
        // 英文那半留空，见模块头。
        VoiceLanguage::Zh => format!("{}{}", crate::readback::SEPARATOR, voice_zh),
        VoiceLanguage::Both => format!("{text} {} {voice_zh}", crate::readback::SEPARATOR),
    };

    Rendered {
        text,
        voice_en,
        voice_zh,
        wire,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZSPD: &str = "ZSPD 251300Z 09004MPS 9999 FEW030 SCT100 25/18 Q1013 NOSIG";

    fn station() -> Station {
        let mut s = Station::new("ZSPD");
        s.name = "Shanghai Pudong International Airport".into();
        s.chinese_name = "上海浦东".into();
        s.chinese_runway = "三五左".into();
        s
    }

    fn rendered(lang: VoiceLanguage) -> Rendered {
        let mut s = station();
        s.voice_language = lang;
        let preset = s.presets[0].clone();
        render(&s, &preset, &Metar::parse(ZSPD), 'B')
    }

    #[test]
    fn the_text_keeps_the_code_and_the_voice_spells_it_out() {
        let r = rendered(VoiceLanguage::En);
        assert!(r.text.contains("09004MPS"), "{}", r.text);
        assert!(
            r.voice_en.contains("wind zero niner zero degrees"),
            "{}",
            r.voice_en
        );
        // 文字稿留 ICAO，语音念机场全名。
        assert!(r.text.starts_with("ZSPD ATIS B"), "{}", r.text);
        assert!(
            r.voice_en
                .starts_with("Shanghai Pudong International Airport"),
            "{}",
            r.voice_en
        );
    }

    /// 中文稿**不是英文的翻译**，是从报文独立渲染的——语序和读法都不一样。
    #[test]
    fn the_chinese_script_is_rendered_from_the_report_not_translated() {
        let r = rendered(VoiceLanguage::Both);
        assert!(
            r.voice_zh.starts_with("上海浦东情报通播 布拉沃"),
            "{}",
            r.voice_zh
        );
        assert!(r.voice_zh.contains("使用跑道 三五左"), "{}", r.voice_zh);
        assert!(
            r.voice_zh.contains("修正海压 幺 洞 幺 三 百帕"),
            "{}",
            r.voice_zh
        );
    }

    /// 只播英文时线上没有分隔符——多一个 `|` 会让机队去找一个不存在的中文半边。
    #[test]
    fn an_english_only_station_puts_no_separator_on_the_wire() {
        let r = rendered(VoiceLanguage::En);
        assert!(!r.wire.contains('|'), "{}", r.wire);
        assert_eq!(r.wire, r.text);
    }

    #[test]
    fn a_bilingual_station_puts_english_first() {
        let r = rendered(VoiceLanguage::Both);
        let (en, zh) = r.wire.split_once('|').expect("one separator");
        assert!(en.contains("09004MPS"), "{en}");
        assert!(zh.contains("上海浦东"), "{zh}");
        assert_eq!(r.wire.matches('|').count(), 1, "{}", r.wire);
    }

    /// **只播中文的那一份把英文半边留空。** 分隔符就是语言的标记，
    /// 没有别的地方能说"这一份只有中文"；不带分隔符的话机队会拿英文读法去念
    /// 一句中文，1007 会变成 "one zero zero seven"。
    #[test]
    fn a_chinese_only_station_leaves_the_english_half_empty() {
        let r = rendered(VoiceLanguage::Zh);
        assert!(r.wire.starts_with('|'), "{}", r.wire);
        assert_eq!(r.wire.matches('|').count(), 1, "{}", r.wire);
        let spoken = crate::station::halves_for_test(&r.wire);
        assert_eq!(spoken.len(), 1, "{spoken:?}");
        assert!(
            spoken[0].1,
            "a chinese-only report must use the chinese voice"
        );
    }

    /// 预设的跑道盖过席位的；预设留空才用席位那个。
    #[test]
    fn the_preset_runway_wins_over_the_station_one() {
        let s = station();
        let mut preset = s.presets[0].clone();
        preset.chinese_runway = "三六右".into();
        let r = render(&s, &preset, &Metar::parse(ZSPD), 'B');
        assert!(r.voice_zh.contains("三六右"), "{}", r.voice_zh);
        assert!(!r.voice_zh.contains("三五左"), "{}", r.voice_zh);
    }

    /// 预设的收尾语是空串时用内置那句，**不是不说收尾语**。
    #[test]
    fn an_empty_preset_closing_falls_back_to_the_builtin() {
        let r = rendered(VoiceLanguage::En);
        assert!(
            r.voice_en.contains("advise on initial contact"),
            "{}",
            r.voice_en
        );
    }
}
