//! Wave voice-cell (t3a) — hermetic fake Cartesia TTS WebSocket for tests.
//!
//! Same shape as [`crate::mock_slack`]: a `TcpListener` on `127.0.0.1:0`, the
//! kernel picks the port, everything runs in a background task. The real
//! endpoint is `wss://api.cartesia.ai/tts/websocket`, authenticated with the
//! `X-API-Key` and `Cartesia-Version` headers (the `api_key`/`cartesia_version`
//! query parameters are the documented browser alternative); this fake answers
//! on `ws://127.0.0.1:<port>/tts/websocket` with the same frames and records
//! both forms, so the adapter under test speaks the real protocol and only the
//! `base_url` param differs.
//!
//! Protocol implemented here, from the official documentation
//! (<https://docs.cartesia.ai/api-reference/tts/tts>, read 2026-09-05):
//!
//! - the client sends one JSON object per synthesis, carrying `context_id`,
//!   `model_id`, `transcript`, `voice`, `output_format{container,encoding,sample_rate}`,
//!   `language`, optional `generation_config{speed,emotion}` and `continue`;
//! - the server answers with `{"type":"chunk","data":<base64>,"done":false,…}`
//!   frames, then exactly one `{"type":"done","done":true,…}`;
//! - a failure arrives as `{"type":"error","done":true,"title","message",…}`;
//! - `{"context_id":…,"cancel":true}` stops an in-flight generation.
//!
//! Where the documentation is silent, the behaviour is a knob rather than a
//! guess: the pause between two chunks, the point at which the stream turns
//! into an error, whether the upgrade is refused with a plain HTTP status, and
//! how long the handshake stalls are all script parameters. A test therefore
//! states which unverified assumption it exercises instead of the fake baking
//! one in as truth. In particular the documentation does not say whether a
//! `done` follows a cancel — this fake sends none, because the cancelling side
//! has already stopped reading.

use futures_util::{SinkExt, StreamExt};
use meclaw_core::serde_json::{Value as JsonValue, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// What the fake replays for every synthesis request on a connection.
#[derive(Clone, Debug, Default)]
pub struct CartesiaScript {
    /// Audio chunks, in order. Each one is sent base64-encoded in a `chunk`
    /// frame; the bytes are whatever the test wants to see arrive.
    pub chunks: Vec<Vec<u8>>,
    /// Pause before each chunk. Zero streams everything at once, which makes a
    /// cancel race the stream; a test that cancels mid-stream sets a delay.
    pub chunk_delay: Duration,
    /// Send an `error` frame instead of the n-th chunk (0 = fail immediately),
    /// and no `done`.
    pub fail_after: Option<usize>,
    /// Refuse the upgrade with this HTTP status and a JSON body instead of
    /// completing the handshake — how the real endpoint rejects a bad key.
    pub http_status: Option<u16>,
    /// Stall this long before answering the handshake, to drive connect
    /// timeouts.
    pub accept_delay: Option<Duration>,
    /// Go quiet after the n-th chunk: no more frames, no `done`, socket stays
    /// open. The documentation says nothing about a provider that simply stops
    /// talking, so it is a knob — this is what an idle deadline is for.
    pub silent_after: Option<usize>,
    /// `status_code` of a scripted `error` frame. `None` = 500. A test that
    /// wants an auth verdict out of a frame (rather than out of the handshake)
    /// sets 401 here.
    pub error_status: Option<u16>,
}

impl CartesiaScript {
    /// An empty script: the handshake succeeds and every request is answered
    /// with `done` and no audio at all.
    pub fn new() -> Self {
        Self::default()
    }
    /// Queue one audio chunk.
    pub fn chunk(mut self, bytes: &[u8]) -> Self {
        self.chunks.push(bytes.to_vec());
        self
    }
    /// Queue several audio chunks at once.
    pub fn with_chunks(mut self, chunks: Vec<Vec<u8>>) -> Self {
        self.chunks.extend(chunks);
        self
    }
    /// Pause this long before each chunk.
    pub fn with_chunk_delay(mut self, d: Duration) -> Self {
        self.chunk_delay = d;
        self
    }
    /// Turn the stream into an `error` frame in place of the n-th chunk.
    pub fn failing_after(mut self, n: usize) -> Self {
        self.fail_after = Some(n);
        self
    }
    /// Refuse the upgrade with this HTTP status.
    pub fn refusing_with_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }
    /// Hold the handshake back for this long.
    pub fn with_accept_delay(mut self, d: Duration) -> Self {
        self.accept_delay = Some(d);
        self
    }
    /// Fall silent after the n-th chunk, leaving the socket open.
    pub fn going_silent_after(mut self, n: usize) -> Self {
        self.silent_after = Some(n);
        self
    }
    /// Give a scripted `error` frame this `status_code`.
    pub fn with_error_status(mut self, status: u16) -> Self {
        self.error_status = Some(status);
        self
    }
}

