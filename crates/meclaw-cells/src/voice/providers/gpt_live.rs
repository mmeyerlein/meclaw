//! The `gpt_live` duplex provider: one WebSocket that carries audio in, audio
//! out and the model's events.
//!
//! The endpoint is the primary session socket (`/v1/live/sessions`): the
//! session opens with a `session.start`, audio rides on
//! `session.input_audio.append` and comes back on
//! `session.output_audio.delta`, and the model's transcripts, delegations and
//! meter readings arrive on the same connection. The credential is a Bearer
//! header and exists nowhere else — not in a query string, not in a log, not in
//! an error.
//!
//! # What this adapter does not do
//!
//! It does not pace or re-cut audio in either direction: one item out of
//! `audio_in` is one `session.input_audio.append`, one
//! `session.output_audio.delta` is one item into `audio_out` (R-L4), and the
//! bounded channels of [`DuplexSession`] are the backpressure (ADR-0023
//! § Consequences). The one queue it has is the short one of its [`Uplink`]:
//! it stands empty while the socket takes what it is given, and while it does
//! not, the reading goes on (GH #1630).
//!
//! It also holds no reconnect policy (OR-L20): one session is one socket and
//! one conversation, so a socket that ends, ends the session — the connection
//! task decides what that means for the call.
//!
//! # What ends a session, and how
//!
//! An `Err` from [`GptLiveDuplex::run_session`] is a disturbance: a refused
//! credential, an unanswered handshake, a socket that went silent. Everything
//! else — the caller hanging up, the model closing, the far side vanishing
//! mid-call — is an ordinary end, reported as [`DuplexEvent::Closed`] with the
//! provider's own reason and meter reading, and returned as `Ok(())`. The
//! connection task turns an `Err` into `error duplex_failed` on the colony's
//! lane, and a finished call is not an incident.

use crate::voice::b64;
use crate::voice::contract::{
    AppendKind, AudioArrivals, AudioFormat, BoxFuture, DuplexControl, DuplexError, DuplexEvent,
    DuplexProvider, DuplexSession, LATE_LISTED_FROM_MS, LATENESS_FROM_MS, ProviderTimeouts,
    SeedMessage, Speaker,
};
use crate::voice::g711::{self, Law};
use crate::voice::params::{AudioWire, G711_SAMPLE_RATE, GptLiveParams};
use futures_util::{Sink, SinkExt, StreamExt};
use meclaw_colony::io_liveness::IoLivenessMark;
use serde_json::{Value as JsonValue, json};
use std::collections::VecDeque;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{oneshot, watch};
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Bytes;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// The model this adapter speaks to unless a param says otherwise (R-25-0).
pub const DEFAULT_MODEL: &str = "gpt-live-1.1";

/// The host the session socket lives on. US, because the live endpoints are
/// there and the region question is deferred (R-25-5).
pub const DEFAULT_BASE_URL: &str = "wss://api.openai.com";

/// The voice the model speaks with unless a param says otherwise. They all
/// speak German; none of them is billed as a German voice.
pub const DEFAULT_VOICE: &str = "marin";

/// The payload of the keepalive ping this adapter sends, and the mark that
/// tells its own pong from anybody else's (R-L7, GH #798).
///
/// A pong carries back the payload of the ping it answers, so a pong with these
/// bytes in it is the answer to a question THIS session asked — and only such a
/// pong resets the idle deadline. Short, printable and unmistakable: it turns
/// up in a packet capture as itself.
pub const KEEPALIVE_PAYLOAD: &[u8] = b"meclaw";

/// A gap between two output chunks above this many milliseconds is counted
/// separately. The measurement of the real endpoint (S0) saw a worst gap of
/// 629 ms and 16 gaps over this line per five minutes, against 112 ms and none
/// on the loopback — so this is the line between "the network breathed" and
/// "the caller heard a hole", and L9's B-6 reads the two counters off the
/// session's closing log line.
const LONG_GAP_MS: u64 = 250;

/// How many frames wait for the provider's socket before the session stops
/// taking the caller's audio and the colony's guidance (GH #1630). At one
/// frame per 20 ms this is 320 ms, well over the slowest send measured (38 ms),
/// and short enough that a mute is not long behind the audio before it.
const SEND_QUEUE: usize = 16;

/// The socket this adapter runs on, once [`open_socket`] handed it over.
type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
/// The reading half.
type Reader = futures_util::stream::SplitStream<Socket>;

/// Opens the provider's socket, held to `within` (hard rule 12).
///
/// With Nagle off (GH #1632): every frame of the caller's audio is one small
/// write, and Nagle holds a small write back until the one before it is
/// acknowledged.
async fn open_socket(
    request: impl IntoClientRequest + Unpin,
    within: Duration,
) -> Result<Socket, DuplexError> {
    match tokio::time::timeout(
        within,
        tokio_tungstenite::connect_async_with_config(request, None, true),
    )
    .await
    {
        Err(_) => Err(DuplexError::Timeout),
        Ok(Err(e)) => Err(classify_connect(e)),
        Ok(Ok((ws, _response))) => Ok(ws),
    }
}

/// The session socket's URL, from a base that may be written either way.
///
/// `http(s)` is rewritten to `ws(s)` — the same courtesy
/// [`crate::voice::providers::openai_stt`] does, and for the same reason: an
/// operator copies a base URL out of a vendor's documentation, where it is
/// written as HTTP, and a refused handshake is a poor way to learn about a
/// scheme (R-V10).
pub fn session_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let base = match base.strip_prefix("https://") {
        Some(rest) => format!("wss://{rest}"),
        None => match base.strip_prefix("http://") {
            Some(rest) => format!("ws://{rest}"),
            None => base.to_string(),
        },
    };
    format!("{base}/v1/live/sessions")
}

/// A live speech-to-speech session over one socket.
pub struct GptLiveDuplex {
    /// Everything the session is opened with.
    params: GptLiveParams,
    /// The two deadlines this adapter is held to (hard rule 12).
    timeouts: ProviderTimeouts,
}

impl GptLiveDuplex {
    /// An adapter for `params`, on the default deadlines.
    pub fn new(params: GptLiveParams) -> Self {
        Self {
            params,
            timeouts: ProviderTimeouts::default(),
        }
    }

    /// Hold this adapter to the cell's two deadlines (hard rule 12).
    pub fn with_timeouts(mut self, t: ProviderTimeouts) -> Self {
        self.timeouts = t;
        self
    }

    /// The settings this session will be opened with.
    pub fn params(&self) -> &GptLiveParams {
        &self.params
    }

    /// The deadlines this adapter is held to.
    pub fn timeouts(&self) -> ProviderTimeouts {
        self.timeouts
    }
}

impl DuplexProvider for GptLiveDuplex {
    fn name(&self) -> &'static str {
        "gpt_live"
    }

    /// One format for both directions, at the configured rate.
    ///
    /// No `rates()` override: the session is STARTED at this rate, so it is the
    /// only one this instance serves, and a client asking for another is
    /// refused rather than converted (R-V2).
    ///
    /// A G.711 session (R-LS-3) declares L16 at 8000 to its client: the edge
    /// streams L16 only (`mod_audio_stream`, OR-LV-5), and this adapter
    /// compands each frame on its way to and from the model.
    fn format(&self) -> AudioFormat {
        match self.params.audio_format {
            AudioWire::Pcm16 => AudioFormat::pcm16_mono(self.params.sample_rate),
            AudioWire::G711Ulaw | AudioWire::G711Alaw => AudioFormat::pcm16_mono(G711_SAMPLE_RATE),
        }
    }

    fn run_session(
        &self,
        format: AudioFormat,
        session: DuplexSession,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), DuplexError>> {
        let params = self.params.clone();
        let timeouts = self.timeouts;
        Box::pin(async move {
            run_session(params, timeouts, format, session, liveness, true, None).await
        })
    }

    /// The same session, without the greeting (GH #896).
    ///
    /// A renewed session opens in the middle of a call: the caller has been
    /// greeted already, and a second greeting would be the model introducing
    /// itself again mid-sentence. Everything else — the instructions, the
    /// voice, the deadlines — is the session [`Self::run_session`] opens.
    fn run_renewed_session(
        &self,
        format: AudioFormat,
        session: DuplexSession,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), DuplexError>> {
        let params = self.params.clone();
        let timeouts = self.timeouts;
        Box::pin(async move {
            run_session(params, timeouts, format, session, liveness, false, None).await
        })
    }

    fn run_metered_session(
        &self,
        format: AudioFormat,
        session: DuplexSession,
        liveness: IoLivenessMark,
        renewed: bool,
        arrivals: oneshot::Sender<AudioArrivals>,
    ) -> BoxFuture<Result<(), DuplexError>> {
        let params = self.params.clone();
        let timeouts = self.timeouts;
        Box::pin(async move {
            run_session(
                params,
                timeouts,
                format,
                session,
                liveness,
                !renewed,
                Some(arrivals),
            )
            .await
        })
    }
}

