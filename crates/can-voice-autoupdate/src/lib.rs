//! 启动时的自动更新。
//!
//! 两条不可让的规矩：
//!
//! - **只在启动时。** 检查发生在用户连上语音之前；一个开着八小时的管制端不会
//!   在第七个小时突然决定重启自己。
//! - **失败要安静，而且绝不挡路。** 每一条错误路径最后都把状态置成 `Done`
//!   然后让界面照常进去。更新服务挂掉不该让全网上不了线。

use serde::Serialize;
use std::sync::{Mutex, PoisonError};
use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::UpdaterExt;

/// 界面轮询的就是这个状态。**每一条路径最后都要置成 `Done`**——界面在
/// 读到它之前是挡着的。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "lowercase")]
pub enum State {
    Checking,
    Downloading { received: u64, total: u64 },
    Installing,
    Done,
}

/// 当前状态。一个进程只有一个更新器，所以用进程全局，不用 Tauri 的 managed
/// state。
///
/// **初值是 `Done`，这一点是承重的。** 如果 [`start`] 根本没被调用——某个应用
/// 没接更新器，或者 `.setup()` 在调到它之前就返回了——界面必须立刻放行，而不是
/// 挂在那里等一个永远不会来的状态。
static STATE: Mutex<State> = Mutex::new(State::Done);

/// 界面读的就是这个。配一条 `#[tauri::command]` 转发出去。
pub fn state() -> State {
    STATE.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

fn set(state: State) {
    *STATE.lock().unwrap_or_else(PoisonError::into_inner) = state;
}

/// 插件本体。端点和公钥在 `tauri.conf.json` 的 `plugins.updater` 里。
pub fn plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R, tauri_plugin_updater::Config> {
    tauri_plugin_updater::Builder::new().build()
}

/// `tauri.conf.json` 里到底有没有 `plugins.updater`。
///
/// **注册插件之前必须问一次。** 插件的配置里 `pubkey` 是必填的，而 tauri 在
/// 配置缺席时喂给它的是 `null`：反序列化失败 → 插件初始化失败 → `run()` 直接
/// 报错，应用一个窗口都不会出来。同一个判据在 [`run`] 里再用一次，因为
/// `handle.updater()` 取的是插件注册时 manage 的那份状态，没注册就是 panic
/// 而不是 `Err`。
///
/// 公钥生成、配置写进四份 `tauri.conf.json` 之后，这个判据恒为真。
pub fn configured(config: &tauri::Config) -> bool {
    config.plugins.0.contains_key("updater")
}

/// 当前这份二进制是哪种包。
///
/// 读的是 `tauri_utils::platform::bundle_type()`——**打包时写进二进制的一个静态
/// 字符串**，不是环境变量。插件的 `install_inner` 读的是同一个函数，所以两边
/// 不会对「这是不是 AppImage」产生分歧。
pub fn bundle_name() -> Option<&'static str> {
    use tauri::utils::config::BundleType;
    match tauri::utils::platform::bundle_type()? {
        BundleType::Deb => Some("deb"),
        BundleType::Rpm => Some("rpm"),
        BundleType::AppImage => Some("appimage"),
        BundleType::Msi => Some("msi"),
        BundleType::Nsis => Some("nsis"),
        BundleType::App => Some("app"),
        _ => None,
    }
}

/// 在 `.setup()` 里调一次。立刻返回，活在后台跑。
pub fn start<R: Runtime>(handle: &AppHandle<R>) {
    // **先置 `Checking`，再 spawn。** 初值是 `Done`，如果留给后台任务去置，
    // 一个抢先轮询到的界面会读到上一行留下的 `Done` 然后提前放行。
    set(State::Checking);

    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        run(&handle).await;
        // 走到哪一步都要放行。
        set(State::Done);
    });
}

async fn run<R: Runtime>(handle: &AppHandle<R>) {
    if !can_voice_update::self_replaceable(bundle_name()) {
        tracing::info!(bundle = ?bundle_name(), "auto-update: this bundle does not replace itself");
        return;
    }

    if !configured(handle.config()) {
        tracing::info!("auto-update: plugins.updater is not configured");
        return;
    }

    set(State::Checking);

    let updater = match handle.updater() {
        Ok(updater) => updater,
        Err(err) => {
            tracing::info!(%err, "auto-update: the updater is not configured");
            return;
        }
    };

    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => {
            tracing::info!("auto-update: already current");
            return;
        }
        Err(err) => {
            tracing::info!(%err, "auto-update: the check failed");
            return;
        }
    };

    tracing::info!(version = %update.version, "auto-update: installing");

    let mut received: u64 = 0;

    // download_and_install 在 Windows 上装完直接 exit(0)，由安装器把新版本拉
    // 起来；Linux 上它会返回，要自己重启。
    let outcome = update
        .download_and_install(
            move |chunk, total| {
                received += chunk as u64;
                set(State::Downloading {
                    received,
                    total: total.unwrap_or(0),
                });
            },
            || set(State::Installing),
        )
        .await;

    match outcome {
        Ok(()) => {
            tracing::info!("auto-update: installed, restarting");
            handle.restart();
        }
        Err(err) => tracing::info!(%err, "auto-update: install failed"),
    }
}
