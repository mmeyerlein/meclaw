//! Cartesia text-to-speech over their TTS WebSocket — the first `TtsProvider`
//! of the `voice` cell (R-V1). Wave voice-cell, strand t3a (2026-09-05).
//!
//! # The wire
//!
//! Protocol from the official documentation
//! (<https://docs.cartesia.ai/api-reference/tts/tts>, read 2026-09-05):
//! connect to `wss://api.cartesia.ai/tts/websocket`, send one JSON object per
//! synthesis and read `chunk` frames whose `data` is Base64 audio in the
//! requested `output_format`, terminated by exactly one `done`. A failure
//! arrives as an `error` frame; `{"context_id":…,"cancel":true}` stops a
//! generation that is still running.
//!
//! Credentials travel in the `X-API-Key` header and the API version in
//! `Cartesia-Version`. Both also exist as `api_key`/`cartesia_version` query
//! parameters — the documented alternative "for use in the browser, where
//! WebSockets do not support headers". This adapter is not a browser, and a key
//! in a URL ends up in every transport error message, so the headers win.
//!
//! # References read before building (R-V11)
//!
//! Two Apache-2.0 implementations were read as protocol references; no code was
//! copied beyond the request/response schema, which is the vendor's, not
//! theirs:
//!
//! - `livekit-plugins/livekit-plugins-cartesia/livekit/plugins/cartesia/tts.py`
//!   (github.com/livekit/agents, Apache-2.0) — the `voice{mode:"id",id}` shape,
//!   `generation_config{speed,emotion}` for the sonic-3 family, and the
//!   deliberate refusal to let a credential reach an exception string.
//! - `src/pipecat/services/cartesia/tts.py` (github.com/pipecat-ai/pipecat,
//!   Apache-2.0) — `X-API-Key`/`Cartesia-Version` as headers, the
//!   `chunk`/`done`/`timestamps`/`flush_done`/`error` frame split, and the
//!   cancel message on an interrupted context.
//!
//! # Models (R-V1, R-V4)
//!
//! The model is a param with a default, never a constant with meaning. At build
//! time the vendor's list (<https://docs.cartesia.ai/build-with-cartesia/tts-models/latest>,
//! read 2026-09-05) named `sonic-3.6` as the current streaming model —
//! `sonic-3.5` is the documented fallback and is what the July voice stack ran
//! (with `emotion: "content"`, `speed: 1.0`). Both take the same request shape;
//! `sonic-3.5` is documented as fully forwards compatible with `sonic-3.6`.
//!
//! Deliberately different here: both references pool one socket across many
//! contexts and therefore have to filter incoming frames by `context_id`. This
//! adapter opens **one connection per `synthesize` call** and closes it
//! afterwards — simple and provably free of cross-turn bleed; reuse is a later
//! measurement question, not a correctness one. Both references also stream a
//! sentence at a time with `continue:true`; the cell hands the provider one
//! finished turn, so a single `continue:false` request is the whole input.

use crate::store::query::hamming::decode_base64;
use crate::voice::contract::{AudioFormat, BoxFuture, ProviderTimeouts, TtsError, TtsProvider};
use crate::voice::params::CartesiaParams;
use futures_util::{SinkExt, StreamExt};
use meclaw_colony::io_liveness::IoLivenessMark;
use meclaw_core::Uuid;
use serde_json::{Value as JsonValue, json};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::{ClientRequestBuilder, Error as WsError};

/// The API version this adapter is written against, sent as `Cartesia-Version`.
///
/// It is a protocol constant, not a tuning knob: it decides how requests and
/// responses are shaped, so an operator changing it would change the wire this
/// code parses. `generation_config` (speed/emotion) exists only from a version
/// newer than `2025-04-16`; older versions carried the same controls inside
/// `voice.__experimental_controls`, which this adapter does not implement.
const CARTESIA_VERSION: &str = "2026-08-14";

/// The default streaming model (R-V1, R-V4). The vendor's model list
/// (<https://docs.cartesia.ai/build-with-cartesia/tts-models/latest>, read
/// 2026-09-05) names `sonic-3.6` as the current production model; `sonic-3.5`
/// is listed there as legacy and is the documented fallback — fully backwards
/// compatible, and what the July voice stack ran.
///
/// `params.rs` is *expected* to read this constant for its serde default
/// (R-V14, assigned to strand t1); until that lands its own default still
/// names `sonic-3.5`. Either way the adapter never substitutes a model of its
/// own: whatever `params.model` says goes on the wire verbatim.
pub const DEFAULT_MODEL: &str = "sonic-3.6";

/// The default endpoint host. A fake in the tests points `params.base_url` at
/// itself instead.
pub const DEFAULT_BASE_URL: &str = "wss://api.cartesia.ai";

