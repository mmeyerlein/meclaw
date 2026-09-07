//! ElevenLabs text-to-speech over their streaming WebSocket — the third
//! `TtsProvider` of the `voice` cell (GH #591, 2026-09-06).
//!
//! # The wire
//!
//! Protocol from the official documentation
//! (<https://elevenlabs.io/docs/api-reference/text-to-speech/v-1-text-to-speech-voice-id-stream-input>
//! and the WebSocket guide
//! <https://elevenlabs.io/docs/eleven-api/guides/how-to/websockets/realtime-tts>,
//! both read 2026-09-06): connect to
//! `wss://api.elevenlabs.io/v1/text-to-speech/{voice_id}/stream-input` with
//! `model_id` and `output_format` in the query, then speak three messages —
//! an initialisation message, the text, and `{"text": ""}` to close the input
//! stream. The server answers with `{"audio": <base64>, "isFinal": false}`
//! frames and one `{"isFinal": true}`.
//!
//! The credential travels in the `xi-api-key` header. The documentation also
//! offers it as an `authorization` query parameter and as an `xi_api_key` field
//! inside the initialisation message; a key in a URL ends up in every transport
//! error message, and a key in a message body ends up in every wire log, so the
//! header wins on both counts.
//!
//! **The voice id is part of the PATH, not of a request body** — that is the
//! one structural difference from Cartesia, and it is why `params.voice` is
//! guarded in the parser (R-V5): an unsubstituted `${…}` would not be a wrong
//! voice, it would be a wrong URL.
//!
//! The server closes an idle socket on its own (`inactivity_timeout`, 20 s by
//! default, settable in the query). It cannot bite here: the whole turn and the
//! end-of-stream marker leave before the first read, so the socket is never
//! idle on the input side while a generation is owed. A close that arrives
//! before `isFinal` is therefore a real fault and is reported as one.
//!
//! # There is no cancel message
//!
//! Cartesia has `{"context_id":…,"cancel":true}`; this protocol has nothing of
//! the kind. The single-context `stream-input` endpoint documents exactly three
//! client messages, none of which retracts audio already asked for. So a cancel
//! here **closes the socket**: a WebSocket close frame, best effort, and the
//! generation dies with the connection. That is not a workaround, it is the
//! whole vocabulary the endpoint has.
//!
//! # References read before building (R-V11)
//!
//! Two Apache-2.0 implementations were read as protocol references; no code was
//! copied beyond the request/response schema, which is the vendor's:
//!
//! - `livekit-plugins/livekit-plugins-elevenlabs/livekit/plugins/elevenlabs/tts.py`
//!   (github.com/livekit/agents) — `xi-api-key` as the one authentication
//!   header, the `model_id`/`output_format`/`language_code` query, the
//!   `{"text": " ", "voice_settings": …}` opener, and reading `audio` (base64)
//!   and `isFinal` off each frame with an `error` field as the failure shape.
//! - `src/pipecat/services/elevenlabs/tts.py` (github.com/pipecat-ai/pipecat)
//!   — the same query construction, `base64.b64decode(msg["audio"])` per
//!   frame, `isFinal` as the end, and the deliberate explicit close of a
//!   context that is no longer wanted.
//!
//! **Deliberately different:** both references drive the *multi*-context
//! endpoint (`multi-stream-input`), pool one socket across turns and therefore
//! carry a `context_id` on every message, a `close_context` handshake and a
//! keep-alive timer. This adapter opens **one connection per `synthesize` call**
//! against the single-context `stream-input` endpoint and closes it afterwards
//! — the cell hands the provider one finished turn, so there is exactly one
//! context and nothing to multiplex; socket reuse is a later measurement
//! question, not a correctness one. Both references also stream sentence by
//! sentence with a chunk schedule; the whole turn is available here, so it
//! travels as one message and the end-of-stream marker flushes it. For the same
//! reason `generation_config.chunk_length_schedule` and `auto_mode` are not
//! sent: they only decide how long the server buffers text that is still
//! arriving, and here none is.
//!
//! # Models and rates (R-V4, R-V14)
//!
//! The model is a param with a default, never a constant with meaning. At build
//! time the vendor's model list (<https://elevenlabs.io/docs/models>, read
//! 2026-09-06) named `eleven_flash_v2_5` as the low-latency streaming model
//! (~75 ms) and `eleven_v3_conversational` as the expressive one (~280 ms);
//! `eleven_multilingual_v2` is the older high-quality model and
//! `eleven_turbo_v2_5` is documented as deprecated in favour of flash. Whatever
//! `params.model` says goes on the wire verbatim.

