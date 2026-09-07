//! One WebSocket connection, for its whole life (wave voice-cell).
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

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use crate::voice::cell::VoiceEvent;
use crate::voice::contract::{AudioFormat, SttError, SttEvent, TtsError};
use crate::voice::io::{ToConnection, VoiceIoShared, register, unregister};
use crate::voice::service::{audio_out, audio_out_frame_ms, tts_name};
use crate::voice::wire::{ClientFrame, Mode, PROTOCOL, ServerFrame, SpeakEndReason, WireErrorCode};

/// How long a lost recognition session is left alone before the one retry.
///
/// One retry, then the connection ends: a provider that refuses twice in a row
/// is not a blip, and a client that keeps talking into a deaf socket is worse
/// off than one that is told to reconnect.
const STT_RETRY_DELAY: Duration = Duration::from_secs(1);

/// How many audio chunks may be in flight between a provider and this task.
const AUDIO_QUEUE: usize = 32;

/// The sending half of the client socket.
type Sink = SplitSink<WebSocket, Message>;

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
    /// Start one session for this connection.
    fn start(shared: &Arc<VoiceIoShared>) -> Self {
        let (audio_tx, audio_rx) = mpsc::channel::<Vec<u8>>(AUDIO_QUEUE);
        let (events_tx, events_rx) = mpsc::channel::<SttEvent>(AUDIO_QUEUE);
        let (done_tx, done_rx) = oneshot::channel();
        let provider = shared.stt.clone();
        let liveness = shared.liveness.clone();
        tokio::spawn(async move {
            let outcome = provider.run_session(audio_rx, events_tx, liveness).await;
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

/// Drive one connection until it ends.
#[allow(clippy::too_many_arguments)]
pub async fn run_connection(
    ws: WebSocket,
    shared: Arc<VoiceIoShared>,
    session_id: String,
    mode: Mode,
    conn_id: u64,
    to_conn_tx: mpsc::Sender<ToConnection>,
    mut to_conn_rx: mpsc::Receiver<ToConnection>,
) {
    let (mut sink, mut stream) = ws.split();
    register(&shared, &session_id, conn_id, to_conn_tx, mode).await;

    let echo = shared.stt.name() == "echo";
    let input = shared.stt.input_format();
    let hello = ServerFrame::Hello {
        protocol: PROTOCOL,
        session_id: session_id.clone(),
        mode,
        audio_in: input,
        audio_out: audio_out(&shared),
        stt: shared.stt.name(),
        tts: tts_name(&shared),
        audio_out_frame_ms: audio_out_frame_ms(&shared),
        speak_plain: shared.speak_plain,
        release_grace_ms: shared.release_grace_ms,
    };
    if send_frame(&mut sink, &hello).await.is_err() {
        unregister(&shared, &session_id, conn_id).await;
        return;
    }

    // The echo path has no provider to lose, so it has no session to restart.
    let mut stt = if echo {
        None
    } else {
        Some(SttSession::start(&shared))
    };
    let mut stt_retried = false;
    let mut retry_at: Option<Instant> = None;
    let mut speaking: Option<Speaking> = None;
    // Beside `speaking` rather than inside it, so the deadline arm below can
    // read it while the chunk arm holds `speaking` mutably. `None` whenever
    // nothing is being spoken.
    let mut speak_deadline: Option<Instant> = None;
    let mut closing: Option<u16> = None;
    // R-V6': how many mis-framed binary frames this connection has sent. The
    // counter lives here because it is per connection and per socket, and it
    // is what makes a systematic mis-framing visible without this half having
    // to pick a threshold.
    let mut bad_frames: u32 = 0;
    let frame_bytes = input.frame_bytes();

    loop {
        tokio::select! {
            biased;

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
                        match start_speak(&shared, &mut sink, &session_id, speak_id, text).await {
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
                stt = Some(SttSession::start(&shared));
            }

            event = next_stt_event(&mut stt) => {
                match event {
                    Some(event) => {
                        // An event is a completed provider round trip (issue #7).
                        shared.liveness.mark_success();
                        shared.emit(VoiceEvent::Stt {
                            session_id: session_id.clone(),
                            event,
                        }).await;
                    }
                    // The session is over, one way or another. Both endings are
                    // the same problem for the client — nothing is being
                    // recognised any more — so both take the retry path.
                    None => {
                        let Some(session) = stt.take() else { break };
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
                let Some(Ok(message)) = incoming else { break };
                match message {
                    Message::Binary(bytes) => {
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
                            if sink.send(Message::Binary(bytes)).await.is_err() {
                                break;
                            }
                        } else if let Some(session) = stt.as_ref() {
                            // Blocks when the provider is behind — that is the
                            // backpressure, and it reaches the client as TCP.
                            if session.audio_tx.send(bytes).await.is_err() {
                                tracing::debug!(%session_id, "voice: the recognition session is gone");
                            }
                        }
                        // Between a lost session and its retry there is nowhere
                        // for audio to go; that window is the price of the one
                        // retry and is a second of it.
                    }
                    Message::Text(text) => {
                        if handle_text(&shared, &session_id, &mut sink, echo, &text)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => {}
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
        let _ = sink
            .send(Message::Close(Some(CloseFrame {
                code,
                reason: Cow::Borrowed(""),
            })))
            .await;
    }
    unregister(&shared, &session_id, conn_id).await;
}

/// One text frame from the client.
///
/// Returns `Err(())` when the socket is gone. An unparseable frame is answered
/// and the connection stays: a client that sent nonsense once can be told so.
async fn handle_text(
    shared: &Arc<VoiceIoShared>,
    session_id: &str,
    sink: &mut Sink,
    echo: bool,
    text: &str,
) -> Result<(), ()> {
    let frame = match meclaw_core::serde_json::from_str::<ClientFrame>(text) {
        Ok(f) => f,
        Err(e) => {
            let frame = ServerFrame::Error {
                code: WireErrorCode::BadFrame,
                detail: e.to_string(),
                bad_frames: None,
            };
            return send_frame(sink, &frame).await;
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
        return send_frame(sink, &frame).await;
    }
    // Everything else is the handler's call: it owns the turn machine and is
    // the only emitter.
    shared
        .emit(VoiceEvent::Control {
            session_id: session_id.to_string(),
            frame,
        })
        .await;
    Ok(())
}

/// Start one synthesis, or report at once why there is none.
async fn start_speak(
    shared: &Arc<VoiceIoShared>,
    sink: &mut Sink,
    session_id: &str,
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
    tokio::spawn(async move {
        let outcome = provider
            .synthesize(text, audio_tx, cancel_rx, liveness)
            .await;
        let _ = done_tx.send(outcome);
    });
    Ok(Some(Speaking {
        speak_id,
        cancel: cancel_tx,
        audio_rx,
        framer: Framer::new(audio_out(shared), audio_out_frame_ms(shared)),
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
    sink.send(Message::Text(text)).await.map_err(|_| ())
}

/// Send audio frames in order, stopping at the first socket that is gone.
async fn send_audio(sink: &mut Sink, frames: Vec<Vec<u8>>) -> Result<(), ()> {
    for frame in frames {
        sink.send(Message::Binary(frame)).await.map_err(|_| ())?;
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