/// One live session on one socket.
///
/// `format` rather than `params.sample_rate` is what the session is opened at:
/// it is the rate the connection negotiated with its client, and with only one
/// rate on offer (see [`GptLiveDuplex::format`]) the two are the same value —
/// but the one the client was promised is the one the model has to speak.
///
/// `greet` says whether the start frame carries the greeting (GH #1628). It is
/// `false` for a renewed session of a running call (GH #896): the greeting is
/// what the caller hears FIRST, and a renewal is not first.
///
/// `arrivals` takes how the audio arrived, once, when the session ends.
async fn run_session(
    params: GptLiveParams,
    timeouts: ProviderTimeouts,
    format: AudioFormat,
    session: DuplexSession,
    liveness: IoLivenessMark,
    greet: bool,
    arrivals: Option<oneshot::Sender<AudioArrivals>>,
) -> Result<(), DuplexError> {
    let DuplexSession {
        mut audio_in,
        audio_out,
        events,
        mut control,
    } = session;
    // The seed's deadline counts from the moment the session was asked for,
    // not from the handshake: the handshake (median 505 ms) and the wait for
    // the curator run side by side, and the caller waits for the longer one.
    let seed_by = Instant::now() + Duration::from_millis(params.seed_wait_ms);
    let law = params.audio_format.law();

    let url = session_url(&params.base_url);
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| DuplexError::Connect(format!("base_url is not a websocket url: {e}")))?;
    // The credential exists only inside this header value. It is never logged,
    // never put in an error and never sent as a query parameter.
    let header = format!("Bearer {}", params.api_key.expose())
        .parse()
        .map_err(|_| DuplexError::Auth("authorization header could not be built".to_string()))?;
    request.headers_mut().insert("authorization", header);
    // Every model request names its app (GH #1455, as the llm lane, GH
    // #1225): unset means no header, a value that is no header value is a
    // configuration error before any byte leaves.
    for (name, value) in [
        ("http-referer", &params.http_referer),
        ("x-title", &params.x_title),
    ] {
        if let Some(v) = value {
            let v = v
                .parse()
                .map_err(|_| DuplexError::Connect(format!("{name} is not a valid header value")))?;
            request.headers_mut().insert(name, v);
        }
    }

    // A-timeout around the handshake (hard rule 12): a silent TCP peer must not
    // park this task with no operation-level guard.
    let ws = open_socket(request, timeouts.external).await?;
    // The far side completed a handshake: a full external round trip.
    liveness.mark_success();
    let (mut write, mut read) = ws.split();

    // The seed (D48): with `seed_wait_ms` set, the opening waits for it, and
    // what else the session is told meanwhile is held, not lost.
    let waited = if params.seed_wait_ms > 0 {
        await_seed(&mut control, &mut audio_in, seed_by).await
    } else {
        SeedWait::default()
    };
    // Review K1: a call that ended while its session waited costs no
    // session -- no `session.start`, no greeting, the socket just closes.
    if waited.ended {
        tracing::info!("gpt_live: the call ended while the session waited for its seed");
        return Ok(());
    }
    let seed = match waited.seed.as_deref() {
        Some(messages) => {
            // OR-d54g.kprofil.14: the seed is cut to the start budget the
            // `seed_ask` lane named, not to `seed_max_tokens` alone. A
            // builder that sizes by its own knob, or an older one that reads
            // no `limits`, may send more than the instructions leave; cut to
            // the ceiling alone, instructions and seed together went past
            // the provider's bound again (R-58-28).
            let budget_tokens = start_budget(&params);
            // GH #1628: the greeting is one message of the same input, and
            // the provider takes 128 of them; the seed leaves it the place.
            let max_messages = if greets(&params, greet) {
                params.seed_max_messages.min(SEED_MAX_MESSAGES - 1)
            } else {
                params.seed_max_messages
            };
            let trim = if greets(&params, greet) {
                trim_folded
            } else {
                trim_seed
            };
            let trimmed = trim(
                messages,
                budget_tokens,
                max_messages,
                params.chars_per_token,
            );
            if trimmed.trimmed > 0 {
                tracing::info!(
                    seed_trimmed = trimmed.trimmed,
                    budget_tokens,
                    "gpt_live: seed_trimmed -- the oldest messages left the seed"
                );
            }
            tracing::info!(
                messages = trimmed.kept.len(),
                est_tokens = trimmed.est_tokens,
                "gpt_live: seed_used"
            );
            Some(trimmed.kept)
        }
        None if params.seed_wait_ms > 0 => {
            tracing::info!(
                seed_wait_ms = params.seed_wait_ms,
                "gpt_live: seed_late -- the session starts without its seed"
            );
            None
        }
        None => None,
    };

    // The epoch is read once, through the clock helper the crate already has;
    // every later time is that reading plus the monotonic clock. A test
    // overrides it through the parameter it is handed over in (wall-clock:
    // opened_epoch_ms).
    let mut meter = SessionMeter::new(
        law,
        format.sample_rate,
        crate::proxy::meclaw::mount::now_ms(),
        arrivals,
    );
    let began = Instant::now();
    let sent = send_within(
        &mut write,
        text_frame(&session_start_frame(
            &params,
            format,
            seed.as_deref(),
            greet,
        )),
        timeouts.external,
    )
    .await;
    meter.saw_send(began.elapsed());
    sent?;

    // One deadline for the whole wait, not one per frame: a peer that keeps
    // sending frames that are not `session.started` would otherwise reset the
    // guard for ever, which is the one thing it exists to prevent.
    let started_by = tokio::time::Instant::now() + timeouts.external;
    let session_id = await_started(&mut read, started_by).await?;
    liveness.mark_success();
    if events
        .send(DuplexEvent::Started {
            session_id: session_id.clone(),
        })
        .await
        .is_err()
    {
        return Ok(());
    }
    // GH #1403: the lab's forced provider abort counts from here, the start
    // of the session, on the tokio clock. `0` arms nothing: its arm in the
    // loop below is never polled and the session is the one it always was.
    let fault_close_at_ms = params.fault_close_at_ms;
    let fault_close = tokio::time::sleep_until(
        tokio::time::Instant::now() + Duration::from_millis(fault_close_at_ms),
    );
    tokio::pin!(fault_close);

    // From here on the writing half belongs to its own task, and nothing below
    // waits for a send (GH #1630).
    let mut uplink = Uplink::spawn(write, timeouts.external);
    meter.sends = Some(uplink.times.clone());

    // What arrived while the opening waited for its seed, in its order of
    // arrival (review K2).
    for held in waited.held {
        let frame = match held {
            Held::Audio(bytes) => audio_append_frame(law, &bytes),
            Held::Control(DuplexControl::Append {
                kind,
                event_id,
                delegation_id,
                content,
            }) => append_frame(kind, &event_id, delegation_id.as_deref(), &content),
            Held::Control(DuplexControl::Mute) => json!({ "type": "session.input_audio.mute" }),
            Held::Control(DuplexControl::Unmute) => {
                json!({ "type": "session.input_audio.unmute" })
            }
            // A close ends the wait and is never held; a second seed is late.
            Held::Control(DuplexControl::Close | DuplexControl::Seed { .. }) => continue,
        };
        uplink.offer_json(&frame);
    }

    let mut control_open = true;
    // The idle deadline lives OUTSIDE the loop and is reset only by a frame
    // that actually arrived. Wrapping the read in `timeout(...)` inside the
    // `select!` would look identical and be useless: every time the audio arm
    // wins, the timeout future is dropped and rebuilt, so against a talking
    // caller and a silent model it would never fire — exactly the case it
    // exists for.
    //
    // What it counts is the last SESSION frame of any kind — an audio delta, a
    // transcript fragment, a meter reading, an ack — and not the meter (R-L7).
    // The sentence that stood here said "the model streams audio and its meter
    // arrives every 15 s", and half of that is measured false: the period is
    // right (nineteen intervals of 14 999-15 005 ms, S0 `a-pacing.json`) and
    // the ARRIVAL is not — a session fed nothing but silence saw no meter at
    // all inside 45 s over four runs (GH #798). Had the deadline really rested
    // on the meter, every quiet line would have been cut here. It never did,
    // and this comment is the only thing that changes.
    //
    // And the session frames alone cannot tell a caller who paused from a wire
    // that died -- thirty seconds of silence on a telephone line read as a dead
    // socket and the call was cut. How quiet a live session really goes while
    // nobody talks is NOT settled here: four runs on 2026-09-21 saw no session
    // frame at all inside 45 s, two runs on 2026-09-22 saw no gap longer than
    // 604 ms on the same endpoint, and what makes the difference is unmeasured
    // (GH #798, open point 6 of the strand report). The keepalive does not rest
    // on that answer. So this adapter ASKS: the keepalive arm below pings every
    // `params.keepalive_ms`, and the pong that comes back on OUR payload resets
    // the deadline too. What it measures is the socket; a caller's pause is not
    // a fault of the wire.
    //
    // A keepalive somebody else sends is the one frame that deliberately does
    // not count; the `Ping`/`Pong` arm below says why.
    let idle_deadline = tokio::time::sleep(timeouts.idle);
    tokio::pin!(idle_deadline);
    // Our own question to the socket, on its own period. `interval_at` rather
    // than `interval` because the first tick of a tokio interval returns at
    // once, and the handshake three lines up already proved the far side is
    // there. `Delay` for the same reason the cell's clock uses it: a ping that
    // was late is not a ping that is owed.
    let keepalive_every = Duration::from_millis(params.keepalive_ms.max(1));
    let mut keepalive = tokio::time::interval_at(
        tokio::time::Instant::now() + keepalive_every,
        keepalive_every,
    );
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // A15: whether this session already sent its one Responses correction.
    let mut responses_corrected = false;
    // The control marks of the model's own transcript, and the piece of one
    // a chunk boundary left behind (d54g callseam).
    let mut marks = ControlMarks::default();
    // A45: tracked only when the note is set on a Responses session;
    // otherwise a Responses delegation is ignored, as it always was.
    let note_superseded =
        !params.responses_superseded_note.is_empty() && params.is_responses_delegation();
    let mut open_responses = OpenResponses::default();
    // What the model said that the connection has no room for yet (GH
    // #1511). The connection hands guidance over with
    // `control_tx.send(..).await` and reads this session's outlets in the same
    // loop, so it reads nothing while it waits for room on `control`. A session
    // that waited on a full outlet the same way -- `events.send(..).await` --
    // and read no guidance meanwhile made each side wait for the other for
    // ever; neither wait is an operation on the socket, so no A-timeout
    // covered it. The frame is held here instead: the socket is not read while
    // it waits (backpressure on the model, as before), guidance and the
    // caller's audio still are.
    let mut held: Option<Said> = None;
    let ending = loop {
        tokio::select! {
            // `biased` so guidance and a close are acted on before the audio
            // already queued behind them — a mute that arrives late is a mute
            // that did not happen — and so the idle deadline is read before the
            // socket that may have gone quiet.
            biased;

            // A send that failed ends the session; room in the queue takes the
            // frame that waited for it.
            step = uplink.step() => if let Err(e) = step {
                break Ending::SendFailed(e);
            },

            // The inlets are read while the send queue has room: a full one
            // holds the frame it refused, and the next stays in its channel
            // (backpressure, no drop and no overtaking).
            command = control.recv(), if control_open && uplink.has_room() => match command {
                Some(DuplexControl::Append { kind, event_id, delegation_id, content }) => {
                    let frame = append_frame(kind, &event_id, delegation_id.as_deref(), &content);
                    uplink.offer_json(&frame);
                }
                Some(DuplexControl::Mute) => {
                    let frame = json!({ "type": "session.input_audio.mute" });
                    uplink.offer_json(&frame);
                }
                Some(DuplexControl::Unmute) => {
                    let frame = json!({ "type": "session.input_audio.unmute" });
                    uplink.offer_json(&frame);
                }
                Some(DuplexControl::Close) => {
                    uplink.offer_json(&close_frame());
                    break Ending::Finalise;
                }
                // A seed is part of the opening, and the session has opened.
                Some(DuplexControl::Seed { .. }) => {
                    tracing::warn!("gpt_live: a seed after the start is dropped, it came late");
                }
                // The colony let go of the guidance channel. The audio is the
                // session and it is still open, so this arm simply stops being
                // polled — a closed receiver answers instantly and would spin.
                None => control_open = false,
            },

            chunk = audio_in.recv(), if uplink.has_room() => match chunk {
                Some(bytes) => {
                    // One item, one append. The connection checked the parity;
                    // what arrives here goes on the wire as it is (R-L4) --
                    // companded first on a G.711 session, sample by sample.
                    let frame = audio_append_frame(law, &bytes);
                    uplink.offer_json(&frame);
                }
                None => {
                    // The client went away: `audio_in` closing IS the end of
                    // the call (contract § 1.1). No guard on this arm and none
                    // needed: the loop leaves here rather than carrying on with
                    // a receiver that would answer `None` for ever.
                    uplink.offer_json(&close_frame());
                    break Ending::Finalise;
                }
            },

            // GH #1403: the lab's switch ends the session the way the
            // provider's own `session.closed reason=content` does (`map_event`),
            // so what follows is the connection's recovery and nothing new.
            // Said once on the colony's lane, so a lab tells a forced abort
            // from a real one; the `session.close` is best effort, so the
            // provider stops billing a session nobody uses any more.
            () = &mut fault_close, if fault_close_at_ms > 0 && held.is_none() => {
                tracing::warn!(
                    fault_close_at_ms,
                    "gpt_live: fault_close_fired -- the lab's forced provider abort"
                );
                let detail = format!(
                    "fault_close_fired: session ended {fault_close_at_ms} ms after its start \
                     as reason=content (fault_close_at_ms, a lab switch)"
                );
                let warning = DuplexEvent::Warning { detail, provider_code: None, client_event_id: None };
                if draining(events.send(warning), &mut control, &mut audio_in).await.is_err() {
                    break Ending::HandlerGone;
                }
                uplink.offer_json(&close_frame());
                let sent = tokio::time::timeout(timeouts.external, uplink.drained());
                let _ = draining(sent, &mut control, &mut audio_in).await;
                break Ending::Reported {
                    reason: "content".to_string(),
                    usage_seconds: meter.usage_seconds,
                };
            }

            // Not while a frame waits for the connection: the socket is not
            // read then, and a stalled outlet is not a dead socket. The
            // deadline is read again as soon as the frame is handed on.
            () = &mut idle_deadline, if held.is_none() => {
                return Err(DuplexError::Closed(format!(
                    "idle for {:?}, no frame — socket presumed dead",
                    timeouts.idle
                )));
            }

            // Ask the socket whether it is still there (R-L7, GH #798). The
            // payload is [`KEEPALIVE_PAYLOAD`], which is what makes the answer
            // recognisable as the answer to THIS question. It goes through the
            // queue like every other frame and is under the same A-timeout.
            _ = keepalive.tick(), if uplink.has_room() => {
                uplink.offer(WsMessage::Ping(Bytes::from_static(KEEPALIVE_PAYLOAD)));
            }

            // The frame the connection had no room for, as soon as it has.
            permit = events.reserve(), if matches!(held, Some(Said::Event(_))) => match permit {
                Ok(permit) => {
                    if let Some(Said::Event(event)) = held.take() {
                        permit.send(event);
                    }
                }
                Err(_) => break Ending::HandlerGone,
            },
            permit = audio_out.reserve(), if matches!(held, Some(Said::Audio(_))) => match permit {
                Ok(permit) => {
                    if let Some(Said::Audio(bytes)) = held.take() {
                        permit.send(bytes);
                        meter.released(Instant::now());
                    }
                }
                Err(_) => break Ending::ClientGone,
            },

            incoming = read.next(), if held.is_none() => {
                let message = match incoming {
                    // A socket that ended before it was asked to is not this
                    // adapter's failure — it is the end of the call, with the
                    // reason said out loud.
                    None => break Ending::ConnectionLost,
                    Some(Err(e)) => {
                        // Same end, same `Ok(())` (OR-L.L2a.3) — but a socket
                        // that tore is not a caller who hung up, and an
                        // `Ok(())` never becomes an `error duplex_failed`.
                        // This warning is the only thing that tells the two
                        // apart on the colony's lane: L2b puts it there as
                        // `duplex_warning` (vertraege.md § 1.4). Without it a
                        // provider resetting every session at ten seconds
                        // reads as a run of short, ordinary calls.
                        tracing::debug!(error = %e, "gpt_live: socket ended mid-session");
                        let detail = format!("socket ended mid-session: {e}");
                        let warning = DuplexEvent::Warning { detail, provider_code: None, client_event_id: None };
                        if draining(events.send(warning), &mut control, &mut audio_in).await.is_err() {
                            break Ending::HandlerGone;
                        }
                        break Ending::ConnectionLost;
                    }
                    Some(Ok(message)) => message,
                };
                let text = match message {
                    WsMessage::Text(text) => text.to_string(),
                    WsMessage::Close(_) => break Ending::ConnectionLost,
                    // The answer to our own keepalive, and the only frame
                    // outside the session that resets the deadline: a pong
                    // carries back the payload of the ping it answers, so
                    // these bytes mean the far side of THIS socket answered a
                    // question this session asked. A full round trip, so the
                    // I/O watchdog is fed as well -- what keeps it alive is the
                    // peer's answer, never our own asking.
                    //
                    // The consequence, said out loud (OR-L.zell-uhr.1): a
                    // provider that answers pings but has stopped sending
                    // session frames no longer trips the watchdog either. That
                    // is what "dead" means from here on -- answers nothing at
                    // all -- and not "has nothing left to say". A deadline
                    // against a mute-but-answering model would be a different
                    // one, and this is not it.
                    WsMessage::Pong(payload)
                        if payload.as_ref() == KEEPALIVE_PAYLOAD =>
                    {
                        idle_deadline
                            .as_mut()
                            .reset(tokio::time::Instant::now() + timeouts.idle);
                        liveness.mark_success();
                        continue;
                    }
                    // Anything else on the transport is the transport talking,
                    // not the session: "reset only by a real frame" means a
                    // frame the session is made of or an answer it asked for,
                    // or a proxy pinging a dead upstream holds the line open
                    // for as long as it likes (OR-L.zell-uhr.1).
                    WsMessage::Ping(_)
                    | WsMessage::Pong(_)
                    | WsMessage::Binary(_)
                    | WsMessage::Frame(_) => continue,
                };
                // A session frame arrived: the far side is audibly alive, so
                // the deadline starts over — and only here.
                idle_deadline
                    .as_mut()
                    .reset(tokio::time::Instant::now() + timeouts.idle);
                liveness.mark_success();
                match marks.spoken(map_event(&text)) {
                    Mapped::Emit(event) => {
                        meter.note(&event);
                        // A15: the provider rejected the session's Responses
                        // settings and kept them; one correction per session,
                        // where the block names one. A second rejection is the
                        // correction failing, and only the warning reports it.
                        if let DuplexEvent::Warning { provider_code: Some(code), .. } = &event
                            && code == RESPONSES_VALIDATION_FAILED
                            && !responses_corrected
                            && let Some(fallback) = &params.responses_fallback
                        {
                            responses_corrected = true;
                            tracing::info!(
                                fallback = %fallback,
                                "gpt_live: responses settings rejected -- sending the \
                                 responses_fallback as session.update"
                            );
                            let frame = responses_correction_frame(fallback);
                            uplink.offer_json(&frame);
                        }
                        // A full channel holds the frame (see `held`):
                        // backpressure on the model, never a drop, and never
                        // a wait that stops the guidance.
                        match events.try_send(event) {
                            Ok(()) => {}
                            Err(TrySendError::Full(event)) => held = Some(Said::Event(event)),
                            Err(TrySendError::Closed(_)) => break Ending::HandlerGone,
                        }
                    }
                    Mapped::Audio(bytes) => {
                        meter.saw_output(Instant::now(), &bytes);
                        match audio_out.try_send(expand_out(law, bytes)) {
                            Ok(()) => {}
                            Err(TrySendError::Full(bytes)) => {
                                meter.holds(Instant::now());
                                held = Some(Said::Audio(bytes));
                            }
                            Err(TrySendError::Closed(_)) => break Ending::ClientGone,
                        }
                    }
                    Mapped::Finished { reason, usage_seconds } => {
                        break Ending::Reported { reason, usage_seconds };
                    }
                    Mapped::ResponsesCreated(id) if note_superseded => {
                        for old in open_responses.created(&id) {
                            tracing::info!(
                                superseded = %old,
                                superseded_by = %id,
                                "gpt_live: responses delegation overtaken -- sending the \
                                 responses_superseded_note as thinking"
                            );
                            let frame =
                                superseded_note_frame(&old, &params.responses_superseded_note);
                            uplink.offer_json(&frame);
                        }
                    }
                    Mapped::ResponsesDone(id) if note_superseded => open_responses.done(&id),
                    Mapped::ResponsesCreated(_) | Mapped::ResponsesDone(_) | Mapped::Ignore => {}
                }
            }
        }
    };
    // An outlet nobody reads any more ends the session like a hang-up, and the
    // provider has to learn of it the same way. With a stalled uplink the
    // hang-up itself may not be seen at all: the inlets are not read then, and
    // the model's next chunk finds its outlet gone first (GH #1630).
    let gone = match &ending {
        Ending::HandlerGone => Some("handler_gone"),
        Ending::ClientGone => Some("client_gone"),
        _ => None,
    };
    if gone.is_some() {
        control.close();
        audio_in.close();
        uplink.offer_json(&close_frame());
    }
    let closes = gone.is_some() || matches!(ending, Ending::Finalise);
    if closes {
        meter.closes();
    }
    let mut closing = Closing {
        uplink: &mut uplink,
        deadline: Instant::now() + timeouts.external,
        grace: Duration::from_millis(params.close_grace_ms),
        sent: !closes,
    };
    // A close can overtake a frame that waited for room: it is still what the
    // model said, and it goes first. An outlet that is gone takes nothing.
    let mut unsent = None;
    if let Some(said) = held.take() {
        let handed = draining(
            hand_on(said, &audio_out, &events),
            &mut control,
            &mut audio_in,
        );
        unsent = closing.beside(handed).await.err();
        meter.released(Instant::now());
    }
    let mut failure = None;
    let (reason, usage_seconds, report) = match ending {
        Ending::Finalise | Ending::HandlerGone | Ending::ClientGone => {
            let last_words = match unsent {
                Some(e) => Err(e),
                None => {
                    let outlets = Outlets {
                        audio_out: &audio_out,
                        events: &events,
                        control: &mut control,
                        audio_in: &mut audio_in,
                        gone,
                    };
                    finalise(&mut read, closing, outlets, &liveness, &mut meter, law).await
                }
            };
            match (last_words, gone) {
                (Ok((reason, seconds)), None) => (reason, seconds, true),
                // Nobody is left to tell, and a close that failed here failed
                // nobody: the log line below is the only record of it.
                (Ok((_, seconds)), Some(gone)) => (gone.to_string(), seconds, false),
                (Err(_), Some(gone)) => (gone.to_string(), meter.usage_seconds, false),
                (Err(e), None) => {
                    failure = Some(e);
                    ("send_failed".to_string(), meter.usage_seconds, false)
                }
            }
        }
        Ending::Reported {
            reason,
            usage_seconds,
        } => (reason, usage_seconds, true),
        Ending::ConnectionLost => ("connection_lost".to_string(), meter.usage_seconds, true),
        Ending::SendFailed(e) => {
            failure = Some(e);
            ("send_failed".to_string(), meter.usage_seconds, false)
        }
    };
    if report {
        let closed = DuplexEvent::Closed {
            reason: reason.clone(),
            usage_seconds,
        };
        let _ = draining(events.send(closed), &mut control, &mut audio_in).await;
    }
    meter.read_sends();
    // The measurement points of the wave, one line per session: L9's B-6 reads
    // the three audio counters off it, and `usage_seconds` is what the session
    // cost.
    tracing::info!(
        session_id = %session_id,
        reason = %reason,
        usage_seconds,
        out_chunks = meter.out_chunks,
        out_gap_max_ms = meter.out_gap_max_ms,
        out_gaps_over_250 = meter.out_gaps_over_250,
        in_send_max_ms = meter.in_send_max_ms,
        late_chunks = meter.late_chunks(),
        late_max_ms = meter.arrivals.late_max_ms,
        lateness = ?meter.arrivals.lateness,
        held_max_ms = meter.arrivals.held_max_ms,
        send_wait_max_ms = meter.arrivals.send_wait_max_ms,
        "gpt_live session ended"
    );
    match failure {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// A frame of the model that waited for room on its outlet.
enum Said {
    /// For `events`.
    Event(DuplexEvent),
    /// For `audio_out`, already expanded to the session's wire format.
    Audio(Vec<u8>),
}

/// Waits for `send` to the connection while still taking what the connection
/// hands over (GH #1511).
///
/// Only used where the session is over or ending: guidance and caller audio
/// that arrive meanwhile have no session left to reach and are dropped. What
/// matters is that they are TAKEN -- the connection reads this session's
/// outlets only between its own sends, so a session that stopped reading
/// `control` or `audio_in` while it waited on an outlet waited on the very
/// connection that waited on it.
async fn draining<F: std::future::Future>(
    send: F,
    control: &mut mpsc::Receiver<DuplexControl>,
    audio_in: &mut mpsc::Receiver<Vec<u8>>,
) -> F::Output {
    tokio::pin!(send);
    let mut control_open = true;
    let mut audio_open = true;
    loop {
        tokio::select! {
            biased;
            out = &mut send => return out,
            command = control.recv(), if control_open => match command {
                Some(_) => tracing::debug!("gpt_live: guidance after the session ended is dropped"),
                None => control_open = false,
            },
            chunk = audio_in.recv(), if audio_open => {
                if chunk.is_none() {
                    audio_open = false;
                }
            }
        }
    }
}

/// Hands `said` to its outlet; `false` when nobody reads it any more.
async fn hand_on(
    said: Said,
    audio_out: &mpsc::Sender<Vec<u8>>,
    events: &mpsc::Sender<DuplexEvent>,
) -> bool {
    match said {
        Said::Event(event) => events.send(event).await.is_ok(),
        Said::Audio(bytes) => audio_out.send(bytes).await.is_ok(),
    }
}

/// The `session.close` on its way out, the last frame of the send queue.
struct Closing<'a> {
    uplink: &'a mut Uplink,
    /// Until when the whole queue has to be on the wire; from then on, until
    /// when the provider may answer.
    deadline: Instant,
    /// How long the provider may take to answer, from the sent close.
    grace: Duration,
    /// Whether the close is on the wire, or none was asked for.
    sent: bool,
}

