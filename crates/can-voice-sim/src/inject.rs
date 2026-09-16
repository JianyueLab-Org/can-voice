//! 他机注入的账：哪几架要新建、哪几架要更新、哪几架该撤掉。
//!
//! MSFS 这一侧没有插件，他机是用 SimConnect 的 AI 机呈现的。真正调 SimConnect
//! 的那几下是 Windows-only 的，**但决定"调哪一下"完全是账本上的事**，和平台无关
//! ——所以它在这里，有测试。
//!
//! # 为什么不是每帧照着他机表重建一遍
//!
//! 新建一架 AI 机要模拟器加载模型，几十到几百毫秒，而且每次都会在原地闪一下。
//! 位置更新则是廉价的。所以账要记住"这架已经建过了"，只在**机型变了**的时候
//! 才重建——涂装换了不值得重建，位置变了更不值得。

use crate::traffic::Entry;
use std::collections::HashMap;

/// 一架已经在模拟器里的 AI 机。
#[derive(Debug, Clone, PartialEq)]
struct Live {
    /// SimConnect 给的对象 id。
    object_id: u32,
    /// 建它时用的机型。变了就得重建。
    equipment: String,
}

/// 这一帧要对模拟器做的事。
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// 新建一架。
    ///
    /// `equipment` 是机型码，调用方拿它去 [`crate::msfs_models::candidates`]
    /// 换机模标题。**`entry` 是当前位置**：MSFS 建 AI 机时要给一个初始位置，
    /// 不给就是 0°N 0°E——那架飞机会在几内亚湾外面出现半秒再跳过来，而且
    /// 模拟器可能顺手去加载那一块地景。
    Create {
        callsign: String,
        equipment: String,
        entry: Box<Entry>,
    },
    /// 更新位置。
    Update {
        object_id: u32,
        callsign: String,
        entry: Box<Entry>,
    },
    /// 撤掉。
    Remove { object_id: u32, callsign: String },
}

/// AI 机的账本。
#[derive(Debug, Default)]
pub struct Injector {
    live: HashMap<String, Live>,
    /// 已经要过、还没拿到 id 的。**不重复要**——重复要会在模拟器里堆出
    /// 好几架同一个呼号的飞机，而它们互相重叠，看起来只是"抖"。
    pending: HashMap<String, String>,
    /// 这架建失败过几次。调用方拿它去取下一个候选机模。
    attempts: HashMap<String, u32>,
    /// 候选试完了还是建不出来的。**不再要**——否则每 500 ms 重试一次，
    /// 一架永远建不出来的飞机会一直刷日志，而且它挤掉的是别的飞机的重试。
    abandoned: HashMap<String, String>,
}

