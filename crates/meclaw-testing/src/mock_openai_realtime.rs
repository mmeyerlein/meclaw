//! Wave voice-cell (t2b) — hermetic fake of the OpenAI Realtime transcription
//! WebSocket, so the real `openai` STT adapter can be tested without a key and
//! without the network.
//!
//! Same shape as `mock_slack`: a `TcpListener` on `127.0.0.1:0`, the kernel
//! picks the port, everything runs in a background task, and the client points
//! its `base_url` param at it. Only the transcription half of the Realtime API
//! is modelled — speech-to-speech is explicitly not part of the `voice` cell.
//!
//! What the fake observes is what the protocol makes observable: the query
//! string of the upgrade, whether an `Authorization` header was present (never
//! its value), every client event verbatim, and the number of audio bytes that
//! arrived *after* base64 decoding. The last one is the interesting counter —
//! it proves the adapter encoded whole PCM frames rather than a padded string
//! of the right length.
//!
//! Where the OpenAI docs are silent, the behaviour is a script knob rather than
//! a guess: the delay before the handshake, the HTTP status of the upgrade and
//! the moment the server closes are all script parameters, so a test states the
//! assumption it exercises instead of the fake hard-coding one as truth.

use futures_util::{SinkExt, StreamExt};
use meclaw_core::serde_json::{Value as JsonValue, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// How long `RequireAudioBytes` waits for the audio before giving up. Generous
/// on purpose: it is a failure marker, not a timing discriminator.
const REQUIRE_AUDIO_CAP: Duration = Duration::from_secs(30);

/// One scripted server-side action on an open transcription socket.
#[derive(Clone, Debug)]
pub enum OaiAction {
    /// Send this JSON value as a server event, verbatim.
    SendEvent(JsonValue),
    /// Send a raw frame that is not necessarily JSON (malformed-input tests).
    SendRaw(String),
    /// Wait before the next action.
    Delay(Duration),
    /// Block the script until this many decoded audio bytes have arrived.
    RequireAudioBytes(usize),
    /// Close the socket from the server side.
    CloseWs,
}

/// A per-connection scenario.
#[derive(Clone, Debug, Default)]
pub struct OpenAiRealtimeScript {
    /// Actions replayed in order once the socket is up.
    pub actions: Vec<OaiAction>,
    /// Reject the upgrade with this HTTP status instead of accepting it.
    pub upgrade_status: Option<u16>,
    /// Stall before the handshake, to drive connect-timeout tests.
    pub accept_delay: Option<Duration>,
    /// Send `session.created` before replaying the script (the real service
    /// does; a test that does not care can switch it off).
    pub send_session_created: bool,
}

impl OpenAiRealtimeScript {
    /// An empty script that greets with `session.created`.
    pub fn new() -> Self {
        Self {
            actions: Vec::new(),
            upgrade_status: None,
            accept_delay: None,
            send_session_created: true,
        }
    }
    /// Queue an arbitrary server event.
    pub fn event(mut self, event: JsonValue) -> Self {
        self.actions.push(OaiAction::SendEvent(event));
        self
    }
    /// Queue a raw, possibly malformed frame.
    pub fn raw(mut self, frame: &str) -> Self {
        self.actions.push(OaiAction::SendRaw(frame.to_string()));
        self
    }
    /// Queue `input_audio_buffer.speech_started`.
    pub fn speech_started(self) -> Self {
        self.event(json!({
            "type": "input_audio_buffer.speech_started",
            "event_id": "event_fake_speech_started",
            "audio_start_ms": 0,
            "item_id": "item_fake"
        }))
    }
    /// Queue `input_audio_buffer.speech_stopped` — the counterpart the cell has
    /// no use for; scripted so a test can pin that nothing happens.
    pub fn speech_stopped(self) -> Self {
        self.event(json!({
            "type": "input_audio_buffer.speech_stopped",
            "event_id": "event_fake_speech_stopped",
            "audio_end_ms": 500,
            "item_id": "item_fake"
        }))
    }
    /// Queue `session.updated` — the service's confirmation that it accepted
    /// the configuration. After it, a top-level `error` is survivable.
    pub fn session_updated(self) -> Self {
        self.event(json!({
            "type": "session.updated",
            "event_id": "event_fake_updated",
            "session": { "type": "transcription", "object": "realtime.session" }
        }))
    }
    /// Queue one incremental transcript for `item_id`.
    pub fn delta(self, item_id: &str, delta: &str) -> Self {
        self.event(json!({
            "type": "conversation.item.input_audio_transcription.delta",
            "event_id": "event_fake_delta",
            "item_id": item_id,
            "content_index": 0,
            "delta": delta
        }))
    }
    /// Queue the final transcript for `item_id`.
    pub fn completed(self, item_id: &str, transcript: &str) -> Self {
        self.event(json!({
            "type": "conversation.item.input_audio_transcription.completed",
            "event_id": "event_fake_completed",
            "item_id": item_id,
            "content_index": 0,
            "transcript": transcript
        }))
    }
    /// Queue a transcription failure for `item_id`.
    pub fn failed(self, item_id: &str, message: &str) -> Self {
        self.event(json!({
            "type": "conversation.item.input_audio_transcription.failed",
            "event_id": "event_fake_failed",
            "item_id": item_id,
            "content_index": 0,
            "error": { "type": "transcription_error", "message": message }
        }))
    }
    /// Queue a top-level `error` event.
    pub fn error(self, message: &str) -> Self {
        self.event(json!({
            "type": "error",
            "event_id": "event_fake_error",
            "error": { "type": "invalid_request_error", "message": message }
        }))
    }
    /// Queue a pause.
    pub fn delay_ms(mut self, ms: u64) -> Self {
        self.actions
            .push(OaiAction::Delay(Duration::from_millis(ms)));
        self
    }
    /// Hold the script back until this many decoded audio bytes arrived.
    pub fn require_audio_bytes(mut self, n: usize) -> Self {
        self.actions.push(OaiAction::RequireAudioBytes(n));
        self
    }
    /// Queue a server-side close.
    pub fn close(mut self) -> Self {
        self.actions.push(OaiAction::CloseWs);
        self
    }
    /// Reject the upgrade with this status (401 for the credential path).
    pub fn with_upgrade_status(mut self, status: u16) -> Self {
        self.upgrade_status = Some(status);
        self
    }
    /// Stall this long before the handshake.
    pub fn with_accept_delay(mut self, d: Duration) -> Self {
        self.accept_delay = Some(d);
        self
    }
    /// Suppress the `session.created` greeting.
    pub fn without_session_created(mut self) -> Self {
        self.send_session_created = false;
        self
    }
}

#[derive(Default)]
struct State {
    script: OpenAiRealtimeScript,
    connects: usize,
    path: String,
    query: HashMap<String, String>,
    authorization_present: bool,
    authorization_is_bearer: bool,
    beta_header: Option<String>,
    client_events: Vec<JsonValue>,
    audio_bytes: usize,
}

/// A running fake OpenAI Realtime transcription endpoint. Dropping it stops the
/// server.
pub struct MockOpenAiRealtime {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockOpenAiRealtime {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockOpenAiRealtime {
    /// Binds and starts serving `script`. Returns once the port is known.
    pub async fn start(script: OpenAiRealtimeScript) -> std::io::Result<Self> {
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
                    serve_ws(stream, st).await;
                });
            }
        });
        Ok(Self { addr, state, task })
    }

    /// Base URL to feed the adapter's `base_url` param, e.g. `ws://127.0.0.1:54321`.
    /// The adapter appends its own path (`/v1/realtime`) and query.
    pub fn base_url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    /// How many sockets were opened — the reconnect proof.
    pub async fn connects(&self) -> usize {
        self.state.lock().await.connects
    }

    /// Path of the upgrade request — the proof that `base_url` was used
    /// verbatim as the base and the adapter appended its own path (R-V10).
    pub async fn received_path(&self) -> String {
        self.state.lock().await.path.clone()
    }

    /// Query parameters of the upgrade request, decoded.
    pub async fn received_query(&self) -> HashMap<String, String> {
        self.state.lock().await.query.clone()
    }

    /// Whether an `Authorization` header was sent. The value is never stored:
    /// a credential has no business in a test fixture, a log or a failure text.
    pub async fn authorization_present(&self) -> bool {
        self.state.lock().await.authorization_present
    }

    /// Whether that header used the `Bearer` scheme.
    pub async fn authorization_is_bearer(&self) -> bool {
        self.state.lock().await.authorization_is_bearer
    }

    /// Value of the `OpenAI-Beta` header, if the client sent one.
    pub async fn beta_header(&self) -> Option<String> {
        self.state.lock().await.beta_header.clone()
    }

    /// Every client event received so far, in arrival order.
    pub async fn client_events(&self) -> Vec<JsonValue> {
        self.state.lock().await.client_events.clone()
    }

    /// The first `session.update` the client sent, if any.
    pub async fn session_update(&self) -> Option<JsonValue> {
        self.state
            .lock()
            .await
            .client_events
            .iter()
            .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("session.update"))
            .cloned()
    }

    /// Audio bytes received, counted after base64 decoding.
    pub async fn received_audio_bytes(&self) -> usize {
        self.state.lock().await.audio_bytes
    }
}