impl Closing<'_> {
    /// Waits until the close is on the wire and starts the grace. Cancel safe;
    /// pending for ever once it is.
    async fn out(&mut self) -> Result<(), DuplexError> {
        if self.sent {
            return std::future::pending().await;
        }
        match tokio::time::timeout_at(self.deadline, self.uplink.drained()).await {
            Err(_) => return Err(DuplexError::Timeout),
            Ok(sent) => sent?,
        }
        self.sent = true;
        self.deadline = Instant::now() + self.grace;
        Ok(())
    }

    /// Runs `wait` to its end while the close goes out beside it: an outlet
    /// that stands does not hold the close, and only a socket that takes
    /// nothing is a failure of it.
    async fn beside<F: std::future::Future>(&mut self, wait: F) -> Result<F::Output, DuplexError> {
        tokio::pin!(wait);
        loop {
            tokio::select! {
                biased;
                done = &mut wait => return Ok(done),
                sent = self.out() => if let Err(e) = sent {
                    // What the model said is handed on all the same.
                    wait.await;
                    return Err(e);
                },
            }
        }
    }
}

/// The session's channels as [`finalise`] needs them: the two outlets it
/// hands the model's last words to, and the two inlets it keeps taking from
/// while it waits on an outlet (see [`draining`]).
struct Outlets<'a> {
    audio_out: &'a mpsc::Sender<Vec<u8>>,
    events: &'a mpsc::Sender<DuplexEvent>,
    control: &'a mut mpsc::Receiver<DuplexControl>,
    audio_in: &'a mut mpsc::Receiver<Vec<u8>>,
    /// The end of the connection that is gone already, and its word for it.
    gone: Option<&'static str>,
}

/// Why the streaming loop stopped.
enum Ending {
    /// A `session.close` is the last frame of the send queue; the provider's
    /// own `session.closed` is still to come.
    Finalise,
    /// The provider closed the session itself and said so.
    Reported {
        /// The provider's own word for it.
        reason: String,
        /// The final meter reading.
        usage_seconds: f64,
    },
    /// The socket ended before anybody asked it to.
    ConnectionLost,
    /// The cell stopped listening to events; a `session.close` still goes out.
    HandlerGone,
    /// The connection stopped taking output audio; a `session.close` still
    /// goes out.
    ClientGone,
    /// The socket would not take a frame in time. This IS a disturbance and
    /// leaves as `Err` — but through the tail, so the session still writes its
    /// measurement line: a session that died on a slow send is exactly the one
    /// whose `in_send_max_ms` L9's B-6 wants to read. Every other ending reads
    /// the send that still hangs as long as it ran ([`SendTimes`]).
    SendFailed(DuplexError),
}

/// Reads until `session.started`, or until the deadline says the peer is not
/// going to answer.
///
/// An `error` here is the verdict on the `session.start` that was just sent and
/// therefore fatal; once the session is running the same event is about one
/// item and is a warning (R-V17).
async fn await_started(
    read: &mut Reader,
    deadline: tokio::time::Instant,
) -> Result<String, DuplexError> {
    loop {
        let message = match tokio::time::timeout_at(deadline, read.next()).await {
            Err(_) => return Err(DuplexError::Timeout),
            Ok(None) => {
                return Err(DuplexError::Closed(
                    "socket ended before session.started".to_string(),
                ));
            }
            Ok(Some(Err(e))) => return Err(DuplexError::Closed(format!("ws read: {e}"))),
            Ok(Some(Ok(message))) => message,
        };
        let text = match message {
            WsMessage::Text(text) => text.to_string(),
            WsMessage::Close(_) => {
                return Err(DuplexError::Closed(
                    "socket closed before session.started".to_string(),
                ));
            }
            WsMessage::Ping(_)
            | WsMessage::Pong(_)
            | WsMessage::Binary(_)
            | WsMessage::Frame(_) => {
                continue;
            }
        };
        let Ok(event) = serde_json::from_str::<JsonValue>(&text) else {
            tracing::warn!("gpt_live: unparsable frame before session.started, ignored");
            continue;
        };
        match event
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or_default()
        {
            "session.started" => {
                return Ok(event
                    .get("session")
                    .and_then(|s| s.get("id"))
                    .and_then(|i| i.as_str())
                    .unwrap_or_default()
                    .to_string());
            }
            "error" => {
                let message = error_message(&event);
                return Err(match provider_code(&event) {
                    Some(code) => DuplexError::Refused { code, message },
                    None => DuplexError::Protocol(message),
                });
            }
            other => tracing::debug!(event = other, "gpt_live: frame before session.started"),
        }
    }
}

/// Waits out the provider's own close, the `session.close` being the last
/// frame in the send queue.
///
/// The queue goes out in its order, all of it within `external`, and the grace
/// counts from the moment the close is on the wire. A send that fails on the
/// way is an `Err`, as it is in the running session.
///
/// Where an outlet is gone (`Outlets::gone`, or while it runs) the reading goes
/// on and what comes is dropped, until the provider answered the close: a
/// socket let go with unread audio in it is reset, and a reset takes the close
/// that still stands in the send buffer with it.
///
/// The session keeps running while it waits: a model that was mid-sentence
/// keeps arriving, and the last of what it said belongs to the caller as much
/// as the rest. What ends the wait is `session.closed`, a dead socket, or
/// `close_grace_ms` — and in the last case the meter reading is the last one
/// that was reported, because no better one is coming.
async fn finalise(
    read: &mut Reader,
    mut closing: Closing<'_>,
    outlets: Outlets<'_>,
    liveness: &IoLivenessMark,
    meter: &mut SessionMeter,
    law: Option<Law>,
) -> Result<(String, f64), DuplexError> {
    let Outlets {
        audio_out,
        events,
        control,
        audio_in,
        mut gone,
    } = outlets;
    loop {
        let deadline = closing.deadline;
        let incoming = tokio::select! {
            biased;
            sent = closing.out() => {
                sent?;
                continue;
            }
            incoming = tokio::time::timeout_at(deadline, read.next()) => incoming,
        };
        let message = match incoming {
            Err(_) if !closing.sent => return Err(DuplexError::Timeout),
            Err(_) => {
                let reason = gone.unwrap_or("finalization_timeout");
                return Ok((reason.to_string(), meter.usage_seconds));
            }
            Ok(None) | Ok(Some(Err(_))) => {
                return Ok(("connection_lost".to_string(), meter.usage_seconds));
            }
            Ok(Some(Ok(message))) => message,
        };
        let text = match message {
            WsMessage::Text(text) => text.to_string(),
            WsMessage::Close(_) => return Ok(("connection_lost".to_string(), meter.usage_seconds)),
            WsMessage::Ping(_)
            | WsMessage::Pong(_)
            | WsMessage::Binary(_)
            | WsMessage::Frame(_) => {
                continue;
            }
        };
        liveness.mark_success();
        let (said, outlet) = match map_event(&text) {
            Mapped::Finished {
                reason,
                usage_seconds,
            } => return Ok((reason, usage_seconds)),
            Mapped::Emit(event) => {
                meter.note(&event);
                (Said::Event(event), "handler_gone")
            }
            Mapped::Audio(bytes) => {
                meter.saw_output(Instant::now(), &bytes);
                (Said::Audio(expand_out(law, bytes)), "client_gone")
            }
            Mapped::ResponsesCreated(_) | Mapped::ResponsesDone(_) | Mapped::Ignore => continue,
        };
        if gone.is_some() {
            continue;
        }
        if matches!(said, Said::Audio(_)) {
            meter.holds(Instant::now());
        }
        let handed = draining(hand_on(said, audio_out, events), control, audio_in);
        let handed = closing.beside(handed).await;
        meter.released(Instant::now());
        if !handed? {
            gone = Some(outlet);
            control.close();
            audio_in.close();
        }
    }
}

/// What one server event means for the session.
///
/// Four outcomes rather than the three an `stt` adapter has, because a duplex
/// socket carries two things a transcription socket does not: audio, which
/// leaves through a different channel than events, and an orderly end. Keeping
/// both out of the mapping function would mean either handing it two senders —
/// making a pure, table-testable function into an I/O one — or losing the
/// reason and the meter reading the provider ended with.
enum Mapped {
    /// Hand this on to the cell.
    Emit(DuplexEvent),
    /// Decoded output audio; it leaves through `audio_out`.
    Audio(Vec<u8>),
    /// The provider ended the session and said why.
    Finished {
        /// The provider's own word for it.
        reason: String,
        /// The final meter reading.
        usage_seconds: f64,
    },
    /// The provider opened a Responses delegation it runs itself (A45).
    ResponsesCreated(String),
    /// A Responses delegation reached its end: completed, failed or
    /// incomplete (A45).
    ResponsesDone(String),
    /// A frame this adapter does not translate.
    Ignore,
}

