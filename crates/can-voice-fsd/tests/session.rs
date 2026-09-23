//! 拿一个假 FSD 服务端把连接的行为跑一遍。
//!
//! 这一层的规矩里有三条**只在时序上成立**，纯函数测不到：首次连不上不重试、
//! 登录后掉线才重试、登录后的 `$ER` 不拆连接。三条都是踩出来的，所以都在这里。

use can_voice_fsd::client::{self, Config, FsdState, Reason};
use can_voice_fsd::observer_client::{self, ObserverConfig};
use can_voice_fsd::packet::{Identity, Position, FACILITY_ATIS, RATING_OBSERVER};
use can_voice_fsd::session::MAX_LINE_BYTES;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// 假服务端收到的每一行，以及它被连了几次。
#[derive(Default)]
struct Log {
    lines: Mutex<Vec<String>>,
    connections: AtomicUsize,
}

impl Log {
    fn lines(&self) -> Vec<String> {
        self.lines.lock().expect("lock").clone()
    }
    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
    fn saw(&self, needle: &str) -> bool {
        self.lines().iter().any(|l| l.contains(needle))
    }
}

/// 起一个假服务端。`script` 对每条连接跑一遍。
async fn fake_server<F, Fut>(script: F) -> (u16, Arc<Log>)
where
    F: Fn(TcpStream, Arc<Log>, usize) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let log = Arc::new(Log::default());
    let for_task = log.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let n = for_task.connections.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(script(stream, for_task.clone(), n));
        }
    });
    (port, log)
}

/// 读客户端发来的行，记进日志，并在看到 CAPS 查询时答一句让它登录成功。
async fn serve(stream: TcpStream, log: Arc<Log>, accept_login: bool, drop_after: Option<usize>) {
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let mut seen = 0usize;
    while let Ok(Some(line)) = lines.next_line().await {
        log.lines.lock().expect("lock").push(line.clone());
        seen += 1;
        if line.starts_with("$CQ") && line.contains(":SERVER:CAPS") && accept_login {
            let _ = write
                .write_all(b"$CRSERVER:ZSPD_ATIS:CAPS:ATCINFO=1\r\n")
                .await;
        }
        if Some(seen) == drop_after {
            return; // 掉线
        }
    }
}

fn config(port: u16) -> Config {
    Config {
        host: "127.0.0.1".into(),
        port,
        identity: Identity::new("ZSPD_ATIS", "1234", "pw", "ATIS", RATING_OBSERVER),
        position: Position {
            frequency: "127.850".into(),
            facility: FACILITY_ATIS,
            vis_range: 50,
            rating: RATING_OBSERVER,
            latitude: 31.142_33,
            longitude: 121.790_84,
        },
        atis_lines: vec!["ZSPD ATIS ALPHA".into()],
        reconnect_limit: 2,
    }
}

