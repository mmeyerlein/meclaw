//! One client connection, for its whole life (wave voice-cell).
//!
//! # The transport is a parameter (GH #643)
//!
//! Nothing here names a socket any more. A connection is driven through a
//! [`ClientLink`] — text in, binary in, text out, binary out, a close with a
//! code — and the two doors that produce one are this cell's own WebSocket and a
//! `voice:<call>` topic on the socket a display page already holds. Everything
//! below that line is the same code for both, which is what makes `hello`,
//! `bad_audio_frame`, `4409` and `client_too_slow` mean the same thing on either.
//!
//! # What this task is for
//!
//! It is the only place where audio exists. Binary frames from the client go
//! straight into this connection's recognition session; the chunks a synthesis
//! produces go straight back out. Neither ever crosses the channel to the
//! handler — what crosses is what a turn machine can act on, and nothing else.
//!
//! The state here is connection-local by construction: the sockets, the running
//! synthesis, the retry bookkeeping. None of it is cell state, none of it
//! survives the connection, and none of it is shared with another task.
//!
//! # The echo path
//!
//! With `stt.provider: "echo"` there is no recognition session at all: every
//! binary frame is written straight back, in order, and the turn frames are
//! refused with `wrong_mode`. It measures the socket and the client's audio
//! path with no model in the way, which is what makes a slow first turn
//! attributable to the right half.
//!
//! # The duplex path (welle live)
//!
//! With `params.duplex` there is no recogniser, no synthesiser and no turn
//! machine: one socket carries both directions of one live model, and the model
//! draws its own turn boundaries (R-25-9). [`run_connection`] hands that case
//! to [`run_duplex`] as its first statement and the loop below is not entered
//! at all. Audio still terminates here — no sample becomes a message
//! (ADR-0023) — and one client frame becomes exactly one `append`, with no
//! buffer and no pacing in between (R-L4).
//!
//! A live model ends a session at a limit of its own, and a call may outlive
//! it: with `renew_after_ms` set the connection opens the call's NEXT session
//! while the current one still runs and hands the call over once the new one
//! has started (GH #896). That is a renewal and not a reconnect — a session
//! that ends on its own still ends the call.
//!
//! # Cancelling a synthesis (R-V11)
//!
//! Two established stacks were read for the wire behaviour rather than the
//! architecture: pipecat's Cartesia turn-based recognition service
//! (`src/pipecat/services/cartesia/turns/stt.py`) and LiveKit's
//! `InterruptionOptions` (`livekit-agents/livekit/agents/voice/turn.py`). Two
//! things are ported here:
//!
//! 1. **Buffered audio is discarded, not flushed** (LiveKit's
//!    `discard_audio_if_uninterruptible`). A cancel drops the whole synthesis —
//!    the receiver with it — so a chunk that was already produced never reaches
//!    the client. Nothing waits for the provider to notice; the socket must go
//!    quiet at once, and the provider's own ending is reported to nobody.
//! 2. **A cancel is reported exactly once.** The running synthesis is *taken*
//!    out of the connection's state before anything is sent, so a second cancel
//!    finds nothing and no `speak_end` can be emitted twice.
//!
//! Deliberately not ported: LiveKit's interruption *policy* — `min_duration`,
//! `min_words`, `false_interruption_timeout` and resuming after a false
//! interruption. Whether an interruption happened is the handler's decision
//! (spec § 4: `SpeechStarted` plus `barge_in`), thresholds are params with
//! defaults (R-V4), and this cell has no resume: a cancelled turn is over. This
//! task is the hand that stops the sound, not the head that decides to.
//!
//! # Outbound framing (`audio_out_frame_ms`)
//!
//! A synthesis provider chooses its own chunk size and Cartesia's are large.
//! The client at the other end may be a telephony edge, and one of them —
//! FreeSWITCH's `mod_audio_stream` 1.0.3, whose playback half is closed
//! source — aborts the process with `SIGABRT` (`free(): corrupted unsorted
//! chunks`) the moment a binary frame carries more than about 100 ms of audio.
//! Measured on the real socket: 960 B (20 ms), 4410 B and 4800 B (100 ms) all
//! survive; 9600 B (200 ms), 14400 B and 19200 B all kill the call.
//!
//! So the chunk a provider hands over is not the frame a client is sent. Every
//! chunk is cut into frames of at most `audio_out_frame_ms` before it leaves
//! ([`Framer`]), 20 ms by default, and `0` restores the old passthrough. The
//! promise is an upper bound, not a fixed size: a frame is either exactly that
//! long or the remainder of a provider chunk, never longer, and an even
//! remainder leaves at once rather than waiting for the next chunk — latency is
//! the reason for framing, so a provider that ships small chunks is not made to
//! wait for a full frame.
//! Nothing is paced and nothing sleeps: a burst of small frames is what that
//! module and every RTP stack behind it expect, and inserting a delay would
//! turn a working call into a stuttering one — the gap between frames is what a
//! playback buffer cannot survive, not the number of them.

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use crate::voice::call_usage::{CallUsage, UsageSlot};
use crate::voice::cell::{SeedReason, VoiceEvent};
use crate::voice::contract::{
    AppendKind, AudioFormat, DuplexControl, DuplexError, DuplexEvent, DuplexProvider,
    DuplexSession, Speaker, SttError, SttEvent, TtsError,
};
use crate::voice::io::{ToConnection, VoiceIoShared, new_session_way, register, unregister};
use crate::voice::link::{ClientLink, Incoming, LinkSink, LinkStream, Outgoing};
use crate::voice::service::{Negotiated, audio_out_frame_ms, stt_name, tts_name};
use crate::voice::wire::{
    CLOSE_SESSION_NOT_TAKEN, ClientFrame, Mode, PROTOCOL, ServerFrame, SpeakEndReason,
    WireErrorCode,
};

/// How long a lost recognition session is left alone before the one retry.
///
/// One retry, then the connection ends: a provider that refuses twice in a row
/// is not a blip, and a client that keeps talking into a deaf socket is worse
/// off than one that is told to reconnect.
const STT_RETRY_DELAY: Duration = Duration::from_secs(1);

/// The length of one frame of the silence a released `hold` is followed by
/// (GH #717).
///
/// 20 ms is the frame this wire recommends in both directions, and the cadence
/// matters more than the size: an endpointing provider measures silence by the
/// audio clock, so the tail has to arrive at roughly the rate real audio would
/// have arrived at. A longer frame would make the provider's own 0.7 s
/// threshold jump in coarser steps and nothing else.
const TAIL_FRAME: Duration = Duration::from_millis(20);

/// How many audio chunks may be in flight between a provider and this task.
const AUDIO_QUEUE: usize = 32;

/// The sending half of the client link, whatever carries it.
type Sink = LinkSink;

/// The text that waits for the line's first quiet moment (GH #1400): its cut
/// (`split_id`), its parts (channel, event id, words) in order, and since when
/// the first part waits.
type QuietHeld = (String, Vec<(AppendKind, String, String)>, Instant);

/// One running recognition session.
struct SttSession {
    /// Audio on its way to the provider. A full channel blocks the reader,
    /// which is the backpressure the spec asks for.
    audio_tx: mpsc::Sender<Vec<u8>>,
    /// What the provider reports.
    events_rx: mpsc::Receiver<SttEvent>,
    /// The verdict, once the session future is over.
    done_rx: oneshot::Receiver<Result<(), SttError>>,
}

impl SttSession {
    /// Start one session for this connection, at the format it negotiated.
    fn start(shared: &Arc<VoiceIoShared>, format: AudioFormat) -> Self {
        let (audio_tx, audio_rx) = mpsc::channel::<Vec<u8>>(AUDIO_QUEUE);
        let (events_tx, events_rx) = mpsc::channel::<SttEvent>(AUDIO_QUEUE);
        let (done_tx, done_rx) = oneshot::channel();
        let provider = shared.stt.clone();
        let liveness = shared.liveness.clone();
        tokio::spawn(async move {
            let outcome = provider
                .run_session(format, audio_rx, events_tx, liveness)
                .await;
            let _ = done_tx.send(outcome);
        });
        Self {
            audio_tx,
            events_rx,
            done_rx,
        }
    }

    /// Why this session ended: the event the handler is told, and the text a
    /// client is shown if this was the second failure.
    async fn outcome(self, limit: Duration) -> (SttEvent, String) {
        let verdict = tokio::time::timeout(limit, self.done_rx).await;
        match verdict {
            Ok(Ok(Ok(()))) => (
                SttEvent::Closed,
                "the provider ended the session".to_string(),
            ),
            Ok(Ok(Err(e))) => {
                let detail = e.to_string();
                (
                    SttEvent::Failed {
                        detail: detail.clone(),
                    },
                    detail,
                )
            }
            Ok(Err(_)) | Err(_) => {
                let detail = "the recognition task ended without a verdict".to_string();
                (
                    SttEvent::Failed {
                        detail: detail.clone(),
                    },
                    detail,
                )
            }
        }
    }
}

/// Whether the first audio of one synthesis has left for the client (d54g
/// callseam): what makes the `first_audio` stamp one per synthesis, however
/// many chunks the provider cuts it into.
#[derive(Debug, Default)]
struct FirstAudio(bool);

impl FirstAudio {
    /// Whether `frames` are this synthesis' first audio. A chunk the framer
    /// held back whole is no audio yet: the stamp waits for the first frame
    /// that actually travels.
    fn leaves_with(&mut self, frames: &[Vec<u8>]) -> bool {
        if self.0 || frames.iter().all(Vec::is_empty) {
            return false;
        }
        self.0 = true;
        true
    }
}