/// Maps one server event (`rohbericht-steuerung.md` § D, measured in S0).
///
/// Every unknown event is ignored rather than fatal: this is a hosted service
/// that adds events, and a call that drops because the vendor shipped a new
/// one would be an outage caused by a release note.
fn map_event(text: &str) -> Mapped {
    let Ok(event) = serde_json::from_str::<JsonValue>(text) else {
        // A malformed frame is not a reason to drop a healthy socket.
        tracing::warn!("gpt_live: unparsable frame ignored");
        return Mapped::Ignore;
    };
    let kind = event
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or_default();
    match kind {
        "session.output_audio.delta" => {
            let encoded = event
                .get("delta")
                .and_then(|d| d.as_str())
                .unwrap_or_default();
            match b64::decode(encoded) {
                Some(bytes) => Mapped::Audio(bytes),
                // One chunk of audio, not the call: a moment of silence is
                // survivable, a dropped call is not.
                None => Mapped::Emit(DuplexEvent::Warning {
                    detail: "session.output_audio.delta was not base64".to_string(),
                    provider_code: None,
                    client_event_id: None,
                }),
            }
        }
        "session.input_transcript.delta" => transcript(&event, Speaker::User),
        "session.output_transcript.delta" => transcript(&event, Speaker::Assistant),
        "session.delegation.created" => {
            let delegation = event.get("delegation");
            let target = delegation
                .and_then(|d| d.get("target"))
                .and_then(|t| t.as_str())
                .unwrap_or_default();
            if target != "client" && target != "responses" {
                // Work the model handed to somebody else is not this colony's
                // to answer (R-25-4).
                tracing::debug!(target, "gpt_live: delegation for another target ignored");
                return Mapped::Ignore;
            }
            let Some(delegation_id) = delegation
                .and_then(|d| d.get("id"))
                .and_then(|i| i.as_str())
                .filter(|id| !id.is_empty())
            else {
                tracing::debug!("gpt_live: delegation without an id ignored");
                return Mapped::Ignore;
            };
            // The provider answers a Responses delegation itself; the session
            // only tracks it, never the cell (A45).
            if target == "responses" {
                return Mapped::ResponsesCreated(delegation_id.to_string());
            }
            Mapped::Emit(DuplexEvent::DelegationCreated {
                delegation_id: delegation_id.to_string(),
                offset_ms: ms(&event, "offset_ms"),
            })
        }
        // A45: the end of a Responses delegation's backend stream, correlated
        // by the OUTER `delegation_id`; every other nested event is the
        // provider's own business.
        "response.event" => {
            let inner = event
                .get("event")
                .and_then(|e| e.get("type"))
                .and_then(|t| t.as_str())
                .unwrap_or_default();
            let terminal = matches!(
                inner,
                "response.completed" | "response.failed" | "response.incomplete"
            );
            match event
                .get("delegation_id")
                .and_then(|i| i.as_str())
                .filter(|id| !id.is_empty())
            {
                Some(id) if terminal => Mapped::ResponsesDone(id.to_string()),
                _ => Mapped::Ignore,
            }
        }
        "session.commentary.appended" => appended(&event, AppendKind::Commentary),
        "session.thinking.appended" => appended(&event, AppendKind::Thinking),
        "session.instructions.appended" => appended(&event, AppendKind::Instructions),
        "session.usage.updated" => Mapped::Emit(DuplexEvent::Usage {
            seconds: event
                .get("usage")
                .and_then(|u| u.get("seconds"))
                .and_then(|s| s.as_f64())
                .unwrap_or_default(),
            usage_ratio: usage_ratio(&event),
        }),
        "session.input_audio.muted" => Mapped::Emit(DuplexEvent::Muted),
        "session.input_audio.unmuted" => Mapped::Emit(DuplexEvent::Unmuted),
        "session.closed" => Mapped::Finished {
            reason: event
                .get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("closed")
                .to_string(),
            usage_seconds: event
                .get("usage")
                .and_then(|u| u.get("seconds"))
                .and_then(|s| s.as_f64())
                .unwrap_or_default(),
        },
        // The session is demonstrably running by the time this function is
        // reached — `session.started` came first — so an error is about one
        // item and the session survives it (R-V17). The fatal case lives in
        // `await_started`.
        // Logged as well as reported: a Responses delegation's settings are
        // validated after the start (`responses_validation_failed`), and the
        // session stays open with them -- an operator reading the log has to
        // see why the backend never answers.
        "error" => {
            let detail = error_message(&event);
            let provider_code = provider_code(&event);
            let client_event_id = refused_client_event(&event);
            tracing::warn!(
                provider_code = provider_code.as_deref().unwrap_or(""),
                client_event_id = client_event_id.as_deref().unwrap_or(""),
                %detail,
                "gpt_live: provider error, the session carries on"
            );
            Mapped::Emit(DuplexEvent::Warning {
                detail,
                provider_code,
                client_event_id,
            })
        }
        other => {
            tracing::debug!(event = other, "gpt_live: event not translated");
            Mapped::Ignore
        }
    }
}

/// The provider's code for Responses settings it rejected after the start
/// (A15): the session stays open and the settings are not rolled back.
pub const RESPONSES_VALIDATION_FAILED: &str = "responses_validation_failed";

/// The `session.update` that replaces a session's Responses settings with
/// `fallback` (A15, A36: the one field set a running session may change).
/// The delegation `type` must match the session's, so it is named.
pub fn responses_correction_frame(fallback: &JsonValue) -> JsonValue {
    json!({
        "type": "session.update",
        "session": {
            "delegation": {
                "type": "responses",
                "responses": fallback,
            }
        }
    })
}

/// The Responses delegations of one session that are open and not yet
/// named as overtaken (A45). Each is named at most once: the moment a newer
/// one opens, every one still here is handed back and forgotten.
#[derive(Debug, Default)]
struct OpenResponses {
    /// Open, unnamed delegation ids, oldest first.
    open: Vec<String>,
}

impl OpenResponses {
    /// `id` opened: returns the older open ones it overtakes.
    fn created(&mut self, id: &str) -> Vec<String> {
        let overtaken = std::mem::take(&mut self.open);
        self.open.push(id.to_string());
        overtaken
    }

    /// `id` reached its end; an unknown or already named id is a no-op.
    fn done(&mut self, id: &str) {
        self.open.retain(|open| open != id);
    }
}

/// The note that tells the model an overtaken Responses delegation's result
/// is outdated (A45). General thinking: the reference refuses a non-null
/// `delegation_id` under a Responses delegation. The `event_id` names the
/// overtaken delegation, so its acknowledgement or refusal reads in a log.
fn superseded_note_frame(overtaken: &str, note: &str) -> JsonValue {
    append_frame(
        AppendKind::Thinking,
        &format!("superseded:{overtaken}"),
        None,
        note,
    )
}

/// One transcript fragment on the model's clock. An empty fragment is not a
/// fragment.
fn transcript(event: &JsonValue, speaker: Speaker) -> Mapped {
    let delta = event
        .get("delta")
        .and_then(|d| d.as_str())
        .unwrap_or_default();
    if delta.is_empty() {
        return Mapped::Ignore;
    }
    Mapped::Emit(DuplexEvent::Transcript {
        speaker,
        delta: delta.to_string(),
        start_ms: ms(event, "start_ms"),
        end_ms: ms(event, "end_ms"),
    })
}

/// The longest piece of text held back as a possible control mark, in
/// characters. A mark is a short name between `<|` and `|>`; a run longer
/// than this is not one, and holding it would hold speech.
const MARK_HELD_MAX_CHARS: usize = 64;

/// Takes this provider's control marks out of the model's output transcript
/// before a turn is made of it (d54g callseam).
///
/// Measured on 2026-11-07: a turn carried `... is due.|message|>Today ...`,
/// a piece of a mark of the form `<|name|>` in the middle of what the model
/// had said; the delegation mark has the same form. The transcript arrives in
/// chunks, so a mark can be whole in one chunk or split over two: the piece at
/// the end of a chunk that may be the beginning of a mark is held until the
/// next chunk says what it was. The pattern is the FORM of this provider's
/// marks and judges no meaning: `<|name|>`, the measured `|name|>` without
/// its opening, a `<|name` that never closes, and a bare `|>`. What is still
/// held when the session ends was the beginning of a mark and is dropped
/// with it. The caller's transcript is not touched.
#[derive(Debug, Default)]
struct ControlMarks {
    /// The end of the last chunk, where it may be the beginning of a mark.
    held: String,
}

impl ControlMarks {
    /// A mapped event with the marks taken out of the model's transcript; a
    /// fragment that was nothing but a mark is no fragment.
    fn spoken(&mut self, mapped: Mapped) -> Mapped {
        match mapped {
            Mapped::Emit(DuplexEvent::Transcript {
                speaker: Speaker::Assistant,
                delta,
                start_ms,
                end_ms,
            }) => {
                let delta = self.feed(&delta);
                if delta.is_empty() {
                    return Mapped::Ignore;
                }
                Mapped::Emit(DuplexEvent::Transcript {
                    speaker: Speaker::Assistant,
                    delta,
                    start_ms,
                    end_ms,
                })
            }
            other => other,
        }
    }

    /// The next chunk without its marks. What may be the beginning of a mark
    /// at its end is kept for the chunk after it.
    fn feed(&mut self, delta: &str) -> String {
        let mut text = std::mem::take(&mut self.held);
        text.push_str(delta);
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let is_name = |c: char| c.is_ascii_alphanumeric() || c == '_';
        // Where the run of name characters that starts at `from` ends.
        let name_end = |from: usize| {
            let mut j = from;
            while j < n && is_name(chars[j]) {
                j += 1;
            }
            j
        };
        // Whether the text from `i` to the end is short enough to be the
        // beginning of a mark.
        let may_hold = |i: usize| n - i <= MARK_HELD_MAX_CHARS;
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        while i < n {
            let c = chars[i];
            if c == '<' {
                if i + 1 == n {
                    break;
                }
                if chars[i + 1] != '|' {
                    out.push(c);
                    i += 1;
                    continue;
                }
                let j = name_end(i + 2);
                if (j == n || (chars[j] == '|' && j + 1 == n)) && may_hold(i) {
                    break;
                }
                if j + 1 < n && chars[j] == '|' && chars[j + 1] == '>' {
                    // A whole mark.
                    i = j + 2;
                } else {
                    // `<|name` that does not close: the piece of a mark.
                    i = j;
                }
                continue;
            }
            if c == '|' {
                if i + 1 == n {
                    break;
                }
                if chars[i + 1] == '>' {
                    // A closing with nothing before it.
                    i += 2;
                    continue;
                }
                let j = name_end(i + 1);
                if j > i + 1 {
                    if (j == n || (chars[j] == '|' && j + 1 == n)) && may_hold(i) {
                        break;
                    }
                    if j + 1 < n && chars[j] == '|' && chars[j + 1] == '>' {
                        // The measured piece: a mark without its opening.
                        i = j + 2;
                        continue;
                    }
                }
            }
            out.push(c);
            i += 1;
        }
        self.held = chars[i..].iter().collect();
        out
    }
}

/// The acknowledgement of one append. The identity that matters is the one
/// this side chose (`client_event_id`); the service's own `event_id` names the
/// acknowledgement, not the append.
fn appended(event: &JsonValue, kind: AppendKind) -> Mapped {
    Mapped::Emit(DuplexEvent::Appended {
        kind,
        event_id: event
            .get("client_event_id")
            .and_then(|i| i.as_str())
            .unwrap_or_default()
            .to_string(),
        start_ms: ms(event, "start_ms"),
        end_ms: ms(event, "end_ms"),
    })
}

/// How full the context window is, where the event says so.
///
/// It sits on the event's ROOT, beside `usage` rather than inside it —
/// measured in S0 against the real endpoint, where the documentation reads as
/// if it were nested. The nested spelling is still accepted: it costs one line
/// and it is what the documentation describes.
fn usage_ratio(event: &JsonValue) -> Option<f64> {
    let at = |parent: &JsonValue| {
        parent
            .get("context_window")
            .and_then(|c| c.get("usage_ratio"))
            .and_then(|r| r.as_f64())
    };
    at(event).or_else(|| event.get("usage").and_then(at))
}

/// A millisecond field of an event, or zero. The model's clock is the contract
/// (R-25-9); a missing stamp is a zero on that clock, never the wall time.
fn ms(event: &JsonValue, key: &str) -> u64 {
    event.get(key).and_then(|v| v.as_u64()).unwrap_or_default()
}

/// The human-readable half of an error event, without anything the service
/// echoed back — which is where a credential would otherwise reappear.
fn error_message(event: &JsonValue) -> String {
    event
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("unspecified error")
        .to_string()
}

/// The provider's own code of an error event (`error.code`), only in
/// identifier form: 1 to 64 characters of ASCII letters, digits or `_.-` —
/// the form the argus door accepts as an origin. Anything else (a URL with
/// userinfo, prose, a number) is foreign text on every consumer's error lane
/// and is dropped, so the error message stays what it always was
/// (OR-d53-3.X5.16, GH #1376).
fn provider_code(event: &JsonValue) -> Option<String> {
    let code = event.get("error")?.get("code")?.as_str()?;
    let identifier = (1..=64).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    identifier.then(|| code.to_string())
}

/// The id of the client event an error is about, where the provider names
/// it: `client_event_id`, the spelling the acknowledgement of an append
/// carries (`appended`), on the event's root or inside `error`. It is only
/// ever compared with ids this side chose, so anything that is not a short
/// string is no id.
fn refused_client_event(event: &JsonValue) -> Option<String> {
    let at = |parent: &JsonValue| {
        parent
            .get("client_event_id")
            .and_then(|i| i.as_str())
            .filter(|id| (1..=128).contains(&id.len()))
            .map(str::to_string)
    };
    at(event).or_else(|| event.get("error").and_then(at))
}

/// A refused handshake: 401/403 is a credential verdict, everything else is a
/// transport one. The message carries the status, never the request.
fn classify_connect(error: WsError) -> DuplexError {
    match error {
        WsError::Http(response) => {
            let status = response.status().as_u16();
            if status == 401 || status == 403 {
                DuplexError::Auth(format!("http {status}"))
            } else {
                DuplexError::Connect(format!("http {status}"))
            }
        }
        other => DuplexError::Connect(other.to_string()),
    }
}

/// The one opening event of the session: the model, the persona, one audio
/// format for both directions, and the delegation target.
///
/// `delegation.type: "client"` is the whole point of the arrangement (R-25-4):
/// the model hands work back to this colony, where talky answers it. Without
/// it, OpenAI would run a second agent of its own.
///
/// The format's wire names are the guide's ("WebSocket audio formats":
/// `audio/pcm` at 24000 or 16000, `audio/pcmu` and `audio/pcma` at 8000). A
/// `seed` adds `session.input` in the shape the live-conversations guide
/// gives: one `message` per entry with one text part, `input_text` for
/// developer and user, `output_text` for assistant. Without a seed the
/// block's `start_input` opens the session in the same shape. With `greet` a
/// non-empty `greeting` follows either of them, or stands alone, as ONE
/// `developer` message and always the last one (GH #1628): the guide puts the
/// greeting instructions there, so they are known from the start, and the
/// seed's cut runs before this frame is built and cannot reach it. The turns
/// of a seed in front of a greeting are folded into one `developer` message
/// (GH #1641); the block's own `start_input` is not. The block's
/// `delegation` replaces the client default and its `safety_identifier` is
/// added; without the three keys, a seed and a greeting the frame is byte for
/// byte the one it always was.
pub fn session_start_frame(
    params: &GptLiveParams,
    format: AudioFormat,
    seed: Option<&[SeedMessage]>,
    greet: bool,
) -> JsonValue {
    let wire = match params.audio_format {
        AudioWire::Pcm16 => json!({ "type": "audio/pcm", "rate": format.sample_rate }),
        AudioWire::G711Ulaw => json!({ "type": "audio/pcmu", "rate": G711_SAMPLE_RATE }),
        AudioWire::G711Alaw => json!({ "type": "audio/pcma", "rate": G711_SAMPLE_RATE }),
    };
    let mut frame = json!({
        "type": "session.start",
        "event_id": "start",
        "session": {
            "model": params.model,
            "instructions": params.instructions,
            "audio": {
                "format": wire,
                "output": { "voice": params.voice },
            },
            "delegation": params
                .delegation
                .clone()
                .unwrap_or_else(|| json!({ "type": "client" })),
        }
    });
    let Some(session) = frame.get_mut("session").and_then(|s| s.as_object_mut()) else {
        return frame;
    };
    if let Some(id) = params
        .safety_identifier
        .as_deref()
        .filter(|i| !i.is_empty())
    {
        session.insert("safety_identifier".to_string(), json!(id));
    }
    // A seed wins over the configured start; the configured start drops empty
    // texts the way the seed's parse does.
    let mut opening: Vec<SeedMessage> = match seed.filter(|m| !m.is_empty()) {
        Some(messages) if greets(params, greet) => fold_turns(messages),
        Some(messages) => messages.to_vec(),
        None => params
            .start_input
            .iter()
            .filter(|m| !m.text.trim().is_empty())
            .map(|m| SeedMessage {
                role: m.role.clone(),
                text: m.text.clone(),
            })
            .collect(),
    };
    if greets(params, greet) {
        opening.push(SeedMessage {
            role: "developer".to_string(),
            text: params.greeting.clone(),
        });
    }
    if !opening.is_empty() {
        let input: Vec<JsonValue> = opening
            .iter()
            .map(|m| {
                let part = if m.role == "assistant" {
                    "output_text"
                } else {
                    "input_text"
                };
                json!({
                    "type": "message",
                    "role": m.role,
                    "content": [{ "type": part, "text": m.text }],
                })
            })
            .collect();
        session.insert("input".to_string(), JsonValue::Array(input));
    }
    frame
}

