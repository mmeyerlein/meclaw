//! Speech-to-text over the OpenAI Realtime **transcription** session
//! (WebSocket). Wave voice-cell (2026-09-05), strand t2b.
//!
//! This adapter speaks to any OpenAI-compatible endpoint: `base_url` is used
//! verbatim as the base and `/v1/realtime` is appended to it, so a self-hosted
//! server that implements the same events (vLLM's `/v1/realtime`, for example)
//! is reached by pointing the param at it — there is no behaviour that keys off
//! the host (R-V10). An `http(s)://` base is rewritten to `ws(s)://`, which is
//! the only transformation applied.
//!
//! Only the transcription intent is modelled. Speech-to-speech — the session
//! that answers with audio and calls tools — is explicitly **not** part of the
//! `voice` cell; the wire protocol reserves `spoken` and `tool_call` for that
//! later composition and this file stays out of it.
//!
//! The wire form follows the current OpenAI documentation and two Apache-2.0
//! reference clients, which agree with each other:
//!
//! - <https://developers.openai.com/api/docs/guides/realtime-transcription>
//!   (session shape, `input_audio_buffer.append`/`.commit`, transcription
//!   delta/completed events),
//!   <https://developers.openai.com/api/docs/guides/realtime-websocket>
//!   (URL and `Authorization: Bearer`),
//!   <https://developers.openai.com/api/docs/guides/realtime-vad>
//!   (`server_vad`/`semantic_vad`, `input_audio_buffer.speech_started`).
//! - `src/pipecat/services/openai/stt.py` in pipecat-ai/pipecat (Apache-2.0):
//!   the exact `session.update` nesting read here is its shape, ported rather
//!   than copied — the framework around it (frames, resampling, settings
//!   deltas) is not. Its `?intent=transcription` query is **not** taken over:
//!   that is the beta form and the GA documentation does not carry it, the
//!   session type comes from `session.update` alone (R-V18).
//! - `livekit-plugins/livekit-plugins-openai/.../realtime/realtime_model.py` in
//!   livekit/agents (Apache-2.0): the `http → ws` rewrite of a configured base
//!   URL is its `process_base_url` idea, reduced to the one case this cell has.
//!
//! Server events this adapter deliberately ignores, because the cell has no
//! use for them: `session.created`, `session.updated`, `input_audio_buffer.
//! speech_stopped`, `input_audio_buffer.committed`, `input_audio_buffer.
//! timeout_triggered` and the `segment` events. They are not errors, they are
//! simply not turn boundaries.
//!
//! What is deliberately different from both references: this adapter never
//! resamples (R-V2 — the cell declares 24 kHz in `hello` and the client
//! adapts), it holds no reconnect policy of its own (the connection task owns
//! that, so one session here is one socket), and every send and every read
//! carries its own operation timeout (hard rule 12) rather than relying on the
//! far side to end the wait.

use crate::voice::contract::{
    AudioFormat, BoxFuture, ProviderTimeouts, SttError, SttEvent, SttProvider,
};
use crate::voice::params::OpenAiSttParams;
use futures_util::{SinkExt, StreamExt};
use meclaw_colony::io_liveness::IoLivenessMark;
use serde_json::{Value as JsonValue, json};
use std::collections::VecDeque;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// The transcription model this adapter defaults to: the current realtime
/// transcription model of the provider's own list, checked against the
/// official documentation on 2026-09-05. `gpt-4o-transcribe` is the July name
/// and stays here as fallback documentation (R-V4) — it is a params value with
/// a default, never a constant with meaning, and `params.model` is written to
/// the wire verbatim whatever it says.
pub const DEFAULT_MODEL: &str = "gpt-live-transcribe";

/// The endpoint this adapter defaults to. It is the base, not the URL: the
/// session path is appended to whatever stands here, so any OpenAI-compatible
/// server is reached by replacing it (R-V10).
pub const DEFAULT_BASE_URL: &str = "wss://api.openai.com";

/// Speech-to-text through an OpenAI-compatible Realtime transcription session.
///
/// One instance per cell, shared by every connection; each call to
/// `run_session` opens its own socket.
pub struct OpenAiTranscriptionStt {
    params: OpenAiSttParams,
    timeouts: ProviderTimeouts,
}