use crate::store::query::hamming::decode_base64;
use crate::voice::contract::{AudioFormat, BoxFuture, ProviderTimeouts, TtsError, TtsProvider};
use crate::voice::params::ElevenLabsParams;
use futures_util::{SinkExt, StreamExt};
use meclaw_colony::io_liveness::IoLivenessMark;
use serde_json::{Value as JsonValue, json};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::{ClientRequestBuilder, Error as WsError};

/// The default streaming model (R-V4, R-V14). The vendor's model list
/// (<https://elevenlabs.io/docs/models>, read 2026-09-06) names
/// `eleven_flash_v2_5` as the lowest-latency model (~75 ms) and recommends it
/// for real-time agents; `eleven_v3_conversational` (~280 ms) is the expressive
/// alternative and `eleven_multilingual_v2` the older high-quality one.
/// `eleven_turbo_v2_5` is listed as deprecated in favour of flash.
///
/// `params.rs` reads this constant for its serde default (R-V14). The adapter
/// never substitutes a model of its own.
pub const DEFAULT_MODEL: &str = "eleven_flash_v2_5";

/// The default endpoint host. A fake in the tests points `params.base_url` at
/// itself instead.
pub const DEFAULT_BASE_URL: &str = "wss://api.elevenlabs.io";

/// The default output rate, in hertz. 24 kHz is what the other two text-to-
/// speech adapters of this cell emit, so a colony that swaps providers keeps
/// the format it announced in `hello` (R-V2: the cell never resamples).
pub const DEFAULT_SAMPLE_RATE: u32 = 24000;

/// Every sample rate the vendor's `pcm_*` output formats offer, per
/// <https://elevenlabs.io/docs/api-reference/text-to-speech/convert> (read
/// 2026-09-06): `pcm_8000`, `pcm_16000`, `pcm_22050`, `pcm_24000`,
/// `pcm_32000`, `pcm_44100`, `pcm_48000`.
///
/// The list is here rather than in the parser because it is a property of the
/// wire this file speaks. Anything outside it is refused by `VoiceParams::parse`
/// with a message naming these values — a rate the vendor does not serve is a
/// cell that announces one format in `hello` and then never speaks, and that is
/// a configuration error, not a runtime one.
pub const SUPPORTED_SAMPLE_RATES: [u32; 7] = [8000, 16000, 22050, 24000, 32000, 44100, 48000];

/// The `output_format` value for a sample rate: `pcm_<rate>`, raw signed
/// 16-bit little-endian mono — byte-for-byte what the cell puts on its own
/// socket, so nothing is decoded and nothing is resampled (R-V2).
pub fn output_format(sample_rate: u32) -> String {
    format!("pcm_{sample_rate}")
}

/// ElevenLabs TTS. One instance per cell, shared by every connection; each
/// `synthesize` call owns its own WebSocket.
pub struct ElevenLabsTts {
    params: ElevenLabsParams,
    timeouts: ProviderTimeouts,
}

impl ElevenLabsTts {
    /// A provider for these params, with the contract's default deadlines
    /// (5 s per operation, 30 s of silence per socket).
    pub fn new(params: ElevenLabsParams) -> Self {
        Self {
            params,
            timeouts: ProviderTimeouts::default(),
        }
    }

    /// The same provider with the cell's own `external_timeout_ms` and
    /// `provider_idle_timeout_ms` (R-V13: `build_tts` passes these).
    pub fn with_timeouts(mut self, timeouts: ProviderTimeouts) -> Self {
        self.timeouts = timeouts;
        self
    }
}