/// The line in front of the folded turns of an earlier call (GH #1641).
const EARLIER_LINE: &str = "Earlier conversation, ended:";

/// Whether a message of a seed is a turn of the conversation.
fn is_turn(m: &SeedMessage) -> bool {
    matches!(m.role.as_str(), "user" | "assistant")
}

/// The first and the last turn of a seed, `None` where it has none.
fn turn_stretch(seed: &[SeedMessage]) -> Option<(usize, usize)> {
    Some((
        seed.iter().position(is_turn)?,
        seed.iter().rposition(is_turn)?,
    ))
}

/// One message as a line of the folded turns, its line break in front. White
/// space becomes one space, so no line of its text reads as a turn of its own.
fn turn_line(m: &SeedMessage) -> String {
    let mut line = format!("\n{}:", m.role);
    for word in m.text.split_whitespace() {
        line.push(' ');
        line.push_str(word);
    }
    line
}

/// The seed of a session that greets: the stretch from its first to its last
/// turn becomes ONE `developer` message at the place of the first, one line per
/// message; what stands in front of it and behind it stays as it came.
///
/// GH #1641: as `user` and `assistant` messages the turns of an earlier call
/// read as the running dialogue of this one: 23 of 23 measured starts on a seed
/// that ends on an assistant turn were silent, 7 of 7 without a turn greeted.
fn fold_turns(seed: &[SeedMessage]) -> Vec<SeedMessage> {
    let Some((first, last)) = turn_stretch(seed) else {
        return seed.to_vec();
    };
    let mut text = EARLIER_LINE.to_string();
    text.extend(seed[first..=last].iter().map(turn_line));
    let mut folded = seed[..first].to_vec();
    folded.push(SeedMessage {
        role: "developer".to_string(),
        text,
    });
    folded.extend_from_slice(&seed[last + 1..]);
    folded
}

/// Whether the start frame of a session carries the block's greeting: it was
/// asked for (`greet`, never on a renewal, GH #896) and there is one.
fn greets(params: &GptLiveParams, greet: bool) -> bool {
    greet && !params.greeting.is_empty()
}

/// One frame of the caller's audio for the wire: PCM16 as it came, or one
/// G.711 byte per sample on a G.711 session.
fn audio_append_frame(law: Option<Law>, bytes: &[u8]) -> JsonValue {
    let encoded = match law {
        Some(law) => b64::encode(&g711::compress(law, bytes)),
        None => b64::encode(bytes),
    };
    json!({
        "type": "session.input_audio.append",
        "audio": encoded,
    })
}

/// The length of one chunk of the model's audio as it arrives, in ms: one
/// byte per sample on a G.711 session, two on PCM16, mono at `rate`.
fn audio_ms(law: Option<Law>, rate: u32, bytes: &[u8]) -> f64 {
    let samples = if law.is_some() {
        bytes.len()
    } else {
        bytes.len() / 2
    };
    samples as f64 * 1000.0 / f64::from(rate.max(1))
}

/// The model's audio for the client: G.711 expanded to PCM16, PCM16 as it is.
fn expand_out(law: Option<Law>, bytes: Vec<u8>) -> Vec<u8> {
    match law {
        Some(law) => g711::expand(law, &bytes),
        None => bytes,
    }
}

/// The most messages the provider takes as a start input (live-conversations
/// guide: "up to 128 messages and 8,192 combined tokens"): the default and
/// the upper bound of `duplex.seed_max_messages`.
pub const SEED_MAX_MESSAGES: usize = 128;
/// The most tokens the provider takes as a start input: the default of
/// `duplex.seed_max_tokens`.
pub const SEED_MAX_TOKENS: u64 = 8_192;
/// The most tokens of instructions the provider takes ("Limited to 16,384
/// client-supplied tokens"). The cell holds the WHOLE start window under it:
/// instructions, greeting and seed together (wave d54g, strand kprofil).
pub const INSTRUCTIONS_MAX_TOKENS: u64 = 16_384;

/// A seed cut to the model's budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrimmedSeed {
    /// What stays, oldest first.
    pub kept: Vec<SeedMessage>,
    /// How many messages left, oldest first.
    pub trimmed: usize,
    /// The estimate of what stays.
    pub est_tokens: u64,
}

/// The cell's own token estimate of one text: its characters divided by
/// `chars_per_token` (the block's param), rounded up. The sender's own
/// `est_tokens` is not trusted.
///
/// It was characters / 4, fixed, while the window's builder counted bytes / 3
/// and the appends of this same cell `chars_per_token`: three rules for one
/// model (wave d54g, strand kprofil). Now the one rate of the block counts
/// everywhere, and the `seed_ask` lane names it, so both sides agree. The
/// rate is counted in thousandths of a character, as
/// [`GptLiveParams::append_limit_chars`] counts it, so the numbers are the
/// same on every machine.
pub fn est_tokens(text: &str, chars_per_token: f64) -> u64 {
    est_chars(text.chars().count(), chars_per_token)
}

/// [`est_tokens`] of a text of `chars` characters.
fn est_chars(chars: usize, chars_per_token: f64) -> u64 {
    let milli_chars = ((chars_per_token * 1_000.0).round() as u64).max(1);
    (chars as u64).saturating_mul(1_000).div_ceil(milli_chars)
}

/// The most tokens the start window of a session may carry besides its
/// instructions and its greeting: the smaller of what the greeting leaves of
/// `seed_max_tokens` and what the two leave of [`INSTRUCTIONS_MAX_TOKENS`],
/// `0` where they leave nothing.
///
/// GH #1628: the greeting is a message of the start input itself, and
/// `seed_max_tokens` is the bound of that input; counted against the
/// instructions alone, a full seed and the greeting together went past it.
///
/// The instructions were never counted, so a long front-end prompt and a
/// full seed together went past the provider's bound (R-58-28: the whole
/// start window stays under it).
pub fn start_budget(params: &GptLiveParams) -> u64 {
    let rate = params.chars_per_token;
    let greeting = est_tokens(&params.greeting, rate);
    let room = INSTRUCTIONS_MAX_TOKENS
        .saturating_sub(est_tokens(&params.instructions, rate))
        .saturating_sub(greeting);
    params.seed_max_tokens.saturating_sub(greeting).min(room)
}

/// What the `seed_ask` lane says about this cell's model in `hop.limits`
/// (wave d54g, strand kprofil): the cell reports no package, so whoever
/// builds its start window could not know how large it may be. Five neutral
/// fields: the model, the tokens a start window and one later feed may
/// carry, the rate this cell estimates with, and `window_profile` as the
/// param holds it. GH #1532: the window and the image cost of the cell's
/// catalog row beside them (`input_soft`, `tokens_per_image`), each only when
/// the block names it.
pub fn ask_limits(params: &GptLiveParams) -> JsonValue {
    let mut limits = json!({
        "model": params.model,
        "start_budget": start_budget(params),
        "feed_budget": params.advise_max_tokens,
        "chars_per_token": params.chars_per_token,
        "profile": params.window_profile,
    });
    for (key, value) in [
        ("input_soft", params.input_soft),
        ("tokens_per_image", params.tokens_per_image),
    ] {
        if let (Some(n), Some(obj)) = (value.filter(|n| *n > 0), limits.as_object_mut()) {
            obj.insert(key.into(), json!(n));
        }
    }
    limits
}

/// Cut a seed to `max_messages` and `max_tokens`, estimated at
/// `chars_per_token`, oldest first: the latest turns are the ones a call
/// continues from. The caller passes the param `seed_max_messages` and the
/// session's [`start_budget`] -- `seed_max_tokens` less the greeting, less
/// still where the instructions leave less (OR-d54g.kprofil.14). A budget
/// of `0` keeps no message.
pub fn trim_seed(
    messages: &[SeedMessage],
    max_tokens: u64,
    max_messages: usize,
    chars_per_token: f64,
) -> TrimmedSeed {
    let est = |m: &SeedMessage| est_tokens(&m.text, chars_per_token);
    let mut start = messages.len().saturating_sub(max_messages);
    let mut total: u64 = messages[start..].iter().map(est).sum();
    while total > max_tokens && start < messages.len() {
        total -= est(&messages[start]);
        start += 1;
    }
    TrimmedSeed {
        kept: messages[start..].to_vec(),
        trimmed: start,
        est_tokens: total,
    }
}

/// Cut a seed the session folds ([`fold_turns`]) to `max_messages` and to
/// `max_tokens` of what it carries folded: the oldest turns first, and what
/// stands around the turns only when none of them is left, as [`trim_seed`]
/// cuts it.
///
/// GH #1641: the curator fills a seed up to [`start_budget`] at this rate, so
/// the head line and the roles never had room: 127 turns of 64 tokens beside a
/// greeting of 40 went out at 8 398 of the provider's 8 192. Cut oldest first
/// over the whole seed, that room came from the fact line and from notes the
/// curator had already booked as said. A `developer` message between two turns
/// (a delegation) leaves with the turn in front of it; alone, it would stand in
/// front of the turns as context of this call.
fn trim_folded(
    messages: &[SeedMessage],
    max_tokens: u64,
    max_messages: usize,
    chars_per_token: f64,
) -> TrimmedSeed {
    let Some((first, last)) = turn_stretch(messages) else {
        return trim_seed(messages, max_tokens, max_messages, chars_per_token);
    };
    let (head, tail) = (&messages[..first], &messages[last + 1..]);
    let around: u64 = head
        .iter()
        .chain(tail)
        .map(|m| est_tokens(&m.text, chars_per_token))
        .sum();
    let lines: Vec<usize> = messages[first..=last]
        .iter()
        .map(|m| turn_line(m).chars().count())
        .collect();
    let mut chars = EARLIER_LINE.chars().count() + lines.iter().sum::<usize>();
    for (cut, line) in lines.iter().enumerate() {
        let start = first + cut;
        let total = around + est_chars(chars, chars_per_token);
        if is_turn(&messages[start]) && messages.len() - cut <= max_messages && total <= max_tokens
        {
            return TrimmedSeed {
                kept: [head, &messages[start..]].concat(),
                trimmed: cut,
                est_tokens: total,
            };
        }
        chars -= line;
    }
    let mut rest = trim_seed(
        &[head, tail].concat(),
        max_tokens,
        max_messages,
        chars_per_token,
    );
    rest.trimmed += lines.len();
    rest
}

/// What the opening of a seeded session collected while it waited.
#[derive(Debug, Default)]
pub struct SeedWait {
    /// The seed, where one came in time.
    pub seed: Option<Vec<SeedMessage>>,
    /// Every other command and the caller's audio, in ONE list in their order
    /// of arrival, for after the start (review K2: a mute, an unmute and the
    /// audio between them replay in that order, not commands first).
    pub held: Vec<Held>,
    /// The call ended while the session waited -- a close, or a caller who
    /// left: the session is not started at all (review K1).
    pub ended: bool,
}

/// One thing a waiting opening held, as it arrived.
#[derive(Debug, Clone, PartialEq)]
pub enum Held {
    /// A command for the session.
    Control(DuplexControl),
    /// A chunk of the caller's audio.
    Audio(Vec<u8>),
}

/// Wait for the seed until `deadline` (a tokio timer, R-AG-1: one deadline,
/// no poll), holding whatever else arrives. Ends at the seed, at the deadline,
/// at a close, or when the caller leaves.
pub async fn await_seed(
    control: &mut mpsc::Receiver<DuplexControl>,
    audio_in: &mut mpsc::Receiver<Vec<u8>>,
    deadline: Instant,
) -> SeedWait {
    let mut out = SeedWait::default();
    let mut control_open = true;
    loop {
        tokio::select! {
            biased;

            command = control.recv(), if control_open => match command {
                Some(DuplexControl::Seed { messages }) => {
                    out.seed = Some(messages);
                    return out;
                }
                Some(DuplexControl::Close) => {
                    out.ended = true;
                    return out;
                }
                Some(other) => out.held.push(Held::Control(other)),
                None => control_open = false,
            },

            chunk = audio_in.recv() => match chunk {
                Some(bytes) => out.held.push(Held::Audio(bytes)),
                None => {
                    out.ended = true;
                    return out;
                }
            },

            () = tokio::time::sleep_until(deadline) => return out,
        }
    }
}

/// One piece of guidance, on the channel its kind names.
fn append_frame(
    kind: AppendKind,
    event_id: &str,
    delegation_id: Option<&str>,
    content: &str,
) -> JsonValue {
    json!({
        "type": format!("session.{}.append", channel_of(kind)),
        "event_id": event_id,
        // Explicitly null rather than absent: the field is part of the frame,
        // and "answers no delegation" is a statement, not an omission.
        "delegation_id": delegation_id,
        "content": content,
    })
}

/// The wire name of an append channel.
fn channel_of(kind: AppendKind) -> &'static str {
    match kind {
        AppendKind::Commentary => "commentary",
        AppendKind::Thinking => "thinking",
        AppendKind::Instructions => "instructions",
    }
}

/// The orderly end, asked for by this side.
fn close_frame() -> JsonValue {
    json!({ "type": "session.close" })
}

/// One JSON frame as the provider's socket takes it.
fn text_frame(frame: &JsonValue) -> WsMessage {
    WsMessage::Text(frame.to_string().into())
}

/// Sends one frame under the A-timeout (hard rule 12).
async fn send_within<S>(
    write: &mut S,
    frame: WsMessage,
    external: Duration,
) -> Result<(), DuplexError>
where
    S: Sink<WsMessage, Error = WsError> + Unpin,
{
    match tokio::time::timeout(external, write.send(frame)).await {
        Err(_) => Err(DuplexError::Timeout),
        Ok(Err(e)) => Err(DuplexError::Closed(format!("send: {e}"))),
        Ok(Ok(())) => Ok(()),
    }
}

