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
//! It does not buffer, pace or re-cut audio in either direction: one item out
//! of `audio_in` is one `session.input_audio.append`, one
//! `session.output_audio.delta` is one item into `audio_out` (R-L4). A queue
//! here would be latency on a live call, and the bounded channels of
//! [`DuplexSession`] are the backpressure (ADR-0023 § Consequences).
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
    AppendKind, AudioFormat, BoxFuture, DuplexControl, DuplexError, DuplexEvent, DuplexProvider,
    DuplexSession, ProviderTimeouts, SeedMessage, Speaker,
};
use crate::voice::g711::{self, Law};
use crate::voice::params::{AudioWire, G711_SAMPLE_RATE, GptLiveParams};
use futures_util::{SinkExt, StreamExt};
use meclaw_colony::io_liveness::IoLivenessMark;
use serde_json::{Value as JsonValue, json};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_tungstenite::tungstenite::Bytes;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// The model this adapter speaks to unless a param says otherwise (R-25-0).
pub const DEFAULT_MODEL: &str = "gpt-live-1";

/// The host the session socket lives on. US, because the live endpoints are
/// there and the region question is deferred (R-25-5).
pub const DEFAULT_BASE_URL: &str = "wss://api.openai.com";

/// The voice the model speaks with unless a param says otherwise. They all
/// speak German; none of them is billed as a German voice.
pub const DEFAULT_VOICE: &str = "marin";

/// The `event_id` the opening greeting is sent under, so its acknowledgement
/// is recognisable in a log without reading the text back.
const GREETING_EVENT_ID: &str = "greeting";

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

/// The socket this adapter runs on, once `connect_async` handed it over.
type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
/// The writing half of that socket.
type Writer = futures_util::stream::SplitSink<Socket, WsMessage>;
/// The reading half.
type Reader = futures_util::stream::SplitStream<Socket>;

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
        Box::pin(
            async move { run_session(params, timeouts, format, session, liveness, true).await },
        )
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
        Box::pin(
            async move { run_session(params, timeouts, format, session, liveness, false).await },
        )
    }
}

