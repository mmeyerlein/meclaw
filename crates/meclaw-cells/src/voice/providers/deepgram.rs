//! Deepgram Flux as a [`SttProvider`] — the turn-based streaming API, where the
//! service itself decides where a turn ends instead of handing out silence
//! timers.
//!
//! # The wire, in one paragraph
//!
//! One WebSocket per session against `wss://api.deepgram.com/v2/listen`, the
//! whole configuration in the query string, the credential in an
//! `Authorization: Token <key>` header. Audio goes up as binary frames in the
//! declared format; the service answers with JSON text frames: one `Connected`
//! on open, then `TurnInfo` messages whose `event` field walks the turn state
//! machine — `StartOfTurn`, `Update`, `EagerEndOfTurn`, `TurnResumed`,
//! `EndOfTurn`. `{"type":"CloseStream"}` from the client ends the stream.
//!
//! Sources: <https://developers.deepgram.com/reference/speech-to-text/listen-flux>
//! (endpoint, query parameters, message schemas),
//! <https://developers.deepgram.com/docs/flux/state> (the turn state machine),
//! <https://developers.deepgram.com/docs/flux/configuration> (threshold ranges
//! and defaults, and which model `language_hint` belongs to),
//! <https://developers.deepgram.com/docs/keyterm> (keyterm prompting: the
//! repeated parameter, the casing rule and the 500-token limit),
//! <https://developers.deepgram.com/docs/flux/language-prompting> (which model
//! `language_hint` is legal on, and what the others answer). Read alongside
//! two Apache-2.0 implementations of the same protocol, as the wave's ruling
//! R-V11 asks: LiveKit's
//! `livekit-plugins-deepgram/livekit/plugins/deepgram/stt_v2.py`
//! (<https://github.com/livekit/agents>, Apache-2.0) and pipecat's
//! `src/pipecat/services/deepgram/flux/stt.py`
//! (<https://github.com/pipecat-ai/pipecat>, BSD-2-Clause). Nothing is copied —
//! what is taken is the reading of the protocol: which query keys the service
//! actually wants, that `EagerEndOfTurn` is a preflight rather than a turn end,
//! and that a failed handshake must never be formatted with its request
//! attached, because the request carries the key (LiveKit issue #6739).
//!
//! # Two things the references do that this adapter deliberately does not
//!
//! **Waiting for `Connected` before streaming.** pipecat holds audio back
//! until the `Connected` message arrives, with a bound, because an endpoint
//! that rejects a connection parameter answers with neither a `Connected` nor
//! an error. Here the session streams as soon as the socket is up: the audio
//! comes from a live client and buffering it means dropping it, and the idle
//! deadline already bounds a service that says nothing at all. The cost is a
//! misconfigured query failing as an idle close rather than as a fast error.
//!
//! **Sending silence to keep a turn alive.** pipecat runs a watchdog that
//! injects silence when the application stops feeding audio after a
//! `StartOfTurn`, because Flux then never closes the turn. The cell's audio is
//! a continuous stream from a WebSocket client, and its silence is real
//! silence the client sent; when the client stops entirely, the session ends
//! with `CloseStream` rather than being kept artificially alive.
//!
//! # Model names are params, not constants (R-V4)
//!
//! Flux ships two models: `flux-general-en` (English only) and
//! `flux-general-multi` (multilingual, the only one that reads `language_hint`).
//! The cell's default language is German, so the default model is
//! `flux-general-multi`; `flux-general-en` is the one to set for an
//! English-only deployment, and the adapter then leaves `language_hint` off,
//! because every model but the multilingual one answers a request carrying it
//! with `400 INVALID_PARAMETER`. Both names are the list as of 2026-09; the
//! field is a param precisely so a new name does not need a release.

use crate::voice::contract::{
    AudioFormat, BoxFuture, ProviderTimeouts, SttError, SttEvent, SttProvider,
};
use crate::voice::params::DeepgramParams;
use futures_util::{SinkExt, StreamExt};
use meclaw_colony::io_liveness::IoLivenessMark;
use serde_json::Value as JsonValue;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// The Flux streaming model this adapter defaults to.
///
/// Checked against Deepgram's own model list on 2026-09-05, which names
/// exactly two Flux models: `flux-general-en` (English only) and
/// `flux-general-multi` (multilingual, and the only one that reads
/// `language_hint`). The cell's default language is German, so the
/// multilingual model is the default — which is also the fallback name the
/// wave's plan carried from July, so nothing moved under us.
/// Source: <https://developers.deepgram.com/reference/speech-to-text/listen-flux>.
pub const DEFAULT_MODEL: &str = "flux-general-multi";