/// The maxima the send queue measured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SendTimes {
    /// The longest time from being handed over to the begin of the send. A
    /// frame whose send never began has no reading: what waited behind a send
    /// that hung to the end of the session is not in here.
    waited: Duration,
    /// The slowest single send, a failed one included. Every frame counts, not
    /// just the audio: it answers whether the socket was ever slow to take
    /// what it was given.
    took: Duration,
    /// When the send that is running began.
    sending: Option<Instant>,
}

/// A frame in the send queue.
struct Queued {
    frame: WsMessage,
    /// When it was handed over.
    since: Instant,
}

/// The way to the provider (GH #1630): a task that alone owns the writing half
/// of the socket, and one bounded queue to it, so that audio, guidance and the
/// keepalive leave in the order they were handed over and the session never
/// waits for a send.
///
/// Dropping it ends the task, and with it the writing half of the socket.
struct Uplink {
    /// The queue; gone once everything was handed over for good.
    queue: Option<mpsc::Sender<Queued>>,
    /// What the full queue refused, in order: one frame from the inlets, which
    /// are not read while anything stands here, and behind it the few frames
    /// the reading itself gives rise to. At the start of the session also what
    /// was taken while the opening waited for its seed.
    held: VecDeque<Queued>,
    /// The one task, and its verdict.
    task: JoinSet<Result<(), DuplexError>>,
    /// What the task measured so far.
    times: watch::Receiver<SendTimes>,
}

impl Uplink {
    /// Hands `write` to a task of its own; `external` bounds each send.
    fn spawn<S>(write: S, external: Duration) -> Self
    where
        S: Sink<WsMessage, Error = WsError> + Unpin + Send + 'static,
    {
        let (queue, frames) = mpsc::channel(SEND_QUEUE);
        let (measured, times) = watch::channel(SendTimes::default());
        let mut task = JoinSet::new();
        task.spawn(write_frames(write, frames, external, measured));
        Self {
            queue: Some(queue),
            held: VecDeque::new(),
            task,
            times,
        }
    }

    /// Hands one frame over. Never waits and never drops: what the queue has
    /// no room for is held, behind what is held already.
    fn offer(&mut self, frame: WsMessage) {
        let item = Queued {
            frame,
            since: Instant::now(),
        };
        let refused = match &self.queue {
            Some(queue) if self.held.is_empty() => match queue.try_send(item) {
                Ok(()) => return,
                // A closed queue is a task that failed, and [`Self::step`]
                // has its verdict.
                Err(TrySendError::Full(item) | TrySendError::Closed(item)) => item,
            },
            _ => item,
        };
        self.held.push_back(refused);
    }

    /// [`Self::offer`] for a JSON frame.
    fn offer_json(&mut self, frame: &JsonValue) {
        self.offer(text_frame(frame));
    }

    /// Whether nothing is held, so the inlets may be read.
    fn has_room(&self) -> bool {
        self.held.is_empty()
    }

    /// Waits for the next thing that happens to the queue: room for a held
    /// frame (`false`), the orderly end of the task (`true`), or the send that
    /// failed. Cancel safe, and pending for ever while nothing of it happens.
    async fn step(&mut self) -> Result<bool, DuplexError> {
        let Self {
            queue, held, task, ..
        } = self;
        let room = async {
            match queue {
                Some(queue) if !held.is_empty() => queue.reserve().await,
                _ => std::future::pending().await,
            }
        };
        let ended = tokio::select! {
            biased;
            ended = task.join_next() => ended,
            room = room => match room {
                Ok(permit) => {
                    if let Some(item) = held.pop_front() {
                        permit.send(item);
                    }
                    return Ok(false);
                }
                // The task let go of the queue: its verdict is on the way.
                Err(_) => task.join_next().await,
            },
        };
        match ended {
            Some(Ok(verdict)) => verdict.map(|()| true),
            // A panic of the task is a panic of the session (AUDIT-PRE14-001).
            Some(Err(e)) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
            Some(Err(_)) | None => Err(DuplexError::Closed("send: the writer ended".to_string())),
        }
    }

    /// Takes no more frames and waits until everything handed over is on the
    /// wire. Cancel safe.
    async fn drained(&mut self) -> Result<(), DuplexError> {
        loop {
            if self.held.is_empty() {
                self.queue = None;
            }
            if self.step().await? {
                return Ok(());
            }
        }
    }
}

/// Writes the queue to the socket, frame by frame, until the queue ends or a
/// send fails.
async fn write_frames<S>(
    mut write: S,
    mut frames: mpsc::Receiver<Queued>,
    external: Duration,
    measured: watch::Sender<SendTimes>,
) -> Result<(), DuplexError>
where
    S: Sink<WsMessage, Error = WsError> + Unpin,
{
    while let Some(Queued { frame, since }) = frames.recv().await {
        let began = Instant::now();
        measured.send_modify(|t| {
            t.waited = t.waited.max(began.duration_since(since));
            t.sending = Some(began);
        });
        let sent = send_within(&mut write, frame, external).await;
        measured.send_modify(|t| {
            t.took = t.took.max(began.elapsed());
            t.sending = None;
        });
        sent?;
    }
    Ok(())
}

/// What one session is worth measuring by (S0, `messungen/README.md` § 1 j).
///
/// Integers, times and one capped list; no audio. It is the only place a gap in
/// the caller's audio is visible at all: nothing downstream of `audio_out` can
/// tell a late chunk from a chunk the model simply had not produced yet.
struct SessionMeter {
    /// Decoded output chunks handed on.
    out_chunks: u64,
    /// The worst gap between two of them, in milliseconds.
    out_gap_max_ms: u64,
    /// How many gaps were longer than [`LONG_GAP_MS`].
    out_gaps_over_250: u64,
    /// The slowest single send on this socket, in milliseconds.
    in_send_max_ms: u64,
    /// When the last output chunk arrived.
    last_out: Option<Instant>,
    /// The last meter reading the provider reported.
    usage_seconds: f64,
    /// The wire encoding and rate, for the length of a chunk.
    law: Option<Law>,
    rate: u32,
    /// The session's zero on the monotonic clock and as Unix ms.
    opened: Instant,
    opened_epoch_ms: u64,
    /// The section that is running: the arrival of its first chunk and the
    /// audio delivered since, in µs.
    section: Option<(Instant, u64)>,
    /// Whether the session asked the provider to close.
    closing: bool,
    /// When the chunk that waits for the connection began to wait.
    held_since: Option<Instant>,
    /// How the audio arrived (#1631).
    arrivals: AudioArrivals,
    /// What the send queue measures, once the session has one.
    sends: Option<watch::Receiver<SendTimes>>,
    /// Takes [`Self::arrivals`] when the session ends.
    report: Option<oneshot::Sender<AudioArrivals>>,
}

/// The session hands over how its audio arrived where its meter ends, so that
/// every way out of the session does: an error half way as much as a close.
impl Drop for SessionMeter {
    fn drop(&mut self) {
        self.read_sends();
        if let Some(report) = self.report.take() {
            let _ = report.send(std::mem::take(&mut self.arrivals));
        }
    }
}

impl SessionMeter {
    /// A meter for a session of `law` at `rate` that opens now, which is
    /// `opened_epoch_ms` in Unix ms: the caller names the epoch, so a test does
    /// too.
    fn new(
        law: Option<Law>,
        rate: u32,
        opened_epoch_ms: u64,
        report: Option<oneshot::Sender<AudioArrivals>>,
    ) -> Self {
        Self {
            out_chunks: 0,
            out_gap_max_ms: 0,
            out_gaps_over_250: 0,
            in_send_max_ms: 0,
            last_out: None,
            usage_seconds: 0.0,
            law,
            rate,
            opened: Instant::now(),
            opened_epoch_ms,
            section: None,
            closing: false,
            held_since: None,
            arrivals: AudioArrivals::default(),
            sends: None,
            report,
        }
    }

    /// Reads how late the chunk `bytes` is that arrived at `at`. Counted in
    /// whole µs, so 20 ms is 20 ms and not a rounding below it. Behind the
    /// `session.close` it is counted and never late: the provider sends a
    /// remainder about a quarter of a second behind its time (250 and 255 ms in
    /// the two recorded greetings), and by then nobody plays it.
    fn saw_arrival(&mut self, at: Instant, bytes: &[u8]) {
        let offset_ms = at.duration_since(self.opened).as_millis() as u64;
        let at_epoch_ms = self.opened_epoch_ms + offset_ms;
        if self.closing {
            self.arrivals.note(at_epoch_ms, 0);
            return;
        }
        let audio_us = (audio_ms(self.law, self.rate, bytes) * 1000.0).round() as u64;
        let late_ms = self.section.map_or(0, |(start, delivered_us)| {
            let since_us = at.duration_since(start).as_micros() as u64;
            since_us.saturating_sub(delivered_us) / 1000
        });
        self.section = Some(match self.section {
            Some((start, delivered_us)) if late_ms < LATE_LISTED_FROM_MS => {
                (start, delivered_us + audio_us)
            }
            _ => (at, audio_us),
        });
        self.arrivals.note(at_epoch_ms, late_ms);
    }

    /// How many chunks were late enough to be listed, the ones beyond the
    /// list's cap included.
    fn late_chunks(&self) -> u64 {
        LATENESS_FROM_MS
            .iter()
            .zip(self.arrivals.lateness)
            .filter(|(from, _)| **from >= LATE_LISTED_FROM_MS)
            .map(|(_, n)| n)
            .sum()
    }

    /// The session asked the provider to close.
    fn closes(&mut self) {
        self.closing = true;
    }

    /// A chunk began to wait for room at the connection at `at`.
    fn holds(&mut self, at: Instant) {
        self.held_since = Some(at);
    }

    /// The chunk that waited was handed on, or given up, at `at`.
    fn released(&mut self, at: Instant) {
        if let Some(since) = self.held_since.take() {
            let held = at.duration_since(since).as_millis() as u64;
            self.arrivals.held_max_ms = self.arrivals.held_max_ms.max(held);
        }
    }

    /// A frame waited `waited` for its turn on the socket.
    fn note_send_wait(&mut self, waited: Duration) {
        let waited = waited.as_millis() as u64;
        self.arrivals.send_wait_max_ms = self.arrivals.send_wait_max_ms.max(waited);
    }

    /// The output chunk `bytes` arrived at `at`.
    fn saw_output(&mut self, at: Instant, bytes: &[u8]) {
        self.saw_arrival(at, bytes);
        if let Some(previous) = self.last_out {
            let gap = at.duration_since(previous).as_millis() as u64;
            self.out_gap_max_ms = self.out_gap_max_ms.max(gap);
            if gap > LONG_GAP_MS {
                self.out_gaps_over_250 += 1;
            }
        }
        self.last_out = Some(at);
        self.out_chunks += 1;
    }

    /// One send took `took`.
    fn saw_send(&mut self, took: Duration) {
        self.in_send_max_ms = self.in_send_max_ms.max(took.as_millis() as u64);
    }

    /// Takes over what the send queue measured so far.
    fn read_sends(&mut self) {
        if let Some(sends) = &self.sends {
            let SendTimes {
                waited,
                took,
                sending,
            } = *sends.borrow();
            self.note_send_wait(waited);
            self.saw_send(took);
            // The send the session ends on, as long as it ran.
            if let Some(began) = sending {
                self.saw_send(began.elapsed());
            }
        }
    }