impl TtsProvider for ElevenLabsTts {
    fn name(&self) -> &'static str {
        "elevenlabs"
    }

    fn output_format(&self) -> AudioFormat {
        // R-V2: whatever rate was ordered is what the client is told to expect.
        // The cell never resamples.
        AudioFormat::pcm16_mono(self.params.sample_rate)
    }

    fn synthesize(
        &self,
        text: String,
        audio: mpsc::Sender<Vec<u8>>,
        cancel: watch::Receiver<bool>,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), TtsError>> {
        let params = self.params.clone();
        let timeouts = self.timeouts;
        Box::pin(async move { run(params, timeouts, text, audio, cancel, liveness).await })
    }
}

/// One synthesis: connect, speak the input stream, read chunks until `isFinal`.
async fn run(
    params: ElevenLabsParams,
    timeouts: ProviderTimeouts,
    text: String,
    audio: mpsc::Sender<Vec<u8>>,
    mut cancel: watch::Receiver<bool>,
    liveness: IoLivenessMark,
) -> Result<(), TtsError> {
    let ProviderTimeouts { external, idle } = timeouts;

    let ws = match tokio::time::timeout(
        external,
        tokio_tungstenite::connect_async(client_request(&params)?),
    )
    .await
    {
        Err(_) => return Err(TtsError::Timeout),
        Ok(Err(e)) => return Err(connect_error(e)),
        Ok(Ok((ws, _response))) => ws,
    };
    // The handshake completed: a full external round trip.
    liveness.mark_success();

    let (mut write, mut read) = ws.split();
    // The documented three-message input stream. The whole turn is available,
    // so the text is one message rather than a sentence stream, and the
    // end-of-stream marker follows immediately — there is nothing to buffer for.
    for message in input_stream(&params, &text) {
        match tokio::time::timeout(external, write.send(WsMessage::Text(message.into()))).await {
            Err(_) => return Err(TtsError::Timeout),
            Ok(Err(e)) => return Err(TtsError::Protocol(format!("elevenlabs send: {e}"))),
            Ok(Ok(())) => {}
        }
    }

    // Two ways to be told to stop, and both must interrupt a stream that is
    // already flowing: the explicit `cancel` flag, and the audio receiver going
    // away (the connection task dropped it). Each is a future built once and
    // polled by reference, so the loop never rebuilds a waiter.
    let cancelled = async move {
        loop {
            if *cancel.borrow_and_update() {
                return;
            }
            if cancel.changed().await.is_err() {
                // Nobody can cancel any more; this arm must simply never fire.
                // R-V16: a dropped cancel SENDER is not a cancel.
                std::future::pending::<()>().await;
            }
        }
    };
    tokio::pin!(cancelled);
    let closed = audio.closed();
    tokio::pin!(closed);

    // One deadline, two phases. Until the FIRST frame this is an ordinary
    // A-timeout (`external`): the request is a bounded operation and the first
    // answer either comes or does not. From then on it is the idle deadline
    // (`idle`): a stream has no bounded length, only a longest tolerable
    // silence. Both live in one `sleep` pinned outside the loop, NOT in a
    // `timeout(…, read.next())` inside the `select!` — a timeout built inside
    // an arm is dropped and rebuilt whenever any other arm wins, which silently
    // restarts the clock and, worse, drops `read.next()` mid frame. Here
    // `read.next()` is only ever dropped on a path that returns.
    let deadline = tokio::time::sleep(external);
    tokio::pin!(deadline);

    loop {
        let received = tokio::select! {
            () = &mut cancelled => {
                return stop(&mut write, external).await;
            }
            () = &mut closed => {
                return stop(&mut write, external).await;
            }
            () = &mut deadline => {
                // No first frame within `external`, or no further frame within
                // `idle`. A provider that goes quiet reports as a timeout,
                // never as a stall.
                return Err(TtsError::Timeout);
            }
            r = read.next() => r,
        };
        // A frame arrived, whatever kind: the socket is alive. The clock
        // starts over — and from here on it is the idle deadline, because the
        // bounded first-answer operation is done.
        deadline.as_mut().reset(tokio::time::Instant::now() + idle);
        let frame = match received {
            // A socket that ends before `isFinal` is a broken synthesis, not a
            // clean one: `TtsError` has no `Closed`, and it should not — the
            // caller's only question is whether the audio is complete.
            None => return Err(TtsError::Protocol(closed_early())),
            Some(Err(e)) => return Err(TtsError::Protocol(format!("elevenlabs read: {e}"))),
            Some(Ok(frame)) => frame,
        };
        let payload = match frame {
            WsMessage::Text(t) => t,
            WsMessage::Close(_) => return Err(TtsError::Protocol(closed_early())),
            WsMessage::Ping(_)
            | WsMessage::Pong(_)
            | WsMessage::Binary(_)
            | WsMessage::Frame(_) => {
                continue;
            }
        };
        let value: JsonValue = serde_json::from_str(&payload)
            .map_err(|e| TtsError::Protocol(format!("elevenlabs frame is not JSON: {e}")))?;

        // A failure is a field on an otherwise ordinary frame, not a frame type
        // of its own — so it is checked before the audio, never after.
        if is_error(&value) {
            return Err(frame_error(&value));
        }
        if let Some(encoded) = value.get("audio").and_then(JsonValue::as_str) {
            let bytes = decode_base64(encoded)
                .map_err(|e| TtsError::Protocol(format!("elevenlabs audio: {e}")))?;
            // Every frame is proof the far side is answering — including one
            // whose `audio` is the empty string, which the vendor sends around
            // the end of a generation.
            liveness.mark_success();
            // But an empty chunk is not audio, and the cell's framing half
            // would have to reason about a zero-length one all the same. It
            // ends here rather than on the client's socket.
            if !bytes.is_empty() {
                tokio::select! {
                    () = &mut cancelled => {
                        return stop(&mut write, external).await;
                    }
                    sent = audio.send(bytes) => {
                        if sent.is_err() {
                            return stop(&mut write, external).await;
                        }
                    }
                }
            }
        }
        // The last frame may carry audio AND the end marker, so this is a
        // second `if` rather than an `else`.
        if value.get("isFinal").and_then(JsonValue::as_bool) == Some(true) {
            return Ok(());
        }
        // `alignment`, `normalizedAlignment` and a keep-alive with neither
        // field are frames this adapter has no use for; an unknown one is not
        // a failure.
    }
}