/// Upgrades to a WebSocket (or refuses to), replays the script, and records
/// client events concurrently.
///
/// The `result_large_err` allow is not laziness: the handshake callback's
/// signature is fixed by tungstenite (`Result<Response, ErrorResponse>`), so the
/// error type is not ours to box.
#[allow(clippy::result_large_err)]
async fn serve_ws(stream: TcpStream, state: Arc<Mutex<State>>) {
    let (accept_delay, upgrade_status, script) = {
        let st = state.lock().await;
        (
            st.script.accept_delay,
            st.script.upgrade_status,
            st.script.clone(),
        )
    };
    if let Some(d) = accept_delay {
        tokio::time::sleep(d).await;
    }

    let mut path = String::new();
    let mut query = HashMap::new();
    let mut authorization: Option<String> = None;
    let mut beta: Option<String> = None;
    let handshake = tokio_tungstenite::accept_hdr_async(stream, |req: &_, resp| {
        let req: &tokio_tungstenite::tungstenite::handshake::server::Request = req;
        path = req.uri().path().to_string();
        query = parse_query(req.uri().query().unwrap_or(""));
        authorization = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split_whitespace().next().unwrap_or("").to_string());
        beta = req
            .headers()
            .get("openai-beta")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());
        match upgrade_status {
            Some(status) => {
                let mut err = tokio_tungstenite::tungstenite::handshake::server::ErrorResponse::new(
                    Some("scripted upgrade refusal".to_string()),
                );
                *err.status_mut() =
                    tokio_tungstenite::tungstenite::http::StatusCode::from_u16(status)
                        .unwrap_or(tokio_tungstenite::tungstenite::http::StatusCode::BAD_REQUEST);
                Err(err)
            }
            None => Ok(resp),
        }
    })
    .await;

    {
        let mut st = state.lock().await;
        st.connects += 1;
        st.path = path;
        st.query = query;
        st.authorization_present = authorization.is_some();
        st.authorization_is_bearer = authorization.as_deref() == Some("Bearer");
        st.beta_header = beta;
    }

    let ws = match handshake {
        Ok(ws) => ws,
        Err(_) => return,
    };
    let (mut write, mut read) = ws.split();

    // Client events are collected in parallel with the script: the adapter
    // keeps appending audio while frames are still arriving, and serialising
    // the two would hide ordering bugs.
    let read_state = state.clone();
    let reader = tokio::spawn(async move {
        while let Some(Ok(msg)) = read.next().await {
            if let WsMessage::Text(txt) = msg
                && let Ok(v) = meclaw_core::serde_json::from_str::<JsonValue>(&txt)
            {
                let mut st = read_state.lock().await;
                if v.get("type").and_then(|t| t.as_str()) == Some("input_audio_buffer.append")
                    && let Some(b64) = v.get("audio").and_then(|a| a.as_str())
                    && let Some(bytes) = b64_decode(b64)
                {
                    st.audio_bytes += bytes.len();
                }
                st.client_events.push(v);
            }
        }
    });

    if script.send_session_created {
        let created = json!({
            "type": "session.created",
            "event_id": "event_fake_created",
            "session": { "type": "transcription", "object": "realtime.session" }
        })
        .to_string();
        if write.send(WsMessage::Text(created.into())).await.is_err() {
            return;
        }
    }

    for action in script.actions {
        let frame = match action {
            OaiAction::Delay(d) => {
                tokio::time::sleep(d).await;
                continue;
            }
            OaiAction::RequireAudioBytes(n) => {
                if !wait_for_audio(&state, n).await {
                    break;
                }
                continue;
            }
            OaiAction::CloseWs => {
                let _ = write.close().await;
                break;
            }
            OaiAction::SendRaw(raw) => raw,
            OaiAction::SendEvent(v) => v.to_string(),
        };
        if write.send(WsMessage::Text(frame.into())).await.is_err() {
            break;
        }
    }

    // Stay open after the script so the client can finish its side; the reader
    // ends when the client goes away.
    let _ = reader.await;
}