/// 等一个满足条件的事件，超时就失败。
async fn wait_for(
    events: &mut tokio::sync::broadcast::Receiver<client::FsdEvent>,
    want: impl Fn(&client::FsdEvent) -> bool,
    what: &str,
) -> client::FsdEvent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(!left.is_zero(), "timed out waiting for {what}");
        match tokio::time::timeout(left, events.recv()).await {
            Ok(Ok(e)) if want(&e) => return e,
            Ok(Ok(_)) => continue,
            Ok(Err(e)) => panic!("the event stream died while waiting for {what}: {e}"),
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

#[tokio::test]
async fn a_login_reaches_online_and_puts_the_station_on_the_map() {
    let (port, log) = fake_server(|s, l, _| serve(s, l, true, None)).await;
    let handle = client::connect(config(port));
    let mut events = handle.events();
    wait_for(&mut events, |e| e.state == FsdState::Online, "online").await;

    assert!(log.saw("$IDZSPD_ATIS:SERVER:0001:"), "{:?}", log.lines());
    assert!(
        log.saw("#AAZSPD_ATIS:SERVER:ATIS:1234:pw:1:100"),
        "{:?}",
        log.lines()
    );
    // 位置包紧跟在登录成功之后，不等第一个 15 秒——否则席位在雷达上要等一刻钟。
    // 要轮询：Online 是我们这一侧发的事件，服务端把那一行读进来还要一点时间。
    until(&log, "%ZSPD_ATIS:27850:7:50:1:31.14233:121.79084:0").await;
    handle.stop();
}

/// 等日志里出现某一行。断言前**必须**过这一道：`Online` 是客户端这一侧发的
/// 事件，服务端有没有把那一行读进来是另一回事，直接断言就是一个会偶发的测试。
async fn until(log: &Arc<Log>, needle: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        if log.saw(needle) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never saw {needle:?} in {:?}", log.lines());
}

/// **首次连不上不重试。**
///
/// 那多半是呼号被占、密码不对或者地址填错——重试只会把同一条错误刷三遍，
/// 还可能触发服务端对认证失败的限流。
#[tokio::test]
async fn the_very_first_connection_is_never_retried() {
    // 一个立刻关掉连接、从不答 CAPS 的服务端：登录失败。
    let (port, log) = fake_server(|s, l, _| serve(s, l, false, Some(1))).await;
    let handle = client::connect(config(port));
    let mut events = handle.events();
    wait_for(
        &mut events,
        |e| matches!(e.reason, Reason::Closed | Reason::LoginTimeout),
        "a failed first login",
    )
    .await;

    // 给它足够长的时间去做那件不该做的事。
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(
        log.connections(),
        1,
        "the first attempt must not be retried: {:?}",
        log.lines()
    );
}

/// **登录成功过之后掉的线才重连**，而且次数用尽要报终态。
///
/// 第一条连接登录成功然后掉线，后面两条连不上——两次用尽，报 `Offline`。
/// 调用方据此把这个席位整个收掉（语音也一起），而不是留一条谁也说不清状态的
/// 连接。
#[tokio::test]
async fn a_drop_after_login_is_retried_and_then_gives_up() {
    // 第一条连接走完登录再掐；后面两条一开口就关掉，所以是"连不上"而不是
    // 慢慢等到登录超时——那样光两次超时就要二十秒。
    let (port, log) =
        fake_server(|s, l, n| serve(s, l, n == 0, if n == 0 { Some(4) } else { Some(1) })).await;
    let handle = client::connect(config(port));
    let mut events = handle.events();

    wait_for(&mut events, |e| e.state == FsdState::Online, "online").await;
    let gave_up = wait_for(
        &mut events,
        |e| e.state == FsdState::Offline,
        "giving up after the retries",
    )
    .await;
    assert_eq!(gave_up.reason, Reason::GaveUp { limit: 2 });
    // 首次 + 两次重连 = 三条连接。
    assert_eq!(log.connections(), 3, "{:?}", log.lines());
}

/// **一次成功的重连把计数清零。**
///
/// 计的是"连着失败几次"，不是"这条连接一辈子断过几次"。不清零的话，一个连了
/// 八小时、中间抖过两次的席位会在第三次抖动时整个下线，而它其实一直好好的。
///
/// 这一条是时序上才成立的，纯函数测不到：让服务端一直"登录成功然后掐线"，
/// 它就该一直重连下去，永远走不到 `Offline`。
#[tokio::test]
async fn a_successful_reconnect_clears_the_counter() {
    let (port, log) = fake_server(|s, l, _| serve(s, l, true, Some(4))).await;
    let handle = client::connect(config(port));
    let mut events = handle.events();
    wait_for(&mut events, |e| e.state == FsdState::Online, "online").await;

    // 重连上限是 2，所以"不清零"的实现在第三次之后就会放弃。
    // 等到连接数明显超过那个数，再看它有没有报终态。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
    while tokio::time::Instant::now() < deadline && log.connections() < 5 {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        log.connections() >= 5,
        "it stopped reconnecting after {} tries",
        log.connections()
    );
    handle.stop();
}

/// 登录被拒时**说得出是哪一条**，而不是一句"连接失败"。
#[tokio::test]
async fn a_refused_login_reports_the_server_reason() {
    let (port, _log) = fake_server(|stream, log, _| async move {
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            log.lines.lock().expect("lock").push(line.clone());
            if line.starts_with("#AA") {
                let _ = write
                    .write_all(b"$ERSERVER:ZSPD_ATIS:006:ZSPD_ATIS:invalid logon\r\n")
                    .await;
                return;
            }
        }
    })
    .await;
    let handle = client::connect(config(port));
    let mut events = handle.events();
    let e = wait_for(
        &mut events,
        |e| matches!(e.reason, Reason::Rejected { .. }),
        "a rejection",
    )
    .await;
    assert_eq!(
        e.reason,
        Reason::Rejected {
            code: "006".into(),
            message: "invalid logon".into()
        }
    );
}

/// **登录之后的 `$ER` 不该把整条连接拆掉。**
///
/// 那多半只是某次查询失败（比如没有该机场的气象）。拆掉的话，一次问不到的
/// METAR 会让整个席位下线。
#[tokio::test]
async fn an_error_after_login_does_not_drop_the_connection() {
    let (port, log) = fake_server(|stream, log, _| async move {
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            log.lines.lock().expect("lock").push(line.clone());
            if line.contains(":SERVER:CAPS") {
                let _ = write
                    .write_all(b"$CRSERVER:ZSPD_ATIS:CAPS:ATCINFO=1\r\n")
                    .await;
                // 登录之后立刻来一条错误，然后照常伺候。
                let _ = write
                    .write_all(b"$ERSERVER:ZSPD_ATIS:009:ZZZZ:no such airport\r\n")
                    .await;
            }
            if line.starts_with("$CQCES123") {
                // 还活着就该答得出通播查询。
            }
        }
    })
    .await;
    let handle = client::connect(config(port));
    let mut events = handle.events();
    wait_for(&mut events, |e| e.state == FsdState::Online, "online").await;

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        log.connections(),
        1,
        "the connection was torn down and retried: {:?}",
        log.lines()
    );
    handle.stop();
}

