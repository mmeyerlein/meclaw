//! Wave live (L6) — hermetic fake of the primary GPT-Live WebSocket, so the
//! `gpt_live` duplex provider can be tested without a key and without the
//! network.
//!
//! Same shape as `mock_openai_realtime`: a `TcpListener` on `127.0.0.1:0`, the
//! kernel picks the port, everything runs in a background task, and the
//! adapter points its `base_url` param at it. The protocol is a different one:
//! one socket carries the session, the audio of both directions and every
//! event, so the fake both replays a script and answers the client while the
//! script is still running.
//!
//! What the fake observes is what the protocol makes observable: whether the
//! upgrade carried a `Bearer` credential (never its value), the `session.start`
//! frame, every non-audio client frame verbatim, and the input audio — decoded,
//! and kept as one entry per append. That last one is the interesting record:
//! a client frame is an append and an append is a frame (R-L4), so a fake that
//! merged them would hide exactly the bug the rule exists for.
//!
//! Where the OpenAI documentation is silent, the behaviour is a script knob
//! rather than a guess: the delay before the handshake, the HTTP status of the
//! upgrade, whether `session.started` is sent at all and how the session ends
//! are all script parameters.

use crate::mock_openai_realtime::b64_decode;
use futures_util::SinkExt;
use futures_util::stream::{SplitSink, StreamExt};
use meclaw_core::serde_json::{Value as JsonValue, json};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// How long a blocking action (`RequireAudioBytes`, `ExpectAppend`,
/// `ExpectMute`, `ExpectUnmute`) waits before it gives up. Generous on
/// purpose: it is a failure marker, not a timing discriminator.
const EXPECT_CAP: Duration = Duration::from_secs(30);

/// How long the fake waits for `session.start` after the upgrade. The real
/// service has no such timeout; the fake needs one so a broken client fails
/// the test instead of hanging it.
const SESSION_START_CAP: Duration = Duration::from_secs(10);

/// The step of the `start_ms`/`end_ms` pair the fake hands back on an
/// acknowledged append. The measured service answered with 200 ms windows
/// (the recorded reference run of 21.09.2026, S0 `st-comm`: 9400/9600).
const APPENDED_STEP_MS: u64 = 200;

/// The writer half of the open socket. Reader task and script both send —
/// `ExpectAppend` answers from the script, a client `session.close` is
/// answered by the reader — so the sink is shared.
type Writer = Arc<Mutex<SplitSink<WebSocketStream<TcpStream>, WsMessage>>>;

/// One scripted server-side action on an open Live session.
#[derive(Clone, Debug)]
pub enum LiveAction {
    /// Send this JSON value as a server event, verbatim.
    Send(JsonValue),
    /// Send these bytes as one `session.output_audio.delta` (base64).
    SendAudio(Vec<u8>),
    /// Send one transcript fragment.
    Transcript {
        /// `user` (input transcript) or `assistant` (output transcript).
        speaker: &'static str,
        /// The fragment, sent verbatim — spaces and repetitions included.
        delta: &'static str,
        /// Start of the fragment on the session timeline.
        start_ms: u64,
        /// End of the fragment on the session timeline.
        end_ms: u64,
    },
    /// Send `session.delegation.created` with `target: "client"`.
    Delegation {
        /// The delegation id the event carries.
        id: &'static str,
        /// Offset of the delegation on the session timeline.
        offset_ms: u64,
    },
    /// Send `session.usage.updated`.
    Usage {
        /// Cumulative voice seconds — a snapshot, never a summand.
        seconds: f64,
        /// Context-window usage ratio, when the event carries one.
        ratio: Option<f64>,
    },
    /// Wait before the next action.
    Delay(Duration),
    /// Block the script until this many decoded input bytes have arrived.
    RequireAudioBytes(usize),
    /// Block the script until such an append arrived, then answer
    /// `session.<kind>.appended`.
    ExpectAppend {
        /// `commentary`, `thinking` or `instructions`.
        kind: &'static str,
        /// Substring the append's `content` has to contain.
        contains: &'static str,
    },
    /// Block until `session.input_audio.mute` arrived; answer `muted`.
    ExpectMute,
    /// Block until `session.input_audio.unmute` arrived; answer `unmuted`.
    ExpectUnmute,
    /// Send `session.closed`, then a close frame.
    CloseWith {
        /// The `reason` of the final event.
        reason: &'static str,
        /// The final `usage.seconds`.
        usage_seconds: f64,
    },
    /// Drop the socket without a close frame — the connection-lost path.
    DropSocket,
    /// Stop reading the socket, and with it stop answering transport pings.
    ///
    /// A WebSocket pong is written by tungstenite itself, on the next read
    /// after the ping arrived — so a fake that wants to play a socket whose far
    /// side has gone away has to stop READING, and this is that gesture. Every
    /// ping that arrives after it is still counted
    /// ([`MockGptLive::pings`]); none of them is answered. Nothing the client
    /// sends is recorded from here on either, which is what a dead peer looks
    /// like from the outside.
    GoDeaf,
}

