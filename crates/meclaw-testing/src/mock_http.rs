//! Phase-7 slice 3 — hand-rolled mock HTTP server for hermetic tests.
//!
//! TcpListener on `127.0.0.1:0` (the kernel picks the port). Accepts connections
//! in a background task, parses the request headers (content irrelevant), sends a
//! fixed `MockResponse`, closes the connection. One connection per request —
//! keep-alive is not supported.
//!
//! Purpose: hermetic HTTP tests without egress (the container allowlist does not
//! permit example.com). reqwest cells fetch `http://127.0.0.1:<port>` — localhost
//! passes the allowlist filter.
//!
//! Phase-8 extension (T9): `start_mock_server_capturing` + `MockResponse::with_delay`
//! for POST body capture (Content-Length-aware) and deterministic A-timeout tests.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Canned HTTP response returned by the mock server for every connection.
#[derive(Clone)]
pub struct MockResponse {
    /// HTTP status code (e.g. 200, 404, 500).
    pub status: u16,
    /// Response body bytes.
    pub body: Vec<u8>,
    /// Content-Type header value.
    pub content_type: String,
    /// Optional artificial delay before writing the response (for A-timeout tests).
    pub delay: Option<Duration>,
}

impl MockResponse {
    /// 200 OK with plain-text body.
    pub fn ok(body: &[u8]) -> Self {
        Self {
            status: 200,
            body: body.to_vec(),
            content_type: "text/plain; charset=utf-8".into(),
            delay: None,
        }
    }
    /// 200 OK with JSON body (Content-Type application/json).
    pub fn ok_json(body: &[u8]) -> Self {
        Self {
            status: 200,
            body: body.to_vec(),
            content_type: "application/json".into(),
            delay: None,
        }
    }
    /// 404 Not Found with plain body "not found".
    pub fn not_found() -> Self {
        Self {
            status: 404,
            body: b"not found".to_vec(),
            content_type: "text/plain; charset=utf-8".into(),
            delay: None,
        }
    }
    /// 500 Internal Server Error.
    pub fn server_error() -> Self {
        Self {
            status: 500,
            body: b"internal server error".to_vec(),
            content_type: "text/plain; charset=utf-8".into(),
            delay: None,
        }
    }
    /// Builder: attach an artificial delay before the server writes the response.
    ///
    /// The handler will `tokio::time::sleep(delay).await` BEFORE writing the
    /// HTTP response head — useful for driving A-Timeout tests deterministically.
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }
}

/// A request captured by `start_mock_server_capturing`.
///
/// Header names are normalized to lowercase. The body is read fully (Content-Length-aware).
#[derive(Debug, Clone)]
pub struct CapturedRequest {
    /// HTTP method (e.g. "GET", "POST").
    pub method: String,
    /// Request path including query string (e.g. "/chat/completions").
    pub path: String,
    /// Request headers, names lowercased for normalization.
    pub headers: HashMap<String, String>,
    /// Full request body (Content-Length bytes).
    pub body: Vec<u8>,
}

/// Spawns a mock HTTP server on `127.0.0.1:0`. Returns `(SocketAddr, JoinHandle)`.
///
/// The JoinHandle can be aborted via `.abort()`; alternatively the background
/// task runs until the test drops it (tokio runtime teardown).
///
/// Each accepted connection receives the canned `response`. The request is
/// read until `\r\n\r\n` then discarded — only GET is relevant for Slice-3.
/// Honors `response.delay` (sleeps before writing).
pub async fn start_mock_server(response: MockResponse) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
    let addr = listener.local_addr().expect("local_addr failed");
    let join = tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(p) => p,
                Err(_) => break,
            };
            let resp = response.clone();
            tokio::spawn(async move {
                let _ = handle_connection(stream, resp).await;
            });
        }
    });
    (addr, join)
}

/// Spawns a mock HTTP server on `127.0.0.1:0` that captures all incoming requests.
///
/// Returns `(SocketAddr, JoinHandle, Arc<Mutex<Vec<CapturedRequest>>>)`. Each accepted
/// connection is read fully (Content-Length-aware for POST bodies), pushed into the
/// shared capture Vec, and answered with the next `MockResponse` from `response_sequence`.
///
/// If the test sends more requests than there are responses, the handler serves
/// the LAST configured response again (instead of panicking) and still records
/// the request — an extra request beyond the canned sequence is a benign,
/// TTL-capped collector double-fire under parallel load (see
/// `phase_14b_fanin_correlation`); serving the terminating last response lets it
/// die cleanly. Correctness is gated by each test's positive receipts
/// (DLQ-empty / `finish_reason` / store rows), not by the mock crashing; the raw
/// request count stays available for diagnostic assertions. An EMPTY response
/// sequence with an incoming request is still a loud panic (genuine misconfig).
pub async fn start_mock_server_capturing(
    response_sequence: Vec<MockResponse>,
) -> (SocketAddr, JoinHandle<()>, Arc<Mutex<Vec<CapturedRequest>>>) {
    start_mock_server_capturing_with_validator(response_sequence, None).await
}