/// 通播查询答的是**此刻**那份文字，不是连接时那份。
#[tokio::test]
async fn an_atis_query_is_answered_with_the_current_lines() {
    let (port, log) = fake_server(|stream, log, _| async move {
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        let mut asked = false;
        while let Ok(Some(line)) = lines.next_line().await {
            log.lines.lock().expect("lock").push(line.clone());
            if line.contains(":SERVER:CAPS") {
                let _ = write
                    .write_all(b"$CRSERVER:ZSPD_ATIS:CAPS:ATCINFO=1\r\n")
                    .await;
            }
            // 等到客户端换过一次文字（位置包之后），再去问。
            if !asked && line.starts_with('%') {
                asked = true;
                tokio::time::sleep(Duration::from_millis(300)).await;
                let _ = write.write_all(b"$CQCES123:ZSPD_ATIS:ATIS\r\n").await;
            }
        }
    })
    .await;
    let handle = client::connect(config(port));
    let mut events = handle.events();
    wait_for(&mut events, |e| e.state == FsdState::Online, "online").await;
    handle.set_atis_lines(vec!["ZSPD ATIS BRAVO".into()]);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && !log.saw("BRAVO") {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        log.saw("$CRZSPD_ATIS:CES123:ATIS:T:ZSPD ATIS BRAVO"),
        "{:?}",
        log.lines()
    );
    assert!(log.saw("$CRZSPD_ATIS:CES123:ATIS:E:1"), "{:?}", log.lines());
    // 问的人不是 SERVER，所以不发 TEXTATIS。
    assert!(!log.saw("TEXTATIS"), "{:?}", log.lines());
    handle.stop();
}

/// 要一份 METAR 走的是服务端自己的气象源，不用再连外部接口。
#[tokio::test]
async fn a_metar_request_is_answered_from_the_server() {
    let (port, _log) = fake_server(|stream, log, _| async move {
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            log.lines.lock().expect("lock").push(line.clone());
            if line.contains(":SERVER:CAPS") {
                let _ = write
                    .write_all(b"$CRSERVER:ZSPD_ATIS:CAPS:ATCINFO=1\r\n")
                    .await;
            }
            if line.starts_with("$AX") && line.contains("METAR:ZSPD") {
                let _ = write
                    .write_all(b"$ARserver:ZSPD_ATIS:METAR:ZSPD 251300Z 09004MPS Q1013\r\n")
                    .await;
            }
        }
    })
    .await;
    let handle = client::connect(config(port));
    let mut events = handle.events();
    wait_for(&mut events, |e| e.state == FsdState::Online, "online").await;

    let report = handle.request_metar("zspd", Duration::from_secs(10)).await;
    assert_eq!(report.as_deref(), Some("ZSPD 251300Z 09004MPS Q1013"));
    handle.stop();
}

#[tokio::test]
async fn an_oversized_login_line_closes_the_session() {
    let (port, _log) = fake_server(|stream, log, _| async move {
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            log.lines.lock().expect("lock").push(line.clone());
            if line.contains(":SERVER:CAPS") {
                let mut packet = vec![b'X'; MAX_LINE_BYTES + 1];
                packet.push(b'\n');
                write
                    .write_all(&packet)
                    .await
                    .expect("write oversized line");
                tokio::time::sleep(Duration::from_millis(200)).await;
                return;
            }
        }
    })
    .await;
    let handle = client::connect(config(port));
    let mut events = handle.events();
    let error = wait_for(
        &mut events,
        |e| matches!(e.reason, Reason::Protocol(_)),
        "protocol error",
    )
    .await;
    assert_eq!(error.state, FsdState::Error);
}

