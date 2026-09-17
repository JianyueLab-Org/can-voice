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

use crate::snapshot::{Ended, Snapshot};
use can_voice_client::stack::RadioStack;
use can_voice_client::{Config, Event, VoiceClient};
use can_voice_token::TokenSource;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Token(#[from] can_voice_token::Error),
    #[error("not connected")]
    NotConnected,
}

impl Error {
    /// 给人看的那一句（#29）。`Display` 是给日志的英文。
    pub fn message(&self) -> can_voice_i18n::Message {
        match self {
            Error::Token(e) => e.message(),
            Error::NotConnected => can_voice_i18n::Message::new("error.voice.not_connected"),
        }
    }
}

/// 一个应用的全部运行时状态。
///
/// 状态装在一个 `Arc` 里，因为**监护任务要能自己换票重连**：它活在
/// `tokio::spawn` 里，拿不到 `&self`。
pub struct Bridge {
    inner: Arc<Inner>,
}

struct Inner {
    client: Mutex<Option<VoiceClient>>,
    stack: Mutex<RadioStack>,
    snapshot: Mutex<Snapshot>,
    /// 重连要的两样东西。`disconnect` 会清掉它 —— 否则主动下线会被监护任务
    /// 当成掉线再连回来。
    session: Mutex<Option<(Config, TokenSource)>>,
}

impl Default for Bridge {
    fn default() -> Self {
        Self::new()
    }
}

impl Bridge {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                client: Mutex::new(None),
                stack: Mutex::new(RadioStack::new()),
                snapshot: Mutex::new(Snapshot::default()),
                session: Mutex::new(None),
            }),
        }
    }

    /// 当前状态。**前端一挂上就读它**，不要试图从事件流拼——事件是广播，
    /// 挂上之前发生的事收不到。
    ///
    /// `tx_budget` 在这里、读的这一刻才算：它一半是台面的状态，见
    /// [`Snapshot::tx_budget`]。两把锁先后拿、不嵌套。
    pub fn snapshot(&self) -> Snapshot {
        let mut snap = self
            .inner
            .snapshot
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        snap.tx_budget = snap.max_tx.map(|max_tx| match self.inner.stack.lock() {
            Ok(s) => s.tx_budget(max_tx),
            Err(p) => p.into_inner().tx_budget(max_tx),
        });
        snap
    }

    /// 连上去。票由 `TokenSource` 换，过期了会换一张再试一次。
    ///
    /// 凭据留在桥里，因为票只活 60 秒：掉线重连拿的必须是一张新票。
    pub async fn connect(&self, cfg: Config, tokens: &TokenSource) -> Result<(), Error> {
        let client = can_voice_token::connect(cfg.clone(), tokens).await?;
        if let Ok(mut s) = self.inner.session.lock() {
            *s = Some((cfg, tokens.clone()));
        }
        self.inner.adopt(client);
        Ok(())
    }

    /// 断开。
    pub async fn disconnect(&self) {
        // 先清重连依据，再关连接。反过来的话，关闭事件到达时监护任务还看得见
        // 凭据，会把一次主动下线当成掉线连回来。
        if let Ok(mut s) = self.inner.session.lock() {
            *s = None;
        }
        let client = self.inner.client.lock().ok().and_then(|mut c| c.take());
        if let Some(c) = client {
            c.shutdown().await;
        }
    }

    /// 改台面，并把闭包的答案带出来。
    ///
    /// **每次都重发一份全量声明**——没有"这次改了哪一个"的增量路径。
    ///
    /// 带返回值是因为有些改动会被台面自己拒绝（正在管的席位频率删不掉），
    /// 而调用方得知道到底删没删掉。中毒的锁照用：台面是纯数据，没有"改到一半"
    /// 的不变量，而放弃它意味着从此一个开关都动不了。
    pub fn with_stack<R>(&self, f: impl FnOnce(&mut RadioStack) -> R) -> R {
        let answer = {
            let mut s = match self.inner.stack.lock() {
                Ok(s) => s,
                Err(p) => p.into_inner(),
            };
            f(&mut s)
        };
        self.inner.push_declaration();
        answer
    }

    /// 此刻允不允许发射。**只读，不触发重新声明。**
    ///
    /// 界面每几百毫秒读一次这个值来决定 TX / XC 画不画灰；走 [`Bridge::with_stack`]
    /// 的话每一次读都会顺手推一份全量声明出去。
    pub fn transmit_allowed(&self) -> bool {
        match self.inner.stack.lock() {
            Ok(s) => s.transmit_allowed(),
            Err(p) => p.into_inner().transmit_allowed(),
        }
    }

    /// 当前台面。
    pub fn radios(&self) -> Vec<can_voice_client::stack::Radio> {
        self.inner
            .stack
            .lock()
            .map(|s| s.radios().to_vec())
            .unwrap_or_default()
    }

    /// 按下 / 松开 PTT。
    pub fn set_transmitting(&self, on: bool) {
        if let Ok(c) = self.inner.client.lock() {
            if let Some(c) = c.as_ref() {
                c.set_transmitting(on);
            }
        }
    }

    /// 换录音 / 播放设备。
    ///
    /// **两件事都要做**：转给正在跑的那条连接（立刻生效），并改掉存着的
    /// `Config`，否则下一次重连又换回旧设备。
    pub fn set_audio_devices(&self, input: Option<String>, output: Option<String>) {
        if let Ok(mut s) = self.inner.session.lock() {
            if let Some((cfg, _)) = s.as_mut() {
                cfg.input_device = input.clone();
                cfg.output_device = output.clone();
            }
        }
        if let Ok(c) = self.inner.client.lock() {
            if let Some(c) = c.as_ref() {
                c.set_audio_devices(input, output);
            }
        }
    }

    /// 某个频率的播放音量。
    pub fn set_volume(&self, freq_khz: u32, gain: f32) {
        self.with_stack(|s| s.set_gain(freq_khz, gain));
        self.push_gain(freq_khz);
    }

    /// 静音 / 取消静音某个频率。
    ///
    /// **和退订是两件事**：静音只是不播出来，包照收、灯照亮；退订（关 RX）会让
    /// 下一次有人叫你时连灯都不亮。一个临时插话的频率要的是前者。
    pub fn set_muted(&self, freq_khz: u32, on: bool) {
        self.with_stack(|s| s.set_muted(freq_khz, on));
        self.push_gain(freq_khz);
    }

    /// 把这个频率此刻**该用的**音量推给播放层。
    ///
    /// 推的是 `effective_gain` 而不是 `gain`：推 `gain` 的话静音就只是一个画在
    /// 界面上的图标——声音照出，而用户以为自己把它关掉了。
    fn push_gain(&self, freq_khz: u32) {
        let gain = self
            .radios()
            .iter()
            .find(|r| r.freq_khz == freq_khz)
            .map(|r| r.effective_gain())
            .unwrap_or(1.0);
        if let Ok(c) = self.inner.client.lock() {
            if let Some(c) = c.as_ref() {
                c.set_frequency_volume(freq_khz, gain);
            }
        }
    }
}