/// Per-request validator hook for `start_mock_server_capturing_with_validator`.
///
/// Runs against every captured request BEFORE a canned response is popped.
/// Returning `Some(response)` short-circuits the connection with that response;
/// the canned sequence is NOT consumed (it answers valid requests only).
/// Provider-specific validation logic (e.g. OpenAI message-sequence rules)
/// lives at the call-site, not here — this stays generic mechanics.
pub type RequestValidator = Arc<dyn Fn(&CapturedRequest) -> Option<MockResponse> + Send + Sync>;

/// Cursor over the canned response sequence. Serves responses in arrival order;
/// once exhausted it repeats the LAST configured response instead of panicking
/// (benign TTL-capped double-fire tolerance — see `start_mock_server_capturing`
/// doc). An empty sequence with an incoming request is a genuine misconfig and
/// panics loudly.
struct ResponseSequence {
    responses: Vec<MockResponse>,
    next: usize,
}

impl ResponseSequence {
    fn pop(&mut self) -> MockResponse {
        assert!(
            !self.responses.is_empty(),
            "start_mock_server_capturing: request received but NO responses configured"
        );
        let idx = self.next.min(self.responses.len() - 1);
        self.next += 1;
        self.responses[idx].clone()
    }
}

/// `start_mock_server_capturing` variant with an optional per-request
/// `RequestValidator` (see type doc). Same capture + sequence semantics.
pub async fn start_mock_server_capturing_with_validator(
    response_sequence: Vec<MockResponse>,
    validator: Option<RequestValidator>,
) -> (SocketAddr, JoinHandle<()>, Arc<Mutex<Vec<CapturedRequest>>>) {
    start_mock_server_capturing_inner(response_sequence, validator, 0).await
}

/// `start_mock_server_capturing` variant that models a **half-dead peer**: the
/// first `hang_after_head` connections receive the response HEAD (status line +
/// `Content-Type` + `Content-Length`) and then **nothing** — the announced body
/// is never written, and the socket is held open for the lifetime of the server
/// task (no FIN, no RST, no error).
///
/// This is the shape a NAT idle-timeout or a silently blackholed path produces,
/// and it is the only shape that separates a header-phase timeout from a
/// full-operation timeout: a client that bounds only `send()` completes the
/// header phase here and then waits on the body read forever.
///
/// The request is still captured, and the hung connection still consumes one
/// entry of `response_sequence` (its head is taken from that entry) — so the
/// sequence is read the same way regardless of hanging.
pub async fn start_mock_server_capturing_hanging_body(
    hang_after_head: usize,
    response_sequence: Vec<MockResponse>,
) -> (SocketAddr, JoinHandle<()>, Arc<Mutex<Vec<CapturedRequest>>>) {
    start_mock_server_capturing_inner(response_sequence, None, hang_after_head).await
}

async fn start_mock_server_capturing_inner(
    response_sequence: Vec<MockResponse>,
    validator: Option<RequestValidator>,
    hang_after_head: usize,
) -> (SocketAddr, JoinHandle<()>, Arc<Mutex<Vec<CapturedRequest>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind failed");
    let addr = listener.local_addr().expect("local_addr failed");
    let captured: Arc<Mutex<Vec<CapturedRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_for_task = captured.clone();
    let responses = Arc::new(Mutex::new(ResponseSequence {
        responses: response_sequence,
        next: 0,
    }));
    let join = tokio::spawn(async move {
        let mut accepted = 0usize;
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(p) => p,
                Err(_) => break,
            };
            accepted += 1;
            let hang = accepted <= hang_after_head;
            let captured_for_conn = captured_for_task.clone();
            let responses_for_conn = responses.clone();
            let validator_for_conn = validator.clone();
            tokio::spawn(async move {
                let _ = handle_capturing_connection(
                    stream,
                    captured_for_conn,
                    responses_for_conn,
                    validator_for_conn,
                    hang,
                )
                .await;
            });
        }
    });
    (addr, join, captured)
}

