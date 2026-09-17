//! 观察员模式（双人机组的右座）：只连语音，不开 FSD 连接。
//!
//! 设计 §7.3：右座的人如果也上 FSD，网络上会多出一架和机长叠在一起的飞机。
//! 所以他只连语音，在 `HELLO.follow` 里报机长那架飞机的呼号，服务端拿那架的
//! 位置给他算射程。**两个人要用各自的账号**——同一个成员号第二次登录会顶掉第一条。
//!
//! 这里是 `xpc-for-can` 和 `msfs-for-can` 共用的那几条规则，照
//! `can-audio/xpc/observer.py` 搬过来：
//!
//! - 手输的频率优先，**不看 COM1 电门**——右座那台模拟器未必开着，开着也未必
//!   调在机长那个频率上；
//! - 没手输就跟本机模拟器的 COM1；
//! - 两样都没有就是没有频率，不去猜。
//!
//! 放在这个 crate 而不是两个应用里，是因为 `apps/` 不在 workspace 里，
//! **应用里的单元测试 CI 不跑**。

use std::ops::RangeInclusive;

/// 能订阅的频率，kHz。和两个飞行员端给 COM1 夹的是同一个波段。
pub const BAND_KHZ: RangeInclusive<u32> = 118_000..=136_975;

/// 填的东西哪里不对。`Display` 是给日志的英文，界面上的那句是 [`Problem::message`]。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Problem {
    #[error("frequency {0:?} is not readable")]
    Unreadable(String),
    #[error("frequency {0:?} is outside 118.000-136.975")]
    OutOfBand(String),
    #[error("an observer needs a callsign to follow")]
    NoFollow,
    #[error("follow callsign {0:?} is not a callsign: 2-10 chars of A-Z 0-9 - _")]
    BadFollow(String),
}

impl Problem {
    /// 给人看的那一句（#29）。
    pub fn message(&self) -> can_voice_i18n::Message {
        use can_voice_i18n::Message;
        match self {
            Problem::Unreadable(value) => {
                Message::new("error.observer.unreadable").with("value", value)
            }
            Problem::OutOfBand(value) => {
                Message::new("error.observer.out_of_band").with("value", value)
            }
            Problem::NoFollow => Message::new("error.observer.no_follow"),
            Problem::BadFollow(callsign) => {
                Message::new("error.observer.bad_follow").with("callsign", callsign)
            }
        }
    }
}

/// 手输的频率 → kHz。空的是 `Ok(None)`，意思是"跟随 COM1"。
///
/// 收 `121.8`、`121.800` 和六位的 `121800`；不带小数点的数小于 1000 当 MHz
/// （`122` 是 122.000）。**先取整到 kHz 再判波段**：`136.9751` 取整之后是
/// 136.975，是合法的。8.33 kHz 间隔不校验，旧版也不校验。
///
/// 只认数字和一个小数点：`f64` 自己的解析还收 `1e2`、`inf` 这类，打错的字不该
/// 碰巧变成一个频率。
pub fn parse_frequency(text: &str) -> Result<Option<u32>, Problem> {
    let t: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if t.is_empty() {
        return Ok(None);
    }
    let unreadable = || Problem::Unreadable(text.trim().to_string());
    let well_formed = t.chars().all(|c| c.is_ascii_digit() || c == '.')
        && t.chars().filter(|&c| c == '.').count() <= 1
        && t.chars().any(|c| c.is_ascii_digit());
    if !well_formed {
        return Err(unreadable());
    }
    let n: f64 = t.parse().map_err(|_| unreadable())?;
    let khz = if t.contains('.') || n < 1000.0 {
        (n * 1000.0).round()
    } else {
        n.round()
    };
    let in_band = khz.is_finite()
        && (f64::from(*BAND_KHZ.start())..=f64::from(*BAND_KHZ.end())).contains(&khz);
    if !in_band {
        return Err(Problem::OutOfBand(text.trim().to_string()));
    }
    Ok(Some(khz as u32))
}

/// 跟随的呼号：去掉首尾空白、转大写、校验。
///
/// **大写要在这里转**：界面上的输入框只是用 CSS 显示成大写，值本身没变；而服务端
/// 按呼号查位置是区分大小写的，`HELLO` 里的小写呼号会被当成非法直接拒掉。
/// 规则是 [`can_voice_client::conn::is_valid_callsign`] 那一条（2–10 位），比 FSD
/// 飞行员呼号的 12 位严——两边对不上时以语音服务端为准，它才是拿这个值的一方。
pub fn follow_callsign(text: &str) -> Result<String, Problem> {
    let callsign = text.trim().to_ascii_uppercase();
    if callsign.is_empty() {
        return Err(Problem::NoFollow);
    }
    if !can_voice_client::conn::is_valid_callsign(&callsign) {
        return Err(Problem::BadFollow(callsign));
    }
    Ok(callsign)
}