/// Milliseconds since the connection opened: the clock of a cascade
/// connection, which has no session clock (d54g callseam).
fn since_ms(opened: Instant) -> u64 {
    u64::try_from(opened.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// One running synthesis.
struct Speaking {
    /// The id its `speak_start`/`speak_end` pair carries.
    speak_id: String,
    /// Set to `true` to stop the provider.
    cancel: watch::Sender<bool>,
    /// The audio it produces.
    audio_rx: mpsc::Receiver<Vec<u8>>,
    /// Cuts that audio into frames the client can swallow.
    framer: Framer,
    /// The verdict, once the synthesis future is over.
    done_rx: oneshot::Receiver<Result<(), TtsError>>,
    /// Whether its first audio has left (the `first_audio` stamp).
    first_audio: FirstAudio,
}

impl Speaking {
    /// Tell the provider to stop. It answers by ending its future; nothing here
    /// waits for that, because the client must not hear another sample.
    ///
    /// The caller takes this value out of the connection's state, which drops
    /// [`Self::audio_rx`] with it — every chunk the provider already produced
    /// and every one still in flight is discarded rather than flushed (R-V11).
    fn stop(&self) {
        let _ = self.cancel.send(true);
    }

    /// Why the synthesis ended.
    async fn outcome(self, limit: Duration) -> (SpeakEndReason, Option<String>) {
        match tokio::time::timeout(limit, self.done_rx).await {
            Ok(Ok(Ok(()))) => (SpeakEndReason::Done, None),
            Ok(Ok(Err(e))) => (SpeakEndReason::Failed, Some(e.to_string())),
            Ok(Err(_)) | Err(_) => (
                SpeakEndReason::Failed,
                Some("the synthesis task ended without a verdict".to_string()),
            ),
        }
    }
}

/// Cuts one synthesis' chunks into frames of a fixed length.
///
/// Two rules, and both are about losing nothing:
///
/// 1. A frame is either exactly [`Self::frame_len`] long or it is the tail of
///    a chunk, which may be shorter. [`Self::frame_len`] is an upper bound, not
///    a size every frame has: an even tail goes out with its own chunk rather
///    than waiting for the next one, because latency is what the framing is
///    for.
/// 2. A tail is **never** cut through a sample. PCM16 mono means two bytes per
///    sample, so an odd tail is held back and prepended to the next chunk
///    rather than sent as half a sample or dropped; what is still held when the
///    synthesis ends leaves as one short last frame ([`Self::flush`]).
///
/// A cancel discards the held bytes with the rest of the synthesis (R-V11):
/// the value lives inside [`Speaking`], so taking that out of the connection's
/// state takes the buffer with it.
struct Framer {
    /// Bytes per outbound frame; `0` is the passthrough — one chunk, one frame.
    ///
    /// Not `frame_bytes`: [`AudioFormat::frame_bytes`] is the size of one
    /// SAMPLE frame, and two words that differ by three orders of magnitude
    /// must not share a name.
    frame_len: usize,
    /// Bytes per sample frame of the outbound format, the boundary a cut has
    /// to respect. 2 for the PCM16 mono of this protocol version.
    sample_bytes: usize,
    /// A tail that was not a whole number of samples, waiting for the rest of
    /// its sample.
    carry: Vec<u8>,
}

impl Framer {
    /// The framer for one synthesis in `format`, cutting `frame_ms` per frame.
    ///
    /// `frame_ms == 0`, an unknown output format, or a rate so low that a frame
    /// would be less than one sample all mean the same thing: pass the chunks
    /// through as they come.
    fn new(format: Option<AudioFormat>, frame_ms: u32) -> Self {
        let sample_bytes = format.map_or(2, |f| f.frame_bytes()).max(1);
        let frame_len = match format {
            Some(f) if frame_ms > 0 => {
                let raw = f.sample_rate as usize * sample_bytes * frame_ms as usize / 1000;
                raw - raw % sample_bytes
            }
            _ => 0,
        };
        Self {
            frame_len,
            sample_bytes,
            carry: Vec::new(),
        }
    }

    /// The frames this chunk becomes, in order.
    fn push(&mut self, chunk: Vec<u8>) -> Vec<Vec<u8>> {
        // The passthrough is literal: one chunk, one frame, whatever it holds.
        // An empty chunk stays an empty frame here rather than being swallowed,
        // because `0` promises the provider's own output and nothing else — the
        // framed path below is the one that decides what a frame is.
        if self.frame_len == 0 {
            return vec![chunk];
        }
        let mut buf = if self.carry.is_empty() {
            chunk
        } else {
            let mut held = std::mem::take(&mut self.carry);
            held.extend_from_slice(&chunk);
            held
        };
        let whole = buf.len() / self.frame_len * self.frame_len;
        let tail = buf.split_off(whole);
        let mut out: Vec<Vec<u8>> = buf.chunks(self.frame_len).map(<[u8]>::to_vec).collect();
        if tail.len() % self.sample_bytes == 0 {
            if !tail.is_empty() {
                out.push(tail);
            }
        } else {
            self.carry = tail;
        }
        out
    }

    /// What is left when the synthesis is over: a last, short frame, or nothing.
    ///
    /// Reachable only when the provider's whole output was not a whole number
    /// of samples — the one case where a frame may be odd, because the
    /// alternative is swallowing a byte the provider sent.
    fn flush(&mut self) -> Option<Vec<u8>> {
        (!self.carry.is_empty()).then(|| std::mem::take(&mut self.carry))
    }
}

/// What the synthesis channel produced.
enum SynthTick {
    /// Audio for the client.
    Chunk(Vec<u8>),
    /// The provider is done; the verdict is waiting.
    End,
}

/// Resolve when the cell's I/O half is gone, or never when there is none.
///
/// `changed()` errors when the last sender drops, and that drop is the whole
/// signal — nobody ever sends on this channel
/// ([`crate::voice::io::VoiceIoShared::shutdown`]).
async fn half_is_gone(shutdown: &mut Option<tokio::sync::watch::Receiver<()>>) {
    match shutdown {
        Some(rx) => {
            let _ = rx.changed().await;
        }
        None => std::future::pending().await,
    }
}

/// Wait until the handler has the session in its table, and only then let the
/// caller say `hello` (GH #836, OR-SN-14).
///
/// `register` puts `Connected` into the event channel, but the handler's loop
/// serves its mailbox before its events (`stop > mailbox > events`, the A4
/// contract of every long-running cell). A `hello` sent right after `register`
/// therefore raced the handler: an `in_speak` already queued in the mailbox was
/// answered `unknown_session` for a connection whose client held its `hello`
/// (measured: 1 of 500 test runs, a 30 s timeout with no name). The bias stays;
/// the handshake waits for the acknowledgement instead.
///
/// Bounded by `external_timeout` (hard rule 12). A handler that does not take
/// the session in time -- or is gone, which drops the acknowledgement -- gets
/// it taken back (`unregister`, so no half-registered session is left behind)
/// and the client a close [`CLOSE_SESSION_NOT_TAKEN`] instead of a `hello`.
/// Returns whether the connection may go on.
async fn session_taken(
    shared: &Arc<VoiceIoShared>,
    sink: &mut LinkSink,
    session_id: &str,
    conn_id: u64,
    taken: oneshot::Receiver<()>,
) -> bool {
    let why = match tokio::time::timeout(shared.external_timeout, taken).await {
        Ok(Ok(())) => return true,
        Ok(Err(_)) => "the handler is gone",
        Err(_) => "the cell did not take the session in time",
    };
    tracing::warn!(
        session_id,
        reason = why,
        timeout_ms = shared.external_timeout.as_millis() as u64,
        "voice: no hello -- the session was not taken"
    );
    unregister(shared, session_id, conn_id).await;
    sink.close(CLOSE_SESSION_NOT_TAKEN, why).await;
    false
}

/// Drive one connection until it ends.
#[allow(clippy::too_many_arguments)]
pub async fn run_connection(
    link: ClientLink,
    shared: Arc<VoiceIoShared>,
    session_id: String,
    mut mode: Mode,
    negotiated: Negotiated,
    conn_id: u64,
    to_conn_tx: mpsc::Sender<ToConnection>,
    mut to_conn_rx: mpsc::Receiver<ToConnection>,
) {
    // The third path, before anything reads the cascade pair (OR-L23). A
    // duplex cell carries an inert `EchoStt` in its recogniser slot, so a
    // reader that asked `shared.stt` first would see the name `echo` and take
    // the loopback branch below — where a `hold` is refused with "the echo
    // provider has no turns" and a model that was listening is never told to
    // unmute. Lock: `voice_duplex_never_reaches_the_echo_path`.
    if let Some(provider) = shared.duplex.clone() {
        return run_duplex(
            link, shared, provider, session_id, mode, negotiated, conn_id, to_conn_tx, to_conn_rx,
        )
        .await;
    }
    // Two halves rather than one link: the loop below reads the client in one
    // `select!` arm and writes to it from four others.
    let (mut sink, mut stream) = link.into_halves();
    let taken = register(&shared, &session_id, conn_id, to_conn_tx, mode).await;
    if !session_taken(&shared, &mut sink, &session_id, conn_id, taken).await {
        return;
    }

    let echo = shared.stt.name() == "echo";
    // The pair this connection agreed to in `?sample_rate=` (GH #619), not the
    // pair the providers declare: a telephony edge runs one call at 8 kHz
    // while a browser on the same cell runs the next at 16 kHz.
    let Negotiated {
        audio_in: input,
        audio_out: output,
    } = negotiated;
    let hello = ServerFrame::Hello {
        protocol: PROTOCOL,
        session_id: session_id.clone(),
        call_id: session_id.clone(),
        mode,
        audio_in: input,
        audio_out: output,
        // Both names through the declaration helpers rather than off `shared`
        // directly: in duplex mode the cascade pair is an inert placeholder and
        // the one provider answers for both directions (OR-L23).
        stt: stt_name(&shared),
        tts: tts_name(&shared),
        audio_out_frame_ms: audio_out_frame_ms(&shared),
        speak_plain: shared.speak_plain,
        release_grace_ms: shared.release_grace_ms,
        duplex: shared.duplex.is_some(),
    };
    if send_frame(&mut sink, &hello).await.is_err() {
        unregister(&shared, &session_id, conn_id).await;
        return;
    }
    // The zero of this connection's clock, for the `first_audio` stamp.
    let opened = Instant::now();

    // The echo path has no provider to lose, so it has no session to restart.
    //
    // Neither has a `hold` connection yet (GH #657): there the session belongs
    // to the hold rather than to the connection, and a page that is looked at
    // before anybody speaks would otherwise spend its whole life paying a
    // provider's idle deadline for silence it was designed to produce.
    let mut stt = if echo || mode == Mode::Hold {
        None
    } else {
        Some(SttSession::start(&shared, input))
    };
    // Whether a `hold` boundary is open (GH #657). The turn machine keeps its
    // own copy — it owns what a boundary MEANS — and this one answers a
    // narrower question the connection has to answer alone: whether a session
    // that just ended was carrying a held key or nobody's silence.
    let mut holding = false;
    // What this hold has pushed at the recognition provider so far.
    let mut hold_frames: u32 = 0;
    let mut hold_bytes: u64 = 0;
    // When the current hold was opened, for the one line it produces.
    let mut hold_since: Option<Instant> = None;
    let mut stt_retried = false;
    let mut retry_at: Option<Instant> = None;
    let mut speaking: Option<Speaking> = None;
    // Beside `speaking` rather than inside it, so the deadline arm below can
    // read it while the chunk arm holds `speaking` mutably. `None` whenever
    // nothing is being spoken.
    let mut speak_deadline: Option<Instant> = None;
    let mut closing: Option<u16> = None;
    // The cell's own way out. An upgraded socket runs on a task axum spawned,
    // which nothing in the I/O half holds a handle to, so this is how it hears
    // that the half is gone — see [`crate::voice::io::VoiceIoShared::shutdown`].
    let mut half_gone = shared.shutdown();
    // R-V6': how many mis-framed binary frames this connection has sent. The
    // counter lives here because it is per connection and per socket, and it
    // is what makes a systematic mis-framing visible without this half having
    // to pick a threshold.
    let mut bad_frames: u32 = 0;
    let frame_bytes = input.frame_bytes();
    // The silence tail of a released hold (GH #717). `tail_until` is the
    // deadline it stops at — the grace, because past that the boundary is cut
    // anyway — and `tail_next` is when the next frame is due. Both `None`
    // whenever no tail is running, which is the whole time outside a drain.
    //
    // WHY this exists at all: an endpointing provider ends a turn after a
    // stretch of silence IN THE AUDIO STREAM (Deepgram Flux `eot_threshold`,
    // 0.7 s by default), and a client stops sending about 120 ms after the key
    // comes up. Measured on the fresh instance on 18.09.2026 with a fixture
    // released 20 ms after the last word: no audio after the release, no
    // `EndOfTurn` ever, the boundary cut at the 1500 ms cap with the interim
    // transcript — which lags the audio, so the end of the sentence was gone.
    // The same take released 1.62 s later, with silence still streaming,
    // closed 137 ms after the release with the full text. The difference was
    // the silence, so the cell produces it.
    let mut tail_until: Option<Instant> = None;
    let mut tail_next: Option<Instant> = None;
    // One 20 ms frame of digital silence at the rate THIS connection
    // negotiated — a telephony edge runs 8 kHz while the browser beside it
    // runs 16 kHz, and a frame of the wrong length is a frame the provider
    // reads as noise.
    let tail_frame: Vec<u8> =
        vec![
            0u8;
            input.sample_rate as usize * frame_bytes * TAIL_FRAME.as_millis() as usize / 1000
        ];

    loop {
        tokio::select! {
            biased;

            // The cell is going away, and the close frame is the point: a
            // caller whose socket reads one knows the call is over, while a
            // socket that merely stops answering is a line nobody is on. `1001`
            // is the code the ordinary end of this half uses as well, so the
            // client sees one story either way.
            _ = half_is_gone(&mut half_gone) => {
                closing = Some(1001);
                break;
            }

            // `biased;` makes this order a decision rather than a coin toss,
            // and the order is by urgency, not by volume. Commands first — a
            // `cancel` must not queue behind anything. Then the two deadlines,
            // which only fire when something is already wrong. Then the
            // provider and the client. **Synthesis chunks come last on
            // purpose:** a provider that bursts a whole sentence would
            // otherwise keep winning this `select!` and hold a barge-in off
            // the socket for the length of its buffer.
            cmd = to_conn_rx.recv() => {
                match cmd {
                    // The registry dropped this connection's sender without
                    // saying anything first. Nothing left to serve.
                    None => break,
                    Some(ToConnection::Close(code)) => {
                        closing = Some(code);
                        break;
                    }
                    Some(ToConnection::Frame(frame)) => {
                        if send_frame(&mut sink, &frame).await.is_err() {
                            break;
                        }
                    }
                    // A cascade connection has no append channel: there is no
                    // model here to take guidance up, only a recogniser and a
                    // voice. The handler refuses an `in_advise` on this engine
                    // with `wrong_engine` before it ever gets this far
                    // (contract § 1.5), so an append arriving HERE is a wiring
                    // fault worth a loud line rather than a dropped sentence.
                    // The duplex path is `run_duplex` and is strand L2b's.
                    Some(ToConnection::Advise { kind, event_id, .. }) => {
                        tracing::error!(
                            %session_id, ?kind, %event_id,
                            "voice: an append reached a cascade connection, which has \
                             no channel for one"
                        );
                    }
                    // A seed opens a duplex session, a renewal renews one; a
                    // cascade has none.
                    Some(ToConnection::Renew) => {
                        tracing::debug!(%session_id, "voice: a renewal reached a cascade connection");
                    }
                    Some(ToConnection::Seed { .. }) => {
                        tracing::debug!(%session_id, "voice: a seed reached a cascade connection");
                    }
                    Some(ToConnection::CancelSpeak) => {
                        speak_deadline = None;
                        if let Some(sp) = speaking.take() {
                            sp.stop();
                            let id = sp.speak_id.clone();
                            if finish_speak(
                                &shared, &session_id, &mut sink, id,
                                SpeakEndReason::Cancelled, None,
                            ).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(ToConnection::Speak { speak_id, text }) => {
                        // One synthesis at a time: the queue is the handler's
                        // (spec § 4). An overlapping order can only mean the
                        // handler moved on, so the running one is stopped and
                        // reported rather than left to fight for the socket.
                        // The deadline is not cleared here: whichever way this
                        // arm ends, it is set from the new synthesis below.
                        if let Some(prev) = speaking.take() {
                            prev.stop();
                            let id = prev.speak_id.clone();
                            if finish_speak(
                                &shared, &session_id, &mut sink, id,
                                SpeakEndReason::Cancelled, None,
                            ).await.is_err() {
                                // The previous synthesis was reported —
                                // `finish_speak` emits before it reports the
                                // write — but THIS order has already left the
                                // command channel and would leave with nobody
                                // ever hearing about it. Same promise, third
                                // arm.
                                shared
                                    .emit(VoiceEvent::SpeakEnded {
                                        session_id: session_id.clone(),
                                        speak_id,
                                        reason: SpeakEndReason::Failed,
                                        detail: Some(
                                            "the connection went away before the \
                                             synthesis started"
                                                .to_string(),
                                        ),
                                    })
                                    .await;
                                break;
                            }
                        }
                        match start_speak(&shared, &mut sink, &session_id, output, speak_id, text)
                            .await
                        {
                            Err(()) => break,
                            Ok(started) => {
                                // The first chunk is the provider's first answer
                                // and gets the connect-shaped A-timeout.
                                speak_deadline = started
                                    .is_some()
                                    .then(|| Instant::now() + shared.external_timeout);
                                speaking = started;
                            }
                        }
                    }
                }
            }

            () = sleep_until_opt(speak_deadline) => {
                // The synthesis socket owed a chunk and did not deliver one.
                speak_deadline = None;
                if let Some(sp) = speaking.take() {
                    sp.stop();
                    let id = sp.speak_id.clone();
                    let detail = format!(
                        "no audio for {:?}, the synthesis socket is presumed dead",
                        shared.idle_timeout
                    );
                    if finish_speak(
                        &shared, &session_id, &mut sink, id,
                        SpeakEndReason::Failed, Some(detail),
                    ).await.is_err() {
                        break;
                    }
                }
            }

            () = sleep_until_opt(retry_at) => {
                retry_at = None;
                // In `hold` a retry the client is not holding for would open a
                // session nobody feeds, and it would die of the same idle
                // deadline a moment later. The next `hold` opens one.
                if mode != Mode::Hold || holding {
                    stt = Some(SttSession::start(&shared, input));
                }
            }

            event = next_stt_event(&mut stt) => {
                match event {
                    Some(event) => {
                        // An event is a completed provider round trip (issue #7).
                        shared.liveness.mark_success();
                        // The tail has done its job the moment the provider
                        // leaves its turn (GH #717): what it was there for was
                        // this event, and more silence after it belongs to
                        // nobody's take.
                        if matches!(event, SttEvent::EndOfTurn { .. }) {
                            tail_until = None;
                            tail_next = None;
                        }
                        shared.emit(VoiceEvent::Stt {
                            session_id: session_id.clone(),
                            event,
                        }).await;
                    }
                    // The session is over, one way or another. Both endings are
                    // the same problem for the client — nothing is being
                    // recognised any more — so both take the retry path.
                    None => {
                        // There is nothing left to feed.
                        tail_until = None;
                        tail_next = None;
                        let Some(session) = stt.take() else { break };
                        // GH #657: in `hold` mode a session that ends with no
                        // key held ended the way the arrangement intends.
                        // Between two holds no audio flows, so the provider's
                        // idle deadline is certain to expire — and telling a
                        // page that its microphone failed, then closing the
                        // socket under it, is how a loaded screen used to end
                        // up with a dead button after one minute of quiet. The
                        // session is dropped in silence and the next `hold`
                        // opens a new one.
                        if mode == Mode::Hold && !holding {
                            // The session goes in silence, but the state machine
                            // is told: `Closed` is the only thing that writes a
                            // provider debt off, and a debt that outlives the
                            // socket it was owed on is paid by the end of the
                            // NEXT take (GH #697). It reaches no client and no
                            // error lane — `cell.rs` reports only `Failed` and
                            // `Warning` — so this is a fact the session learns,
                            // not a failure anybody is told about.
                            tracing::info!(
                                %session_id,
                                "voice: the recognition session ended between two holds"
                            );
                            shared.emit(VoiceEvent::Stt {
                                session_id: session_id.clone(),
                                event: SttEvent::Closed,
                            }).await;
                            drop(session);
                            // And it costs no retry: the budget exists for a
                            // provider that refuses twice in a row, not for a
                            // connection that outlives a day of quiet holds.
                            // Without this a single failure in the morning
                            // makes the next one in the evening a `1011` —
                            // the dead button this task was built against.
                            stt_retried = false;
                            continue;
                        }
                        let (event, detail) = session.outcome(shared.external_timeout).await;
                        shared.emit(VoiceEvent::Stt {
                            session_id: session_id.clone(),
                            event,
                        }).await;
                        // Told on the FIRST failure, not only on the last: for
                        // the second the retry takes there is nowhere for audio
                        // to go, and a client that keeps talking into that gap
                        // has a right to know its words are being dropped
                        // rather than merely mis-heard.
                        let frame = ServerFrame::Error {
                            code: WireErrorCode::SttFailed,
                            detail,
                            bad_frames: None,
                        };
                        if send_frame(&mut sink, &frame).await.is_err() {
                            break;
                        }
                        if stt_retried {
                            closing = Some(1011);
                            break;
                        }
                        stt_retried = true;
                        retry_at = Some(Instant::now() + STT_RETRY_DELAY);
                    }
                }
            }

            incoming = stream.next() => {
                let Some(message) = incoming else { break };
                match message {
                    Incoming::Binary(bytes) => {
                        if frame_bytes == 0 || bytes.len() % frame_bytes != 0 {
                            // Never trimmed, never guessed — and since R-V6'
                            // never fatal either: the frame is dropped, the
                            // call goes on, and the counter is what tells one
                            // lost buffer apart from a client that has the
                            // sample format wrong. No threshold: hanging up on
                            // somebody mid-sentence is a worse answer than a
                            // number that climbs.
                            //
                            // The client's `error` frame is the **handler's**
                            // to send. It already dispatches one for this event
                            // (`cell.rs`, `VoiceEvent::BadAudioFrame`), so
                            // sending a second from here would double every
                            // report. That fixes the order as lane-then-frame,
                            // out of one place, rather than the other way round
                            // out of two.
                            bad_frames = bad_frames.saturating_add(1);
                            shared.emit(VoiceEvent::BadAudioFrame {
                                session_id: session_id.clone(),
                                len: bytes.len(),
                                count: bad_frames,
                            }).await;
                            continue;
                        }
                        if echo {
                            if sink.send(Outgoing::Binary(bytes)).await.is_err() {
                                break;
                            }
                        } else {
                            // Audio with nowhere to go opens the door itself
                            // (GH #657), and only in `hold` mode: there a
                            // session ends whenever the key is up, so a client
                            // that keeps its capture graph running between two
                            // holds must not stream into a session that is not
                            // there. In `auto` this cannot fire: the session is
                            // opened with the connection or with the switch
                            // that asked for `auto`, and the gap a retry leaves
                            // is the retry's to fill.
                            if stt.is_none() && retry_at.is_none() && mode == Mode::Hold {
                                stt = Some(SttSession::start(&shared, input));
                            }
                            let len = bytes.len() as u64;
                            if let Some(session) = stt.as_ref() {
                                // Blocks when the provider is behind — that is
                                // the backpressure, and it reaches the client
                                // as TCP.
                                if session.audio_tx.send(bytes).await.is_err() {
                                    tracing::debug!(%session_id, "voice: the recognition session is gone");
                                } else if holding {
                                    hold_frames = hold_frames.saturating_add(1);
                                    hold_bytes = hold_bytes.saturating_add(len);
                                } else if tail_next.is_some() {
                                    // The client is still draining its capture
                                    // graph, so its audio outranks the cell's
                                    // silence (GH #717): the tail waits a frame
                                    // rather than interleaving zeroes between
                                    // the last two words of the take.
                                    tail_next = Some(Instant::now() + TAIL_FRAME);
                                }
                            }
                        }
                        // Between a lost session and its retry there is nowhere
                        // for audio to go; that window is the price of the one
                        // retry and is a second of it.
                    }
                    Incoming::Text(text) => {
                        match handle_text(&shared, &session_id, &mut sink, echo, &text).await {
                            Err(()) => break,
                            // The boundary frames are the only ones this loop
                            // reads at all; what they MEAN is still the turn
                            // machine's business (GH #657).
                            // Only in `hold`: in `auto` the turn machine
                            // answers this frame with `wrong_mode` and draws no
                            // boundary, and a connection that marked itself as
                            // holding anyway would ignore the `mode` frame that
                            // usually follows — leaving the two halves in two
                            // modes, which is this whole task's defect again.
                            Ok(Some(ClientFrame::Hold)) if mode == Mode::Hold => {
                                // Only a hold that OPENS counts: a second `hold`
                                // inside an open boundary is refused by the turn
                                // machine (`already_holding`) and must not zero
                                // the numbers of the one that is running.
                                if !holding {
                                    // A new take owns the stream: whatever is
                                    // left of the previous one's tail would be
                                    // silence in the middle of this one's
                                    // first word.
                                    tail_until = None;
                                    tail_next = None;
                                    hold_frames = 0;
                                    hold_bytes = 0;
                                    hold_since = Some(Instant::now());
                                    // What the recognition half looked like when
                                    // the key went down. A hold that opens onto a
                                    // session that is not there, or onto one that
                                    // has been sitting since the last take, is the
                                    // difference between a lost take and a slow
                                    // one (GH #697).
                                    tracing::info!(
                                        %session_id,
                                        stt = if stt.is_some() { "reused" }
                                              else if retry_at.is_some() { "retrying" }
                                              else { "fresh" },
                                        "voice: hold opened"
                                    );
                                }
                                holding = true;
                                // A new hold is a new attempt, and the one
                                // retry belongs to it: the budget exists for a
                                // provider that refuses twice while somebody is
                                // speaking, not for a page that has been open
                                // since this morning. Without this line a
                                // single failure hours ago turns the next one
                                // into a `1011` — the dead button again.
                                stt_retried = false;
                                // The channel exists the moment the session
                                // does, so the audio frames behind this text
                                // frame queue up in it rather than waiting for
                                // a provider socket to come up.
                                if stt.is_none() && retry_at.is_none() && !echo {
                                    stt = Some(SttSession::start(&shared, input));
                                }
                            }
                            // `cancel` does NOT close it: the turn machine
                            // leaves the boundary standing (it drops the queue
                            // and the synthesis, nothing else), and a
                            // connection that thought otherwise would read the
                            // next provider death as nobody's silence while
                            // the client is still holding the key down.
                            Ok(Some(ClientFrame::Release)) => {
                                holding = false;
                                // The drain begins, and with it the tail
                                // (GH #717). Only where it can do anything: in
                                // `hold`, with a session to feed, and with a
                                // grace to run inside — `release_grace_ms: 0`
                                // cuts on this very frame, so a tail would be
                                // audio for a boundary that is already closed.
                                if mode == Mode::Hold
                                    && stt.is_some()
                                    && shared.release_grace_ms > 0
                                {
                                    let now = Instant::now();
                                    tail_until = Some(
                                        now + Duration::from_millis(shared.release_grace_ms),
                                    );
                                    // The origin of the tail's raster (GH #1199):
                                    // every later frame is due a whole number of
                                    // `TAIL_FRAME`s after this one, see
                                    // [`next_tail_slot`].
                                    tail_next = Some(now + TAIL_FRAME);
                                }
                                // The one line a lost take is reconstructed from:
                                // how much audio this hold actually pushed, and
                                // how long it was open. No transcript, ever —
                                // what somebody said is not log material. Only
                                // for a hold that was open: a `release` with no
                                // boundary is `not_holding` and has nothing to
                                // report.
                                if let Some(since) = hold_since.take() {
                                    tracing::info!(
                                        %session_id,
                                        frames = hold_frames,
                                        bytes = hold_bytes,
                                        ms = since.elapsed().as_millis() as u64,
                                        "voice: hold released"
                                    );
                                }
                            }
                            // The mode of this connection is not frozen at the
                            // handshake (GH #657 review): the built-in page
                            // switches it on a live socket, and the two arms
                            // above decide on it. Outside a hold, because that
                            // is exactly where the turn machine accepts the
                            // switch — inside one it refuses, and the two
                            // copies must not drift apart.
                            Ok(Some(ClientFrame::Mode { mode: wanted })) if !holding => {
                                mode = wanted;
                                // In `auto` the session IS the connection:
                                // there is no `hold` to open one, and a client
                                // that switched its radio streams from the next
                                // frame on. So the switch is where it opens —
                                // without this the audio of a page that asked
                                // for `auto` goes nowhere, silently and for as
                                // long as the socket lives.
                                if mode == Mode::Auto
                                    && stt.is_none()
                                    && retry_at.is_none()
                                    && !echo
                                {
                                    stt = Some(SttSession::start(&shared, input));
                                }
                            }
                            Ok(_) => {}
                        }
                    }
                    Incoming::Close => break,
                }
            }
            // The silence tail of a released hold (GH #717). Deliberately below
            // the client and the provider in the `biased` order: real audio and
            // a provider event both outrank a frame the cell invented.
            () = sleep_until_opt(tail_next) => {
                let now = Instant::now();
                match (tail_until, stt.as_ref()) {
                    (Some(until), Some(session)) if now < until => {
                        // `try_send`, never `send`. The client's own frames may
                        // block this loop — that is its backpressure and the
                        // socket carries it — but a frame the cell made up must
                        // not: a full queue is already 32 frames the provider
                        // has not read, one more would not move its endpointing,
                        // and waiting for room would park the very arm that is
                        // waiting for `EndOfTurn`.
                        if session.audio_tx.try_send(tail_frame.clone()).is_err() {
                            // Reported rather than swallowed: a drain whose
                            // silence never lands is a take that will be cut at
                            // the cap again, and the queue being full says the
                            // provider is already 32 frames behind.
                            tracing::debug!(
                                %session_id,
                                "voice: the silence tail dropped a frame — the \
                                 recognition queue is full"
                            );
                        }
                        // On the raster, not from the wake (GH #1199): the
                        // next frame is due one `TAIL_FRAME` after the one
                        // that was due now, whenever this arm got to run.
                        let due = tail_next.unwrap_or(now);
                        tail_next = Some(next_tail_slot(due, now));
                    }
                    // The grace has run out, or there is nothing left to feed.
                    // The cap closes the boundary (`turns.rs`, `ProviderDebt`),
                    // and the tail ends with it rather than running for the
                    // rest of the call.
                    _ => {
                        tail_until = None;
                        tail_next = None;
                    }
                }
            }

            tick = next_synth_tick(&mut speaking) => {
                match tick {
                    SynthTick::Chunk(bytes) => {
                        // A chunk is a completed provider round trip (issue #7),
                        // and the only thing that moves the deadline: from here
                        // on the socket owes the next chunk within the idle
                        // deadline rather than the connect-shaped one.
                        shared.liveness.mark_success();
                        speak_deadline = Some(Instant::now() + shared.idle_timeout);
                        // Framed here rather than at the provider: the chunk
                        // size is the vendor's business, the frame size is the
                        // client's, and this is the only place that knows both.
                        let (frames, first) = match speaking.as_mut() {
                            Some(sp) => {
                                let frames = sp.framer.push(bytes);
                                let first = (shared.emit_first_audio
                                    && sp.first_audio.leaves_with(&frames))
                                .then(|| sp.speak_id.clone());
                                (frames, first)
                            }
                            None => (vec![bytes], None),
                        };
                        if send_audio(&mut sink, frames).await.is_err() {
                            break;
                        }
                        // d54g callseam: the stamp of the way through a
                        // speech synthesis. After the send, so it says the
                        // audio went out; only where ordered, so a cell that
                        // did not ask sends the handler what it always sent.
                        if let Some(speak_id) = first {
                            shared
                                .emit(VoiceEvent::SpeakAudioOut {
                                    session_id: session_id.clone(),
                                    speak_id,
                                    at_ms: since_ms(opened),
                                })
                                .await;
                        }
                    }
                    SynthTick::End => {
                        speak_deadline = None;
                        if let Some(mut sp) = speaking.take() {
                            let id = sp.speak_id.clone();
                            // A held part-sample is the provider's last bytes,
                            // not noise: it leaves as a short last frame ahead
                            // of the verdict, in the order a client plays it.
                            let last: Vec<Vec<u8>> = sp.framer.flush().into_iter().collect();
                            // A synthesis shorter than one frame: its held
                            // part-sample is its first audio.
                            let first =
                                shared.emit_first_audio && sp.first_audio.leaves_with(&last);
                            let flushed = send_audio(&mut sink, last).await;
                            if first && flushed.is_ok() {
                                shared
                                    .emit(VoiceEvent::SpeakAudioOut {
                                        session_id: session_id.clone(),
                                        speak_id: id.clone(),
                                        at_ms: since_ms(opened),
                                    })
                                    .await;
                            }
                            let (reason, detail) = sp.outcome(shared.external_timeout).await;
                            if flushed.is_err() {
                                // The socket died on the last frame. `speaking`
                                // is already empty, so the post-loop block will
                                // find nothing — the verdict still has to reach
                                // the handler, which counts on exactly one
                                // `SpeakEnded` per `Speak`. Only the lane, not
                                // the frame: nobody is reading the socket.
                                shared
                                    .emit(VoiceEvent::SpeakEnded {
                                        session_id: session_id.clone(),
                                        speak_id: id,
                                        reason,
                                        detail,
                                    })
                                    .await;
                                break;
                            }
                            if finish_speak(
                                &shared, &session_id, &mut sink, id, reason, detail,
                            ).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }

        }
    }

    // The connection ended with a synthesis still running — a client close, a
    // displacement, a bad audio frame, a broken socket. The handler is counting
    // on exactly one `SpeakEnded` per `Speak` it issued, and this is the only
    // place left to send it. No `speak_end` frame: there is nobody to read it.
    if let Some(sp) = speaking.take() {
        sp.stop();
        shared
            .emit(VoiceEvent::SpeakEnded {
                session_id: session_id.clone(),
                speak_id: sp.speak_id.clone(),
                reason: SpeakEndReason::Cancelled,
                detail: None,
            })
            .await;
    }
    if let Some(code) = closing {
        sink.close(code, "").await;
    }
    unregister(&shared, &session_id, conn_id).await;
}

// ──────────────────────────────────────────────────────────────────────────
// The duplex path (welle live, contract § 1.3)
// ──────────────────────────────────────────────────────────────────────────

/// The cut of one append for a provider whose block names no ceiling of its
/// own, in characters, and the most `duplex.advise_split_chars` may say.
///
/// `gpt_live` counts its ceiling in tokens (`advise_max_tokens` times
/// `chars_per_token`, 1 080 characters as shipped): 1 800 characters was the
/// first reading of its 500 tokens, and the provider refused appends under
/// it. Longer guidance is split by the handler, never here.
pub(crate) const APPEND_MAX_CHARS: usize = 1_800;

/// What stands before the part of the caller's sentence a session is told
/// when it takes over inside that sentence (d54g callseam).
///
/// Written here and not by the colony: the takeover at the cap happens on the
/// connection's clock, and a round trip through a colony at that moment is the
/// wait the cap ends. It names what the words are and nothing about them, and
/// it holds whether the caller is still talking or the model is answering
/// inside the same turn.
const HEARD_SO_FAR_LEAD: &str =
    "The caller's turn is still open. What they have said in it so far: ";

/// `text` in pieces one append can carry ([`APPEND_MAX_CHARS`]), cut between
/// words where there is a word boundary and inside a word only where one word
/// alone is longer than an append. Joined, the pieces are `text`.
///
/// The handler splits the guidance a colony writes; this is for the one text
/// the connection writes itself, the caller's sentence at a takeover.
fn append_parts(text: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut part = String::new();
    let mut len = 0usize;
    for word in text.split_inclusive(char::is_whitespace) {
        if len > 0 && len + word.chars().count() > APPEND_MAX_CHARS {
            parts.push(std::mem::take(&mut part));
            len = 0;
        }
        for ch in word.chars() {
            if len == APPEND_MAX_CHARS {
                parts.push(std::mem::take(&mut part));
                len = 0;
            }
            part.push(ch);
            len += 1;
        }
    }
    if !part.is_empty() {
        parts.push(part);
    }
    parts
}

/// One running duplex session, as the connection holds it.
///
/// The counterpart of [`SttSession`] and [`Speaking`] in one: a duplex model is
/// both directions on one socket, so there is one thing to start and one
/// verdict to wait for rather than two of each.
///
/// A call normally holds one of these for its whole life. A call whose session
/// is renewed (GH #896) holds two for a while — the second is opened BEFORE the
/// first ends, so the two overlap and nobody hears a gap — and
/// [`Self::clock_base_ms`] is what puts the second one's clock where the first
/// one's stood.
struct DuplexRun {
    /// Audio on its way to the model. One client frame is one item and nothing
    /// is merged (R-L4); a full channel blocks the reader, which is the
    /// backpressure ADR-0023 asks for.
    audio_tx: mpsc::Sender<Vec<u8>>,
    /// Audio the model produced, one decoded chunk per item.
    audio_out_rx: mpsc::Receiver<Vec<u8>>,
    /// Everything the model said about itself.
    events_rx: mpsc::Receiver<DuplexEvent>,
    /// What this colony tells the model between two turns.
    control_tx: mpsc::Sender<DuplexControl>,
    /// The verdict, once the session future is over.
    done_rx: oneshot::Receiver<Result<(), DuplexError>>,
    /// Where this session's clock zero lies on the call's timeline, in
    /// milliseconds (GH #896).
    ///
    /// A provider session counts its own clock from zero, and a renewed one
    /// starts that count again. The turn machine in the handler measures gaps
    /// against ONE continuous clock, so every stamped event of this session is
    /// moved by this much before it is read or passed on ([`shift_event`]).
    /// `0` for the first session of a call, which is all a call that is never
    /// renewed ever has.
    clock_base_ms: u64,
}

impl DuplexRun {
    /// Start the first session of a call, at the format it negotiated.
    fn start(
        shared: &Arc<VoiceIoShared>,
        provider: &Arc<dyn DuplexProvider>,
        format: AudioFormat,
    ) -> Self {
        Self::spawn(shared, provider, format, false)
    }

    /// Start a renewed session of a call that is already running (GH #896).
    ///
    /// The same session at the same format, opened through
    /// [`DuplexProvider::run_renewed_session`] so that a provider which greets
    /// a caller does not greet one who is mid-conversation.
    fn start_renewed(
        shared: &Arc<VoiceIoShared>,
        provider: &Arc<dyn DuplexProvider>,
        format: AudioFormat,
    ) -> Self {
        Self::spawn(shared, provider, format, true)
    }

    /// The four channels, the task that runs the provider on them, and the
    /// verdict it hands back.
    fn spawn(
        shared: &Arc<VoiceIoShared>,
        provider: &Arc<dyn DuplexProvider>,
        format: AudioFormat,
        renewed: bool,
    ) -> Self {
        let (audio_tx, audio_in) = mpsc::channel::<Vec<u8>>(AUDIO_QUEUE);
        let (audio_out, audio_out_rx) = mpsc::channel::<Vec<u8>>(AUDIO_QUEUE);
        let (events, events_rx) = mpsc::channel::<DuplexEvent>(AUDIO_QUEUE);
        let (control_tx, control) = mpsc::channel::<DuplexControl>(AUDIO_QUEUE);
        let (done_tx, done_rx) = oneshot::channel();
        let provider = provider.clone();
        let liveness = shared.liveness.clone();
        tokio::spawn(async move {
            let session = DuplexSession {
                audio_in,
                audio_out,
                events,
                control,
            };
            let outcome = if renewed {
                provider
                    .run_renewed_session(format, session, liveness)
                    .await
            } else {
                provider.run_session(format, session, liveness).await
            };
            let _ = done_tx.send(outcome);
        });
        Self {
            audio_tx,
            audio_out_rx,
            events_rx,
            control_tx,
            done_rx,
            clock_base_ms: 0,
        }
    }
}

/// A duplex session opened while its line still rings (GH #1055).
///
/// The model is told nothing different: it is the session `run_duplex` would
/// open, opened earlier. Its two outbound channels do not reach the
/// connection directly while it is parked: a [`park_pump`] sits between, so
/// the provider's loop never waits on a queue nobody empties (see there), and
/// what the model said worth hearing reaches the caller the moment the media
/// connection adopts the run ([`Self::adopt`]). Dropping it ends the session:
/// the pump goes with the adoption signal it waits for, and the provider sees
/// its way in close.
pub(crate) struct Prewarmed {
    run: DuplexRun,
    format: AudioFormat,
    /// Fired by [`Self::adopt`]; dropped unfired, it stops the pump.
    adopt: oneshot::Sender<()>,
}

impl Prewarmed {
    /// Hand the parked session its seed (D48): it waits for it before its
    /// `session.start`, while the line still rings. Never waits: a full
    /// control channel is a session that is not reading, and its own
    /// deadline starts it without the seed.
    pub(crate) fn seed(&self, messages: Vec<crate::voice::contract::SeedMessage>) {
        if self
            .run
            .control_tx
            .try_send(DuplexControl::Seed { messages })
            .is_err()
        {
            // Review K8: never silent -- the session starts without it.
            tracing::warn!("voice: the parked session is not reading, its seed was dropped");
        }
    }

    /// Whether the provider's run is already over: its handshake failed while
    /// the line rang, or it ran and ended (GH #1055, the review's finding
    /// M1). The verdict is there, or the task that owed it is gone. Such a
    /// session has no model behind it and is not adopted.
    fn is_over(&mut self) -> bool {
        !matches!(
            self.run.done_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        )
    }

    /// The connection takes the run: the pump sends what it kept, ahead of
    /// everything the model says from here on.
    fn adopt(self) -> DuplexRun {
        let _ = self.adopt.send(());
        self.run
    }
}

/// How much of what a parked model SAID is kept for the caller, in
/// milliseconds of audible audio. It bounds the speech, not the ring: the
/// silence a model streams while it waits for the pickup does not count, so a
/// ring longer than this leaves the greeting where it was. Past it the oldest
/// goes, so a caller who picks up late hears the end of what the model said
/// rather than a minute of it (GH #1055).
const PARK_KEEP_MS: u64 = 15_000;

/// What a parked session keeps of its model for the caller who picks up
/// (GH #1055, the review's finding C1).
///
/// A live model streams silence between and after its phrases. Kept as it
/// came, ten seconds of ringing after the greeting were ten seconds of
/// silence in front of every later word, and the cap counted them, so a ring
/// longer than [`PARK_KEEP_MS`] pushed the greeting out. Hence three states
/// for a chunk:
/// - silence before the first audible window: dropped;
/// - audible: kept, and counted against the cap;
/// - silence after something audible: `pending`. It is the model's own pause
///   if it speaks again, and joins what is kept then; it is the wait for the
///   pickup if it does not, and is dropped at the adoption.
///
/// Only the model's first answer, the greeting, is kept. What it says after
/// it into the ring is said to a caller who is not there yet: eight seconds of
/// its own silence and a second phrase behind the greeting were what the
/// caller heard first. Where the greeting ends is the turn machine's verdict
/// ([`ParkedGreeting`]), and [`Self::end_greeting`] is what it does here.
struct ParkBuffer {
    rate: u32,
    level: f64,
    /// Kept for the caller, oldest first; `true` marks a chunk with speech.
    /// The number is where the chunk stood on the session's clock when it
    /// arrived, for a pause where the pause began.
    kept: std::collections::VecDeque<(Vec<u8>, bool, u64)>,
    /// The audible part of `kept`, the one [`PARK_KEEP_MS`] bounds.
    spoken_ms: u64,
    /// Silence since the last audible chunk, not yet a pause and not yet the
    /// wait for the pickup. Bounded by the session's deadline
    /// (`prewarm_ttl_ms`), like the events a parked session keeps.
    pending: Vec<Vec<u8>>,
    /// Where `pending` began on the session's clock.
    pending_at_ms: u64,
    /// Silence dropped before the first audible window.
    dropped_ms: u64,
    /// What the cap pushed out.
    trimmed_ms: u64,
    /// The greeting is over: nothing the model sends from here on is kept.
    closed: bool,
    /// What the model said and streamed after its greeting, dropped.
    cut_ms: u64,
}

/// What a [`ParkBuffer`] hands the connection at the adoption.
struct Parked {
    /// The chunks for the caller, oldest first.
    audio: std::collections::VecDeque<Vec<u8>>,
    /// Their length.
    kept_ms: u64,
    /// Silence dropped before the first audible window.
    dropped_ms: u64,
    /// What the cap pushed out.
    trimmed_ms: u64,
    /// The silence after the last audible chunk, dropped here.
    tail_ms: u64,
    /// What the model sent after its greeting, dropped.
    cut_ms: u64,
}

impl ParkBuffer {
    fn new(rate: u32, level: f64) -> Self {
        Self {
            rate,
            level,
            kept: std::collections::VecDeque::new(),
            spoken_ms: 0,
            pending: Vec::new(),
            pending_at_ms: 0,
            dropped_ms: 0,
            trimmed_ms: 0,
            closed: false,
            cut_ms: 0,
        }
    }

    /// File one chunk the parked model sent; `at_ms` is where the session's
    /// clock stood when it arrived.
    fn push(&mut self, chunk: Vec<u8>, at_ms: u64) {
        if self.closed {
            self.cut_ms += pcm_ms(chunk.len(), self.rate);
            return;
        }
        let Some(at) = crate::voice::duck::first_audible(&chunk, self.rate, self.level) else {
            if self.kept.is_empty() {
                self.dropped_ms += pcm_ms(chunk.len(), self.rate);
            } else {
                if self.pending.is_empty() {
                    self.pending_at_ms = at_ms;
                }
                self.pending.push(chunk);
            }
            return;
        };
        let chunk = if self.kept.is_empty() {
            // The first word starts what is kept, to within one window.
            self.dropped_ms += pcm_ms(at, self.rate);
            chunk[at..].to_vec()
        } else {
            chunk
        };
        // The model speaks again: the silence before this was its own pause.
        let pause_at_ms = self.pending_at_ms;
        for pause in self.pending.drain(..) {
            self.kept.push_back((pause, false, pause_at_ms));
        }
        self.spoken_ms += pcm_ms(chunk.len(), self.rate);
        self.kept.push_back((chunk, true, at_ms));
        while self.spoken_ms > PARK_KEEP_MS {
            let Some((old, spoken, _)) = self.kept.pop_front() else {
                break;
            };
            let ms = pcm_ms(old.len(), self.rate);
            self.trimmed_ms += ms;
            if spoken {
                self.spoken_ms = self.spoken_ms.saturating_sub(ms);
            }
        }
        // What is kept never starts with a pause.
        while matches!(self.kept.front(), Some((_, false, _))) {
            if let Some((old, _, _)) = self.kept.pop_front() {
                self.trimmed_ms += pcm_ms(old.len(), self.rate);
            }
        }
    }

    /// The greeting is over: the model began a second answer at `start_ms` on
    /// its clock. What is kept ends where the pause before that answer began,
    /// and nothing that arrives from here on is kept.
    ///
    /// The pause is the last one that began at or before `start_ms`: the
    /// silence still pending where the second answer's audio is yet to come,
    /// a pause already among the kept chunks where it came before its
    /// transcript did (audio and events travel on two channels). Where no
    /// such pause is found what is kept stays whole: a greeting cut short is
    /// worse than a word too many.
    fn end_greeting(&mut self, start_ms: u64) {
        if self.closed {
            return;
        }
        self.closed = true;
        let rate = self.rate;
        let pending_is_the_pause = !self.pending.is_empty() && self.pending_at_ms <= start_ms;
        if !pending_is_the_pause {
            let mut cut = None;
            let mut after_speech = false;
            for (i, (_, spoken, at_ms)) in self.kept.iter().enumerate() {
                if after_speech && !*spoken && *at_ms <= start_ms {
                    cut = Some(i);
                }
                after_speech = *spoken;
            }
            if let Some(i) = cut {
                let after: Vec<(Vec<u8>, bool, u64)> = self.kept.drain(i..).collect();
                for (chunk, spoken, _) in after {
                    let ms = pcm_ms(chunk.len(), rate);
                    self.cut_ms += ms;
                    if spoken {
                        self.spoken_ms = self.spoken_ms.saturating_sub(ms);
                    }
                }
            }
        }
        let pending: Vec<Vec<u8>> = std::mem::take(&mut self.pending);
        for chunk in pending {
            self.cut_ms += pcm_ms(chunk.len(), rate);
        }
    }

    /// The adoption: what is kept goes to the caller, the silence the model
    /// streamed after its last word, until the pickup, does not.
    fn adopt(self) -> Parked {
        let rate = self.rate;
        let tail_ms: u64 = self.pending.iter().map(|c| pcm_ms(c.len(), rate)).sum();
        let kept_ms: u64 = self
            .kept
            .iter()
            .map(|(c, _, _)| pcm_ms(c.len(), rate))
            .sum();
        Parked {
            audio: self.kept.into_iter().map(|(c, _, _)| c).collect(),
            kept_ms,
            dropped_ms: self.dropped_ms,
            trimmed_ms: self.trimmed_ms,
            tail_ms,
            cut_ms: self.cut_ms,
        }
    }
}

/// Where the greeting of a parked model ends.
///
/// Nobody forms turns while a session is parked: its events reach the handler
/// only after the adoption. So the pump asks the turn machine's own rule for
/// the one verdict it needs. The model's first answer is its first BLOCK, and
/// a fragment of the model's that starts more than `turn_gap_ms` after the
/// block last spoke opens a second one
/// ([`crate::voice::live_turns::opens_new_block`], the rule the open turn
/// counts blocks with). Measured on the model's clock, with the gap the cell
/// forms its turns with; no value of its own.
struct ParkedGreeting {
    gap_ms: u64,
    /// The session's clock, kept the way `run_duplex` keeps the call's
    /// ([`stamp_model_clock`], [`clock_now_ms`]): the last stamp and when it
    /// arrived.
    clock: Option<(u64, Instant)>,
    opened: Instant,
    /// Where the model's running block last spoke, on its clock.
    block_end_ms: Option<u64>,
    /// The second block began.
    over: bool,
}

impl ParkedGreeting {
    fn new(gap_ms: u64) -> Self {
        Self {
            gap_ms,
            clock: None,
            opened: Instant::now(),
            block_end_ms: None,
            over: false,
        }
    }

    /// Where the session's clock stands now: what a chunk arriving now is
    /// filed under, like the audio `run_duplex` reports to the turn machine.
    fn now_ms(&self) -> u64 {
        clock_now_ms(self.clock, self.opened)
    }

    /// Read one event of the parked session. It comes back where it is kept
    /// for the handler; a fragment of what the model said after its greeting
    /// does not, because the caller never hears those words and the handler
    /// would take them for said.
    fn judge(&mut self, event: DuplexEvent, park: &mut ParkBuffer) -> Option<DuplexEvent> {
        match &event {
            DuplexEvent::Transcript { end_ms, .. } | DuplexEvent::Appended { end_ms, .. } => {
                stamp_model_clock(&mut self.clock, *end_ms);
            }
            DuplexEvent::DelegationCreated { offset_ms, .. } => {
                stamp_model_clock(&mut self.clock, *offset_ms);
            }
            _ => {}
        }
        let DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            start_ms,
            end_ms,
            ..
        } = &event
        else {
            return Some(event);
        };
        if self.over {
            return None;
        }
        if self.block_end_ms.is_some()
            && crate::voice::live_turns::opens_new_block(self.block_end_ms, *start_ms, self.gap_ms)
        {
            self.over = true;
            park.end_greeting(*start_ms);
            return None;
        }
        self.block_end_ms = Some(self.block_end_ms.map_or(*end_ms, |end| end.max(*end_ms)));
        Some(event)
    }
}

/// The length of one frame of the silence a parked session is fed.
const PARK_FEED_FRAME: Duration = Duration::from_millis(20);

/// Milliseconds of `bytes` of PCM16 mono at `rate`.
fn pcm_ms(bytes: usize, rate: u32) -> u64 {
    (bytes as u64 / 2) * 1_000 / u64::from(rate.max(1))
}

/// Where in a chunk of model audio the first audible window starts, in ms --
/// `None` while the first-audio mark is not armed or the chunk is silence
/// (BAT R1e, fix-round C-1): `t_first_audio_ms` is the first word, not the
/// first of the silent chunks a live model streams before it.
fn audible_offset_ms(armed: bool, chunk: &[u8], rate: u32, level: f64) -> Option<u64> {
    if !armed {
        return None;
    }
    crate::voice::duck::first_audible(chunk, rate, level).map(|at| pcm_ms(at, rate))
}

/// The pump between a parked session and the connection that will adopt it
/// (GH #1055 R1).
///
/// A live model streams a continuous channel, silence included, in 100 ms
/// chunks paced in real time (`meclaw-next/25-gpt-live/messungen`, S0 a1/a2),
/// and its adapter stops reading the socket while a channel is full
/// (`providers/gpt_live.rs`, the frame `held` for `audio_out`/`events`; until
/// d54g weblive it blocked outright). Parked with nobody reading, the 32-chunk
/// queue was full after 3.2 s of ringing and the provider loop stood still —
/// socket reads and idle deadline with it — and what was queued by then was the silence BEFORE the greeting, all of
/// it sent to the caller at the pickup and kept as a lag for the whole call.
///
/// So while parked this task reads both channels at once:
/// - audio: dropped until the first audible 20 ms (the gate's own test,
///   [`crate::voice::duck::first_audible`] at `barge_level_dbfs`); from there
///   on what the model says is kept, at most [`PARK_KEEP_MS`] of it (oldest
///   first out), with its own pauses, and the silence after its last word is
///   dropped at the adoption ([`ParkBuffer`]). Only the greeting: what the
///   model says after its first answer is dropped, audio and transcript
///   ([`ParkedGreeting`]);
/// - events: read before the audio, so a fragment is judged before the
///   audio that follows it, and kept (the TTL bounds them);
/// - the model's way in: fed 20 ms frames of digital silence, the line every
///   measured session heard from a quiet caller (S0a: continuous silence
///   frames, 640 B every 20 ms) — a model that paces its output on its
///   input then still talks into the ring.
///
/// At the adoption it stops feeding, sends what it kept at once and goes on
/// forwarding. What it sent ahead is a lag at the edge's playback buffer, so
/// that much of the model's following SILENCE is dropped (`catchup`); audible
/// chunks always pass. After the catch-up the pump is a plain relay with the
/// backpressure the run always had: it takes from the provider only while
/// fewer than [`AUDIO_QUEUE`] items wait for the connection.
#[allow(clippy::too_many_arguments)]
async fn park_pump(
    mut audio_from: mpsc::Receiver<Vec<u8>>,
    mut events_from: mpsc::Receiver<DuplexEvent>,
    audio_to: mpsc::Sender<Vec<u8>>,
    events_to: mpsc::Sender<DuplexEvent>,
    feed: mpsc::Sender<Vec<u8>>,
    mut adopt: oneshot::Receiver<()>,
    format: AudioFormat,
    level: f64,
    gap_ms: u64,
    session_id: String,
) {
    use std::collections::VecDeque;
    let rate = format.sample_rate;
    let silence = vec![0u8; pcm_ms_bytes(PARK_FEED_FRAME, rate)];
    let mut feed = Some(feed);
    let mut tick = tokio::time::interval(PARK_FEED_FRAME);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut adopted = false;
    // Holds the model's audio until the adoption, `audio` from there on.
    let mut park = Some(ParkBuffer::new(rate, level));
    let mut audio: VecDeque<Vec<u8>> = VecDeque::new();
    let mut events: VecDeque<DuplexEvent> = VecDeque::new();
    let mut catchup_ms: u64 = 0;
    let mut greeting = ParkedGreeting::new(gap_ms);
    let (mut audio_open, mut events_open) = (true, true);
    loop {
        if adopted && !audio_open && !events_open && audio.is_empty() && events.is_empty() {
            return;
        }
        tokio::select! {
            biased;

            signal = &mut adopt, if !adopted => match signal {
                Ok(()) => {
                    adopted = true;
                    // The connection's reader feeds the model from here on.
                    feed = None;
                    // What the model said before the pickup is judged as
                    // parked, also where its event still waits in the queue.
                    while let Ok(e) = events_from.try_recv() {
                        let kept = match park.as_mut() {
                            Some(p) => greeting.judge(e, p),
                            None => Some(e),
                        };
                        events.extend(kept);
                    }
                    if let Some(parked) = park.take().map(ParkBuffer::adopt) {
                        // Only what goes out ahead is a lag to catch up on;
                        // the tail was never sent.
                        catchup_ms = parked.kept_ms;
                        tracing::info!(
                            %session_id, kept_ms = parked.kept_ms,
                            dropped_ms = parked.dropped_ms, trimmed_ms = parked.trimmed_ms,
                            tail_ms = parked.tail_ms, cut_ms = parked.cut_ms,
                            events = events.len(),
                            "voice: the early session is adopted with what the model said"
                        );
                        audio = parked.audio;
                    }
                }
                // Dropped unadopted: the call ended, the deadline struck, or
                // the connection asked for another format.
                Err(_) => return,
            },

            permit = audio_to.reserve(), if adopted && !audio.is_empty() => match permit {
                Ok(p) => {
                    if let Some(c) = audio.pop_front() {
                        p.send(c);
                    }
                }
                Err(_) => return,
            },

            permit = events_to.reserve(), if adopted && !events.is_empty() => match permit {
                Ok(p) => {
                    if let Some(e) = events.pop_front() {
                        p.send(e);
                    }
                }
                Err(_) => return,
            },

            // Before the audio: a fragment sent ahead of its audio is read ahead
            // of it, so the greeting is over before a word after it is filed.
            e = events_from.recv(), if events_open && (!adopted || events.len() < AUDIO_QUEUE) => match e {
                None => events_open = false,
                Some(e) => {
                    let kept = match park.as_mut() {
                        Some(p) => greeting.judge(e, p),
                        None => Some(e),
                    };
                    events.extend(kept);
                }
            },

            c = audio_from.recv(), if audio_open && (!adopted || audio.len() < AUDIO_QUEUE) => match c {
                None => audio_open = false,
                Some(c) if !adopted => {
                    if let Some(p) = park.as_mut() {
                        p.push(c, greeting.now_ms());
                    }
                }
                Some(c) => {
                    if catchup_ms > 0
                        && crate::voice::duck::first_audible(&c, rate, level).is_none()
                    {
                        catchup_ms = catchup_ms.saturating_sub(pcm_ms(c.len(), rate));
                        continue;
                    }
                    audio.push_back(c);
                }
            },

            _ = tick.tick(), if feed.is_some() => {
                // Never waits: a model that does not read its input is not
                // made to, and the pump goes on reading what it says.
                if let Some(f) = feed.as_ref() {
                    let _ = f.try_send(silence.clone());
                }
            }
        }
    }
}

/// Bytes of `d` of PCM16 mono at `rate`.
fn pcm_ms_bytes(d: Duration, rate: u32) -> usize {
    (u64::from(rate) * d.as_millis() as u64 / 1_000) as usize * 2
}

/// Ask the colony for the seed of a session that is opening (D48), where the
/// cell waits for one (`seed_wait_ms`). The ask leaves the moment the session
/// is asked for — at the ring, at the media connection, at a renewal — so the
/// curator's answer and the provider's handshake run side by side.
/// `renewal_n` names the asking session (`0` = the call's first, review M1).
async fn ask_seed(
    shared: &Arc<VoiceIoShared>,
    session_id: &str,
    reason: SeedReason,
    renewal_n: u32,
) {
    if shared.duplex_seed_wait_ms == 0 {
        return;
    }
    shared
        .emit(VoiceEvent::SeedAsk {
            session_id: session_id.to_string(),
            reason,
            renewal_n,
            // Wave d54g, strand kprofil: every ask names the limits of the
            // model the session runs on, a renewal's as well as the first.
            limits: shared.duplex_limits.clone(),
        })
        .await;
}

/// Open the duplex session of a ringing call and park it for its media
/// connection (GH #1055).
///
/// Bounded by `duplex_prewarm_ttl_ms`: a ring nobody answers, and a media
/// connection that never comes, cost at most that much provider time. The
/// session is opened at the format the provider declares, which is the format
/// a telephony edge negotiates; a connection that asks for another one does not
/// adopt it and opens its own. A call that already has a session — parked by
/// an earlier ring (`180` then `183`) or held by a connection — opens nothing:
/// the check and the opening are one step under the registry's lock, so no
/// provider handshake is paid for a session that is thrown away (R5).
///
/// Returns whether this call opened and parked one (d54g weblive, review
/// M-3: a page counts past the name of an early session that ran out).
pub(crate) async fn prewarm(shared: &Arc<VoiceIoShared>, session_id: &str) -> bool {
    let Some(provider) = shared.duplex.clone() else {
        return false;
    };
    let ttl = shared.duplex_prewarm_ttl_ms;
    if ttl == 0 || session_id.is_empty() {
        return false;
    }
    let format = provider.format();
    let level = crate::voice::duck::linear(shared.duck.level_dbfs);
    let open = || {
        let mut run = DuplexRun::start(shared, &provider, format);
        let (audio_to, audio_rx) = mpsc::channel::<Vec<u8>>(AUDIO_QUEUE);
        let (events_to, events_rx) = mpsc::channel::<DuplexEvent>(AUDIO_QUEUE);
        let audio_from = std::mem::replace(&mut run.audio_out_rx, audio_rx);
        let events_from = std::mem::replace(&mut run.events_rx, events_rx);
        let (adopt, adopted) = oneshot::channel();
        tokio::spawn(park_pump(
            audio_from,
            events_from,
            audio_to,
            events_to,
            run.audio_tx.clone(),
            adopted,
            format,
            level,
            shared.duplex_turn_gap_ms,
            session_id.to_string(),
        ));
        Prewarmed { run, format, adopt }
    };
    let Some(token) = shared.park_prewarm(session_id, open).await else {
        tracing::debug!(%session_id, "voice: the call already has a session, no early one");
        return false;
    };
    tracing::info!(%session_id, ttl_ms = ttl, "voice: duplex session opened while the line rings");
    // The seed is asked for at the ring, not at the media (GH #1055 opened
    // the session early so the caller hears no handshake; the seed rides the
    // same head start).
    ask_seed(shared, session_id, SeedReason::Start, 0).await;
    let shared = Arc::clone(shared);
    let session_id = session_id.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(ttl)).await;
        if shared
            .take_prewarm(&session_id, Some(token))
            .await
            .is_some()
        {
            tracing::info!(
                %session_id, ttl_ms = ttl,
                "voice: an early duplex session nobody connected to is closed"
            );
        }
    });
    true
}

/// The call ended before its media connection came: close its early session
/// (GH #1055). An adopted one is the connection's and is not here any more.
pub(crate) async fn drop_prewarm(shared: &Arc<VoiceIoShared>, session_id: &str) {
    if shared.take_prewarm(session_id, None).await.is_some() {
        tracing::info!(%session_id, "voice: the call ended while ringing, its early session is closed");
    }
}

/// Move a duplex session's clock to `offset_ms` on the model's timeline.
///
/// The pair is "where the model said it was" and "when that reached us", and
/// the tick reads the sum. It only ever moves FORWARD: a stamp behind where the
/// clock already stands is a frame that took longer to arrive than the ones
/// before it, and a clock set back would hold every open turn open for the
/// difference (R-L7).
fn stamp_model_clock(clock: &mut Option<(u64, Instant)>, offset_ms: u64) {
    let now = Instant::now();
    let projected = clock.map(|(offset, stamped)| {
        offset.saturating_add(
            u64::try_from(now.duration_since(stamped).as_millis()).unwrap_or(u64::MAX),
        )
    });
    if projected.is_none_or(|standing| offset_ms >= standing) {
        *clock = Some((offset_ms, now));
    }
}

/// Where the call's clock stands now (R-L7): the last offset a session
/// stamped, plus the time since that stamp arrived — or, before the first
/// stamp, the time since the connection opened.
///
/// One computation for the two readers it has: the tick hands it to the turn
/// machine, and a renewal puts the new session's zero on it (GH #896), so the
/// new session's first word lands where the clock the turn machine was just
/// told stands.
fn clock_now_ms(model_clock: Option<(u64, Instant)>, opened: Instant) -> u64 {
    match model_clock {
        Some((offset_ms, stamped)) => offset_ms
            .saturating_add(u64::try_from(stamped.elapsed().as_millis()).unwrap_or(u64::MAX)),
        None => u64::try_from(opened.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

/// Move one event of a session onto the call's timeline (GH #896).
///
/// Every stamp a session carries is on ITS clock, which starts at zero when
/// the session does; `base_ms` is where that zero lies on the clock of the
/// call. The three stamped events move, everything else passes unchanged, and
/// a base of `0` — the first session of every call — changes nothing at all.
fn shift_event(event: DuplexEvent, base_ms: u64) -> DuplexEvent {
    if base_ms == 0 {
        return event;
    }
    match event {
        DuplexEvent::Transcript {
            speaker,
            delta,
            start_ms,
            end_ms,
        } => DuplexEvent::Transcript {
            speaker,
            delta,
            start_ms: start_ms.saturating_add(base_ms),
            end_ms: end_ms.saturating_add(base_ms),
        },
        DuplexEvent::Appended {
            kind,
            event_id,
            start_ms,
            end_ms,
        } => DuplexEvent::Appended {
            kind,
            event_id,
            start_ms: start_ms.saturating_add(base_ms),
            end_ms: end_ms.saturating_add(base_ms),
        },
        DuplexEvent::DelegationCreated {
            delegation_id,
            offset_ms,
        } => DuplexEvent::DelegationCreated {
            delegation_id,
            offset_ms: offset_ms.saturating_add(base_ms),
        },
        other => other,
    }
}

/// What one event of a session does to the connection itself, before it is
/// passed on: it moves the call's clock, and it tends the quiet clock of the
/// spoken section that is open.
///
/// One place for both sessions a renewal briefly runs side by side (GH #896):
/// the last words of the session that is retiring end a spoken section exactly
/// like the words of the one the call now runs on. The event has to be on the
/// call's timeline already ([`shift_event`]).
fn observe(
    event: &DuplexEvent,
    model_clock: &mut Option<(u64, Instant)>,
    spoken: &mut Option<Spoken>,
    quiet: Duration,
) {
    // Every event that carries a stamp moves the session's clock onto the
    // model's timeline. Only forward: a fragment whose `end_ms` lies behind
    // where the clock already stands is a late arrival, not time running
    // backwards, and setting the clock back would hold every open turn open
    // for the difference.
    match event {
        DuplexEvent::Transcript { end_ms, .. } | DuplexEvent::Appended { end_ms, .. } => {
            stamp_model_clock(model_clock, *end_ms);
        }
        DuplexEvent::DelegationCreated { offset_ms, .. } => {
            stamp_model_clock(model_clock, *offset_ms);
        }
        _ => {}
    }
    // The two events the SECTION is measured by, before the handler ever sees
    // them. The `appended` answer is what starts the quiet clock — the model
    // has taken the guidance up — and every assistant fragment after it
    // pushes the clock out again.
    match event {
        DuplexEvent::Appended {
            kind: AppendKind::Commentary,
            event_id,
            ..
        } => {
            if let Some(sp) = spoken.as_mut()
                && sp.speak_id == *event_id
            {
                sp.quiet_until = Some(Instant::now() + quiet);
            }
        }
        DuplexEvent::Transcript {
            speaker: Speaker::Assistant,
            ..
        } => {
            if let Some(sp) = spoken.as_mut()
                && sp.quiet_until.is_some()
            {
                sp.quiet_until = Some(Instant::now() + quiet);
            }
        }
        _ => {}
    }
}

/// The spoken section one append opened (OR-L19).
///
/// A duplex model sends no end of speech at all — it streams audio until it
/// stops — and the telephony hive downstream counts one `speak_end` down for
/// every `in_speak` it counted up. So the end is a heuristic, and it lives here
/// rather than in the handler because this half is the one with a clock.
struct Spoken {
    /// What the `speak_start` / `speak_end` pair carries. The same string as
    /// the `event_id` of the append that opened it, which is what lets the
    /// provider's own `appended` answer be recognised (contract § 1.4).
    speak_id: String,
    /// When the section is over for want of further assistant text. `None`
    /// until the model has confirmed the append: before that there is nothing
    /// to be quiet AFTER, and a section that started its quiet clock on the
    /// order would end before the model had said a word.
    quiet_until: Option<Instant>,
    /// The ceiling on the same heuristic. A model that never stops talking
    /// must not hold a hang-up open for the rest of the call.
    cap_until: Instant,
}

// ──────────────────────────────────────────────────────────────────────────
// Renewing the session inside the call (GH #896)
// ──────────────────────────────────────────────────────────────────────────

/// How many delegation handles of earlier sessions a call remembers. A call
/// carries none or one delegation, two on a bad day, and a renewal comes at a
/// provider's session limit, which is minutes rather than seconds: 64 is
/// headroom, and the bound keeps a call that runs for a week from growing the
/// list without end.
const EARLIER_DELEGATIONS_KEPT: usize = 64;

/// A renewal of the call's session that has been opened and is not yet the
/// session the call runs on.
///
/// It hears nothing of the caller until it takes over — the caller's audio
/// goes to the session in charge — and what it says before that is not passed
/// on, with two exceptions: its start, which is the `renewed` lane, and a
/// warning, which is the colony's to read. What it IS told before it takes
/// over is its handover block, the moment the block arrives.
struct Renewal {
    /// The new session.
    run: DuplexRun,
    /// Which renewal of this call it is, counting from 1.
    renewal_n: u32,
    /// Whether it has said `Started`. Only a started session takes over: a
    /// provider that has not opened its socket yet has nobody to hand the
    /// caller to.
    started: bool,
    /// Whether its audio channel is still open, for the reason the call's own
    /// flag exists: a closed receiver answers `None` at once, for ever.
    audio_open: bool,
    /// Whether its handover block reached it. A renewal that takes over
    /// without one takes the block, should it still come, as an ordinary
    /// append once it is in charge.
    briefed: bool,
    /// Whether its seed (D48, `seed_ask {reason: renew}`) has been answered.
    /// Until then, and only while it has not `started` either, an asked
    /// `renew` joins it -- the seed still to come carries the conversation as
    /// it stands; after the seed, or after a start without one (the session
    /// drops a later seed, `seed_late`), an asked `renew` is a new boundary
    /// no session knows (review M1, review kv-2 A-1).
    seeded: bool,
    /// Its entry in the call's usage ledger (D48, DK): it is billed from its
    /// first reading, whether or not it ever takes over.
    usage_slot: UsageSlot,
}

impl Renewal {
    /// Whether the provider behind it is still there to take a call over.
    ///
    /// A provider session that ended has dropped its way in and its control
    /// channel with it, and that shows on the senders at once — before its
    /// event channel has been read to the end, which is a wake later or, where
    /// something still holds a sender of it, never (review I-3: a takeover
    /// that trusted the event channel alone handed the caller to a session
    /// that was already gone).
    fn alive(&self) -> bool {
        !self.run.control_tx.is_closed() && !self.run.audio_tx.is_closed()
    }
}

/// What an opened renewal said.
enum RenewalTick {
    /// One of its events, or `None` where its event channel ended — the
    /// session is over before it ever took over.
    Event(Option<DuplexEvent>),
    /// A chunk of its audio, or `None` where its audio channel closed.
    Audio(Option<Vec<u8>>),
}

/// The session a renewal replaced, while it finishes.
///
/// Its ear is shut: the way in was dropped at the takeover, which is how a
/// provider is told that the conversation is over (contract § 1.1). What it
/// still says is its last sentence — transcripts, the answer to an append it
/// was given, its closing meter reading — and the connection keeps reading
/// until the verdict, or until `close_grace_ms` says it has waited long
/// enough.
struct Retiring {
    /// Everything it still says about itself.
    events_rx: mpsc::Receiver<DuplexEvent>,
    /// Whether that channel is still open.
    events_open: bool,
    /// Its verdict.
    done_rx: oneshot::Receiver<Result<(), DuplexError>>,
    /// Where its clock zero lies on the call's timeline.
    clock_base_ms: u64,
    /// The renewal that replaced it, for the lines it leaves in the log.
    renewal_n: u32,
    /// When the connection stops waiting for it.
    until: Instant,
    /// Its entry in the call's usage ledger (D48, DK).
    usage_slot: UsageSlot,
    /// A renewal that never took over (GH #1377, review M-2): it never spoke
    /// on the line, so only its usage and verdict count, never its words.
    silent: bool,
}

/// The audio the retiring session still produces, and the framer that cuts
/// it — its own, because a part-sample held back for ITS next chunk must not
/// lead a chunk of the other session.
struct RetiringAudio {
    /// The chunks.
    rx: mpsc::Receiver<Vec<u8>>,
    /// Cuts them into frames the client can swallow.
    framer: Framer,
}

/// What the retiring session said.
enum RetiringTick {
    /// One of its events.
    Event(DuplexEvent),
    /// Its event channel ended.
    EventsClosed,
    /// Its verdict.
    Verdict(Result<Result<(), DuplexError>, oneshot::error::RecvError>),
    /// `close_grace_ms` passed without one.
    Expired,
}

/// Which delegation handles the sessions of a call opened (GH #896).
///
/// A delegation is a handle inside ONE provider session. After a renewal the
/// new session has never seen the handles of the session it replaced, and an
/// append naming one would point the model at a question it never asked. So
/// the connection remembers which handles the EARLIER sessions of the call
/// opened, and an answer to one of those is appended without its handle: it
/// is said as an ordinary, late fact — which is what it is (the reading of
/// GH #728) — instead of being tied to something the new model does not know.
///
/// A handle no session of this call was seen opening passes unchanged, as it
/// always has: the connection strips only what it KNOWS belongs to a session
/// that is gone, and a call that is never renewed has no such handle at all.
#[derive(Default)]
struct Delegations {
    /// Opened by the session the call runs on now.
    current: Vec<String>,
    /// Opened by sessions this call has been renewed away from, oldest first.
    earlier: Vec<String>,
}

impl Delegations {
    /// The session the call runs on opened `id`.
    fn opened(&mut self, id: &str) {
        self.current.push(id.to_string());
    }

    /// A session this call was renewed away from opened `id` on its way out.
    fn opened_earlier(&mut self, id: &str) {
        self.earlier.push(id.to_string());
        self.bound();
    }

    /// The session the call runs on is being replaced: every handle it opened
    /// is an earlier session's from now on. Idempotent.
    fn renewed(&mut self) {
        self.earlier.append(&mut self.current);
        self.bound();
    }

    /// The handle an append to the session the call runs on may carry.
    fn admit(&self, id: Option<String>) -> Option<String> {
        id.filter(|id| !self.earlier.contains(id) || self.current.contains(id))
    }

    /// Keep [`EARLIER_DELEGATIONS_KEPT`] and drop the oldest.
    fn bound(&mut self) {
        let excess = self.earlier.len().saturating_sub(EARLIER_DELEGATIONS_KEPT);
        self.earlier.drain(..excess);
    }
}

/// The provider sessions of one duplex call (GH #896).
///
/// One for the whole call, where nothing renews it. Where something does, for
/// a while there are two: a renewal that has been opened and is not in charge
/// yet, or the session a renewal replaced, finishing its last sentence. All of
/// it lives in the connection task and nowhere else — no lock and no second
/// task: a session is a set of channels, and the connection's `select!` reads
/// them field by field.
struct Lineup {
    /// The session the call runs on. The caller's audio goes here, and so does
    /// every append.
    run: DuplexRun,
    /// Whether `run`'s audio channel is still open. Without it the arm that
    /// reads it would spin on a closed receiver: `recv` answers `None` at
    /// once, for ever.
    audio_open: bool,
    /// Cuts `run`'s chunks to what the client can swallow — a telephony edge
    /// aborts the call above about 100 ms (see the module note) — and to
    /// nothing else. No jitter buffer, no pacing (R-L4).
    framer: Framer,
    /// A renewal that has been opened and has not taken over.
    renewal: Option<Renewal>,
    /// The session the last renewal replaced, while it finishes.
    retiring: Option<Retiring>,
    /// Its audio, while there is any.
    retiring_audio: Option<RetiringAudio>,
    /// When the next renewal is opened. `None` while one is open, and always
    /// where renewals are off (`renew_after_ms: 0`).
    renew_at: Option<Instant>,
    /// When a started renewal that has heard no handover block is due to take
    /// over without one (`renew_grace_ms`).
    takeover_by: Option<Instant>,
    /// Since when the waiting renewal is due to take over: from its handover
    /// block, or from `takeover_by` passing without one. Due is not yet in
    /// charge — it takes over at the first quiet moment of the line, and at
    /// the latest `spoken_cap_ms` after this (review I-2).
    due_since: Option<Instant>,
    /// When the line last carried speech: a transcript fragment of either
    /// side, or a chunk of the model's audio. `None` before anything was
    /// said. A duplex model sends no end of speech and this connection runs no
    /// detector of its own, so a quiet moment is read off what the provider
    /// says: `spoken_quiet_ms` without any of it (review I-2).
    last_speech: Option<Instant>,
    /// The caller's open turn, as the handler last told the client: the words
    /// of it so far. `None` while no turn of the caller's is open.
    ///
    /// The provider sends no event for the beginning of the caller's speech
    /// (d54g callseam: its event list was checked against the recordings), so
    /// "the caller speaks" is read off the one place that forms turns. A pause
    /// inside a sentence is longer than `spoken_quiet_ms` and shorter than the
    /// gap that closes a turn, and a renewal that took over in it split the
    /// sentence between two sessions (measured 2026-11-07).
    caller_turn: Option<String>,
    /// The `renewal_n` of the session in charge; `0` for the first session of
    /// a call. A handover block that names it is the rest of a block split in
    /// parts, or one that came after a takeover without it, and is that
    /// session's to hear; a block that names neither it nor the renewal
    /// waiting is the answer to a renewal that failed, and is dropped rather
    /// than read to a model that was in the conversation all along (review
    /// M-8).
    in_charge_n: u32,
    /// The `renewal_n` of the last renewed session that started — and so of
    /// the last `renewed` lane. A renewal that fails before it starts gives its
    /// number to the next one; one that fails after has already been announced
    /// and keeps it, so no number is ever announced twice.
    announced: u32,
    /// Which delegation handles belong to which session.
    delegations: Delegations,
    /// An asked `renew` that came after the open renewal's seed was answered
    /// (review M1): the next renewal opens as soon as this one took over (or
    /// failed), with a seed asked for then.
    renew_again: bool,
    /// What the call used, one entry per provider session (D48, DK). Here
    /// because the connection is the one place that sees every session: the
    /// one in charge, the one retiring, a renewal before it takes over, and
    /// what is left of them when the call ends (review B-1).
    usage: CallUsage,
    /// The ledger entry of the session in charge (`run`).
    in_charge_slot: UsageSlot,
    /// Whether the session in charge took the place of one the provider
    /// ended mid-call (GH #1377): its seed, asked for under `in_charge_n`,
    /// reaches it. Cleared by the next takeover.
    recovered: bool,
    /// The `renewal_n` of a recovery that has not started yet: its start is
    /// the `renewed` lane, as a renewal's is (GH #896).
    recovering: Option<u32>,
}

impl Lineup {
    /// Tell the model's ear something: the session the call runs on and, where
    /// one is open, the renewal that will take over from it — a renewal must
    /// take over with the ear the call has THEN, not the one it had when the
    /// renewal was opened.
    async fn ear(&self, cmd: DuplexControl) {
        let _ = self.run.control_tx.send(cmd.clone()).await;
        if let Some(renewal) = self.renewal.as_ref() {
            let _ = renewal.run.control_tx.send(cmd).await;
        }
    }

    /// Make `renewal` the session the call runs on, and send the one it
    /// replaces into retirement.
    ///
    /// The new session's clock zero goes where the call's clock stands
    /// (`base_ms`), so the turn machine sees one clock across the change. The
    /// old session keeps its audio OUT — the model may finish its sentence,
    /// and the caller hears it — and loses its way IN: dropping the audio
    /// channel is how a provider is told the conversation is over, and it
    /// finalises. Nothing in here awaits.
    ///
    /// Returns the frames left over from a framer that has nothing more to
    /// cut, for the caller to send.
    fn take_over(
        &mut self,
        renewal: Renewal,
        base_ms: u64,
        fresh: Framer,
        close_grace: Duration,
        renew_after: Duration,
        session_id: &str,
    ) -> Vec<Vec<u8>> {
        let Renewal {
            mut run,
            renewal_n,
            audio_open,
            usage_slot,
            ..
        } = renewal;
        run.clock_base_ms = base_ms;
        let old_slot = std::mem::replace(&mut self.in_charge_slot, usage_slot);
        let old = std::mem::replace(&mut self.run, run);
        let old_audio_open = std::mem::replace(&mut self.audio_open, audio_open);
        let mut old_framer = std::mem::replace(&mut self.framer, fresh);
        let DuplexRun {
            audio_tx,
            audio_out_rx,
            events_rx,
            control_tx,
            done_rx,
            clock_base_ms,
        } = old;
        drop(audio_tx);
        drop(control_tx);
        let mut leftover = Vec::new();
        // Two renewals inside one `close_grace_ms` is a host that renews far
        // more often than any provider limit asks for. The older one has had
        // the grace it is going to get.
        if let Some(mut prev) = self.retiring.take() {
            // What it said on its way out is already in its channel: a close
            // that is there still counts as its number, else its last meter
            // reading stands in (D48, DK, review B-1 d). With `renew_again`
            // (review M1) a takeover right after the last one is the ordinary
            // course of a call renewed at every fact turn.
            while let Ok(event) = prev.events_rx.try_recv() {
                self.usage.observe(prev.usage_slot, &event);
            }
            tracing::info!(
                %session_id, renewal_n = prev.renewal_n,
                "voice: a retiring duplex session had not finished when the next renewal \
                 took over -- it is let go"
            );
        }
        if let Some(mut prev) = self.retiring_audio.take() {
            leftover.extend(prev.framer.flush());
        }
        self.retiring = Some(Retiring {
            events_rx,
            events_open: true,
            done_rx,
            clock_base_ms,
            renewal_n,
            until: Instant::now() + close_grace,
            usage_slot: old_slot,
            silent: false,
        });
        if old_audio_open {
            self.retiring_audio = Some(RetiringAudio {
                rx: audio_out_rx,
                framer: old_framer,
            });
        } else {
            leftover.extend(old_framer.flush());
        }
        self.delegations.renewed();
        self.takeover_by = None;
        self.due_since = None;
        self.in_charge_n = renewal_n;
        self.recovered = false;
        self.recovering = None;
        self.renew_at = self.next_renew_at(renew_after);
        leftover
    }

    /// Put `fresh` in charge of the call at once, in place of a session the
    /// provider ended mid-call (GH #1377).
    ///
    /// Not a takeover: there is nobody to wait for a quiet moment with, the
    /// session in charge is gone. A renewal that was opening was opened to
    /// take over from that session and asked its seed before the abort, so it
    /// retires (its close still counts, D48/DK) and the recovery opens on a
    /// seed asked for now. Nothing in here awaits.
    ///
    /// Returns the dead session's verdict and the frames of what it had
    /// already said, for the caller to send.
    #[allow(clippy::too_many_arguments)]
    fn recover(
        &mut self,
        mut fresh: DuplexRun,
        recovery_n: u32,
        base_ms: u64,
        framer: Framer,
        close_grace: Duration,
        renew_after: Duration,
        session_id: &str,
    ) -> (oneshot::Receiver<Result<(), DuplexError>>, Vec<Vec<u8>>) {
        fresh.clock_base_ms = base_ms;
        if let Some(opening) = self.renewal.take() {
            let Renewal {
                run,
                renewal_n,
                usage_slot,
                ..
            } = opening;
            let DuplexRun {
                audio_tx,
                control_tx,
                events_rx,
                done_rx,
                clock_base_ms,
                ..
            } = run;
            drop(audio_tx);
            drop(control_tx);
            if let Some(mut prev) = self.retiring.take() {
                while let Ok(event) = prev.events_rx.try_recv() {
                    self.usage.observe(prev.usage_slot, &event);
                }
            }
            tracing::info!(
                %session_id, renewal_n,
                "voice: a renewal was opening when the session in charge ended -- it retires"
            );
            self.retiring = Some(Retiring {
                events_rx,
                events_open: true,
                done_rx,
                clock_base_ms,
                renewal_n,
                until: Instant::now() + close_grace,
                usage_slot,
                silent: true,
            });
        }
        let mut dead = std::mem::replace(&mut self.run, fresh);
        self.in_charge_slot = self.usage.session();
        let mut old_framer = std::mem::replace(&mut self.framer, framer);
        let mut leftover = Vec::new();
        while let Ok(chunk) = dead.audio_out_rx.try_recv() {
            leftover.extend(old_framer.push(chunk));
        }
        leftover.extend(old_framer.flush());
        self.audio_open = true;
        self.delegations.renewed();
        self.takeover_by = None;
        self.due_since = None;
        self.in_charge_n = recovery_n;
        // Its number is taken now, not at its start: a renewal due before
        // that must not be given the same one.
        self.announced = recovery_n;
        self.recovered = true;
        self.recovering = Some(recovery_n);
        // A `renew` asked while the renewal was opening is answered by the
        // recovery's seed, which carries the call as it stands (review M-1).
        self.renew_again = false;
        self.renew_at = renew_deadline(renew_after);
        (dead.done_rx, leftover)
    }

    /// Something was said on the line (the field `last_speech`).
    fn heard(&mut self) {
        self.last_speech = Some(Instant::now());
    }

    /// Whether the line is quiet: no spoken section open, nothing said for
    /// `quiet`, and no audio of the model's waiting to be read -- audio comes
    /// last in the connection's `select!`, so a chunk can be here before the
    /// clock of the last one has run out.
    fn quiet(&self, spoken_open: bool, quiet: Duration) -> bool {
        !spoken_open
            && self.run.audio_out_rx.is_empty()
            && self.last_speech.is_none_or(|at| at.elapsed() >= quiet)
    }

    /// Read the caller's open turn off a frame on its way to the client: a
    /// `partial` carries the turn's words so far, a `turn` closes it. Both
    /// come from the handler in the order its turn machine decided them.
    fn caller_frame(&mut self, frame: &ServerFrame) {
        match frame {
            ServerFrame::Partial { text, eager: false } => self.caller_turn = Some(text.clone()),
            ServerFrame::Turn { .. } => self.caller_turn = None,
            _ => {}
        }
    }

    /// What a session that takes over inside the caller's turn is told first:
    /// the part of the sentence the call has heard so far, as something to
    /// know and not to say. Empty where no turn of the caller's is open or
    /// nothing of it was heard.
    ///
    /// Nothing of it is dropped: words past the ceiling of one append
    /// ([`APPEND_MAX_CHARS`]) travel in a further append, in order.
    fn heard_so_far(&self, session_id: &str, renewal_n: u32) -> Vec<DuplexControl> {
        let Some(text) = self.caller_turn.as_deref().filter(|t| !t.trim().is_empty()) else {
            return Vec::new();
        };
        append_parts(&format!("{HEARD_SO_FAR_LEAD}{text}"))
            .into_iter()
            .enumerate()
            .map(|(part, content)| DuplexControl::Append {
                kind: AppendKind::Thinking,
                event_id: format!("{session_id}-heard-{renewal_n}-{part}"),
                delegation_id: None,
                content,
            })
            .collect()
    }

    /// When the waiting renewal takes over: never before it is due; then at
    /// the first quiet moment (`quiet` after the last speech, no spoken
    /// section open, no turn of the caller's open); at the latest `cap` after
    /// it became due, so a model that never stops and a caller who never
    /// pauses still hand the call over. `None` where nothing is due.
    ///
    /// The cap is one number for both speakers (`spoken_cap_ms`): it is one
    /// moment -- "somebody is still speaking this long after the renewal was
    /// due" -- and the room a host leaves below the provider's limit is sized
    /// on it once (`renew_after_ms`).
    fn takeover_at(&self, spoken_open: bool, quiet: Duration, cap: Duration) -> Option<Instant> {
        let due = self.due_since?;
        if !self.renewal.as_ref().is_some_and(|r| r.started) {
            return None;
        }
        Some(self.quiet_at(spoken_open || self.caller_turn.is_some(), quiet, cap, due))
    }

    /// The first quiet moment after `since` (`quiet` after the last speech, no
    /// spoken section open), at the latest `since + cap`: the moment a due
    /// renewal takes over at, and an append `when_quiet` goes at (GH #1400).
    fn quiet_at(
        &self,
        spoken_open: bool,
        quiet: Duration,
        cap: Duration,
        since: Instant,
    ) -> Instant {
        let capped = since + cap;
        if spoken_open {
            return capped;
        }
        let calm = self.last_speech.map_or(since, |at| (at + quiet).max(since));
        calm.min(capped)
    }

    /// The waiting renewal is gone before it took over: the session in charge
    /// stays in charge, and the next attempt comes `renew_after` from now.
    fn renewal_gone(&mut self, renew_after: Duration) {
        self.renewal = None;
        self.takeover_by = None;
        self.due_since = None;
        self.renew_at = self.next_renew_at(renew_after);
    }

    /// When the next renewal opens once the open one has taken over or is
    /// gone: at once where an asked `renew` waits for it (`renew_again`),
    /// else `renew_after` from now.
    fn next_renew_at(&mut self, renew_after: Duration) -> Option<Instant> {
        if std::mem::take(&mut self.renew_again) {
            Some(Instant::now())
        } else {
            renew_deadline(renew_after)
        }
    }
}

/// What an asked `renew` (D48, `in_advise`) does to the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenewAsked {
    /// No renewal is opening: one opens now.
    OpenNow,
    /// The opening renewal's seed is still to come, it has not started, and
    /// the seed will carry the conversation as it stands: the ask joins it.
    Joins,
    /// The opening renewal's seed was answered, or it started without one,
    /// before this turn's facts: the next renewal opens as soon as it took
    /// over (review M1, review kv-2 A-1).
    AfterTakeover,
}

/// What [`renew_asked`] needs to know of the renewal that is opening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpeningRenewal {
    /// Whether its seed was answered.
    seeded: bool,
    /// Whether it said `Started`.
    started: bool,
}

/// Decide an asked `renew` from the renewal that is opening (`None` = none is
/// opening).
fn renew_asked(opening: Option<OpeningRenewal>) -> RenewAsked {
    match opening {
        None => RenewAsked::OpenNow,
        Some(r) if r.seeded || r.started => RenewAsked::AfterTakeover,
        Some(_) => RenewAsked::Joins,
    }
}

/// Tell the colony that a renewal ended before it took over (GH #896): its
/// verdict, on the warning lane and in this cell's log. The session in charge
/// is untouched; the caller hears nothing of it.
///
/// What it said before its verdict still counts for the call (D48, DK, review
/// re-check Re-1): three ways let a failed renewal go -- the end of its
/// events, a handover block that finds it dead, a takeover moment that finds
/// it gone -- and only the first had read its events to the end, so a
/// renewal's `Closed` (before a takeover usually its only number, GH #798)
/// was lost on the other two and the final said `complete: true`. Its events
/// are read here, once, after the verdict, when the provider has said all it
/// will. One that opened and never closed leaves the total open.
async fn report_failed_renewal(
    shared: &Arc<VoiceIoShared>,
    session_id: &str,
    failed: Renewal,
    usage: &mut CallUsage,
) {
    let renewal_n = failed.renewal_n;
    let slot = failed.usage_slot;
    let mut opened = failed.started;
    let DuplexRun {
        mut events_rx,
        done_rx,
        ..
    } = failed.run;
    // A renewal refused with the provider's own code (a spent quota on the
    // new session) carries it like a refused start does (review m1, GH #1376).
    let (why, provider_code) = match tokio::time::timeout(shared.external_timeout, done_rx).await {
        Ok(Ok(Ok(()))) => ("the provider ended it".to_string(), None),
        Ok(Ok(Err(e))) => (e.to_string(), e.provider_code().map(str::to_string)),
        Ok(Err(_)) | Err(_) => ("the renewal ended without a verdict".to_string(), None),
    };
    let mut moved = false;
    while let Ok(event) = events_rx.try_recv() {
        opened |= matches!(event, DuplexEvent::Started { .. });
        moved |= usage.observe(slot, &event);
    }
    if opened {
        moved |= usage.unclosed(slot);
    }
    if moved {
        report_usage(shared, session_id, usage, false).await;
    }
    tracing::warn!(
        %session_id, renewal_n, reason = %why,
        "voice: the renewed duplex session ended before it took over"
    );
    shared
        .emit(VoiceEvent::Live {
            session_id: session_id.to_string(),
            event: DuplexEvent::Warning {
                detail: format!("the renewed duplex session ended before it took over: {why}"),
                provider_code,
                client_event_id: None,
            },
        })
        .await;
}

/// When the next renewal of a call is due, counted from now; `None` where
/// renewals are off.
fn renew_deadline(after: Duration) -> Option<Instant> {
    (!after.is_zero()).then(|| Instant::now() + after)
}

/// Milliseconds since the Unix epoch, for the `renewed` lane. A clock set
/// before 1970 reads as `0` rather than failing a call over a timestamp.
fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// The next thing an opened renewal says, or never where none is open.
async fn next_renewal(renewal: &mut Option<Renewal>) -> RenewalTick {
    let Some(r) = renewal else {
        return std::future::pending().await;
    };
    tokio::select! {
        biased;
        event = r.run.events_rx.recv() => RenewalTick::Event(event),
        chunk = r.run.audio_out_rx.recv(), if r.audio_open => RenewalTick::Audio(chunk),
    }
}

/// The next thing the retiring session says, or never where none is retiring.
async fn next_retiring(retiring: &mut Option<Retiring>) -> RetiringTick {
    let Some(r) = retiring else {
        return std::future::pending().await;
    };
    tokio::select! {
        biased;
        event = r.events_rx.recv(), if r.events_open => match event {
            Some(event) => RetiringTick::Event(event),
            None => RetiringTick::EventsClosed,
        },
        verdict = &mut r.done_rx => RetiringTick::Verdict(verdict),
        () = tokio::time::sleep_until(r.until) => RetiringTick::Expired,
    }
}

/// The next chunk of the retiring session's audio, or never where there is
/// none.
async fn next_retiring_audio(audio: &mut Option<RetiringAudio>) -> Option<Vec<u8>> {
    match audio {
        None => std::future::pending().await,
        Some(a) => a.rx.recv().await,
    }
}

/// What the connection does with one event of the session a renewal replaced:
/// moved onto the call's timeline, then passed on or kept here (GH #896).
///
/// Its last words are passed on — a transcript, the answer to an append, a
/// delegation it still opened — because they belong in the turn the caller is
/// in. Its close is NOT: `Closed` would close that turn in the handler while
/// the call goes on, so it becomes a line in this cell's log; what the session
/// cost is counted in the call's usage ledger before this is asked (D48, DK).
/// The rest — its start, its meter, its ear, a warning about an item it no
/// longer carries — is nobody's business any more.
fn retiring_event(
    event: DuplexEvent,
    base_ms: u64,
    renewal_n: u32,
    session_id: &str,
    delegations: &mut Delegations,
) -> Option<DuplexEvent> {
    match shift_event(event, base_ms) {
        DuplexEvent::DelegationCreated {
            delegation_id,
            offset_ms,
        } => {
            delegations.opened_earlier(&delegation_id);
            Some(DuplexEvent::DelegationCreated {
                delegation_id,
                offset_ms,
            })
        }
        event @ (DuplexEvent::Transcript { .. } | DuplexEvent::Appended { .. }) => Some(event),
        DuplexEvent::Closed {
            reason,
            usage_seconds,
        } => {
            tracing::info!(
                %session_id, %reason, usage_seconds, renewal_n,
                "voice: the duplex session a renewal replaced is closed"
            );
            None
        }
        other => {
            tracing::debug!(
                %session_id, event = ?other, renewal_n,
                "voice: the retiring duplex session said something nobody needs any more"
            );
            None
        }
    }
}

/// Hand the handler the call's usage (D48, DK): the running total while the
/// call goes on, and once, at its end, the `final` one with whether every
/// session that counts closed ([`CallUsage::complete`]).
async fn report_usage(
    shared: &Arc<VoiceIoShared>,
    session_id: &str,
    usage: &CallUsage,
    last: bool,
) {
    shared
        .emit(VoiceEvent::CallUsage {
            session_id: session_id.to_string(),
            seconds: usage.total(),
            last,
            complete: usage.complete(),
        })
        .await;
}

/// Drive one duplex connection until it ends (contract § 1.3).
///
/// A separate function rather than a branch inside [`run_connection`]'s loop,
/// and deliberately so: the cascade loop above is byte-identical to what it was
/// before this path existed, and a reader can see that with one `git diff`.
/// What the two share they share as helpers — [`Framer`], [`send_frame`],
/// [`send_audio`], [`finish_speak`], [`sleep_until_opt`] — not as arms.
///
/// The three sentences this body is shaped by:
///
/// 1. **One client frame is one `append`** (R-L4). Nothing is collected,
///    nothing is paced, and the only buffer is the bounded channel.
/// 2. **Audio out comes last in the `select!`.** A model that bursts a
///    sentence must not keep winning the loop and hold a `cancel` off the
///    socket — the same reason synthesis chunks come last in the cascade.
/// 3. **There is no reconnect** (OR-L20). A session that ends on its own ends
///    the call; a fresh socket would be a fresh conversation with no memory of
///    this one.
///
/// **A renewal is not a reconnect** (GH #896). Where `renew_after_ms` is set,
/// the connection opens a SECOND session of the same call while the first is
/// still running — the same `call_id`, the same instructions, the caller's
/// audio still flowing into the first. The colony's handover block goes to the
/// second the moment it arrives, before it has heard anything of the caller;
/// the call itself is handed over only at a quiet moment of the line — the
/// model not speaking, the caller not in mid-sentence — and at the latest
/// `spoken_cap_ms` after the block (or after `renew_grace_ms` without one).
/// The first then loses its way in; what it still says reaches the caller
/// until the second speaks, never interleaved with it. A renewal that fails —
/// before its block, on it, or before the takeover — leaves the session in
/// charge running; the session in charge that ends on its own ends the call,
/// with or without a renewal waiting (OR-KY-G7).
#[allow(clippy::too_many_arguments)]
async fn run_duplex(
    link: ClientLink,
    shared: Arc<VoiceIoShared>,
    provider: Arc<dyn DuplexProvider>,
    session_id: String,
    mut mode: Mode,
    negotiated: Negotiated,
    conn_id: u64,
    to_conn_tx: mpsc::Sender<ToConnection>,
    mut to_conn_rx: mpsc::Receiver<ToConnection>,
) {
    let (mut sink, mut stream) = link.into_halves();
    // The connection's way in is the registry's, never its own: the sender
    // moves into `register`, and the delivery task it feeds is the one strong
    // holder. When that delivery gives up on the client (`client_too_slow`,
    // or `external_timeout`) it lets go, `to_conn_rx` ends, and so does the
    // call -- the end a phone always had (review I-1 of d54g weblive: a clone
    // kept here held the channel open, and the call ran on with the
    // provider's clock under a session the handler had already let go of).
    // A page's sessions get a way of their own each (see the loop below).
    let taken = register(&shared, &session_id, conn_id, to_conn_tx, mode).await;
    if !session_taken(&shared, &mut sink, &session_id, conn_id, taken).await {
        return;
    }

    // One format for both directions: what the model hears is what it says.
    let Negotiated {
        audio_in: input,
        audio_out: output,
    } = negotiated;
    let hello = ServerFrame::Hello {
        protocol: PROTOCOL,
        session_id: session_id.clone(),
        call_id: session_id.clone(),
        mode,
        audio_in: input,
        audio_out: output,
        stt: stt_name(&shared),
        tts: tts_name(&shared),
        audio_out_frame_ms: audio_out_frame_ms(&shared),
        speak_plain: shared.speak_plain,
        release_grace_ms: shared.release_grace_ms,
        duplex: true,
    };
    if send_frame(&mut sink, &hello).await.is_err() {
        unregister(&shared, &session_id, conn_id).await;
        return;
    }

    // d54g weblive, contract 4: a page has sessions as a phone has calls. In
    // `auto` -- the phone -- the connection IS the call and its one session
    // opens with the `hello`, as it always did. In `hold` -- the page -- the
    // provider session opens with the first hold (or a switch to `auto`), not
    // when the page hangs on: measured in a browser against the live model
    // (2026-11-13), the page joins when it loads, so the provider's clock ran
    // from the page view on. Each session
    // of a page carries a name of its own in the place of `call_id` (the
    // connection's name and its number, OR-54g-21 e), is reported like a call
    // (`seed_ask`, `usage` `final`), and ends after `hold_idle_ms` idle; the
    // page keeps its connection, and its next hold opens the next session.
    if mode != Mode::Hold {
        duplex_session(
            &mut sink,
            &mut stream,
            Arc::clone(&shared),
            Arc::clone(&provider),
            session_id.clone(),
            mode,
            input,
            output,
            &mut to_conn_rx,
            None,
            false,
        )
        .await;
        unregister(&shared, &session_id, conn_id).await;
        return;
    }
    let mut page_n: u32 = 0;
    loop {
        let wait = page_wait(
            &mut sink,
            &mut stream,
            &shared,
            &session_id,
            &mut page_n,
            &mut mode,
            input.frame_bytes(),
            &mut to_conn_rx,
        )
        .await;
        if wait == PageWait::Gone {
            break;
        }
        page_n = page_n.saturating_add(1);
        let next = page_session_name(&session_id, page_n);
        // The session's name is a session of this cell like a call's: the
        // handler holds its state from `Connected` to `Disconnected`, and an
        // answer the colony addresses to it reaches this connection.
        // A way of its own (OR-d54g.weblive.20): its delivery
        // is the one strong holder, so when that delivery gives up on the
        // page (`client_too_slow`, `external_timeout`) the session's way in
        // ends and the session with it, as a call does. On the connection's
        // way it never could: the registration of the page's own name holds
        // that sender for the whole connection, and the session ran on until
        // `hold_idle_ms` (`0`, never, by default) or a hang-up.
        let (session_tx, mut session_rx) = new_session_way();
        let taken = register(&shared, &next, conn_id, session_tx, mode).await;
        if !session_taken(&shared, &mut sink, &next, conn_id, taken).await {
            break;
        }
        let (gone, left_in) = duplex_session(
            &mut sink,
            &mut stream,
            Arc::clone(&shared),
            Arc::clone(&provider),
            next.clone(),
            mode,
            input,
            output,
            &mut session_rx,
            Some(&mut to_conn_rx),
            mode == Mode::Hold,
        )
        .await;
        mode = left_in;
        unregister(&shared, &next, conn_id).await;
        if gone {
            break;
        }
    }
    unregister(&shared, &session_id, conn_id).await;
}

/// The name of a page's `n`-th session (d54g weblive, OR-54g-21 e): the
/// connection's name and the session's number, counting from 1. One per
/// session, not per connection: a turn after a session's `usage` `final`
/// under that session's name would be dropped as late (d54g callseam).
fn page_session_name(connection: &str, n: u32) -> String {
    format!("{connection}-{n}")
}

/// What ended a page's wait for its next session.
#[derive(Debug, PartialEq, Eq)]
enum PageWait {
    /// The key went down (or the page switched to `auto`): open it.
    Open,
    /// The connection is gone, or was closed.
    Gone,
}

/// A page in `hold` with no session open (d54g weblive, contract 4).
///
/// Nothing reaches a provider here. A microphone frame goes nowhere -- the
/// key is the one door to the microphone (contract 2) -- and a misframed one
/// is counted like on the session's way. The page's early word (`ready`)
/// opens the next session ahead of the hold, the way a ringing line does
/// ([`prewarm`], `prewarm_ttl_ms`; nothing where that is `0`): measured on
/// the live model, a page's first hold waited 2 661 ms (median, n = 5) for
/// its answer against 1 696 ms on a warm session (measured in a browser
/// against the live model, 2026-11-13). An append or a synthesis order of
/// the handler for a session that is not there is answered as refused; the
/// other orders have nothing to act on and are dropped.
#[allow(clippy::too_many_arguments)]
async fn page_wait(
    sink: &mut LinkSink,
    stream: &mut LinkStream,
    shared: &Arc<VoiceIoShared>,
    session_id: &str,
    page_n: &mut u32,
    mode: &mut Mode,
    frame_bytes: usize,
    to_conn_rx: &mut mpsc::Receiver<ToConnection>,
) -> PageWait {
    let mut half_gone = shared.shutdown();
    let mut bad_frames: u32 = 0;
    // Whether this wait opened an early session for the page's next one.
    // One that ran out (`prewarm_ttl_ms`) was reported like a ring nobody
    // answered -- `seed_ask` under its name, no `usage` -- so its name is
    // spent: what opens next takes the number after it (review M-3 of d54g
    // weblive: the name came back with a second `seed_ask`).
    let mut warmed = false;
    loop {
        tokio::select! {
            biased;

            _ = half_is_gone(&mut half_gone) => {
                sink.close(1001, "").await;
                return PageWait::Gone;
            }

            cmd = to_conn_rx.recv() => {
                match cmd {
                    None => return PageWait::Gone,
                    Some(ToConnection::Close(code)) => {
                        sink.close(code, "").await;
                        return PageWait::Gone;
                    }
                    Some(ToConnection::Frame(frame)) => {
                        if send_frame(sink, &frame).await.is_err() {
                            return PageWait::Gone;
                        }
                    }
                    // An append has no model to reach while no session is
                    // open. Said on the handler's lanes, never dropped in
                    // silence (review M-2 of d54g weblive): the model's
                    // refusal of an append is the one report the handler
                    // already has for "the model never got it" --
                    // `advise_append_refused` on its error lane, and a
                    // delegation it answered falls back. This half does not
                    // hold the message, so a dead letter is not its to send.
                    Some(ToConnection::Advise { event_id, .. }) => {
                        shared.emit(VoiceEvent::Live {
                            session_id: session_id.to_string(),
                            event: DuplexEvent::Warning {
                                detail: "no session of the page is open: the append \
                                         never reached a model"
                                    .to_string(),
                                provider_code: None,
                                client_event_id: Some(event_id),
                            },
                        }).await;
                    }
                    // A wiring fault on this engine, answered as on a session's
                    // way: the handler counts on exactly one `SpeakEnded` per
                    // `Speak` it issued.
                    Some(ToConnection::Speak { speak_id, .. }) => {
                        shared.emit(VoiceEvent::SpeakEnded {
                            session_id: session_id.to_string(),
                            speak_id,
                            reason: SpeakEndReason::Failed,
                            detail: Some(
                                "no session of the page is open, and a duplex \
                                 connection speaks through the model"
                                    .to_string(),
                            ),
                        }).await;
                    }
                    // Nothing to cancel, renew or seed without a session.
                    Some(_) => tracing::info!(
                        %session_id,
                        "voice: an order for a page with no session open is dropped"
                    ),
                }
            }

            incoming = stream.next() => {
                let Some(message) = incoming else { return PageWait::Gone };
                match message {
                    Incoming::Binary(bytes) => {
                        if frame_bytes == 0 || bytes.len() % frame_bytes != 0 {
                            bad_frames = bad_frames.saturating_add(1);
                            shared.emit(VoiceEvent::BadAudioFrame {
                                session_id: session_id.to_string(),
                                len: bytes.len(),
                                count: bad_frames,
                            }).await;
                        }
                    }
                    Incoming::Text(text) => {
                        let frame = match duplex_text(sink, &text).await {
                            Err(()) => return PageWait::Gone,
                            Ok(None) => continue,
                            Ok(Some(frame)) => frame,
                        };
                        match frame {
                            ClientFrame::Hold if *mode == Mode::Hold => {
                                spend_expired_prewarm(shared, session_id, page_n, &mut warmed)
                                    .await;
                                return PageWait::Open;
                            }
                            ClientFrame::Hold | ClientFrame::Release => {
                                let refusal = ServerFrame::Error {
                                    code: WireErrorCode::WrongMode,
                                    detail: "hold and release are only frames in \
                                             `hold` mode".to_string(),
                                    bad_frames: None,
                                };
                                if *mode != Mode::Hold
                                    && send_frame(sink, &refusal).await.is_err()
                                {
                                    return PageWait::Gone;
                                }
                            }
                            ClientFrame::Ready => {
                                if *mode == Mode::Hold {
                                    spend_expired_prewarm(shared, session_id, page_n, &mut warmed)
                                        .await;
                                    let next =
                                        page_session_name(session_id, page_n.saturating_add(1));
                                    if prewarm(shared, &next).await {
                                        warmed = true;
                                    }
                                }
                            }
                            ClientFrame::Cancel => {}
                            ClientFrame::Mode { mode: wanted } => {
                                *mode = wanted;
                                if send_frame(sink, &ServerFrame::Mode { mode: wanted })
                                    .await
                                    .is_err()
                                {
                                    return PageWait::Gone;
                                }
                                // `auto` is an open ear by definition: the
                                // session is opened now.
                                if wanted == Mode::Auto {
                                    spend_expired_prewarm(shared, session_id, page_n, &mut warmed)
                                        .await;
                                    return PageWait::Open;
                                }
                            }
                        }
                    }
                    Incoming::Close => return PageWait::Gone,
                }
            }
        }
    }
}

/// Count past the page's next session where the early session this wait
/// opened for it ran out (review M-3 of d54g weblive): its name carried a
/// `seed_ask` and is not given to a second one.
async fn spend_expired_prewarm(
    shared: &Arc<VoiceIoShared>,
    session_id: &str,
    page_n: &mut u32,
    warmed: &mut bool,
) {
    if !*warmed {
        return;
    }
    let next = page_session_name(session_id, page_n.saturating_add(1));
    if !shared.prewarm_parked(&next).await {
        *page_n = page_n.saturating_add(1);
        *warmed = false;
    }
}

/// One provider session of a duplex connection, from its start to its
/// verdict and its `final` usage. Returns whether the connection is gone --
/// `false` only for a page whose session ended for being idle
/// (`hold_idle_ms`, d54g weblive), which keeps its connection for the next --
/// and the mode the client left it in: a switch inside the session holds for
/// the page's wait and its next session (review M-1 of d54g weblive: the mode
/// from before the session refused the next `hold` with `wrong_mode` after
/// the client had switched back and the cell had acknowledged it).
/// `ear_open`: the session was opened by the key going down, so the model's
/// ear opens with it.
/// `to_conn_rx` is the session's way in; `page_rx`, for a page's session, is
/// the page's connection way beside it. A page's session whose own way ends
/// ends alone and returns "not gone": the page keeps its connection.
#[allow(clippy::too_many_arguments)]
async fn duplex_session(
    sink: &mut LinkSink,
    stream: &mut LinkStream,
    shared: Arc<VoiceIoShared>,
    provider: Arc<dyn DuplexProvider>,
    session_id: String,
    mut mode: Mode,
    input: AudioFormat,
    output: Option<AudioFormat>,
    to_conn_rx: &mut mpsc::Receiver<ToConnection>,
    mut page_rx: Option<&mut mpsc::Receiver<ToConnection>>,
    ear_open: bool,
) -> (bool, Mode) {
    // Started with the `hello` and not with the first frame: a live model
    // greets before the caller speaks (`params.duplex.greeting`, OR-L25), and
    // a session opened on the first buffer would greet nobody. Unless the
    // line rang first (GH #1055): then the session was opened while it rang,
    // and this connection adopts it with whatever it has said so far.
    let mut usage = CallUsage::default();
    let early = shared
        .take_prewarm(&session_id, None)
        .await
        .map(|mut early| {
            let over = early.is_over();
            (early, over)
        });
    let adopted = matches!(&early, Some((e, false)) if e.format == input);
    let first = match early {
        Some((early, false)) if early.format == input => {
            tracing::info!(%session_id, "voice: the media connection adopts the early session");
            early.adopt()
        }
        Some((early, over)) => {
            // A run that is over is never adopted, whatever it kept: the
            // caller would be handed a session with no model behind it, and
            // the call would end on its first frame (M1).
            let same_format = early.format == input;
            tracing::warn!(
                %session_id, over, same_format,
                "voice: the early session is over or runs at another format, a fresh one is opened"
            );
            // It was opened while the line rang and billed, and its close goes
            // with the pump it is dropped with: the call's total says it may
            // be short (D48, DK, review re-check Re-k4).
            let early = usage.session();
            usage.unclosed(early);
            ask_seed(&shared, &session_id, SeedReason::Start, 0).await;
            DuplexRun::start(&shared, &provider, input)
        }
        None => {
            ask_seed(&shared, &session_id, SeedReason::Start, 0).await;
            DuplexRun::start(&shared, &provider, input)
        }
    };
    // Ducking the model while the caller talks over it (GH #1055); off
    // unless `params.duplex.barge_duck_ms` asks.
    let mut duck = crate::voice::duck::Duck::new(shared.duck, input.sample_rate);
    // The adopted session greeted into the ring: noise on the line just
    // picked up does not duck its greeting, only the caller's words do
    // (d54g, contract 5).
    if adopted {
        duck.greeting_runs(shared.duplex_turn_gap_ms);
    }
    // In `hold` the ear stays shut until the key goes down (OR-L24) --
    // unless the key going down opened this session (d54g weblive): then the
    // ear opens with it.
    if ear_open {
        let _ = first.control_tx.send(DuplexControl::Unmute).await;
    } else if mode == Mode::Hold {
        let _ = first.control_tx.send(DuplexControl::Mute).await;
    }
    // What the connection last told the model's ear, so a renewal opens with
    // the ear the call has NOW (GH #896): shut in `hold` until the key goes
    // down, open while it is held and in `auto`.
    let mut ear_shut = mode == Mode::Hold && !ear_open;
    // d54g weblive, contract 3: a release whose mute waits for the caller's
    // last frames. The model's socket reads its control before its audio
    // (`gpt_live::run_session`, `biased`), so a mute sent while frames of the
    // hold still queue in `audio_tx` reached the model first and cut the end
    // of the sentence off. The mute goes once the queue is empty.
    let mut mute_due = false;
    // d54g weblive, contract 2 and probe (d): a hold that came while the model
    // was speaking. Until the key comes up, the model's audio leaves as
    // silence of the same length. The live model has no event that cancels
    // its speech (its socket knows `mute`/`unmute` and the appends), and it
    // goes on for seconds after it is interrupted (S0: 4.4 to 11.0 s), so the
    // stop is the playback on this side -- the place the provider names for
    // it ("control playback at the client or media relay").
    let mut hushed = false;
    // Whether this stretch outside a hold has dropped a frame yet: one log
    // line per stretch, not one per frame.
    let mut dropped_outside = false;
    // d54g weblive, contract 4: a page's session ends after `hold_idle_ms`
    // in which nothing happened -- no hold, no audible speech of the model,
    // no delegation open, no renewal under way. `0` (shipped) never ends it
    // before the connection ends. `idle_since` is the last key event; the
    // model's last audible audio counts from `model_audio_at` below.
    let idle_after = Duration::from_millis(shared.duplex_hold_idle_ms);
    let mut idle_since = Instant::now();
    let mut open_delegations: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut idle_ended = false;
    // The delivery of a page's session gave up on the page (OR-d54g.weblive.20).
    let mut session_lost = false;
    // The renewal's three numbers (GH #896). `renew_after` counts from the
    // moment a session takes charge of the call, the first one included, and
    // `0` never renews: a call on the shipped params runs exactly as it did.
    let renew_after = Duration::from_millis(shared.duplex_renew_after_ms);
    let renew_grace = Duration::from_millis(shared.duplex_renew_grace_ms);
    let close_grace = Duration::from_millis(shared.close_grace_ms);
    let frame_ms = audio_out_frame_ms(&shared);
    let in_charge_slot = usage.session();
    let mut lineup = Lineup {
        run: first,
        audio_open: true,
        framer: Framer::new(output, frame_ms),
        renewal: None,
        retiring: None,
        retiring_audio: None,
        renew_at: renew_deadline(renew_after),
        takeover_by: None,
        due_since: None,
        last_speech: None,
        caller_turn: None,
        in_charge_n: 0,
        announced: 0,
        delegations: Delegations::default(),
        renew_again: false,
        usage,
        in_charge_slot,
        recovered: false,
        recovering: None,
    };

    let mut spoken: Option<Spoken> = None;
    let quiet = Duration::from_millis(shared.spoken_quiet_ms);
    let cap = Duration::from_millis(shared.spoken_cap_ms);
    // The session's own clock (R-L7, GH #798). `MissedTickBehavior::Delay`
    // because a tick that was late is not a tick that is owed: a scheduler
    // hiccup must not fire three deadlines in a row, and what this carries is
    // "time has passed", never a count.
    //
    // What it carries is the MODEL's timeline, not this task's. The two are not
    // the same clock: this one starts when `run_duplex` does, before the
    // handshake and before the provider has answered anything, and S0's
    // `a-pacing.json` measured it 626 to 750 ms ahead of the `offset_ms` the
    // model stamps -- one-sidedly, because the lead is the handshake. The turn
    // machine compares this number against the model's own `end_ms`, so on this
    // task's clock a shipped gap of 1 000 ms would have been a real wait of
    // 250 to 375 ms and turns would close on a caller who is still talking. So
    // the clock is anchored: the last offset the provider stamped, plus the
    // time since that stamp arrived. `opened` is what there is to say before
    // the first stamp, and nothing is open to be measured against it then.
    //
    // Across a renewal (GH #896) it stays ONE clock: the new session's stamps
    // are moved onto it ([`shift_event`]) rather than starting it again.
    let opened = Instant::now();
    let mut model_clock: Option<(u64, Instant)> = None;
    // Whether the next AUDIBLE audio sent to the client is the first since a
    // transcript fragment went to the handler (BAT R1e, C3): its time on
    // `model_clock` is what a turn's `t_first_audio_ms` is read from. One
    // `LiveAudioOut` per fragment at most, never one per frame. Audible, not
    // sent: a live model streams its silence too, in 100 ms chunks
    // ([`park_pump`], GH #798), so the first chunk after a fragment is mostly
    // silence one chunk later and the mark read the provider's cadence
    // (fix-round C-1). The level is the gate's and the park pump's.
    let mut audio_mark_armed = false;
    let audible_rate = input.sample_rate;
    let audible_level = crate::voice::duck::linear(shared.duck.level_dbfs);
    // When audible audio of the model last went to the client (d54g
    // callseam): a model whose audio went out inside `spoken_quiet_ms` of the
    // end of the connection was speaking when the line went, and the turn
    // that closes there says its sentence was cut. The same number that calls
    // the line quiet for a `speak_end` and for a takeover.
    let mut model_audio_at: Option<Instant> = None;
    let mut ticker = tokio::time::interval(Duration::from_millis(shared.duplex_tick_ms.max(1)));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The call's time marks (D48, DK): one timer, in this loop, so it ends
    // with the call -- a call that is over cannot fire a mark.
    let mut call_marks =
        crate::voice::call_marks::CallMarkClock::new(shared.duplex_call_marks.clone(), opened);
    let mut closing: Option<u16> = None;
    let mut half_gone = shared.shutdown();
    let mut bad_frames: u32 = 0;
    let frame_bytes = input.frame_bytes();
    // Whether the loop ended because the PROVIDER ended. It decides what the
    // verdict below means: a provider that gave up while somebody was on the
    // line is a `duplex_failed` and a `1011`, the same provider ending after
    // the caller hung up is the ordinary end of a call.
    let mut provider_ended = false;
    // GH #1377: how often this call may still replace a session the provider
    // ended mid-call (`params.duplex.recover_max`, `0` = never, the call ends
    // with the session as it always did), and the provider's word for the last
    // end of the session in charge -- the cause the `duplex_failed` names once
    // the attempts are spent. Measured on the phone (BAT R2): `reason=content`
    // 4 to 11 s after a sentence the model's moderation reads as a crisis,
    // with an ordinary verdict, so the caller talked on into a dead line.
    let recover_max = shared.duplex_recover_max;
    let mut recoveries: u32 = 0;
    let mut last_close: Option<String> = None;
    // GH #1400: the text that waits for the line's first quiet moment (the
    // curator's horizon, `when_quiet`) -- its cut (`split_id`), its parts
    // (channel, event id, words) in order, and since when the first part
    // waits. One text at a time: a newer one replaces it. The parts of ONE
    // text queue behind each other (d54g callseam): a text over
    // the append ceiling leaves the cell in parts, and with one waiting append
    // every part but the last was thrown away (strand deleg, finding
    // `quiet_held`).
    let mut quiet_held: Option<QuietHeld> = None;

    loop {
        // When a due renewal takes over (GH #896, review I-2), read before the
        // `select!` and not inside it: `takeover_at` borrows the whole lineup,
        // and the arms below hold `lineup.run.events_rx` and `lineup.retiring`
        // mutably while every arm is polled. The value is the same one the
        // macro would read, because building the arms' futures polls nothing.
        let takeover_at = lineup.takeover_at(spoken.is_some(), quiet, cap);
        let quiet_held_at = quiet_held
            .as_ref()
            .map(|h| lineup.quiet_at(spoken.is_some(), quiet, cap, h.2));
        let idle_at = (!idle_after.is_zero()
            && mode == Mode::Hold
            && ear_shut
            && !mute_due
            && spoken.is_none()
            && open_delegations.is_empty()
            && lineup.renewal.is_none()
            && lineup.retiring.is_none())
        .then(|| model_audio_at.map_or(idle_since, |at| at.max(idle_since)) + idle_after);
        tokio::select! {
            biased;

            // The cell is going away. Same code as the cascade path uses, so a
            // client sees one story whichever engine was behind it.
            _ = half_is_gone(&mut half_gone) => {
                closing = Some(1001);
                break;
            }

            // Commands first — a `cancel` must not queue behind a burst of the
            // model's own audio.
            way = next_command(to_conn_rx, &mut page_rx) => {
                let cmd = match way {
                    // A page's session whose delivery gave up: the session
                    // ends, the page keeps its connection.
                    Way::Session(None) if page_rx.is_some() => {
                        session_lost = true;
                        None
                    }
                    Way::Session(cmd) | Way::Connection(cmd) => cmd,
                };
                match cmd {
                    None => break,
                    Some(ToConnection::Close(code)) => {
                        closing = Some(code);
                        break;
                    }
                    Some(ToConnection::Frame(frame)) => {
                        // d54g callseam: the handler's word on the caller's
                        // turn, which a due renewal waits for.
                        lineup.caller_frame(&frame);
                        if send_frame(sink, &frame).await.is_err() {
                            break;
                        }
                    }
                    Some(ToConnection::CancelSpeak) => {
                        if let Some(sp) = spoken.take()
                            && finish_speak(
                                &shared, &session_id, sink, sp.speak_id,
                                SpeakEndReason::Cancelled, None,
                            ).await.is_err()
                        {
                            break;
                        }
                    }
                    Some(ToConnection::Advise {
                        kind, event_id, delegation_id, content, speak_id, takeover, when_quiet,
                        split_id,
                    }) => {
                        // d54g weblive: an answer to a delegation closes it
                        // for the idle end of a page's session.
                        if let Some(id) = &delegation_id {
                            open_delegations.remove(id);
                        }
                        // GH #1400: an append for the first quiet moment goes
                        // now where the line is quiet and otherwise waits for
                        // it -- the takeover's rule, which hears the model's
                        // own speech too (o8.6) -- replacing a text that waits.
                        // A further part of the waiting text (same
                        // `split_id`) queues behind it instead (d54g callseam).
                        if when_quiet {
                            let earlier = match quiet_held.take() {
                                Some((held, parts, since)) if held == split_id => {
                                    Some((parts, since))
                                }
                                Some(_) => {
                                    tracing::info!(
                                        %session_id,
                                        "voice: a newer text for a quiet moment replaces the waiting one"
                                    );
                                    None
                                }
                                None => None,
                            };
                            if !lineup.quiet(spoken.is_some(), quiet) {
                                let (mut parts, since) =
                                    earlier.unwrap_or_else(|| (Vec::new(), Instant::now()));
                                parts.push((kind, event_id, content));
                                quiet_held = Some((split_id, parts, since));
                                continue;
                            }
                            // Quiet now, while earlier parts of this text
                            // still wait: they go first, in their order.
                            for (kind, event_id, content) in
                                earlier.map(|(parts, _)| parts).unwrap_or_default()
                            {
                                let append = DuplexControl::Append {
                                    kind,
                                    event_id,
                                    delegation_id: None,
                                    content,
                                };
                                if lineup.run.control_tx.send(append).await.is_err() {
                                    tracing::debug!(
                                        %session_id,
                                        "voice: the duplex session is gone, the append was dropped"
                                    );
                                }
                            }
                        } else if (speak_id.is_some() || delegation_id.is_some())
                            && quiet_held.take().is_some()
                        {
                            // GH #1404 (V3 review, N1): the answer goes out -- a
                            // new spoken section, or an append that answers a
                            // delegation -- while a horizon waits: said after
                            // it, the horizon would announce what the caller
                            // has already heard. It lapses.
                            tracing::info!(
                                %session_id,
                                "voice: the answer goes out -- the waiting horizon lapses"
                            );
                        }
                        if let Some(id) = speak_id {
                            // One spoken section at a time, exactly as one
                            // synthesis at a time in the cascade: an
                            // overlapping order can only mean the handler
                            // moved on, so the open one is closed and
                            // reported rather than left to fight for the
                            // socket.
                            if let Some(prev) = spoken.take()
                                && finish_speak(
                                    &shared, &session_id, sink, prev.speak_id,
                                    SpeakEndReason::Done, None,
                                ).await.is_err()
                            {
                                break;
                            }
                            let frame = ServerFrame::SpeakStart {
                                speak_id: id.clone(),
                            };
                            if send_frame(sink, &frame).await.is_err() {
                                break;
                            }
                            spoken = Some(Spoken {
                                speak_id: id,
                                quiet_until: None,
                                cap_until: Instant::now() + cap,
                            });
                        }
                        // THE HANDOVER (GH #896). An append marked as one goes
                        // to a renewal that has started and is not in charge
                        // yet, AT ONCE: it has heard nothing of the caller, so
                        // the block is the first thing it hears. The CALL is
                        // handed over only where the line is quiet — here, if
                        // it already is, in the same arm, so no frame of the
                        // caller's gets in between; otherwise at the first
                        // quiet moment, the arm below (review I-2: a takeover
                        // mid-utterance split the caller's sentence between
                        // two sessions and put two models' audio on one
                        // line). A renewal that is gone takes nothing: the
                        // session in charge stays (review I-3).
                        let waiting = lineup
                            .renewal
                            .as_ref()
                            .filter(|r| r.started)
                            .map(|r| r.renewal_n);
                        if takeover.is_some() && takeover == waiting {
                            let briefing = DuplexControl::Append {
                                kind,
                                event_id,
                                // A handle is a question inside ONE session,
                                // and the renewal asked none.
                                delegation_id: None,
                                content,
                            };
                            let to_renewal = lineup
                                .renewal
                                .as_ref()
                                .filter(|r| r.alive())
                                .map(|r| r.run.control_tx.clone());
                            let sent = match to_renewal {
                                Some(tx) => tx.send(briefing).await.is_ok(),
                                None => false,
                            };
                            if !sent {
                                if let Some(failed) = lineup.renewal.take() {
                                    report_failed_renewal(&shared, &session_id, failed, &mut lineup.usage).await;
                                }
                                lineup.renewal_gone(renew_after);
                                continue;
                            }
                            if let Some(r) = lineup.renewal.as_mut()
                                && !r.briefed
                            {
                                r.briefed = true;
                                lineup.takeover_by = None;
                                lineup.due_since = Some(Instant::now());
                                tracing::info!(
                                    %session_id, renewal_n = r.renewal_n,
                                    "voice: the renewed duplex session heard its handover"
                                );
                            }
                            // d54g callseam: nor into the caller's open
                            // turn, however long the pause inside it.
                            if !lineup.quiet(spoken.is_some(), quiet)
                                || lineup.caller_turn.is_some()
                            {
                                continue;
                            }
                            let Some(renewal) = lineup.renewal.take() else { continue };
                            let renewal_n = renewal.renewal_n;
                            let base = clock_now_ms(model_clock, opened);
                            let leftover = lineup.take_over(
                                renewal,
                                base,
                                Framer::new(output, frame_ms),
                                close_grace,
                                renew_after,
                                &session_id,
                            );
                            tracing::info!(
                                %session_id, renewal_n, clock_ms = base,
                                "voice: the renewed duplex session took over with its handover, \
                                 on a quiet line"
                            );
                            if send_audio(sink, leftover).await.is_err() {
                                break;
                            }
                            continue;
                        }
                        if let Some(n) = takeover
                            && n != lineup.in_charge_n
                        {
                            // A block for a renewal that is neither waiting
                            // nor in charge: it failed. The session in charge
                            // was in the conversation all along, and a summary
                            // of it read back to it is noise (review M-8).
                            tracing::debug!(
                                %session_id, renewal_n = n,
                                "voice: a handover block for a renewal that is gone -- dropped"
                            );
                            continue;
                        }
                        // Everything else — the rest of a split block for the
                        // session that took over with its first part, a block
                        // for one that took over at its grace without it —
                        // goes to the session in charge. An answer to a
                        // delegation an EARLIER session of this call opened
                        // goes without its handle: the model it reaches never
                        // saw that handle, and the answer is said as a late
                        // fact rather than tied to a question nobody on this
                        // session asked (see [`Delegations`]).
                        let append = DuplexControl::Append {
                            kind,
                            event_id,
                            delegation_id: lineup.delegations.admit(delegation_id),
                            content,
                        };
                        if lineup.run.control_tx.send(append).await.is_err() {
                            tracing::debug!(
                                %session_id,
                                "voice: the duplex session is gone, the append was dropped"
                            );
                        }
                    }
                    // The seed of a session that is opening (D48), to the
                    // session that asked for it and to no other (review M1):
                    // `renewal_n` 0 is the call's first session while it is
                    // still in charge, any other number the open renewal of
                    // that number (a session that has started drops it
                    // itself, with a warning). A late answer -- to a session
                    // that took over or was replaced -- is dropped loud: opening a newer session on it would open it with
                    // an older conversation than its own answer carries.
                    // A renewal asked for by the colony (D48, `in_advise`
                    // `renew`): due now, on the same arm a due
                    // `renew_after_ms` takes. A renewal whose seed is still
                    // to come and which has not started absorbs it -- that
                    // seed carries the conversation as it stands. One whose
                    // seed was answered, or which started without it (and so
                    // drops a later seed), does not: the asked renewal opens
                    // right after it took over, or else a fact of the turn in
                    // between never reaches a session (review M1, kv-2 A-1).
                    // The session in charge
                    // keeps the call (and its open delegations) until the
                    // handover.
                    Some(ToConnection::Renew) => {
                        if shared.duplex_seed_wait_ms == 0 {
                            tracing::warn!(
                                %session_id,
                                "voice: a renewal was asked for without `seed_wait_ms` -- the \
                                 new session opens on its instructions alone (review K3)"
                            );
                        }
                        let opening = lineup.renewal.as_ref().map(|r| {
                            (
                                r.renewal_n,
                                OpeningRenewal {
                                    seeded: r.seeded,
                                    started: r.started,
                                },
                            )
                        });
                        match renew_asked(opening.map(|(_, o)| o)) {
                            RenewAsked::OpenNow => lineup.renew_at = Some(Instant::now()),
                            RenewAsked::AfterTakeover => {
                                lineup.renew_again = true;
                                tracing::debug!(
                                    %session_id, renewal_n = ?opening.map(|o| o.0),
                                    "voice: a renewal was asked for after the opening one's \
                                     seed -- the next one opens after its takeover"
                                );
                            }
                            RenewAsked::Joins => tracing::debug!(
                                %session_id, renewal_n = ?opening.map(|o| o.0),
                                "voice: a renewal is opening, its seed still to come -- the \
                                 asked one joins it"
                            ),
                        }
                    }
                    Some(ToConnection::Seed { renewal_n, messages }) => {
                        let target = match lineup.renewal.as_mut() {
                            Some(r) if r.renewal_n == renewal_n => {
                                r.seeded = true;
                                Some(r.run.control_tx.clone())
                            }
                            // The call's first session while it leads, or a
                            // recovery that took its place (GH #1377).
                            _ if renewal_n == lineup.in_charge_n
                                && (renewal_n == 0 || lineup.recovered) =>
                            {
                                Some(lineup.run.control_tx.clone())
                            }
                            _ => None,
                        };
                        match target {
                            Some(target) => {
                                if target.send(DuplexControl::Seed { messages }).await.is_err() {
                                    tracing::debug!(
                                        %session_id,
                                        "voice: the duplex session is gone, the seed was dropped"
                                    );
                                }
                            }
                            None => tracing::warn!(
                                %session_id, renewal_n,
                                in_charge_n = lineup.in_charge_n,
                                open_renewal = ?lineup.renewal.as_ref().map(|r| r.renewal_n),
                                "voice: a seed for a session that is not opening, dropped"
                            ),
                        }
                    }
                    // A synthesis order on a duplex connection is a wiring
                    // fault: the handler sends `Advise` on this engine
                    // (OR-L6). Loud, and answered — the handler counts on
                    // exactly one `SpeakEnded` per `Speak` it issued, and a
                    // hive that holds a hang-up until the sentence is over
                    // waits for ever otherwise.
                    Some(ToConnection::Speak { speak_id, .. }) => {
                        tracing::error!(
                            %session_id, %speak_id,
                            "voice: a synthesis order reached a duplex connection, which \
                             has no synthesiser"
                        );
                        shared.emit(VoiceEvent::SpeakEnded {
                            session_id: session_id.clone(),
                            speak_id,
                            reason: SpeakEndReason::Failed,
                            detail: Some(
                                "a duplex connection speaks through the model, not \
                                 through a synthesiser".to_string(),
                            ),
                        }).await;
                    }
                }
            }

            // The model has been quiet since its last fragment for as long as
            // this section was allowed to be (OR-L19).
            () = sleep_until_opt(spoken.as_ref().and_then(|s| s.quiet_until)) => {
                if let Some(sp) = spoken.take()
                    && finish_speak(
                        &shared, &session_id, sink, sp.speak_id,
                        SpeakEndReason::Done, None,
                    ).await.is_err()
                {
                    break;
                }
            }

            // And the ceiling on the same heuristic: a model that never stops
            // must not hold a waiting hang-up open for the rest of the call.
            () = sleep_until_opt(spoken.as_ref().map(|s| s.cap_until)) => {
                if let Some(sp) = spoken.take()
                    && finish_speak(
                        &shared, &session_id, sink, sp.speak_id,
                        SpeakEndReason::Done, None,
                    ).await.is_err()
                {
                    break;
                }
            }

            event = lineup.run.events_rx.recv() => {
                match event {
                    Some(event) => {
                        // An event is a completed provider round trip (issue #7).
                        shared.liveness.mark_success();
                        let event = shift_event(event, lineup.run.clock_base_ms);
                        // A meter reading moves the call's running total; a
                        // close is counted and goes out in the `final` (D48).
                        let metered = lineup.usage.observe(lineup.in_charge_slot, &event)
                            && matches!(event, DuplexEvent::Usage { .. });
                        // GH #1377: a recovery's start is the `renewed` lane,
                        // as a renewal's is -- the colony's handover answers it.
                        if matches!(event, DuplexEvent::Started { .. })
                            && let Some(renewal_n) = lineup.recovering.take()
                        {
                            tracing::info!(
                                %session_id, renewal_n,
                                "voice: the recovered duplex session started"
                            );
                            shared.emit(VoiceEvent::Renewed {
                                session_id: session_id.clone(),
                                renewal_n,
                                renewed_at_ms: epoch_ms(),
                            }).await;
                            continue;
                        }
                        // Inside this loop nothing of the connection's own ends
                        // the session in charge -- a takeover moves it out of
                        // `run` first, a hang-up leaves the loop -- so its close
                        // is the provider's. While an attempt is left it is no
                        // end of the call and stays out of the handler, as a
                        // retiring session's close does; it is counted above.
                        if let DuplexEvent::Closed { reason, .. } = &event {
                            last_close = Some(reason.clone());
                            if recoveries < recover_max {
                                tracing::info!(
                                    %session_id, %reason,
                                    "voice: the provider closed the duplex session in charge \
                                     mid-call"
                                );
                                continue;
                            }
                        }
                        if let DuplexEvent::DelegationCreated { delegation_id, .. } = &event {
                            lineup.delegations.opened(delegation_id);
                            open_delegations.insert(delegation_id.clone());
                        }
                        if matches!(event, DuplexEvent::Transcript { .. }) {
                            lineup.heard();
                            audio_mark_armed = true;
                        }
                        duck.transcript(&event);
                        observe(&event, &mut model_clock, &mut spoken, quiet);
                        shared.emit(VoiceEvent::Live {
                            session_id: session_id.clone(),
                            event,
                        }).await;
                        if metered {
                            report_usage(&shared, &session_id, &lineup.usage, false).await;
                        }
                    }
                    // The provider is done, one way or another. Its verdict is
                    // waiting below; there is no retry and no reconnect
                    // (OR-L20) -- and a renewal that may be waiting does not
                    // step in either (OR-KY-G7): it was opened to take over
                    // from a session that is RUNNING, and the one that just
                    // ended took the conversation with it.
                    //
                    // GH #1377: unless `recover_max` asks otherwise. Then a
                    // fresh session of the same call takes the dead one's
                    // place at once -- a renewal (GH #896) that asks for its
                    // seed with `reason: recover`, so it opens on the
                    // conversation as it stands -- at most `recover_max`
                    // times; the end after that is the call's.
                    None if recoveries < recover_max => {
                        recoveries += 1;
                        let recovery_n = lineup
                            .announced
                            .max(lineup.in_charge_n)
                            .max(lineup.renewal.as_ref().map_or(0, |r| r.renewal_n))
                            .saturating_add(1);
                        ask_seed(&shared, &session_id, SeedReason::Recover, recovery_n).await;
                        let run = DuplexRun::start_renewed(&shared, &provider, input);
                        if ear_shut {
                            let _ = run.control_tx.send(DuplexControl::Mute).await;
                        }
                        let base = clock_now_ms(model_clock, opened);
                        let (done_rx, leftover) = lineup.recover(
                            run,
                            recovery_n,
                            base,
                            Framer::new(output, frame_ms),
                            close_grace,
                            renew_after,
                            &session_id,
                        );
                        // Its events are over, so the provider's task is too:
                        // the verdict is there or a moment away. Bounded all
                        // the same (rule 12).
                        let cause = match tokio::time::timeout(close_grace, done_rx).await {
                            Ok(Ok(Ok(()))) => last_close
                                .take()
                                .unwrap_or_else(|| "closed".to_string()),
                            Ok(Ok(Err(e))) => e.to_string(),
                            Ok(Err(_)) | Err(_) => {
                                "the duplex task ended without a verdict".to_string()
                            }
                        };
                        tracing::warn!(
                            %session_id, %cause, renewal_n = recovery_n,
                            attempt = recoveries, recover_max,
                            "voice: the provider ended the duplex session mid-call -- \
                             recovering with a fresh session"
                        );
                        if send_audio(sink, leftover).await.is_err() {
                            break;
                        }
                    }
                    None => {
                        provider_ended = true;
                        break;
                    }
                }
            }

            // The last words of the session a renewal replaced (GH #896).
            // Directly below the events of the session in charge and above the
            // clock, for the reason the clock sits below those: a fragment
            // that is already waiting belongs in the turn, and a tick that won
            // first would close the turn for a silence that had already ended.
            tick = next_retiring(&mut lineup.retiring) => {
                match tick {
                    RetiringTick::Event(event) => {
                        shared.liveness.mark_success();
                        let (base, renewal_n, slot) = lineup
                            .retiring
                            .as_ref()
                            .map_or((0, 0, None), |r| {
                                (r.clock_base_ms, r.renewal_n, Some(r.usage_slot))
                            });
                        let silent = lineup.retiring.as_ref().is_some_and(|r| r.silent);
                        if let Some(slot) = slot
                            && lineup.usage.observe(slot, &event)
                        {
                            report_usage(&shared, &session_id, &lineup.usage, false).await;
                        }
                        if !silent && let Some(event) = retiring_event(
                            event, base, renewal_n, &session_id, &mut lineup.delegations,
                        ) {
                            if matches!(event, DuplexEvent::Transcript { .. }) {
                                lineup.heard();
                                audio_mark_armed = true;
                            }
                            observe(&event, &mut model_clock, &mut spoken, quiet);
                            shared.emit(VoiceEvent::Live {
                                session_id: session_id.clone(),
                                event,
                            }).await;
                        }
                    }
                    RetiringTick::EventsClosed => {
                        if let Some(r) = lineup.retiring.as_mut() {
                            r.events_open = false;
                        }
                    }
                    RetiringTick::Verdict(verdict) => {
                        if let Some(mut r) = lineup.retiring.take() {
                            // What it said on its way out is already in the
                            // channel, and it is read before the session is
                            // let go.
                            while let Ok(event) = r.events_rx.try_recv() {
                                if lineup.usage.observe(r.usage_slot, &event) {
                                    report_usage(&shared, &session_id, &lineup.usage, false)
                                        .await;
                                }
                                if !r.silent && let Some(event) = retiring_event(
                                    event, r.clock_base_ms, r.renewal_n, &session_id,
                                    &mut lineup.delegations,
                                ) {
                                    observe(&event, &mut model_clock, &mut spoken, quiet);
                                    shared.emit(VoiceEvent::Live {
                                        session_id: session_id.clone(),
                                        event,
                                    }).await;
                                }
                            }
                            match verdict {
                                Ok(Ok(())) => tracing::info!(
                                    %session_id, renewal_n = r.renewal_n,
                                    "voice: the duplex session a renewal replaced has ended"
                                ),
                                Ok(Err(e)) => tracing::warn!(
                                    %session_id, renewal_n = r.renewal_n, error = %e,
                                    "voice: the duplex session a renewal replaced ended badly"
                                ),
                                Err(_) => tracing::warn!(
                                    %session_id, renewal_n = r.renewal_n,
                                    "voice: the duplex session a renewal replaced ended \
                                     without a verdict"
                                ),
                            }
                        }
                        // And so is its audio: after the verdict nothing more
                        // is coming, and what did come is the model finishing
                        // its sentence.
                        if let Some(mut a) = lineup.retiring_audio.take() {
                            let mut frames = Vec::new();
                            while let Ok(chunk) = a.rx.try_recv() {
                                frames.extend(a.framer.push(chunk));
                            }
                            frames.extend(a.framer.flush());
                            if send_audio(sink, frames).await.is_err() {
                                break;
                            }
                        }
                    }
                    RetiringTick::Expired => {
                        if let Some(r) = lineup.retiring.take() {
                            tracing::warn!(
                                %session_id, renewal_n = r.renewal_n,
                                grace_ms = shared.close_grace_ms,
                                "voice: the duplex session a renewal replaced did not finish \
                                 inside close_grace_ms -- it is let go"
                            );
                        }
                        if let Some(mut a) = lineup.retiring_audio.take()
                            && send_audio(sink, a.framer.flush().into_iter().collect())
                                .await
                                .is_err()
                        {
                            break;
                        }
                    }
                }
            }

            // The quiet moment a due renewal waits for (GH #896, review I-2),
            // or the cap on it. Above the client and the clock: once the line
            // is quiet the next frame of the caller's belongs to the new
            // session, and it must not slip to the old one because it was
            // polled first. Below the events of both sessions, which move the
            // moment out while anybody still speaks.
            () = sleep_until_opt(takeover_at) => {
                match lineup.renewal.take() {
                    Some(renewal) if renewal.started && renewal.alive() => {
                        let calm = lineup.quiet(spoken.is_some(), quiet)
                            && lineup.caller_turn.is_none();
                        let capped = lineup.due_since.is_some_and(|due| due.elapsed() >= cap);
                        if !calm && !capped {
                            // The model's next words are already waiting: it
                            // is still speaking, and the moment moves out.
                            lineup.renewal = Some(renewal);
                            lineup.heard();
                            continue;
                        }
                        let renewal_n = renewal.renewal_n;
                        // d54g callseam: at the cap the call changes hands
                        // inside the caller's sentence. The new session is
                        // told the part already heard BEFORE the caller's
                        // audio reaches it -- its control channel is read
                        // ahead of the takeover, as the handover block is --
                        // so it does not start in the middle of a sentence it
                        // knows nothing of. The turn itself stays one turn:
                        // the handler's clock runs across the takeover.
                        let heard = lineup.heard_so_far(&session_id, renewal_n);
                        let handed_over = !heard.is_empty();
                        for part in heard {
                            if renewal.run.control_tx.send(part).await.is_err() {
                                tracing::warn!(
                                    %session_id, renewal_n,
                                    "voice: the renewed duplex session did not take the part of \
                                     the caller's sentence heard so far"
                                );
                                break;
                            }
                        }
                        let briefed = renewal.briefed;
                        let base = clock_now_ms(model_clock, opened);
                        let leftover = lineup.take_over(
                            renewal,
                            base,
                            Framer::new(output, frame_ms),
                            close_grace,
                            renew_after,
                            &session_id,
                        );
                        tracing::info!(
                            %session_id, renewal_n, clock_ms = base, briefed,
                            on_a_quiet_line = calm,
                            inside_the_callers_turn = handed_over,
                            "voice: the renewed duplex session took over"
                        );
                        if send_audio(sink, leftover).await.is_err() {
                            break;
                        }
                    }
                    // Gone before it could take over (review I-3): the
                    // session in charge stays in charge.
                    Some(failed) if failed.started => {
                        report_failed_renewal(&shared, &session_id, failed, &mut lineup.usage).await;
                        lineup.renewal_gone(renew_after);
                    }
                    other => {
                        lineup.renewal = other;
                        lineup.due_since = None;
                    }
                }
            }

            // GH #1400: the line's first quiet moment for the append that
            // waits for it, re-checked as the takeover is: the model's next
            // words already waiting move the moment out; at the cap it goes.
            () = sleep_until_opt(quiet_held_at) => {
                let Some((split_id, parts, since)) = quiet_held.take() else {
                    continue;
                };
                if !lineup.quiet(spoken.is_some(), quiet) && since.elapsed() < cap {
                    quiet_held = Some((split_id, parts, since));
                    lineup.heard();
                    continue;
                }
                // Every part of the waiting text, in its order (d54g callseam).
                for (kind, event_id, content) in parts {
                    let append = DuplexControl::Append {
                        kind,
                        event_id,
                        delegation_id: None,
                        content,
                    };
                    if lineup.run.control_tx.send(append).await.is_err() {
                        tracing::debug!(
                            %session_id,
                            "voice: the duplex session is gone, the append was dropped"
                        );
                    }
                }
            }

            // Time passed, and nobody said anything. A turn here is cut on
            // the MODEL's timeline, so a caller who stops talking closes a turn
            // only when a clock says so — and the provider's running meter,
            // which used to be that clock (OR-L8), does not arrive on a quiet
            // line at all (GH #798, four runs, no meter inside 45 s). Deliberately
            // NOT `liveness.mark_success()`: this is our own clock, and a
            // watchdog that its own ticking keeps alive watches nothing.
            //
            // BELOW the events arm, and that is the whole reason `biased` is
            // written out here: both can be ready in the same wake — a tick
            // falls due while a transcript fragment is already in the channel,
            // one poll away — and the arm that is polled first wins the
            // iteration outright. With the clock first the loser is always the
            // caller: the turn is closed for silence that had already ended,
            // and the waiting fragment opens a second turn with the rest of the
            // sentence in it. A deadline that fires one tick later is a
            // deadline that fired; a turn cut in half is a turn lost.
            // Below the events for the reason the tick is: a mark that is a
            // poll late is still the mark it was.
            () = call_marks.due() => {
                let elapsed_s = call_marks.fire();
                shared.emit(VoiceEvent::CallMark {
                    session_id: session_id.clone(),
                    elapsed_s,
                }).await;
            }

            _ = ticker.tick() => {
                let now_ms = clock_now_ms(model_clock, opened);
                shared.emit(VoiceEvent::LiveTick {
                    session_id: session_id.clone(),
                    now_ms,
                }).await;
            }

            // d54g weblive, contract 3: the release's mute, once every frame of
            // the hold is with the model. Not awaited in the release's arm: the
            // loop goes on carrying the model's audio while it waits.
            () = audio_drained(&lineup.run.audio_tx), if mute_due => {
                mute_due = false;
                lineup.ear(DuplexControl::Mute).await;
            }

            incoming = stream.next() => {
                let Some(message) = incoming else { break };
                match message {
                    Incoming::Binary(bytes) => {
                        if frame_bytes == 0 || bytes.len() % frame_bytes != 0 {
                            // R-V6': dropped, counted, never fatal — the same
                            // answer the cascade gives, so a mis-framing client
                            // reads one story whichever engine is behind it.
                            bad_frames = bad_frames.saturating_add(1);
                            shared.emit(VoiceEvent::BadAudioFrame {
                                session_id: session_id.clone(),
                                len: bytes.len(),
                                count: bad_frames,
                            }).await;
                            continue;
                        }
                        // d54g weblive, contract 2: in `hold` the key is the
                        // one door to the microphone. A frame outside a hold
                        // goes nowhere -- not to the model, and not as silence
                        // either -- also while the model speaks. The page
                        // sends none (0 of 1 165, measured in a browser
                        // against the live model, 2026-11-13); this is
                        // the cell keeping the promise for any client.
                        if mode == Mode::Hold && ear_shut {
                            if !dropped_outside {
                                dropped_outside = true;
                                tracing::info!(
                                    %session_id,
                                    "voice: microphone audio outside a hold is dropped"
                                );
                            }
                            continue;
                        }
                        if let Some(closed) = duck.caller(&bytes) {
                            tracing::info!(
                                %session_id, ducked = closed, n = duck.ducked,
                                "voice: the caller talks over the model"
                            );
                        }
                        // One frame, one `input_audio.append`. Blocking when
                        // the model is behind: that is the backpressure, and it
                        // reaches the client as TCP. Always the session in
                        // charge: a renewal hears the caller only once it has
                        // taken over (GH #896).
                        if lineup.run.audio_tx.send(bytes).await.is_err() {
                            tracing::debug!(%session_id, "voice: the duplex session is gone");
                        }
                    }
                    Incoming::Text(text) => {
                        let frame = match duplex_text(sink, &text).await {
                            Err(()) => break,
                            Ok(None) => continue,
                            Ok(Some(frame)) => frame,
                        };
                        // Nothing here reaches `turns.rs`: the cascade's turn
                        // machine is not instantiated on this path, and the
                        // model draws its own boundaries (R-25-9). The ear is
                        // told to the session in charge AND to a renewal
                        // waiting to take over (GH #896).
                        match frame {
                            // In `hold` the key IS the ear (OR-L24).
                            ClientFrame::Hold | ClientFrame::Release
                                if mode != Mode::Hold =>
                            {
                                let refusal = ServerFrame::Error {
                                    code: WireErrorCode::WrongMode,
                                    detail: "hold and release are only frames in \
                                             `hold` mode".to_string(),
                                    bad_frames: None,
                                };
                                if send_frame(sink, &refusal).await.is_err() {
                                    break;
                                }
                            }
                            ClientFrame::Hold => {
                                ear_shut = false;
                                dropped_outside = false;
                                idle_since = Instant::now();
                                // A release whose mute still waits is over:
                                // the ear never shut, and it stays open.
                                mute_due = false;
                                // Probe (d): a hold into the model's speech
                                // stops it. Speaking = a section is open or
                                // the model was audible within the quiet the
                                // line is judged by (`spoken_quiet_ms`).
                                let speaking = spoken.is_some()
                                    || model_audio_at.is_some_and(|at| at.elapsed() < quiet);
                                if speaking {
                                    hushed = true;
                                    tracing::info!(
                                        %session_id,
                                        "voice: a hold into the model's speech stops it"
                                    );
                                    if let Some(sp) = spoken.take()
                                        && finish_speak(
                                            &shared, &session_id, sink, sp.speak_id,
                                            SpeakEndReason::Cancelled, None,
                                        ).await.is_err()
                                    {
                                        break;
                                    }
                                }
                                lineup.ear(DuplexControl::Unmute).await;
                            }
                            ClientFrame::Release => {
                                ear_shut = true;
                                idle_since = Instant::now();
                                // The model's answer to what was just said is
                                // heard again.
                                hushed = false;
                                // Contract 3: the mute is the provider's one
                                // word for "the caller is done" (its socket has
                                // no commit and no turn trigger); it goes in
                                // the arm below once the last frame of the hold
                                // is with the model.
                                mute_due = true;
                            }
                            // The session is open already: nothing to warm.
                            ClientFrame::Ready => {}
                            ClientFrame::Cancel => {
                                if let Some(sp) = spoken.take()
                                    && finish_speak(
                                        &shared, &session_id, sink, sp.speak_id,
                                        SpeakEndReason::Cancelled, None,
                                    ).await.is_err()
                                {
                                    break;
                                }
                            }
                            ClientFrame::Mode { mode: wanted } => {
                                mode = wanted;
                                // `auto` is an open ear by definition; `hold`
                                // shuts it until the next key.
                                let cmd = match mode {
                                    Mode::Auto => DuplexControl::Unmute,
                                    Mode::Hold => DuplexControl::Mute,
                                };
                                ear_shut = mode == Mode::Hold;
                                // The switch says what the ear is now; a
                                // waiting mute and a stopped model are over.
                                mute_due = false;
                                hushed = false;
                                lineup.ear(cmd).await;
                                if send_frame(sink, &ServerFrame::Mode { mode })
                                    .await
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        }
                    }
                    Incoming::Close => break,
                }
            }

            // A renewal that is not in charge yet (GH #896). Below the client:
            // nothing it says before it takes over is the caller's business,
            // and the one thing that is — its start — can wait one frame.
            tick = next_renewal(&mut lineup.renewal) => {
                match tick {
                    RenewalTick::Event(Some(DuplexEvent::Started { session_id: provider_session })) => {
                        shared.liveness.mark_success();
                        let Some(renewal) = lineup.renewal.as_mut() else { continue };
                        if renewal.started {
                            continue;
                        }
                        renewal.started = true;
                        let renewal_n = renewal.renewal_n;
                        lineup.announced = renewal_n;
                        // From now on it can take over: at the colony's
                        // handover block, or at the grace without one.
                        lineup.takeover_by = Some(Instant::now() + renew_grace);
                        tracing::info!(
                            %session_id, %provider_session, renewal_n,
                            "voice: the renewed duplex session started; waiting for its handover"
                        );
                        shared.emit(VoiceEvent::Renewed {
                            session_id: session_id.clone(),
                            renewal_n,
                            renewed_at_ms: epoch_ms(),
                        }).await;
                    }
                    // A fault the new session survived is the colony's to
                    // read, whether or not that session is in charge yet.
                    RenewalTick::Event(Some(w @ DuplexEvent::Warning { .. })) => {
                        shared.liveness.mark_success();
                        shared.emit(VoiceEvent::Live {
                            session_id: session_id.clone(),
                            event: w,
                        }).await;
                    }
                    RenewalTick::Event(Some(other)) => {
                        shared.liveness.mark_success();
                        // Its meter and its close count for the call before it
                        // takes over: a renewal that fails here was billed all
                        // the same (D48, DK, review B-1 f). One ledger entry
                        // per session, so one that takes over counts on and
                        // is not counted twice.
                        if let Some(slot) = lineup.renewal.as_ref().map(|r| r.usage_slot)
                            && lineup.usage.observe(slot, &other)
                        {
                            report_usage(&shared, &session_id, &lineup.usage, false).await;
                        }
                        tracing::debug!(
                            %session_id, event = ?other,
                            "voice: the renewed duplex session said something before it took \
                             over -- not passed on"
                        );
                    }
                    // The renewal is over before it ever took over. The session
                    // in charge is untouched and the call goes on; the colony
                    // is told on its warning lane, and the next attempt comes
                    // `renew_after_ms` from now.
                    RenewalTick::Event(None) => {
                        let Some(failed) = lineup.renewal.take() else { continue };
                        report_failed_renewal(&shared, &session_id, failed, &mut lineup.usage).await;
                        lineup.renewal_gone(renew_after);
                    }
                    // Nothing it says is heard before it takes over: two
                    // voices on one line is worse than a word lost, and a new
                    // session that has not heard the caller has nothing to
                    // say to them.
                    RenewalTick::Audio(Some(_)) => {
                        tracing::debug!(
                            %session_id,
                            "voice: the renewed duplex session spoke before it took over -- \
                             dropped"
                        );
                    }
                    RenewalTick::Audio(None) => {
                        if let Some(renewal) = lineup.renewal.as_mut() {
                            renewal.audio_open = false;
                        }
                    }
                }
            }

            // Time to open the next session of this call (GH #896). It opens
            // with the ear the call has now and hears nothing of the caller
            // yet.
            () = sleep_until_opt(lineup.renew_at) => {
                lineup.renew_at = None;
                // A seeded renewal asks again (D48): the new session opens
                // on the conversation as it stands NOW. The old one keeps
                // talking meanwhile, so the wait is no hole on the line.
                let renewal_n = lineup.announced.saturating_add(1);
                ask_seed(&shared, &session_id, SeedReason::Renew, renewal_n).await;
                let run = DuplexRun::start_renewed(&shared, &provider, input);
                if ear_shut {
                    let _ = run.control_tx.send(DuplexControl::Mute).await;
                }
                tracing::info!(
                    %session_id, renewal_n,
                    "voice: renewing the duplex session inside the call"
                );
                lineup.renewal = Some(Renewal {
                    run,
                    renewal_n,
                    started: false,
                    audio_open: true,
                    briefed: false,
                    seeded: false,
                    usage_slot: lineup.usage.session(),
                });
            }

            // The handover did not come in time. The call is worth more than
            // the summary: the new session is due to take over knowing its
            // instructions and nothing else — at the next quiet moment, like a
            // briefed one (the arm above) — and a block that arrives later
            // still reaches it as an ordinary append once it is in charge.
            () = sleep_until_opt(lineup.takeover_by) => {
                lineup.takeover_by = None;
                if let Some(r) = lineup.renewal.as_ref()
                    && r.started
                    && lineup.due_since.is_none()
                {
                    tracing::info!(
                        %session_id, renewal_n = r.renewal_n,
                        grace_ms = shared.duplex_renew_grace_ms,
                        "voice: no handover within renew_grace_ms -- the renewed duplex \
                         session takes over without one at the next quiet moment"
                    );
                    lineup.due_since = Some(Instant::now());
                }
            }

            // The retiring session finishing its sentence (GH #896). The
            // caller hears it — a model cut off mid-word is worse than a
            // sentence that ends a little late — until the new session speaks:
            // from its first chunk on the old one's tail is dropped (the arm
            // below), so two voices never share the line. It comes last but
            // one, for the reason audio comes last at all.
            chunk = next_retiring_audio(&mut lineup.retiring_audio) => {
                match chunk {
                    Some(mut bytes) => {
                        shared.liveness.mark_success();
                        lineup.heard();
                        if crate::voice::duck::first_audible(&bytes, audible_rate, audible_level)
                            .is_some()
                        {
                            model_audio_at = Some(Instant::now());
                        }
                        // Its transcripts arm the mark too (review M-3): the
                        // answer the retiring session speaks is the turn's.
                        let audible = audible_offset_ms(
                            // Silenced by a hold, nothing of it is heard.
                            audio_mark_armed && !hushed,
                            &bytes,
                            audible_rate,
                            audible_level,
                        );
                        duck.model(&mut bytes);
                        // Probe (d): a hold came into the model's speech.
                        if hushed {
                            bytes.fill(0);
                        }
                        let frames = match lineup.retiring_audio.as_mut() {
                            Some(a) => a.framer.push(bytes),
                            None => vec![bytes],
                        };
                        if send_audio(sink, frames).await.is_err() {
                            break;
                        }
                        if let Some(offset) = audible {
                            audio_mark_armed = false;
                            shared.emit(VoiceEvent::LiveAudioOut {
                                session_id: session_id.clone(),
                                at_ms: clock_now_ms(model_clock, opened) + offset,
                            }).await;
                        }
                    }
                    None => {
                        if let Some(mut a) = lineup.retiring_audio.take()
                            && send_audio(sink, a.framer.flush().into_iter().collect())
                                .await
                                .is_err()
                        {
                            break;
                        }
                    }
                }
            }

            // Last on purpose (see the doc comment): a burst of model audio
            // must not hold a `cancel` off the socket.
            chunk = lineup.run.audio_out_rx.recv(), if lineup.audio_open => {
                match chunk {
                    Some(mut bytes) => {
                        shared.liveness.mark_success();
                        lineup.heard();
                        if crate::voice::duck::first_audible(&bytes, audible_rate, audible_level)
                            .is_some()
                        {
                            model_audio_at = Some(Instant::now());
                        }
                        // Heard before the duck: ducked speech is still the
                        // answer the model gave (fix-round C-1).
                        let audible = audible_offset_ms(
                            // Silenced by a hold, nothing of it is heard.
                            audio_mark_armed && !hushed,
                            &bytes,
                            audible_rate,
                            audible_level,
                        );
                        // Read for its level, then silenced while the caller
                        // talks over it (GH #1055).
                        duck.model(&mut bytes);
                        // Probe (d): a hold came into the model's speech.
                        if hushed {
                            bytes.fill(0);
                        }
                        // The session in charge speaks: whatever the session a
                        // renewal replaced still had to say is dropped from
                        // here on. Two models' frames interleaved on one sink
                        // are noise to the caller, not an overlap (review I-2).
                        if let Some(tail) = lineup.retiring_audio.take() {
                            tracing::debug!(
                                %session_id,
                                "voice: the renewed session speaks -- the tail of the session \
                                 it replaced is dropped"
                            );
                            drop(tail);
                        }
                        let frames = lineup.framer.push(bytes);
                        if send_audio(sink, frames).await.is_err() {
                            break;
                        }
                        // Read after the frames are on the sink, on the clock
                        // the tick reads: the time the caller's side got them,
                        // to the millisecond, not the model's 200 ms raster,
                        // plus the silence that leads the chunk. A chunk shorter
                        // than the framer's rest goes out with the next one,
                        // at most one frame (20 ms) after this mark.
                        if let Some(offset) = audible {
                            audio_mark_armed = false;
                            shared.emit(VoiceEvent::LiveAudioOut {
                                session_id: session_id.clone(),
                                at_ms: clock_now_ms(model_clock, opened) + offset,
                            }).await;
                        }
                    }
                    // The model's audio is over; whether the SESSION is over is
                    // what the event channel says.
                    None => lineup.audio_open = false,
                }
            }

            // d54g weblive, contract 4: the page was idle for `hold_idle_ms`.
            // Its session ends the way a call does -- the open turn, the
            // provider's close, the `final` usage -- and the connection stays
            // for the next hold. Last: anything else that is ready moves it.
            () = sleep_until_opt(idle_at) => {
                tracing::info!(
                    %session_id, idle_ms = shared.duplex_hold_idle_ms,
                    "voice: the page was idle -- its session ends, the page stays"
                );
                idle_ended = true;
                break;
            }
        }
    }

    // The connection is over (d54g callseam): the handler closes the open turn
    // NOW, from the transcript it holds, and emits it -- ahead of the
    // provider's close, which `duplex_verdict` below may wait `close_grace_ms`
    // for, and so ahead of the call's `final` usage. Measured on 2026-11-07:
    // the turn that waited for the provider left 330 ms and more after the
    // line was gone and stood at the beginning of the next conversation.
    shared
        .emit(VoiceEvent::LiveEnded {
            session_id: session_id.clone(),
            at_ms: clock_now_ms(model_clock, opened),
            model_audible: model_audio_at.is_some_and(|at| at.elapsed() < quiet),
        })
        .await;
    // A renewal still waiting and a session still retiring end with the call
    // (GH #896). Dropping them drops their way in, and a provider reads that
    // as the end of the conversation.
    let Lineup {
        mut run,
        mut framer,
        mut renewal,
        mut retiring,
        retiring_audio,
        mut usage,
        in_charge_slot,
        ..
    } = lineup;
    // What they already said counts before they go (D48, DK, review B-1 e): a
    // close in the channel is their number, else their last meter reading
    // stands in and the call's `final` says it is not complete.
    if let Some(r) = renewal.as_mut() {
        while let Ok(event) = r.run.events_rx.try_recv() {
            usage.observe(r.usage_slot, &event);
        }
    }
    if let Some(r) = retiring.as_mut() {
        while let Ok(event) = r.events_rx.try_recv() {
            usage.observe(r.usage_slot, &event);
        }
    }
    drop(renewal);
    drop(retiring);
    if let Some(mut a) = retiring_audio {
        let _ = send_audio(sink, a.framer.flush().into_iter().collect()).await;
    }
    // A held part-sample is the model's last bytes, not noise.
    let _ = send_audio(sink, framer.flush().into_iter().collect()).await;
    // Exactly one `SpeakEnded` per section this connection opened, whatever
    // ended it — the telephony hive holds a hang-up until this arrives.
    if let Some(sp) = spoken.take() {
        let _ = finish_speak(
            &shared,
            &session_id,
            sink,
            sp.speak_id,
            SpeakEndReason::Cancelled,
            None,
        )
        .await;
    }
    // The client is gone, and `audio_in` closing IS the end of the call
    // (contract § 1.1). The provider now sends its own close and answers with
    // the final meter reading.
    drop(run.audio_tx);
    // The wait is the operator's `params.duplex.close_grace_ms`, carried on the
    // shared half by the factory (OR-L.L2b.1). It is the connection's backstop
    // AROUND the provider's own wait, so the two have to be the same number: a
    // shorter one cuts off the final `Closed { usage_seconds }`, the last
    // session's share of the call's `usage` total (D48, DK).
    let verdict = duplex_verdict(
        &shared,
        &session_id,
        &mut run.events_rx,
        run.done_rx,
        run.clock_base_ms,
        close_grace,
        (&mut usage, in_charge_slot),
    )
    .await;
    if provider_ended {
        match verdict {
            // GH #1377: where the call may be recovered, a session that ends
            // after the last attempt fails the call even in good order: the
            // caller is still on the line, and the error lane is what the
            // telephony edge answers with its announcement and a hang-up.
            Ok(()) if recover_max > 0 => {
                let cause = last_close.unwrap_or_else(|| "closed".to_string());
                tracing::warn!(
                    %session_id, %cause, recoveries, recover_max,
                    "voice: the duplex session ended mid-call with no recovery left"
                );
                shared
                    .emit(VoiceEvent::DuplexFailed {
                        session_id: session_id.clone(),
                        detail: format!(
                            "the provider ended the duplex session mid-call ({cause}); \
                             {recoveries} of recover_max {recover_max} recoveries spent"
                        ),
                        provider_code: None,
                    })
                    .await;
                closing = Some(1011);
            }
            Ok(()) => closing = Some(1000),
            Err(e) => {
                // No retry, no reconnect (OR-L20): the handler turns this into
                // `error duplex_failed` and the client reads a close. GH #1373
                // (BAT R1d C2): the failure is also said on a `WARN` line --
                // eight refused starts (401) stood only in an error body.
                tracing::warn!(
                    %session_id, error = %e,
                    "voice: the duplex session failed"
                );
                shared
                    .emit(VoiceEvent::DuplexFailed {
                        session_id: session_id.clone(),
                        detail: e.to_string(),
                        provider_code: e.provider_code().map(str::to_string),
                    })
                    .await;
                closing = Some(1011);
            }
        }
    } else if let Err(e) = verdict {
        // The call had already ended when the provider gave up. Nobody is on
        // the line to be told, and a call that is over cannot fail.
        tracing::warn!(
            %session_id, error = %e,
            "voice: the duplex session ended badly after the client had gone"
        );
    }
    if let Some(code) = closing {
        sink.close(code, "").await;
    }
    // The call is over: its one `final` usage, whatever ended it (D48, DK,
    // review B-1) -- before `unregister`, whose `Disconnected` is the
    // handler's last word of the call.
    report_usage(&shared, &session_id, &usage, true).await;
    (!idle_ended && !session_lost, mode)
}

/// Which way a command came in on during a session.
enum Way {
    /// The session's own way (`None`: its delivery let go).
    Session(Option<ToConnection>),
    /// A page's connection way, for what reaches the page's own name while a
    /// session is open (`None`: that delivery let go too).
    Connection(Option<ToConnection>),
}

/// The next command for a session: its own way first, then -- for a page's
/// session -- what reaches the page's own name (a close of the connection, an
/// order addressed to it). Both receives are cancel-safe, so nothing is lost
/// when another arm of the session's `select!` wins.
async fn next_command(
    session: &mut mpsc::Receiver<ToConnection>,
    page: &mut Option<&mut mpsc::Receiver<ToConnection>>,
) -> Way {
    match page {
        None => Way::Session(session.recv().await),
        Some(connection) => tokio::select! {
            biased;
            cmd = session.recv() => Way::Session(cmd),
            cmd = connection.recv() => Way::Connection(cmd),
        },
    }
}

/// One text frame from a duplex client.
///
/// The counterpart of [`handle_text`], and the difference is what is MISSING:
/// no `VoiceEvent::Control`. The cascade hands every control frame to the turn
/// machine because that machine owns what a boundary means; a duplex connection
/// has no turn machine — the model draws its own boundaries — so a `hold` here
/// is an instruction to the model's ear and nothing else (contract § 1.3).
///
/// Returns the frame that was read, or `None` for one that was answered with an
/// error instead. `Err(())` when the socket is gone.
async fn duplex_text(sink: &mut Sink, text: &str) -> Result<Option<ClientFrame>, ()> {
    match meclaw_core::serde_json::from_str::<ClientFrame>(text) {
        Ok(f) => Ok(Some(f)),
        Err(e) => {
            let frame = ServerFrame::Error {
                code: WireErrorCode::BadFrame,
                detail: e.to_string(),
                bad_frames: None,
            };
            send_frame(sink, &frame).await.map(|()| None)
        }
    }
}

/// Wait for the provider's verdict while still forwarding what it says.
///
/// The final `Closed { reason, usage_seconds }` travels on the EVENT channel,
/// not on the verdict, so a connection that dropped the receiver in order to
/// wait would lose its close, the one reading that is a session's own final
/// cost -- without it the ledger has only the last meter to stand in
/// (OR-L22: there is no `session` lane; the number rides out on `turn`, in
/// the cell's own log and, since D48/DK, in the call's usage ledger, which
/// the `final` of the `usage` lane reads). Its stamps are moved onto the
/// call's timeline like every other event of the session (`clock_base_ms`,
/// GH #896).
async fn duplex_verdict(
    shared: &Arc<VoiceIoShared>,
    session_id: &str,
    events_rx: &mut mpsc::Receiver<DuplexEvent>,
    done_rx: oneshot::Receiver<Result<(), DuplexError>>,
    clock_base_ms: u64,
    limit: Duration,
    (usage, slot): (&mut CallUsage, UsageSlot),
) -> Result<(), DuplexError> {
    let mut done = done_rx;
    let deadline = Instant::now() + limit;
    let mut events_open = true;
    loop {
        tokio::select! {
            biased;

            outcome = &mut done => {
                // Whatever the provider said on its way out is already in the
                // channel; it is taken before the verdict is answered, so the
                // handler cannot learn the session ended before it learns what
                // it cost.
                while let Ok(event) = events_rx.try_recv() {
                    usage.observe(slot, &event);
                    after_the_end(shared, session_id, event, clock_base_ms).await;
                }
                return match outcome {
                    Ok(verdict) => verdict,
                    Err(_) => Err(DuplexError::Closed(
                        "the duplex task ended without a verdict".to_string(),
                    )),
                };
            }

            event = events_rx.recv(), if events_open => {
                match event {
                    Some(event) => {
                        usage.observe(slot, &event);
                        after_the_end(shared, session_id, event, clock_base_ms).await;
                    }
                    None => events_open = false,
                }
            }

            () = tokio::time::sleep_until(deadline) => {
                return Err(DuplexError::Timeout);
            }
        }
    }
}

/// One event a provider delivered after the connection ended (d54g callseam).
///
/// A transcript fragment is booked in the log and goes no further: the open
/// turn closed with the connection ([`VoiceEvent::LiveEnded`]), and words that
/// arrive after it would open a turn nobody is on the line for -- measured on
/// 2026-11-07, the tail of one call stood at the beginning of the next
/// conversation. Its length and stamps are logged, never its words. Every
/// other event travels on as before: the close carries the session's cost.
async fn after_the_end(
    shared: &Arc<VoiceIoShared>,
    session_id: &str,
    event: DuplexEvent,
    clock_base_ms: u64,
) {
    if let DuplexEvent::Transcript {
        speaker,
        delta,
        start_ms,
        end_ms,
    } = &event
    {
        tracing::info!(
            %session_id,
            ?speaker,
            chars = delta.chars().count(),
            start_ms,
            end_ms,
            "voice: a transcript fragment arrived after the connection ended -- \
             booked here, no turn is made of it"
        );
        return;
    }
    shared
        .emit(VoiceEvent::Live {
            session_id: session_id.to_string(),
            event: shift_event(event, clock_base_ms),
        })
        .await;
}

/// One text frame from the client.
///
/// Returns the frame that was handed on, or `None` for one that was answered
/// with an error instead — the caller reads it to keep the one piece of turn
/// state a connection has of its own (GH #657), and a refused frame must not
/// move it. `Err(())` when the socket is gone. An unparseable frame is answered
/// and the connection stays: a client that sent nonsense once can be told so.
async fn handle_text(
    shared: &Arc<VoiceIoShared>,
    session_id: &str,
    sink: &mut Sink,
    echo: bool,
    text: &str,
) -> Result<Option<ClientFrame>, ()> {
    let frame = match meclaw_core::serde_json::from_str::<ClientFrame>(text) {
        Ok(f) => f,
        Err(e) => {
            let frame = ServerFrame::Error {
                code: WireErrorCode::BadFrame,
                detail: e.to_string(),
                bad_frames: None,
            };
            return send_frame(sink, &frame).await.map(|()| None);
        }
    };
    if echo
        && matches!(
            frame,
            ClientFrame::Hold | ClientFrame::Release | ClientFrame::Cancel
        )
    {
        let frame = ServerFrame::Error {
            code: WireErrorCode::WrongMode,
            detail: "the echo provider has no turns and nothing to say".to_string(),
            bad_frames: None,
        };
        return send_frame(sink, &frame).await.map(|()| None);
    }
    // Everything else is the handler's call: it owns the turn machine and is
    // the only emitter.
    shared
        .emit(VoiceEvent::Control {
            session_id: session_id.to_string(),
            frame: frame.clone(),
        })
        .await;
    Ok(Some(frame))
}

/// Start one synthesis, or report at once why there is none.
#[allow(clippy::too_many_arguments)]
async fn start_speak(
    shared: &Arc<VoiceIoShared>,
    sink: &mut Sink,
    session_id: &str,
    output: Option<AudioFormat>,
    speak_id: String,
    text: String,
) -> Result<Option<Speaking>, ()> {
    let Some(provider) = shared.tts.clone() else {
        finish_speak(
            shared,
            session_id,
            sink,
            speak_id,
            SpeakEndReason::Failed,
            Some("no text-to-speech provider is configured".to_string()),
        )
        .await?;
        return Ok(None);
    };
    let frame = ServerFrame::SpeakStart {
        speak_id: speak_id.clone(),
    };
    if send_frame(sink, &frame).await.is_err() {
        // The socket died between the order and its first frame. `speak_id`
        // has already left the command channel and the caller is about to
        // leave the loop with nothing in `speaking`, so the post-loop block
        // finds nothing to report — this is the only place left to keep the
        // one-`SpeakEnded`-per-`Speak` promise the handler counts on, and a
        // hive that holds a hang-up until the sentence is over waits for ever
        // without it. No `speak_end` frame: there is nobody to read it.
        shared
            .emit(VoiceEvent::SpeakEnded {
                session_id: session_id.to_string(),
                speak_id,
                reason: SpeakEndReason::Failed,
                detail: Some("the connection went away before the synthesis started".to_string()),
            })
            .await;
        return Err(());
    }

    let (audio_tx, audio_rx) = mpsc::channel::<Vec<u8>>(AUDIO_QUEUE);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let (done_tx, done_rx) = oneshot::channel();
    let liveness = shared.liveness.clone();
    // The format this CONNECTION was told in `hello`, so the framer cuts at
    // the rate the client is actually being sent (GH #619) — a 20 ms frame is
    // 320 bytes at 8 kHz and 960 at 24 kHz, and a framer reading the
    // provider's declared rate would cut the wrong length for a negotiated
    // one. `unwrap_or` cannot fire: `provider` above exists, so the connection
    // negotiated an output beside it.
    let format = output.unwrap_or_else(|| provider.output_format());
    tokio::spawn(async move {
        let outcome = provider
            .synthesize(format, text, audio_tx, cancel_rx, liveness)
            .await;
        let _ = done_tx.send(outcome);
    });
    Ok(Some(Speaking {
        speak_id,
        cancel: cancel_tx,
        audio_rx,
        framer: Framer::new(Some(format), audio_out_frame_ms(shared)),
        done_rx,
        first_audio: FirstAudio::default(),
    }))
}

/// Tell the client and the handler that a synthesis is over.
async fn finish_speak(
    shared: &Arc<VoiceIoShared>,
    session_id: &str,
    sink: &mut Sink,
    speak_id: String,
    reason: SpeakEndReason,
    detail: Option<String>,
) -> Result<(), ()> {
    let frame = ServerFrame::SpeakEnd {
        speak_id: speak_id.clone(),
        reason,
        detail: detail.clone(),
    };
    let sent = send_frame(sink, &frame).await;
    shared
        .emit(VoiceEvent::SpeakEnded {
            session_id: session_id.to_string(),
            speak_id,
            reason,
            detail,
        })
        .await;
    sent
}

/// Serialise and send one server frame.
async fn send_frame(sink: &mut Sink, frame: &ServerFrame) -> Result<(), ()> {
    let text = match meclaw_core::serde_json::to_string(frame) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "voice: a server frame did not serialise");
            return Ok(());
        }
    };
    sink.send(Outgoing::Text(text)).await.map_err(|_| ())
}

/// Resolves once the model has taken every frame queued for it (d54g weblive,
/// contract 3).
///
/// Holding every permit of the queue at once is only possible while nothing
/// waits in it; a provider takes a frame and puts it on its wire before it
/// reads its next control, so from here on a control frame goes behind the
/// caller's last word. A queue whose receiver is gone resolves at once: there
/// is nothing left to wait for. The permits are given back on the spot.
async fn audio_drained(audio_tx: &mpsc::Sender<Vec<u8>>) {
    let _ = audio_tx.reserve_many(audio_tx.max_capacity()).await;
}

/// Send audio frames in order, stopping at the first socket that is gone.
async fn send_audio(sink: &mut Sink, frames: Vec<Vec<u8>>) -> Result<(), ()> {
    for frame in frames {
        sink.send(Outgoing::Binary(frame)).await.map_err(|_| ())?;
    }
    Ok(())
}

/// The next thing the running synthesis produces, if one is running.
///
/// No timeout lives in here, and that is the point. A `timeout` rebuilt on
/// every pass of a `select!` is reset by every *other* arm that wins, so on a
/// connection with traffic it would never fire. The deadline is therefore a
/// plain `Instant` in the loop, watched by an arm of its own and moved only
/// when this synthesis actually produces something.
async fn next_synth_tick(speaking: &mut Option<Speaking>) -> SynthTick {
    match speaking {
        None => std::future::pending().await,
        Some(sp) => match sp.audio_rx.recv().await {
            None => SynthTick::End,
            Some(bytes) => SynthTick::Chunk(bytes),
        },
    }
}

/// The next recognition event, if a session is running.
///
/// `None` means the session ended — the caller decides whether that is a retry
/// or the end of the connection. Without a session this never resolves.
async fn next_stt_event(stt: &mut Option<SttSession>) -> Option<SttEvent> {
    match stt {
        None => std::future::pending().await,
        Some(session) => session.events_rx.recv().await,
    }
}

/// When the next frame of the silence tail is due, after the one due at `due`
/// went out at `now` (GH #1199).
///
/// The tail runs on a fixed raster from the release, one slot every
/// [`TAIL_FRAME`], and a late wake moves nothing after it. It used to re-arm
/// from the wake (`now + TAIL_FRAME`), so every late wake shifted every later
/// frame and the delays added up: the provider needs 90 frames (1.8 s
/// measured, GH #743) before it ends the turn, the grace caps the drain at
/// 2.5 s, and that leaves 28 ms a frame. Under CPUQuota 80 % that was not
/// enough: the #743 lock lost the end of the sentence in 1 of 5 runs on
/// build01 and 2 of 5 on build04, and a loaded host loses it the same way.
///
/// A wake a whole frame or more late does NOT catch up: it sends its one
/// frame and the next is due on the first slot after `now`. The provider
/// counts the tail as audio arriving in real time, so ten frames in one
/// instant would be 200 ms of silence that never took 200 ms, and the
/// recognition queue (`AUDIO_QUEUE`) would take the burst instead of the
/// provider. A skipped slot is the same frame the old re-arming lost; a wake
/// late by less than a frame loses nothing any more.
fn next_tail_slot(due: Instant, now: Instant) -> Instant {
    let next = due + TAIL_FRAME;
    if next > now {
        return next;
    }
    // `now` is past the next slot already: the first slot after it.
    let behind = now.saturating_duration_since(due).as_nanos();
    let into_slot = behind % TAIL_FRAME.as_nanos();
    now + TAIL_FRAME - Duration::from_nanos(into_slot as u64)
}

/// Wait for a deadline, or forever when there is none.
async fn sleep_until_opt(at: Option<Instant>) {
    match at {
        None => std::future::pending().await,
        Some(deadline) => tokio::time::sleep_until(deadline).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::contract::AudioFormat;

    /// d54g callseam: the caller's sentence at a takeover is cut for the
    /// append ceiling between words, and nothing of it is lost.
    #[test]
    fn append_parts_lose_nothing_and_cut_between_words() {
        assert_eq!(append_parts("one two"), vec!["one two".to_string()]);
        assert!(append_parts("").is_empty());

        let long = "word ".repeat(APPEND_MAX_CHARS / 5 + 3);
        let parts = append_parts(&long);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts.concat(), long);
        assert_eq!(parts[0].chars().count(), APPEND_MAX_CHARS);
        assert!(
            parts[1].starts_with("word "),
            "cut between words: {parts:?}"
        );

        // One word longer than an append is cut inside, in order.
        let word = "x".repeat(APPEND_MAX_CHARS + 7);
        let parts = append_parts(&word);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts.concat(), word);
    }

    /// d54g callseam: one stamp per synthesis, at the first frame that
    /// actually travels.
    #[test]
    fn the_first_audio_of_a_synthesis_is_counted_once() {
        let mut first = FirstAudio::default();
        assert!(
            !first.leaves_with(&[]),
            "a chunk the framer held back whole is no audio yet"
        );
        assert!(first.leaves_with(&[vec![0u8; 4]]), "the first frame out");
        assert!(
            !first.leaves_with(&[vec![0u8; 4], vec![0u8; 4]]),
            "every later frame belongs to a synthesis already stamped"
        );
    }

    /// 24 kHz PCM16 mono, 20 ms: the 960 bytes the FreeSWITCH finding is about.
    #[test]
    fn twenty_milliseconds_at_24k_is_960_bytes() {
        let f = Framer::new(Some(AudioFormat::pcm16_mono(24_000)), 20);
        assert_eq!(f.frame_len, 960);
        assert_eq!(f.sample_bytes, 2);
        assert_eq!(
            Framer::new(Some(AudioFormat::pcm16_mono(16_000)), 20).frame_len,
            640,
            "the rate comes from the declared output format, never from a constant"
        );
    }

    /// Review M1: an asked `renew` joins an opening renewal only while that
    /// one's seed is still to come; after the seed it is one more renewal.
    #[test]
    fn an_asked_renew_joins_only_an_unseeded_renewal() {
        let opening = |seeded, started| Some(OpeningRenewal { seeded, started });
        assert_eq!(renew_asked(None), RenewAsked::OpenNow);
        assert_eq!(renew_asked(opening(false, false)), RenewAsked::Joins);
        assert_eq!(renew_asked(opening(true, false)), RenewAsked::AfterTakeover);
        assert_eq!(renew_asked(opening(true, true)), RenewAsked::AfterTakeover);
    }

    /// Review kv-2 A-1: a renewal that started without its seed (the curator
    /// slower than `seed_wait_ms`) drops a seed that comes later, so it can
    /// carry no later fact: an asked `renew` after its start is one more
    /// renewal, opened after the takeover, never absorbed.
    #[test]
    fn an_asked_renew_after_an_unseeded_start_opens_one_more_renewal() {
        let started_unseeded = Some(OpeningRenewal {
            seeded: false,
            started: true,
        });
        assert_eq!(renew_asked(started_unseeded), RenewAsked::AfterTakeover);
    }

    /// A rate whose 20 ms is not a whole number of samples rounds DOWN to one:
    /// 11.025 kHz * 2 B * 20 ms = 441 B, and 441 would cut a sample in half.
    #[test]
    fn a_frame_length_is_rounded_down_to_a_whole_sample() {
        let f = Framer::new(Some(AudioFormat::pcm16_mono(11_025)), 20);
        assert_eq!(f.frame_len, 440);
        assert_eq!(f.frame_len % f.sample_bytes, 0);
    }

    /// Two chunks with an even tail each: the tail of the first does NOT wait
    /// for the second, so the second is framed exactly like the first.
    #[test]
    fn an_even_tail_does_not_bleed_into_the_next_chunk() {
        let mut f = Framer::new(Some(AudioFormat::pcm16_mono(24_000)), 20);
        for round in 0..2 {
            let frames = f.push(vec![1; 4096]);
            let lens: Vec<usize> = frames.iter().map(Vec::len).collect();
            assert_eq!(
                lens,
                vec![960, 960, 960, 960, 256],
                "round {round}: 4 * 960 + 256, and nothing carried over"
            );
            assert!(f.carry.is_empty());
        }
    }

    /// The passthrough, and the two ways to reach it.
    #[test]
    fn zero_and_a_missing_format_both_pass_a_chunk_through() {
        for mut f in [
            Framer::new(Some(AudioFormat::pcm16_mono(24_000)), 0),
            Framer::new(None, 20),
        ] {
            assert_eq!(f.frame_len, 0);
            assert_eq!(f.push(vec![7; 40_000]), vec![vec![7; 40_000]]);
            assert_eq!(
                f.push(Vec::new()),
                vec![Vec::<u8>::new()],
                "the passthrough is literal: what the provider produced, unchanged"
            );
            assert!(f.flush().is_none(), "nothing is held in the passthrough");
        }
    }

    /// One big chunk: whole frames, then a shorter tail, and nothing invented.
    #[test]
    fn a_big_chunk_becomes_whole_frames_and_one_shorter_tail() {
        let mut f = Framer::new(Some(AudioFormat::pcm16_mono(24_000)), 20);
        let frames = f.push(vec![3; 40_960]);
        assert_eq!(frames.len(), 43, "42 * 960 + 640");
        assert!(
            frames[..42].iter().all(|fr| fr.len() == 960),
            "every frame but the last is a full one"
        );
        assert_eq!(frames[42].len(), 640);
        assert_eq!(frames.iter().map(Vec::len).sum::<usize>(), 40_960);
        assert!(f.flush().is_none());
    }

    /// An odd tail is held, not cut through a sample and not dropped.
    #[test]
    fn an_odd_tail_waits_for_the_next_chunk() {
        let mut f = Framer::new(Some(AudioFormat::pcm16_mono(24_000)), 20);
        let first = f.push(vec![1; 961]);
        assert_eq!(first, vec![vec![1; 960]], "the odd byte did not go out");
        assert_eq!(f.carry, vec![1], "it is held");

        let second = f.push(vec![2; 959]);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].len(), 960);
        assert_eq!(second[0][0], 1, "the held byte leads the next frame");
        assert!(second[0][1..].iter().all(|b| *b == 2));
        assert!(f.flush().is_none(), "nothing is left over");
    }

    /// The one frame that may be odd: what is left when the synthesis ends.
    #[test]
    fn the_last_frame_carries_what_the_provider_could_not_fill() {
        let mut f = Framer::new(Some(AudioFormat::pcm16_mono(24_000)), 20);
        assert!(f.push(vec![9; 5]).is_empty(), "an odd first tail is held");
        assert_eq!(f.flush(), Some(vec![9; 5]), "and sent at the end");
        assert!(f.flush().is_none(), "exactly once");
    }

    /// An even tail leaves at once: latency is the point of framing, so a
    /// provider that ships small chunks is not made to wait for a full frame.
    #[test]
    fn an_even_tail_goes_out_with_its_own_chunk() {
        let mut f = Framer::new(Some(AudioFormat::pcm16_mono(24_000)), 20);
        assert_eq!(f.push(vec![4; 4]), vec![vec![4; 4]]);
        assert!(f.push(Vec::new()).is_empty(), "an empty chunk is no frame");
    }

    // ── GH #896: a renewed session on the call's timeline ────────────────

    /// The three stamped events move by the base, everything else passes
    /// unchanged, and a base of zero — every call that is never renewed —
    /// changes nothing.
    #[test]
    fn gh896_a_renewed_sessions_stamps_move_onto_the_calls_timeline() {
        let transcript = DuplexEvent::Transcript {
            speaker: Speaker::User,
            delta: "there".to_string(),
            start_ms: 100,
            end_ms: 300,
        };
        assert_eq!(
            shift_event(transcript.clone(), 5_400),
            DuplexEvent::Transcript {
                speaker: Speaker::User,
                delta: "there".to_string(),
                start_ms: 5_500,
                end_ms: 5_700,
            }
        );
        assert_eq!(
            shift_event(
                DuplexEvent::Appended {
                    kind: AppendKind::Thinking,
                    event_id: "e1".to_string(),
                    start_ms: 0,
                    end_ms: 40,
                },
                5_400,
            ),
            DuplexEvent::Appended {
                kind: AppendKind::Thinking,
                event_id: "e1".to_string(),
                start_ms: 5_400,
                end_ms: 5_440,
            }
        );
        assert_eq!(
            shift_event(
                DuplexEvent::DelegationCreated {
                    delegation_id: "d1".to_string(),
                    offset_ms: 7,
                },
                5_400,
            ),
            DuplexEvent::DelegationCreated {
                delegation_id: "d1".to_string(),
                offset_ms: 5_407,
            }
        );
        let closed = DuplexEvent::Closed {
            reason: "close_requested".to_string(),
            usage_seconds: 1.5,
        };
        assert_eq!(
            shift_event(closed.clone(), 5_400),
            closed,
            "an unstamped event is not moved"
        );
        assert_eq!(
            shift_event(transcript.clone(), 0),
            transcript,
            "the first session of a call is on the call's clock already"
        );
        assert_eq!(
            shift_event(
                DuplexEvent::Transcript {
                    speaker: Speaker::User,
                    delta: String::new(),
                    start_ms: u64::MAX - 1,
                    end_ms: u64::MAX,
                },
                10,
            ),
            DuplexEvent::Transcript {
                speaker: Speaker::User,
                delta: String::new(),
                start_ms: u64::MAX,
                end_ms: u64::MAX,
            },
            "saturating, never a panic"
        );
    }

    /// A handle an EARLIER session opened loses it; one the session in charge
    /// opened keeps it; one no session was seen opening passes as it always
    /// has — the connection strips only what it knows is gone.
    #[test]
    fn gh896_only_a_handle_of_an_earlier_session_is_stripped() {
        let mut d = Delegations::default();
        d.opened("d-old");
        assert_eq!(
            d.admit(Some("d-old".to_string())).as_deref(),
            Some("d-old"),
            "before any renewal nothing changes"
        );
        d.renewed();
        d.renewed();
        assert_eq!(
            d.earlier,
            vec!["d-old".to_string()],
            "renewed is idempotent"
        );
        d.opened("d-new");
        assert_eq!(d.admit(Some("d-old".to_string())), None);
        assert_eq!(d.admit(Some("d-new".to_string())).as_deref(), Some("d-new"));
        assert_eq!(
            d.admit(Some("never-seen".to_string())).as_deref(),
            Some("never-seen")
        );
        assert_eq!(d.admit(None), None);
        d.opened_earlier("d-late");
        assert_eq!(
            d.admit(Some("d-late".to_string())),
            None,
            "a delegation the retiring session opened on its way out is an earlier one"
        );
        for n in 0..(2 * EARLIER_DELEGATIONS_KEPT) {
            d.opened_earlier(&format!("x-{n}"));
        }
        assert_eq!(d.earlier.len(), EARLIER_DELEGATIONS_KEPT, "bounded");
        assert!(
            !d.earlier.contains(&"d-old".to_string()),
            "the oldest go first"
        );
    }

    /// The last words of a retiring session are passed on, moved; its close is
    /// kept here, because in the handler it would close the turn the caller is
    /// still in; and its meter is nobody's business any more.
    #[test]
    fn gh896_a_retiring_session_passes_its_words_and_keeps_its_close() {
        let mut d = Delegations::default();
        let words = retiring_event(
            DuplexEvent::Transcript {
                speaker: Speaker::Assistant,
                delta: "bye".to_string(),
                start_ms: 10,
                end_ms: 20,
            },
            1_000,
            1,
            "call-1",
            &mut d,
        );
        assert_eq!(
            words,
            Some(DuplexEvent::Transcript {
                speaker: Speaker::Assistant,
                delta: "bye".to_string(),
                start_ms: 1_010,
                end_ms: 1_020,
            })
        );
        let delegation = retiring_event(
            DuplexEvent::DelegationCreated {
                delegation_id: "d-9".to_string(),
                offset_ms: 5,
            },
            1_000,
            1,
            "call-1",
            &mut d,
        );
        assert!(matches!(
            delegation,
            Some(DuplexEvent::DelegationCreated {
                offset_ms: 1_005,
                ..
            })
        ));
        assert_eq!(d.admit(Some("d-9".to_string())), None);
        for kept in [
            DuplexEvent::Closed {
                reason: "close_requested".to_string(),
                usage_seconds: 3.0,
            },
            DuplexEvent::Usage {
                seconds: 3.0,
                usage_ratio: Some(0.4),
            },
            DuplexEvent::Started {
                session_id: "s".to_string(),
            },
            DuplexEvent::Muted,
            DuplexEvent::Warning {
                detail: "x".to_string(),
                provider_code: None,
                client_event_id: None,
            },
        ] {
            assert_eq!(
                retiring_event(kept.clone(), 1_000, 1, "call-1", &mut d),
                None,
                "{kept:?} stays in the connection"
            );
        }
    }

    // ── GH #836: `hello` only after the handler took the session ─────────

    /// The failure marker of this repo: generous, never a budget.
    const MARKER: Duration = Duration::from_secs(30);

    /// One cascade connection on a channel link, driven by hand: the test IS
    /// the handler, so it decides when -- and whether -- `Connected` is taken.
    struct Hand {
        events: mpsc::Receiver<VoiceEvent>,
        to_cell: mpsc::Sender<meclaw_colony::LinkFrame>,
        from_cell: mpsc::Receiver<meclaw_colony::LinkFrame>,
        conn: tokio::task::JoinHandle<()>,
    }

    fn hand(external_timeout: Duration) -> Hand {
        hand_in(Mode::Auto, external_timeout)
    }

    fn hand_in(mode: Mode, external_timeout: Duration) -> Hand {
        hand_on(
            mode,
            external_timeout,
            Arc::new(crate::voice::providers::echo::EchoStt::new()),
        )
    }

    /// A recogniser with turns: the echo provider under another name. `echo`
    /// refuses `hold`, `release` and `cancel` (`wrong_mode`), so a test about a
    /// take runs on this one; what it hears is the echo's business.
    struct TurnStt(crate::voice::providers::echo::EchoStt);

    impl crate::voice::contract::SttProvider for TurnStt {
        fn name(&self) -> &'static str {
            "turns"
        }
        fn input_format(&self) -> crate::voice::contract::AudioFormat {
            self.0.input_format()
        }
        fn input_rates(&self) -> Vec<u32> {
            self.0.input_rates()
        }
        fn negotiate_input(&self, sample_rate: u32) -> Option<crate::voice::contract::AudioFormat> {
            self.0.negotiate_input(sample_rate)
        }
        fn run_session(
            &self,
            format: crate::voice::contract::AudioFormat,
            audio: mpsc::Receiver<Vec<u8>>,
            events: mpsc::Sender<crate::voice::contract::SttEvent>,
            liveness: meclaw_colony::io_liveness::IoLivenessMark,
        ) -> crate::voice::contract::BoxFuture<Result<(), crate::voice::contract::SttError>>
        {
            self.0.run_session(format, audio, events, liveness)
        }
    }

    fn hand_on(
        mode: Mode,
        external_timeout: Duration,
        stt: Arc<dyn crate::voice::contract::SttProvider>,
    ) -> Hand {
        let (events_tx, events) = mpsc::channel::<VoiceEvent>(16);
        let shared =
            crate::voice::io::VoiceIoShared::for_test_with_stt(events_tx, external_timeout, stt);
        let (to_cell, to_cell_rx) = mpsc::channel(16);
        let (from_cell_tx, from_cell) = mpsc::channel(16);
        let link = ClientLink::chan(to_cell_rx, from_cell_tx);
        let negotiated =
            crate::voice::service::negotiate(&shared, None).expect("the echo half negotiates");
        let (conn_id, to_conn_tx, to_conn_rx) = crate::voice::io::new_connection_slot();
        let conn = tokio::spawn(run_connection(
            link,
            shared,
            "call-1".to_string(),
            mode,
            negotiated,
            conn_id,
            to_conn_tx,
            to_conn_rx,
        ));
        Hand {
            events,
            to_cell,
            from_cell,
            conn,
        }
    }

    impl Hand {
        async fn event(&mut self) -> VoiceEvent {
            tokio::time::timeout(MARKER, self.events.recv())
                .await
                .expect("the connection reports to the handler")
                .expect("the event channel is open")
        }

        async fn frame(&mut self) -> meclaw_colony::LinkFrame {
            tokio::time::timeout(MARKER, self.from_cell.recv())
                .await
                .expect("the connection says something to its client")
                .expect("the link is open")
        }
    }

    fn is_hello(frame: &meclaw_colony::LinkFrame) -> bool {
        matches!(frame, meclaw_colony::LinkFrame::Text(t) if t.contains("\"hello\""))
    }

    /// The defect: `hello` left right after `Connected` was queued, so a
    /// client could hold its `hello` while the handler did not know the
    /// session yet. The silence below has its control in the same test: once
    /// the event is taken, the very next frame IS the `hello`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gh836_hello_waits_until_the_handler_took_the_session() {
        let mut h = hand(MARKER);
        let mut connected = h.event().await;
        assert!(
            matches!(&connected, VoiceEvent::Connected { session_id, .. } if session_id == "call-1"),
            "the connection reports itself first: {connected:?}"
        );
        // Held, not taken: nothing may reach the client yet. The window is a
        // semantic one (the control is the frame right after it), not a
        // guess at how long anything takes.
        let early = tokio::time::timeout(Duration::from_millis(500), h.from_cell.recv()).await;
        assert!(
            early.is_err(),
            "no frame may reach the client before the handler took the session; got {early:?}"
        );
        connected.acknowledge();
        let first = h.frame().await;
        assert!(
            is_hello(&first),
            "the first frame after the acknowledgement is the hello: {first:?}"
        );
        drop(h.to_cell);
        let _ = tokio::time::timeout(MARKER, h.conn).await;
    }

    /// A handshake nobody takes is bounded by `external_timeout`: no `hello`,
    /// a close `1013` naming why, and the session given back -- the
    /// `Disconnected` is the receipt that nothing stays registered.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gh836_a_session_nobody_takes_is_closed_and_given_back() {
        let mut h = hand(Duration::from_millis(300));
        let _held = h.event().await;
        let frame = h.frame().await;
        let meclaw_colony::LinkFrame::Close { code, reason } = frame else {
            panic!("the first and only frame is the close, never a hello: {frame:?}")
        };
        assert_eq!(code, CLOSE_SESSION_NOT_TAKEN);
        assert!(reason.contains("in time"), "the close says why: {reason:?}");
        let gone = h.event().await;
        assert!(
            matches!(&gone, VoiceEvent::Disconnected { session_id } if session_id == "call-1"),
            "the session is taken back rather than left half-registered: {gone:?}"
        );
        tokio::time::timeout(MARKER, h.conn)
            .await
            .expect("the connection task ends")
            .expect("no panic");
    }

    /// A handler that is gone drops the acknowledgement, and the connection
    /// does not wait out its deadline for an answer that cannot come.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gh836_a_handler_that_dropped_the_event_closes_at_once() {
        let mut h = hand(MARKER);
        drop(h.event().await);
        // Far below MARKER, the deadline this connection was given: the close
        // has to come from the dropped acknowledgement, not from the clock.
        let frame = tokio::time::timeout(Duration::from_secs(10), h.from_cell.recv())
            .await
            .expect("the close does not wait for the deadline")
            .expect("the link is open");
        assert!(
            matches!(&frame, meclaw_colony::LinkFrame::Close { code, .. } if *code == CLOSE_SESSION_NOT_TAKEN),
            "no hello for a session nobody holds: {frame:?}"
        );
    }

    /// OR-T10-15 with K8 (b) (welle-testing tv-bn M10f): a screen that loses its link in
    /// the middle of a take ends the take locally and never sends the `release` --
    /// so the cell must not keep the take open for a release that cannot come. The
    /// channel's end IS the end of the take: the session is handed back
    /// (`Disconnected`, which takes the turn machine's state with it), and the cell
    /// makes up no `release` of its own on the way out.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_hold_whose_channel_ends_is_closed_by_the_cell() {
        let mut h = hand_on(
            Mode::Hold,
            MARKER,
            Arc::new(TurnStt(crate::voice::providers::echo::EchoStt::new())),
        );
        let mut connected = h.event().await;
        assert!(
            matches!(&connected, VoiceEvent::Connected { session_id, .. } if session_id == "call-1"),
            "the connection reports itself first: {connected:?}"
        );
        connected.acknowledge();
        let hello = h.frame().await;
        assert!(is_hello(&hello), "the hello comes first: {hello:?}");
        h.to_cell
            .send(meclaw_colony::LinkFrame::Text(r#"{"type":"hold"}"#.into()))
            .await
            .expect("the link takes the hold");
        let held = h.event().await;
        assert!(
            matches!(
                &held,
                VoiceEvent::Control {
                    frame: ClientFrame::Hold,
                    ..
                }
            ),
            "the hold reaches the handler: {held:?}"
        );
        // The channel ends with the take open: no `release`, ever.
        let (unrelated, _) = mpsc::channel(1);
        drop(std::mem::replace(&mut h.to_cell, unrelated));
        loop {
            let ev = h.event().await;
            match ev {
                VoiceEvent::Disconnected { ref session_id } => {
                    assert_eq!(session_id, "call-1");
                    break;
                }
                VoiceEvent::Control { ref frame, .. } => {
                    panic!("the cell made up a control frame on the channel's end: {frame:?}")
                }
                _ => continue,
            }
        }
        tokio::time::timeout(MARKER, h.conn)
            .await
            .expect("the connection task ends with its channel")
            .expect("no panic");
    }

    /// The grace `VoiceIoShared::for_test_with_stt` ships: the tail stops at the
    /// first wake on or after this many milliseconds past the release.
    const TAP_GRACE: Duration = Duration::from_millis(1500);

    /// A recogniser that only listens (GH #1199): every frame it is handed is
    /// stamped with the moment it arrived and passed on to the test. It never
    /// ends a turn, so a released boundary drains for its whole grace and the
    /// stamps ARE the silence tail's cadence.
    struct TapStt {
        inner: crate::voice::providers::echo::EchoStt,
        taps: mpsc::UnboundedSender<Instant>,
    }

    impl crate::voice::contract::SttProvider for TapStt {
        fn name(&self) -> &'static str {
            "tap"
        }
        fn input_format(&self) -> crate::voice::contract::AudioFormat {
            self.inner.input_format()
        }
        fn input_rates(&self) -> Vec<u32> {
            self.inner.input_rates()
        }
        fn negotiate_input(&self, sample_rate: u32) -> Option<crate::voice::contract::AudioFormat> {
            self.inner.negotiate_input(sample_rate)
        }
        fn run_session(
            &self,
            _format: crate::voice::contract::AudioFormat,
            mut audio: mpsc::Receiver<Vec<u8>>,
            events: mpsc::Sender<crate::voice::contract::SttEvent>,
            _liveness: meclaw_colony::io_liveness::IoLivenessMark,
        ) -> crate::voice::contract::BoxFuture<Result<(), crate::voice::contract::SttError>>
        {
            let taps = self.taps.clone();
            Box::pin(async move {
                // Held, not dropped: a closed event channel reads to the
                // connection as a session that ended, and the tail with it.
                let _events = events;
                while audio.recv().await.is_some() {
                    let _ = taps.send(Instant::now());
                }
                Ok(())
            })
        }
    }

    /// Wait for the handler's copy of one control frame, skipping whatever
    /// else the connection reports on the way.
    async fn control(h: &mut Hand, wanted: ClientFrame) {
        loop {
            if let VoiceEvent::Control { frame, .. } = h.event().await
                && frame == wanted
            {
                return;
            }
        }
    }

    /// Open a take on a [`TapStt`], release it, and return when each frame of
    /// the silence tail reached the provider, in ms after the release.
    ///
    /// `starve` is the load the flake was about (GH #1199): right after the
    /// `n`-th frame the clock jumps by `by` while the connection is not polled,
    /// which is what a runner with no CPU to spare does to a task whose timer
    /// is due. Virtual time, so every stamp is exact: the only waits on this
    /// path are tokio channels and the tail's own `sleep_until`.
    async fn tail_offsets(starve: Option<(usize, Duration)>) -> Vec<u64> {
        let (taps_tx, mut taps) = mpsc::unbounded_channel();
        let mut h = hand_on(
            Mode::Hold,
            MARKER,
            Arc::new(TapStt {
                inner: crate::voice::providers::echo::EchoStt::new(),
                taps: taps_tx,
            }),
        );
        let mut connected = h.event().await;
        connected.acknowledge();
        let hello = h.frame().await;
        assert!(is_hello(&hello), "the hello comes first: {hello:?}");
        h.to_cell
            .send(meclaw_colony::LinkFrame::Text(r#"{"type":"hold"}"#.into()))
            .await
            .expect("the link takes the hold");
        control(&mut h, ClientFrame::Hold).await;
        h.to_cell
            .send(meclaw_colony::LinkFrame::Text(
                r#"{"type":"release"}"#.into(),
            ))
            .await
            .expect("the link takes the release");
        control(&mut h, ClientFrame::Release).await;
        // The connection arms the tail in the same poll that reported the
        // release, and the paused clock only moves when every task is idle:
        // this IS the release instant.
        let released = Instant::now();
        let end = released + TAP_GRACE + TAIL_FRAME;
        let mut offsets = Vec::new();
        while let Ok(Some(at)) = tokio::time::timeout_at(end, taps.recv()).await {
            offsets.push(at.duration_since(released).as_millis() as u64);
            if let Some((n, by)) = starve
                && offsets.len() == n
            {
                tokio::time::advance(by).await;
            }
        }
        drop(h.to_cell);
        let _ = tokio::time::timeout(MARKER, h.conn).await;
        offsets
    }

    /// The raster the tail is due on: one frame every `TAIL_FRAME` from the
    /// release, `from..=to` in ms.
    fn raster(from: u64, to: u64) -> Vec<u64> {
        (from..=to)
            .step_by(TAIL_FRAME.as_millis() as usize)
            .collect()
    }

    /// GH #1199 (b): undisturbed, the tail is exactly what it was before the
    /// fix, a frame every 20 ms from the release, the last one on the wake
    /// before the 1500 ms grace, 74 in all. Green on the old code as well: it
    /// is the control that the fix changed nothing where nothing was late.
    #[tokio::test(start_paused = true)]
    async fn gh1199_an_undisturbed_tail_keeps_its_twenty_millisecond_raster() {
        let got = tail_offsets(None).await;
        assert_eq!(got, raster(20, 1480), "one frame every 20 ms, 74 in all");
    }

    /// GH #1199 (a): a wake 7 ms late sends its frame late and shifts nothing
    /// after it. The old tail re-armed from the wake (`now + TAIL_FRAME`), so
    /// every late wake moved all later frames; measured, 1 of 5 (build01) and
    /// 2 of 5 (build04) runs of the #743 lock lost the end of the sentence
    /// under CPUQuota 80 %.
    #[tokio::test(start_paused = true)]
    async fn gh1199_a_late_wake_does_not_shift_the_frames_after_it() {
        let got = tail_offsets(Some((10, Duration::from_millis(27)))).await;
        let mut want = raster(20, 200);
        want.push(227);
        want.extend(raster(240, 1480));
        assert_eq!(
            got, want,
            "the frame after a late one is due on the raster, not 20 ms after the wake"
        );
        assert_eq!(got.len(), 74, "a late wake costs no frame");
    }

    /// GH #1199 (c): a wake ten frames late sends ONE frame and resumes on the
    /// next slot of the raster. The missed slots are gone, never sent in a
    /// burst: a provider reads the tail as audio arriving in real time, and ten
    /// frames in one instant are 200 ms of audio that never took 200 ms.
    #[tokio::test(start_paused = true)]
    async fn gh1199_a_long_sleep_skips_the_missed_frames_instead_of_bursting() {
        let got = tail_offsets(Some((10, Duration::from_millis(213)))).await;
        assert!(
            got.windows(2).all(|w| w[0] < w[1]),
            "no two frames of the tail leave in the same instant: {got:?}"
        );
        let mut want = raster(20, 200);
        want.push(413);
        want.extend(raster(420, 1480));
        assert_eq!(
            got, want,
            "one frame on the late wake, then the next raster slot after it"
        );
    }
}
