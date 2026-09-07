//! Text-to-speech over OpenAI's streaming speech endpoint — and, by R-V10, over
//! **any OpenAI-compatible `/v1/audio/speech` endpoint** (a self-hosted Kokoro
//! or Orpheus server speaks the same request and answers the same body). That
//! is why `base_url` is used verbatim: `<base_url>/v1/audio/speech`, with
//! `https://api.openai.com` as nothing more than the default. There is no
//! behaviour keyed on the host, and there never should be — the moment one host
//! gets a special case, the second endpoint stops being a configuration choice
//! and becomes a fork.
//!
//! The wire, per the vendor documentation
//! (<https://developers.openai.com/api/docs/guides/text-to-speech>,
//! <https://developers.openai.com/api/docs/api-reference/audio/createSpeech>):
//! `POST /v1/audio/speech` with a JSON body of `model`, `input`, `voice` and
//! `response_format`. With `response_format: "pcm"` the response body is raw
//! signed 16-bit little-endian mono samples at 24 kHz, headerless, delivered
//! with chunked transfer encoding so playback can start before synthesis ends.
//! That is exactly the format the cell puts on its own socket (R-V2), so the
//! cell never resamples and never decodes: the body bytes ARE the audio.
//!
//! Model and voice are params with defaults (`voice/params.rs`, R-V4), never
//! constants here. Current as of 2026-09-05: `gpt-4o-mini-tts` is the newest
//! general model and the only one of the three that accepts `instructions`
//! (dated snapshot `gpt-4o-mini-tts-2025-12-15`); fallback doc for the older
//! pair is `tts-1` (lower latency) and `tts-1-hd` (higher quality). The
//! built-in voices are `alloy`, `ash`, `ballad`, `cedar`, `coral`, `echo`,
//! `fable`, `marin`, `nova`, `onyx`, `sage`, `shimmer`, `verse`. None of that
//! is validated here: an OpenAI-compatible server carries its own names, and
//! its own rejection is the honest answer.
//!
//! `stream_format` is deliberately not sent. It is an OpenAI-specific extension
//! whose `sse` value wraps the audio in base64 inside `speech.audio.delta`
//! events; omitting it yields the plain audio body on OpenAI and on every
//! compatible server alike, which is the one shape R-V10 can rely on.
//! Reference implementations consulted for the wire handling (Apache-2.0 /
//! BSD-2-Clause, read for protocol shape, not copied): LiveKit Agents
//! `livekit-plugins-openai/livekit/plugins/openai/tts.py` — which documents that
//! compatible servers ignore `stream_format` and answer with the audio bytes —
//! and Pipecat `src/pipecat/services/openai/tts.py`, which asks for `pcm` at
//! 24 kHz and forwards `iter_bytes` chunks unaltered.

use crate::voice::contract::{AudioFormat, BoxFuture, ProviderTimeouts, TtsError, TtsProvider};
use crate::voice::params::{OpenAiTtsParams, Secret};
use meclaw_colony::io_liveness::IoLivenessMark;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

/// Default speech model, checked against the vendor model list on 2026-09-05:
/// `gpt-4o-mini-tts` is the newest general text-to-speech model and the only
/// one of the three that accepts `instructions` (dated snapshot at that time:
/// `gpt-4o-mini-tts-2025-12-15`). Fallback doc for the older pair, still
/// served: `tts-1` (lower latency) and `tts-1-hd` (higher quality).
/// `voice/params.rs` returns this as its `model` default (R-V4/R-V14).
pub const DEFAULT_MODEL: &str = "gpt-4o-mini-tts";

/// Default voice, checked 2026-09-05. The built-in set at that time: `alloy`,
/// `ash`, `ballad`, `cedar`, `coral`, `echo`, `fable`, `marin`, `nova`,
/// `onyx`, `sage`, `shimmer`, `verse`. The name is never validated here — an
/// OpenAI-compatible server carries its own voices, and its own rejection is
/// the honest answer.
pub const DEFAULT_VOICE: &str = "alloy";

/// Default endpoint base. Only a default: R-V10 makes every OpenAI-compatible
/// `/v1/audio/speech` server a first-class target, reached by setting
/// `base_url` and nothing else.
pub const DEFAULT_BASE_URL: &str = "https://api.openai.com";

/// The path appended to `base_url`. Identical on OpenAI and on every
/// OpenAI-compatible server; that sameness is the whole point of R-V10.
const SPEECH_PATH: &str = "/v1/audio/speech";

