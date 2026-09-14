//! 后台任务：把控制面、订阅状态机与事件流接起来。Task 11 填完整。

use crate::client::{Command, Config, Event};
use crate::conn::Link;

/// 后台任务的骨架。完整的收发与重连在 Task 11。
pub(crate) async fn run(
    _cfg: Config,
    _link: Link,
    _events: tokio::sync::broadcast::Sender<Event>,
    mut cmds: tokio::sync::mpsc::UnboundedReceiver<Command>,
) {
    while let Some(cmd) = cmds.recv().await {
        match cmd {
            Command::Declare(sub) => {
                tracing::debug!(rx = sub.rx.len(), tx = sub.tx.len(), xc = sub.xc.len(), "declaration queued")
            }
            Command::Transmit(on) => tracing::debug!(on, "ptt"),
            Command::Volume { freq_khz, gain } => tracing::debug!(freq_khz, gain, "volume"),
            Command::Shutdown => return,
        }
    }
}
