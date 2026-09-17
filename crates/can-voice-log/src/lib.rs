//! 日志：落盘、滚动、panic 留痕、回传。
//!
//! # 为什么一定要落盘
//!
//! 打包出来的是一个没有控制台的 GUI 进程（Windows 上尤其如此），`stdout` 写到
//! 哪里谁也不知道。用户报"连不上"的时候，手里必须有一份能发出来的东西，
//! 否则每一次排查都是"你再试一次，这次描述得细一点"。
//!
//! 规格照 `can-audio/*/applog.py` 来：**4 份 × 1 MiB**。一行大约 80 字节，
//! 1 MiB 是一万三千行；四份加起来 4 MiB，用户发得动，而覆盖的时长够长。
//!
//! # 写不进去不是错误
//!
//! 装在 `Program Files` 下、或者配置目录不可写时，日志文件建不起来。那时候
//! 程序照常跑，只是没有文件——**为了记日志而起不来的客户端**是这个模块的反面。
//!
//! # panic 只要一个钩子
//!
//! Python 版要装两个（`sys.excepthook` 加 `threading.excepthook`），因为线程里的
//! 异常走的是另一条路。Rust 的 `panic::set_hook` 是**进程级**的，任何线程里的
//! panic 都从它过，所以一个就够——包括 cpal 的音频线程和 tokio 的工作线程。

use can_voice_i18n::Message;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// 单份上限：1 MiB。见模块文档。
pub const MAX_BYTES: u64 = 1024 * 1024;

/// 除当前这份之外再留几份。
pub const BACKUPS: usize = 3;

/// 回传时最多带多少字节。
///
/// 和 can-api 那边的 `maxLogBytes` 一样是 1 MiB：它只保留末尾这么多，
/// 多发的部分在那边会被丢掉，而请求体本身有 4 MiB 的硬上限。
pub const MAX_UPLOAD_BYTES: usize = 1024 * 1024;

/// 当前这份日志的路径。没能落盘时是 `None`。
static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 装好日志：文件 + 控制台 + panic 钩子。返回日志文件路径。
///
/// **写不进去不是错误**：装在 `Program Files` 下、或者配置目录不可写的时候
/// 返回 `None`，程序照常跑，只是这一次没有文件。为了记日志而起不来的客户端
/// 是这个模块的反面。
///
/// `product` 是四个产品名之一（`audio-for-can` 等），它同时是目录名和文件名。
/// 具体落在哪见 [`log_path`]。
///
/// `version` 由调用方传 `env!("CARGO_PKG_VERSION")`。**不能在这里自己取**：在这个
/// crate 里展开的是 can-voice-log 自己的版本（workspace 的 0.1.0），于是每一份
/// 日志头都写着 0.1.0，和 [`upload`] 报的、更新检查比的那个版本对不上。
pub fn init(product: &str, version: &str, debug: bool) -> Option<PathBuf> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::EnvFilter;

    let default_level = if debug { "debug" } else { "info" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));

    let file = log_path(product).and_then(|p| {
        match RotatingWriter::open(&p, MAX_BYTES, BACKUPS) {
            Ok(w) => Some((p, Shared(Arc::new(Mutex::new(w))))),
            Err(e) => {
                // stderr 在打包后的 Windows GUI 进程里多半没人看得见，
                // 但这是这条路径上唯一还剩下的出口。
                eprintln!("cannot write the log file: {e}");
                None
            }
        }
    });
    let path = file.as_ref().map(|(p, _)| p.clone());
    let _ = PATH.set(path.clone());

    let console = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    // `try_init` 而不是 `init`：后者在已经装过订阅器时**直接 panic**，
    // 于是一个重复调用把"日志装不上"变成了"程序起不来"。
    let installed = match file {
        Some((_, writer)) => tracing_subscriber::registry()
            .with(filter)
            .with(console)
            // 文件里不要 ANSI 转义：那些 `\x1b[2m` 会让日志在记事本里没法读。
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(writer),
            )
            .try_init(),
        None => tracing_subscriber::registry()
            .with(filter)
            .with(console)
            .try_init(),
    };
    if let Err(e) = installed {
        eprintln!("the log subscriber was already installed: {e}");
    }

    install_panic_hook();

    // 一行环境。用户发上来的日志十有八九缺的就是这些：什么系统、哪一版、
    // 日志写在哪。问一遍要一个来回，写进去零成本。
    tracing::info!(
        product,
        version,
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
        log = %path.as_deref().map(|p| p.display().to_string()).unwrap_or_else(|| "(none)".into()),
        "starting"
    );
    path
}