    /// Keeps the last meter reading, so a session that ends without a final
    /// one still reports what it cost.
    fn note(&mut self, event: &DuplexEvent) {
        if let DuplexEvent::Usage { seconds, .. } = event {
            self.usage_seconds = *seconds;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::contract::{LATE_LISTED_MAX, LateChunk};

    /// d54g callseam: a whole mark, a mark split over two chunks and the
    /// measured piece without its opening all leave the transcript, and
    /// nothing else does.
    #[test]
    fn control_marks_leave_the_transcript_and_speech_stays() {
        let mut marks = ControlMarks::default();
        assert_eq!(marks.feed("Booked.<|message|>"), "Booked.");
        assert_eq!(marks.feed(" On Friday.<|mes"), " On Friday.");
        assert_eq!(marks.feed("sage|> At nine."), " At nine.");
        assert_eq!(marks.feed(" Due.|message|>Today."), " Due.Today.");
        // Split inside the opening, and inside the closing.
        assert_eq!(marks.feed("One <"), "One ");
        assert_eq!(marks.feed("|tool|"), "");
        assert_eq!(marks.feed("> two."), " two.");
        // A `<` that opens no mark is speech, one chunk late at most.
        assert_eq!(marks.feed("3 <"), "3 ");
        assert_eq!(marks.feed(" 4."), "< 4.");
        // A mark that never closes is a piece of one; a bare closing too.
        assert_eq!(marks.feed("a <|tool b |> c"), "a  b  c");
        assert_eq!(marks.feed("plain | text, a|b."), "plain | text, a|b.");
    }

    /// d54g callseam: only the model's transcript is filtered, and a fragment
    /// that was nothing but a mark is no fragment.
    #[test]
    fn only_the_models_transcript_loses_its_marks() {
        let mut marks = ControlMarks::default();
        let fragment = |speaker: Speaker, delta: &str| {
            Mapped::Emit(DuplexEvent::Transcript {
                speaker,
                delta: delta.to_string(),
                start_ms: 0,
                end_ms: 100,
            })
        };
        assert!(matches!(
            marks.spoken(fragment(Speaker::User, "I typed <|x|>.")),
            Mapped::Emit(DuplexEvent::Transcript { delta, .. }) if delta == "I typed <|x|>."
        ));
        assert!(matches!(
            marks.spoken(fragment(Speaker::Assistant, "<|message|>")),
            Mapped::Ignore
        ));
        assert!(matches!(
            marks.spoken(fragment(Speaker::Assistant, "Yes.<|x|>")),
            Mapped::Emit(DuplexEvent::Transcript { delta, .. }) if delta == "Yes."
        ));
    }

    /// The URL is built, not configured: an operator names a host, and the path
    /// is this adapter's own knowledge (R-V10).
    #[test]
    fn the_session_url_is_the_host_plus_this_adapters_path() {
        assert_eq!(
            session_url("wss://api.openai.com"),
            "wss://api.openai.com/v1/live/sessions"
        );
        assert_eq!(
            session_url("https://api.openai.com/"),
            "wss://api.openai.com/v1/live/sessions",
            "a base copied out of a vendor's HTTP documentation still connects"
        );
        assert_eq!(
            session_url("http://127.0.0.1:8080"),
            "ws://127.0.0.1:8080/v1/live/sessions",
            "and a local stand-in is reached without TLS"
        );
    }

    /// GH #1632: a 20 ms frame of audio is a small write, and Nagle holds a
    /// small write back while the one before is unacknowledged -- up to a round
    /// trip on every frame of the caller's voice. The socket is opened without.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn the_provider_socket_disables_nagle() {
        use meclaw_testing::mock_gpt_live::{LiveScript, MockGptLive};
        let mock = MockGptLive::start(LiveScript::default())
            .await
            .expect("bind fake");
        let socket = open_socket(
            session_url(&mock.base_url()),
            meclaw_testing::deadline::HANG,
        )
        .await
        .expect("the handshake succeeded");
        let tokio_tungstenite::MaybeTlsStream::Plain(tcp) = socket.get_ref() else {
            panic!("the fake speaks plain TCP");
        };
        assert!(tcp.nodelay().expect("the option is readable"));
    }

    /// The three channels are three different things, and their wire names are
    /// the only place that distinction survives.
    #[test]
    fn an_append_names_its_channel_and_its_delegation() {
        let frame = append_frame(AppendKind::Thinking, "ev_1", Some("dlg_7"), "she is 84");
        assert_eq!(frame["type"], "session.thinking.append");
        assert_eq!(frame["event_id"], "ev_1");
        assert_eq!(frame["delegation_id"], "dlg_7");
        assert_eq!(frame["content"], "she is 84");

        let free = append_frame(AppendKind::Commentary, "ev_2", None, "one moment");
        assert_eq!(free["type"], "session.commentary.append");
        assert_eq!(
            free["delegation_id"],
            JsonValue::Null,
            "answering nothing is said, not left out"
        );
        assert_eq!(
            append_frame(AppendKind::Instructions, "ev_3", None, "speak slower")["type"],
            "session.instructions.append"
        );
    }

    /// Where S0 found the window reading, and where the documentation puts it.
    #[test]
    fn the_window_reading_is_read_from_either_spelling() {
        let measured = json!({
            "type": "session.usage.updated",
            "usage": { "seconds": 30.0 },
            "context_window": { "usage_ratio": 0.62 }
        });
        let documented = json!({
            "type": "session.usage.updated",
            "usage": { "seconds": 30.0, "context_window": { "usage_ratio": 0.62 } }
        });
        for event in [measured, documented] {
            let Mapped::Emit(DuplexEvent::Usage {
                seconds,
                usage_ratio,
            }) = map_event(&event.to_string())
            else {
                panic!("a meter reading is a meter reading");
            };
            assert_eq!(seconds, 30.0);
            assert_eq!(usage_ratio, Some(0.62));
        }
        let bare = json!({ "type": "session.usage.updated", "usage": { "seconds": 1.0 } });
        let Mapped::Emit(DuplexEvent::Usage { usage_ratio, .. }) = map_event(&bare.to_string())
        else {
            panic!("a meter reading is a meter reading");
        };
        assert_eq!(usage_ratio, None, "the window reading is optional");
    }

    fn seed(role: &str, text: &str) -> SeedMessage {
        SeedMessage {
            role: role.to_string(),
            text: text.to_string(),
        }
    }

    /// D48 Test 2 (the clock half): no seed -- the opening waits EXACTLY
    /// `seed_wait_ms`, on the paused clock, and then starts without one.
    #[tokio::test(start_paused = true)]
    async fn gpt_live_seed_wait_ends_exactly_at_the_deadline() {
        let (_control_tx, mut control) = mpsc::channel::<DuplexControl>(4);
        let (_audio_tx, mut audio) = mpsc::channel::<Vec<u8>>(4);
        let began = Instant::now();
        let waited = await_seed(&mut control, &mut audio, began + Duration::from_millis(400)).await;
        assert_eq!(began.elapsed(), Duration::from_millis(400));
        assert!(waited.seed.is_none());
    }

    /// D48 Test 1 (the clock half): a seed after 100 ms ends the wait at
    /// 100 ms, and what came before it is held in order, not lost.
    #[tokio::test(start_paused = true)]
    async fn gpt_live_seed_ends_the_wait_when_it_comes() {
        let (control_tx, mut control) = mpsc::channel::<DuplexControl>(4);
        let (audio_tx, mut audio) = mpsc::channel::<Vec<u8>>(4);
        let began = Instant::now();
        tokio::spawn(async move {
            audio_tx.send(vec![1, 2]).await.ok();
            tokio::time::sleep(Duration::from_millis(1)).await;
            control_tx.send(DuplexControl::Mute).await.ok();
            tokio::time::sleep(Duration::from_millis(99)).await;
            control_tx
                .send(DuplexControl::Seed {
                    messages: vec![seed("user", "hi")],
                })
                .await
                .ok();
            // Held open past the wait, so the end is the seed and not a
            // closed channel.
            tokio::time::sleep(Duration::from_secs(10)).await;
            drop(audio_tx);
        });
        let waited = await_seed(&mut control, &mut audio, began + Duration::from_millis(400)).await;
        assert_eq!(began.elapsed(), Duration::from_millis(100));
        assert_eq!(waited.seed, Some(vec![seed("user", "hi")]));
        assert_eq!(
            waited.held,
            vec![Held::Audio(vec![1, 2]), Held::Control(DuplexControl::Mute)]
        );
    }

    /// Review K2: what arrives while the opening waits is held in ONE list, in
    /// its order of arrival across both channels -- mute, unmute, audio, mute
    /// replays with the audio inside the open ear, not after the last mute.
    #[tokio::test(start_paused = true)]
    async fn gpt_live_seed_wait_holds_commands_and_audio_in_arrival_order() {
        let (control_tx, mut control) = mpsc::channel::<DuplexControl>(4);
        let (audio_tx, mut audio) = mpsc::channel::<Vec<u8>>(4);
        let began = Instant::now();
        tokio::spawn(async move {
            let step = Duration::from_millis(1);
            control_tx.send(DuplexControl::Mute).await.ok();
            tokio::time::sleep(step).await;
            control_tx.send(DuplexControl::Unmute).await.ok();
            tokio::time::sleep(step).await;
            audio_tx.send(vec![7, 7]).await.ok();
            tokio::time::sleep(step).await;
            control_tx.send(DuplexControl::Mute).await.ok();
            tokio::time::sleep(Duration::from_secs(10)).await;
            drop(audio_tx);
        });
        let waited = await_seed(&mut control, &mut audio, began + Duration::from_millis(400)).await;
        assert_eq!(
            waited.held,
            vec![
                Held::Control(DuplexControl::Mute),
                Held::Control(DuplexControl::Unmute),
                Held::Audio(vec![7, 7]),
                Held::Control(DuplexControl::Mute),
            ]
        );
        assert!(!waited.ended);
    }

    /// Review K1: a close or a caller who leaves ends the wait as an end, so
    /// the session is never started for a call that is over.
    #[tokio::test(start_paused = true)]
    async fn gpt_live_seed_wait_ends_at_a_close_or_a_caller_who_left() {
        let (control_tx, mut control) = mpsc::channel::<DuplexControl>(4);
        let (_audio_tx, mut audio) = mpsc::channel::<Vec<u8>>(4);
        control_tx.send(DuplexControl::Close).await.ok();
        let waited = await_seed(
            &mut control,
            &mut audio,
            Instant::now() + Duration::from_secs(5),
        )
        .await;
        assert!(waited.ended && waited.seed.is_none() && waited.held.is_empty());

        let (_control_tx, mut control) = mpsc::channel::<DuplexControl>(4);
        let (audio_tx, mut audio) = mpsc::channel::<Vec<u8>>(4);
        drop(audio_tx);
        let waited = await_seed(
            &mut control,
            &mut audio,
            Instant::now() + Duration::from_secs(5),
        )
        .await;
        assert!(waited.ended);
    }

    /// Review K7: on a G.711 session an odd byte is no half sample (dropped,
    /// the connection already refused the frame), and PCM16 silence goes out
    /// as the law's silence code, one byte per sample.
    #[test]
    fn gpt_live_g711_odd_and_silent_frames() {
        let wire = |law, bytes: &[u8]| {
            let frame = audio_append_frame(Some(law), bytes);
            b64::decode(frame["audio"].as_str().unwrap_or_default()).unwrap_or_default()
        };
        assert_eq!(wire(Law::Mu, &[0, 0, 0, 0, 9]), vec![0xFF, 0xFF]);
        assert_eq!(wire(Law::A, &[0, 0, 0, 0]), vec![0xD5, 0xD5]);
        assert!(wire(Law::Mu, &[]).is_empty());
        assert!(
            expand_out(Some(Law::Mu), vec![0xFF])
                .iter()
                .all(|b| *b == 0)
        );
    }

    /// D48 Test 8: 9 000 estimated tokens -- the oldest leave until the seed
    /// fits 8 192, and the count says how many.
    #[test]
    fn gpt_live_seed_trims_oldest_over_budget() {
        // 9 messages of 4 000 chars = 1 000 tokens each.
        let text = "x".repeat(4_000);
        let messages: Vec<SeedMessage> = (0..9)
            .map(|i| {
                seed(
                    if i % 2 == 0 { "user" } else { "assistant" },
                    &format!("{i}{text}"),
                )
            })
            .collect();
        let cut = trim_seed(&messages, SEED_MAX_TOKENS, SEED_MAX_MESSAGES, 4.0);
        assert_eq!(cut.trimmed, 1, "one message over budget, the oldest one");
        assert_eq!(cut.kept, messages[1..].to_vec());
        assert!(cut.est_tokens <= SEED_MAX_TOKENS);
        // Over 128 messages the oldest go even under the token budget.
        let many: Vec<SeedMessage> = (0..130).map(|i| seed("user", &i.to_string())).collect();
        let cut = trim_seed(&many, SEED_MAX_TOKENS, SEED_MAX_MESSAGES, 4.0);
        assert_eq!((cut.trimmed, cut.kept.len()), (2, SEED_MAX_MESSAGES));
        assert_eq!(cut.kept[0].text, "2");
        // Characters, not bytes, at the rate the block names.
        assert_eq!(est_tokens("\u{e9}\u{e8}\u{ea}\u{eb}", 4.0), 1);
        assert_eq!(est_tokens("\u{e9}\u{e8}\u{ea}\u{eb}", 2.4), 2);
    }

    /// A gap is counted between two chunks, never before the first one.
    #[test]
    fn the_meter_counts_gaps_between_chunks() {
        let mut meter = SessionMeter::new(None, 24_000, 0, None);
        let start = meter.opened;
        meter.saw_output(start, &[]);
        meter.saw_output(start + Duration::from_millis(20), &[]);
        meter.saw_output(start + Duration::from_millis(320), &[]);
        assert_eq!(meter.out_chunks, 3);
        assert_eq!(meter.out_gap_max_ms, 300);
        assert_eq!(
            meter.out_gaps_over_250, 1,
            "one of the two gaps was long enough to hear"
        );
    }

    /// A meter that opened at 1 000 000 Unix ms, fed chunks of 100 ms PCM16 at
    /// 24 kHz that arrive `at_ms` after its opening.
    fn metered(at_ms: &[u64]) -> SessionMeter {
        let mut meter = SessionMeter::new(None, 24_000, 1_000_000, None);
        for at in at_ms {
            let at = meter.opened + Duration::from_millis(*at);
            meter.saw_output(at, &[0; 4_800]);
        }
        meter
    }

    /// #1631: a late chunk is listed once, with its arrival and how late it
    /// was against the audio delivered so far; what follows it in time is not
    /// late again.
    #[test]
    fn a_late_chunk_is_listed_with_its_lateness() {
        let meter = metered(&[50, 150, 310, 410, 515, 1_010]);
        let late = |at_epoch_ms, late_ms| LateChunk {
            at_epoch_ms,
            late_ms,
        };
        assert_eq!(
            meter.arrivals.late,
            vec![late(1_000_310, 60), late(1_001_010, 400)]
        );
        // 5 at 515 stays in the section the chunk at 310 opened; the one at
        // 1 010 is due at 610.
        assert_eq!(meter.arrivals.lateness, [1, 0, 1, 0, 1]);
        assert_eq!(meter.arrivals.late_max_ms, 400);
        assert_eq!(meter.late_chunks(), 2);
        assert_eq!(meter.arrivals.chunks, 6);
    }

    /// #1631: audio that came ahead of time is a lead, and a pause it covers
    /// is no lateness, however long the gap between two chunks.
    #[test]
    fn a_burst_ahead_of_time_is_not_late() {
        let meter = metered(&[0, 1, 2, 3, 400, 500]);
        assert_eq!(meter.out_gap_max_ms, 397);
        assert_eq!(meter.arrivals.late, vec![]);
        assert_eq!(meter.arrivals.lateness, [0; 5]);
        assert_eq!(meter.arrivals.late_max_ms, 0);
    }

    /// #1631: what the provider still sends behind the `session.close` is
    /// counted and not measured: it fills no hole anybody hears. Measured in the
    /// recorded greetings: 250 and 255 ms late, each the session's maximum.
    #[test]
    fn a_chunk_behind_the_close_is_counted_and_not_late() {
        let mut meter = metered(&[0, 100]);
        meter.closes();
        for at in [200, 550] {
            let at = meter.opened + Duration::from_millis(at);
            meter.saw_output(at, &[0; 4_800]);
        }
        assert_eq!(meter.arrivals.late, vec![]);
        assert_eq!(meter.arrivals.lateness, [0; 5]);
        assert_eq!(meter.arrivals.late_max_ms, 0);
        assert_eq!(meter.arrivals.chunks, 4, "every chunk is counted");
        assert_eq!(meter.out_chunks, 4);
    }

    /// #1631: the list ends at its cap, the classes and the maximum do not.
    #[test]
    fn the_list_is_capped_and_the_histogram_is_not() {
        let over = LATE_LISTED_MAX as u64 + 10;
        // Every chunk 130 ms after the one before: 30 ms late, each time.
        let at: Vec<u64> = (0..=over).map(|n| n * 130).collect();
        let meter = metered(&at);
        assert_eq!(meter.arrivals.late.len(), LATE_LISTED_MAX);
        assert_eq!(meter.arrivals.lateness, [0, over, 0, 0, 0]);
        assert_eq!(meter.late_chunks(), over);
    }

    /// #1631: a G.711 chunk is one byte per sample.
    #[test]
    fn the_length_of_a_g711_chunk_is_its_bytes() {
        let mut meter = SessionMeter::new(Some(Law::Mu), 8_000, 0, None);
        let start = meter.opened;
        meter.saw_output(start, &[0; 800]);
        meter.saw_output(start + Duration::from_millis(140), &[0; 800]);
        assert_eq!(meter.arrivals.late_max_ms, 40);
    }

    /// #1631: the longest hold and the longest send wait are maxima.
    #[test]
    fn the_longest_hold_and_the_longest_send_wait_are_kept() {
        let mut meter = SessionMeter::new(None, 24_000, 0, None);
        let start = meter.opened;
        for held in [70, 30] {
            meter.holds(start);
            meter.released(start + Duration::from_millis(held));
        }
        meter.released(start + Duration::from_millis(900));
        meter.note_send_wait(Duration::from_millis(12));
        meter.saw_send(Duration::from_millis(38));
        meter.note_send_wait(Duration::from_millis(5));
        assert_eq!(meter.arrivals.held_max_ms, 70, "no hold, no reading");
        assert_eq!(meter.arrivals.send_wait_max_ms, 12);
        assert_eq!(meter.in_send_max_ms, 38, "a send is not a wait");
    }

    /// #1630: the frame a full queue refuses is held and the inlets stay
    /// closed until it has its place; text and ping leave in one order, and
    /// none is lost.
    #[tokio::test(start_paused = true)]
    async fn a_full_send_queue_holds_one_frame_and_loses_none() {
        let (wire, mut seen) = mpsc::unbounded_channel();
        let socket = futures_util::sink::unfold(wire, |wire, frame| async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            let _ = wire.send(frame);
            Ok::<_, WsError>(wire)
        });
        let mut uplink = Uplink::spawn(Box::pin(socket), Duration::from_secs(1));
        let frame = |n: usize| match n % 3 {
            0 => WsMessage::Ping(Bytes::from(vec![n as u8])),
            _ => WsMessage::Text(n.to_string().into()),
        };
        // Nothing was awaited yet, so the task has taken nothing.
        for n in 0..SEND_QUEUE {
            uplink.offer(frame(n));
            assert!(uplink.has_room(), "frame {n} has its place in the queue");
        }
        uplink.offer(frame(SEND_QUEUE));
        assert!(!uplink.has_room(), "the queue is full");
        assert_eq!(uplink.held.len(), 1, "one frame is held");
        assert!(matches!(uplink.step().await, Ok(false)));
        assert!(uplink.has_room(), "the held frame has its place");
        uplink.offer(frame(SEND_QUEUE + 1));
        assert!(!uplink.has_room());

        uplink.drained().await.expect("every send succeeds");
        for n in 0..SEND_QUEUE + 2 {
            assert_eq!(seen.try_recv().ok(), Some(frame(n)), "frame {n}");
        }
        assert!(seen.try_recv().is_err(), "no frame left twice");
    }