/// 观察员此刻该在哪个频率上。
///
/// `com1` 是本机模拟器的 COM1，**已经按电门和波段筛过**（电门关着就是 `None`）。
pub fn frequency_for(manual: Option<u32>, com1: Option<u32>) -> Option<u32> {
    manual.or(com1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_way_people_write_a_frequency_is_read() {
        assert_eq!(parse_frequency("121.8"), Ok(Some(121_800)));
        assert_eq!(parse_frequency("121.800"), Ok(Some(121_800)));
        assert_eq!(parse_frequency("121800"), Ok(Some(121_800)));
        assert_eq!(parse_frequency(" 124. 35 "), Ok(Some(124_350)));
        assert_eq!(parse_frequency("122"), Ok(Some(122_000)));
        assert_eq!(parse_frequency("121."), Ok(Some(121_000)));
    }

    /// 空的是"跟随 COM1"，不是错误：清空输入框就是回到跟座舱走的那个办法。
    #[test]
    fn an_empty_box_means_follow_com1() {
        assert_eq!(parse_frequency(""), Ok(None));
        assert_eq!(parse_frequency("   "), Ok(None));
    }

    #[test]
    fn junk_is_refused_rather_than_guessed() {
        for junk in [
            "abc", "121,8", "1e2", "inf", "NaN", "-121.8", "121.8.0", ".",
        ] {
            assert!(
                matches!(parse_frequency(junk), Err(Problem::Unreadable(_))),
                "{junk:?} should be unreadable"
            );
        }
    }

    /// 打错的那个数会变成一个谁也不在的频率，而一切看起来正常。
    #[test]
    fn a_frequency_outside_the_band_is_refused() {
        assert_eq!(parse_frequency("118.000"), Ok(Some(118_000)));
        assert_eq!(parse_frequency("136.975"), Ok(Some(136_975)));
        for out in ["117.999", "136.976", "99.5", "0", "1215", "999999"] {
            assert!(
                matches!(parse_frequency(out), Err(Problem::OutOfBand(_))),
                "{out:?} should be out of band"
            );
        }
    }

    /// 先取整再判波段，和 COM1 那一条同一个顺序。
    #[test]
    fn rounding_happens_before_the_band_check() {
        assert_eq!(parse_frequency("136.9751"), Ok(Some(136_975)));
        assert_eq!(parse_frequency("117.9996"), Ok(Some(118_000)));
    }

    #[test]
    fn the_message_names_what_was_typed() {
        let m = parse_frequency(" 12x ").unwrap_err().message();
        assert_eq!(m.key, "error.observer.unreadable");
        assert_eq!(m.values["value"], "12x");
    }

    #[test]
    fn the_follow_callsign_is_uppercased_before_it_is_checked() {
        assert_eq!(follow_callsign("  cca1501 "), Ok("CCA1501".to_string()));
        assert_eq!(follow_callsign("B-6789"), Ok("B-6789".to_string()));
    }

    #[test]
    fn a_follow_callsign_the_voice_server_would_refuse_is_caught_here() {
        assert_eq!(follow_callsign(""), Err(Problem::NoFollow));
        assert_eq!(follow_callsign("   "), Err(Problem::NoFollow));
        assert_eq!(follow_callsign("C"), Err(Problem::BadFollow("C".into())));
        assert_eq!(
            follow_callsign("CCA1501 X"),
            Err(Problem::BadFollow("CCA1501 X".into()))
        );
        // FSD 放得进 12 位，语音服务端只收 10 位。
        assert_eq!(
            follow_callsign("ABCDEFGHIJK"),
            Err(Problem::BadFollow("ABCDEFGHIJK".into()))
        );
    }

    /// 手输的赢，而且不看 COM1——右座的模拟器未必调在机长那个频率上。
    #[test]
    fn a_typed_frequency_wins_over_com1() {
        assert_eq!(frequency_for(Some(121_800), Some(124_350)), Some(121_800));
        assert_eq!(frequency_for(Some(121_800), None), Some(121_800));
        assert_eq!(frequency_for(None, Some(124_350)), Some(124_350));
        assert_eq!(frequency_for(None, None), None);
    }
}