impl OpenAiTranscriptionStt {
    /// A provider configured by `params`, holding the default deadlines until
    /// [`Self::with_timeouts`] replaces them.
    pub fn new(params: OpenAiSttParams) -> Self {
        Self {
            params,
            timeouts: ProviderTimeouts::default(),
        }
    }

    /// Adopt the cell's `external_timeout_ms` / `provider_idle_timeout_ms`
    /// (R-V13). `external` bounds every single operation — handshake, session
    /// configuration, each audio append; `idle` is the longest silence
    /// tolerated between two server frames before the socket is presumed dead.
    pub fn with_timeouts(mut self, timeouts: ProviderTimeouts) -> Self {
        self.timeouts = timeouts;
        self
    }
}

impl SttProvider for OpenAiTranscriptionStt {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn input_format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(OpenAiSttParams::SAMPLE_RATE)
    }

    fn run_session(
        &self,
        _format: AudioFormat,
        audio: mpsc::Receiver<Vec<u8>>,
        events: mpsc::Sender<SttEvent>,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), SttError>> {
        let params = self.params.clone();
        let timeouts = self.timeouts;
        Box::pin(async move { run_session(params, timeouts, audio, events, liveness).await })
    }
}

/// One transcription session on one socket.
async fn run_session(
    params: OpenAiSttParams,
    timeouts: ProviderTimeouts,
    mut audio: mpsc::Receiver<Vec<u8>>,
    events: mpsc::Sender<SttEvent>,
    liveness: IoLivenessMark,
) -> Result<(), SttError> {
    let url = session_url(&params.base_url);
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| SttError::Connect(format!("base_url is not a websocket url: {e}")))?;
    // The credential exists only inside this header value. It is never logged,
    // never put in an error and never sent as a query parameter.
    let header = format!("Bearer {}", params.api_key.expose())
        .parse()
        .map_err(|_| SttError::Auth("authorization header could not be built".to_string()))?;
    request.headers_mut().insert("authorization", header);

    // A-timeout around the handshake (hard rule 12): a silent TCP peer must not
    // park this task with no operation-level guard.
    let ws =
        match tokio::time::timeout(timeouts.external, tokio_tungstenite::connect_async(request))
            .await
        {
            Err(_) => return Err(SttError::Timeout),
            Ok(Err(e)) => return Err(classify_connect(e)),
            Ok(Ok((ws, _resp))) => ws,
        };
    // The far side completed a handshake: a full external round trip.
    liveness.mark_success();
    let (mut write, mut read) = ws.split();

    let update = session_update(&params).to_string();
    match tokio::time::timeout(
        timeouts.external,
        write.send(WsMessage::Text(update.into())),
    )
    .await
    {
        Err(_) => return Err(SttError::Timeout),
        Ok(Err(e)) => return Err(SttError::Protocol(format!("session.update: {e}"))),
        Ok(Ok(())) => {}
    }

    // The service streams `delta` fragments and may have more than one item in
    // flight at a time, so the fragments accumulate PER ITEM. Mixing them into
    // one buffer would splice two speakers' turns into one transcript.
    let mut transcripts = Transcripts::default();
    // Until the service confirmed the configuration, a top-level `error` is the
    // verdict on that configuration and ends the session; afterwards the
    // session survived it and it is a warning (R-V17).
    let mut configured = false;
    let mut audio_open = true;

    // The idle deadline lives OUTSIDE the loop and is reset only by a frame
    // that actually arrived. Wrapping the read in `timeout(...)` inside the
    // `select!` would look identical and be useless: every time the audio arm
    // wins, the timeout future is dropped and rebuilt, so against a talking
    // client with a silent provider it would never fire — exactly the case it
    // exists for.
    let idle_deadline = tokio::time::sleep(timeouts.idle);
    tokio::pin!(idle_deadline);

    loop {
        tokio::select! {
            chunk = audio.recv(), if audio_open => match chunk {
                Some(bytes) => {
                    let frame = json!({
                        "type": "input_audio_buffer.append",
                        "audio": b64_encode(&bytes),
                    })
                    .to_string();
                    match tokio::time::timeout(
                        timeouts.external,
                        write.send(WsMessage::Text(frame.into())),
                    )
                    .await
                    {
                        Err(_) => return Err(SttError::Timeout),
                        Ok(Err(e)) => return Err(SttError::Closed(format!("audio send: {e}"))),
                        Ok(Ok(())) => {}
                    }
                }
                None => {
                    // The connection dropped its audio end. With endpointing
                    // switched off nobody else will ever close the buffer, so
                    // commit it by hand; with a VAD running, a hand-written
                    // commit would cut the turn the VAD is still measuring.
                    audio_open = false;
                    if commits_by_hand(&params) {
                        let commit = json!({ "type": "input_audio_buffer.commit" }).to_string();
                        let _ = tokio::time::timeout(
                            timeouts.external,
                            write.send(WsMessage::Text(commit.into())),
                        )
                        .await;
                    }
                    let _ = tokio::time::timeout(timeouts.external, write.close()).await;
                }
            },
            // Silence says nothing about the credentials, only about this
            // socket — the connection task decides whether to reconnect.
            () = &mut idle_deadline => {
                return Err(SttError::Closed(format!(
                    "idle for {:?}, no frame — socket presumed dead",
                    timeouts.idle
                )));
            }
            incoming = read.next() => {
                let msg = match incoming {
                    None => {
                        let _ = events.send(SttEvent::Closed).await;
                        return Ok(());
                    }
                    Some(Ok(m)) => m,
                    Some(Err(e)) => return Err(SttError::Closed(format!("ws read: {e}"))),
                };
                // A frame arrived: the socket is audibly alive, so the deadline
                // starts over — and only here.
                idle_deadline
                    .as_mut()
                    .reset(tokio::time::Instant::now() + timeouts.idle);
                // Every frame is proof this connection still carries traffic.
                liveness.mark_success();
                let text = match msg {
                    WsMessage::Text(t) => t.to_string(),
                    WsMessage::Close(_) => {
                        let _ = events.send(SttEvent::Closed).await;
                        return Ok(());
                    }
                    WsMessage::Ping(_)
                    | WsMessage::Pong(_)
                    | WsMessage::Binary(_)
                    | WsMessage::Frame(_) => continue,
                };
                match map_event(&text, &mut transcripts, &mut configured) {
                    Mapped::Emit(event) => {
                        // A full channel blocks: backpressure, never drop.
                        if events.send(event).await.is_err() {
                            // Nobody listens any more; the session is over.
                            return Ok(());
                        }
                    }
                    Mapped::Ignore => {}
                    Mapped::Fatal(e) => return Err(e),
                }
            }
        }
    }
}