/// Polls until `n` decoded audio bytes arrived. `false` means the cap elapsed —
/// the script then stops rather than hanging a test forever.
async fn wait_for_audio(state: &Arc<Mutex<State>>, n: usize) -> bool {
    let deadline = tokio::time::Instant::now() + REQUIRE_AUDIO_CAP;
    loop {
        if state.lock().await.audio_bytes >= n {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

fn parse_query(raw: &str) -> HashMap<String, String> {
    raw.split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_string(), percent_decode(v)))
        .collect()
}

/// Minimal percent-decoding — query values here are model names and intents,
/// not arbitrary blobs.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Standard-alphabet base64 decoder with padding. `None` on any character the
/// alphabet does not contain — `base64` is not on the tech-stack allow-list, and
/// counting audio bytes needs exactly this much of it.
pub fn b64_decode(input: &str) -> Option<Vec<u8>> {
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    for c in input.bytes() {
        if c == b'=' {
            break;
        }
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'\n' | b'\r' => continue,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    fn b64_encode(bytes: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(A[(n >> 18) as usize & 63] as char);
            out.push(A[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                A[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                A[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    #[test]
    fn b64_decode_matches_known_vectors() {
        assert_eq!(b64_decode("").unwrap(), b"");
        assert_eq!(b64_decode("Zg==").unwrap(), b"f");
        assert_eq!(b64_decode("Zm8=").unwrap(), b"fo");
        assert_eq!(b64_decode("Zm9v").unwrap(), b"foo");
        assert_eq!(b64_decode("Zm9vYmFy").unwrap(), b"foobar");
        assert_eq!(b64_decode("/w+A").unwrap(), vec![0xFF, 0x0F, 0x80]);
        assert!(b64_decode("not base64!").is_none());
    }

    #[tokio::test]
    async fn script_replays_after_the_required_audio_arrived() {
        let script = OpenAiRealtimeScript::new()
            .require_audio_bytes(3200)
            .delta("item_1", "hal")
            .completed("item_1", "hallo")
            .close();
        let mock = MockOpenAiRealtime::start(script).await.expect("bind");

        let url = format!("{}/v1/realtime?intent=transcription", mock.base_url());
        let mut req = url.as_str().into_client_request().expect("request");
        req.headers_mut()
            .insert("authorization", "Bearer test-token".parse().expect("hv"));
        let stream = TcpStream::connect(mock.base_url().trim_start_matches("ws://"))
            .await
            .expect("connect");
        let (ws, _resp) = tokio_tungstenite::client_async(req, stream)
            .await
            .expect("handshake");
        let (mut write, mut read) = ws.split();

        let greeting = read.next().await.expect("frame").expect("ok");
        let greeting: JsonValue = match greeting {
            WsMessage::Text(t) => meclaw_core::serde_json::from_str(&t).expect("json"),
            other => panic!("expected text, got {other:?}"),
        };
        assert_eq!(greeting["type"], "session.created");

        let audio = b64_encode(&vec![7u8; 3200]);
        write
            .send(WsMessage::Text(
                json!({ "type": "input_audio_buffer.append", "audio": audio })
                    .to_string()
                    .into(),
            ))
            .await
            .expect("send audio");

        let mut kinds = Vec::new();
        while let Some(Ok(msg)) = read.next().await {
            if let WsMessage::Text(t) = msg {
                let v: JsonValue = meclaw_core::serde_json::from_str(&t).expect("json");
                kinds.push(v["type"].as_str().unwrap_or_default().to_string());
                if kinds.len() == 2 {
                    break;
                }
            }
        }
        assert_eq!(
            kinds,
            vec![
                "conversation.item.input_audio_transcription.delta".to_string(),
                "conversation.item.input_audio_transcription.completed".to_string(),
            ]
        );
        assert_eq!(mock.received_audio_bytes().await, 3200);
        assert_eq!(mock.received_path().await, "/v1/realtime");
        assert_eq!(
            mock.received_query()
                .await
                .get("intent")
                .map(String::as_str),
            Some("transcription")
        );
        assert!(mock.authorization_present().await);
        assert!(mock.authorization_is_bearer().await);
        assert_eq!(mock.connects().await, 1);
    }

    #[tokio::test]
    async fn scripted_upgrade_status_refuses_the_handshake() {
        let mock = MockOpenAiRealtime::start(OpenAiRealtimeScript::new().with_upgrade_status(401))
            .await
            .expect("bind");
        let url = format!("{}/v1/realtime", mock.base_url());
        let req = url.as_str().into_client_request().expect("request");
        let stream = TcpStream::connect(mock.base_url().trim_start_matches("ws://"))
            .await
            .expect("connect");
        let err = match tokio_tungstenite::client_async(req, stream).await {
            Ok(_) => panic!("the scripted 401 must refuse the handshake"),
            Err(e) => e,
        };
        assert!(
            format!("{err}").contains("401"),
            "expected a 401 in {err}, got none"
        );
    }
}
