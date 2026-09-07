//! GH #591 — hermetic fake ElevenLabs text-to-speech WebSocket for tests.
//!
//! Same shape as [`crate::mock_cartesia`]: a `TcpListener` on `127.0.0.1:0`,
//! the kernel picks the port, everything runs in a background task. The real
//! endpoint is
//! `wss://api.elevenlabs.io/v1/text-to-speech/{voice_id}/stream-input`,
//! authenticated with the `xi-api-key` header; this fake answers the same path
//! on `ws://127.0.0.1:<port>` with the same frames, so the adapter under test
//! speaks the real protocol and only the `base_url` param differs.
//!
//! Protocol implemented here, from the official documentation
//! (<https://elevenlabs.io/docs/api-reference/text-to-speech/v-1-text-to-speech-voice-id-stream-input>
//! and <https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/realtime-tts>,
//! both read 2026-09-06):
//!
//! - the client connects to `/v1/text-to-speech/{voice_id}/stream-input` with
//!   `model_id` and `output_format` in the query and the credential in the
//!   `xi-api-key` header;
//! - the first client message initialises the stream — `{"text": " ",
//!   "voice_settings": {…}, "generation_config": {…}}`;
//! - every further message carries text — `{"text": "…"}`;
//! - `{"text": ""}` ends the input stream;
//! - the server answers with `{"audio": <base64>, "isFinal": false,
//!   "alignment": …}` frames and one final `{"isFinal": true}`.
//!
//! # What this fake never stores
//!
//! Header **names** are recorded, header values are not. The one header that
//! matters here carries a credential, and a fake that keeps it invites a test
//! to assert on it — at which point the credential is in the test source, the
//! assertion output and any failure log. Presence of the name plus an empty
//! query string is the whole proof the adapter owes: the key travels in a
//! header, and never in a URL that a transport error could quote.
//!
//! # Where the documentation is silent, there is a knob
//!
//! The pause between two chunks, the point at which the stream turns into an
//! error, whether the upgrade is refused with a plain HTTP status, how long the
//! handshake stalls and whether the server simply goes quiet are all script
//! parameters rather than baked-in behaviour. Two more choices are the fake's
//! own and are named here because the documentation does not settle them:
//!
//! - **When generation starts.** The real service begins generating once its
//!   chunk schedule is satisfied or the input stream is closed. This fake
//!   always starts on the end-of-stream message, which is the later of the two
//!   — an adapter that works against this one works against an earlier start
//!   too, since a chunk arriving sooner is only a chunk arriving sooner.
//! - **What follows a client-side close.** The single-context `stream-input`
//!   protocol has no cancel message at all, so a caller that wants no more
//!   audio closes the socket. This fake records the close and stops; nothing
//!   follows it.

use futures_util::{SinkExt, StreamExt};
use meclaw_core::serde_json::{Value as JsonValue, json};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// What the fake replays once a synthesis input stream is closed.
#[derive(Clone, Debug, Default)]
pub struct ElevenLabsScript {
    /// Audio chunks, in order. Each one is sent base64-encoded as the `audio`
    /// field of a frame; the bytes are whatever the test wants to see arrive.
    pub chunks: Vec<Vec<u8>>,
    /// Pause before each chunk. Zero streams everything at once, which makes a
    /// cancel race the stream; a test that cancels mid-stream sets a delay.
    pub chunk_delay: Duration,
    /// Send an error frame instead of the n-th chunk (0 = fail immediately),
    /// and no final frame.
    pub fail_after: Option<usize>,
    /// Refuse the upgrade with this HTTP status and a JSON body instead of
    /// completing the handshake — how the real endpoint rejects a bad key.
    pub http_status: Option<u16>,
    /// Stall this long before answering the handshake, to drive connect
    /// timeouts.
    pub accept_delay: Option<Duration>,
    /// Go quiet after the n-th chunk: no more frames, no `isFinal`, socket
    /// stays open. The documentation says nothing about a provider that simply
    /// stops talking, so it is a knob — this is what an idle deadline is for.
    pub silent_after: Option<usize>,
    /// `code` of a scripted error frame. `None` = 500. A test that wants an
    /// auth verdict out of a frame (rather than out of the handshake) sets 401
    /// here.
    pub error_status: Option<u16>,
    /// Spell the scripted failure WITHOUT an `error` field — nothing but a
    /// `code` and a `message`. The vendor documents no error schema at all, so
    /// which of the two shapes arrives is a knob rather than an assumption.
    pub bare_error: bool,
    /// Send an `{"audio": ""}` frame before the n-th chunk. The vendor emits
    /// one around the end of a generation; the documentation does not say
    /// when, so the position is the test's to choose.
    pub empty_audio_before: Option<usize>,
}

