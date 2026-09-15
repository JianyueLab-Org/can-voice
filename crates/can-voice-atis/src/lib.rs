//! ATIS 的共用逻辑：服务端通播机队和桌面通播客户端都从这里取。
//!
//! 这个 crate 同时是一个库和一个可执行文件。**库那半边是给 `apps/atis`
//! 的 Tauri 客户端用的**——它要做的事（解析 METAR、渲染模板、把稿子念出来）
//! 和服务端机队是同一套，而同一套逻辑写两遍，迟早会在某个机场上念出两份不同的
//! 通播。
//!
//! 可执行文件那半边在 `main.rs`，它是服务端机队，只用到
//! [`datafeed`]、[`fleet`]、[`readback`]、[`station`]、[`tts`]。

//! # 一个还没有定的问题：同一个席位会不会被播两遍
//!
//! 服务端机队（`main.rs`）播的是 datafeed 里**每一个** `_ATIS` 席位。而一个席位
//! 之所以出现在 datafeed 里，正是因为有人开着桌面通播客户端把它挂上了 FSD。
//! 所以只要桌面那一支自己也出声，同一个频率上就有两个声音。
//!
//! `can-audio` 那边两边都出声（`atis/broadcast.py` 开自己的 Mumble 连接，
//! `server/ATIS/mumble.py` 又把 datafeed 里的全播一遍），而它的文档从没提过
//! 这件事——所以这不是一条有人拍板过的设计，是没人看见过的重叠。
//!
//! **这一侧的桌面客户端按"只做稿子"写**：连 FSD、发文字、答查询，声音归机队。
//! 理由是机队跑在集群里而一台笔记本会睡觉、会掉线，而两个声音一定比一个差。
//! 这一条要是改了，改的是 `apps/atis`，不是这里。

pub mod airports;
pub mod chinese;
pub mod datafeed;
pub mod fleet;
pub mod metar;
pub mod profile;
pub mod readback;
pub mod station;
pub mod template;
pub mod tts;
pub mod voicefix;