/// A text-to-speech provider speaking `POST <base_url>/v1/audio/speech`.
///
/// One instance per cell, shared by every connection: `synthesize` borrows
/// `&self` and owns nothing mutable, and the `reqwest::Client` pools
/// connections across calls, so a second synthesis usually skips the TLS
/// handshake entirely.
#[derive(Debug)]
pub struct OpenAiTts {
    /// The HTTP client, or why it could not be built. Keeping the failure
    /// instead of falling back to a default client means a misconfigured
    /// builder surfaces as a named error at the first synthesis rather than
    /// as a silently different client.
    client: Result<reqwest::Client, String>,
    url: String,
    /// Still a `Secret`: redaction is structural, not a habit. The value is
    /// exposed at exactly one call site, where it goes on the wire.
    api_key: Secret,
    model: String,
    voice: String,
    timeouts: ProviderTimeouts,
}

impl OpenAiTts {
    /// Builds the provider from its params sub-object.
    ///
    /// `base_url` is taken verbatim (one trailing slash tolerated) and
    /// [`SPEECH_PATH`] appended — a base carrying a path prefix, as a
    /// self-hosted server behind a mount usually does, keeps that prefix.
    pub fn new(params: OpenAiTtsParams) -> Self {
        // Defensive only: `build_tts` hands the cell's real budget straight to
        // [`Self::with_timeouts`] (R-V13). The contract default is the params
        // default, so a provider built in isolation still bounds its I/O
        // (hard rule 12) instead of waiting forever.
        let timeouts = ProviderTimeouts::default();
        Self {
            client: build_client(timeouts),
            url: format!("{}{SPEECH_PATH}", params.base_url.trim_end_matches('/')),
            api_key: params.api_key,
            model: params.model,
            voice: params.voice,
            timeouts,
        }
    }

    /// Applies the cell's timeout budget (R-V13). `external` bounds the request
    /// and the response head; `idle` bounds the gap between two body chunks —
    /// a stream that stops mid-sentence is a stall, not a slow synthesis, and
    /// only the second deadline can tell the two apart.
    pub fn with_timeouts(mut self, timeouts: ProviderTimeouts) -> Self {
        self.timeouts = timeouts;
        self.client = build_client(timeouts);
        self
    }
}

impl TtsProvider for OpenAiTts {
    fn name(&self) -> &'static str {
        "openai"
    }

    fn output_format(&self) -> AudioFormat {
        AudioFormat::pcm16_mono(OpenAiTtsParams::SAMPLE_RATE)
    }

    fn synthesize(
        &self,
        text: String,
        audio: mpsc::Sender<Vec<u8>>,
        cancel: watch::Receiver<bool>,
        liveness: IoLivenessMark,
    ) -> BoxFuture<Result<(), TtsError>> {
        let client = match &self.client {
            Ok(c) => c.clone(),
            Err(why) => {
                let why = why.clone();
                return Box::pin(async move {
                    Err(TtsError::Connect(format!("http client unavailable: {why}")))
                });
            }
        };
        let req = SpeechRequest {
            client,
            url: self.url.clone(),
            api_key: self.api_key.clone(),
            model: self.model.clone(),
            voice: self.voice.clone(),
            external: self.timeouts.external,
            idle: self.timeouts.idle,
        };
        Box::pin(async move { req.run(text, audio, cancel, liveness).await })
    }
}

/// Everything one synthesis needs, cloned out of the shared provider so the
/// returned future owns its inputs and borrows nothing.
#[derive(Debug)]
struct SpeechRequest {
    client: reqwest::Client,
    url: String,
    api_key: Secret,
    model: String,
    voice: String,
    external: Duration,
    idle: Duration,
}