/// Scheme and host of the real service. The path (`/v2/listen`) and the whole
/// query are the adapter's business, so a fake only has to replace this.
pub const DEFAULT_BASE_URL: &str = "wss://api.deepgram.com";

/// What the client sends to end the stream.
const CLOSE_STREAM: &str = r#"{"type":"CloseStream"}"#;

/// Speech-to-text through Deepgram Flux. One instance per cell; every
/// connection gets its own WebSocket from [`SttProvider::run_session`].
pub struct DeepgramFluxStt {
    params: DeepgramParams,
    timeouts: ProviderTimeouts,
}

impl DeepgramFluxStt {
    /// Build the adapter from its parsed params sub-object. The cell holds it
    /// to its own deadlines right afterwards through [`Self::with_timeouts`].
    pub fn new(params: DeepgramParams) -> Self {
        Self {
            params,
            timeouts: ProviderTimeouts::default(),
        }
    }

    /// Hold this adapter to the cell's two deadlines (hard rule 12):
    /// `external` wraps every single socket operation, `idle` is how long a
    /// live session may stay silent before it counts as dead.
    pub fn with_timeouts(mut self, t: ProviderTimeouts) -> Self {
        self.timeouts = t;
        self
    }

    /// The full session URL, query and all. `base_url` is the scheme and host
    /// only (`wss://api.deepgram.com`), so a test can point it at a fake.
    fn session_url(&self) -> String {
        let p = &self.params;
        let mut url = format!(
            "{}/v2/listen?model={}&encoding=linear16&sample_rate={}&eot_threshold={}&eager_eot_threshold={}&eot_timeout_ms={}",
            p.base_url.trim_end_matches('/'),
            encode(&p.model),
            p.sample_rate,
            p.eot_threshold,
            p.eager_eot_threshold,
            p.eot_timeout_ms,
        );
        // `language_hint` is Flux's language bias, and Deepgram states it as an
        // ALLOWLIST: "`language_hint` is only supported on `flux-general-multi`.
        // Sending it to any other model (including `flux-general-en`) returns a
        // `400` error" -- `INVALID_PARAMETER`. So the test is the `-multi`
        // suffix and not the `-en` one, because the two are not each other's
        // opposite: a model name this build has never heard of then loses the
        // bias, where a denylist would have lost the whole request. It is
        // deliberately the only string test in this function, and it decides
        // whether a parameter is legal for the configured model, never what the
        // configured value is (R-V4).
        if !p.language.is_empty() {
            if p.model.ends_with("-multi") {
                url.push_str("&language_hint=");
                url.push_str(&encode(&p.language));
            } else {
                tracing::debug!(
                    model = %p.model,
                    language = %p.language,
                    "deepgram: language_hint omitted, only flux-general-multi takes it"
                );
            }
        }
        // Keyterm prompting: one `keyterm` parameter per term, repeated, which
        // is how the service reads a list. Percent-encoding is what keeps a
        // multi-word term one term -- the space becomes `%20` rather than
        // splitting the phrase into two boosts. Surrounding whitespace is
        // trimmed, because a config file is written by hand and a leading space
        // is a typo rather than part of a name; a blank entry would boost
        // nothing and is dropped instead of travelling as `keyterm=`. Deepgram
        // caps the whole list at 500 tokens per request and refuses anything
        // longer -- this adapter does not count, it just sends what it is
        // handed.
        for term in &p.keyterms {
            let term = term.trim();
            if term.is_empty() {
                continue;
            }
            url.push_str("&keyterm=");
            url.push_str(&encode(term));
        }
        url
    }
}

/// Percent-encode one query value.
///
/// The values here are model names, language codes and numbers, so in practice
/// nothing needs escaping — but they come from a config file, and a config
/// value that silently reshapes the query is how an injection starts. Only the
/// RFC 3986 unreserved set survives unescaped; everything else becomes `%XX`.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

impl SttProvider for DeepgramFluxStt {
    fn name(&self) -> &'static str {
        "deepgram"
    }

    fn input_format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(self.params.sample_rate)
    }

    fn run_session(
        &self,
        audio: mpsc::Receiver<Vec<u8>>,
        events: mpsc::Sender<SttEvent>,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), SttError>> {
        let url = self.session_url();
        let credential = format!("Token {}", self.params.api_key.expose());
        let external = self.timeouts.external;
        let idle = self.timeouts.idle;
        Box::pin(
            async move { session(url, credential, external, idle, audio, events, liveness).await },
        )
    }
}

