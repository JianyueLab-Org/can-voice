use can_voice_listen::{Config as GatewayConfig, ListenGateway};
use std::env;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const INDEX: &str = include_str!("index.html");

#[tokio::main]
async fn main() {
    let addr = env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = TcpListener::bind(&addr)
        .await
        .expect("LISTEN_ADDR must bind");
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        tokio::spawn(handle(stream));
    }
}

async fn handle(mut stream: TcpStream) {
    let Some(request) = read_request(&mut stream).await else {
        return;
    };
    let (method, target, cookie) = request;
    if method != "GET" {
        let _ = response(
            &mut stream,
            "405 Method Not Allowed",
            "text/plain",
            b"method not allowed\n",
        )
        .await;
        return;
    }
    if target == "/healthz" {
        let _ = response(&mut stream, "200 OK", "text/plain", b"ok\n").await;
        return;
    }
    if target == "/" {
        let _ = response(
            &mut stream,
            "200 OK",
            "text/html; charset=utf-8",
            INDEX.as_bytes(),
        )
        .await;
        return;
    }
    let Some(query) = target.strip_prefix("/audio?") else {
        let _ = response(&mut stream, "404 Not Found", "text/plain", b"not found\n").await;
        return;
    };
    let frequency = query
        .split('&')
        .find_map(|part| part.strip_prefix("frequency=")?.parse::<u32>().ok())
        .unwrap_or(118500);
    if !valid_frequency(frequency) {
        let _ = response(
            &mut stream,
            "400 Bad Request",
            "text/plain",
            b"invalid frequency\n",
        )
        .await;
        return;
    }
    let Some(cookie) = cookie else {
        let _ = response(
            &mut stream,
            "401 Unauthorized",
            "text/plain",
            b"login required\n",
        )
        .await;
        return;
    };
    let Some(token) = issue_listener_token(&cookie, frequency).await else {
        let _ = response(
            &mut stream,
            "401 Unauthorized",
            "text/plain",
            b"login required\n",
        )
        .await;
        return;
    };
    let voice_server =
        env::var("CAN_VOICE_SERVER").unwrap_or_else(|_| "audio.ceruleanavi.net:64738".into());
    let server_name = voice_server
        .rsplit_once(':')
        .map(|(host, _)| host.to_string())
        .unwrap_or_else(|| voice_server.clone());
    let config = GatewayConfig {
        server: voice_server,
        server_name,
        token,
        client_id: format!("can-listen/{frequency}"),
        extra_roots: Vec::new(),
    };
    let Ok(mut gateway) = ListenGateway::connect(config, frequency).await else {
        let _ = response(
            &mut stream,
            "503 Service Unavailable",
            "text/plain",
            b"voice unavailable\n",
        )
        .await;
        return;
    };
    let header = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nCache-Control: no-store\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
    if stream.write_all(header).await.is_err() {
        return;
    }
    while let Ok(frame) = gateway.frames().recv().await {
        let mut bytes = Vec::with_capacity(frame.len() * 2);
        for sample in frame {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let prefix = format!("{:X}\r\n", bytes.len());
        if stream.write_all(prefix.as_bytes()).await.is_err()
            || stream.write_all(&bytes).await.is_err()
            || stream.write_all(b"\r\n").await.is_err()
        {
            break;
        }
    }
    gateway.shutdown().await;
}

async fn read_request(stream: &mut TcpStream) -> Option<(String, String, Option<String>)> {
    let mut buf = Vec::with_capacity(4096);
    loop {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).await.ok()?;
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            break;
        }
        if buf.len() > 8192 {
            return None;
        }
    }
    let text = String::from_utf8(buf).ok()?;
    let mut lines = text.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let target = first.next()?.to_string();
    let cookie = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.eq_ignore_ascii_case("cookie")).then(|| value.trim().to_string())
    });
    Some((method, target, cookie))
}

async fn response(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let header = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len());
    stream.write_all(header.as_bytes()).await?;
    stream.write_all(body).await
}

async fn issue_listener_token(cookie: &str, frequency: u32) -> Option<String> {
    let origin = env::var("CAN_API_ORIGIN")
        .unwrap_or_else(|_| "http://app.can-api.svc.cluster.local".into());
    let authority = origin.strip_prefix("http://")?;
    let mut stream = TcpStream::connect(authority).await.ok()?;
    let body = format!("{{\"frequency\":{frequency}}}");
    let request = format!("POST /api/v1/voice/listen-token HTTP/1.1\r\nHost: {authority}\r\nCookie: {cookie}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    stream.write_all(request.as_bytes()).await.ok()?;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.ok()?;
    let text = String::from_utf8(bytes).ok()?;
    if !text.starts_with("HTTP/1.1 200") {
        return None;
    }
    json_string(&text, "token")
}

fn json_string(body: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let start = body.find(&needle)? + needle.len();
    let end = body[start..].find('"')? + start;
    Some(body[start..end].to_string())
}

fn valid_frequency(frequency: u32) -> bool {
    (118000..=136975).contains(&frequency) && frequency % 5 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_parser_reads_the_api_response() {
        assert_eq!(
            json_string(r#"{"token":"abc.def"}"#, "token").as_deref(),
            Some("abc.def")
        );
    }

    #[test]
    fn frequency_validation_matches_the_voice_raster() {
        assert!(valid_frequency(118500));
        assert!(!valid_frequency(118501));
    }
}