/// A per-connection scenario.
#[derive(Clone, Debug)]
pub struct LiveScript {
    /// The session id the fake reports in `session.started`.
    pub session_id: String,
    /// Actions replayed in order once `session.start` has arrived.
    pub actions: Vec<LiveAction>,
    /// Reject the upgrade with this HTTP status instead of accepting it.
    pub upgrade_status: Option<u16>,
    /// Stall this long before the handshake, to drive connect-timeout tests.
    pub accept_delay: Option<Duration>,
    /// Send `session.started` on `session.start`. Default `true`; a test that
    /// exercises the handshake timeout switches it off.
    pub started: bool,
}

impl Default for LiveScript {
    fn default() -> Self {
        Self {
            session_id: "sess_mock".to_string(),
            actions: Vec::new(),
            upgrade_status: None,
            accept_delay: None,
            started: true,
        }
    }
}

#[derive(Default)]
struct State {
    connects: usize,
    authorization_is_bearer: bool,
    session_start: Option<JsonValue>,
    client_events: Vec<JsonValue>,
    appends: Vec<JsonValue>,
    audio: Vec<Vec<u8>>,
    audio_bytes: usize,
    closed_requested: bool,
    appended_ms: u64,
    pings: Vec<Vec<u8>>,
    deaf: bool,
}

/// A running fake GPT-Live primary socket. Dropping it stops the server.
pub struct MockGptLive {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockGptLive {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockGptLive {
    /// Binds and starts serving `script`. Returns once the port is known.
    pub async fn start(script: LiveScript) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let state = Arc::new(Mutex::new(State::default()));
        let st = state.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let st = st.clone();
                let script = script.clone();
                tokio::spawn(async move {
                    serve_ws(stream, st, script).await;
                });
            }
        });
        Ok(Self { addr, state, task })
    }

    /// Base URL to feed the adapter's `base_url` param, e.g.
    /// `ws://127.0.0.1:54321`. The adapter appends its own path
    /// (`/v1/live/sessions`).
    pub fn base_url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    /// How many sockets were opened — a duplex session is never reconnected,
    /// so a second one is a finding.
    pub async fn connects(&self) -> usize {
        self.state.lock().await.connects
    }

    /// The first `session.start` frame the client sent, if any.
    pub async fn session_start(&self) -> Option<JsonValue> {
        self.state.lock().await.session_start.clone()
    }

    /// Every non-audio client frame, in arrival order. Audio appends are
    /// counted instead (see [`MockGptLive::received_audio`]), because a
    /// session carries thousands of them.
    pub async fn client_events(&self) -> Vec<JsonValue> {
        self.state.lock().await.client_events.clone()
    }

    /// Input audio bytes received, counted after base64 decoding — the proof
    /// that whole PCM frames were encoded rather than a padded string of the
    /// right length.
    pub async fn received_audio_bytes(&self) -> usize {
        self.state.lock().await.audio_bytes
    }

    /// The decoded input audio, one entry per append: a client frame is an
    /// append and an append is a frame (R-L4). Merged frames would show up
    /// here as a shorter list.
    pub async fn received_audio(&self) -> Vec<Vec<u8>> {
        self.state.lock().await.audio.clone()
    }

    /// Every guidance `*.append` frame the client sent, in arrival order --
    /// `commentary`, `thinking` and `instructions`. The audio appends are not
    /// among them: a minute of speech is three thousand of them, and they are
    /// kept separately (see [`MockGptLive::received_audio`]).
    pub async fn appends(&self) -> Vec<JsonValue> {
        self.state.lock().await.appends.clone()
    }

    /// The payload of every transport ping the client sent, in arrival order.
    ///
    /// A keepalive the client sends ITSELF is the only frame that can tell a
    /// live socket from a dead one on a line where nobody speaks, so a fake
    /// that cannot be asked about it cannot test one (GH #798).
    pub async fn pings(&self) -> Vec<Vec<u8>> {
        self.state.lock().await.pings.clone()
    }

    /// Whether a `session.close` arrived — the graceful-close proof.
    pub async fn closed_requested(&self) -> bool {
        self.state.lock().await.closed_requested
    }

    /// Whether the upgrade carried an `Authorization` header with the `Bearer`
    /// scheme. The value is never stored: a credential has no business in a
    /// test fixture, a log or a failure text.
    pub async fn authorization_is_bearer(&self) -> bool {
        self.state.lock().await.authorization_is_bearer
    }
}