async fn handle_connection(mut stream: TcpStream, response: MockResponse) -> std::io::Result<()> {
    let mut buf = vec![0u8; 4096];
    let mut total = 0;
    while total < buf.len() {
        let n = stream.read(&mut buf[total..]).await?;
        if n == 0 {
            break;
        }
        total += n;
        if buf[..total].windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    if let Some(d) = response.delay {
        tokio::time::sleep(d).await;
    }
    write_response(&mut stream, &response).await
}

async fn handle_capturing_connection(
    mut stream: TcpStream,
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
    responses: Arc<Mutex<ResponseSequence>>,
    validator: Option<RequestValidator>,
    hang_after_head: bool,
) -> std::io::Result<()> {
    // Read until end of header block (\r\n\r\n).
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1024];
    let header_end_idx = loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            // Connection closed before headers complete.
            break None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = find_header_end(&buf) {
            break Some(pos);
        }
    };
    let Some(header_end) = header_end_idx else {
        return Ok(());
    };

    // Parse the header block (everything before \r\n\r\n).
    let header_block = &buf[..header_end];
    let header_str = String::from_utf8_lossy(header_block).to_string();
    let mut lines = header_str.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut headers: HashMap<String, String> = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    // Content-Length-aware body read. body_start = header_end + 4 (\r\n\r\n).
    let body_start = header_end + 4;
    let content_length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let mut body = if buf.len() > body_start {
        buf[body_start..].to_vec()
    } else {
        Vec::new()
    };
    while body.len() < content_length {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);

    let request = CapturedRequest {
        method,
        path,
        headers,
        body,
    };
    // Validator runs BEFORE the capture push and the sequence pop; a rejection
    // answers the connection without consuming a canned response.
    let rejection = validator.as_ref().and_then(|v| v(&request));

    // Push capture.
    {
        let mut cap = captured.lock().await;
        cap.push(request);
    }

    // Validator rejection, or pop the next response (repeats last on exhaustion).
    let response = match rejection {
        Some(r) => r,
        None => responses.lock().await.pop(),
    };

    if let Some(d) = response.delay {
        tokio::time::sleep(d).await;
    }
    if hang_after_head {
        // Half-dead peer: head out, body never. The socket stays open (this task
        // owns it and never returns), so the client sees neither FIN nor RST — it
        // simply waits on a body that will not arrive.
        write_response_head(&mut stream, &response).await?;
        std::future::pending::<()>().await;
        return Ok(());
    }
    write_response(&mut stream, &response).await
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

async fn write_response(stream: &mut TcpStream, response: &MockResponse) -> std::io::Result<()> {
    write_response_head(stream, response).await?;
    stream.write_all(&response.body).await?;
    stream.shutdown().await?;
    Ok(())
}

/// Writes only the response head (status line + headers + blank line). The
/// announced `Content-Length` is the real body length — a client that trusts it
/// blocks until the body arrives.
async fn write_response_head(
    stream: &mut TcpStream,
    response: &MockResponse,
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        status_reason(response.status),
        response.content_type,
        response.body.len(),
    );
    stream.write_all(head.as_bytes()).await
}