impl Inner {
    /// 接管一条新连接：起监护任务、存起来、把台面重发一遍。
    ///
    /// **重连之后台面要重发**，而重发就是恢复——声明是幂等的全量声明，
    /// 这正是声明式 API 换来的东西。
    fn adopt(self: &Arc<Self>, client: VoiceClient) {
        let events = client.events();
        if let Ok(mut slot) = self.client.lock() {
            *slot = Some(client);
        }
        tokio::spawn(supervise(self.clone(), events));
        self.push_declaration();
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

/// 把事件泵进快照，并在票过期时换一张再连一次。
async fn supervise(inner: Arc<Inner>, mut events: tokio::sync::broadcast::Receiver<Event>) {
    let started = std::time::Instant::now();
    loop {
        match events.recv().await {
            Ok(e) => {
                let ended = {
                    let Ok(mut s) = inner.snapshot.lock() else {
                        return;
                    };
                    s.apply(&e);
                    s.ended.clone()
                };
                let Some(ended) = ended else { continue };
                if should_renew_after(&ended, started.elapsed()) {
                    renew(inner).await;
                }
                // 无论换不换，这一条链路结束了，这个任务也该结束——
                // 新的一条由 `adopt` 起一个新的。
                return;
            }
            // 跟不上就继续：快照只关心最新的状态，丢掉的中间事件不影响它收敛。
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                tracing::debug!(skipped = n, "the ui fell behind the event stream");
            }
            Err(_) => return,
        }
    }
}

/// 换一张票再连一次。
async fn renew(inner: Arc<Inner>) {
    let Some((cfg, tokens)) = inner.session.lock().ok().and_then(|s| s.clone()) else {
        // 主动下线清掉了凭据。什么都不做才是对的。
        return;
    };
    tracing::info!("the token had expired; fetching a fresh one and reconnecting");
    match can_voice_token::connect(cfg, &tokens).await {
        Ok(client) => inner.adopt(client),
        Err(e) => tracing::warn!(error = %e, "could not reconnect with a fresh token"),
    }
}

/// 一次会话至少要活这么久，才值得为它换票重连。
///
/// 比票的寿命（60 秒）短一半：一条活过半分钟的会话是真的在用，
/// 而"连上就掉"说明问题不在票上。
const MIN_SESSION_BEFORE_RENEWAL: Duration = Duration::from_secs(30);

/// 这次掉线该不该换一张票再连一次。
///
/// **换票是这一层的事，不是核心库的。** 核心库刻意不碰凭据，所以它撞上
/// `token_expired` 只能进 Offline，并在日志里说"由上层换一张再 connect 一次"。
/// 在这个函数存在之前，那个上层不存在：can-api 签的是 60 秒的票，于是在线超过
/// 一分钟之后掉一次线，重连握手拿的还是同一张过期票，用户只能重新手打密码。
pub fn should_renew_after(ended: &Ended, session_lasted: Duration) -> bool {
    matches!(ended, Ended::Refused(r) if r.is_recoverable())
        && session_lasted >= MIN_SESSION_BEFORE_RENEWAL
}

#[cfg(test)]
mod tests {
    use super::*;
    use can_voice_client::conn::RefusedReason;
    use can_voice_client::session::Limits;

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

