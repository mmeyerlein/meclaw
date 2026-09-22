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

use crate::voice::cell::VoiceEvent;
use crate::voice::contract::{
    AppendKind, AudioFormat, DuplexControl, DuplexError, DuplexEvent, DuplexProvider,
    DuplexSession, Speaker, SttError, SttEvent, TtsError,
};
use crate::voice::io::{ToConnection, VoiceIoShared, register, unregister};
use crate::voice::link::{ClientLink, Incoming, LinkSink, Outgoing};
use crate::voice::service::{Negotiated, audio_out_frame_ms, stt_name, tts_name};
use crate::voice::wire::{ClientFrame, Mode, PROTOCOL, ServerFrame, SpeakEndReason, WireErrorCode};

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
    register(&shared, &session_id, conn_id, to_conn_tx, mode).await;

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
                        tail_next = Some(now + TAIL_FRAME);
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
                        let frames = match speaking.as_mut() {
                            Some(sp) => sp.framer.push(bytes),
                            None => vec![bytes],
                        };
                        if send_audio(&mut sink, frames).await.is_err() {
                            break;
                        }
                    }
                    SynthTick::End => {
                        speak_deadline = None;
                        if let Some(mut sp) = speaking.take() {
                            let id = sp.speak_id.clone();
                            // A held part-sample is the provider's last bytes,
                            // not noise: it leaves as a short last frame ahead
                            // of the verdict, in the order a client plays it.
                            let flushed =
                                send_audio(&mut sink, sp.framer.flush().into_iter().collect())
                                    .await;
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

/// The longest append one `session.*.append` may carry, in characters.
///
/// The documented ceiling is 500 tokens; 1 800 characters is the wave's
/// conservative reading of it (contract § 1.5). Longer guidance is split at
/// paragraph boundaries by the handler, never here.
pub(crate) const APPEND_MAX_CHARS: usize = 1_800;

/// One running duplex session, as the connection holds it.
///
/// The counterpart of [`SttSession`] and [`Speaking`] in one: a duplex model is
/// both directions on one socket, so there is one thing to start and one
/// verdict to wait for rather than two of each.
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
}

impl DuplexRun {
    /// Start one session for this connection, at the format it negotiated.
    fn start(
        shared: &Arc<VoiceIoShared>,
        provider: &Arc<dyn DuplexProvider>,
        format: AudioFormat,
    ) -> Self {
        let (audio_tx, audio_in) = mpsc::channel::<Vec<u8>>(AUDIO_QUEUE);
        let (audio_out, audio_out_rx) = mpsc::channel::<Vec<u8>>(AUDIO_QUEUE);
        let (events, events_rx) = mpsc::channel::<DuplexEvent>(AUDIO_QUEUE);
        let (control_tx, control) = mpsc::channel::<DuplexControl>(AUDIO_QUEUE);
        let (done_tx, done_rx) = oneshot::channel();
        let provider = provider.clone();
        let liveness = shared.liveness.clone();
        tokio::spawn(async move {
            let outcome = provider
                .run_session(
                    format,
                    DuplexSession {
                        audio_in,
                        audio_out,
                        events,
                        control,
                    },
                    liveness,
                )
                .await;
            let _ = done_tx.send(outcome);
        });
        Self {
            audio_tx,
            audio_out_rx,
            events_rx,
            control_tx,
            done_rx,
        }
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
/// 3. **There is no reconnect** (OR-L20). The session IS the conversation; a
///    fresh socket would be a fresh conversation with no memory of this one.
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
    register(&shared, &session_id, conn_id, to_conn_tx, mode).await;

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

    // Started with the `hello` and not with the first frame: a live model
    // greets before the caller speaks (`params.duplex.greeting`, OR-L25), and
    // a session opened on the first buffer would greet nobody.
    let mut run = DuplexRun::start(&shared, &provider, input);
    // In `hold` the ear stays shut until the key goes down (OR-L24).
    if mode == Mode::Hold {
        let _ = run.control_tx.send(DuplexControl::Mute).await;
    }

    // The model's chunks are cut to what the client can swallow — a telephony
    // edge aborts the call above about 100 ms (see the module note) — and to
    // nothing else. No jitter buffer, no pacing (R-L4).
    let mut framer = Framer::new(output, audio_out_frame_ms(&shared));
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
    let opened = Instant::now();
    let mut model_clock: Option<(u64, Instant)> = None;
    let mut ticker = tokio::time::interval(Duration::from_millis(shared.duplex_tick_ms.max(1)));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut closing: Option<u16> = None;
    let mut half_gone = shared.shutdown();
    let mut bad_frames: u32 = 0;
    let frame_bytes = input.frame_bytes();
    // Whether the model's audio channel is still open. Without it the arm
    // below would spin on a closed receiver: `recv` answers `None` at once,
    // for ever.
    let mut audio_open = true;
    // Whether the loop ended because the PROVIDER ended. It decides what the
    // verdict below means: a provider that gave up while somebody was on the
    // line is a `duplex_failed` and a `1011`, the same provider ending after
    // the caller hung up is the ordinary end of a call.
    let mut provider_ended = false;

    loop {
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
            cmd = to_conn_rx.recv() => {
                match cmd {
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
                    Some(ToConnection::CancelSpeak) => {
                        if let Some(sp) = spoken.take()
                            && finish_speak(
                                &shared, &session_id, &mut sink, sp.speak_id,
                                SpeakEndReason::Cancelled, None,
                            ).await.is_err()
                        {
                            break;
                        }
                    }
                    Some(ToConnection::Advise {
                        kind, event_id, delegation_id, content, speak_id,
                    }) => {
                        if let Some(id) = speak_id {
                            // One spoken section at a time, exactly as one
                            // synthesis at a time in the cascade: an
                            // overlapping order can only mean the handler
                            // moved on, so the open one is closed and
                            // reported rather than left to fight for the
                            // socket.
                            if let Some(prev) = spoken.take()
                                && finish_speak(
                                    &shared, &session_id, &mut sink, prev.speak_id,
                                    SpeakEndReason::Done, None,
                                ).await.is_err()
                            {
                                break;
                            }
                            let frame = ServerFrame::SpeakStart {
                                speak_id: id.clone(),
                            };
                            if send_frame(&mut sink, &frame).await.is_err() {
                                break;
                            }
                            spoken = Some(Spoken {
                                speak_id: id,
                                quiet_until: None,
                                cap_until: Instant::now() + cap,
                            });
                        }
                        if run.control_tx.send(DuplexControl::Append {
                            kind, event_id, delegation_id, content,
                        }).await.is_err() {
                            tracing::debug!(
                                %session_id,
                                "voice: the duplex session is gone, the append was dropped"
                            );
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
                        &shared, &session_id, &mut sink, sp.speak_id,
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
                        &shared, &session_id, &mut sink, sp.speak_id,
                        SpeakEndReason::Done, None,
                    ).await.is_err()
                {
                    break;
                }
            }

            event = run.events_rx.recv() => {
                match event {
                    Some(event) => {
                        // An event is a completed provider round trip (issue #7).
                        shared.liveness.mark_success();
                        // And every event that carries a stamp moves the
                        // session's clock onto the model's timeline. Only
                        // forward: a fragment whose `end_ms` lies behind where
                        // the clock already stands is a late arrival, not time
                        // running backwards, and setting the clock back would
                        // hold every open turn open for the difference.
                        match &event {
                            DuplexEvent::Transcript { end_ms, .. }
                            | DuplexEvent::Appended { end_ms, .. } => {
                                stamp_model_clock(&mut model_clock, *end_ms);
                            }
                            DuplexEvent::DelegationCreated { offset_ms, .. } => {
                                stamp_model_clock(&mut model_clock, *offset_ms);
                            }
                            _ => {}
                        }
                        // The two events the SECTION is measured by, before the
                        // handler ever sees them. The `appended` answer is what
                        // starts the quiet clock — the model has taken the
                        // guidance up — and every assistant fragment after it
                        // pushes the clock out again.
                        match &event {
                            DuplexEvent::Appended {
                                kind: AppendKind::Commentary, event_id, ..
                            } => {
                                if let Some(sp) = spoken.as_mut()
                                    && sp.speak_id == *event_id
                                {
                                    sp.quiet_until = Some(Instant::now() + quiet);
                                }
                            }
                            DuplexEvent::Transcript {
                                speaker: Speaker::Assistant, ..
                            } => {
                                if let Some(sp) = spoken.as_mut()
                                    && sp.quiet_until.is_some()
                                {
                                    sp.quiet_until = Some(Instant::now() + quiet);
                                }
                            }
                            _ => {}
                        }
                        shared.emit(VoiceEvent::Live {
                            session_id: session_id.clone(),
                            event,
                        }).await;
                    }
                    // The provider is done, one way or another. Its verdict is
                    // waiting below; there is no retry and no second session
                    // (OR-L20).
                    None => {
                        provider_ended = true;
                        break;
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
            _ = ticker.tick() => {
                let now_ms = match model_clock {
                    Some((offset_ms, stamped)) => offset_ms.saturating_add(
                        u64::try_from(stamped.elapsed().as_millis()).unwrap_or(u64::MAX),
                    ),
                    None => u64::try_from(opened.elapsed().as_millis()).unwrap_or(u64::MAX),
                };
                shared.emit(VoiceEvent::LiveTick {
                    session_id: session_id.clone(),
                    now_ms,
                }).await;
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
                        // One frame, one `input_audio.append`. Blocking when
                        // the model is behind: that is the backpressure, and it
                        // reaches the client as TCP.
                        if run.audio_tx.send(bytes).await.is_err() {
                            tracing::debug!(%session_id, "voice: the duplex session is gone");
                        }
                    }
                    Incoming::Text(text) => {
                        let frame = match duplex_text(&mut sink, &text).await {
                            Err(()) => break,
                            Ok(None) => continue,
                            Ok(Some(frame)) => frame,
                        };
                        // Nothing here reaches `turns.rs`: the cascade's turn
                        // machine is not instantiated on this path, and the
                        // model draws its own boundaries (R-25-9).
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
                                if send_frame(&mut sink, &refusal).await.is_err() {
                                    break;
                                }
                            }
                            ClientFrame::Hold => {
                                let _ = run.control_tx.send(DuplexControl::Unmute).await;
                            }
                            ClientFrame::Release => {
                                let _ = run.control_tx.send(DuplexControl::Mute).await;
                            }
                            ClientFrame::Cancel => {
                                if let Some(sp) = spoken.take()
                                    && finish_speak(
                                        &shared, &session_id, &mut sink, sp.speak_id,
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
                                let _ = run.control_tx.send(cmd).await;
                                if send_frame(&mut sink, &ServerFrame::Mode { mode })
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

            // Last on purpose (see the doc comment): a burst of model audio
            // must not hold a `cancel` off the socket.
            chunk = run.audio_out_rx.recv(), if audio_open => {
                match chunk {
                    Some(bytes) => {
                        shared.liveness.mark_success();
                        if send_audio(&mut sink, framer.push(bytes)).await.is_err() {
                            break;
                        }
                    }
                    // The model's audio is over; whether the SESSION is over is
                    // what the event channel says.
                    None => audio_open = false,
                }
            }
        }
    }

    // A held part-sample is the model's last bytes, not noise.
    let _ = send_audio(&mut sink, framer.flush().into_iter().collect()).await;
    // Exactly one `SpeakEnded` per section this connection opened, whatever
    // ended it — the telephony hive holds a hang-up until this arrives.
    if let Some(sp) = spoken.take() {
        let _ = finish_speak(
            &shared,
            &session_id,
            &mut sink,
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
    // shorter one cuts off the final `Closed { usage_seconds }`, and after
    // OR-L22 that is the only place a session's cost is ever reported.
    let verdict = duplex_verdict(
        &shared,
        &session_id,
        &mut run.events_rx,
        run.done_rx,
        Duration::from_millis(shared.close_grace_ms),
    )
    .await;
    if provider_ended {
        match verdict {
            Ok(()) => closing = Some(1000),
            Err(e) => {
                // No retry, no reconnect (OR-L20): the handler turns this into
                // `error duplex_failed` and the client reads a close.
                shared
                    .emit(VoiceEvent::DuplexFailed {
                        session_id: session_id.clone(),
                        detail: e.to_string(),
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
    unregister(&shared, &session_id, conn_id).await;
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
/// wait would lose the only reading a session's cost is ever reported in
/// (OR-L22: there is no `session` lane; the number rides out on `turn` and in
/// the cell's own log).
async fn duplex_verdict(
    shared: &Arc<VoiceIoShared>,
    session_id: &str,
    events_rx: &mut mpsc::Receiver<DuplexEvent>,
    done_rx: oneshot::Receiver<Result<(), DuplexError>>,
    limit: Duration,
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
                    shared.emit(VoiceEvent::Live {
                        session_id: session_id.to_string(),
                        event,
                    }).await;
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
                        shared.emit(VoiceEvent::Live {
                            session_id: session_id.to_string(),
                            event,
                        }).await;
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
}