/// The transcription endpoint for a configured base. `base_url` is the base and
/// nothing is inferred from its host (R-V10); `http(s)` is rewritten to `ws(s)`
/// so an operator can paste the same URL they use for the REST API. No query
/// parameter: the session type travels in `session.update` (R-V18).
fn session_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let base = match base.strip_prefix("https://") {
        Some(rest) => format!("wss://{rest}"),
        None => match base.strip_prefix("http://") {
            Some(rest) => format!("ws://{rest}"),
            None => base.to_string(),
        },
    };
    format!("{base}/v1/realtime")
}

/// The one configuration event of the session. `turn_detection: "none"` sends
/// an explicit `null`, which is how the service is told to endpoint nothing —
/// no turn ever ends by itself then, so only the `hold` mode makes sense with
/// it, and the buffer has to be committed by hand.
fn session_update(params: &OpenAiSttParams) -> JsonValue {
    let mut transcription = json!({ "model": params.model });
    if !params.language.is_empty() {
        // `languages`, an array: that is what the current transcription model
        // takes. The singular `language` belonged to the older models.
        transcription["languages"] = json!([params.language]);
    }
    json!({
        "type": "session.update",
        "session": {
            "type": "transcription",
            "audio": {
                "input": {
                    "format": { "type": "audio/pcm", "rate": OpenAiSttParams::SAMPLE_RATE },
                    "transcription": transcription,
                    "turn_detection": turn_detection(params),
                }
            }
        }
    })
}

/// Whether this session has to close the audio buffer itself — derived from the
/// very value `session_update` puts on the wire, so the two can never disagree.
/// With a VAD running the service decides where a turn ends and a hand-written
/// commit would cut the turn it is still measuring; with endpointing switched
/// off, nobody else ever closes the buffer.
fn commits_by_hand(params: &OpenAiSttParams) -> bool {
    turn_detection(params).is_null()
}

