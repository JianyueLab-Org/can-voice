//! ATIS 的共用逻辑：服务端通播机队和桌面通播客户端都从这里取。
//!
//! 这个 crate 同时是一个库和一个可执行文件。**库那半边是给 `apps/atis`
//! 的 Tauri 客户端用的**——它要做的事（解析 METAR、渲染模板、把稿子念出来）
//! 和服务端机队是同一套，而同一套逻辑写两遍，迟早会在某个机场上念出两份不同的
//! 通播。
//!
//! 可执行文件那半边在 `main.rs`，它是服务端机队，只用到
//! [`datafeed`]、[`fleet`]、[`readback`]、[`station`]、[`tts`]。

pub mod chinese;
pub mod datafeed;
pub mod fleet;
pub mod metar;
pub mod readback;
pub mod station;
pub mod template;
pub mod tts;
pub mod voicefix;
