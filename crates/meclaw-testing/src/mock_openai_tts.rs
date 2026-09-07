//! Wave voice-cell (2026-09-05), strand t3b — hermetic fake for OpenAI's
//! streaming text-to-speech endpoint `POST /v1/audio/speech`.
//!
//! Same shape as `mock_http`: a `TcpListener` on `127.0.0.1:0`, the kernel picks
//! the port, everything runs in a background task, one connection per request.
//! The difference that earns a file of its own is the body: the real endpoint
//! answers `response_format: "pcm"` with raw 24 kHz signed 16-bit little-endian
//! mono samples, headerless, delivered with chunked transfer encoding so audio
//! can play before synthesis is finished
//! (<https://developers.openai.com/api/docs/guides/text-to-speech>). A canned
//! `Content-Length` body cannot express that: a client streaming the response
//! would see one chunk no matter how the server behaved, and every assertion
//! about ordering, cancellation mid-stream or per-chunk liveness would pass by
//! construction.
//!
//! So this fake writes real chunked framing, one script chunk per HTTP chunk,
//! with an optional pause between them. What the script cannot decide, it does
//! not guess: the HTTP status, the error body, the delay before the response
//! head and the delay between chunks are all knobs, so a test states which
//! server behaviour it exercises instead of hard-coding one as truth.
//!
//! The request is captured verbatim (path, `Authorization`, parsed JSON body),
//! which is what makes "the adapter really asked for `pcm`" provable rather
//! than assumed.

use meclaw_core::serde_json::{Value as JsonValue, json};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

/// A captured call to `POST /v1/audio/speech`.
#[derive(Clone, Debug)]
pub struct CapturedSpeechRequest {
    /// HTTP method, e.g. `POST`.
    pub method: String,
    /// Request path including any query string, e.g. `/v1/audio/speech`.
    pub path: String,
    /// The `Authorization` header value, absent when the caller sent none.
    pub authorization: Option<String>,
    /// The `Content-Type` header value, absent when the caller sent none.
    pub content_type: Option<String>,
    /// Request body parsed as JSON; `JsonValue::Null` when it was not JSON.
    pub body: JsonValue,
}

impl CapturedSpeechRequest {
    /// The `model` field of the request body.
    pub fn model(&self) -> Option<&str> {
        self.body.get("model").and_then(|v| v.as_str())
    }
    /// The `input` field — the text to synthesize.
    pub fn input(&self) -> Option<&str> {
        self.body.get("input").and_then(|v| v.as_str())
    }
    /// The `voice` field.
    pub fn voice(&self) -> Option<&str> {
        self.body.get("voice").and_then(|v| v.as_str())
    }
    /// The `response_format` field — `pcm` is what the voice cell asks for.
    pub fn response_format(&self) -> Option<&str> {
        self.body.get("response_format").and_then(|v| v.as_str())
    }
}

/// One scripted server behaviour, replayed for every request.
///
/// Default: `200 OK`, no chunks, no delays — an empty but well-formed stream.
#[derive(Clone, Debug)]
pub struct OpenAiTtsScript {
    /// Audio chunks, one HTTP chunk each, written in order.
    pub chunks: Vec<Vec<u8>>,
    /// Pause between two chunks. `None` writes them back to back.
    pub chunk_delay: Option<Duration>,
    /// Pause before the response head is written at all — the shape a slow
    /// first byte has, and the only way to drive a first-response timeout.
    pub head_delay: Option<Duration>,
    /// HTTP status of the response. Anything but 200 is answered with
    /// `error_body` and no chunked framing, the way the real API reports.
    pub status: u16,
    /// Body served with a non-200 status. Defaults to an OpenAI-shaped error.
    pub error_body: Option<String>,
    /// Stop after this many chunks and close the socket without the
    /// terminating zero-length chunk — a truncated stream, not a clean end.
    /// Setting this at all means the terminator is omitted, `n` larger than
    /// the chunk count included: "serve everything and then vanish" is a real
    /// server failure, and a knob that silently turned into a clean stream at
    /// `n >= chunks.len()` would quietly stop testing what it names.
    pub truncate_after: Option<usize>,
}

impl Default for OpenAiTtsScript {
    fn default() -> Self {
        Self {
            chunks: Vec::new(),
            chunk_delay: None,
            head_delay: None,
            status: 200,
            error_body: None,
            truncate_after: None,
        }
    }
}