#[tokio::test]
async fn observer_logs_in_at_facility_zero_and_publishes_own_position() {
    let (port, log) = fake_server(|stream, log, _| async move {
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            log.lines.lock().expect("lock").push(line.clone());
            if line.contains(":SERVER:CAPS") {
                write
                    .write_all(b"$CRSERVER:ZSPD_OBS:CAPS:ATCINFO=1\r\n")
                    .await
                    .expect("login");
            }
        }
    })
    .await;
    let observer = observer_client::connect(ObserverConfig {
        host: "127.0.0.1".into(),
        port,
        identity: Identity::new("ZSPD_OBS", "1234", "pw", "Observer", RATING_OBSERVER),
        reconnect_limit: 0,
    });
    observer.update_position(31.14233, 121.79084);
    until(&log, "%ZSPD_OBS:99998:0:100:1:31.14233:121.79084:0").await;
    let mut events = observer.events();
    wait_for(
        &mut events,
        |e| e.state == FsdState::Online,
        "observer online after delayed subscription read",
    )
    .await;
    assert!(log.saw("#AAZSPD_OBS:SERVER:Observer:1234:pw:1:100"));
    assert!(!log.saw("#APZSPD_OBS"));
    observer.stop();
}

#[tokio::test]
async fn expired_metar_requests_release_capacity_without_reconnecting() {
    let (port, log) = fake_server(|stream, log, _| async move {
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            log.lines.lock().expect("lock").push(line.clone());
            if line.contains(":SERVER:CAPS") {
                write
                    .write_all(b"$CRSERVER:ZSPD_ATIS:CAPS:ATCINFO=1\r\n")
                    .await
                    .expect("login");
            }
        }
    })
    .await;
    let handle = client::connect(config(port));
    let mut events = handle.events();
    wait_for(&mut events, |e| e.state == FsdState::Online, "online").await;

    let mut requests = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let handle = handle.clone();
        requests.spawn(async move {
            handle
                .request_metar("ZSPD", Duration::from_millis(100))
                .await
        });
    }
    while let Some(result) = requests.join_next().await {
        assert_eq!(result.expect("request"), None);
    }
    let request = handle.request_metar("ZSPD", Duration::from_millis(200));
    assert_eq!(request.await, None);
    until(&log, "METAR:ZSPD").await;
    let sent = log
        .lines()
        .iter()
        .filter(|line| line.contains("METAR:ZSPD"))
        .count();
    assert_eq!(sent, 33, "expired requests must not exhaust the cap");
    assert_eq!(log.connections(), 1);
    handle.stop();
}

#[tokio::test]
async fn excess_metar_requests_are_rejected_without_sending_to_fsd() {
    let (port, log) = fake_server(|stream, log, _| async move {
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            log.lines.lock().expect("lock").push(line.clone());
            if line.contains(":SERVER:CAPS") {
                write
                    .write_all(b"$CRSERVER:ZSPD_ATIS:CAPS:ATCINFO=1\r\n")
                    .await
                    .expect("login");
            }
        }
    })
    .await;
    let handle = client::connect(config(port));
    let mut events = handle.events();
    wait_for(&mut events, |e| e.state == FsdState::Online, "online").await;

    let mut requests = tokio::task::JoinSet::new();
    for _ in 0..33 {
        let handle = handle.clone();
        requests.spawn(async move { handle.request_metar("ZSPD", Duration::from_secs(3)).await });
    }
    let first = tokio::time::timeout(Duration::from_secs(1), requests.join_next())
        .await
        .expect("over-cap request should return immediately")
        .expect("request task")
        .expect("request join");
    assert_eq!(first, None);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        log.lines()
            .iter()
            .filter(|line| line.contains("METAR:ZSPD"))
            .count(),
        32
    );
    requests.abort_all();
    handle.stop();
}

/// 按停止就要**下线**，而且要告诉服务端——留一条不打招呼就断的连接，
/// 席位会在在线列表里挂到超时。
#[tokio::test]
async fn stopping_says_goodbye_and_reaches_a_terminal_state() {
    let (port, log) = fake_server(|s, l, _| serve(s, l, true, None)).await;
    let handle = client::connect(config(port));
    let mut events = handle.events();
    wait_for(&mut events, |e| e.state == FsdState::Online, "online").await;

    handle.stop();
    let e = wait_for(&mut events, |e| e.state == FsdState::Stopped, "stopped").await;
    assert_eq!(e.reason, Reason::Stopped);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && !log.saw("#DA") {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(log.saw("#DAZSPD_ATIS:1234"), "{:?}", log.lines());
    // 停止之后不重连。
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(log.connections(), 1, "{:?}", log.lines());
}

/// 呼号根本不合规的时候，**一个包都不该发出去**。
#[tokio::test]
async fn a_bad_callsign_never_opens_a_socket() {
    let (port, log) = fake_server(|s, l, _| serve(s, l, true, None)).await;
    let mut cfg = config(port);
    cfg.identity = Identity::new("ZSPD_TWR", "1234", "pw", "ATIS", 1);
    let handle = client::connect(cfg);
    let mut events = handle.events();
    wait_for(
        &mut events,
        |e| matches!(e.reason, Reason::Callsign(_)),
        "a callsign complaint",
    )
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(log.connections(), 0, "{:?}", log.lines());
}
