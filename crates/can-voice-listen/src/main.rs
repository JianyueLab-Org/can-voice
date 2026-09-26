use can_voice_listen::{Config as GatewayConfig, ListenGateway};
use std::env;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const INDEX: &str = include_str!("index.html");
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const TOKEN_RESPONSE_LIMIT: usize = 64 * 1024;

#[tokio::main]
async fn main() {
    let addr = env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let listener = TcpListener::bind(&addr)
        .await
        .expect("LISTEN_ADDR must bind");
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { continue };
                tokio::spawn(handle(stream));
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
}

async fn handle(mut stream: TcpStream) {
    let Some(request) = tokio::time::timeout(REQUEST_TIMEOUT, read_request(&mut stream))
        .await
        .ok()
        .flatten()
    else {
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
    let Some(Ok(mut gateway)) =
        tokio::time::timeout(CONNECT_TIMEOUT, ListenGateway::connect(config, frequency))
            .await
            .ok()
    else {
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
    if tokio::time::timeout(IO_TIMEOUT, stream.write_all(header))
        .await
        .ok()
        .and_then(Result::ok)
        .is_none()
    {
        return;
    }
    let mut closed = [0u8; 1];
    loop {
        tokio::select! {
            frame = gateway.frames().recv() => {
                let Ok(frame) = frame else { break };
                let mut bytes = Vec::with_capacity(frame.len() * 2);
                for sample in frame {
                    bytes.extend_from_slice(&sample.to_le_bytes());
                }
                if !write_stream_chunk(&mut stream, &bytes).await {
                    break;
                }
            }
            result = stream.read(&mut closed) => {
                match result {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
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
    tokio::time::timeout(IO_TIMEOUT, stream.write_all(header.as_bytes()))
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "response header timeout")
        })??;
    tokio::time::timeout(IO_TIMEOUT, stream.write_all(body))
        .await
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::TimedOut, "response body timeout")
        })??;
    Ok(())
}

async fn write_stream_chunk<W>(stream: &mut W, bytes: &[u8]) -> bool
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let prefix = format!("{:X}\r\n", bytes.len());
    if tokio::time::timeout(IO_TIMEOUT, stream.write_all(prefix.as_bytes()))
        .await
        .ok()
        .and_then(Result::ok)
        .is_none()
    {
        return false;
    }
    if tokio::time::timeout(IO_TIMEOUT, stream.write_all(bytes))
        .await
        .ok()
        .and_then(Result::ok)
        .is_none()
    {
        return false;
    }
    tokio::time::timeout(IO_TIMEOUT, stream.write_all(b"\r\n"))
        .await
        .ok()
        .and_then(Result::ok)
        .is_some()
}

async fn issue_listener_token(cookie: &str, frequency: u32) -> Option<String> {
    let origin = env::var("CAN_API_ORIGIN")
        .unwrap_or_else(|_| "http://app.can-api.svc.cluster.local".into());
    let authority = origin.strip_prefix("http://")?;
    let mut stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(authority))
        .await
        .ok()
        .and_then(Result::ok)?;
    let body = format!("{{\"frequency\":{frequency}}}");
    let request = format!("POST /api/v1/voice/listen-token HTTP/1.1\r\nHost: {authority}\r\nCookie: {cookie}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    tokio::time::timeout(IO_TIMEOUT, stream.write_all(request.as_bytes()))
        .await
        .ok()
        .and_then(Result::ok)?;
    let bytes = tokio::time::timeout(IO_TIMEOUT, read_token_response(&mut stream))
        .await
        .ok()
        .flatten()?;
    let text = String::from_utf8(bytes).ok()?;
    if !text.starts_with("HTTP/1.1 200") {
        return None;
    }
    json_string(&text, "token")
}

async fn read_token_response<R>(stream: &mut R) -> Option<Vec<u8>>
where
    R: AsyncRead + Unpin,
{
    let mut bytes = Vec::with_capacity(4096);
    loop {
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return Some(bytes);
        }
        if bytes.len().saturating_add(n) > TOKEN_RESPONSE_LIMIT {
            return None;
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
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

    #[tokio::test]
    async fn stream_chunk_write_reports_a_closed_connection() {
        let (mut writer, reader) = tokio::io::duplex(1);
        drop(reader);

        assert!(!write_stream_chunk(&mut writer, b"audio").await);
    }

    #[tokio::test]
    async fn token_response_is_bounded_even_when_peer_sends_more() {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        let send = tokio::spawn(async move {
            writer
                .write_all(&vec![b'x'; TOKEN_RESPONSE_LIMIT + 1])
                .await
        });
        assert!(read_token_response(&mut reader).await.is_none());
        send.await.unwrap().unwrap();
    }
}