#[derive(Default)]
struct State {
    script: CartesiaScript,
    requests: Vec<JsonValue>,
    cancels: Vec<String>,
    queries: Vec<String>,
    headers: Vec<HashMap<String, String>>,
    connections: usize,
}

/// A running fake Cartesia. Dropping it stops the server.
pub struct MockCartesia {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockCartesia {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockCartesia {
    /// Binds and starts serving `script`. Returns once the port is known.
    pub async fn start(script: CartesiaScript) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let state = Arc::new(Mutex::new(State {
            script,
            ..State::default()
        }));
        let st = state.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let st = st.clone();
                tokio::spawn(async move {
                    serve_conn(stream, st).await;
                });
            }
        });
        Ok(Self { addr, state, task })
    }

    /// Base URL for the cell's `base_url` param, e.g. `ws://127.0.0.1:54321`.
    /// The adapter appends `/tts/websocket` and the query itself, exactly as it
    /// does against the real host.
    pub fn base_url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    /// The synthesis requests the fake parsed, in arrival order. Cancels are
    /// not in here — they are in [`Self::cancels`].
    pub async fn received_requests(&self) -> Vec<JsonValue> {
        self.state.lock().await.requests.clone()
    }

    /// The `context_id`s that were cancelled, in arrival order.
    pub async fn cancels(&self) -> Vec<String> {
        self.state.lock().await.cancels.clone()
    }

    /// Whether any cancel arrived at all.
    pub async fn cancelled(&self) -> bool {
        !self.state.lock().await.cancels.is_empty()
    }

    /// The raw query string of each handshake.
    pub async fn queries(&self) -> Vec<String> {
        self.state.lock().await.queries.clone()
    }

    /// The request headers of each handshake, names lower-cased — the proof
    /// that credentials and API version travel where the documentation puts
    /// them (`X-API-Key`, `Cartesia-Version`).
    pub async fn headers(&self) -> Vec<HashMap<String, String>> {
        self.state.lock().await.headers.clone()
    }

    /// How many WebSocket connections were established. One synthesis opens
    /// exactly one, which is what the adapter promises.
    pub async fn connections(&self) -> usize {
        self.state.lock().await.connections
    }
}

/// Refuses with a plain HTTP response, or upgrades and replays the script.
async fn serve_conn(stream: TcpStream, state: Arc<Mutex<State>>) {
    let script = state.lock().await.script.clone();
    if let Some(delay) = script.accept_delay {
        tokio::time::sleep(delay).await;
    }
    if let Some(status) = script.http_status {
        refuse_http(stream, status).await;
        return;
    }
    serve_ws(stream, state, script).await;
}

/// Answers the upgrade request with a status and a JSON body, then closes —
/// the shape a rejected key produces before any WebSocket exists.
async fn refuse_http(mut stream: TcpStream, status: u16) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    loop {
        match stream.read(&mut tmp).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
        if find_subslice(&buf, b"\r\n\r\n").is_some() {
            break;
        }
    }
    let body = json!({ "error": "fake cartesia refused the upgrade" }).to_string();
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
         Content-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        reason = reason_phrase(status),
        len = body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.flush().await;
}