/// The `turn_detection` value of the session. `none` is the one spelling that
/// means "no endpointing" and becomes an explicit `null`; every other value is
/// passed through as the kind. `params.turn_detection` is validated in
/// `params.rs`, so no further spellings are invented here.
fn turn_detection(params: &OpenAiSttParams) -> JsonValue {
    match params.turn_detection.as_str() {
        "none" => JsonValue::Null,
        kind => json!({ "type": kind }),
    }
}

/// The deltas in flight, one buffer per item.
///
/// Two items can overlap on the wire, and a `completed` for one of them must
/// not touch the other's buffer — a single shared buffer spliced two turns
/// into one transcript. `done` remembers the recently finished items so a late
/// `delta` for an item the service already closed is dropped instead of
/// starting that item over.
#[derive(Default)]
struct Transcripts {
    open: VecDeque<(String, String)>,
    done: VecDeque<String>,
}

impl Transcripts {
    /// How many finished item ids are remembered. Enough to catch a late
    /// delta, small enough to never grow into a leak on a long session.
    const DONE_MEMORY: usize = 32;
    /// How many items may be in flight at once. A service that never closes an
    /// item must not be able to grow this map without end either, so the
    /// oldest buffer falls out — far more than the two or three a real
    /// conversation overlaps.
    const OPEN_MEMORY: usize = 8;

    /// Appends `delta` to `item`'s transcript and returns the whole of it.
    /// `None` when the item is already finished — that delta is stale.
    fn append(&mut self, item: &str, delta: &str) -> Option<String> {
        if self.done.iter().any(|id| id == item) {
            return None;
        }
        if let Some((_, buffer)) = self.open.iter_mut().find(|(id, _)| id == item) {
            buffer.push_str(delta);
            return Some(buffer.clone());
        }
        self.open.push_back((item.to_string(), delta.to_string()));
        while self.open.len() > Self::OPEN_MEMORY {
            self.open.pop_front();
        }
        Some(delta.to_string())
    }

    /// Marks `item` finished and forgets its buffer. Every other item keeps its
    /// own.
    fn finish(&mut self, item: &str) {
        self.open.retain(|(id, _)| id != item);
        if item.is_empty() {
            return;
        }
        self.done.push_back(item.to_string());
        while self.done.len() > Self::DONE_MEMORY {
            self.done.pop_front();
        }
    }
}

/// Every server event that proves the session is up and doing its work. The
/// first of them ends the configuration phase, after which a top-level `error`
/// is survivable rather than a verdict on the `session.update` we sent.
const SESSION_IS_RUNNING: [&str; 6] = [
    "session.updated",
    "input_audio_buffer.speech_started",
    "input_audio_buffer.committed",
    "conversation.item.input_audio_transcription.delta",
    "conversation.item.input_audio_transcription.completed",
    "conversation.item.input_audio_transcription.failed",
];

/// What one server event means for the session.
enum Mapped {
    /// Hand this on to the cell.
    Emit(SttEvent),
    /// A frame this adapter does not translate (`session.created`,
    /// `speech_stopped`, `committed`, `segment`, anything newer).
    Ignore,
    /// The session cannot continue.
    Fatal(SttError),
}

