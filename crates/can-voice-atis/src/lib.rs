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
//! 而且两个声音**念的还不是同一份稿子**，这一点更要紧：机队手上只有
//! `text_atis`，它念的是 [`readback`] 展开过的**电码原文**
//! （`09004MPS` → "zero niner zero zero four MPS"）；桌面那一支念的是模板渲染
//! 出来的语音形态（"wind zero niner zero degrees four meters per second"）。
//! 后者是给人听的，前者不是。**模板那套 `:VOX`、`voicefix`、中文稿，机队一样
//! 都用不上**——它们只在本地合成的那条路上有意义。
//!
//! 所以"谁出声"不是一个可以顺手定的实现细节，它决定全网通播听起来是哪一种。
//! 三条路各有代价：桌面出声（好听，但笔记本会睡觉，且要有办法让机队让开）、
//! 机队出声（稳，但念的是电码）、或者把语音稿也放上线（`text_atis` 是飞行员
//! 要读的东西，塞语音稿进去会很怪）。
//!
//! **这一条留给人拍板。** 在此之前 `apps/atis` 只做稿子：连 FSD、发文字、
//! 答查询。渲染出来的语音形态照样算、照样存，所以哪条路都不必重写。

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
