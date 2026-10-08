//! Minimal local HTTP/1.1 server (§14.1) and a tiny blocking client for the CLI.
//! Requests are handed to the controller loop together with a reply channel.

use std::io::{Read, Write};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

pub const MAX_REQUEST: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    /// without the query string
    pub path: String,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(v: &serde_json::Value) -> Self {
        Response { status: 200, content_type: "application/json", body: serde_json::to_vec(v).unwrap_or_default() }
    }
    pub fn pretty(v: &serde_json::Value) -> Self {
        Response { status: 200, content_type: "application/json", body: serde_json::to_vec_pretty(v).unwrap_or_default() }
    }
    pub fn ok() -> Self {
        Self::raw(200, r#"{"ok":true}"#)
    }
    pub fn error(status: u16, msg: &str) -> Self {
        Response {
            status,
            content_type: "application/json",
            body: serde_json::to_vec(&serde_json::json!({ "error": msg })).unwrap_or_default(),
        }
    }
    pub fn raw(status: u16, json: &str) -> Self {
        Response { status, content_type: "application/json", body: json.as_bytes().to_vec() }
    }
    pub fn png(bytes: Vec<u8>) -> Self {
        Response { status: 200, content_type: "image/png", body: bytes }
    }
}

pub type Incoming = (Request, oneshot::Sender<Response>);

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        404 => "Not Found",
        _ => "Bad Request",
    }
}

/// Binds 127.0.0.1:port. Each request goes to `tx`; the connection is closed after the reply.
pub async fn serve(port: u16, tx: mpsc::UnboundedSender<Incoming>) -> std::io::Result<tokio::task::JoinHandle<()>> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    Ok(tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { continue };
            let tx = tx.clone();
            tokio::spawn(async move {
                let Some(req) = read_request(&mut sock).await else { return };
                let (rtx, rrx) = oneshot::channel();
                if tx.send((req, rtx)).is_err() {
                    return;
                }
                let resp = rrx.await.unwrap_or_else(|_| Response::error(400, "shutting down"));
                let head = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    resp.status,
                    status_text(resp.status),
                    resp.content_type,
                    resp.body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&resp.body).await;
                let _ = sock.shutdown().await;
            });
        }
    }))
}

async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<Request> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let n = tokio::time::timeout_at(deadline, sock.read(&mut chunk)).await.ok()?.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_REQUEST {
            return None;
        }
        if let Some((req, done)) = parse_request(&buf) {
            if done {
                return Some(req);
            }
        }
    }
}

/// `(request, complete)`; `None` while the header is incomplete.
pub fn parse_request(buf: &[u8]) -> Option<(Request, bool)> {
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&buf[..end]);
    let mut lines = head.split('\n');
    let first = lines.next()?.trim();
    let mut parts = first.split(' ');
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/");
    let path = target.split('?').next().unwrap_or("/").to_string();
    let mut length = 0usize;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let body_start = end + 4;
    let complete = buf.len() >= body_start + length;
    let body = buf[body_start..buf.len().min(body_start + length)].to_vec();
    Some((Request { method, path, body }, complete))
}

/// Blocking request to the running instance: `(status, body)`, or `None` if nobody answers.
pub fn request(port: u16, method: &str, path: &str, body: &[u8], timeout: Duration) -> Option<(u16, Vec<u8>)> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(700)).ok()?;
    s.set_read_timeout(Some(timeout)).ok()?;
    s.set_write_timeout(Some(timeout)).ok()?;
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).ok()?;
    s.write_all(body).ok()?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).ok()?;
    let end = out.windows(4).position(|w| w == b"\r\n\r\n")?;
    let status = String::from_utf8_lossy(&out[..end]).split(' ').nth(1)?.parse().ok()?;
    Some((status, out[end + 4..].to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_requests() {
        let raw = b"POST /hook?x=1 HTTP/1.1\r\nHost: a\r\ncontent-length: 5\r\n\r\n{\"a\"";
        let (r, done) = parse_request(raw).unwrap();
        assert!(!done, "body incomplete");
        assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/hook"));
        let mut full = raw.to_vec();
        full.push(b'}');
        let (r, done) = parse_request(&full).unwrap();
        assert!(done);
        assert_eq!(r.body, b"{\"a\"}");
        assert!(parse_request(b"GET / HTTP/1.1\r\n").is_none());
    }

    #[test]
    fn serves_and_replies() {
        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().worker_threads(2).build().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel::<Incoming>();
        let port = 47_000 + (std::process::id() % 700) as u16;
        rt.block_on(async {
            serve(port, tx).await.unwrap();
            tokio::spawn(async move {
                while let Some((req, reply)) = rx.recv().await {
                    let _ = reply.send(if req.path == "/status" { Response::raw(200, r#"{"state":"chilling"}"#) } else { Response::error(404, "not found") });
                }
            });
        });
        let (st, body) = request(port, "GET", "/status", b"", Duration::from_secs(2)).unwrap();
        assert_eq!(st, 200);
        assert_eq!(body, br#"{"state":"chilling"}"#);
        assert_eq!(request(port, "GET", "/nope", b"", Duration::from_secs(2)).unwrap().0, 404);
        // a second bind on the same port fails ("порт занят")
        let (tx2, _rx2) = mpsc::unbounded_channel::<Incoming>();
        assert!(rt.block_on(serve(port, tx2)).is_err());
    }
}
