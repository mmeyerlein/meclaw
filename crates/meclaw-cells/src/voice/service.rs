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
use meclaw_colony::surfaces::{BoxFuture, LINK_QUEUE};
use meclaw_colony::{Link, LinkOpener, LinkRefused, LinkRequest};
use meclaw_core::serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::voice::connection::run_connection;
use crate::voice::contract::{AudioFormat, Encoding};
use crate::voice::io::{VoiceIoShared, new_connection_slot};
use crate::voice::link::ClientLink;
use crate::voice::testpage;
use crate::voice::wire::{Mode, PROTOCOL};

/// The longest session identity a client may choose.
const SESSION_MAX: usize = 128;

/// The two formats one connection runs at (GH #619).
///
/// They are not necessarily the same rate, and that asymmetry is the design:
/// what a client SENDS has to be understood, so a rate the recogniser cannot
/// serve is a refused connection; what it is SENT it can merely dislike, so a
/// rate the synthesiser cannot serve falls back to the provider's own and is
/// declared. The cell resamples in neither direction (R-V2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Negotiated {
    /// What the client must send, and what the recognition session runs at.
    pub audio_in: AudioFormat,
    /// What the client will be sent; `None` when there is no synthesis.
    pub audio_out: Option<AudioFormat>,
}

/// The cell's router.
pub fn router(io: Arc<VoiceIoShared>) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/info", get(info))
        .route("/ws", get(ws_upgrade))
        .with_state(io)
}

/// The cell's router under a mount's prefix.
///
/// The one listener hands over a stream whose request line already carries
/// `/<mount>/…`, so the paths a mounted cell answers are its own three routes
/// with the mount in front of them. Nothing else differs: it is the same router,
/// the same state, the same admission.
///
/// The extra route is axum's trailing slash: `nest("/voice", …)` answers
/// `/voice` for the inner `/` and `/voice/info` for the inner `/info`, but
/// **not** `/voice/` — the wildcard it registers does not match an empty rest.
/// A person types the slash, and the page is the same page, so it is named
/// rather than left as a 404 nobody can explain.
pub fn mounted_router(io: Arc<VoiceIoShared>, mount: &str) -> Router {
    Router::new()
        .nest(&format!("/{mount}"), router(Arc::clone(&io)))
        .route(&format!("/{mount}/"), get(page).with_state(io))
}

/// The format the client will be sent, if anything is ever sent.
///
/// With the echo provider and NO synthesis provider the answer is the input
/// format: what goes in comes back, unchanged.
pub(crate) fn audio_out(io: &VoiceIoShared) -> Option<AudioFormat> {
    negotiate(io, None).map(|n| n.audio_out).unwrap_or_default()
}

/// What this cell would agree to for a client asking for `sample_rate`.
///
/// `None` is a refusal, and it has exactly one cause: the recogniser does not
/// serve that rate.
pub(crate) fn negotiate(io: &VoiceIoShared, sample_rate: Option<u32>) -> Option<Negotiated> {
    let audio_in = match sample_rate {
        None => io.stt.input_format(),
        Some(rate) => io.stt.negotiate_input(rate)?,
    };
    let audio_out = match io.tts.as_ref() {
        // The loopback with nothing behind it: the frames that come back are
        // the frames that went in, so the declaration is the input format.
        None if io.stt.name() == "echo" => Some(audio_in),
        None => None,
        // WITH a synthesis provider the provider is asked, echo or not. The
        // echo branch used to short-circuit here and declare the input format
        // for both directions -- and a cell configured `stt: echo` with an
        // OpenAI synthesis block then announced 8000 in `hello` and put 24000
        // on the socket, because `start_speak` reads `shared.tts` and does not
        // care which recogniser is in front of it. A declaration nobody can
        // act on is worse than no declaration.
        Some(t) => Some(match sample_rate {
            None => t.output_format(),
            // The fallback, and the reason it is not a refusal: a caller who
            // has to hear 24 kHz over an 8 kHz line hears something; a caller
            // whose words are never transcribed says nothing at all.
            Some(rate) => t
                .negotiate_output(rate)
                .unwrap_or_else(|| t.output_format()),
        }),
    };
    Some(Negotiated {
        audio_in,
        audio_out,
    })
}

