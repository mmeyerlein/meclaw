//! P12 S8/S9 — hermetic fake Slack (Socket Mode + Web API) for tests.
//!
//! Same shape as `mock_http`: a `TcpListener` on `127.0.0.1:0`, the kernel picks
//! the port, everything runs in a background task. This one serves BOTH
//! protocols on that single port, because the real thing does too: the app calls
//! `apps.connections.open` over HTTP, receives a WebSocket URL, and connects
//! back. Peeking the first four bytes of each accepted connection is enough to
//! tell a `GET ` upgrade from a `POST`.
//!
//! Scripts are keyed by app token. That is what makes the multi-bot scenario
//! provable: two cells with two different tokens open two connections against
//! one fake, and each connection only ever replays its own script — exactly the
//! property that would be false if a naive implementation broadcast every event
//! to every socket.
//!
//! Where the Slack docs are silent, the behaviour is a knob rather than a guess.
//! The rate-limit body, the delay before the first frame and the disconnect
//! reason are all script parameters, so a test states which unverified
//! assumption it is exercising instead of hard-coding one as truth.

use futures_util::{SinkExt, StreamExt};
use meclaw_core::serde_json::{Value as JsonValue, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// One scripted server-side action on an open WebSocket.
#[derive(Clone, Debug)]
pub enum ServerAction {
    /// Send a raw frame verbatim (used for malformed-input tests).
    SendRaw(String),
    /// Send an `events_api` envelope wrapping this `event_callback` payload.
    SendEnvelope {
        /// Envelope id the client must echo back in its ack.
        envelope_id: String,
        /// The `event_callback` payload.
        payload: JsonValue,
    },
    /// Send a `disconnect` frame with this reason.
    SendDisconnect(String),
    /// Send a WebSocket ping. Slack keeps a Socket Mode connection audible this
    /// way even when the workspace produces no events, which is what makes an
    /// idle deadline on the read loop calibratable at all (issue #50).
    SendPing,
    /// Wait before the next action.
    Delay(Duration),
    /// Close the socket from the server side.
    CloseWs,
}

/// A per-connection scenario.
#[derive(Clone, Debug, Default)]
pub struct SlackScript {
    /// The app id announced in the `hello` frame. Loop rule R3 reads this.
    pub app_id: String,
    /// Actions replayed in order once the socket is up.
    pub actions: Vec<ServerAction>,
    /// Delay before the `hello` frame (Slack documents no timing guarantee).
    pub hello_delay: Option<Duration>,
}

impl SlackScript {
    /// Empty script announcing `app_id`.
    pub fn new(app_id: &str) -> Self {
        Self {
            app_id: app_id.to_string(),
            actions: Vec::new(),
            hello_delay: None,
        }
    }
    /// Queue an `events_api` envelope.
    pub fn envelope(mut self, envelope_id: &str, payload: JsonValue) -> Self {
        self.actions.push(ServerAction::SendEnvelope {
            envelope_id: envelope_id.to_string(),
            payload,
        });
        self
    }
    /// Queue a raw frame.
    pub fn raw(mut self, frame: &str) -> Self {
        self.actions.push(ServerAction::SendRaw(frame.to_string()));
        self
    }
    /// Queue a `disconnect`.
    pub fn disconnect(mut self, reason: &str) -> Self {
        self.actions
            .push(ServerAction::SendDisconnect(reason.to_string()));
        self
    }
    /// Queue a WebSocket ping — Slack's own keepalive on a quiet connection.
    pub fn ping(mut self) -> Self {
        self.actions.push(ServerAction::SendPing);
        self
    }
    /// Queue a pause.
    pub fn delay_ms(mut self, ms: u64) -> Self {
        self.actions
            .push(ServerAction::Delay(Duration::from_millis(ms)));
        self
    }
    /// Queue a server-side close.
    pub fn close(mut self) -> Self {
        self.actions.push(ServerAction::CloseWs);
        self
    }
    /// Hold the `hello` frame back, for connect-timeout tests.
    pub fn with_hello_delay(mut self, d: Duration) -> Self {
        self.hello_delay = Some(d);
        self
    }
}

/// A captured `chat.postMessage` call.
#[derive(Clone, Debug)]
pub struct CapturedPost {
    /// Bearer token the caller used — the proof of which bot identity posted.
    pub authorization: Option<String>,
    /// Parsed JSON body.
    pub body: JsonValue,
}

impl CapturedPost {
    /// `channel` field of the post.
    pub fn channel(&self) -> Option<&str> {
        self.body.get("channel").and_then(|v| v.as_str())
    }
    /// `text` field of the post.
    pub fn text(&self) -> Option<&str> {
        self.body.get("text").and_then(|v| v.as_str())
    }
    /// `thread_ts` field, absent when the reply is not threaded.
    pub fn thread_ts(&self) -> Option<&str> {
        self.body.get("thread_ts").and_then(|v| v.as_str())
    }
}

#[derive(Default)]
struct State {
    open_tokens: Vec<Option<String>>,
    posts: Vec<CapturedPost>,
    acks: Vec<String>,
    scripts: HashMap<String, Vec<SlackScript>>,
    conn_counts: HashMap<String, usize>,
    default_script: Option<SlackScript>,
    conn_tokens: HashMap<String, String>,
    next_conn: u64,
    open_error: Option<String>,
    post_error: Option<(u16, String)>,
    ws_connects: usize,
    http_delay: Option<Duration>,
}

/// A running fake Slack. Dropping it stops the server.
pub struct MockSlack {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MockSlack {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MockSlack {
    /// Binds and starts serving. Returns once the port is known.
    pub async fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let state = Arc::new(Mutex::new(State::default()));
        let st = state.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let st = st.clone();
                tokio::spawn(async move {
                    let _ = serve_conn(stream, st, addr).await;
                });
            }
        });
        Ok(Self { addr, state, task })
    }

    /// Web API base, e.g. `http://127.0.0.1:54321/api`. Feed this to the cell's
    /// `base_url` param — the same override mechanism the Telegram variant uses.
    pub fn base_url(&self) -> String {
        format!("http://{}/api", self.addr)
    }

    /// Register the script served to every connection opened with `app_token`.
    ///
    /// Scripts are keyed by app token rather than held globally, and that is a
    /// load-bearing property of the proof rather than a convenience: the
    /// multi-bot test asserts that an event addressed to bot A reaches only
    /// bot A. A fake that broadcast every scripted frame to every open socket
    /// would satisfy that assertion by construction — it would be the fake, not
    /// the implementation, producing the separation. Keyed scripts mean each
    /// connection can only ever observe what its own token was scripted.
    pub async fn script_for(&self, app_token: &str, script: SlackScript) {
        self.state
            .lock()
            .await
            .scripts
            .insert(app_token.to_string(), vec![script]);
    }

    /// Register a sequence: the n-th connection made with this token replays
    /// the n-th script, and the last one repeats thereafter. This is what makes
    /// reconnect observable — the first connection can drop the socket while
    /// the second delivers, which a single fixed script cannot express.
    pub async fn scripts_for(&self, app_token: &str, scripts: Vec<SlackScript>) {
        self.state
            .lock()
            .await
            .scripts
            .insert(app_token.to_string(), scripts);
    }

    /// Script for connections whose token has no specific script.
    pub async fn default_script(&self, script: SlackScript) {
        self.state.lock().await.default_script = Some(script);
    }

    /// Make `apps.connections.open` fail with this Slack error code.
    pub async fn fail_open_with(&self, error: &str) {
        self.state.lock().await.open_error = Some(error.to_string());
    }

    /// Make `chat.postMessage` fail with this HTTP status and error code.
    /// Slack has TWO rate-limit spellings (`rate_limited` on the app tier,
    /// `ratelimited` on the HTTP 429 path); both are expressible here.
    pub async fn fail_post_with(&self, status: u16, error: &str) {
        self.state.lock().await.post_error = Some((status, error.to_string()));
    }

    /// Stall every HTTP response by this long, to drive A-timeout tests.
    pub async fn delay_http(&self, d: Duration) {
        self.state.lock().await.http_delay = Some(d);
    }

    /// Envelope ids acknowledged by clients, in arrival order.
    pub async fn acks(&self) -> Vec<String> {
        self.state.lock().await.acks.clone()
    }

    /// Captured `chat.postMessage` calls.
    pub async fn posts(&self) -> Vec<CapturedPost> {
        self.state.lock().await.posts.clone()
    }

    /// How many times `apps.connections.open` was called — the reconnect proof.
    pub async fn open_calls(&self) -> usize {
        self.state.lock().await.open_tokens.len()
    }

    /// Authorization headers seen on `apps.connections.open`.
    pub async fn open_tokens(&self) -> Vec<Option<String>> {
        self.state.lock().await.open_tokens.clone()
    }

    /// How many WebSocket connections were established.
    pub async fn ws_connects(&self) -> usize {
        self.state.lock().await.ws_connects
    }
}