impl ElevenLabsScript {
    /// An empty script: the handshake succeeds and every input stream is
    /// answered with a final frame and no audio at all.
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
    /// Turn the stream into an error frame in place of the n-th chunk.
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
    /// Give a scripted error frame this `code`.
    pub fn with_error_status(mut self, status: u16) -> Self {
        self.error_status = Some(status);
        self
    }
    /// Spell the scripted failure with only a `code` and a `message`.
    pub fn failing_without_an_error_field(mut self) -> Self {
        self.bare_error = true;
        self
    }
    /// Send an `{"audio": ""}` frame before the n-th chunk.
    pub fn with_empty_audio_before(mut self, n: usize) -> Self {
        self.empty_audio_before = Some(n);
        self
    }
}

#[derive(Default)]
struct State {
    script: ElevenLabsScript,
    messages: Vec<JsonValue>,
    paths: Vec<String>,
    queries: Vec<String>,
    header_names: Vec<Vec<String>>,
    connections: usize,
    closes: usize,
}

/// A running fake ElevenLabs. Dropping it stops the server.
pub struct MockElevenLabs {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockElevenLabs {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockElevenLabs {
    /// Binds and starts serving `script`. Returns once the port is known.
    pub async fn start(script: ElevenLabsScript) -> std::io::Result<Self> {
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
    /// The adapter appends the path and the query itself, exactly as it does
    /// against the real host.
    pub fn base_url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    /// Every client message the fake parsed, in arrival order — the
    /// initialisation message, the text, and the end-of-stream marker.
    pub async fn received_messages(&self) -> Vec<JsonValue> {
        self.state.lock().await.messages.clone()
    }

    /// The request path of each handshake, e.g.
    /// `/v1/text-to-speech/<voice>/stream-input`. The voice id lives in the
    /// path on this API, so this is where a test reads it back.
    pub async fn paths(&self) -> Vec<String> {
        self.state.lock().await.paths.clone()
    }

    /// The raw query string of each handshake.
    pub async fn queries(&self) -> Vec<String> {
        self.state.lock().await.queries.clone()
    }

    /// The request header NAMES of each handshake, lower-cased. Values are
    /// deliberately not kept: the interesting header carries a credential, and
    /// the name plus an empty query is the whole proof the adapter owes.
    pub async fn header_names(&self) -> Vec<Vec<String>> {
        self.state.lock().await.header_names.clone()
    }

    /// How many WebSocket connections were established. One synthesis opens
    /// exactly one, which is what the adapter promises.
    pub async fn connections(&self) -> usize {
        self.state.lock().await.connections
    }

    /// How many client-side close frames arrived. The protocol has no cancel
    /// message, so a close is the only way a caller can say "stop".
    pub async fn closes(&self) -> usize {
        self.state.lock().await.closes
    }

    /// Whether the client closed a socket at all.
    pub async fn closed_by_client(&self) -> bool {
        self.state.lock().await.closes > 0
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
    let body = json!({ "detail": "fake elevenlabs refused the upgrade" }).to_string();
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
         Content-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        reason = reason_phrase(status),
        len = body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.flush().await;
}

/// Upgrades, then replays the script once the input stream is closed.
///
/// The `result_large_err` allow is not laziness: the handshake callback's
/// signature is fixed by tungstenite (`Result<Response, ErrorResponse>`), so
/// the error type is not ours to box.
#[allow(clippy::result_large_err)]
async fn serve_ws(stream: TcpStream, state: Arc<Mutex<State>>, script: ElevenLabsScript) {
    let mut path = String::new();
    let mut query = String::new();
    let mut header_names = Vec::new();
    let ws = match tokio_tungstenite::accept_hdr_async(stream, |req: &_, resp| {
        path = uri_path(req);
        query = uri_query(req);
        header_names = request_header_names(req);
        Ok(resp)
    })
    .await
    {
        Ok(ws) => ws,
        Err(_) => return,
    };
    {
        let mut st = state.lock().await;
        st.paths.push(path);
        st.queries.push(query);
        st.header_names.push(header_names);
        st.connections += 1;
    }

    let (mut write, mut read) = ws.split();
    // A close must be able to interrupt a stream that is mid-flight, so the
    // reader runs beside the writer instead of before it. `notify_one` stores
    // one permit, so a close that lands while a chunk is being written is not
    // lost — the next wait returns immediately.
    let closed = Arc::new(Notify::new());
    let (go_tx, mut go_rx) = mpsc::channel::<()>(4);
    let read_state = state.clone();
    let read_closed = closed.clone();
    let reader = tokio::spawn(async move {
        while let Some(msg) = read.next().await {
            match msg {
                Ok(WsMessage::Text(txt)) => {
                    let Ok(v) = meclaw_core::serde_json::from_str::<JsonValue>(&txt) else {
                        continue;
                    };
                    let end_of_stream = v.get("text").and_then(JsonValue::as_str) == Some("");
                    read_state.lock().await.messages.push(v);
                    if end_of_stream && go_tx.send(()).await.is_err() {
                        break;
                    }
                }
                Ok(WsMessage::Close(_)) => {
                    read_state.lock().await.closes += 1;
                    read_closed.notify_one();
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    });

    while go_rx.recv().await.is_some() {
        if !stream_one(&mut write, &script, &closed, &state).await {
            break;
        }
    }
    let _ = reader.await;
}

/// Replays the script once. Returns `false` when the socket died.
async fn stream_one<S>(
    write: &mut S,
    script: &ElevenLabsScript,
    closed: &Notify,
    state: &Arc<Mutex<State>>,
) -> bool
where
    S: SinkExt<WsMessage> + Unpin,
{
    for (i, chunk) in script.chunks.iter().enumerate() {
        if script.fail_after == Some(i) {
            return send_json(
                write,
                error_frame(script.error_status.unwrap_or(500), script.bare_error),
            )
            .await;
        }
        if script.silent_after == Some(i) {
            // Nothing more, not even a final frame. The socket stays open, so
            // only a deadline on the client's side can end this.
            return true;
        }
        if script.empty_audio_before == Some(i)
            && !send_json(write, json!({ "audio": "", "isFinal": false })).await
        {
            return false;
        }
        if !script.chunk_delay.is_zero() {
            tokio::select! {
                () = tokio::time::sleep(script.chunk_delay) => {}
                // Only a wake-up, never a verdict — the check below decides.
                () = closed.notified() => {}
            }
        }
        // Belt and braces: the notify only wakes early, the recorded close is
        // the decision. Nothing follows a close.
        if state.lock().await.closes > 0 {
            return true;
        }
        let frame = json!({
            "audio": encode_base64(chunk),
            "isFinal": false,
            "normalizedAlignment": JsonValue::Null,
            "alignment": JsonValue::Null,
        });
        if !send_json(write, frame).await {
            return false;
        }
    }
    if script.fail_after == Some(script.chunks.len()) {
        return send_json(
            write,
            error_frame(script.error_status.unwrap_or(500), script.bare_error),
        )
        .await;
    }
    if script.silent_after == Some(script.chunks.len()) {
        return true;
    }
    send_json(write, json!({ "audio": JsonValue::Null, "isFinal": true })).await
}

/// The shape an ElevenLabs failure takes on an established socket: a message,
/// a machine-readable name and a code — or, with `bare`, the same thing minus
/// the name, which is the other shape seen in the wild.
fn error_frame(status: u16, bare: bool) -> JsonValue {
    if bare {
        return json!({
            "message": "the fake was told to fail here",
            "code": status,
        });
    }
    json!({
        "error": "scripted_failure",
        "message": "the fake was told to fail here",
        "code": status,
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

fn uri_path(req: &tokio_tungstenite::tungstenite::handshake::server::Request) -> String {
    req.uri().path().to_string()
}

fn uri_query(req: &tokio_tungstenite::tungstenite::handshake::server::Request) -> String {
    req.uri().query().unwrap_or_default().to_string()
}

fn request_header_names(
    req: &tokio_tungstenite::tungstenite::handshake::server::Request,
) -> Vec<String> {
    req.headers()
        .iter()
        .map(|(name, _value)| name.as_str().to_ascii_lowercase())
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
        422 => "Unprocessable Entity",
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

/// Standard Base64 with padding — what ElevenLabs puts in `audio`.
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

    /// Connects without the `connect` feature: the TCP side is ours, only the
    /// handshake comes from tungstenite.
    async fn connect(
        mock: &MockElevenLabs,
    ) -> tokio_tungstenite::WebSocketStream<tokio::net::TcpStream> {
        let stream = TcpStream::connect(mock.addr).await.expect("tcp connect");
        let url = format!(
            "{}/v1/text-to-speech/test-voice/stream-input\
             ?model_id=eleven_flash_v2_5&output_format=pcm_24000",
            mock.base_url()
        );
        let req = url.into_client_request().expect("request");
        let (ws, _) = tokio_tungstenite::client_async(req, stream)
            .await
            .expect("handshake");
        ws
    }

    /// The documented three-message input stream: initialise, text, end.
    async fn speak<S>(ws: &mut S, text: &str)
    where
        S: SinkExt<WsMessage> + Unpin,
    {
        for msg in [
            json!({ "text": " ", "voice_settings": { "stability": 0.5 } }),
            json!({ "text": text }),
            json!({ "text": "" }),
        ] {
            if ws
                .send(WsMessage::Text(msg.to_string().into()))
                .await
                .is_err()
            {
                panic!("the fake accepted no message");
            }
        }
    }

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

    #[tokio::test]
    async fn fake_streams_scripted_chunks_then_a_final_frame() {
        let mock = MockElevenLabs::start(ElevenLabsScript::new().chunk(b"\x01\x02").chunk(b"\x03"))
            .await
            .expect("start");
        let mut ws = connect(&mock).await;
        speak(&mut ws, "hallo ").await;

        let mut audio = Vec::new();
        let mut saw_final = false;
        while let Some(Ok(WsMessage::Text(t))) = ws.next().await {
            let v: JsonValue = meclaw_core::serde_json::from_str(&t).expect("json");
            if let Some(data) = v["audio"].as_str() {
                audio.push(data.to_string());
            }
            if v["isFinal"] == json!(true) {
                saw_final = true;
                break;
            }
        }
        assert!(saw_final, "the stream must end with isFinal");
        assert_eq!(
            audio,
            vec![encode_base64(b"\x01\x02"), encode_base64(b"\x03")]
        );

        let msgs = mock.received_messages().await;
        assert_eq!(msgs.len(), 3, "initialise, text, end-of-stream");
        assert_eq!(msgs[1]["text"], "hallo ");
        assert_eq!(msgs[2]["text"], "");
        assert_eq!(mock.connections().await, 1);
        assert_eq!(
            mock.paths().await,
            vec!["/v1/text-to-speech/test-voice/stream-input".to_string()],
            "the voice id travels in the path on this API"
        );
        assert!(
            mock.header_names()
                .await
                .first()
                .is_some_and(|names| names.iter().any(|n| n == "host")),
            "header names are recorded, values never are"
        );
    }

    #[tokio::test]
    async fn fake_records_a_client_close_and_stops_streaming() {
        let mock = MockElevenLabs::start(
            ElevenLabsScript::new()
                .chunk(b"\x01")
                .chunk(b"\x02")
                .chunk(b"\x03")
                .with_chunk_delay(Duration::from_millis(50)),
        )
        .await
        .expect("start");
        let mut ws = connect(&mock).await;
        speak(&mut ws, "hallo ").await;
        // One chunk, then close: the remaining two must never arrive.
        let first = ws.next().await.expect("frame").expect("ok");
        assert!(matches!(first, WsMessage::Text(_)));
        ws.send(WsMessage::Close(None)).await.expect("send close");

        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline && !mock.closed_by_client().await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(mock.closed_by_client().await, "the close must be recorded");
        assert_eq!(mock.closes().await, 1);
    }

    #[tokio::test]
    async fn fake_can_fail_the_stream_after_n_chunks() {
        let mock = MockElevenLabs::start(
            ElevenLabsScript::new()
                .chunk(b"\x01")
                .chunk(b"\x02")
                .failing_after(1)
                .with_error_status(401),
        )
        .await
        .expect("start");
        let mut ws = connect(&mock).await;
        speak(&mut ws, "hallo ").await;

        let mut kinds = Vec::new();
        while let Some(Ok(WsMessage::Text(t))) = ws.next().await {
            let v: JsonValue = meclaw_core::serde_json::from_str(&t).expect("json");
            if let Some(error) = v["error"].as_str() {
                kinds.push(error.to_string());
                assert_eq!(v["code"], 401);
                break;
            }
            kinds.push("audio".to_string());
        }
        assert_eq!(
            kinds,
            vec!["audio".to_string(), "scripted_failure".to_string()]
        );
    }

    #[tokio::test]
    async fn fake_can_refuse_the_upgrade_with_a_status() {
        let mock = MockElevenLabs::start(ElevenLabsScript::new().refusing_with_status(401))
            .await
            .expect("start");
        let stream = TcpStream::connect(mock.addr).await.expect("tcp connect");
        let req = format!("{}/v1/text-to-speech/v/stream-input", mock.base_url())
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