/// Upgrades, then answers every synthesis request with chunks and a `done`.
///
/// The `result_large_err` allow is not laziness: the handshake callback's
/// signature is fixed by tungstenite (`Result<Response, ErrorResponse>`), so
/// the error type is not ours to box.
#[allow(clippy::result_large_err)]
async fn serve_ws(stream: TcpStream, state: Arc<Mutex<State>>, script: CartesiaScript) {
    let mut query = String::new();
    let mut headers = HashMap::new();
    let ws = match tokio_tungstenite::accept_hdr_async(stream, |req: &_, resp| {
        query = uri_query(req);
        headers = request_headers(req);
        Ok(resp)
    })
    .await
    {
        Ok(ws) => ws,
        Err(_) => return,
    };
    {
        let mut st = state.lock().await;
        st.queries.push(query);
        st.headers.push(headers);
        st.connections += 1;
    }

    let (mut write, mut read) = ws.split();
    // A cancel must be able to interrupt a stream that is mid-flight, so the
    // reader runs beside the writer instead of before it. `notify_one` stores
    // one permit, so a cancel that lands while a chunk is being written is not
    // lost — the next wait returns immediately.
    let cancel = Arc::new(Notify::new());
    let (req_tx, mut req_rx) = mpsc::channel::<JsonValue>(16);
    let read_state = state.clone();
    let read_cancel = cancel.clone();
    let reader = tokio::spawn(async move {
        while let Some(Ok(msg)) = read.next().await {
            let WsMessage::Text(txt) = msg else { continue };
            let Ok(v) = meclaw_core::serde_json::from_str::<JsonValue>(&txt) else {
                continue;
            };
            if v.get("cancel").and_then(JsonValue::as_bool) == Some(true) {
                let ctx = v
                    .get("context_id")
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default()
                    .to_string();
                read_state.lock().await.cancels.push(ctx);
                read_cancel.notify_one();
                continue;
            }
            read_state.lock().await.requests.push(v.clone());
            if req_tx.send(v).await.is_err() {
                break;
            }
        }
    });

    while let Some(req) = req_rx.recv().await {
        let context_id = req
            .get("context_id")
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_string();
        if !stream_one(&mut write, &script, &context_id, &cancel, &state).await {
            break;
        }
    }
    let _ = reader.await;
}

/// Replays the script for one request. Returns `false` when the socket died.
async fn stream_one<S>(
    write: &mut S,
    script: &CartesiaScript,
    context_id: &str,
    cancel: &Notify,
    state: &Arc<Mutex<State>>,
) -> bool
where
    S: SinkExt<WsMessage> + Unpin,
{
    for (i, chunk) in script.chunks.iter().enumerate() {
        if script.fail_after == Some(i) {
            return send_json(
                write,
                error_frame(context_id, script.error_status.unwrap_or(500)),
            )
            .await;
        }
        if script.silent_after == Some(i) {
            // Nothing more, not even `done`. The socket stays open, so only a
            // deadline on the client's side can end this.
            return true;
        }
        if !script.chunk_delay.is_zero() {
            tokio::select! {
                _ = tokio::time::sleep(script.chunk_delay) => {}
                // Only a wake-up, never a verdict — the per-context check below
                // decides whether this stream is the one being cancelled.
                _ = cancel.notified() => {}
            }
        }
        // Belt and braces, and per context: the notify only wakes early, the
        // decision is whether THIS context was cancelled. A cancel for another
        // context on the same socket must not silence this one.
        if state.lock().await.cancels.iter().any(|c| c == context_id) {
            return true;
        }
        let frame = json!({
            "type": "chunk",
            "data": encode_base64(chunk),
            "done": false,
            "status_code": 206,
            "context_id": context_id,
        });
        if !send_json(write, frame).await {
            return false;
        }
    }
    if script.fail_after == Some(script.chunks.len()) {
        return send_json(
            write,
            error_frame(context_id, script.error_status.unwrap_or(500)),
        )
        .await;
    }
    if script.silent_after == Some(script.chunks.len()) {
        return true;
    }
    send_json(
        write,
        json!({
            "type": "done",
            "done": true,
            "status_code": 206,
            "context_id": context_id,
        }),
    )
    .await
}