/// Splits an accepted connection into the HTTP or the WebSocket path.
async fn serve_conn(
    stream: TcpStream,
    state: Arc<Mutex<State>>,
    addr: SocketAddr,
) -> std::io::Result<()> {
    let mut head = [0u8; 4];
    // peek leaves the bytes in the socket for whichever handler takes over.
    let n = stream.peek(&mut head).await?;
    if n == 4 && &head == b"GET " {
        serve_ws(stream, state).await;
    } else {
        serve_http(stream, state, addr).await?;
    }
    Ok(())
}

/// Minimal HTTP/1.1: read head, read body by Content-Length, answer, close.
async fn serve_http(
    mut stream: TcpStream,
    state: Arc<Mutex<State>>,
    addr: SocketAddr,
) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let head_end = loop {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(p) = find_subslice(&buf, b"\r\n\r\n") {
            break p;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let path = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string();
    let authorization = header_value(&head, "authorization");
    let content_length: usize = header_value(&head, "content-length")
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);

    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    let body_json: JsonValue =
        meclaw_core::serde_json::from_slice(&body).unwrap_or(JsonValue::Null);

    let (status, payload) = if path.ends_with("apps.connections.open") {
        let mut st = state.lock().await;
        st.open_tokens.push(authorization.clone());
        if let Some(err) = st.open_error.clone() {
            (200u16, json!({ "ok": false, "error": err }))
        } else {
            st.next_conn += 1;
            let conn = format!("/link/{}", st.next_conn);
            if let Some(tok) = authorization
                .as_deref()
                .and_then(|a| a.strip_prefix("Bearer "))
            {
                st.conn_tokens.insert(conn.clone(), tok.to_string());
            }
            (
                200,
                json!({ "ok": true, "url": format!("ws://{addr}{conn}") }),
            )
        }
    } else if path.ends_with("chat.postMessage") {
        let mut st = state.lock().await;
        st.posts.push(CapturedPost {
            authorization: authorization.clone(),
            body: body_json.clone(),
        });
        if let Some((code, err)) = st.post_error.clone() {
            (code, json!({ "ok": false, "error": err }))
        } else {
            (
                200,
                json!({
                    "ok": true,
                    "channel": body_json.get("channel").cloned().unwrap_or(JsonValue::Null),
                    "ts": "1503435956.000247",
                    "message": { "bot_id": "B_FAKE", "subtype": "bot_message" }
                }),
            )
        }
    } else {
        (404, json!({ "ok": false, "error": "unknown_method" }))
    };

    if let Some(d) = state.lock().await.http_delay {
        tokio::time::sleep(d).await;
    }

    let body = payload.to_string();
    let mut resp = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
        body.len()
    );
    if status == 429 {
        resp.push_str("Retry-After: 1\r\n");
    }
    resp.push_str("Connection: close\r\n\r\n");
    resp.push_str(&body);
    stream.write_all(resp.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

/// Upgrades to a WebSocket, replays the script, and records acks concurrently.
///
/// The `result_large_err` allow is not laziness: the handshake callback's
/// signature is fixed by tungstenite (`Result<Response, ErrorResponse>`), so the
/// error type is not ours to box.
#[allow(clippy::result_large_err)]
async fn serve_ws(stream: TcpStream, state: Arc<Mutex<State>>) {
    let mut path = String::new();
    let ws = match tokio_tungstenite::accept_hdr_async(stream, |req: &_, resp| {
        path = uri_path(req);
        Ok(resp)
    })
    .await
    {
        Ok(ws) => ws,
        Err(_) => return,
    };

    let script = {
        let mut st = state.lock().await;
        st.ws_connects += 1;
        let token = st.conn_tokens.get(&path).cloned();
        let picked = token.as_ref().and_then(|t| {
            let idx = *st.conn_counts.get(t).unwrap_or(&0);
            st.conn_counts.insert(t.clone(), idx + 1);
            let seq = st.scripts.get(t)?;
            // Past the end of the sequence the last script repeats, so a test
            // only has to spell out the connections it actually cares about.
            seq.get(idx).or_else(|| seq.last()).cloned()
        });
        picked
            .or_else(|| st.default_script.clone())
            .unwrap_or_else(|| SlackScript::new("A_FAKE"))
    };

    let (mut write, mut read) = ws.split();

    // Acks are collected in parallel with the script: a client acknowledges
    // while further frames are still arriving, and serialising the two would
    // hide ordering bugs.
    let ack_state = state.clone();
    let reader = tokio::spawn(async move {
        while let Some(Ok(msg)) = read.next().await {
            if let WsMessage::Text(txt) = msg
                && let Ok(v) = meclaw_core::serde_json::from_str::<JsonValue>(&txt)
                && let Some(id) = v.get("envelope_id").and_then(|x| x.as_str())
            {
                ack_state.lock().await.acks.push(id.to_string());
            }
        }
    });

    if let Some(d) = script.hello_delay {
        tokio::time::sleep(d).await;
    }
    let hello = json!({
        "type": "hello",
        "connection_info": { "app_id": script.app_id },
        "num_connections": 1,
        "debug_info": { "host": "fake-slack", "approximate_connection_time": 3600 }
    })
    .to_string();
    if write.send(WsMessage::Text(hello.into())).await.is_err() {
        return;
    }

    for action in script.actions {
        let frame = match action {
            ServerAction::Delay(d) => {
                tokio::time::sleep(d).await;
                continue;
            }
            ServerAction::CloseWs => {
                let _ = write.close().await;
                break;
            }
            ServerAction::SendPing => {
                if write
                    .send(WsMessage::Ping(Vec::new().into()))
                    .await
                    .is_err()
                {
                    break;
                }
                continue;
            }
            ServerAction::SendRaw(raw) => raw,
            ServerAction::SendDisconnect(reason) => json!({
                "type": "disconnect",
                "reason": reason,
                "debug_info": { "host": "fake-slack" }
            })
            .to_string(),
            ServerAction::SendEnvelope {
                envelope_id,
                payload,
            } => json!({
                "type": "events_api",
                "envelope_id": envelope_id,
                "accepts_response_payload": false,
                "payload": payload
            })
            .to_string(),
        };
        if write.send(WsMessage::Text(frame.into())).await.is_err() {
            break;
        }
    }

    // Stay open after the script so the client can finish acking; the reader
    // ends when the client goes away.
    let _ = reader.await;
}

fn uri_path(req: &tokio_tungstenite::tungstenite::handshake::server::Request) -> String {
    req.uri().path().to_string()
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn header_value(head: &str, name: &str) -> Option<String> {
    head.lines()
        .find(|l| {
            l.to_ascii_lowercase()
                .starts_with(&format!("{}:", name.to_ascii_lowercase()))
        })
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
}

/// Builds an `event_callback` payload around one inner event — the shape every
/// Socket Mode envelope carries.
pub fn event_callback(api_app_id: &str, event: JsonValue) -> JsonValue {
    json!({
        "type": "event_callback",
        "api_app_id": api_app_id,
        "team_id": "T0001",
        "event_id": "Ev0001",
        "event_time": 1515449522i64,
        "event": event
    })
}

/// An `app_mention` event. `thread_ts` present means the mention happened
/// inside an existing thread.
pub fn app_mention(
    channel: &str,
    user: &str,
    text: &str,
    ts: &str,
    thread_ts: Option<&str>,
) -> JsonValue {
    let mut ev = json!({
        "type": "app_mention",
        "channel": channel,
        "user": user,
        "text": text,
        "ts": ts,
        "event_ts": ts.replace('.', "")
    });
    if let Some(t) = thread_ts {
        ev["thread_ts"] = json!(t);
    }
    ev
}

/// A plain `message` event in a conversation of `channel_type`.
pub fn message_event(
    channel: &str,
    channel_type: &str,
    user: &str,
    text: &str,
    ts: &str,
    thread_ts: Option<&str>,
) -> JsonValue {
    let mut ev = json!({
        "type": "message",
        "channel": channel,
        "channel_type": channel_type,
        "user": user,
        "text": text,
        "ts": ts,
        "event_ts": ts.replace('.', "")
    });
    if let Some(t) = thread_ts {
        ev["thread_ts"] = json!(t);
    }
    ev
}