/// The message a socket that ended before `isFinal` deserves. One function so
/// the two call sites cannot drift apart.
fn closed_early() -> String {
    "elevenlabs closed the socket before isFinal".to_string()
}

/// Close the socket, then report the cancellation.
///
/// This protocol has no cancel message (see the module docs), so closing the
/// connection IS the cancel. It is best-effort by design: the reason we are
/// here is that nobody wants this audio any more, so a socket that cannot take
/// the close frame is not a second failure — the verdict stays `Cancelled`
/// either way.
async fn stop<S>(write: &mut S, external: Duration) -> Result<(), TtsError>
where
    S: SinkExt<WsMessage> + Unpin,
{
    let _ = tokio::time::timeout(external, write.send(WsMessage::Close(None))).await;
    Err(TtsError::Cancelled)
}

/// The handshake request: endpoint, model, output format, and the credential
/// in its header.
fn client_request(params: &ElevenLabsParams) -> Result<ClientRequestBuilder, TtsError> {
    let uri = endpoint(params)
        .parse()
        .map_err(|e| TtsError::Connect(format!("elevenlabs base_url is not a URL: {e}")))?;
    Ok(ClientRequestBuilder::new(uri).with_header("xi-api-key", params.api_key.expose()))
}

/// The full endpoint URL. The voice id is a path segment on this API; the
/// parser refuses one that could reshape the URL, so it travels verbatim.
fn endpoint(params: &ElevenLabsParams) -> String {
    format!(
        "{base}/v1/text-to-speech/{voice}/stream-input?model_id={model}&output_format={format}",
        base = params.base_url.trim_end_matches('/'),
        voice = params.voice,
        model = params.model,
        format = output_format(params.sample_rate),
    )
}

/// The three client messages of one synthesis, in order.
///
/// The opener carries the voice settings, because the documentation allows them
/// on the first message only; it is omitted from the JSON entirely when neither
/// knob is configured, so the vendor's own defaults stand rather than this
/// adapter's guess at them.
fn input_stream(params: &ElevenLabsParams, text: &str) -> Vec<String> {
    let mut initialise = json!({ "text": " " });
    let mut voice_settings = serde_json::Map::new();
    if let Some(stability) = params.stability {
        voice_settings.insert("stability".into(), json!(stability));
    }
    if let Some(similarity) = params.similarity_boost {
        voice_settings.insert("similarity_boost".into(), json!(similarity));
    }
    if !voice_settings.is_empty() {
        initialise["voice_settings"] = JsonValue::Object(voice_settings);
    }
    vec![
        initialise.to_string(),
        // "Should always end with a single space string" — the vendor's own
        // words about a text message, because the server concatenates what it
        // is sent and would otherwise glue this turn to the next token.
        json!({ "text": with_trailing_space(text) }).to_string(),
        // The end of the input stream. Everything buffered is generated now.
        json!({ "text": "" }).to_string(),
    ]
}