/// Upgrades to a WebSocket (or refuses to), waits for `session.start`, replays
/// the script and answers the client concurrently.
///
/// The `result_large_err` allow is not laziness: the handshake callback's
/// signature is fixed by tungstenite (`Result<Response, ErrorResponse>`), so
/// the error type is not ours to box.
#[allow(clippy::result_large_err)]
async fn serve_ws(stream: TcpStream, state: Arc<Mutex<State>>, script: LiveScript) {
    if let Some(d) = script.accept_delay {
        tokio::time::sleep(d).await;
    }

    let upgrade_status = script.upgrade_status;
    let mut authorization: Option<String> = None;
    let handshake = tokio_tungstenite::accept_hdr_async(stream, |req: &_, resp| {
        let req: &tokio_tungstenite::tungstenite::handshake::server::Request = req;
        authorization = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split_whitespace().next().unwrap_or("").to_string());
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
        st.authorization_is_bearer = authorization.as_deref() == Some("Bearer");
    }

    let ws = match handshake {
        Ok(ws) => ws,
        Err(_) => return,
    };
    let (write, mut read) = ws.split();
    let write: Writer = Arc::new(Mutex::new(write));

    // A client `session.close` is answered wherever the script happens to be —
    // the real service accepts it at any time — unless the script wants the
    // socket to die instead. The usage the answer reports is the one the
    // script would have reported itself.
    let drops_socket = script
        .actions
        .iter()
        .any(|a| matches!(a, LiveAction::DropSocket));
    let auto_close_usage = script
        .actions
        .iter()
        .rev()
        .find_map(|a| match a {
            LiveAction::CloseWith { usage_seconds, .. } => Some(*usage_seconds),
            _ => None,
        })
        .unwrap_or(0.0);

    // Client frames are collected in parallel with the script: the adapter
    // keeps appending audio while the script is still running, and serialising
    // the two would hide ordering bugs.
    let read_state = state.clone();
    let read_write = write.clone();
    let reader = tokio::spawn(async move {
        while let Some(Ok(msg)) = read.next().await {
            if let WsMessage::Ping(payload) = &msg {
                let mut st = read_state.lock().await;
                st.pings.push(payload.to_vec());
            }
            // Deliberately checked AFTER the frame was taken off the socket and
            // BEFORE the next read: tungstenite queues the pong when the ping
            // arrives and flushes it on the following read, so leaving the loop
            // here is what makes the last ping go unanswered.
            if read_state.lock().await.deaf {
                break;
            }
            let WsMessage::Text(txt) = msg else { continue };
            let Ok(v) = meclaw_core::serde_json::from_str::<JsonValue>(&txt) else {
                continue;
            };
            let kind = v
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_string();
            {
                let mut st = read_state.lock().await;
                match kind.as_str() {
                    "session.input_audio.append" => {
                        let decoded = v
                            .get("audio")
                            .and_then(|a| a.as_str())
                            .and_then(b64_decode)
                            .unwrap_or_default();
                        st.audio_bytes += decoded.len();
                        st.audio.push(decoded);
                        continue;
                    }
                    "session.start" => {
                        if st.session_start.is_none() {
                            st.session_start = Some(v.clone());
                        }
                    }
                    "session.close" => st.closed_requested = true,
                    _ => {}
                }
                if kind.ends_with(".append") {
                    st.appends.push(v.clone());
                }
                st.client_events.push(v);
            }
            if kind == "session.close" && !drops_socket {
                let closed = json!({
                    "type": "session.closed",
                    "event_id": "event_mock_closed",
                    "reason": "close_requested",
                    "usage": { "seconds": auto_close_usage }
                });
                let mut w = read_write.lock().await;
                let _ = w.send(WsMessage::Text(closed.to_string().into())).await;
                let _ = w.close().await;
                break;
            }
        }
    });

    if !wait_for(&state, SESSION_START_CAP, |st| st.session_start.is_some()).await {
        panic!("mock_gpt_live: no session.start within 10s — the script cannot start");
    }
    if script.started {
        let started = started_frame(&state, &script.session_id).await;
        if send(&write, started).await.is_err() {
            return;
        }
    }

    for action in script.actions {
        if !play(&write, &state, action).await {
            reader.abort();
            return;
        }
    }

    // Stay open after the script so the client can finish its side; the reader
    // ends when the client goes away.
    let _ = reader.await;
}

