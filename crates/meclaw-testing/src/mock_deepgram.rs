//! Wave voice-cell — hermetic fake of Deepgram's Flux streaming API.
//!
//! Same shape as `mock_slack`: a `TcpListener` on `127.0.0.1:0`, the kernel
//! picks the port, everything runs in a background task. The real endpoint is
//! `wss://api.deepgram.com/v2/listen`; the fake accepts a WebSocket upgrade on
//! any path, so the adapter's `base_url` param is the only thing a test has to
//! redirect.
//!
//! What the protocol says, the fake does: it answers the upgrade, sends a
//! `Connected` message, then replays its script of `TurnInfo` messages while a
//! reader half counts the incoming binary audio and records every text frame
//! the client sent (that is where `{"type":"CloseStream"}` shows up). Sources:
//! <https://developers.deepgram.com/reference/speech-to-text/listen-flux> and
//! <https://developers.deepgram.com/docs/flux/state>.
//!
//! Where the docs are silent, the behaviour is a script knob rather than a
//! guess: how long the server stalls before answering the upgrade, whether it
//! answers with an HTTP status instead of upgrading at all, and how long it
//! waits between messages are all fields of [`DeepgramScript`], so a test
//! states which unverified assumption it exercises instead of baking one in.

use futures_util::{SinkExt, StreamExt};
use meclaw_core::serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::handshake::server::ErrorResponse;
use tokio_tungstenite::tungstenite::http::StatusCode;

/// One scripted server-side action on an open Flux socket.
#[derive(Clone, Debug)]
pub enum DgAction {
    /// Send a `TurnInfo` message with this `event` and `transcript`.
    ///
    /// `event` is one of `StartOfTurn`, `Update`, `EagerEndOfTurn`,
    /// `TurnResumed`, `EndOfTurn` — the fake does not validate it, so a test
    /// can also send an unknown event and assert the adapter ignores it.
    SendTurnInfo {
        /// Value of the `event` field.
        event: String,
        /// Value of the `transcript` field.
        transcript: String,
    },
    /// Wait before the next action.
    Delay(Duration),
    /// Send a raw text frame verbatim (malformed-input tests).
    SendRaw(String),
    /// Close the socket from the server side.
    CloseWs,
    /// Block the script until this many audio bytes have arrived in total.
    ///
    /// This is what makes "the adapter forwarded the audio" provable without a
    /// sleep: the transcript only appears once the bytes are actually in.
    RequireAudioBytes(usize),
}

/// A per-connection scenario.
#[derive(Clone, Debug)]
pub struct DeepgramScript {
    /// Actions replayed in order once the socket is up.
    pub actions: Vec<DgAction>,
    /// Stall this long before answering the WebSocket upgrade. Deepgram
    /// documents no timing guarantee, so connect-timeout tests state it here.
    pub accept_delay: Option<Duration>,
    /// Answer the upgrade with this HTTP status instead of upgrading — `401`
    /// is what a rejected credential looks like from the client's side.
    pub reject_status: Option<u16>,
    /// Send the `Connected` message before the script. True by default,
    /// because the real service always opens with it.
    pub send_connected: bool,
}

impl Default for DeepgramScript {
    fn default() -> Self {
        Self {
            actions: Vec::new(),
            accept_delay: None,
            reject_status: None,
            send_connected: true,
        }
    }
}

impl DeepgramScript {
    /// An empty script that upgrades, greets, and then stays quiet.
    pub fn new() -> Self {
        Self::default()
    }
    /// Queue a `TurnInfo` message.
    pub fn turn_info(mut self, event: &str, transcript: &str) -> Self {
        self.actions.push(DgAction::SendTurnInfo {
            event: event.to_string(),
            transcript: transcript.to_string(),
        });
        self
    }
    /// Queue a raw text frame.
    pub fn raw(mut self, frame: &str) -> Self {
        self.actions.push(DgAction::SendRaw(frame.to_string()));
        self
    }
    /// Queue a pause.
    pub fn delay_ms(mut self, ms: u64) -> Self {
        self.actions
            .push(DgAction::Delay(Duration::from_millis(ms)));
        self
    }
    /// Queue a server-side close.
    pub fn close(mut self) -> Self {
        self.actions.push(DgAction::CloseWs);
        self
    }
    /// Queue a wait for `n` audio bytes in total.
    pub fn require_audio_bytes(mut self, n: usize) -> Self {
        self.actions.push(DgAction::RequireAudioBytes(n));
        self
    }
    /// Hold the upgrade back, for connect-timeout tests.
    pub fn with_accept_delay(mut self, d: Duration) -> Self {
        self.accept_delay = Some(d);
        self
    }
    /// Refuse the upgrade with this HTTP status.
    pub fn rejecting_with(mut self, status: u16) -> Self {
        self.reject_status = Some(status);
        self
    }
    /// Suppress the opening `Connected` message.
    pub fn without_connected(mut self) -> Self {
        self.send_connected = false;
        self
    }
}