/// One Flux session: connect, pump audio up, map messages down.
async fn session(
    url: String,
    credential: String,
    external: Duration,
    idle: Duration,
    mut audio: mpsc::Receiver<Vec<u8>>,
    events: mpsc::Sender<SttEvent>,
    liveness: IoLivenessMark,
) -> Result<(), SttError> {
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| SttError::Connect(format!("bad deepgram url: {e}")))?;
    let value = credential
        .parse()
        .map_err(|_| SttError::Auth("credential is not a valid header value".to_string()))?;
    request.headers_mut().insert("authorization", value);

    let stream = match timeout(external, tokio_tungstenite::connect_async(request)).await {
        Err(_) => return Err(SttError::Timeout),
        Ok(Err(e)) => return Err(classify_handshake(e)),
        Ok(Ok((stream, _response))) => stream,
    };
    let (mut write, mut read) = stream.split();

    // `closing` flips once the audio side is done: from then on the client has
    // sent `CloseStream` and only drains what the service still has to say.
    let mut closing = false;

    // The deadline measures the RECEIVING side and nothing else. It is pinned
    // outside the loop and reset only when a frame actually arrived, because a
    // deadline rebuilt each iteration is reset by every outgoing chunk too —
    // and then a live client keeps a mute Flux socket looking healthy forever,
    // which is exactly the stall this deadline exists to end.
    let deadline = tokio::time::sleep(idle);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            chunk = audio.recv(), if !closing => match chunk {
                Some(bytes) => {
                    send(&mut write, WsMessage::Binary(bytes.into()), external).await?;
                }
                None => {
                    send(&mut write, WsMessage::Text(CLOSE_STREAM.into()), external).await?;
                    closing = true;
                    // Draining after our own `CloseStream` is a single
                    // operation, so it gets the operation timeout, not the
                    // idle deadline.
                    deadline.as_mut().reset(Instant::now() + external);
                }
            },
            () = &mut deadline => {
                // A silent live session is dead; a silent CLOSING session
                // simply said everything it had to say.
                return if closing {
                    Ok(())
                } else {
                    Err(SttError::Closed("idle".to_string()))
                };
            }
            received = read.next() => {
                let message = match received {
                    None => return closed(&events, closing).await,
                    Some(Err(e)) => return Err(SttError::Protocol(format!("deepgram read failed: {e}"))),
                    Some(Ok(m)) => m,
                };
                liveness.mark_success();
                deadline
                    .as_mut()
                    .reset(Instant::now() + if closing { external } else { idle });
                match message {
                    WsMessage::Text(text) => {
                        if !dispatch(&text, &events).await? {
                            return Ok(());
                        }
                    }
                    WsMessage::Close(_) => return closed(&events, closing).await,
                    // Flux answers in JSON; binary and control frames carry
                    // nothing this adapter needs.
                    _ => {}
                }
            }
        }
    }
}

/// Report a socket the service closed, then end the session.
async fn closed(events: &mpsc::Sender<SttEvent>, closing: bool) -> Result<(), SttError> {
    // After our own `CloseStream` the close is the expected answer, not news.
    if !closing {
        let _ = events.send(SttEvent::Closed).await;
    }
    Ok(())
}

/// Send one frame under the operation timeout (AGENTS rule 12).
async fn send<S>(sink: &mut S, message: WsMessage, external: Duration) -> Result<(), SttError>
where
    S: SinkExt<WsMessage> + Unpin,
    <S as futures_util::Sink<WsMessage>>::Error: std::fmt::Display,
{
    match timeout(external, sink.send(message)).await {
        Err(_) => Err(SttError::Timeout),
        Ok(Err(e)) => Err(SttError::Closed(format!("deepgram write failed: {e}"))),
        Ok(Ok(())) => Ok(()),
    }
}