/// Plays one action. `false` means the socket is done — a scripted close, a
/// dropped socket, a dead peer or an expectation that never came true.
async fn play(write: &Writer, state: &Arc<Mutex<State>>, action: LiveAction) -> bool {
    let frame = match action {
        LiveAction::Delay(d) => {
            tokio::time::sleep(d).await;
            return true;
        }
        LiveAction::RequireAudioBytes(n) => {
            return wait_for(state, EXPECT_CAP, |st| st.audio_bytes >= n).await;
        }
        LiveAction::ExpectAppend { kind, contains } => {
            let want = format!("session.{kind}.append");
            let hit = |st: &State| {
                st.appends.iter().any(|a| {
                    a.get("type").and_then(|t| t.as_str()) == Some(want.as_str())
                        && a.get("content")
                            .and_then(|c| c.as_str())
                            .is_some_and(|c| c.contains(contains))
                })
            };
            if !wait_for(state, EXPECT_CAP, hit).await {
                return false;
            }
            let client_event_id = {
                let st = state.lock().await;
                st.appends
                    .iter()
                    .rev()
                    .find(|a| a.get("type").and_then(|t| t.as_str()) == Some(want.as_str()))
                    .and_then(|a| a.get("event_id").cloned())
                    .unwrap_or(JsonValue::Null)
            };
            let (start_ms, end_ms) = next_appended_window(state).await;
            json!({
                "type": format!("session.{kind}.appended"),
                "event_id": "event_mock_appended",
                "client_event_id": client_event_id,
                "start_ms": start_ms,
                "end_ms": end_ms
            })
        }
        LiveAction::ExpectMute | LiveAction::ExpectUnmute => {
            let muting = matches!(action, LiveAction::ExpectMute);
            let want = if muting {
                "session.input_audio.mute"
            } else {
                "session.input_audio.unmute"
            };
            let seen = |st: &State| {
                st.client_events
                    .iter()
                    .any(|e| e.get("type").and_then(|t| t.as_str()) == Some(want))
            };
            if !wait_for(state, EXPECT_CAP, seen).await {
                return false;
            }
            let client_event_id = {
                let st = state.lock().await;
                st.client_events
                    .iter()
                    .rev()
                    .find(|e| e.get("type").and_then(|t| t.as_str()) == Some(want))
                    .and_then(|e| e.get("event_id").cloned())
                    .unwrap_or(JsonValue::Null)
            };
            json!({
                "type": if muting { "session.input_audio.muted" } else { "session.input_audio.unmuted" },
                "event_id": "event_mock_mute_ack",
                "client_event_id": client_event_id
            })
        }
        LiveAction::CloseWith {
            reason,
            usage_seconds,
        } => {
            let closed = json!({
                "type": "session.closed",
                "event_id": "event_mock_closed",
                "reason": reason,
                "usage": { "seconds": usage_seconds }
            });
            if send(write, closed).await.is_err() {
                return false;
            }
            let _ = write.lock().await.close().await;
            return false;
        }
        LiveAction::GoDeaf => {
            state.lock().await.deaf = true;
            return true;
        }
        LiveAction::DropSocket => return false,
        LiveAction::Send(v) => v,
        LiveAction::SendAudio(bytes) => json!({
            "type": "session.output_audio.delta",
            "event_id": "event_mock_output_audio",
            "delta": b64_encode(&bytes)
        }),
        LiveAction::Transcript {
            speaker,
            delta,
            start_ms,
            end_ms,
        } => {
            let kind = match speaker {
                "user" => "session.input_transcript.delta",
                "assistant" => "session.output_transcript.delta",
                other => panic!("mock_gpt_live: unknown speaker {other:?}"),
            };
            json!({
                "type": kind,
                "event_id": "event_mock_transcript",
                "delta": delta,
                "start_ms": start_ms,
                "end_ms": end_ms
            })
        }
        LiveAction::Delegation { id, offset_ms } => json!({
            "type": "session.delegation.created",
            "event_id": "event_mock_delegation",
            "offset_ms": offset_ms,
            "delegation": { "id": id, "type": "delegation", "target": "client" }
        }),
        LiveAction::Usage { seconds, ratio } => {
            let mut event = json!({
                "type": "session.usage.updated",
                "event_id": "event_mock_usage",
                "usage": { "seconds": seconds }
            });
            if let Some(ratio) = ratio
                && let Some(obj) = event.as_object_mut()
            {
                obj.insert(
                    "context_window".to_string(),
                    json!({ "usage_ratio": ratio }),
                );
            }
            event
        }
    };
    send(write, frame).await.is_ok()
}