/// Cartesia TTS. One instance per cell, shared by every connection; each
/// `synthesize` call owns its own WebSocket.
pub struct CartesiaTts {
    params: CartesiaParams,
    timeouts: ProviderTimeouts,
}

impl CartesiaTts {
    /// A provider for these params, with the contract's default deadlines
    /// (5 s per operation, 30 s of silence per socket).
    pub fn new(params: CartesiaParams) -> Self {
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

impl TtsProvider for CartesiaTts {
    fn name(&self) -> &'static str {
        "cartesia"
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

/// One synthesis: connect, send the request, stream chunks until `done`.
async fn run(
    params: CartesiaParams,
    timeouts: ProviderTimeouts,
    text: String,
    audio: mpsc::Sender<Vec<u8>>,
    mut cancel: watch::Receiver<bool>,
    liveness: IoLivenessMark,
) -> Result<(), TtsError> {
    let ProviderTimeouts { external, idle } = timeouts;
    let context_id = Uuid::now_v7().to_string();
    let request = build_request(&params, &text, &context_id);

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
    match tokio::time::timeout(
        external,
        write.send(WsMessage::Text(request.to_string().into())),
    )
    .await
    {
        Err(_) => return Err(TtsError::Timeout),
        Ok(Err(e)) => return Err(TtsError::Protocol(format!("cartesia send: {e}"))),
        Ok(Ok(())) => {}
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
                return stop(&mut write, &context_id, external).await;
            }
            () = &mut closed => {
                return stop(&mut write, &context_id, external).await;
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
            // A socket that ends before `done` is a broken synthesis, not a
            // clean one: `TtsError` has no `Closed`, and it should not — the
            // caller's only question is whether the audio is complete.
            None => {
                return Err(TtsError::Protocol(
                    "cartesia closed the socket before done".into(),
                ));
            }
            Some(Err(e)) => return Err(TtsError::Protocol(format!("cartesia read: {e}"))),
            Some(Ok(frame)) => frame,
        };
        let payload = match frame {
            WsMessage::Text(t) => t,
            WsMessage::Close(_) => {
                return Err(TtsError::Protocol(
                    "cartesia closed the socket before done".into(),
                ));
            }
            WsMessage::Ping(_)
            | WsMessage::Pong(_)
            | WsMessage::Binary(_)
            | WsMessage::Frame(_) => {
                continue;
            }
        };
        let value: JsonValue = serde_json::from_str(&payload)
            .map_err(|e| TtsError::Protocol(format!("cartesia frame is not JSON: {e}")))?;

        match value.get("type").and_then(JsonValue::as_str) {
            Some("chunk") => {
                let encoded = value
                    .get("data")
                    .and_then(JsonValue::as_str)
                    .ok_or_else(|| TtsError::Protocol("cartesia chunk without data".into()))?;
                let bytes = decode_base64(encoded)
                    .map_err(|e| TtsError::Protocol(format!("cartesia chunk: {e}")))?;
                // Every chunk is proof the far side is answering.
                liveness.mark_success();
                tokio::select! {
                    () = &mut cancelled => {
                        return stop(&mut write, &context_id, external).await;
                    }
                    sent = audio.send(bytes) => {
                        if sent.is_err() {
                            return stop(&mut write, &context_id, external).await;
                        }
                    }
                }
            }
            Some("done") => return Ok(()),
            Some("error") => return Err(frame_error(&value)),
            // `timestamps`, `phoneme_timestamps` and `flush_done` are frames
            // this adapter never asks for; an unknown one is not a failure.
            _ => continue,
        }
    }
}

/// Tell Cartesia to drop this context, then report the cancellation.
///
/// The cancel is best-effort by design: the reason we are here is that nobody
/// wants this audio any more, so a socket that cannot take the message is not a
/// second failure — the verdict stays `Cancelled` either way.
async fn stop<S>(write: &mut S, context_id: &str, external: Duration) -> Result<(), TtsError>
where
    S: SinkExt<WsMessage> + Unpin,
{
    let frame = json!({ "context_id": context_id, "cancel": true }).to_string();
    let _ = tokio::time::timeout(external, write.send(WsMessage::Text(frame.into()))).await;
    Err(TtsError::Cancelled)
}

/// The handshake request: endpoint plus the two authentication headers.
fn client_request(params: &CartesiaParams) -> Result<ClientRequestBuilder, TtsError> {
    let url = format!("{}/tts/websocket", params.base_url.trim_end_matches('/'));
    let uri = url
        .parse()
        .map_err(|e| TtsError::Connect(format!("cartesia base_url is not a URL: {e}")))?;
    Ok(ClientRequestBuilder::new(uri)
        .with_header("X-API-Key", params.api_key.expose())
        .with_header("Cartesia-Version", CARTESIA_VERSION))
}

/// The synthesis request for one whole turn.
fn build_request(params: &CartesiaParams, text: &str, context_id: &str) -> JsonValue {
    let mut request = json!({
        "context_id": context_id,
        "model_id": params.model,
        "transcript": text,
        // R-V5: the voice is always the param, never a literal.
        "voice": { "mode": "id", "id": params.voice },
        "output_format": {
            "container": "raw",
            "encoding": "pcm_s16le",
            "sample_rate": params.sample_rate,
        },
        "language": params.language,
        // One request per turn, so there is never a continuation.
        "continue": false,
    });
    let mut generation_config = serde_json::Map::new();
    if let Some(speed) = params.speed {
        generation_config.insert("speed".into(), json!(speed));
    }
    if let Some(emotion) = &params.emotion {
        generation_config.insert("emotion".into(), json!(emotion));
    }
    if !generation_config.is_empty() {
        request["generation_config"] = JsonValue::Object(generation_config);
    }
    request
}

/// Classify a failed handshake. The key travels in a header, never in the URL,
/// so no transport error can carry it into this string.
fn connect_error(e: WsError) -> TtsError {
    match e {
        WsError::Http(response) => {
            let status = response.status();
            if status == 401 || status == 403 {
                TtsError::Auth(format!("cartesia refused the key (HTTP {status})"))
            } else {
                TtsError::Connect(format!("cartesia refused the upgrade (HTTP {status})"))
            }
        }
        other => TtsError::Connect(format!("cartesia connect: {other}")),
    }
}

/// Classify an `error` frame. Cartesia names both a machine code and a status.
fn frame_error(value: &JsonValue) -> TtsError {
    let code = value
        .get("error_code")
        .and_then(JsonValue::as_str)
        .unwrap_or("unknown");
    let message = value
        .get("message")
        .or_else(|| value.get("title"))
        .and_then(JsonValue::as_str)
        .unwrap_or("no message");
    let status = value
        .get("status_code")
        .and_then(JsonValue::as_u64)
        .unwrap_or(0);
    if status == 401 || status == 403 {
        TtsError::Auth(format!("cartesia: {code}: {message}"))
    } else {
        TtsError::Protocol(format!("cartesia: {code}: {message}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> CartesiaParams {
        serde_json::from_value(json!({
            "api_key": "k",
            "voice": "v",
        }))
        .expect("params")
    }

    /// R-V14: the constant this strand owns names the vendor's current
    /// streaming model, and the adapter never rewrites it — whatever
    /// `params.model` says goes on the wire verbatim. Whether `params.rs`
    /// actually reads the constant for its serde default is t1's half of
    /// R-V14 and is pinned by t1's own unit tests, not here.
    #[test]
    fn default_model_is_the_current_streaming_model() {
        assert_eq!(
            DEFAULT_MODEL, "sonic-3.6",
            "the vendor's model list of 2026-09-05 names sonic-3.6 as current; \
             sonic-3.5 is the legacy fallback"
        );
        assert_eq!(DEFAULT_BASE_URL, "wss://api.cartesia.ai");

        let mut p = params();
        p.model = "sonic-3.5".to_string();
        assert_eq!(
            build_request(&p, "hallo", "c1")["model_id"],
            "sonic-3.5",
            "the adapter sends the configured model verbatim, never a constant"
        );
    }

    #[test]
    fn request_carries_the_documented_shape() {
        let req = build_request(&params(), "hallo", "c1");
        assert_eq!(req["context_id"], "c1");
        assert_eq!(req["transcript"], "hallo");
        assert_eq!(req["voice"]["mode"], "id");
        assert_eq!(req["voice"]["id"], "v");
        assert_eq!(req["output_format"]["container"], "raw");
        assert_eq!(req["output_format"]["encoding"], "pcm_s16le");
        assert_eq!(req["continue"], false);
        assert!(
            req.get("generation_config").is_none(),
            "no controls set means no generation_config at all"
        );
    }

    #[test]
    fn speed_and_emotion_land_in_generation_config() {
        let mut p = params();
        p.speed = Some(1.2);
        p.emotion = Some("calm".to_string());
        let req = build_request(&p, "hallo", "c1");
        assert_eq!(req["generation_config"]["speed"], 1.2);
        assert_eq!(req["generation_config"]["emotion"], "calm");
    }

    #[test]
    fn an_unauthorized_error_frame_is_an_auth_error() {
        let frame = json!({
            "type": "error", "error_code": "unauthorized",
            "message": "bad key", "status_code": 401,
        });
        assert!(matches!(frame_error(&frame), TtsError::Auth(_)));
        let other = json!({
            "type": "error", "error_code": "model_not_found",
            "message": "nope", "status_code": 400,
        });
        assert!(matches!(frame_error(&other), TtsError::Protocol(_)));
    }
}
