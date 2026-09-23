//! FSD 协议的**客户端**那一侧。
//!
//! can-voice 的三支桌面客户端都要连 can-fsd：通播席位要出现在在线列表和数据源
//! 里，飞行员客户端要把自己放上网。协议是同一套，所以它住在一个 crate 里而不是
//! 在每支客户端里各抄一遍——`can-audio` 那边就是各抄一遍，于是
//! `client/` 和 `xplane_client/` 成了两份"改一处要改两遍"的近似副本。
//!
//! 服务端那一侧在 `can-fsd`（Go）。这里只做客户端，**不解析自己不关心的包**。
//!
//! # 和语音是两条独立的链路
//!
//! 一个通播席位同时有两条连接：这条 FSD 连接让席位上网并回答文字通播查询，
//! 另一条（can-voice 的 QUIC）在同一频率上出声。两条各管各的重连，
//! 但"整个下线"要是同一个意思——见 [`client::ReconnectPolicy`]。

pub mod client;
pub mod observer_client;
pub mod packet;
pub mod pilot;
pub mod pilot_client;
pub mod session;
