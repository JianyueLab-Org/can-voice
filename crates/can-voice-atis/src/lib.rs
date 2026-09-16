//! ATIS 的共用逻辑：服务端通播机队和桌面通播客户端都从这里取。
//!
//! 这个 crate 同时是一个库和一个可执行文件。**库那半边是给 `apps/atis`
//! 的 Tauri 客户端用的**——它要做的事（解析 METAR、渲染模板、把稿子念出来）
//! 和服务端机队是同一套，而同一套逻辑写两遍，迟早会在某个机场上念出两份不同的
//! 通播。
//!
//! 可执行文件那半边在 `main.rs`，它是服务端机队，只用到
//! [`datafeed`]、[`fleet`]、[`readback`]、[`station`]、[`tts`]。

//! # 谁出声：机队。已经拍板
//!
//! 服务端机队（`main.rs`）播的是 datafeed 里**每一个** `_ATIS` 席位；桌面那一支
//! （`apps/atis`）**只做稿子**——连 FSD、发文字、答查询，不出声。
//!
//! 这条线要划清楚，因为它一旦模糊，同一个频率上就会有两个声音。`can-audio`
//! 那边正是这样：`atis/broadcast.py` 开自己的 Mumble 连接，而
//! `server/ATIS/mumble.py` 又把 datafeed 里的全播一遍。它的文档从没提过这件事
//! ——那不是一条有人拍板过的设计，是没人看见过的重叠。
//!
//! 选机队的理由是它**不会睡觉**：通播要在没有人盯着的时候一直播下去，而桌面
//! 那条路系在一台会合盖、会断网、会被关掉的笔记本上。
//!
//! ## 代价：念的是电码原文
//!
//! 机队手上只有 `text_atis`，所以它念的是 [`readback`] 展开过的电码
//! （`09004MPS` → "zero niner zero zero four MPS"），而不是模板渲染出来的语音
//! 形态（"wind zero niner zero degrees four meters per second"）。后者是给人
//! 听的，前者不是。
//!
//! **模板那套 `:VOX`、[`voicefix`]、中文稿，机队一样都用不上**——它们只在本地
//! 合成的那条路上有意义。它们照样算、照样存（桌面那一支要用来给操作员看），
//! 只是没有被念出来。
//!
//! 要把这个代价去掉，得让语音形态也上线，而 `text_atis` 是飞行员要照着读的
//! 东西，往里塞语音稿会很怪。那需要 can-fsd 上一个新字段，**不是这里能单方面
//! 决定的事**，所以留着。

pub mod airports;
pub mod chinese;
pub mod datafeed;
pub mod fleet;
pub mod metar;
pub mod profile;
pub mod readback;
pub mod script;
pub mod station;
pub mod template;
pub mod tts;
pub mod voicefix;