/// Map one server message onto the provider contract. `Ok(false)` means the
/// consumer went away and the session is over.
async fn dispatch(text: &str, events: &mpsc::Sender<SttEvent>) -> Result<bool, SttError> {
    let value: JsonValue = serde_json::from_str(text)
        .map_err(|e| SttError::Protocol(format!("deepgram sent invalid json: {e}")))?;
    let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match kind {
        "TurnInfo" => {
            let transcript = value
                .get("transcript")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let event = value.get("event").and_then(|v| v.as_str()).unwrap_or("");
            for mapped in map_turn_event(event, transcript) {
                if events.send(mapped).await.is_err() {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        // `Error` is fatal: the service says so and stops transcribing. Both
        // fields are the service's own text, never the request — the code is
        // what an operator can look up, the description what they can read.
        "Error" => {
            let code = value
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("UNKNOWN");
            let description = value
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("deepgram reported an error");
            Err(SttError::Protocol(format!("[{code}] {description}")))
        }
        // `Connected`, `ConfigureSuccess`, `ConfigureFailure` and anything the
        // service grows later are not events of this contract.
        _ => Ok(true),
    }
}

/// The turn state machine, as the contract sees it.
///
/// `StartOfTurn` is two things at once — speech began, and there is already a
/// first transcript — so it maps to two events. `EagerEndOfTurn` is the
/// preflight: the service thinks the turn is probably over but has not
/// committed, which is exactly what `Partial { eager: true }` means. A
/// `TurnResumed` withdraws that guess.
fn map_turn_event(event: &str, transcript: String) -> Vec<SttEvent> {
    match event {
        "StartOfTurn" => {
            let mut out = vec![SttEvent::SpeechStarted];
            if !transcript.is_empty() {
                out.push(SttEvent::Partial {
                    text: transcript,
                    eager: false,
                });
            }
            out
        }
        // Flux emits an `Update` about every 250 ms of audio, and the ones
        // before the first word carry an empty transcript. Forwarding those
        // would flood the partial lane with nothing.
        "Update" if transcript.is_empty() => Vec::new(),
        "Update" => vec![SttEvent::Partial {
            text: transcript,
            eager: false,
        }],
        "EagerEndOfTurn" => vec![SttEvent::Partial {
            text: transcript,
            eager: true,
        }],
        "TurnResumed" => vec![SttEvent::TurnResumed],
        "EndOfTurn" => vec![SttEvent::EndOfTurn { text: transcript }],
        _ => Vec::new(),
    }
}

/// Turn a failed handshake into a contract error — status only.
///
/// The error is deliberately built from the status code and nothing else. A
/// tungstenite handshake error can carry the response, and formatting a whole
/// handshake failure is how another implementation leaked the API key into its
/// logs (LiveKit issue #6739); a credential has no business in an error type.
fn classify_handshake(error: tokio_tungstenite::tungstenite::Error) -> SttError {
    if let tokio_tungstenite::tungstenite::Error::Http(response) = &error {
        let status = response.status();
        if status == 401 || status == 403 {
            return SttError::Auth(format!("deepgram refused the credential ({status})"));
        }
        return SttError::Connect(format!("deepgram handshake failed ({status})"));
    }
    // Every other variant is a transport or protocol failure that never saw
    // the request headers, so its own text is safe and useful to keep.
    SttError::Connect(format!("deepgram handshake failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_of_turn_reports_speech_and_a_first_partial() {
        assert_eq!(
            map_turn_event("StartOfTurn", "hi".to_string()),
            vec![
                SttEvent::SpeechStarted,
                SttEvent::Partial {
                    text: "hi".to_string(),
                    eager: false
                }
            ]
        );
    }

    #[test]
    fn an_empty_start_of_turn_is_only_speech_started() {
        assert_eq!(
            map_turn_event("StartOfTurn", String::new()),
            vec![SttEvent::SpeechStarted]
        );
    }

    #[test]
    fn eager_end_of_turn_is_an_eager_partial() {
        assert_eq!(
            map_turn_event("EagerEndOfTurn", "done".to_string()),
            vec![SttEvent::Partial {
                text: "done".to_string(),
                eager: true
            }]
        );
    }

    /// Flux sends an `Update` roughly every 250 ms; before the first word they
    /// carry an empty transcript and must not reach the partial lane.
    #[test]
    fn an_empty_update_is_not_a_partial() {
        assert_eq!(map_turn_event("Update", String::new()), Vec::new());
    }

    #[test]
    fn a_query_value_is_percent_encoded() {
        assert_eq!(encode("flux-general-multi"), "flux-general-multi");
        assert_eq!(encode("de"), "de");
        assert_eq!(encode("a b&c=d"), "a%20b%26c%3Dd");
    }

    /// Params with a keyterm list; everything else stays at its default so
    /// the assertions below see only what the list changed.
    fn params_with_keyterms(keyterms: serde_json::Value) -> DeepgramParams {
        serde_json::from_value(serde_json::json!({
            "api_key": "not-a-real-key",
            "keyterms": keyterms,
        }))
        .expect("params parse")
    }

    /// The service reads a list of keyterms as a repeated parameter, not as one
    /// comma-joined value, so the adapter appends one `keyterm=` per term.
    #[test]
    fn keyterms_are_repeated_query_parameters() {
        let url = DeepgramFluxStt::new(params_with_keyterms(serde_json::json!(["Egon", "meclaw"])))
            .session_url();
        assert!(
            url.ends_with("&keyterm=Egon&keyterm=meclaw"),
            "each term is its own parameter, in order: {url}"
        );
        assert_eq!(url.matches("keyterm=").count(), 2, "{url}");
    }

    /// An empty list is the default, and it must leave the query byte-for-byte
    /// what it was before keyterms existed.
    #[test]
    fn no_keyterms_leaves_the_query_untouched() {
        let bare = DeepgramFluxStt::new(params_with_keyterms(serde_json::json!([]))).session_url();
        assert!(!bare.contains("keyterm"), "{bare}");
    }

    /// A blank entry would boost nothing; it is dropped rather than sent as an
    /// empty parameter the service has to reject.
    #[test]
    fn an_empty_keyterm_is_skipped() {
        let url = DeepgramFluxStt::new(params_with_keyterms(serde_json::json!([
            "", "   ", " Egon "
        ])))
        .session_url();
        assert!(
            url.ends_with("&keyterm=Egon"),
            "only the real term travels, trimmed rather than encoded as %20Egon%20: {url}"
        );
        assert_eq!(url.matches("keyterm=").count(), 1, "{url}");
    }

    /// A phrase is one boost, not two: the space is percent-encoded, so the
    /// parameter stays whole on the wire.
    #[test]
    fn a_keyterm_with_spaces_stays_one_parameter() {
        let url = DeepgramFluxStt::new(params_with_keyterms(serde_json::json!(["Ada Lovelace"])))
            .session_url();
        assert!(
            url.ends_with("&keyterm=Ada%20Lovelace"),
            "the space is encoded, the term is not split: {url}"
        );
        assert_eq!(url.matches("keyterm=").count(), 1, "{url}");
    }

    #[test]
    fn an_unknown_event_maps_to_nothing() {
        assert_eq!(map_turn_event("SomethingNew", "x".to_string()), Vec::new());
    }

    /// The model name is a param with a default, never a constant with
    /// meaning (R-V4): this pins which name the default is, and that the
    /// adapter puts whatever it was handed on the wire verbatim.
    #[test]
    fn default_model_is_the_current_streaming_model() {
        assert_eq!(DEFAULT_MODEL, "flux-general-multi");
        assert_eq!(DEFAULT_BASE_URL, "wss://api.deepgram.com");

        let params: DeepgramParams = serde_json::from_value(serde_json::json!({
            "api_key": "not-a-real-key",
            "base_url": DEFAULT_BASE_URL,
        }))
        .expect("params parse");
        let url = DeepgramFluxStt::new(params).session_url();
        assert!(
            url.starts_with("wss://api.deepgram.com/v2/listen?model=flux-general-multi&"),
            "the configured model goes on the wire verbatim: {url}"
        );
        assert!(
            url.ends_with("&language_hint=de"),
            "the multilingual model takes the language bias: {url}"
        );
    }

    /// `language_hint` is legal on `flux-general-multi` and on nothing else:
    /// every other model answers `400 INVALID_PARAMETER`. The rule is an
    /// allowlist, so the hint stays home for a monolingual model AND for a name
    /// this build has never seen — the query keeps everything else, the model
    /// name included.
    #[test]
    fn language_hint_is_omitted_for_a_monolingual_model() {
        for model in ["flux-general-en", "flux-something-nobody-has-shipped-yet"] {
            let params: DeepgramParams = serde_json::from_value(serde_json::json!({
                "api_key": "not-a-real-key",
                "model": model,
                "language": "en",
            }))
            .expect("params parse");
            let url = DeepgramFluxStt::new(params).session_url();
            assert!(
                url.contains(&format!("model={model}")),
                "the configured model still goes on the wire verbatim: {url}"
            );
            assert!(
                !url.contains("language_hint"),
                "only flux-general-multi takes the language bias: {url}"
            );
        }
    }
}
