//! 手工验证用的命令行客户端。
//!
//!     cargo run -p can-voice-client --example canvoice-cli -- \
//!         --server audio.ceruleanavi.net:64738 --token "$TOKEN" --rx 118000,121800
//!
//! 加 `--audio` 才真的开声卡（默认不开，免得在没有声卡的机器上报错）。
//! 打本地自签的服务端时加 `--root target/e2e/ca.der`。**没有"跳过校验"的开关**，
//! 而且不会有：这条链路上跑的是成员的网络密码。

use std::time::Duration;

fn freq_list(s: String) -> Vec<u32> {
    s.split(',').filter_map(|x| x.trim().parse().ok()).collect()
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter("info").init();

    let mut server = String::from("127.0.0.1:64738");
    let mut token = String::new();
    let mut follow = String::new();
    let mut rx: Vec<u32> = Vec::new();
    let mut roots: Vec<Vec<u8>> = Vec::new();
    let mut audio = false;
    let mut tx: Vec<u32> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--audio" => audio = true,
            "--server" => server = args.next().unwrap_or_default(),
            "--token" => token = args.next().unwrap_or_default(),
            "--follow" => follow = args.next().unwrap_or_default(),
            "--root" => {
                let path = args.next().unwrap_or_default();
                match std::fs::read(&path) {
                    Ok(der) => roots.push(der),
                    Err(e) => {
                        eprintln!("could not read {path}: {e}");
                        std::process::exit(2);
                    }
                }
            }
            "--rx" => rx = freq_list(args.next().unwrap_or_default()),
            "--tx" => tx = freq_list(args.next().unwrap_or_default()),
            other => eprintln!("unknown argument {other}"),
        }
    }
    if token.is_empty() {
        eprintln!("--token is required");
        std::process::exit(2);
    }

    let server_name = server.split(':').next().unwrap_or("localhost").to_string();
    let cfg = can_voice_client::Config {
        server,
        server_name,
        token,
        client_id: concat!("canvoice-cli/", env!("CARGO_PKG_VERSION")).into(),
        follow,
        input_device: None,
        output_device: None,
        audio_devices: audio,
        extra_roots: roots,
    };

    let client = match can_voice_client::VoiceClient::connect(cfg).await {
        Ok(c) => c,
        Err(e) => {
            // connect 等到握手完成才返回，所以这条错误是说得出原因的：
            // 地址解析不了、token 过期、版本太旧，各有各的话。
            eprintln!("connect failed: {e}");
            std::process::exit(1);
        }
    };
    let mut events = client.events();
    client.set_subscription(can_voice_proto::control::Sub {
        rx,
        tx,
        ..Default::default()
    });

    loop {
        match tokio::time::timeout(Duration::from_secs(30), events.recv()).await {
            Ok(Ok(e)) => tracing::info!(?e, "event"),
            Ok(Err(_)) => break,
            Err(_) => tracing::info!("no events for 30 s"),
        }
    }
}