impl SpeechRequest {
    async fn run(
        self,
        text: String,
        audio: mpsc::Sender<Vec<u8>>,
        mut cancel: watch::Receiver<bool>,
        liveness: IoLivenessMark,
    ) -> Result<(), TtsError> {
        if *cancel.borrow_and_update() {
            return Err(TtsError::Cancelled);
        }

        let body = serde_json::json!({
            "model": self.model,
            "input": text,
            "voice": self.voice,
            "response_format": "pcm",
        });

        // A-timeout (rule 12) around the request and the response head.
        let send = self
            .client
            .post(&self.url)
            // The one place the credential is exposed.
            .bearer_auth(self.api_key.expose())
            .json(&body)
            .send();
        let response = match tokio::time::timeout(self.external, send).await {
            Err(_elapsed) => return Err(TtsError::Timeout),
            Ok(Err(e)) => return Err(request_error(e)),
            Ok(Ok(r)) => r,
        };

        let status = response.status();
        if !status.is_success() {
            // Status only, never the response body: a self-hosted
            // OpenAI-compatible server is free to echo the request it rejected
            // — headers included — and an error string is exactly the place a
            // bearer token would then reappear. The status is what an operator
            // acts on anyway.
            let message = if status.as_u16() == 404 {
                // A 404 is almost always a `base_url` that points at a server
                // without this route. Naming the path helps; naming the host
                // would put configuration into an error string.
                format!("http 404 for {SPEECH_PATH} under the configured base_url")
            } else {
                format!("http {}", status.as_u16())
            };
            return Err(if status.as_u16() == 401 || status.as_u16() == 403 {
                TtsError::Auth(message)
            } else {
                TtsError::Protocol(message)
            });
        }
        // The head came back: the far side answered. That is a round trip.
        liveness.mark_success();

        // `Response::chunk` is the streaming read that needs no reqwest
        // `stream` feature: it yields each body chunk as it lands and `None`
        // when the body ended cleanly.
        let mut response = response;
        // The idle deadline is pinned OUTSIDE the loop and reset only when the
        // provider actually said something. Wrapping the read in
        // `timeout(idle, …)` inside the `select!` would look equivalent and is
        // not: every time another arm wins, that future is dropped and rebuilt,
        // so a client that merely writes to the cancel watch would keep
        // refreshing a deadline whose whole job is to notice that the far side
        // stopped talking.
        let idle_deadline = tokio::time::sleep(self.idle);
        tokio::pin!(idle_deadline);
        // A dropped watch SENDER is not a cancellation: it means no further
        // cancel signal can ever arrive, so the synthesis runs on. Only `true`
        // on the watch, or a dropped audio receiver, stops it. The flag also
        // retires that `select!` arm, because `changed()` on a closed channel
        // returns `Err` immediately and would otherwise spin the loop.
        let mut cancel_live = true;
        loop {
            let chunk = tokio::select! {
                biased;
                // Cancellation wins over a pending chunk: a barge-in must not
                // wait for the provider to finish the sentence it started.
                changed = cancel.changed(), if cancel_live => {
                    match changed {
                        Err(_sender_gone) => cancel_live = false,
                        Ok(()) => {
                            if *cancel.borrow() {
                                return Err(TtsError::Cancelled);
                            }
                        }
                    }
                    continue;
                }
                () = &mut idle_deadline => return Err(TtsError::Timeout),
                item = response.chunk() => match item {
                    Ok(None) => return Ok(()),
                    Err(e) => {
                        return Err(TtsError::Protocol(format!(
                            "audio stream broke: {}",
                            e.without_url()
                        )));
                    }
                    Ok(Some(bytes)) => bytes,
                },
            };
            if chunk.is_empty() {
                continue;
            }
            // Audio: the far side is alive, the clock starts over.
            idle_deadline
                .as_mut()
                .reset(tokio::time::Instant::now() + self.idle);
            liveness.mark_success();
            // One send future for this chunk, polled to completion across as
            // many `select!` passes as it takes. Re-creating it per pass would
            // re-allocate the `Vec` every time the other arm won; dropping out
            // with `continue` would swallow a chunk that already left the
            // response.
            let mut send = std::pin::pin!(audio.send(chunk.to_vec()));
            let sent = loop {
                tokio::select! {
                    biased;
                    changed = cancel.changed(), if cancel_live => {
                        match changed {
                            Err(_sender_gone) => cancel_live = false,
                            Ok(()) => {
                                if *cancel.borrow() {
                                    return Err(TtsError::Cancelled);
                                }
                            }
                        }
                    }
                    res = &mut send => break res,
                }
            };
            // A dropped receiver is the second cancellation channel of the
            // trait contract: nobody is listening, so stop synthesizing.
            if sent.is_err() {
                return Err(TtsError::Cancelled);
            }
            // A slow consumer is backpressure, not a stalled provider: the time
            // spent blocked on the mailbox must not count against the deadline.
            idle_deadline
                .as_mut()
                .reset(tokio::time::Instant::now() + self.idle);
        }
    }
}

/// Builds the HTTP client, keeping the reason on failure. No global request
/// timeout: that would cap the whole streaming response. Only the connect
/// phase is bounded here; the rest is bounded explicitly in `run`.
fn build_client(timeouts: ProviderTimeouts) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(timeouts.external)
        .build()
        .map_err(|e| format!("{}", e.without_url()))
}

/// Classifies a request-phase failure. `without_url` strips the endpoint from
/// the message — a self-hosted server may carry a credential in its query
/// string, and an error text is exactly where that would end up quoted.
fn request_error(e: reqwest::Error) -> TtsError {
    if e.is_timeout() {
        TtsError::Timeout
    } else {
        TtsError::Connect(format!("{}", e.without_url()))
    }
}
