//! 端到端：真的起一个 P2 的服务端，跑一遍握手、订阅、收发。
//!
//! **这是唯一能发现各层拼接错误的测试。** 它需要几件夹具，全部由
//! `go run ./server/cmd/can-voice-e2e-fixture` 一条命令产出，见 `fixture_dir` 的注释。
//! 没有夹具时跳过而不是失败——CI 之外的机器不该因为这个测试红。

use std::process::{Child, Command as Proc};
use std::time::Duration;

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// 夹具目录。仓库根下的 `target/e2e/`，里面应当有：
///
/// ```text
/// can-voice     服务端二进制  go build -o target/e2e/can-voice ./server/cmd/can-voice
/// cert.pem key.pem           由 ca.der 签出的叶证书与私钥（服务端用）
/// ca.der                     一次性的根证书（客户端当额外根证书用）
/// ca.der                   同一张证书的 DER（客户端当额外根证书用）
/// api.pub  token.txt         Ed25519 公钥与一张签好的 token
/// ```
///
/// 后四项由 `go run ./server/cmd/can-voice-e2e-fixture` 一次产出。
fn fixture_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/e2e")
}

fn start_server(port: u16) -> Option<(Server, String)> {
    if std::env::var("CAN_VOICE_E2E").is_err() {
        eprintln!("skipping e2e: set CAN_VOICE_E2E=1 to run it");
        return None;
    }
    let dir = fixture_dir();
    let bin = dir.join("can-voice");
    if !bin.exists() {
        panic!(
            "CAN_VOICE_E2E is set but {} is missing — build it with\n  \
             go build -o target/e2e/can-voice ./server/cmd/can-voice",
            bin.display()
        );
    }
    let pubkey = std::fs::read_to_string(dir.join("api.pub"))
        .expect("api.pub — run `go run ./server/cmd/can-voice-e2e-fixture`");
    let child = Proc::new(&bin)
        .env("CAN_VOICE_ADDR", format!("127.0.0.1:{port}"))
        .env("CAN_VOICE_TLS_CERT", dir.join("cert.pem"))
        .env("CAN_VOICE_TLS_KEY", dir.join("key.pem"))
        .env("CAN_VOICE_API_PUBKEY", pubkey.trim())
        // 射程过滤要 can-fsd 的 SSE；这里没有，服务端会降级为不过滤，
        // 那正是我们要的——本测试测的是拼接，不是射程。
        .env("CAN_VOICE_FSD_FEED", "http://127.0.0.1:1/nope")
        .spawn()
        .expect("spawn the server");
    std::thread::sleep(Duration::from_millis(700));
    Some((Server(child), format!("127.0.0.1:{port}")))
}

#[tokio::test]
async fn a_client_can_hand_shake_subscribe_and_receive() {
    let Some((_srv, addr)) = start_server(64738) else {
        return;
    };
    let dir = fixture_dir();

    let token = std::fs::read_to_string(dir.join("token.txt")).expect("token fixture");
    // **自签证书通过"额外的根证书"进来，不是通过一个跳过校验的开关。**
    let root = std::fs::read(dir.join("ca.der")).expect("ca.der fixture");

    let cfg = can_voice_client::Config {
        server: addr,
        server_name: "localhost".into(),
        token: token.trim().into(),
        client_id: "e2e-test/1".into(),
        follow: String::new(),
        input_device: None,
        output_device: None,
        extra_roots: vec![root],
    };
    let client = can_voice_client::VoiceClient::connect(cfg)
        .await
        .expect("connect");
    let mut events = client.events();

    client.set_subscription(can_voice_proto::control::Sub {
        rx: vec![118_000, 121_800],
        tx: vec![121_800],
        ..Default::default()
    });

    // 应当先看到 Online。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut saw_online = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(500), events.recv()).await {
            Ok(Ok(can_voice_client::Event::State(can_voice_client::LinkState::Online))) => {
                saw_online = true;
                break;
            }
            Ok(Ok(_)) => continue,
            _ => continue,
        }
    }
    assert!(saw_online, "the client never reported Online");
    client.shutdown().await;
}

/// 过期的 token 必须变成一条**说得出原因**的错误，而不是几秒之后一个
/// 没有理由的 Offline。这同时验证了握手真的在 `connect` 里等完了（M6）。
#[tokio::test]
async fn an_expired_token_is_refused_with_a_reason() {
    let Some((_srv, addr)) = start_server(64739) else {
        return;
    };
    let dir = fixture_dir();
    let Ok(token) = std::fs::read_to_string(dir.join("token-expired.txt")) else {
        return;
    };
    let root = std::fs::read(dir.join("ca.der")).expect("ca.der fixture");

    let cfg = can_voice_client::Config {
        server: addr,
        server_name: "localhost".into(),
        token: token.trim().into(),
        client_id: "e2e-test/1".into(),
        follow: String::new(),
        input_device: None,
        output_device: None,
        extra_roots: vec![root],
    };
    let err = can_voice_client::VoiceClient::connect(cfg)
        .await
        .expect_err("must be refused");
    let text = format!("{err}");
    assert!(
        text.contains("TokenExpired") || text.contains("token_expired"),
        "the refusal must name its reason, got {text}"
    );
}

/// 声明超过 `max_tx` 的频率，看服务端真的拒掉、而客户端**用差集**算出是哪几个。
///
/// 这条把 H2 的那条规则端到端地验了一遍：夹具签的 token 里 `MaxTX = 8`，
/// 所以声明 10 个 TX 会有 2 个拿不到。它同时也是"SUB 真的到了服务端、
/// SUBACK 真的回来了"的唯一证据——只看 `Online` 证明不了控制面在动。
#[tokio::test]
async fn declaring_more_tx_than_allowed_comes_back_as_a_denial_per_frequency() {
    let Some((_srv, addr)) = start_server(64740) else {
        return;
    };
    let dir = fixture_dir();
    let token = std::fs::read_to_string(dir.join("token.txt")).expect("token fixture");
    let root = std::fs::read(dir.join("ca.der")).expect("ca.der fixture");

    let cfg = can_voice_client::Config {
        server: addr,
        server_name: "localhost".into(),
        token: token.trim().into(),
        client_id: "e2e-test/1".into(),
        follow: String::new(),
        input_device: None,
        output_device: None,
        extra_roots: vec![root],
    };
    let client = can_voice_client::VoiceClient::connect(cfg)
        .await
        .expect("connect");
    let mut events = client.events();

    // MaxTX 是 8，这里声明 10 个。
    let freqs: Vec<u32> = (0..10).map(|i| 118_000 + i * 25).collect();
    client.set_subscription(can_voice_proto::control::Sub {
        rx: freqs.clone(),
        tx: freqs.clone(),
        ..Default::default()
    });

    let mut denied_tx = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline && denied_tx.len() < 2 {
        if let Ok(Ok(can_voice_client::Event::TxDenied { freq_khz, .. })) =
            tokio::time::timeout(Duration::from_millis(300), events.recv()).await
        {
            denied_tx.push(freq_khz);
        }
    }
    assert_eq!(
        denied_tx.len(),
        2,
        "10 declared tx against max_tx=8 must come back as exactly 2 denials, got {denied_tx:?}"
    );
    for f in &denied_tx {
        assert!(freqs.contains(f), "{f} was never declared");
    }
    client.shutdown().await;
}
