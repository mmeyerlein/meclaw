//! The three routes a `voice` cell serves: the socket, the declaration and the
//! built-in test page (wave voice-cell).
//!
//! The protocol itself is `docs/voice-wire-protocol.en.md`; this module is only
//! the door. There is no authentication and no TLS here — the listener binds
//! loopback by default and anything else belongs behind a reverse proxy, the
//! same stance the `web` cell takes.
//!
//! # Why `GET /info` exists
//!
//! The `hello` frame declares the two audio formats a client has to adapt to,
//! because the cell never resamples (R-V2). Before this route, learning them
//! meant opening a session — which is a side effect for a question that has
//! none. `/info` is that question with a read for an answer (R-V6): the same
//! declaration, minus the `session_id`, because nothing was opened.

use axum::Router;
use axum::extract::{Query, State, WebSocketUpgrade};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use meclaw_core::serde_json::json;
use std::sync::Arc;

use crate::voice::connection::run_connection;
use crate::voice::contract::AudioFormat;
use crate::voice::io::{VoiceIoShared, new_connection_slot};
use crate::voice::testpage;
use crate::voice::wire::{Mode, PROTOCOL};

/// The longest session identity a client may choose.
const SESSION_MAX: usize = 128;

/// The cell's router.
pub fn router(io: Arc<VoiceIoShared>) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/info", get(info))
        .route("/ws", get(ws_upgrade))
        .with_state(io)
}

/// The format the client will be sent, if anything is ever sent.
///
/// The echo provider is its own answer: what goes in comes back, so the output
/// format is the input format and no synthesis provider is involved.
pub(crate) fn audio_out(io: &VoiceIoShared) -> Option<AudioFormat> {
    if io.stt.name() == "echo" {
        return Some(io.stt.input_format());
    }
    io.tts.as_ref().map(|t| t.output_format())
}

/// The outbound frame length this instance will actually cut, in milliseconds.
///
/// `0` means "unframed": with no synthesis provider nothing is ever cut, and
/// the echo provider writes back exactly the frames it was given — byte-
/// identical is the whole point of it. Everywhere else it is the `params`
/// value, and it is declared so a client (a phone edge above all) can read what
/// it is about to be sent instead of measuring it.
pub(crate) fn audio_out_frame_ms(io: &VoiceIoShared) -> u32 {
    if io.stt.name() == "echo" || io.tts.is_none() {
        return 0;
    }
    io.audio_out_frame_ms
}

/// The synthesis provider's name, as the declaration reports it.
pub(crate) fn tts_name(io: &VoiceIoShared) -> Option<&'static str> {
    if io.stt.name() == "echo" {
        return None;
    }
    io.tts.as_ref().map(|t| t.name())
}

/// `GET /` — the built-in browser test page (R-V9).
async fn page() -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        testpage::html(),
    )
        .into_response()
}

/// `GET /info` — the `hello` declaration, without opening a session (R-V6).
async fn info(State(io): State<Arc<VoiceIoShared>>) -> Response {
    let body = json!({
        "protocol": PROTOCOL,
        "mode": io.default_mode,
        "audio_in": io.stt.input_format(),
        "audio_out": audio_out(&io),
        "audio_out_frame_ms": audio_out_frame_ms(&io),
        "stt": io.stt.name(),
        "tts": tts_name(&io),
        "speak_plain": io.speak_plain,
        "release_grace_ms": io.release_grace_ms,
    });
    (
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// What a client may ask for in the query string. All of it is optional.
#[derive(serde::Deserialize)]
struct WsQuery {
    /// The session identity the client chooses.
    session: Option<String>,
    /// The same field under the name a telephony edge already uses (R-V15).
    ///
    /// FreeSWITCH's `mod_audio_fork` and the gateways built around it carry a
    /// call identity as `session_token`, and a bridge that has to rewrite a
    /// query parameter is a bridge that can get it wrong. Same validation, same
    /// meaning; `session` wins when a client sends both.
    session_token: Option<String>,
    /// `auto` or `hold`, overriding the cell's default for this connection.
    mode: Option<String>,
}

/// Whether a client-chosen session identity is one this cell will address.
///
/// The identity is a key in a table and travels on every emission as
/// `hop.session_id`, so its shape is a contract rather than a preference:
/// bounded length, and no character that would make it ambiguous in a log line
/// or a URL.
fn session_is_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= SESSION_MAX
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
}

/// `GET /ws` — the upgrade. A plain `GET` is a `400`: the path is right, the
/// request is not.
async fn ws_upgrade(
    State(io): State<Arc<VoiceIoShared>>,
    Query(q): Query<WsQuery>,
    upgrade: Option<WebSocketUpgrade>,
) -> Response {
    let session_id = match q.session.or(q.session_token) {
        None => meclaw_core::Uuid::now_v7().to_string(),
        Some(s) if session_is_valid(&s) => s,
        // Refused rather than silently replaced: a client that asked for an
        // identity and got another one would address a session that is not its
        // own on every reconnect.
        Some(_) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("session must be 1..={SESSION_MAX} characters from [A-Za-z0-9._:-]\n"),
            )
                .into_response();
        }
    };
    let mode = match q.mode.as_deref() {
        None => io.default_mode,
        Some("auto") => Mode::Auto,
        Some("hold") => Mode::Hold,
        Some(_) => {
            return (StatusCode::BAD_REQUEST, "mode must be auto or hold\n").into_response();
        }
    };
    let Some(up) = upgrade else {
        return (
            StatusCode::BAD_REQUEST,
            "this path is a websocket endpoint\n",
        )
            .into_response();
    };
    let (conn_id, to_conn_tx, to_conn_rx) = new_connection_slot();
    up.on_upgrade(move |ws| {
        run_connection(ws, io, session_id, mode, conn_id, to_conn_tx, to_conn_rx)
    })
}