    // ——— 掉线之后该不该换票重连 ———

    /// **票过期是上层唯一能救的掉线。** 核心库拿不到新票，所以它只能进 Offline；
    /// 换一张再连一次是桥的事，而桥恰恰没做，于是在线超过票的寿命之后
    /// 掉一次线就要用户重新手打密码。
    #[test]
    fn an_expired_token_after_a_working_session_is_worth_a_fresh_one() {
        assert!(should_renew_after(
            &Ended::Refused(RefusedReason::TokenExpired),
            Duration::from_secs(600),
        ));
    }

    /// 其余几种换多少张票都一样：顶号要换的是别处那个人，版本不对要换的是客户端。
    #[test]
    fn the_other_endings_are_not_a_token_problem() {
        for e in [
            Ended::Offline,
            Ended::Evicted,
            Ended::Refused(RefusedReason::TokenInvalid),
            Ended::Refused(RefusedReason::ProtoUnsupported),
            Ended::Refused(RefusedReason::Refused),
            Ended::Refused(RefusedReason::Other("nope".into())),
        ] {
            assert!(
                !should_renew_after(&e, Duration::from_secs(600)),
                "{e:?} 不该换票重连"
            );
        }
    }

    /// **刚连上就过期，问题在时钟上，不在票上。** 再换只是把同一件事重演，
    /// 而换票走 can-api 的鉴权路由——它和 FSD 登录共用同一个按 CAN ID 的限流桶，
    /// 循环换票会把这个账号连 FSD 一起锁出去。
    #[test]
    fn an_immediate_second_expiry_is_a_clock_problem() {
        assert!(!should_renew_after(
            &Ended::Refused(RefusedReason::TokenExpired),
            Duration::from_secs(2),
        ));
    }

    /// **界面读到的快照里带着台面对发射上限的处境**，而且是读的那一刻的台面。
    ///
    /// 存一份副本的话，开关一动它就过期，而界面照着一份过期的处境提示"会超额"
    /// 或者该提示时不提示。
    #[test]
    fn the_snapshot_carries_the_tx_budget_of_the_stack_as_it_is_now() {
        let bridge = Bridge::new();
        bridge.with_stack(|s| {
            s.add(118_000);
            s.add(121_800);
            s.set_tx(118_000, true);
        });
        // 没连上就没有上限，也就没有处境可说：界面什么都不显示。
        assert_eq!(bridge.snapshot().tx_budget, None);

        bridge
            .inner
            .snapshot
            .lock()
            .expect("snapshot")
            .apply(&Event::Limits(Limits {
                max_tx: 1,
                max_rx: 32,
            }));
        let v = serde_json::to_value(bridge.snapshot()).expect("serialize");
        // 这是界面照着读的形状。
        assert_eq!(v["max_tx"], serde_json::json!(1));
        assert_eq!(
            v["tx_budget"],
            serde_json::json!({
                "max_tx": 1,
                "declared": 1,
                "tx_over": [121_800],
                "xc_over": [121_800],
            })
        );

        bridge.with_stack(|s| s.set_tx(118_000, false));
        let b = bridge
            .snapshot()
            .tx_budget
            .expect("the limit is still known");
        assert_eq!(b.declared, 0);
        assert!(b.tx_over.is_empty(), "{b:?}");
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