/// `session.started` carries the resolved configuration and the session id —
/// so the fake answers with what the client asked for, plus the id.
async fn started_frame(state: &Arc<Mutex<State>>, session_id: &str) -> JsonValue {
    let mut session = state
        .lock()
        .await
        .session_start
        .as_ref()
        .and_then(|v| v.get("session").cloned())
        .unwrap_or_else(|| json!({}));
    if let Some(obj) = session.as_object_mut() {
        obj.insert("id".to_string(), json!(session_id));
        obj.insert("object".to_string(), json!("live.session"));
    }
    json!({
        "type": "session.started",
        "event_id": "event_mock_started",
        "session": session
    })
}

/// The next `start_ms`/`end_ms` window of an acknowledged append. The service
/// hands back a running window rather than the wall clock, so the fake does
/// too — deterministic, and monotone across a session.
async fn next_appended_window(state: &Arc<Mutex<State>>) -> (u64, u64) {
    let mut st = state.lock().await;
    let start = st.appended_ms;
    st.appended_ms += APPENDED_STEP_MS;
    (start, start + APPENDED_STEP_MS)
}

async fn send(write: &Writer, frame: JsonValue) -> Result<(), ()> {
    write
        .lock()
        .await
        .send(WsMessage::Text(frame.to_string().into()))
        .await
        .map_err(|_| ())
}

/// Polls until `done` holds. `false` means the cap elapsed — the script then
/// stops rather than hanging a test forever.
async fn wait_for<F>(state: &Arc<Mutex<State>>, cap: Duration, done: F) -> bool
where
    F: Fn(&State) -> bool,
{
    let deadline = tokio::time::Instant::now() + cap;
    loop {
        if done(&*state.lock().await) {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// Standard-alphabet base64 encoder with padding — the counterpart of
/// [`crate::mock_openai_realtime::b64_decode`], and public because a test that
/// scripts audio needs to encode it. `base64` is not on the tech-stack
/// allow-list.
pub fn b64_encode(bytes: &[u8]) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock_openai_realtime::b64_decode;

    #[test]
    fn b64_encode_matches_known_vectors() {
        assert_eq!(b64_encode(b""), "");
        assert_eq!(b64_encode(b"f"), "Zg==");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_encode(b"foo"), "Zm9v");
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64_encode(&[0xFF, 0x0F, 0x80]), "/w+A");
    }

    #[test]
    fn b64_round_trips_every_byte() {
        let all: Vec<u8> = (0..=255u8).collect();
        assert_eq!(b64_decode(&b64_encode(&all)).expect("decodes"), all);
    }
}