/// 当前这份日志在哪。[`init`] 之前、或者落不了盘时是 `None`。
///
/// 界面上的"打开日志"和回传都读它。
pub fn path() -> Option<PathBuf> {
    PATH.get().cloned().flatten()
}

/// 把未捕获的 panic 写进日志。
///
/// **一个钩子就够**：`panic::set_hook` 是进程级的，cpal 的音频线程、tokio 的
/// 工作线程、Tauri 的主线程都从它过。Python 版要装两个是因为那边线程走另一条路。
///
/// 原来的钩子照旧调用：它负责把那段熟悉的 `thread '…' panicked at …` 打到
/// stderr，而从源码跑的人正看着那个。
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        tracing::error!(
            location = location.unwrap_or_else(|| "?".into()),
            thread = std::thread::current().name().unwrap_or("?"),
            // 强制抓栈：默认要 RUST_BACKTRACE 才有，而用户机器上没人设那个，
            // 于是崩溃报告里最有用的那一段永远是空的。
            backtrace = %std::backtrace::Backtrace::force_capture(),
            "a panic went uncaught: {}",
            panic_text(info.payload())
        );
        previous(info);
    }));
}

/// 日志文件放哪：各平台放日志的那个老地方，文件名是产品名。
///
/// - Windows：`%APPDATA%\<产品名>\<产品名>.log`（和设置同一个目录）
/// - macOS：`~/Library/Logs/<产品名>/`
/// - Linux：`$XDG_STATE_HOME`（默认 `~/.local/state`）`/<产品名>/`
///
/// **不写当前目录。** `can-audio` 的 Python 版写的是相对路径，于是客户端必须在
/// 自己那个目录里跑；一个从开始菜单启动的程序当前目录是什么全看是谁启动的，
/// 结果日志散落在各处，而用户被问到"日志在哪"时只能猜。
fn log_path(product: &str) -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_STATE_HOME").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("HOME").map(|h| {
                let h = PathBuf::from(h);
                if cfg!(target_os = "macos") {
                    h.join("Library").join("Logs")
                } else {
                    h.join(".local").join("state")
                }
            })
        })?;
    Some(base.join(product).join(format!("{product}.log")))
}