fn status_reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream as StdTcpStream;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mock_server_responds_with_canned_200() {
        let (addr, _join) = start_mock_server(MockResponse::ok(b"hello world")).await;
        // Sync TCP client for hermeticity (no reqwest dep in meclaw-testing).
        let mut stream = StdTcpStream::connect(addr).unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();
        let resp = String::from_utf8_lossy(&buf);
        assert!(resp.starts_with("HTTP/1.1 200"), "expected 200, got {resp}");
        assert!(resp.contains("Content-Type: text/plain"));
        assert!(resp.contains("Content-Length: 11"));
        assert!(resp.ends_with("hello world"), "body missing in {resp}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mock_server_responds_with_canned_404() {
        let (addr, _join) = start_mock_server(MockResponse::not_found()).await;
        let mut stream = StdTcpStream::connect(addr).unwrap();
        stream
            .write_all(b"GET /nope HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();
        let resp = String::from_utf8_lossy(&buf);
        assert!(resp.starts_with("HTTP/1.1 404"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mock_server_responds_with_canned_500() {
        let (addr, _join) = start_mock_server(MockResponse::server_error()).await;
        let mut stream = StdTcpStream::connect(addr).unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();
        let resp = String::from_utf8_lossy(&buf);
        assert!(resp.starts_with("HTTP/1.1 500"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mock_server_responds_with_canned_json() {
        let (addr, _join) = start_mock_server(MockResponse::ok_json(br#"{"x":1}"#)).await;
        let mut stream = StdTcpStream::connect(addr).unwrap();
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();
        let resp = String::from_utf8_lossy(&buf);
        assert!(resp.starts_with("HTTP/1.1 200"));
        assert!(resp.contains("Content-Type: application/json"));
        assert!(resp.ends_with(r#"{"x":1}"#));
    }

    // ---------- T9: CapturedRequest + start_mock_server_capturing + with_delay ----------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn captured_request_struct_fields() {
        let r = CapturedRequest {
            method: "POST".into(),
            path: "/foo".into(),
            headers: HashMap::new(),
            body: b"hello".to_vec(),
        };
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, "/foo");
        assert_eq!(r.body, b"hello");
    }

    /// Send a raw HTTP POST with body, return the full response bytes.
    fn raw_post_sync(
        addr: SocketAddr,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Vec<u8> {
        let mut stream = StdTcpStream::connect(addr).unwrap();
        let mut req = format!("POST {path} HTTP/1.1\r\nHost: x\r\n");
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        stream.write_all(req.as_bytes()).unwrap();
        stream.write_all(body).unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();
        buf
    }

    /// Extract the body (after \r\n\r\n) from a full HTTP response byte buffer.
    fn split_body(resp: &[u8]) -> Vec<u8> {
        let pos = resp
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("no header terminator in response");
        resp[pos + 4..].to_vec()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn capturing_server_records_post_body_and_headers() {
        let (addr, _join, captured) =
            start_mock_server_capturing(vec![MockResponse::ok(b"world")]).await;
        let resp = tokio::task::spawn_blocking(move || {
            raw_post_sync(
                addr,
                "/some/path",
                &[("authorization", "Bearer test")],
                b"hello",
            )
        })
        .await
        .unwrap();
        let resp_str = String::from_utf8_lossy(&resp);
        assert!(resp_str.starts_with("HTTP/1.1 200"));
        assert_eq!(split_body(&resp), b"world".to_vec());
        let caps = captured.lock().await;
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].method, "POST");
        assert_eq!(caps[0].path, "/some/path");
        assert_eq!(caps[0].body, b"hello".to_vec());
        // Header keys normalized to lowercase.
        assert_eq!(
            caps[0].headers.get("authorization").map(|s| s.as_str()),
            Some("Bearer test")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn capturing_server_response_sequence_order() {
        let (addr, _join, _captured) = start_mock_server_capturing(vec![
            MockResponse::ok(b"first"),
            MockResponse::ok(b"second"),
        ])
        .await;
        let r1 = tokio::task::spawn_blocking(move || raw_post_sync(addr, "/x", &[], b""))
            .await
            .unwrap();
        assert_eq!(split_body(&r1), b"first".to_vec());
        let r2 = tokio::task::spawn_blocking(move || raw_post_sync(addr, "/x", &[], b""))
            .await
            .unwrap();
        assert_eq!(split_body(&r2), b"second".to_vec());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn validator_rejection_overrides_canned_response_without_consuming_it() {
        let validator: RequestValidator = Arc::new(|req: &CapturedRequest| {
            if req.body.windows(3).any(|w| w == b"bad") {
                Some(MockResponse {
                    status: 400,
                    body: b"rejected".to_vec(),
                    content_type: "text/plain; charset=utf-8".into(),
                    delay: None,
                })
            } else {
                None
            }
        });
        let (addr, _join, _captured) = start_mock_server_capturing_with_validator(
            vec![MockResponse::ok(b"canned")],
            Some(validator),
        )
        .await;
        // Invalid request → validator rejection, canned sequence untouched.
        let r1 = tokio::task::spawn_blocking(move || raw_post_sync(addr, "/x", &[], b"bad stuff"))
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&r1).starts_with("HTTP/1.1 400"));
        assert_eq!(split_body(&r1), b"rejected".to_vec());
        // Valid request afterwards still gets the (unconsumed) canned response.
        let r2 = tokio::task::spawn_blocking(move || raw_post_sync(addr, "/x", &[], b"good"))
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&r2).starts_with("HTTP/1.1 200"));
        assert_eq!(split_body(&r2), b"canned".to_vec());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn with_delay_holds_back_response() {
        let delay = Duration::from_millis(150);
        let (addr, _join, _captured) =
            start_mock_server_capturing(vec![MockResponse::ok(b"slow").with_delay(delay)]).await;
        let started = std::time::Instant::now();
        let resp = tokio::task::spawn_blocking(move || raw_post_sync(addr, "/x", &[], b""))
            .await
            .unwrap();
        let elapsed = started.elapsed();
        assert_eq!(split_body(&resp), b"slow".to_vec());
        assert!(elapsed >= delay, "delay={:?}, elapsed={:?}", delay, elapsed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn with_delay_also_works_on_plain_start_mock_server() {
        // Sanity: the delay knob is generic — start_mock_server honors it too.
        let delay = Duration::from_millis(100);
        let (addr, _join) = start_mock_server(MockResponse::ok(b"slowget").with_delay(delay)).await;
        let started = std::time::Instant::now();
        let resp = tokio::task::spawn_blocking(move || {
            let mut stream = StdTcpStream::connect(addr).unwrap();
            stream
                .write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
                .unwrap();
            let mut buf = Vec::new();
            stream.read_to_end(&mut buf).unwrap();
            buf
        })
        .await
        .unwrap();
        let elapsed = started.elapsed();
        assert_eq!(split_body(&resp), b"slowget".to_vec());
        assert!(elapsed >= delay, "delay={:?}, elapsed={:?}", delay, elapsed);
    }
}