/// `text` with exactly the trailing space the wire wants, and no second one.
fn with_trailing_space(text: &str) -> String {
    if text.ends_with(' ') {
        text.to_string()
    } else {
        format!("{text} ")
    }
}

/// Whether a frame reports a failure rather than audio.
///
/// Two shapes, because the vendor documents neither. The plain one carries an
/// `error` field. The second carries only a `code` and/or a `message` and
/// nothing else — no audio, no end marker — and reading THAT as an unknown
/// frame would be the expensive mistake: the frame is skipped, no further one
/// comes, and a failure the provider spelled out arrives at the caller as an
/// idle timeout instead. A frame that carries payload is never an error, so an
/// audio frame with an alignment `message` beside it cannot be caught here.
fn is_error(value: &JsonValue) -> bool {
    if value.get("error").is_some_and(|e| !e.is_null()) {
        return true;
    }
    let carries_payload =
        value.get("audio").is_some_and(|a| !a.is_null()) || value.get("isFinal").is_some();
    let names_a_failure = value.get("code").is_some_and(|c| !c.is_null())
        || value.get("message").is_some_and(|m| !m.is_null());
    !carries_payload && names_a_failure
}

/// Classify a failed handshake. The key travels in a header, never in the URL,
/// so no transport error can carry it into this string.
fn connect_error(e: WsError) -> TtsError {
    match e {
        WsError::Http(response) => {
            let status = response.status();
            if status == 401 || status == 403 {
                TtsError::Auth(format!("elevenlabs refused the key (HTTP {status})"))
            } else {
                TtsError::Connect(format!("elevenlabs refused the upgrade (HTTP {status})"))
            }
        }
        other => TtsError::Connect(format!("elevenlabs connect: {other}")),
    }
}