/// Maps one server event. `transcripts` holds the deltas in flight per item;
/// `configured` flips at the first sign that the session is actually running
/// and decides whether a top-level `error` is a verdict on the configuration
/// (fatal) or something the running session survived (a warning, R-V17).
///
/// That sign is deliberately not just `session.updated`: an OpenAI-compatible
/// server that never sends one, but does send transcripts, is running a
/// session all the same (R-V10), and every later error there would otherwise
/// kill a working stream.
///
/// `SttEvent::TurnResumed` has no counterpart here — this service never
/// withdraws an end of turn, so the variant is simply never produced, which the
/// contract explicitly allows.
fn map_event(text: &str, transcripts: &mut Transcripts, configured: &mut bool) -> Mapped {
    let Ok(event) = serde_json::from_str::<JsonValue>(text) else {
        // A malformed frame is not a reason to drop a healthy socket.
        tracing::warn!("openai stt: unparsable frame ignored");
        return Mapped::Ignore;
    };
    let item = event
        .get("item_id")
        .and_then(|i| i.as_str())
        .unwrap_or_default()
        .to_string();
    let kind = event
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or_default();
    if SESSION_IS_RUNNING.contains(&kind) {
        *configured = true;
    }
    match kind {
        "session.updated" => Mapped::Ignore,
        "input_audio_buffer.speech_started" => Mapped::Emit(SttEvent::SpeechStarted),
        "conversation.item.input_audio_transcription.delta" => {
            let delta = event
                .get("delta")
                .and_then(|d| d.as_str())
                .unwrap_or_default();
            if delta.is_empty() {
                return Mapped::Ignore;
            }
            match transcripts.append(&item, delta) {
                Some(text) => Mapped::Emit(SttEvent::Partial {
                    text,
                    // This service marks no transcript as preflight; `eager` is
                    // the Deepgram Flux notion and stays false here.
                    eager: false,
                }),
                None => Mapped::Ignore,
            }
        }
        "conversation.item.input_audio_transcription.completed" => {
            transcripts.finish(&item);
            Mapped::Emit(SttEvent::EndOfTurn {
                text: event
                    .get("transcript")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_string(),
            })
        }
        "conversation.item.input_audio_transcription.failed" => {
            // One item failed, not the session: the cell reports it and the
            // socket keeps running (R-V17).
            transcripts.finish(&item);
            Mapped::Emit(SttEvent::Warning {
                detail: error_message(&event),
            })
        }
        "error" => {
            if *configured {
                Mapped::Emit(SttEvent::Warning {
                    detail: error_message(&event),
                })
            } else {
                Mapped::Fatal(SttError::Protocol(error_message(&event)))
            }
        }
        _ => Mapped::Ignore,
    }
}

/// The human-readable half of an error event, without anything the service
/// echoed back.
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
fn classify_connect(error: WsError) -> SttError {
    match error {
        WsError::Http(response) => {
            let status = response.status().as_u16();
            if status == 401 || status == 403 {
                SttError::Auth(format!("http {status}"))
            } else {
                SttError::Connect(format!("http {status}"))
            }
        }
        other => SttError::Connect(other.to_string()),
    }
}