impl Injector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// 模拟器回了新建的结果。
    pub fn created(&mut self, callsign: &str, object_id: u32) {
        let Some(equipment) = self.pending.remove(callsign) else {
            return;
        };
        // 建出来了，计数清掉——下次这架要重建（换机型）时从最像的那个重新开始。
        self.attempts.remove(callsign);
        self.abandoned.remove(callsign);
        self.live.insert(
            callsign.to_string(),
            Live {
                object_id,
                equipment,
            },
        );
    }

    /// 新建失败了（模型找不到之类）。把待办撤掉，下一帧会**换一个机模**再要
    /// 一次——重试同一个失败过的标题没有意义，它不会因为再来一次就存在。
    pub fn failed(&mut self, callsign: &str) {
        self.pending.remove(callsign);
        *self.attempts.entry(callsign.to_string()).or_insert(0) += 1;
    }

    /// 这架建失败过几次。调用方用它当候选机模的下标：第 0 次用最像的那个，
    /// 失败一次就退一格。
    pub fn attempt(&self, callsign: &str) -> u32 {
        self.attempts.get(callsign).copied().unwrap_or(0)
    }

    /// 候选试完了，放弃这架。**这一帧之后不再要它**，直到它离线再回来。
    ///
    /// 没有这一步的话，一架在这台机器上没有任何机模能顶的飞机会每 500 ms
    /// 重试一次，永远。
    pub fn give_up(&mut self, callsign: &str, equipment: &str) {
        self.pending.remove(callsign);
        self.abandoned
            .insert(callsign.to_string(), equipment.to_string());
    }

    /// 放弃了几架。界面上要说得出"有 3 架画不出来"，而不是让人以为天上就这么空。
    pub fn abandoned(&self) -> usize {
        self.abandoned.len()
    }

    /// 拿这一帧的他机表算出要做的事。
    pub fn reconcile(&mut self, entries: &[Entry]) -> Vec<Action> {
        let mut actions = Vec::new();
        let mut seen: Vec<&str> = Vec::with_capacity(entries.len());

        for entry in entries {
            seen.push(&entry.callsign);
            // **机型还不知道就先不建。** 建了之后机型才到，就得拆了重建，
            // 而重建会让飞机在原地闪一下。
            if entry.equipment.is_empty() {
                continue;
            }
            match self.live.get(&entry.callsign) {
                Some(live) if live.equipment == entry.equipment => {
                    actions.push(Action::Update {
                        object_id: live.object_id,
                        callsign: entry.callsign.clone(),
                        entry: Box::new(entry.clone()),
                    });
                }
                // 机型变了：拆了重建。**涂装变了不算**——重建的代价是原地闪
                // 一下，不值得为一个涂装付。
                Some(live) => {
                    actions.push(Action::Remove {
                        object_id: live.object_id,
                        callsign: entry.callsign.clone(),
                    });
                    self.live.remove(&entry.callsign);
                    self.want(entry, &mut actions);
                }
                None => self.want(entry, &mut actions),
            }
        }

        // 不在这一帧里的撤掉。
        let gone: Vec<String> = self
            .live
            .keys()
            .filter(|c| !seen.contains(&c.as_str()))
            .cloned()
            .collect();
        for callsign in gone {
            if let Some(live) = self.live.remove(&callsign) {
                actions.push(Action::Remove {
                    object_id: live.object_id,
                    callsign,
                });
            }
        }
        self.pending.retain(|c, _| seen.contains(&c.as_str()));
        // 走掉的飞机把计数也带走。**它回来时该从最像的机模重新试一遍**——
        // 上一次失败可能是因为当时模拟器正在加载，不是因为机模不存在。
        self.attempts.retain(|c, _| seen.contains(&c.as_str()));
        self.abandoned.retain(|c, _| seen.contains(&c.as_str()));
        actions
    }

    fn want(&mut self, entry: &Entry, actions: &mut Vec<Action>) {
        let (callsign, equipment) = (&entry.callsign, &entry.equipment);
        if self.pending.contains_key(callsign) {
            return; // 已经要过了，等回音
        }
        // 放弃过的不再要——除非它换了机型，那是一组全新的候选。
        match self.abandoned.get(callsign) {
            Some(gave_up_on) if gave_up_on == equipment => return,
            Some(_) => {
                self.abandoned.remove(callsign);
                self.attempts.remove(callsign);
            }
            None => {}
        }
        self.pending.insert(callsign.clone(), equipment.clone());
        actions.push(Action::Create {
            callsign: callsign.clone(),
            equipment: equipment.clone(),
            entry: Box::new(entry.clone()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traffic::{Sample, TrafficTable};

    fn entries(specs: &[(&str, &str)]) -> Vec<Entry> {
        let mut t = TrafficTable::new();
        for (callsign, equipment) in specs {
            t.update_position(
                callsign,
                2000,
                Sample {
                    time: 0.0,
                    latitude: 31.0,
                    longitude: 121.0,
                    altitude: 1000.0,
                    pitch: 0.0,
                    bank: 0.0,
                    heading: 0.0,
                    on_ground: false,
                    groundspeed: 250.0,
                },
            );
            if !equipment.is_empty() {
                t.set_plane_info(callsign, 0.0, equipment, "");
            }
        }
        t.snapshot(0.0, None, None, None)
    }

    #[test]
    fn a_new_aircraft_is_created_once() {
        let mut i = Injector::new();
        let table = entries(&[("CES123", "A320")]);
        let first = i.reconcile(&table);
        assert_eq!(
            first,
            vec![Action::Create {
                callsign: "CES123".into(),
                equipment: "A320".into(),
                entry: Box::new(table[0].clone()),
            }]
        );
        // **不重复要。** 重复要会在模拟器里堆出好几架同一个呼号的飞机，
        // 而它们互相重叠，看起来只是"抖"。
        assert!(i.reconcile(&entries(&[("CES123", "A320")])).is_empty());
    }

    #[test]
    fn once_created_it_is_only_updated() {
        let mut i = Injector::new();
        i.reconcile(&entries(&[("CES123", "A320")]));
        i.created("CES123", 42);
        match i.reconcile(&entries(&[("CES123", "A320")])).as_slice() {
            [Action::Update {
                object_id,
                callsign,
                ..
            }] => {
                assert_eq!(*object_id, 42);
                assert_eq!(callsign, "CES123");
            }
            other => panic!("got {other:?}"),
        }
        assert_eq!(i.len(), 1);
    }

    /// **机型还不知道就先不建。** 建了之后机型才到，就得拆了重建，而重建会让
    /// 飞机在原地闪一下。
    #[test]
    fn an_aircraft_without_a_type_yet_is_not_created() {
        let mut i = Injector::new();
        assert!(i.reconcile(&entries(&[("CES123", "")])).is_empty());
        assert!(i.is_empty());
    }

    /// 机型变了要拆了重建；**涂装变了不算**——重建的代价是原地闪一下。
    #[test]
    fn only_a_changed_type_is_worth_a_rebuild() {
        let mut i = Injector::new();
        i.reconcile(&entries(&[("CES123", "A320")]));
        i.created("CES123", 42);

        let changed = entries(&[("CES123", "B738")]);
        let actions = i.reconcile(&changed);
        assert_eq!(
            actions,
            vec![
                Action::Remove {
                    object_id: 42,
                    callsign: "CES123".into()
                },
                Action::Create {
                    callsign: "CES123".into(),
                    equipment: "B738".into(),
                    entry: Box::new(changed[0].clone()),
                }
            ]
        );
    }

    #[test]
    fn an_aircraft_that_went_away_is_removed() {
        let mut i = Injector::new();
        i.reconcile(&entries(&[("CES123", "A320")]));
        i.created("CES123", 42);
        assert_eq!(
            i.reconcile(&[]),
            vec![Action::Remove {
                object_id: 42,
                callsign: "CES123".into()
            }]
        );
        assert!(i.is_empty());
    }

    /// 新建失败（模型找不到之类）之后下一帧要再试一次，不能永远卡在待办里。
    #[test]
    fn a_failed_creation_is_retried() {
        let mut i = Injector::new();
        i.reconcile(&entries(&[("CES123", "A320")]));
        assert!(i.reconcile(&entries(&[("CES123", "A320")])).is_empty());
        i.failed("CES123");
        assert_eq!(i.reconcile(&entries(&[("CES123", "A320")])).len(), 1);
    }

    /// **每失败一次就该换一个机模。** 调用方拿这个数当候选表的下标；它要是
    /// 不涨，重试的就永远是同一个已经证明不存在的标题。
    #[test]
    fn each_failure_moves_one_step_down_the_candidate_list() {
        let mut i = Injector::new();
        assert_eq!(i.attempt("CES123"), 0);
        i.reconcile(&entries(&[("CES123", "A320")]));
        i.failed("CES123");
        assert_eq!(i.attempt("CES123"), 1);
        i.reconcile(&entries(&[("CES123", "A320")]));
        i.failed("CES123");
        assert_eq!(i.attempt("CES123"), 2);
    }

    /// 建出来了就把计数清掉——下次因为换机型重建时，从最像的那个重新开始。
    #[test]
    fn a_success_forgets_the_earlier_failures() {
        let mut i = Injector::new();
        i.reconcile(&entries(&[("CES123", "A320")]));
        i.failed("CES123");
        i.reconcile(&entries(&[("CES123", "A320")]));
        i.created("CES123", 7);
        assert_eq!(i.attempt("CES123"), 0);
    }

    /// **放弃之后不再要。** 没有这一步的话，一架在这台机器上没有任何机模能顶
    /// 的飞机会每 500 ms 重试一次，永远——而它挤掉的是别的飞机的重试。
    #[test]
    fn an_abandoned_aircraft_is_not_requested_again() {
        let mut i = Injector::new();
        i.reconcile(&entries(&[("CES123", "A320")]));
        i.give_up("CES123", "A320");
        assert_eq!(i.abandoned(), 1);
        assert!(i.reconcile(&entries(&[("CES123", "A320")])).is_empty());
        assert!(i.reconcile(&entries(&[("CES123", "A320")])).is_empty());
    }

    /// 放弃是针对**那个机型**的。飞行员改了机型就是一组全新的候选，该重试。
    #[test]
    fn a_new_type_is_worth_trying_even_after_giving_up() {
        let mut i = Injector::new();
        i.reconcile(&entries(&[("CES123", "A320")]));
        i.give_up("CES123", "A320");
        assert!(i.reconcile(&entries(&[("CES123", "A320")])).is_empty());
        let actions = i.reconcile(&entries(&[("CES123", "B738")]));
        assert_eq!(actions.len(), 1, "{actions:?}");
        assert_eq!(i.attempt("CES123"), 0, "候选下标也要跟着重来");
    }

    /// 走掉再回来的从头试一遍——**上一次失败可能是因为当时模拟器正在加载**，
    /// 不是因为机模不存在。
    #[test]
    fn leaving_and_coming_back_starts_over() {
        let mut i = Injector::new();
        i.reconcile(&entries(&[("CES123", "A320")]));
        i.give_up("CES123", "A320");
        i.reconcile(&entries(&[])); // 走了
        assert_eq!(i.abandoned(), 0);
        assert_eq!(i.reconcile(&entries(&[("CES123", "A320")])).len(), 1);
    }

    /// 还没建出来就走了的，待办也要清掉——否则它的回音到了会凭空建出一架
    /// 已经不在网上的飞机。
    #[test]
    fn a_pending_aircraft_that_left_is_forgotten() {
        let mut i = Injector::new();
        i.reconcile(&entries(&[("CES123", "A320")]));
        i.reconcile(&[]);
        i.created("CES123", 42);
        assert!(i.is_empty(), "回音到得太晚，不该再建出来");
    }
}
