//! can-voice 客户端核心：四个 Tauri 应用与服务端 ATIS 机器人共用。
//!
//! 公开 API 是**声明式**的：调用方声明"我要收哪些频率、发哪些频率"，
//! 库负责让服务端状态收敛过去，重连后自动重发。没有 join/leave，
//! 没有 channel id，没有任何"需要记住"的连接状态 —— 那正是旧实现里
//! 一整类"UI 是绿的但人还在 root 频道"的 bug 的根源。

pub mod rx;
pub mod session;
pub mod stack;
pub mod tx;