/// One live session on one socket.
///
/// `format` rather than `params.sample_rate` is what the session is opened at:
/// it is the rate the connection negotiated with its client, and with only one
/// rate on offer (see [`GptLiveDuplex::format`]) the two are the same value —
/// but the one the client was promised is the one the model has to speak.
///
/// `greet` is `false` for a renewed session of a running call (GH #896): the
/// greeting is what the caller hears FIRST, and a renewal is not first.
async fn run_session(
    params: GptLiveParams,
    timeouts: ProviderTimeouts,
    format: AudioFormat,
    session: DuplexSession,
    liveness: IoLivenessMark,
    greet: bool,
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

    // A-timeout around the handshake (hard rule 12): a silent TCP peer must not
    // park this task with no operation-level guard.
    let ws =
        match tokio::time::timeout(timeouts.external, tokio_tungstenite::connect_async(request))
            .await
        {
            Err(_) => return Err(DuplexError::Timeout),
            Ok(Err(e)) => return Err(classify_connect(e)),
            Ok(Ok((ws, _response))) => ws,
        };
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
            let trimmed = trim_seed(messages);
            if trimmed.trimmed > 0 {
                tracing::info!(
                    seed_trimmed = trimmed.trimmed,
                    budget_tokens = SEED_MAX_TOKENS,
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

    let mut meter = SessionMeter::default();
    send_frame(
        &mut write,
        session_start_frame(&params, format, seed.as_deref()),
        timeouts.external,
        &mut meter,
    )
    .await?;

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

    // Exactly once, and only now: a greeting is what the caller should HEAR
    // first, so it travels on `commentary` rather than on `instructions`
    // (OR-L51 — measured 3/3 spoken against 2/9, median 938 ms). Never on a
    // renewed session (GH #896): that one opens mid-call, and the caller was
    // greeted by the session it replaces.
    if greet && !params.greeting.is_empty() {
        send_frame(
            &mut write,
            append_frame(
                AppendKind::Commentary,
                GREETING_EVENT_ID,
                None,
                &params.greeting,
            ),
            timeouts.external,
            &mut meter,
        )
        .await?;
    }

    // What arrived while the opening waited for its seed, in its order of
    // arrival (review K2).
    let mut early_end = None;
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
        if let Err(e) = send_frame(&mut write, frame, timeouts.external, &mut meter).await {
            early_end = Some(Ending::SendFailed(e));
            break;
        }
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

    let ending = loop {
        // A close, a departed caller or a failed send while the opening
        // waited for its seed ends the session before the first select.
        if let Some(ending) = early_end.take() {
            break ending;
        }
        tokio::select! {
            // `biased` so guidance and a close are acted on before the audio
            // already queued behind them — a mute that arrives late is a mute
            // that did not happen — and so the idle deadline is read before the
            // socket that may have gone quiet.
            biased;

            command = control.recv(), if control_open => match command {
                Some(DuplexControl::Append { kind, event_id, delegation_id, content }) => {
                    let frame = append_frame(kind, &event_id, delegation_id.as_deref(), &content);
                    if let Err(e) = send_frame(&mut write, frame, timeouts.external, &mut meter).await {
                        break Ending::SendFailed(e);
                    }
                }
                Some(DuplexControl::Mute) => {
                    let frame = json!({ "type": "session.input_audio.mute" });
                    if let Err(e) = send_frame(&mut write, frame, timeouts.external, &mut meter).await {
                        break Ending::SendFailed(e);
                    }
                }
                Some(DuplexControl::Unmute) => {
                    let frame = json!({ "type": "session.input_audio.unmute" });
                    if let Err(e) = send_frame(&mut write, frame, timeouts.external, &mut meter).await {
                        break Ending::SendFailed(e);
                    }
                }
                Some(DuplexControl::Close) => {
                    match send_frame(&mut write, close_frame(), timeouts.external, &mut meter).await {
                        Ok(()) => break Ending::Finalise,
                        Err(e) => break Ending::SendFailed(e),
                    }
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

            chunk = audio_in.recv() => match chunk {
                Some(bytes) => {
                    // One item, one append. The connection checked the parity;
                    // what arrives here goes on the wire as it is (R-L4) --
                    // companded first on a G.711 session, sample by sample.
                    let frame = audio_append_frame(law, &bytes);
                    if let Err(e) = send_frame(&mut write, frame, timeouts.external, &mut meter).await {
                        break Ending::SendFailed(e);
                    }
                }
                None => {
                    // The client went away: `audio_in` closing IS the end of
                    // the call (contract § 1.1). No guard on this arm and none
                    // needed: the loop leaves here rather than carrying on with
                    // a receiver that would answer `None` for ever.
                    match send_frame(&mut write, close_frame(), timeouts.external, &mut meter).await {
                        Ok(()) => break Ending::Finalise,
                        Err(e) => break Ending::SendFailed(e),
                    }
                }
            },

            // GH #1403: the lab's switch ends the session the way the
            // provider's own `session.closed reason=content` does (`map_event`),
            // so what follows is the connection's recovery and nothing new.
            // Said once on the colony's lane, so a lab tells a forced abort
            // from a real one; the `session.close` is best effort, so the
            // provider stops billing a session nobody uses any more.
            () = &mut fault_close, if fault_close_at_ms > 0 => {
                tracing::warn!(
                    fault_close_at_ms,
                    "gpt_live: fault_close_fired -- the lab's forced provider abort"
                );
                let detail = format!(
                    "fault_close_fired: session ended {fault_close_at_ms} ms after its start \
                     as reason=content (fault_close_at_ms, a lab switch)"
                );
                if events.send(DuplexEvent::Warning { detail }).await.is_err() {
                    break Ending::HandlerGone;
                }
                let _ = send_frame(&mut write, close_frame(), timeouts.external, &mut meter).await;
                break Ending::Reported {
                    reason: "content".to_string(),
                    usage_seconds: meter.usage_seconds,
                };
            }

            () = &mut idle_deadline => {
                return Err(DuplexError::Closed(format!(
                    "idle for {:?}, no frame — socket presumed dead",
                    timeouts.idle
                )));
            }

            // Ask the socket whether it is still there (R-L7, GH #798). The
            // payload is [`KEEPALIVE_PAYLOAD`], which is what makes the answer
            // recognisable as the answer to THIS question. An A-timeout around
            // it like every other send (hard rule 12): a peer that accepts no
            // bytes must not park this task either.
            _ = keepalive.tick() => {
                let began = Instant::now();
                let sent = tokio::time::timeout(
                    timeouts.external,
                    write.send(WsMessage::Ping(Bytes::from_static(KEEPALIVE_PAYLOAD))),
                ).await;
                meter.saw_send(began.elapsed());
                match sent {
                    Err(_) => break Ending::SendFailed(DuplexError::Timeout),
                    Ok(Err(e)) => break Ending::SendFailed(
                        DuplexError::Closed(format!("keepalive: {e}"))
                    ),
                    Ok(Ok(())) => {}
                }
            }

            incoming = read.next() => {
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
                        if events.send(DuplexEvent::Warning { detail }).await.is_err() {
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
                match map_event(&text) {
                    Mapped::Emit(event) => {
                        meter.note(&event);
                        // A full channel blocks: backpressure, never drop.
                        if events.send(event).await.is_err() {
                            break Ending::HandlerGone;
                        }
                    }
                    Mapped::Audio(bytes) => {
                        meter.saw_output(Instant::now());
                        if audio_out.send(expand_out(law, bytes)).await.is_err() {
                            break Ending::ClientGone;
                        }
                    }
                    Mapped::Finished { reason, usage_seconds } => {
                        break Ending::Reported { reason, usage_seconds };
                    }
                    Mapped::Ignore => {}
                }
            }
        }
    };
    let grace = Duration::from_millis(params.close_grace_ms);
    let mut failure = None;
    let (reason, usage_seconds, report) = match ending {
        Ending::Finalise => {
            let (reason, seconds) = finalise(
                &mut read, &audio_out, &events, &liveness, &mut meter, grace, law,
            )
            .await;
            (reason, seconds, true)
        }
        Ending::Reported {
            reason,
            usage_seconds,
        } => (reason, usage_seconds, true),
        Ending::ConnectionLost => ("connection_lost".to_string(), meter.usage_seconds, true),
        // Nobody is left to tell. The session still ends here and the log line
        // below is the only record of it.
        Ending::HandlerGone => ("handler_gone".to_string(), meter.usage_seconds, false),
        Ending::ClientGone => ("client_gone".to_string(), meter.usage_seconds, false),
        Ending::SendFailed(e) => {
            failure = Some(e);
            ("send_failed".to_string(), meter.usage_seconds, false)
        }
    };
    if report {
        let _ = events
            .send(DuplexEvent::Closed {
                reason: reason.clone(),
                usage_seconds,
            })
            .await;
    }
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
        "gpt_live session ended"
    );
    match failure {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Why the streaming loop stopped.
enum Ending {
    /// A `session.close` went out; the provider's own `session.closed` is still
    /// to come.
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
    /// The cell stopped listening to events.
    HandlerGone,
    /// The connection stopped taking output audio.
    ClientGone,
    /// The socket would not take a frame in time. This IS a disturbance and
    /// leaves as `Err` — but through the tail, so the session still writes its
    /// measurement line: a session that died on a slow send is exactly the one
    /// whose `in_send_max_ms` L9's B-6 wants to read.
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
            "error" => return Err(DuplexError::Protocol(error_message(&event))),
            other => tracing::debug!(event = other, "gpt_live: frame before session.started"),
        }
    }
}

/// Waits out the provider's own close after a `session.close` went out.
///
/// The session keeps running while it waits: a model that was mid-sentence
/// keeps arriving, and the last of what it said belongs to the caller as much
/// as the rest. What ends the wait is `session.closed`, a dead socket, or
/// `close_grace_ms` — and in the last case the meter reading is the last one
/// that was reported, because no better one is coming.
async fn finalise(
    read: &mut Reader,
    audio_out: &mpsc::Sender<Vec<u8>>,
    events: &mpsc::Sender<DuplexEvent>,
    liveness: &IoLivenessMark,
    meter: &mut SessionMeter,
    grace: Duration,
    law: Option<Law>,
) -> (String, f64) {
    let deadline = tokio::time::Instant::now() + grace;
    loop {
        let message = match tokio::time::timeout_at(deadline, read.next()).await {
            Err(_) => return ("finalization_timeout".to_string(), meter.usage_seconds),
            Ok(None) | Ok(Some(Err(_))) => {
                return ("connection_lost".to_string(), meter.usage_seconds);
            }
            Ok(Some(Ok(message))) => message,
        };
        let text = match message {
            WsMessage::Text(text) => text.to_string(),
            WsMessage::Close(_) => return ("connection_lost".to_string(), meter.usage_seconds),
            WsMessage::Ping(_)
            | WsMessage::Pong(_)
            | WsMessage::Binary(_)
            | WsMessage::Frame(_) => {
                continue;
            }
        };
        liveness.mark_success();
        match map_event(&text) {
            Mapped::Finished {
                reason,
                usage_seconds,
            } => return (reason, usage_seconds),
            Mapped::Emit(event) => {
                meter.note(&event);
                if events.send(event).await.is_err() {
                    return ("handler_gone".to_string(), meter.usage_seconds);
                }
            }
            Mapped::Audio(bytes) => {
                meter.saw_output(Instant::now());
                if audio_out.send(expand_out(law, bytes)).await.is_err() {
                    return ("client_gone".to_string(), meter.usage_seconds);
                }
            }
            Mapped::Ignore => {}
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
            if target != "client" {
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
            Mapped::Emit(DuplexEvent::DelegationCreated {
                delegation_id: delegation_id.to_string(),
                offset_ms: ms(&event, "offset_ms"),
            })
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
        "error" => Mapped::Emit(DuplexEvent::Warning {
            detail: error_message(&event),
        }),
        other => {
            tracing::debug!(event = other, "gpt_live: event not translated");
            Mapped::Ignore
        }
    }
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
/// developer and user, `output_text` for assistant. Without a seed the frame
/// is byte for byte the one it always was.
pub fn session_start_frame(
    params: &GptLiveParams,
    format: AudioFormat,
    seed: Option<&[SeedMessage]>,
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
            "delegation": { "type": "client" },
        }
    });
    if let Some(messages) = seed.filter(|m| !m.is_empty())
        && let Some(session) = frame.get_mut("session").and_then(|s| s.as_object_mut())
    {
        let input: Vec<JsonValue> = messages
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

/// The model's audio for the client: G.711 expanded to PCM16, PCM16 as it is.
fn expand_out(law: Option<Law>, bytes: Vec<u8>) -> Vec<u8> {
    match law {
        Some(law) => g711::expand(law, &bytes),
        None => bytes,
    }
}

/// The most messages a seed may carry (live-conversations guide: "up to 128
/// messages and 8,192 combined tokens").
pub const SEED_MAX_MESSAGES: usize = 128;
/// The most tokens a seed may carry, by the cell's own estimate.
pub const SEED_MAX_TOKENS: u64 = 8_192;

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

/// The cell's own token estimate of one text: characters / 4, rounded up —
/// the rule the route uses (its Python is not callable from here), and the
/// sender's own `est_tokens` is not trusted.
pub fn est_tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

/// Cut a seed to [`SEED_MAX_MESSAGES`] and [`SEED_MAX_TOKENS`], oldest
/// first: the latest turns are the ones a call continues from.
pub fn trim_seed(messages: &[SeedMessage]) -> TrimmedSeed {
    let mut start = messages.len().saturating_sub(SEED_MAX_MESSAGES);
    let mut total: u64 = messages[start..].iter().map(|m| est_tokens(&m.text)).sum();
    while total > SEED_MAX_TOKENS && start < messages.len() {
        total -= est_tokens(&messages[start].text);
        start += 1;
    }
    TrimmedSeed {
        kept: messages[start..].to_vec(),
        trimmed: start,
        est_tokens: total,
    }
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

/// Sends one frame under the A-timeout (hard rule 12) and records how long it
/// took.
///
/// Every send is measured, not just the audio: `in_send_max_ms` answers the
/// question "was the socket ever slow to take what we gave it", and a slow
/// append is as much an answer as a slow audio frame.
async fn send_frame(
    write: &mut Writer,
    frame: JsonValue,
    external: Duration,
    meter: &mut SessionMeter,
) -> Result<(), DuplexError> {
    let began = Instant::now();
    let sent = tokio::time::timeout(
        external,
        write.send(WsMessage::Text(frame.to_string().into())),
    )
    .await;
    meter.saw_send(began.elapsed());
    match sent {
        Err(_) => Err(DuplexError::Timeout),
        Ok(Err(e)) => Err(DuplexError::Closed(format!("send: {e}"))),
        Ok(Ok(())) => Ok(()),
    }
}

/// What one session is worth measuring by (S0, `messungen/README.md` § 1 j).
///
/// The counters are cheap — three integers and an `Instant` — and they are the
/// only place a gap in the caller's audio is visible at all: nothing
/// downstream of `audio_out` can tell a late chunk from a chunk the model
/// simply had not produced yet.
#[derive(Default)]
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
}

impl SessionMeter {
    /// An output chunk arrived at `at`.
    fn saw_output(&mut self, at: Instant) {
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
        let cut = trim_seed(&messages);
        assert_eq!(cut.trimmed, 1, "one message over budget, the oldest one");
        assert_eq!(cut.kept, messages[1..].to_vec());
        assert!(cut.est_tokens <= SEED_MAX_TOKENS);
        // Over 128 messages the oldest go even under the token budget.
        let many: Vec<SeedMessage> = (0..130).map(|i| seed("user", &i.to_string())).collect();
        let cut = trim_seed(&many);
        assert_eq!((cut.trimmed, cut.kept.len()), (2, SEED_MAX_MESSAGES));
        assert_eq!(cut.kept[0].text, "2");
        // Characters, not bytes: the estimate is the route's chars/4.
        assert_eq!(est_tokens("\u{e9}\u{e8}\u{ea}\u{eb}"), 1);
    }

    /// A gap is counted between two chunks, never before the first one.
    #[test]
    fn the_meter_counts_gaps_between_chunks() {
        let mut meter = SessionMeter::default();
        let start = Instant::now();
        meter.saw_output(start);
        meter.saw_output(start + Duration::from_millis(20));
        meter.saw_output(start + Duration::from_millis(320));
        assert_eq!(meter.out_chunks, 3);
        assert_eq!(meter.out_gap_max_ms, 300);
        assert_eq!(
            meter.out_gaps_over_250, 1,
            "one of the two gaps was long enough to hear"
        );
    }
}