/// Standard-alphabet base64 with padding — what `input_audio_buffer.append`
/// carries. Written out because `base64` is not on the tech-stack allow-list
/// and this is the whole of what the protocol needs.
fn b64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GH #619: this adapter overrides neither negotiation method, so it gets
    /// the contract's default — one rate, its own. It is the only recogniser
    /// in the tree that cannot take a telephone call at its own rate, and
    /// `hello` says which rate it does speak rather than quietly resampling.
    #[test]
    fn openai_transcription_serves_exactly_one_rate() {
        let stt = OpenAiTranscriptionStt::new(params("wss://example", "server_vad"));
        assert_eq!(stt.input_rates(), vec![OpenAiSttParams::SAMPLE_RATE]);
        assert_eq!(
            stt.negotiate_input(OpenAiSttParams::SAMPLE_RATE),
            Some(AudioFormat::pcm16_mono(OpenAiSttParams::SAMPLE_RATE))
        );
        assert_eq!(
            stt.negotiate_input(8000),
            None,
            "8 kHz is refused, never resampled (R-V2)"
        );
    }

    fn params(base_url: &str, turn_detection: &str) -> OpenAiSttParams {
        serde_json::from_value(json!({
            "api_key": "test-key",
            "model": "gpt-live-transcribe",
            "language": "de",
            "turn_detection": turn_detection,
            "base_url": base_url,
        }))
        .expect("params parse")
    }

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
    fn default_model_is_the_current_transcription_model() {
        // R-V4: the name is a default, and this pins which one the strand
        // chose. The second half is what actually matters on the wire — a
        // configured model is written through verbatim, never overridden by
        // the constant.
        assert_eq!(DEFAULT_MODEL, "gpt-live-transcribe");
        assert_eq!(DEFAULT_BASE_URL, "wss://api.openai.com");
        let update = session_update(&params("wss://example", "server_vad"));
        assert_eq!(
            update["session"]["audio"]["input"]["transcription"]["model"],
            "gpt-live-transcribe"
        );
        let other = session_update(&OpenAiSttParams {
            model: "some-other-transcribe".to_string(),
            ..params("wss://example", "server_vad")
        });
        assert_eq!(
            other["session"]["audio"]["input"]["transcription"]["model"],
            "some-other-transcribe"
        );
    }

    #[test]
    fn session_url_appends_the_path_to_any_base() {
        assert_eq!(
            session_url("wss://api.openai.com"),
            "wss://api.openai.com/v1/realtime"
        );
        // An OpenAI-compatible server reached over plain http, trailing slash
        // and all (R-V10).
        assert_eq!(
            session_url("http://127.0.0.1:8000/"),
            "ws://127.0.0.1:8000/v1/realtime"
        );
        assert_eq!(
            session_url("https://gateway.example/openai"),
            "wss://gateway.example/openai/v1/realtime"
        );
    }

    #[test]
    fn session_update_carries_the_documented_nesting() {
        let update = session_update(&params("wss://example", "semantic_vad"));
        assert_eq!(update["type"], "session.update");
        assert_eq!(update["session"]["type"], "transcription");
        let input = &update["session"]["audio"]["input"];
        assert_eq!(input["format"]["type"], "audio/pcm");
        assert_eq!(input["format"]["rate"], 24_000);
        assert_eq!(input["transcription"]["model"], "gpt-live-transcribe");
        assert_eq!(input["transcription"]["languages"], json!(["de"]));
        assert_eq!(input["turn_detection"]["type"], "semantic_vad");
    }

    #[test]
    fn turn_detection_none_is_an_explicit_null() {
        let update = session_update(&params("wss://example", "none"));
        assert!(update["session"]["audio"]["input"]["turn_detection"].is_null());
    }

    fn delta_frame(item: &str, text: &str) -> String {
        json!({
            "type": "conversation.item.input_audio_transcription.delta",
            "item_id": item,
            "delta": text
        })
        .to_string()
    }

    fn completed_frame(item: &str, text: &str) -> String {
        json!({
            "type": "conversation.item.input_audio_transcription.completed",
            "item_id": item,
            "transcript": text
        })
        .to_string()
    }

    #[test]
    fn deltas_accumulate_into_the_partial_of_the_turn() {
        let mut t = Transcripts::default();
        let mut configured = false;
        match map_event(&delta_frame("i1", "hal"), &mut t, &mut configured) {
            Mapped::Emit(SttEvent::Partial { text, eager }) => {
                assert_eq!(text, "hal");
                assert!(!eager);
            }
            _ => panic!("expected a partial"),
        }
        match map_event(&delta_frame("i1", "lo"), &mut t, &mut configured) {
            Mapped::Emit(SttEvent::Partial { text, .. }) => assert_eq!(text, "hallo"),
            _ => panic!("expected a partial"),
        }
        match map_event(&completed_frame("i1", "hallo"), &mut t, &mut configured) {
            Mapped::Emit(SttEvent::EndOfTurn { text }) => assert_eq!(text, "hallo"),
            _ => panic!("expected an end of turn"),
        }
        assert!(t.open.is_empty(), "the item buffer must be gone");
    }

    #[test]
    fn two_items_in_flight_keep_separate_buffers() {
        let mut t = Transcripts::default();
        let mut configured = false;
        fn partial(m: Mapped) -> String {
            match m {
                Mapped::Emit(SttEvent::Partial { text, .. }) => text,
                _ => panic!("expected a partial"),
            }
        }
        assert_eq!(
            partial(map_event(&delta_frame("i1", "gu"), &mut t, &mut configured)),
            "gu"
        );
        assert_eq!(
            partial(map_event(&delta_frame("i2", "ha"), &mut t, &mut configured)),
            "ha"
        );
        assert_eq!(
            partial(map_event(
                &delta_frame("i1", "ten"),
                &mut t,
                &mut configured
            )),
            "guten"
        );
        // Finishing i1 leaves i2 untouched.
        map_event(&completed_frame("i1", "guten"), &mut t, &mut configured);
        assert_eq!(
            partial(map_event(
                &delta_frame("i2", "llo"),
                &mut t,
                &mut configured
            )),
            "hallo"
        );
    }

    #[test]
    fn a_delta_after_the_completed_of_its_item_is_dropped() {
        let mut t = Transcripts::default();
        let mut configured = false;
        map_event(&delta_frame("i1", "hal"), &mut t, &mut configured);
        map_event(&completed_frame("i1", "hallo"), &mut t, &mut configured);
        assert!(matches!(
            map_event(&delta_frame("i1", "lo"), &mut t, &mut configured),
            Mapped::Ignore
        ));
    }

    #[test]
    fn an_item_that_failed_is_a_warning_not_a_failure() {
        let mut t = Transcripts::default();
        let mut configured = false;
        let frame = json!({
            "type": "conversation.item.input_audio_transcription.failed",
            "item_id": "i1",
            "error": { "message": "audio too short" }
        })
        .to_string();
        match map_event(&frame, &mut t, &mut configured) {
            Mapped::Emit(SttEvent::Warning { detail }) => assert_eq!(detail, "audio too short"),
            _ => panic!("expected a warning"),
        }
    }

    #[test]
    fn a_top_level_error_before_the_session_was_confirmed_ends_it() {
        let mut t = Transcripts::default();
        let mut configured = false;
        let frame = json!({ "type": "error", "error": { "message": "bad model" } }).to_string();
        match map_event(&frame, &mut t, &mut configured) {
            Mapped::Fatal(SttError::Protocol(detail)) => assert_eq!(detail, "bad model"),
            _ => panic!("expected a fatal protocol error"),
        }
    }

    #[test]
    fn any_running_session_event_ends_the_configuration_phase() {
        // R-V10: a compatible server that never sends `session.updated` still
        // proves the session runs by transcribing.
        for evidence in [
            delta_frame("i1", "hal"),
            completed_frame("i1", "hallo"),
            r#"{"type":"input_audio_buffer.speech_started"}"#.to_string(),
            r#"{"type":"input_audio_buffer.committed"}"#.to_string(),
        ] {
            let mut t = Transcripts::default();
            let mut configured = false;
            map_event(&evidence, &mut t, &mut configured);
            assert!(configured, "{evidence} must end the configuration phase");
        }
    }

    #[test]
    fn an_item_buffer_falls_out_when_too_many_items_stay_open() {
        let mut t = Transcripts::default();
        let mut configured = false;
        for i in 0..=Transcripts::OPEN_MEMORY {
            map_event(&delta_frame(&format!("i{i}"), "x"), &mut t, &mut configured);
        }
        assert_eq!(t.open.len(), Transcripts::OPEN_MEMORY);
        assert!(
            !t.open.iter().any(|(id, _)| id == "i0"),
            "the oldest open buffer must fall out, not the newest"
        );
    }

    #[test]
    fn a_top_level_error_after_the_session_was_confirmed_is_a_warning() {
        let mut t = Transcripts::default();
        let mut configured = false;
        map_event(r#"{"type":"session.updated"}"#, &mut t, &mut configured);
        assert!(configured, "session.updated must confirm the session");
        let frame = json!({ "type": "error", "error": { "message": "unknown field" } }).to_string();
        match map_event(&frame, &mut t, &mut configured) {
            Mapped::Emit(SttEvent::Warning { detail }) => assert_eq!(detail, "unknown field"),
            _ => panic!("expected a warning"),
        }
    }

    #[test]
    fn unknown_and_malformed_frames_are_ignored() {
        let mut t = Transcripts::default();
        let mut configured = false;
        for frame in [
            r#"{"type":"input_audio_buffer.committed"}"#,
            r#"{"type":"input_audio_buffer.speech_stopped"}"#,
            r#"{"type":"input_audio_buffer.timeout_triggered"}"#,
            r#"{"type":"conversation.item.input_audio_transcription.segment"}"#,
            "not json",
        ] {
            assert!(
                matches!(map_event(frame, &mut t, &mut configured), Mapped::Ignore),
                "{frame} must be ignored"
            );
        }
    }

    #[test]
    fn only_a_session_without_endpointing_commits_by_hand() {
        assert!(commits_by_hand(&params("wss://example", "none")));
        assert!(!commits_by_hand(&params("wss://example", "server_vad")));
        assert!(!commits_by_hand(&params("wss://example", "semantic_vad")));
    }
}
