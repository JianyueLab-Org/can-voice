//! 机队的对账：datafeed 说该播什么，现在在播什么，差额是什么。
//!
//! 拆成一个纯函数，是因为这里面每一条规则都是踩出来的，而它们在一个带网络和
//! 定时器的循环里没法测。

use crate::datafeed::Station;
use std::collections::HashMap;

/// 一路正在跑的通播。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub freq_khz: u32,
    pub text: String,
    /// 那个任务还活着吗。**这一位是承重的**，见 `reconcile` 的文档。
    pub alive: bool,
}

/// 对账之后要做的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Start(Station),
    UpdateText { callsign: String, text: String },
    Stop(String),
}

/// 算出从"现在这样"到"该是那样"要做哪些事。
///
/// # 死掉的那一路要重新拉起
///
/// Python 版为此写过一条注释，值得原样带过来：只查"**在不在字典里**"的话，
/// 一次瞬时故障（启动那次登录失败、频率被拒……）就让这个席位**永远停播**，
/// 而管理器还以为它好好的、每 30 秒给它更新一次文本。所以判据是
/// `alive`，不是"在不在表里"。
///
/// # 先停后起
///
/// 重开时 `Stop` 一定排在 `Start` 前面。反过来的话，同一个呼号的新旧两路会有
/// 一瞬间同时在线，而服务端对同一个 CID 会**顶号**——两边互相把对方踢下去，
/// 而且每一轮重连都"成功"，计数器一次都不累加。
pub fn reconcile(current: &HashMap<String, Running>, wanted: &[Station]) -> Vec<Action> {
    let mut actions = Vec::new();

    for station in wanted {
        match current.get(&station.callsign) {
            None => actions.push(Action::Start(station.clone())),
            Some(run) if !run.alive || run.freq_khz != station.freq_khz => {
                // 频率变了也要重开：订阅是握手时定的，改不了。
                actions.push(Action::Stop(station.callsign.clone()));
                actions.push(Action::Start(station.clone()));
            }
            Some(run) if run.text != station.text => {
                // 报文变了只换文本，不重开——重开意味着断一次连接、重新握手，
                // 而报文每隔几分钟就会变一次（信息识别码进位）。
                actions.push(Action::UpdateText {
                    callsign: station.callsign.clone(),
                    text: station.text.clone(),
                });
            }
            Some(_) => {}
        }
    }

    for callsign in current.keys() {
        if !wanted.iter().any(|s| &s.callsign == callsign) {
            actions.push(Action::Stop(callsign.clone()));
        }
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datafeed::Station;

    fn st(callsign: &str, khz: u32, text: &str) -> Station {
        Station {
            callsign: callsign.into(),
            freq_khz: khz,
            text: text.into(),
        }
    }

    fn running(khz: u32, text: &str, alive: bool) -> Running {
        Running {
            freq_khz: khz,
            text: text.into(),
            alive,
        }
    }

    fn fleet(entries: &[(&str, Running)]) -> HashMap<String, Running> {
        entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn a_new_station_is_started() {
        let acts = reconcile(&HashMap::new(), &[st("ZSSS_ATIS", 132_250, "a")]);
        assert_eq!(acts, vec![Action::Start(st("ZSSS_ATIS", 132_250, "a"))]);
    }

    #[test]
    fn a_station_that_went_off_the_air_is_stopped() {
        let f = fleet(&[("ZSSS_ATIS", running(132_250, "a", true))]);
        assert_eq!(reconcile(&f, &[]), vec![Action::Stop("ZSSS_ATIS".into())]);
    }

    #[test]
    fn an_unchanged_station_is_left_alone() {
        let f = fleet(&[("ZSSS_ATIS", running(132_250, "a", true))]);
        assert!(reconcile(&f, &[st("ZSSS_ATIS", 132_250, "a")]).is_empty());
    }

    /// 报文变了只换文本，**不重开这一路**：重开意味着断一次连接、重新握手，
    /// 而报文每隔几分钟就会变一次（信息识别码进位）。
    #[test]
    fn a_changed_report_only_updates_the_text() {
        let f = fleet(&[("ZSSS_ATIS", running(132_250, "old", true))]);
        assert_eq!(
            reconcile(&f, &[st("ZSSS_ATIS", 132_250, "new")]),
            vec![Action::UpdateText {
                callsign: "ZSSS_ATIS".into(),
                text: "new".into()
            }]
        );
    }

    /// **死掉的那一路要重新拉起。**
    ///
    /// Python 版为此写过一条注释：只查"在不在字典里"的话，一次瞬时故障
    /// （启动那次登录失败、频率被拒……）就让这个席位**永远停播**，
    /// 而管理器还以为它好好的、每 30 秒给它更新一次文本。
    #[test]
    fn a_dead_station_is_relaunched_not_merely_updated() {
        let f = fleet(&[("ZSSS_ATIS", running(132_250, "a", false))]);
        let acts = reconcile(&f, &[st("ZSSS_ATIS", 132_250, "a")]);
        assert_eq!(
            acts,
            vec![
                Action::Stop("ZSSS_ATIS".into()),
                Action::Start(st("ZSSS_ATIS", 132_250, "a"))
            ],
            "a dead thread must be replaced, not left to receive text updates forever"
        );
    }

    /// 换频率要重开：订阅是握手时定的，改不了。
    #[test]
    fn a_changed_frequency_restarts_the_station() {
        let f = fleet(&[("ZSSS_ATIS", running(132_250, "a", true))]);
        let acts = reconcile(&f, &[st("ZSSS_ATIS", 127_800, "a")]);
        assert_eq!(acts[0], Action::Stop("ZSSS_ATIS".into()));
        assert_eq!(acts[1], Action::Start(st("ZSSS_ATIS", 127_800, "a")));
    }

    /// 一路出问题不该带倒别的：三个席位里死了一个，另外两个照播。
    #[test]
    fn one_broken_station_does_not_disturb_the_others() {
        let f = fleet(&[
            ("A_ATIS", running(118_000, "a", true)),
            ("B_ATIS", running(119_000, "b", false)),
            ("C_ATIS", running(120_000, "c", true)),
        ]);
        let acts = reconcile(
            &f,
            &[
                st("A_ATIS", 118_000, "a"),
                st("B_ATIS", 119_000, "b"),
                st("C_ATIS", 120_000, "c"),
            ],
        );
        assert_eq!(acts.len(), 2, "only the dead one is touched: {acts:?}");
        assert!(acts.contains(&Action::Stop("B_ATIS".into())));
    }

    /// 动作有稳定顺序，先停后起——否则同一个呼号的新旧两路会有一瞬间同时在线，
    /// 而服务端对同一个 CID 会顶号，两边互相把对方踢下去。
    #[test]
    fn a_restart_stops_before_it_starts() {
        let f = fleet(&[("ZSSS_ATIS", running(132_250, "a", false))]);
        let acts = reconcile(&f, &[st("ZSSS_ATIS", 132_250, "a")]);
        let stop = acts.iter().position(|a| matches!(a, Action::Stop(_)));
        let start = acts.iter().position(|a| matches!(a, Action::Start(_)));
        assert!(stop < start, "stop must come first: {acts:?}");
    }
}