impl OpenAiTtsScript {
    /// An empty successful script.
    pub fn new() -> Self {
        Self::default()
    }
    /// Serve these audio chunks, one HTTP chunk each, in order.
    pub fn with_chunks(mut self, chunks: Vec<Vec<u8>>) -> Self {
        self.chunks = chunks;
        self
    }
    /// Append one audio chunk.
    pub fn chunk(mut self, bytes: &[u8]) -> Self {
        self.chunks.push(bytes.to_vec());
        self
    }
    /// Pause this long between two chunks.
    pub fn with_chunk_delay(mut self, d: Duration) -> Self {
        self.chunk_delay = Some(d);
        self
    }
    /// Hold the response head back this long before writing anything.
    pub fn with_head_delay(mut self, d: Duration) -> Self {
        self.head_delay = Some(d);
        self
    }
    /// Answer with this status and body instead of an audio stream.
    pub fn with_status(mut self, status: u16, body: &str) -> Self {
        self.status = status;
        self.error_body = Some(body.to_string());
        self
    }
    /// Cut the stream after `n` chunks: no terminating chunk, socket closed.
    /// `n >= chunks.len()` serves every chunk and still omits the terminator.
    pub fn with_truncate_after(mut self, n: usize) -> Self {
        self.truncate_after = Some(n);
        self
    }
}

#[derive(Default)]
struct State {
    requests: Vec<CapturedSpeechRequest>,
}

/// A running fake OpenAI speech endpoint. Dropping it stops the server.
pub struct MockOpenAiTts {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockOpenAiTts {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockOpenAiTts {
    /// Binds `127.0.0.1:0` and starts serving `script`. Returns once the port
    /// is known, so a test can hand `base_url()` to the cell immediately.
    pub async fn start(script: OpenAiTtsScript) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let state = Arc::new(Mutex::new(State::default()));
        let st = state.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let st = st.clone();
                let script = script.clone();
                tokio::spawn(async move {
                    let _ = serve_conn(stream, st, script).await;
                });
            }
        });
        Ok(Self { addr, state, task })
    }

    /// API base without a trailing slash, e.g. `http://127.0.0.1:54321`. Feed
    /// this to the provider's `base_url` param; the adapter appends the path.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The address the fake listens on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Captured requests, in arrival order.
    pub async fn requests(&self) -> Vec<CapturedSpeechRequest> {
        self.state.lock().await.requests.clone()
    }
}