/// Every outbound rate a client may negotiate, for `GET /info`.
///
/// It follows [`negotiate`] exactly, including the loopback case: a cell that
/// hands frames straight back can be asked for any rate its recogniser takes,
/// so `audio_out_rates` mirrors `audio_in_rates` there instead of being `null`
/// beside an `audio_out` that is set.
pub(crate) fn audio_out_rates(io: &VoiceIoShared) -> Option<Vec<u32>> {
    match io.tts.as_ref() {
        None if io.stt.name() == "echo" => Some(io.stt.input_rates()),
        None => None,
        Some(t) => Some(t.output_rates()),
    }
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
        // GH #619: what a client MAY ask for, so it learns the negotiable set
        // by reading rather than by being refused (R-V6). `audio_in`/
        // `audio_out` above are what it gets when it asks for nothing.
        "audio_in_rates": io.stt.input_rates(),
        "audio_out_rates": audio_out_rates(&io),
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
    /// The rate this client sends, and would like to be sent (GH #619).
    ///
    /// Absent means "whatever the providers declare", which is what every
    /// client did before this parameter existed. Present, it is BINDING for
    /// the inbound direction: the recognition session runs at it, or the
    /// connection is refused. For the outbound direction it is a wish.
    sample_rate: Option<u32>,
    /// The sample encoding this client speaks. Absent means the one this
    /// version has; a name it does not have is refused rather than assumed,
    /// because a telephony edge configured for companded audio would otherwise
    /// send bytes that are perfectly valid noise.
    encoding: Option<String>,
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

/// What an admitted client got, before anything of its own exists.
pub(crate) struct Admitted {
    /// The session identity this connection speaks for.
    pub(crate) session_id: String,
    /// The mode it starts in.
    pub(crate) mode: Mode,
    /// The two formats it runs at.
    pub(crate) negotiated: Negotiated,
}

/// Decide whether this cell will serve a client, and on what terms.
///
/// One parser for both doors (the one-parser rule of this tree,
/// `docs/cell-types.md` § `web` on `component.define`): the socket door calls it
/// before the upgrade, a topic on a display's socket calls it before the link
/// exists. So a display page reads the same sentence a `curl` would, and a
/// refusal cannot mean two different things depending on how a client arrived.
///
/// The status travels beside the text because the socket door answers HTTP with
/// it and the topic door repeats both to whoever joined.
pub(crate) fn admit(
    io: &VoiceIoShared,
    session: Option<String>,
    session_token: Option<String>,
    mode: Option<&str>,
    sample_rate: Option<u32>,
    encoding: Option<&str>,
) -> Result<Admitted, (StatusCode, String)> {
    let session_id = match session.or(session_token) {
        None => meclaw_core::Uuid::now_v7().to_string(),
        Some(s) if session_is_valid(&s) => s,
        // Refused rather than silently replaced: a client that asked for an
        // identity and got another one would address a session that is not its
        // own on every reconnect.
        Some(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("session must be 1..={SESSION_MAX} characters from [A-Za-z0-9._:-]\n"),
            ));
        }
    };
    let mode = match mode {
        None => io.default_mode,
        Some("auto") => Mode::Auto,
        Some("hold") => Mode::Hold,
        Some(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                "mode must be auto or hold\n".to_string(),
            ));
        }
    };
    // GH #619. Both refusals answer BEFORE anything is opened, like `session`
    // and `mode` above and for the same reason: there is nothing to close, and a
    // `400` a client reads in its connect error is louder than a close code it
    // has to look up.
    if let Some(name) = encoding
        && Encoding::parse(name).is_none()
    {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "encoding must be `{}`; this protocol version speaks no other\n",
                Encoding::PcmS16Le.as_str()
            ),
        ));
    }
    let Some(negotiated) = negotiate(io, sample_rate) else {
        let asked = sample_rate.unwrap_or_default();
        let rates = io
            .stt
            .input_rates()
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "sample_rate {asked} is not one the `{}` recogniser serves ({rates}); \
                 this cell never resamples\n",
                io.stt.name()
            ),
        ));
    };
    Ok(Admitted {
        session_id,
        mode,
        negotiated,
    })
}

/// The second door: whoever holds a socket asks this to open a link on it.
///
/// Same admission, same connection code as the socket door — what differs is
/// only what carries the frames (`voice/link.rs`). The opener holds the I/O
/// half's shared state, so a link is served by the cell that owns it rather than
/// by whoever forwarded the frames.
pub struct VoiceLinkOpener {
    /// The I/O half this opener speaks for.
    pub shared: Arc<VoiceIoShared>,
}

impl LinkOpener for VoiceLinkOpener {
    fn open(&self, req: LinkRequest) -> BoxFuture<'static, Result<Link, LinkRefused>> {
        let shared = Arc::clone(&self.shared);
        Box::pin(async move {
            let admitted = admit(
                &shared,
                req.session,
                None,
                req.mode.as_deref(),
                req.sample_rate,
                req.encoding.as_deref(),
            )
            .map_err(|(status, detail)| LinkRefused {
                status: status.as_u16(),
                detail,
            })?;
            let (conn_id, to_conn_tx, to_conn_rx) = new_connection_slot();
            let (to_cell_tx, to_cell_rx) = mpsc::channel(LINK_QUEUE);
            let (from_cell_tx, from_cell_rx) = mpsc::channel(LINK_QUEUE);
            tokio::spawn(run_connection(
                ClientLink::chan(to_cell_rx, from_cell_tx),
                shared,
                admitted.session_id,
                admitted.mode,
                admitted.negotiated,
                conn_id,
                to_conn_tx,
                to_conn_rx,
            ));
            Ok(Link {
                to_cell: to_cell_tx,
                from_cell: from_cell_rx,
            })
        })
    }
}

/// `GET /ws` — the upgrade. A plain `GET` is a `400`: the path is right, the
/// request is not.
async fn ws_upgrade(
    State(io): State<Arc<VoiceIoShared>>,
    Query(q): Query<WsQuery>,
    upgrade: Option<WebSocketUpgrade>,
) -> Response {
    let Admitted {
        session_id,
        mode,
        negotiated,
    } = match admit(
        &io,
        q.session,
        q.session_token,
        q.mode.as_deref(),
        q.sample_rate,
        q.encoding.as_deref(),
    ) {
        Ok(admitted) => admitted,
        Err((status, detail)) => return (status, detail).into_response(),
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
        run_connection(
            ClientLink::ws(ws),
            io,
            session_id,
            mode,
            negotiated,
            conn_id,
            to_conn_tx,
            to_conn_rx,
        )
    })
}
