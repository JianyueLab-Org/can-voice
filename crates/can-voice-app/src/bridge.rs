//! 四个 Tauri 应用共用的命令面。
//!
//! Rust 侧持 [`can_voice_client::VoiceClient`] 和一份 [`Snapshot`]，把事件流泵进
//! 快照；前端只做两件事：发命令、读快照。
//!
//! # 前端能做的事，就是核心库公开的那几件，不多一件
//!
//! 这一层**不得**引入 `join` / `leave` / `channel_id`。核心库有一条扫源码的测试
//! 钉住这件事，这里有第二条——桥是离用户最近的一层，"声明式"在这里被绕开的话，
//! 前面所有的防守都白做。
//!
//! # 耦合规则只有一份实现
//!
//! 台面（[`RadioStack`]）折算成声明，桥不自己拼订阅。前端改开关、桥问台面要
//! 一份全量声明、发出去——三条耦合规则（关 RX 清 TX/XC、开 TX 强制 RX、
//! 开 XC 强制 RX+TX）在核心库里只写了一遍。

use crate::snapshot::Snapshot;
use can_voice_client::stack::RadioStack;
use can_voice_client::{Config, Event, VoiceClient};
use can_voice_token::TokenSource;
use std::sync::{Arc, Mutex};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Token(#[from] can_voice_token::Error),
    #[error("not connected")]
    NotConnected,
}

/// 一个应用的全部运行时状态。
pub struct Bridge {
    client: Mutex<Option<VoiceClient>>,
    stack: Mutex<RadioStack>,
    snapshot: Arc<Mutex<Snapshot>>,
}

impl Default for Bridge {
    fn default() -> Self {
        Self::new()
    }
}

impl Bridge {
    pub fn new() -> Self {
        Self {
            client: Mutex::new(None),
            stack: Mutex::new(RadioStack::new()),
            snapshot: Arc::new(Mutex::new(Snapshot::default())),
        }
    }

    /// 当前状态。**前端一挂上就读它**，不要试图从事件流拼——事件是广播，
    /// 挂上之前发生的事收不到。
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// 连上去。票由 `TokenSource` 换，过期了会换一张再试一次。
    pub async fn connect(&self, cfg: Config, tokens: &TokenSource) -> Result<(), Error> {
        let client = can_voice_token::connect(cfg, tokens).await?;
        let events = client.events();
        let snapshot = self.snapshot.clone();
        tokio::spawn(pump_into_snapshot(events, snapshot));

        if let Ok(mut slot) = self.client.lock() {
            *slot = Some(client);
        }
        self.push_declaration();
        Ok(())
    }

    /// 断开。
    pub async fn disconnect(&self) {
        let client = self.client.lock().ok().and_then(|mut c| c.take());
        if let Some(c) = client {
            c.shutdown().await;
        }
    }

    /// 改台面。**每次都重发一份全量声明**——没有"这次改了哪一个"的增量路径。
    pub fn with_stack(&self, f: impl FnOnce(&mut RadioStack)) {
        if let Ok(mut s) = self.stack.lock() {
            f(&mut s);
        }
        self.push_declaration();
    }

    /// 当前台面。
    pub fn radios(&self) -> Vec<can_voice_client::stack::Radio> {
        self.stack
            .lock()
            .map(|s| s.radios().to_vec())
            .unwrap_or_default()
    }

    /// 按下 / 松开 PTT。
    pub fn set_transmitting(&self, on: bool) {
        if let Ok(c) = self.client.lock() {
            if let Some(c) = c.as_ref() {
                c.set_transmitting(on);
            }
        }
    }

    /// 某个频率的播放音量。
    pub fn set_volume(&self, freq_khz: u32, gain: f32) {
        self.with_stack(|s| s.set_gain(freq_khz, gain));
        if let Ok(c) = self.client.lock() {
            if let Some(c) = c.as_ref() {
                c.set_frequency_volume(freq_khz, gain);
            }
        }
    }

    /// 把台面折算成声明发出去。夹掉的耦合对返回给调用方显示。
    fn push_declaration(&self) {
        let Ok(stack) = self.stack.lock() else { return };
        let declaration = stack.to_subscription();
        if !declaration.dropped_xc.is_empty() {
            tracing::warn!(
                dropped = declaration.dropped_xc.len(),
                "cross-couple pairs beyond the server limit were not declared"
            );
        }
        if let Ok(c) = self.client.lock() {
            if let Some(c) = c.as_ref() {
                c.set_subscription(declaration.sub);
            }
        }
    }
}

async fn pump_into_snapshot(
    mut events: tokio::sync::broadcast::Receiver<Event>,
    snapshot: Arc<Mutex<Snapshot>>,
) {
    loop {
        match events.recv().await {
            Ok(e) => {
                if let Ok(mut s) = snapshot.lock() {
                    s.apply(&e);
                }
            }
            // 跟不上就继续：快照只关心最新的状态，丢掉的中间事件不影响它收敛。
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::debug!(skipped = n, "the ui fell behind the event stream");
            }
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **桥这一层不得引入 `join` / `leave` / `channel_id`。**
    ///
    /// 核心库有一条扫全部模块文件的同款测试。这里必须有第二条：桥是离用户最近的
    /// 一层，"声明式"这条设计在这里被绕开的话，前面所有的防守都白做——而那正是
    /// 旧实现里"UI 是绿的但人还在 root 频道"那一整类 bug 的藏身处。
    #[test]
    fn the_bridge_has_no_imperative_channel_verbs() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut scanned = 0usize;
        for entry in std::fs::read_dir(&root).expect("read_dir") {
            let path = entry.expect("entry").path();
            if !matches!(path.extension().and_then(|e| e.to_str()), Some("rs")) {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("read");
            let mut in_tests = false;
            for line in src.lines() {
                if line.trim_start().starts_with("mod tests") {
                    in_tests = true;
                }
                if in_tests {
                    continue;
                }
                let t = line.trim_start();
                if !t.starts_with("pub fn") && !t.starts_with("pub async fn") {
                    continue;
                }
                scanned += 1;
                for banned in ["join", "leave", "channel_id", "current_channel"] {
                    assert!(
                        !t.to_lowercase().contains(banned),
                        "{}: {banned:?} — the bridge is the layer closest to the user; \
                         a declarative core with an imperative bridge defends nothing: {t}",
                        path.display()
                    );
                }
            }
        }
        assert!(
            scanned > 5,
            "only {scanned} public fns scanned; the walk is probably broken"
        );
    }

    /// 台面直接折算成声明，桥不自己拼订阅——耦合规则只有一份实现。
    #[test]
    fn the_declaration_comes_straight_from_the_stack() {
        let mut stack = RadioStack::new();
        stack.add(118_000);
        stack.add(121_800);
        stack.set_tx(121_800, true);

        let d = stack.to_subscription();
        assert_eq!(d.sub.rx, vec![118_000, 121_800]);
        assert_eq!(d.sub.tx, vec![121_800]);
    }

    /// 夹掉的耦合对要能报给界面——服务端第 129 对往后是静默丢弃的。
    #[test]
    fn clamped_cross_couple_pairs_are_visible_to_the_ui() {
        let mut stack = RadioStack::new();
        let freqs: Vec<u32> = (0..17).map(|i| 118_000 + i * 25).collect();
        for f in &freqs {
            stack.add(*f);
            stack.set_xc(*f, true);
        }
        assert!(!stack.to_subscription().dropped_xc.is_empty());
    }
}