    /// #1630: a `session.close` the full queue refused goes out while the
    /// session waits on an outlet that stands, and an outlet that stands longer
    /// than the sends may take is no failure of them.
    #[tokio::test(start_paused = true)]
    async fn a_standing_outlet_does_not_hold_the_close() {
        let (wire, mut seen) = mpsc::unbounded_channel();
        let socket = futures_util::sink::unfold(wire, |wire, frame| async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            let _ = wire.send(frame);
            Ok::<_, WsError>(wire)
        });
        let external = Duration::from_secs(1);
        let mut uplink = Uplink::spawn(Box::pin(socket), external);
        for n in 0..SEND_QUEUE {
            uplink.offer(WsMessage::Text(n.to_string().into()));
        }
        uplink.offer_json(&close_frame());
        assert!(!uplink.has_room(), "the close is the frame that is held");
        let mut closing = Closing {
            uplink: &mut uplink,
            deadline: Instant::now() + external,
            grace: Duration::from_secs(15),
            sent: false,
        };
        // The outlet stands until the close is on the wire, and then on.
        let outlet = async {
            while seen.recv().await != Some(text_frame(&close_frame())) {}
            tokio::time::sleep(external * 5).await;
            "handed on"
        };
        let handed = closing.beside(outlet).await;
        assert!(matches!(handed, Ok("handed on")), "{handed:?}");
        assert!(closing.sent);
    }

    /// #1630: the wait is the time from the hand-over to the begin of the
    /// send, the send is measured on its own, and the meter reads both.
    #[tokio::test(start_paused = true)]
    async fn the_send_queue_measures_its_waits_and_its_sends() {
        let socket = futures_util::sink::unfold((), |(), _frame| async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            Ok::<_, WsError>(())
        });
        let mut uplink = Uplink::spawn(Box::pin(socket), Duration::from_secs(1));
        let (report, handed_over) = oneshot::channel();
        let mut meter = SessionMeter::new(None, 24_000, 0, Some(report));
        meter.sends = Some(uplink.times.clone());
        for _ in 0..3 {
            uplink.offer_json(&close_frame());
        }
        uplink.drained().await.expect("every send succeeds");
        assert_eq!(
            *uplink.times.borrow(),
            SendTimes {
                waited: Duration::from_millis(80),
                took: Duration::from_millis(40),
                sending: None,
            }
        );
        meter.read_sends();
        assert_eq!(meter.in_send_max_ms, 40);
        drop(meter);
        let arrivals = handed_over.await.expect("the meter reports");
        assert_eq!(arrivals.send_wait_max_ms, 80);
    }

    /// #1630: every send keeps its own A-timeout (hard rule 12), and the one
    /// that hangs is the verdict of the queue.
    #[tokio::test(start_paused = true)]
    async fn a_send_that_hangs_fails_at_its_own_deadline() {
        let socket = futures_util::sink::unfold((), |(), _frame| {
            std::future::pending::<Result<(), WsError>>()
        });
        let mut uplink = Uplink::spawn(Box::pin(socket), Duration::from_secs(1));
        let began = Instant::now();
        uplink.offer_json(&close_frame());
        assert!(matches!(uplink.step().await, Err(DuplexError::Timeout)));
        assert_eq!(began.elapsed(), Duration::from_secs(1));
        assert_eq!(uplink.times.borrow().took, Duration::from_secs(1));
    }

    /// #1630: a send that still hangs when the session ends is a reading too,
    /// as long as it ran.
    #[tokio::test(start_paused = true)]
    async fn a_send_the_session_ends_on_is_measured() {
        let socket = futures_util::sink::unfold((), |(), _frame| {
            std::future::pending::<Result<(), WsError>>()
        });
        let mut uplink = Uplink::spawn(Box::pin(socket), Duration::from_secs(1));
        let mut meter = SessionMeter::new(None, 24_000, 0, None);
        meter.sends = Some(uplink.times.clone());
        uplink.offer_json(&close_frame());
        let ended = tokio::time::timeout(Duration::from_millis(300), uplink.step()).await;
        assert!(ended.is_err(), "the send still hangs");
        drop(uplink);
        meter.read_sends();
        assert_eq!(meter.in_send_max_ms, 300);
    }

    /// #1630: a panic of the writing task is a panic of the session
    /// (AUDIT-PRE14-001), never an ordinary end.
    #[tokio::test(start_paused = true)]
    #[should_panic(expected = "the writer broke")]
    async fn a_panic_of_the_writer_reaches_the_session() {
        let socket = futures_util::sink::unfold((), |(), _frame| async {
            panic!("the writer broke");
            #[allow(unreachable_code)]
            Ok::<_, WsError>(())
        });
        let mut uplink = Uplink::spawn(Box::pin(socket), Duration::from_secs(1));
        uplink.offer_json(&close_frame());
        let _ = uplink.step().await;
    }

    fn live_params(extra: JsonValue) -> GptLiveParams {
        let mut raw = json!({"api_key": "k", "instructions": "Be brief."});
        if let (Some(obj), Some(more)) = (raw.as_object_mut(), extra.as_object()) {
            for (k, v) in more {
                obj.insert(k.clone(), v.clone());
            }
        }
        serde_json::from_value(raw).expect("params parse")
    }

    fn pcm24() -> AudioFormat {
        AudioFormat::pcm16_mono(24_000)
    }

    /// POC gpt-live-1: without the three new keys the opening is the one it
    /// always was -- client delegation, no `safety_identifier`, no `input`.
    #[test]
    fn gpt_live_start_frame_without_the_new_keys_is_unchanged() {
        let frame = session_start_frame(&live_params(json!({})), pcm24(), None, true);
        let session = &frame["session"];
        assert_eq!(session["delegation"], json!({"type": "client"}));
        assert!(session.get("safety_identifier").is_none());
        assert!(session.get("input").is_none());
    }

    /// POC gpt-live-1 (A08/A11): a named delegation goes out as it was written.
    #[test]
    fn gpt_live_start_frame_carries_the_named_delegation() {
        let delegation = json!({
            "type": "responses",
            "responses": {"model": "gpt-6-luna", "instructions": "Think."}
        });
        let frame = session_start_frame(
            &live_params(json!({"delegation": delegation.clone(), "safety_identifier": "h-1"})),
            pcm24(),
            None,
            true,
        );
        assert_eq!(frame["session"]["delegation"], delegation);
        assert_eq!(frame["session"]["safety_identifier"], "h-1");
    }

    /// POC gpt-live-1 (A39/A59): `start_input` opens the session in the seed's
    /// shape, empty texts stay out, and a seed wins over it. GH #1628: a
    /// greeting is the last message behind either, and no renewal carries it.
    #[test]
    fn gpt_live_start_input_opens_the_session_unless_a_seed_came() {
        let params = live_params(json!({"start_input": [
            {"role": "developer", "text": "Today is Tuesday."},
            {"role": "user", "text": "  "},
            {"role": "assistant", "text": "Hello."}
        ]}));
        let frame = session_start_frame(&params, pcm24(), None, true);
        assert_eq!(
            frame["session"]["input"],
            json!([
                {"type": "message", "role": "developer",
                 "content": [{"type": "input_text", "text": "Today is Tuesday."}]},
                {"type": "message", "role": "assistant",
                 "content": [{"type": "output_text", "text": "Hello."}]}
            ])
        );
        let hi = json!({"type": "message", "role": "user",
                        "content": [{"type": "input_text", "text": "hi"}]});
        let seeded = session_start_frame(&params, pcm24(), Some(&[seed("user", "hi")]), true);
        assert_eq!(seeded["session"]["input"], json!([hi.clone()]));

        let greeting = json!({"type": "message", "role": "developer",
                              "content": [{"type": "input_text", "text": "Greet the caller."}]});
        let mut params = params;
        params.greeting = "Greet the caller.".to_string();
        let input = |seed: Option<&[SeedMessage]>, greet: bool| {
            session_start_frame(&params, pcm24(), seed, greet)["session"]["input"].clone()
        };
        let started = input(None, true);
        assert_eq!(started.as_array().map(Vec::len), Some(3), "{started}");
        assert_eq!(started[2], greeting, "behind the configured start");
        assert_eq!(
            input(Some(&[seed("user", "hi")]), true),
            json!([
                {"type": "message", "role": "developer",
                 "content": [{"type": "input_text",
                              "text": "Earlier conversation, ended:\nuser: hi"}]},
                greeting
            ]),
            "behind the seed, whose turns are folded (GH #1641)"
        );
        assert_eq!(
            input(Some(&[seed("user", "hi")]), false),
            json!([hi]),
            "a renewal greets nobody"
        );
    }

    /// POC gpt-live-1: a delegation the provider runs itself (Responses
    /// delegation) is not this colony's to answer.
    #[test]
    fn gpt_live_a_responses_delegation_is_ignored() {
        let frame = json!({
            "type": "session.delegation.created",
            "delegation": {"id": "dlg_9", "type": "delegation", "target": "responses"}
        });
        assert!(matches!(
            map_event(&frame.to_string()),
            Mapped::ResponsesCreated(id) if id == "dlg_9"
        ));
        // Another target, or one without an id, stays out entirely.
        let decisions = json!({
            "type": "session.delegation.created",
            "delegation": {"id": "dlg_8", "type": "delegation", "target": "decisions"}
        });
        assert!(matches!(map_event(&decisions.to_string()), Mapped::Ignore));
        let nameless = json!({
            "type": "session.delegation.created",
            "delegation": {"type": "delegation", "target": "responses"}
        });
        assert!(matches!(map_event(&nameless.to_string()), Mapped::Ignore));
    }

    /// POC gpt-live-1 (A45): a terminal Responses event closes the delegation
    /// its outer `delegation_id` names; anything else on the stream does not.
    #[test]
    fn gpt_live_a_terminal_responses_event_closes_its_delegation() {
        for inner in [
            "response.completed",
            "response.failed",
            "response.incomplete",
        ] {
            let frame = json!({
                "type": "response.event",
                "event_id": "ev_1",
                "delegation_id": "dlg_9",
                "event": {"type": inner, "response": {"id": "resp_1"}}
            });
            assert!(
                matches!(map_event(&frame.to_string()), Mapped::ResponsesDone(id) if id == "dlg_9"),
                "{inner} ends the delegation"
            );
        }
        let delta = json!({
            "type": "response.event",
            "delegation_id": "dlg_9",
            "event": {"type": "response.output_text.delta", "delta": "Ham"}
        });
        assert!(matches!(map_event(&delta.to_string()), Mapped::Ignore));
        let uncorrelated = json!({
            "type": "response.event",
            "delegation_id": null,
            "event": {"type": "response.completed"}
        });
        assert!(matches!(
            map_event(&uncorrelated.to_string()),
            Mapped::Ignore
        ));
    }

    /// POC gpt-live-1 (A45): a new Responses delegation names every older one
    /// still open, each exactly once; a finished one is never named.
    #[test]
    fn gpt_live_a_new_responses_delegation_supersedes_the_open_ones_once() {
        let mut open = OpenResponses::default();
        assert!(open.created("dlg_1").is_empty(), "nothing was open");
        assert_eq!(open.created("dlg_2"), vec!["dlg_1".to_string()]);
        assert!(
            open.created("dlg_3").contains(&"dlg_2".to_string()),
            "the newer one is overtaken in turn"
        );
        // dlg_1 was named already; it is not named twice.
        open.done("dlg_3");
        assert!(open.created("dlg_4").is_empty(), "dlg_3 finished first");
        open.done("dlg_4");
        open.done("dlg_unknown");
        assert!(open.created("dlg_5").is_empty());
    }

    /// POC gpt-live-1 (A45): the note goes out as general thinking -- the
    /// reference refuses a non-null `delegation_id` under a Responses
    /// delegation -- under an `event_id` that names the overtaken delegation.
    #[test]
    fn gpt_live_the_superseded_note_is_general_thinking() {
        let frame = superseded_note_frame("dlg_1", "That request is outdated.");
        assert_eq!(frame["type"], "session.thinking.append");
        assert_eq!(frame["delegation_id"], JsonValue::Null);
        assert_eq!(frame["content"], "That request is outdated.");
        assert_eq!(frame["event_id"], "superseded:dlg_1");
    }

    /// POC gpt-live-1 (A15): a refused Responses setting is a warning with its
    /// code; the session stays open.
    #[test]
    fn gpt_live_responses_validation_failed_is_a_coded_warning() {
        let frame = json!({
            "type": "error",
            "error": {"code": "responses_validation_failed", "message": "bad tool schema"}
        });
        let Mapped::Emit(DuplexEvent::Warning {
            detail,
            provider_code,
            ..
        }) = map_event(&frame.to_string())
        else {
            panic!("an error after the start is a warning, not an end");
        };
        assert_eq!(detail, "bad tool schema");
        assert_eq!(
            provider_code.as_deref(),
            Some("responses_validation_failed")
        );
    }

    /// Wave d54g, strand deleg: the id of the refused client event travels
    /// with the warning -- on the root, where the acknowledgement of an
    /// append carries it, or inside `error`; an event that names none, or
    /// names something that is not an id, carries none.
    #[test]
    fn gpt_live_a_refused_client_event_is_named_on_the_warning() {
        let named = |frame: JsonValue| match map_event(&frame.to_string()) {
            Mapped::Emit(DuplexEvent::Warning {
                client_event_id, ..
            }) => client_event_id,
            _ => panic!("an error after the start is a warning"),
        };
        let error = json!({"code": "invalid_value", "message": "too long"});
        assert_eq!(
            named(json!({"type": "error", "client_event_id": "ev-1", "error": error})).as_deref(),
            Some("ev-1")
        );
        assert_eq!(
            named(json!({"type": "error", "error": {
                "code": "invalid_value", "message": "too long", "client_event_id": "ev-2"
            }}))
            .as_deref(),
            Some("ev-2")
        );
        assert_eq!(named(json!({"type": "error", "error": error})), None);
        for bad in [json!(null), json!(""), json!(7), json!("x".repeat(129))] {
            assert_eq!(
                named(json!({"type": "error", "client_event_id": bad, "error": error})),
                None,
                "{bad}"
            );
        }
    }
}