/// 可克隆的写入端。`tracing-subscriber` 的每一层都要一个自己的 writer，
/// 而滚动状态只能有一份——那是"什么时候滚"的唯一真相。
#[derive(Clone)]
struct Shared(Arc<Mutex<RotatingWriter>>);

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.0.lock() {
            Ok(mut w) => w.write(buf),
            // 写日志的那个线程 panic 了。**不要再 panic 一次**：
            // 那会把一次崩溃变成两次，而第二次没有任何信息。
            Err(p) => p.into_inner().write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self.0.lock() {
            Ok(mut w) => w.flush(),
            Err(p) => p.into_inner().flush(),
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Shared {
    type Writer = Shared;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// 一个按大小滚动的日志文件。
///
/// 自己写而不是用 `tracing-appender`：那个只按时间滚（天/小时），而这里要的是
/// **按大小**——一个开了一整天没说过话的客户端和一个开了十分钟的繁忙席位，
/// 写出来的量差一个数量级，按时间滚的结果是要么留不住现场、要么几百兆。
pub struct RotatingWriter {
    path: PathBuf,
    max_bytes: u64,
    backups: usize,
    file: std::fs::File,
    written: u64,
}

impl RotatingWriter {
    pub fn open(path: &Path, max_bytes: u64, backups: usize) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        // 接着上次那份写，所以起点是它现在的大小——从 0 开始算的话，
        // 一个反复重启的客户端永远滚不动，文件一直长。
        let written = file.seek(std::io::SeekFrom::End(0))?;
        Ok(Self {
            path: path.to_path_buf(),
            max_bytes,
            backups,
            file,
            written,
        })
    }

    /// `app.log` → `app.log.1`，`app.log.1` → `app.log.2`，最老的那份丢掉。
    fn rotate(&mut self) -> std::io::Result<()> {
        let _ = self.file.flush();
        for i in (1..=self.backups).rev() {
            let from = backup_path(&self.path, i - 1);
            let to = backup_path(&self.path, i);
            if from.exists() {
                // 最老的那份被下一份盖掉，不用先删。
                let _ = std::fs::rename(&from, &to);
            }
        }
        self.file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&self.path)?;
        self.written = 0;
        Ok(())
    }
}

/// 第 `n` 份的名字；0 是当前这份。
fn backup_path(path: &Path, n: usize) -> PathBuf {
    if n == 0 {
        return path.to_path_buf();
    }
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{n}"));
    PathBuf::from(name)
}

impl Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // **整条写完再滚**，不在一行中间滚：半行日志两头都读不懂。
        if self.written + buf.len() as u64 > self.max_bytes && self.written > 0 {
            self.rotate()?;
        }
        let n = self.file.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// 回传要等多久。比查更新宽：一份日志可以有一兆。
const UPLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// 把当前这份日志寄回去。
///
/// **认的是 CAN 号加网络密码**，不是会话：桌面端手里只有这一对（can-api 的
/// `handleLogUpload` 也是这么判的，而且刻意不看 rating——客户端坏掉的未定级成员
/// 正是最需要寄日志的人）。密码用完就丢，不存。
pub async fn upload(
    http: &reqwest::Client,
    api_origin: &str,
    product: &str,
    version: &str,
    cid: &str,
    password: &str,
) -> Result<(), Message> {
    let path = path().ok_or_else(|| Message::new("error.log.no_file"))?;
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| Message::new("error.log.unreadable").with("detail", e))?;
    upload_text(
        http,
        api_origin,
        product,
        version,
        cid,
        password,
        tail(&raw, MAX_UPLOAD_BYTES),
    )
    .await
}

/// 自带 HTTP 客户端的版本，给手上没有现成 client 的调用方用。
///
/// 通播制作端就是这一种：它本来不需要 reqwest。理由同
/// `can_voice_update::check_once`。
pub async fn upload_once(
    api_origin: &str,
    product: &str,
    version: &str,
    cid: &str,
    password: &str,
) -> Result<(), Message> {
    upload(
        &reqwest::Client::new(),
        api_origin,
        product,
        version,
        cid,
        password,
    )
    .await
}

/// [`upload`] 除去"从哪读"之外的那一半。
#[allow(clippy::too_many_arguments)]
async fn upload_text(
    http: &reqwest::Client,
    api_origin: &str,
    product: &str,
    version: &str,
    cid: &str,
    password: &str,
    log: &str,
) -> Result<(), Message> {
    let url = format!("{}/api/v1/logs", api_origin.trim_end_matches('/'));
    let resp = http
        .post(&url)
        .json(&serde_json::json!({
            "cid": cid,
            "password": password,
            "client": product,
            "version": version,
            "log": log,
        }))
        .timeout(UPLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "log upload could not reach can-api");
            Message::new("error.log.unreachable")
        })?;

    match resp.status() {
        s if s.is_success() => Ok(()),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            Err(Message::new("error.log.credentials"))
        }
        reqwest::StatusCode::TOO_MANY_REQUESTS => Err(Message::new("error.log.rate_limited")),
        reqwest::StatusCode::SERVICE_UNAVAILABLE => {
            // can-api 的 `LogUploadMailTo` 没配的时候就是这一条。告诉用户
            // "服务端没开这个功能"，比让他反复重试强。
            Err(Message::new("error.log.disabled"))
        }
        other => Err(Message::new("error.log.rejected").with("status", other.as_u16())),
    }
}