/// Classify an error frame. The vendor documents no schema for these, so all
/// three shapes seen in the wild are read: a machine-readable `error`, a human
/// `message`, and a numeric `code` (the WebSocket close code, which is an HTTP
/// status for the authentication failures).
fn frame_error(value: &JsonValue) -> TtsError {
    let code = value
        .get("error")
        .and_then(JsonValue::as_str)
        .unwrap_or("unknown");
    let message = value
        .get("message")
        .or_else(|| value.get("detail"))
        .and_then(JsonValue::as_str)
        .unwrap_or("no message");
    let status = value
        .get("code")
        .or_else(|| value.get("status"))
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    if status == 401 || status == 403 {
        TtsError::Auth(format!("elevenlabs: {code}: {message}"))
    } else {
        TtsError::Protocol(format!("elevenlabs: {code}: {message}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> ElevenLabsParams {
        serde_json::from_value(json!({
            "api_key": "k",
            "voice": "v",
        }))
        .expect("params")
    }

    /// R-V14: the constant this file owns names the vendor's current streaming
    /// model, and the adapter never rewrites it — whatever `params.model` says
    /// goes on the wire verbatim.
    #[test]
    fn default_model_is_the_current_streaming_model() {
        assert_eq!(
            DEFAULT_MODEL, "eleven_flash_v2_5",
            "the vendor's model list of 2026-09-06 names eleven_flash_v2_5 as \
             the low-latency streaming model"
        );
        assert_eq!(DEFAULT_BASE_URL, "wss://api.elevenlabs.io");

        let mut p = params();
        p.model = "eleven_v3_conversational".to_string();
        assert!(
            endpoint(&p).contains("model_id=eleven_v3_conversational"),
            "the adapter sends the configured model verbatim, never a constant"
        );
    }

    #[test]
    fn the_endpoint_carries_voice_model_and_format() {
        let url = endpoint(&params());
        assert_eq!(
            url,
            "wss://api.elevenlabs.io/v1/text-to-speech/v/stream-input\
             ?model_id=eleven_flash_v2_5&output_format=pcm_24000"
        );
        assert!(
            !url.contains("api_key") && !url.contains("authorization"),
            "the credential is never a query parameter: {url}"
        );
    }

    #[test]
    fn a_base_url_keeps_its_prefix_and_loses_one_trailing_slash() {
        let mut p = params();
        p.base_url = "ws://127.0.0.1:9/".to_string();
        assert!(endpoint(&p).starts_with("ws://127.0.0.1:9/v1/text-to-speech/v/stream-input"));
    }

    #[test]
    fn output_format_names_the_rate() {
        assert_eq!(output_format(24000), "pcm_24000");
        assert_eq!(output_format(16000), "pcm_16000");
        for rate in SUPPORTED_SAMPLE_RATES {
            assert_eq!(output_format(rate), format!("pcm_{rate}"));
        }
        assert!(SUPPORTED_SAMPLE_RATES.contains(&DEFAULT_SAMPLE_RATE));
    }

    #[test]
    fn the_input_stream_is_opener_text_and_end_marker() {
        let msgs = input_stream(&params(), "hallo welt");
        assert_eq!(msgs.len(), 3);
        let opener: JsonValue = serde_json::from_str(&msgs[0]).expect("json");
        assert_eq!(opener["text"], " ");
        assert!(
            opener.get("voice_settings").is_none(),
            "no knob set means no voice_settings at all, so the vendor's \
             defaults stand"
        );
        let body: JsonValue = serde_json::from_str(&msgs[1]).expect("json");
        assert_eq!(
            body["text"], "hallo welt ",
            "the wire wants one trailing space"
        );
        let end: JsonValue = serde_json::from_str(&msgs[2]).expect("json");
        assert_eq!(end["text"], "");
    }

    #[test]
    fn voice_settings_travel_in_the_opener_only() {
        let mut p = params();
        p.stability = Some(0.4);
        p.similarity_boost = Some(0.8);
        let msgs = input_stream(&p, "hallo ");
        let opener: JsonValue = serde_json::from_str(&msgs[0]).expect("json");
        assert_eq!(opener["voice_settings"]["stability"], 0.4);
        assert_eq!(opener["voice_settings"]["similarity_boost"], 0.8);
        let body: JsonValue = serde_json::from_str(&msgs[1]).expect("json");
        assert!(body.get("voice_settings").is_none());
        assert_eq!(body["text"], "hallo ", "an existing space is not doubled");
    }

    #[test]
    fn an_unauthorized_error_frame_is_an_auth_error() {
        let frame = json!({ "error": "unauthorized", "message": "bad key", "code": 401 });
        assert!(is_error(&frame));
        assert!(matches!(frame_error(&frame), TtsError::Auth(_)));
        let other = json!({ "error": "model_not_found", "message": "nope", "code": 400 });
        assert!(matches!(frame_error(&other), TtsError::Protocol(_)));
        assert!(
            !is_error(&json!({ "audio": "AAAA", "isFinal": false })),
            "an ordinary audio frame is not an error"
        );
    }

    /// The shape with no `error` field: nothing but a code and a message. It
    /// must not fall through to "unknown frame", or a named failure would
    /// reach the caller as an idle timeout.
    #[test]
    fn a_frame_with_only_a_code_and_a_message_is_a_failure() {
        let bare = json!({ "code": 1008, "message": "policy violation" });
        assert!(is_error(&bare));
        assert!(matches!(frame_error(&bare), TtsError::Protocol(_)));
        assert!(is_error(&json!({ "code": 401, "message": "bad key" })));
        assert!(matches!(
            frame_error(&json!({ "code": 401, "message": "bad key" })),
            TtsError::Auth(_)
        ));
        // Payload wins: a frame that carries audio or the end marker is never
        // an error, whatever else it says.
        assert!(!is_error(&json!({ "audio": "AAAA", "message": "aligned" })));
        assert!(!is_error(
            &json!({ "audio": JsonValue::Null, "isFinal": true })
        ));
        // And a frame that says nothing at all is still only an unknown frame.
        assert!(!is_error(&json!({ "alignment": { "chars": ["a"] } })));
    }
}