#[derive(Default)]
struct State {
    query: HashMap<String, String>,
    query_pairs: Vec<(String, String)>,
    authorization_seen: bool,
    client_messages: Vec<String>,
    connections: usize,
}

/// A running fake Deepgram Flux endpoint. Dropping it stops the server.
pub struct MockDeepgram {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    audio_rx: watch::Receiver<usize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockDeepgram {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockDeepgram {
    /// Binds and starts serving `script`. Returns once the port is known;
    /// every connection replays the same script.
    pub async fn start(script: DeepgramScript) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let state = Arc::new(Mutex::new(State::default()));
        let (audio_tx, audio_rx) = watch::channel(0usize);
        let st = state.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let st = st.clone();
                let script = script.clone();
                let audio_tx = audio_tx.clone();
                tokio::spawn(async move {
                    serve_ws(stream, st, script, audio_tx).await;
                });
            }
        });
        Ok(Self {
            addr,
            state,
            audio_rx,
            task,
        })
    }

    /// WebSocket base, e.g. `ws://127.0.0.1:54321`. Feed this to the cell's
    /// `base_url` param — the adapter appends `/v2/listen` and the query.
    pub fn base_url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    /// Total binary audio bytes received across all connections.
    pub fn received_audio_bytes(&self) -> usize {
        *self.audio_rx.borrow()
    }

    /// Query parameters of the most recent upgrade request, already split.
    /// Values are taken verbatim — the fake does not percent-decode, and the
    /// Flux query carries only numbers, model names and language codes.
    pub fn received_query(&self) -> HashMap<String, String> {
        self.lock().query.clone()
    }

    /// The same query as [`Self::received_query`], but in arrival order and
    /// with repetitions kept. A map cannot answer a repeated key, and a
    /// repeated key is exactly how a list of keyterms reaches the service, so
    /// any assertion about one has to read the pairs.
    pub fn received_query_pairs(&self) -> Vec<(String, String)> {
        self.lock().query_pairs.clone()
    }

    /// Whether the most recent upgrade carried an `Authorization` header. The
    /// value is deliberately not stored: a credential has no business in a
    /// test fixture, let alone in an assertion message.
    pub fn saw_authorization(&self) -> bool {
        self.lock().authorization_seen
    }

    /// Text frames the client sent, in arrival order — this is where
    /// `{"type":"CloseStream"}` turns up.
    pub fn client_messages(&self) -> Vec<String> {
        self.lock().client_messages.clone()
    }

    /// How many WebSocket connections were established.
    pub fn connections(&self) -> usize {
        self.lock().connections
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        // A poisoned lock means one of the fake's tasks panicked. The recorded
        // facts are still readable and still true, so the accessor hands them
        // out rather than poisoning the assertion too.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Upgrades to a WebSocket, replays the script, and counts audio concurrently.
///
/// The `result_large_err` allow is not laziness: the handshake callback's
/// signature is fixed by tungstenite (`Result<Response, ErrorResponse>`), so
/// the error type is not ours to box.
#[allow(clippy::result_large_err)]
async fn serve_ws(
    stream: TcpStream,
    state: Arc<Mutex<State>>,
    script: DeepgramScript,
    audio_tx: watch::Sender<usize>,
) {
    if let Some(d) = script.accept_delay {
        tokio::time::sleep(d).await;
    }

    let reject = script.reject_status;
    let cb_state = state.clone();
    let ws = match tokio_tungstenite::accept_hdr_async(stream, |req: &_, resp| {
        record_request(req, &cb_state);
        match reject {
            Some(code) => {
                let mut err = ErrorResponse::new(Some("rejected by mock deepgram".to_string()));
                *err.status_mut() =
                    StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                Err(err)
            }
            None => Ok(resp),
        }
    })
    .await
    {
        Ok(ws) => ws,
        Err(_) => return,
    };

    if let Ok(mut st) = state.lock() {
        st.connections += 1;
    }

    let (mut write, mut read) = ws.split();

    // The reader runs in parallel with the script: `RequireAudioBytes` only
    // works if audio keeps arriving while the script is blocked on it.
    let read_state = state.clone();
    let read_audio = audio_tx.clone();
    let reader = tokio::spawn(async move {
        while let Some(Ok(msg)) = read.next().await {
            match msg {
                WsMessage::Binary(bytes) => {
                    read_audio.send_modify(|total| *total += bytes.len());
                }
                WsMessage::Text(txt) => {
                    if let Ok(mut st) = read_state.lock() {
                        st.client_messages.push(txt.to_string());
                    }
                }
                _ => {}
            }
        }
    });

    let mut sequence_id = 0u64;
    if script.send_connected {
        let frame = json!({
            "type": "Connected",
            "request_id": "00000000-0000-4000-8000-000000000000",
            "sequence_id": sequence_id,
        })
        .to_string();
        sequence_id += 1;
        if write.send(WsMessage::Text(frame.into())).await.is_err() {
            return;
        }
    }

    let mut turn_index = 0u64;
    for action in script.actions {
        let frame = match action {
            DgAction::Delay(d) => {
                tokio::time::sleep(d).await;
                continue;
            }
            DgAction::CloseWs => {
                let _ = write.close().await;
                break;
            }
            DgAction::RequireAudioBytes(n) => {
                let mut rx = audio_tx.subscribe();
                while *rx.borrow() < n {
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
                continue;
            }
            DgAction::SendRaw(raw) => raw,
            DgAction::SendTurnInfo { event, transcript } => {
                let frame = json!({
                    "type": "TurnInfo",
                    "request_id": "00000000-0000-4000-8000-000000000000",
                    "sequence_id": sequence_id,
                    "event": event,
                    "turn_index": turn_index,
                    "audio_window_start": 0.0,
                    "audio_window_end": 1.0,
                    "transcript": transcript,
                    "words": [],
                    "end_of_turn_confidence": 0.5,
                })
                .to_string();
                if event == "EndOfTurn" {
                    turn_index += 1;
                }
                frame
            }
        };
        sequence_id += 1;
        if write.send(WsMessage::Text(frame.into())).await.is_err() {
            break;
        }
    }

    // Stay open after the script so the client can still send `CloseStream`;
    // the reader ends when the client goes away.
    let _ = reader.await;
}

/// Records the query parameters and whether a credential was presented.
fn record_request(
    req: &tokio_tungstenite::tungstenite::handshake::server::Request,
    state: &Arc<Mutex<State>>,
) {
    let query_pairs: Vec<(String, String)> = req
        .uri()
        .query()
        .unwrap_or("")
        .split('&')
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    // The map is the convenient view and loses repetitions; the pairs keep
    // them. Both are recorded so a test can pick whichever it needs.
    let query: HashMap<String, String> = query_pairs.iter().cloned().collect();
    let authorization_seen = req.headers().contains_key("authorization");
    if let Ok(mut st) = state.lock() {
        st.query = query;
        st.query_pairs = query_pairs;
        st.authorization_seen = authorization_seen;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpStream;

    /// The script of the adapter's first test, played against a raw client:
    /// the fake holds its transcripts back until the audio actually arrived,
    /// and reports query, credential presence and byte count afterwards.
    #[tokio::test]
    async fn script_waits_for_audio_then_sends_turn_info() {
        let mock = MockDeepgram::start(
            DeepgramScript::new()
                .require_audio_bytes(3200)
                .turn_info("Update", "hal")
                .turn_info("EndOfTurn", "hallo"),
        )
        .await
        .expect("mock starts");

        let url = format!(
            "{}/v2/listen?model=flux-general-multi&encoding=linear16&sample_rate=16000",
            mock.base_url()
        );
        let mut request =
            tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
                url.as_str(),
            )
            .expect("request builds");
        request
            .headers_mut()
            .insert("authorization", "Token secret".parse().expect("header"));
        let stream = TcpStream::connect(mock.base_url().trim_start_matches("ws://"))
            .await
            .expect("connects");
        let (mut ws, _) = tokio_tungstenite::client_async(request, stream)
            .await
            .expect("upgrades");

        let first = next_json(&mut ws).await;
        assert_eq!(first["type"], "Connected");

        // Nothing arrives before the audio does.
        let early = tokio::time::timeout(Duration::from_millis(150), ws.next()).await;
        assert!(early.is_err(), "fake must wait for the required audio");

        for _ in 0..5 {
            ws.send(WsMessage::Binary(vec![0u8; 640].into()))
                .await
                .expect("audio sent");
        }

        let update = next_json(&mut ws).await;
        assert_eq!(update["type"], "TurnInfo");
        assert_eq!(update["event"], "Update");
        assert_eq!(update["transcript"], "hal");

        let end = next_json(&mut ws).await;
        assert_eq!(end["event"], "EndOfTurn");
        assert_eq!(end["transcript"], "hallo");

        ws.send(WsMessage::Text(r#"{"type":"CloseStream"}"#.into()))
            .await
            .expect("close stream sent");
        // Give the reader a moment to record the frame.
        for _ in 0..50 {
            if !mock.client_messages().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(mock.received_audio_bytes(), 3200);
        assert_eq!(mock.connections(), 1);
        assert!(mock.saw_authorization());
        let query = mock.received_query();
        assert_eq!(
            query.get("model").map(String::as_str),
            Some("flux-general-multi")
        );
        assert_eq!(query.get("encoding").map(String::as_str), Some("linear16"));
        assert_eq!(query.get("sample_rate").map(String::as_str), Some("16000"));
        assert_eq!(
            mock.client_messages(),
            vec![r#"{"type":"CloseStream"}"#.to_string()]
        );
    }

    /// A rejected upgrade fails the handshake on the client's side — the shape
    /// an unusable credential has before any message is exchanged.
    #[tokio::test]
    async fn rejecting_status_fails_the_handshake() {
        let mock = MockDeepgram::start(DeepgramScript::new().rejecting_with(401))
            .await
            .expect("mock starts");
        let url = format!("{}/v2/listen", mock.base_url());
        let stream = TcpStream::connect(mock.base_url().trim_start_matches("ws://"))
            .await
            .expect("connects");
        let err = tokio_tungstenite::client_async(url.as_str(), stream)
            .await
            .expect_err("handshake is refused");
        assert!(
            format!("{err}").to_ascii_lowercase().contains("401")
                || matches!(err, tokio_tungstenite::tungstenite::Error::Http(_)),
            "unexpected error: {err}"
        );
    }

    /// A server-side close ends the client's stream.
    #[tokio::test]
    async fn close_ws_ends_the_stream() {
        let mock = MockDeepgram::start(DeepgramScript::new().close())
            .await
            .expect("mock starts");
        let url = format!("{}/v2/listen", mock.base_url());
        let stream = TcpStream::connect(mock.base_url().trim_start_matches("ws://"))
            .await
            .expect("connects");
        let (mut ws, _) = tokio_tungstenite::client_async(url.as_str(), stream)
            .await
            .expect("upgrades");
        let first = next_json(&mut ws).await;
        assert_eq!(first["type"], "Connected");
        let closed = tokio::time::timeout(Duration::from_secs(30), ws.next())
            .await
            .expect("close arrives");
        assert!(
            matches!(closed, Some(Ok(WsMessage::Close(_))) | None),
            "expected a close frame"
        );
    }

    async fn next_json<S>(
        ws: &mut tokio_tungstenite::WebSocketStream<S>,
    ) -> meclaw_core::serde_json::Value
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let msg = tokio::time::timeout(Duration::from_secs(30), ws.next())
            .await
            .expect("a frame arrives")
            .expect("stream is open")
            .expect("frame is readable");
        match msg {
            WsMessage::Text(txt) => meclaw_core::serde_json::from_str(&txt).expect("frame is json"),
            other => panic!("expected a text frame, got {other:?}"),
        }
    }
}