/// 留末尾 `max` 字节，**不切断字符**。
///
/// 留末尾是因为故障发生在最后，can-api 那边也是这么截的。切在多字节字符中间
/// 会得到一个不是 UTF-8 的串，而这份日志里全是中文。
fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

/// 从 panic 的载荷里取出那句话。
///
/// 三种形状都要认：`panic!("literal")` 给的是 `&str`，`panic!("{x}")` 给的是
/// `String`，而 `panic_any` 什么都可能给。取不出来时留一句能看的话——
/// 空串会让日志里出现一条"什么都没说"的 CRITICAL。
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    "a panic payload that is neither &str nor String".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 临时目录放在 `target/` 下面，不用系统 temp：它是这个仓库自己的构建产物
    /// 目录，已经被忽略，而且跟着 `cargo clean` 一起走。
    fn scratch(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-logs")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    // ——— 滚动 ———

    /// 满了就滚，而且**只留这么多份**。
    ///
    /// 不滚的话一个开着一整天的客户端会写出一个几百兆的文件，用户根本发不出来；
    /// 不删旧的话磁盘只涨不落。
    #[test]
    fn a_full_file_rotates_and_keeps_only_the_backups() {
        let dir = scratch("rotate");
        let path = dir.join("app.log");
        let mut w = RotatingWriter::open(&path, 100, 2).expect("open");

        for _ in 0..40 {
            use std::io::Write;
            w.write_all(b"0123456789\n").expect("write");
        }
        drop(w);

        assert!(path.exists(), "the current file must exist");
        assert!(path.with_extension("log.1").exists(), "one backup");
        assert!(path.with_extension("log.2").exists(), "two backups");
        assert!(
            !path.with_extension("log.3").exists(),
            "and no more than the backup count"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 滚过之后写的还是**同一个名字**的那份文件——"当前日志"这个路径不能变，
    /// 界面上的"打开日志"和回传都指着它。
    #[test]
    fn writing_continues_in_the_current_file_after_a_rotation() {
        let dir = scratch("rotate-current");
        let path = dir.join("app.log");
        let mut w = RotatingWriter::open(&path, 20, 1).expect("open");
        {
            use std::io::Write;
            w.write_all(b"aaaaaaaaaaaaaaaaaaaaaa\n").expect("write");
            w.write_all(b"newest\n").expect("write");
        }
        drop(w);

        let current = std::fs::read_to_string(&path).expect("read");
        assert!(current.contains("newest"), "got {current:?}");
        assert!(
            !current.contains("aaaa"),
            "the old lines moved out: {current:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ——— 回传时取的是尾巴 ———

    /// **留末尾，不留开头。** 故障发生在最后，而 can-api 那边也只保留末尾
    /// 1 MiB；从头截等于把最有用的那一段扔掉。
    #[test]
    fn the_tail_keeps_the_end_not_the_beginning() {
        assert_eq!(tail("0123456789", 4), "6789");
        assert_eq!(tail("short", 100), "short");
    }

    /// 切在字符中间会切出一个不是 UTF-8 的串，而日志里全是中文。
    #[test]
    fn the_tail_never_splits_a_character() {
        // 每个汉字 3 字节，上限给 4 —— 只能装下一个。
        assert_eq!(tail("甲乙丙", 4), "丙");
        assert!(tail("甲乙丙", 2).is_empty());
    }

    // ——— 回传 ———

    /// 起一个只答一次的 HTTP 服务，返回 `(origin, 收到的整个请求)`。
    async fn serve_once(
        status_line: &'static str,
        body: &'static str,
    ) -> (String, tokio::sync::oneshot::Receiver<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            // 请求有 body，一次 read 未必读完；读到能看见正文结尾为止。
            loop {
                let n = sock.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                if got.ends_with(b"}") {
                    break;
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&got).to_string());
            let resp = format!(
                "HTTP/1.1 {status_line}\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(resp.as_bytes()).await;
        });
        (format!("http://{addr}"), rx)
    }

    /// can-api 的 `/api/v1/logs` 认的是 **CAN 号加网络密码**，不是会话——
    /// 桌面端手里只有这一对。少发 client / version 的话，收到邮件的人不知道
    /// 这份日志是哪个客户端的哪一版。
    #[tokio::test]
    async fn the_log_goes_up_with_the_credentials_and_the_client_name() {
        let (origin, req) = serve_once("200 OK", r#"{"ok":true}"#).await;

        upload_text(
            &reqwest::Client::new(),
            &origin,
            "audio-for-can",
            "27.0.3",
            "1000",
            "hunter2",
            "the last line\n",
        )
        .await
        .expect("a 200 must be a success");

        let raw = req.await.expect("the server saw a request");
        assert!(raw.contains("POST /api/v1/logs"), "{raw}");
        assert!(raw.contains(r#""cid":"1000""#), "{raw}");
        assert!(raw.contains(r#""password":"hunter2""#), "{raw}");
        assert!(raw.contains(r#""client":"audio-for-can""#), "{raw}");
        assert!(raw.contains(r#""version":"27.0.3""#), "{raw}");
        assert!(raw.contains("the last line"), "{raw}");
    }

    /// **密码打错不能报成"发送失败"。** 那句话会把人送去查网络，
    /// 而他要做的只是重打一遍密码。
    #[tokio::test]
    async fn wrong_credentials_say_which_thing_is_wrong() {
        let (origin, _req) = serve_once("401 Unauthorized", r#"{"error":"nope"}"#).await;
        let err = upload_text(
            &reqwest::Client::new(),
            &origin,
            "audio-for-can",
            "27.0.3",
            "1000",
            "wrong",
            "log\n",
        )
        .await
        .expect_err("401 is not a success");
        assert_eq!(err.key, "error.log.credentials");
    }

    /// 限流也有自己的话：can-api 对这条路径按 IP 和按成员各限一道，
    /// 而"过一会儿再试"和"密码不对"要的动作完全不同。
    #[tokio::test]
    async fn being_rate_limited_says_to_wait() {
        let (origin, _req) = serve_once("429 Too Many Requests", r#"{"error":"slow"}"#).await;
        let err = upload_text(
            &reqwest::Client::new(),
            &origin,
            "audio-for-can",
            "27.0.3",
            "1000",
            "hunter2",
            "log\n",
        )
        .await
        .expect_err("429 is not a success");
        assert_eq!(err.key, "error.log.rate_limited");
    }

    // ——— panic 要留下记录 ———

    /// 一个可以读回来的 writer，只给下面那条测试用。
    #[derive(Clone)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("captured").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Captured;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// 钩子装上之后，一次没接住的 panic 要**真的落进日志**，而且带着地点。
    ///
    /// 只测 `panic_text` 不够：那只证明"话取得出来"，不证明有人把它写下去。
    /// GUI 程序里崩溃的表现是窗口没了、日志干净，而那正是这条要挡住的。
    #[test]
    fn an_uncaught_panic_is_written_to_the_log() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(Captured(buf.clone()))
            .finish();

        let restore = std::panic::take_hook();
        // 先装一个哑的：我们的钩子会接着调它，不静音的话测试输出里会多出
        // 一段吓人的 panic 回显。
        std::panic::set_hook(Box::new(|_| {}));
        install_panic_hook();

        tracing::subscriber::with_default(subscriber, || {
            let _ = std::panic::catch_unwind(|| panic!("boom in a thread nobody watches"));
        });
        std::panic::set_hook(restore);

        let got = String::from_utf8(buf.lock().expect("captured").clone()).expect("utf8");
        assert!(got.contains("boom in a thread nobody watches"), "got {got}");
        assert!(got.contains("a panic went uncaught"), "got {got}");
        assert!(
            got.contains("lib.rs"),
            "the location must be in there: {got}"
        );
    }

    /// GUI 程序里一个没接住的 panic 本来什么都不留：窗口没了，日志干净。
    #[test]
    fn a_panic_payload_is_read_out_of_whatever_shape_it_has() {
        assert_eq!(panic_text(&"boom"), "boom");
        assert_eq!(panic_text(&String::from("boom")), "boom");
        // 既不是 &str 也不是 String 的载荷要留下一句能看的话，不是空串。
        assert!(!panic_text(&42i32).is_empty());
    }
}