/// Minimal HTTP/1.1: read the head, read the body by `Content-Length`, capture
/// it, then answer per script.
async fn serve_conn(
    mut stream: TcpStream,
    state: Arc<Mutex<State>>,
    script: OpenAiTtsScript,
) -> std::io::Result<()> {
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut tmp = [0u8; 1024];
    let head_end = loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut authorization = None;
    let mut content_type = None;
    let mut content_length = 0usize;
    for line in lines {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim().to_string());
        match k.as_str() {
            "authorization" => authorization = Some(v),
            "content-type" => content_type = Some(v),
            "content-length" => content_length = v.parse().unwrap_or(0),
            _ => {}
        }
    }

    let body_start = head_end + 4;
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

    let parsed = meclaw_core::serde_json::from_slice::<JsonValue>(&body).unwrap_or(JsonValue::Null);
    state.lock().await.requests.push(CapturedSpeechRequest {
        method,
        path,
        authorization,
        content_type,
        body: parsed,
    });

    if let Some(d) = script.head_delay {
        tokio::time::sleep(d).await;
    }

    if script.status != 200 {
        let payload = script.error_body.clone().unwrap_or_else(|| {
            json!({"error": {"message": "mock failure", "type": "invalid_request_error"}})
                .to_string()
        });
        let resp = format!(
            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            script.status,
            reason(script.status),
            payload.len(),
            payload,
        );
        stream.write_all(resp.as_bytes()).await?;
        stream.shutdown().await?;
        return Ok(());
    }

    // Chunked transfer encoding: this is what makes the stream a stream.
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: audio/pcm\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        )
        .await?;
    stream.flush().await?;

    let truncating = script.truncate_after.is_some();
    let limit = script.truncate_after.unwrap_or(script.chunks.len());
    for (i, chunk) in script.chunks.iter().enumerate() {
        if i >= limit {
            // Truncated stream: no terminating chunk, the socket just ends.
            return Ok(());
        }
        if i > 0
            && let Some(d) = script.chunk_delay
        {
            tokio::time::sleep(d).await;
        }
        stream
            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
            .await?;
        stream.write_all(chunk).await?;
        stream.write_all(b"\r\n").await?;
        stream.flush().await?;
    }
    if truncating {
        // Every chunk served, terminator still withheld: the socket just ends.
        return Ok(());
    }
    stream.write_all(b"0\r\n\r\n").await?;
    stream.shutdown().await?;
    Ok(())
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        _ => "Error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream as StdTcpStream;

    /// Failure-marker timeout for the blocking client calls below: generous
    /// against cargo-parallel load, finite so a hung fake fails as a test.
    const MARKER: Duration = Duration::from_secs(30);

    /// Run `raw_post` on a blocking thread under the failure marker.
    async fn post(addr: SocketAddr, path: &'static str, body: &'static [u8]) -> Vec<u8> {
        post_with(addr, path, &[], body).await
    }

    /// `post` with request headers.
    async fn post_with(
        addr: SocketAddr,
        path: &'static str,
        headers: &'static [(&'static str, &'static str)],
        body: &'static [u8],
    ) -> Vec<u8> {
        tokio::time::timeout(
            MARKER,
            tokio::task::spawn_blocking(move || raw_post(addr, path, headers, body)),
        )
        .await
        .expect("mock did not answer within the failure marker")
        .expect("join")
    }

    /// Send a raw HTTP POST, return the full response bytes.
    fn raw_post(addr: SocketAddr, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
        let mut stream = StdTcpStream::connect(addr).expect("connect");
        let mut req = format!("POST {path} HTTP/1.1\r\nHost: x\r\n");
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        req.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        stream.write_all(req.as_bytes()).expect("write head");
        stream.write_all(body).expect("write body");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).expect("read");
        buf
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn streams_script_chunks_with_chunked_framing() {
        let mock = MockOpenAiTts::start(
            OpenAiTtsScript::new()
                .chunk(&[1u8, 2, 3, 4])
                .chunk(&[5u8, 6]),
        )
        .await
        .expect("start");
        let addr = mock.addr();
        let resp = post(addr, "/v1/audio/speech", br#"{"model":"m"}"#).await;
        let text = String::from_utf8_lossy(&resp);
        assert!(text.starts_with("HTTP/1.1 200 OK"), "head: {text}");
        assert!(
            text.contains("Transfer-Encoding: chunked"),
            "no chunked framing: {text}"
        );
        // Two chunks, then the terminator.
        let pos = resp
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("head terminator");
        let body = &resp[pos + 4..];
        assert_eq!(body, b"4\r\n\x01\x02\x03\x04\r\n2\r\n\x05\x06\r\n0\r\n\r\n");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn captures_the_request_json_and_authorization() {
        let mock = MockOpenAiTts::start(OpenAiTtsScript::new())
            .await
            .expect("start");
        let addr = mock.addr();
        let body = br#"{"model":"m","input":"hi","voice":"alloy","response_format":"pcm"}"#;
        post_with(
            addr,
            "/v1/audio/speech",
            &[
                ("Authorization", "Bearer test-token"),
                ("Content-Type", "application/json"),
            ],
            body,
        )
        .await;
        let reqs = mock.requests().await;
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, "POST");
        assert_eq!(reqs[0].path, "/v1/audio/speech");
        assert_eq!(reqs[0].authorization.as_deref(), Some("Bearer test-token"));
        assert_eq!(reqs[0].content_type.as_deref(), Some("application/json"));
        assert_eq!(reqs[0].model(), Some("m"));
        assert_eq!(reqs[0].input(), Some("hi"));
        assert_eq!(reqs[0].voice(), Some("alloy"));
        assert_eq!(reqs[0].response_format(), Some("pcm"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scripted_status_replaces_the_stream() {
        let mock = MockOpenAiTts::start(
            OpenAiTtsScript::new()
                .chunk(&[9u8])
                .with_status(401, r#"{"error":{"message":"bad key"}}"#),
        )
        .await
        .expect("start");
        let addr = mock.addr();
        let resp = post(addr, "/v1/audio/speech", br#"{"model":"m"}"#).await;
        let text = String::from_utf8_lossy(&resp);
        assert!(
            text.starts_with("HTTP/1.1 401 Unauthorized"),
            "head: {text}"
        );
        assert!(
            text.ends_with(r#"{"error":{"message":"bad key"}}"#),
            "{text}"
        );
        assert!(
            !text.contains("chunked"),
            "error path must not stream: {text}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn chunk_delay_spaces_the_chunks_out() {
        let delay = Duration::from_millis(120);
        let mock = MockOpenAiTts::start(
            OpenAiTtsScript::new()
                .chunk(&[1u8])
                .chunk(&[2u8])
                .with_chunk_delay(delay),
        )
        .await
        .expect("start");
        let addr = mock.addr();
        let started = std::time::Instant::now();
        post(addr, "/v1/audio/speech", b"{}").await;
        assert!(
            started.elapsed() >= delay,
            "elapsed {:?} < delay {:?}",
            started.elapsed(),
            delay
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn head_delay_holds_the_response_back() {
        let delay = Duration::from_millis(120);
        let mock =
            MockOpenAiTts::start(OpenAiTtsScript::new().chunk(&[7u8]).with_head_delay(delay))
                .await
                .expect("start");
        let addr = mock.addr();
        let started = std::time::Instant::now();
        let resp = post(addr, "/v1/audio/speech", b"{}").await;
        assert!(
            started.elapsed() >= delay,
            "elapsed {:?}",
            started.elapsed()
        );
        assert!(String::from_utf8_lossy(&resp).starts_with("HTTP/1.1 200"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn truncate_after_omits_the_terminating_chunk() {
        let mock = MockOpenAiTts::start(
            OpenAiTtsScript::new()
                .chunk(&[1u8])
                .chunk(&[2u8])
                .with_truncate_after(1),
        )
        .await
        .expect("start");
        let addr = mock.addr();
        let resp = post(addr, "/v1/audio/speech", b"{}").await;
        let pos = resp
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("head terminator");
        let body = &resp[pos + 4..];
        assert_eq!(body, b"1\r\n\x01\r\n");
    }
}
