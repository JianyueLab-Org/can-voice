//! can-voice 线协议。
//!
//! 这是一个跨实现契约：Go 侧（can-voice 服务端）有一份独立实现，
//! 两边都测 `server/testdata/wire-golden.json`。

pub mod control;
pub mod wire;
