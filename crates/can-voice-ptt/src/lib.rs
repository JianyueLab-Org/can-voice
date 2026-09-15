//! PTT：按下任意一个绑定即发话。
//!
//! 规则是从 `can-audio/controller/ptt.py` 搬过来的，**逐条都是踩出来的**，
//! 不要重新发明。每一条为什么存在写在对应的测试上。

pub mod binding;
pub mod state;
pub mod watcher;

pub use binding::{mouse_supported, Binding, MouseButton};
pub use state::{DeviceReporter, PttState, Router, Sources};
pub use watcher::PttWatcher;