fn error_frame(context_id: &str, status: u16) -> JsonValue {
    json!({
        "type": "error",
        "done": true,
        "title": "Scripted failure",
        "message": "the fake was told to fail here",
        "error_code": "internal_error",
        "status_code": status,
        "context_id": context_id,
    })
}

async fn send_json<S>(write: &mut S, value: JsonValue) -> bool
where
    S: SinkExt<WsMessage> + Unpin,
{
    write
        .send(WsMessage::Text(value.to_string().into()))
        .await
        .is_ok()
}

fn uri_query(req: &tokio_tungstenite::tungstenite::handshake::server::Request) -> String {
    req.uri().query().unwrap_or_default().to_string()
}

fn request_headers(
    req: &tokio_tungstenite::tungstenite::handshake::server::Request,
) -> HashMap<String, String> {
    req.headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.as_str().to_ascii_lowercase(), v.to_string()))
        })
        .collect()
}

/// The reason phrase of a status line. A wrong one is not a protocol error,
/// but a client that logs the status line should not read `401 OK`.
fn reason_phrase(status: u16) -> &'static str {
    match status {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Standard Base64 with padding — what Cartesia puts in `chunk.data`.
///
/// A local six-liner rather than a dependency: the allow-list has no Base64
/// crate, and the decoding half already exists in `meclaw-cells`
/// (`store::query::hamming::decode_base64`), so the two are tested against each
/// other by the adapter's round-trip test.
fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let b0 = u32::from(group[0]);
        let b1 = group.get(1).copied().map_or(0, u32::from);
        let b2 = group.get(2).copied().map_or(0, u32::from);
        let acc = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(acc >> 18) as usize & 63] as char);
        out.push(ALPHABET[(acc >> 12) as usize & 63] as char);
        out.push(if group.len() > 1 {
            ALPHABET[(acc >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if group.len() > 2 {
            ALPHABET[acc as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    #[test]
    fn base64_encodes_every_residue_class() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foob"), "Zm9vYg==");
        assert_eq!(encode_base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
        // The high bits exercise the two characters outside [A-Za-z0-9].
        assert_eq!(encode_base64(&[0xFF, 0xFF, 0xFE]), "///+");
    }

    /// Connects without the `connect` feature: the TCP side is ours, only the
    /// handshake comes from tungstenite.
    async fn connect(
        mock: &MockCartesia,
    ) -> tokio_tungstenite::WebSocketStream<tokio::net::TcpStream> {
        let stream = TcpStream::connect(mock.addr).await.expect("tcp connect");
        let url = format!(
            "{}/tts/websocket?api_key=k&cartesia_version=2026-08-14",
            mock.base_url()
        );
        let req = url.into_client_request().expect("request");
        let (ws, _) = tokio_tungstenite::client_async(req, stream)
            .await
            .expect("handshake");
        ws
    }

    fn request(context_id: &str) -> String {
        json!({
            "context_id": context_id,
            "model_id": "sonic-3.6",
            "transcript": "hallo",
            "voice": { "mode": "id", "id": "v" },
            "output_format": { "container": "raw", "encoding": "pcm_s16le", "sample_rate": 24000 },
            "language": "de",
            "continue": false,
        })
        .to_string()
    }

    #[tokio::test]
    async fn fake_streams_scripted_chunks_then_done() {
        let mock = MockCartesia::start(CartesiaScript::new().chunk(b"\x01\x02").chunk(b"\x03"))
            .await
            .expect("start");
        let mut ws = connect(&mock).await;
        ws.send(WsMessage::Text(request("c1").into()))
            .await
            .expect("send");

        let mut datas = Vec::new();
        let mut saw_done = false;
        while let Some(Ok(WsMessage::Text(t))) = ws.next().await {
            let v: JsonValue = meclaw_core::serde_json::from_str(&t).expect("json");
            match v["type"].as_str() {
                Some("chunk") => datas.push(v["data"].as_str().unwrap_or_default().to_string()),
                Some("done") => {
                    saw_done = true;
                    break;
                }
                _ => panic!("unexpected frame {v}"),
            }
        }
        assert!(saw_done, "the stream must end with a done frame");
        assert_eq!(
            datas,
            vec![encode_base64(b"\x01\x02"), encode_base64(b"\x03")]
        );

        let reqs = mock.received_requests().await;
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0]["model_id"], "sonic-3.6");
        assert_eq!(mock.connections().await, 1);
        assert_eq!(
            mock.queries().await,
            vec!["api_key=k&cartesia_version=2026-08-14".to_string()]
        );
    }

    #[tokio::test]
    async fn fake_records_cancel_and_stops_streaming() {
        let mock = MockCartesia::start(
            CartesiaScript::new()
                .chunk(b"\x01")
                .chunk(b"\x02")
                .chunk(b"\x03")
                .with_chunk_delay(Duration::from_millis(50)),
        )
        .await
        .expect("start");
        let mut ws = connect(&mock).await;
        ws.send(WsMessage::Text(request("c1").into()))
            .await
            .expect("send");
        // One chunk, then cancel: the remaining two must never arrive.
        let first = ws.next().await.expect("frame").expect("ok");
        assert!(matches!(first, WsMessage::Text(_)));
        ws.send(WsMessage::Text(
            json!({ "context_id": "c1", "cancel": true })
                .to_string()
                .into(),
        ))
        .await
        .expect("send cancel");

        let quiet = tokio::time::timeout(Duration::from_millis(300), ws.next()).await;
        assert!(
            quiet.is_err(),
            "a cancelled stream sends nothing more, not even done: {quiet:?}"
        );
        assert!(mock.cancelled().await);
        assert_eq!(mock.cancels().await, vec!["c1".to_string()]);
    }

    #[tokio::test]
    async fn fake_can_fail_the_stream_after_n_chunks() {
        let mock = MockCartesia::start(
            CartesiaScript::new()
                .chunk(b"\x01")
                .chunk(b"\x02")
                .failing_after(1),
        )
        .await
        .expect("start");
        let mut ws = connect(&mock).await;
        ws.send(WsMessage::Text(request("c1").into()))
            .await
            .expect("send");
        let mut types = Vec::new();
        while let Some(Ok(WsMessage::Text(t))) = ws.next().await {
            let v: JsonValue = meclaw_core::serde_json::from_str(&t).expect("json");
            let ty = v["type"].as_str().unwrap_or_default().to_string();
            let last = ty == "error";
            types.push(ty);
            if last {
                break;
            }
        }
        assert_eq!(types, vec!["chunk".to_string(), "error".to_string()]);
    }

    #[tokio::test]
    async fn fake_can_refuse_the_upgrade_with_a_status() {
        let mock = MockCartesia::start(CartesiaScript::new().refusing_with_status(401))
            .await
            .expect("start");
        let stream = TcpStream::connect(mock.addr).await.expect("tcp connect");
        let req = format!("{}/tts/websocket", mock.base_url())
            .into_client_request()
            .expect("request");
        let err = match tokio_tungstenite::client_async(req, stream).await {
            Err(e) => e,
            Ok(_) => panic!("a refused upgrade must not produce a WebSocket"),
        };
        assert!(
            format!("{err}").contains("401"),
            "the refusal must carry the status: {err}"
        );
    }
}
