//! The `voice` cell's params: where it listens, how it ends a turn, and which
//! two providers it speaks to.
//!
//! Model names and thresholds are **params with defaults**, never constants
//! with meaning (R-V4): a colony that wants another model says so in its
//! config, and nothing in this tree has to be rebuilt for it. And the default
//! lives with the adapter (R-V4/R-V14): the model name and the endpoint of a
//! provider are that provider's own knowledge, so this file asks it rather
//! than keeping a second copy that drifts.
//!
//! Credentials arrive as `${VAR}` and are substituted before a cell is spawned.
//! An unresolved one is refused here, by NAME — the value never appears in a
//! message, a log, or a `Debug` rendering (see [`Secret`]).

use crate::params_overlay::OverlayParams;
use crate::voice::wire::Mode;
use meclaw_core::JsonValue;

/// The highest port number there is. Named rather than inlined so the refusal
/// message and the check cannot drift apart.
const MAX_PORT: u64 = 65535;

/// Default outbound frame length in milliseconds, and the reason it is 20.
///
/// A telephony edge is the strictest reader of this wire: FreeSWITCH's
/// `mod_audio_stream` (1.0.3) aborts the whole call — a `SIGABRT` in its
/// playback half — as soon as a single binary frame carries more than about
/// 100 ms of audio. 20 ms is the frame length every softphone and every RTP
/// stack on that path already works in, so it is safe by two decades of
/// practice rather than by one measurement.
pub const DEFAULT_AUDIO_OUT_FRAME_MS: u32 = 20;

/// Whether the cell turns a written answer into speech text before it is
/// synthesised, unless a config says otherwise.
///
/// `true`, because an assistant writes markdown without being asked to and a
/// speech provider reads it out character by character — the first live call of
/// this cell had Cartesia pronouncing the stars around a bolded word. The knob
/// exists for the one case where the default is wrong: a colony whose answers
/// are already plain text and which would rather spend nothing on the check.
pub const DEFAULT_SPEAK_PLAIN: bool = true;

/// How long a released `hold` boundary waits for the provider's own end of
/// turn before it cuts with what it has, unless a config says otherwise.
///
/// A recognition provider reports the end of a turn some time after the audio
/// carrying its last words was sent — Deepgram Flux takes 400-700 ms, and the
/// number is a property of the model and the endpointing threshold, not of this
/// cell. `release` used to cut on the frame, which dropped exactly those words
/// from every take. 1500 ms is that measurement with room over it: long enough
/// that the cap is the exception rather than the rule, short enough that a
/// provider which has gone quiet costs a person one and a half seconds, once.
/// `0` restores the old behaviour for a client that would rather have the last
/// words missing than the turn late.
pub const DEFAULT_RELEASE_GRACE_MS: u64 = 1500;

/// The longest grace this cell will wait. Beyond this a turn boundary is not
/// waiting for a provider any more, it is stuck — and the bound is here so that
/// `15000` typed for `1500` is refused by name rather than felt as a call that
/// went silent for fifteen seconds.
const MAX_RELEASE_GRACE_MS: u64 = 10_000;

/// The longest frame this cell will cut. A "frame" of more than a second is a
/// buffer, not a frame, and every edge measured so far refuses one long before
/// this — the bound is here so a typo (`2000` for `20`) is refused by name
/// rather than sent at a phone.
const MAX_AUDIO_OUT_FRAME_MS: u64 = 1000;

/// Every key a `voice` cell's params may name. Anything else is a typo, and a
/// typo that is silently ignored is a setting an operator believes in.
const KNOWN_PARAMS_KEYS: &[&str] = &[
    "port",
    "bind",
    "default_mode",
    "barge_in",
    "emit_partials",
    "emit_speak_end",
    "external_timeout_ms",
    "provider_idle_timeout_ms",
    "audio_out_frame_ms",
    "speak_plain",
    "release_grace_ms",
    "stt",
    "tts",
];

/// Everything a `voice` cell needs to serve one WebSocket endpoint.
#[derive(Debug, Clone)]
pub struct VoiceParams {
    /// The TCP port this instance owns. Required, and `0` is refused — the same
    /// reason the `web` cell gives: `0` means "assign me anything", and a
    /// client cannot be told in advance where the cell went.
    pub port: u16,
    /// The address to bind. Loopback by default: the cell has no auth story,
    /// so its default must not be reachable off-host.
    pub bind: String,
    /// The mode a connection starts in when its URL carries no `?mode=`.
    pub default_mode: Mode,
    /// Whether `SpeechStarted` cancels a running synthesis (`auto` mode only).
    pub barge_in: bool,
    /// Whether interim transcripts are emitted on the `partial` lane.
    ///
    /// **Off by default** (R-V8'). Whoever listens orders it, with
    /// `override_params: {"emit_partials": true}`; without a listener every
    /// interim would dead-letter visibly at the channel container, and a colony
    /// would be reading its own noise. `false` changes nothing the client sees:
    /// the `partial` frame on the socket is a separate path and always travels.
    pub emit_partials: bool,
    /// Whether the end of a synthesis is announced on the `speak_end` lane.
    ///
    /// **Off by default**, the same rule the `partial` lane runs under (R-V8'):
    /// the emission goes to this cell's own path and the out-edges decide, so
    /// without an edge that drains it every synthesis would dead-letter
    /// visibly. Whoever listens orders it — a telephony hive that has to wait
    /// for the sentence to finish before it hangs the line up sets
    /// `emit_speak_end: true` in `override_params`, in the same breath as the
    /// edge that carries the lane.
    pub emit_speak_end: bool,
    /// Operation-timeout (hard rule 12, A) around every provider I/O.
    pub external_timeout_ms: u64,
    /// Idle deadline per provider socket; elapsing is the reconnect path, never
    /// a standstill.
    pub provider_idle_timeout_ms: u64,
    /// How much audio one outbound binary frame may carry, in milliseconds.
    ///
    /// The cell cuts every synthesis chunk into frames of at most this length
    /// before it sends them — an upper bound, not a fixed size; `0` sends each
    /// chunk as the provider produced it.
    /// See [`DEFAULT_AUDIO_OUT_FRAME_MS`] for why the default is 20 and not
    /// "whatever the provider chose".
    pub audio_out_frame_ms: u32,
    /// Whether an assistant turn is turned into speech text before synthesis.
    ///
    /// **On by default** ([`DEFAULT_SPEAK_PLAIN`]). The handler runs
    /// [`crate::voice::speech_text::to_speech`] over the text of an `in_speak`
    /// before it enters the session's queue, so markdown an assistant wrote for
    /// a screen — emphasis, headings, list markers, links, code fences, table
    /// pipes — never reaches a provider that would read it aloud. `false` hands
    /// the text over exactly as it arrived.
    pub speak_plain: bool,
    /// How long a released `hold` boundary waits for the provider's own
    /// `EndOfTurn` before it cuts with what it has, in milliseconds.
    ///
    /// `release` says that no NEW audio belongs to the turn; the provider still
    /// owes the end of the audio already sent, and this is how long the cell
    /// waits for it. `0` cuts on the frame — the behaviour before this param
    /// existed. See [`DEFAULT_RELEASE_GRACE_MS`] for where 1500 comes from.
    pub release_grace_ms: u64,
    /// The speech-to-text provider and its settings.
    pub stt: SttParams,
    /// The text-to-speech provider and its settings. `None` is only allowed
    /// with [`SttParams::Echo`], which speaks nothing back but the audio it got.
    pub tts: Option<TtsParams>,
}

/// Which speech-to-text provider, and what it needs.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case", deny_unknown_fields)]
pub enum SttParams {
    /// The loopback provider: audio in, the same audio out. Calibrates the wire
    /// before anyone blames a model.
    Echo,
    /// Deepgram Flux over WebSocket.
    Deepgram(DeepgramParams),
    /// OpenAI realtime transcription over WebSocket.
    Openai(OpenAiSttParams),
}

/// Which text-to-speech provider, and what it needs.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case", deny_unknown_fields)]
pub enum TtsParams {
    /// Cartesia over WebSocket.
    Cartesia(CartesiaParams),
    /// OpenAI streaming speech over HTTP.
    Openai(OpenAiTtsParams),
    /// ElevenLabs over WebSocket.
    Elevenlabs(ElevenLabsParams),
}

/// Settings of the Deepgram Flux speech-to-text adapter.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeepgramParams {
    /// API credential, `${DEEPGRAM_API_KEY}` in a config.
    pub api_key: Secret,
    /// Streaming model; default [`crate::voice::providers::deepgram::DEFAULT_MODEL`].
    #[serde(default = "DeepgramParams::default_model")]
    pub model: String,
    /// Spoken language.
    #[serde(default = "DeepgramParams::default_language")]
    pub language: String,
    /// End-of-turn confidence at which a turn is closed.
    #[serde(default = "DeepgramParams::default_eot_threshold")]
    pub eot_threshold: f64,
    /// Confidence at which a preflight (`eager`) transcript is sent.
    #[serde(default = "DeepgramParams::default_eager")]
    pub eager_eot_threshold: f64,
    /// How long silence alone may end a turn.
    #[serde(default = "DeepgramParams::default_eot_timeout")]
    pub eot_timeout_ms: u64,
    /// The rate the client must send. The cell never resamples (R-V2).
    #[serde(default = "DeepgramParams::default_rate")]
    pub sample_rate: u32,
    /// Provider endpoint; a fake in the tests points this at itself.
    #[serde(default = "DeepgramParams::default_base_url")]
    pub base_url: String,
    /// Words the recogniser should expect: names, product terms, anything a
    /// general model has no reason to know. Each entry travels as its own
    /// `keyterm` query parameter, so a term made of several words stays one
    /// term. Surrounding whitespace is trimmed and a blank entry is dropped.
    /// Deepgram caps the list at 500 tokens per request and refuses anything
    /// longer -- this side does not count, so keep it to the 20-50 terms the
    /// service recommends. Empty by default -- an empty list changes nothing
    /// about the query at all.
    #[serde(default)]
    pub keyterms: Vec<String>,
}

impl DeepgramParams {
    fn default_model() -> String {
        crate::voice::providers::deepgram::DEFAULT_MODEL.to_string()
    }
    fn default_language() -> String {
        "de".to_string()
    }
    fn default_eot_threshold() -> f64 {
        0.7
    }
    fn default_eager() -> f64 {
        0.3
    }
    fn default_eot_timeout() -> u64 {
        5000
    }
    fn default_rate() -> u32 {
        16000
    }
    fn default_base_url() -> String {
        crate::voice::providers::deepgram::DEFAULT_BASE_URL.to_string()
    }
}

/// Settings of the OpenAI realtime-transcription speech-to-text adapter.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenAiSttParams {
    /// API credential, `${OPENAI_API_KEY}` in a config.
    pub api_key: Secret,
    /// Transcription model; default [`crate::voice::providers::openai_stt::DEFAULT_MODEL`].
    #[serde(default = "OpenAiSttParams::default_model")]
    pub model: String,
    /// Spoken language.
    #[serde(default = "OpenAiSttParams::default_language")]
    pub language: String,
    /// Where the turn boundary comes from: `server_vad`, `semantic_vad`, or
    /// `none`. `none` switches the provider's own endpointing off entirely,
    /// which only makes sense in `hold` mode — the client's key is then the
    /// only thing that ends a turn.
    #[serde(default = "OpenAiSttParams::default_turn_detection")]
    pub turn_detection: String,
    /// Provider endpoint; a fake in the tests points this at itself.
    #[serde(default = "OpenAiSttParams::default_base_url")]
    pub base_url: String,
}

impl OpenAiSttParams {
    /// The rate this provider takes, fixed: the cell never resamples (R-V2).
    pub const SAMPLE_RATE: u32 = 24000;

    fn default_model() -> String {
        crate::voice::providers::openai_stt::DEFAULT_MODEL.to_string()
    }
    fn default_language() -> String {
        "de".to_string()
    }
    fn default_turn_detection() -> String {
        "server_vad".to_string()
    }
    fn default_base_url() -> String {
        crate::voice::providers::openai_stt::DEFAULT_BASE_URL.to_string()
    }
}

/// Settings of the Cartesia text-to-speech adapter.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CartesiaParams {
    /// API credential, `${CARTESIA_API_KEY}` in a config.
    pub api_key: Secret,
    /// Voice id (R-V5: a param, never wired into the code). Arrives as
    /// `${CARTESIA_VOICE}` in a config and is refused if that stayed standing.
    pub voice: String,
    /// Streaming model; default [`crate::voice::providers::cartesia::DEFAULT_MODEL`].
    #[serde(default = "CartesiaParams::default_model")]
    pub model: String,
    /// Spoken language.
    #[serde(default = "CartesiaParams::default_language")]
    pub language: String,
    /// The rate the cell sends to the client. The cell never resamples (R-V2).
    #[serde(default = "CartesiaParams::default_rate")]
    pub sample_rate: u32,
    /// Optional emotion control.
    #[serde(default)]
    pub emotion: Option<String>,
    /// Optional speaking rate.
    #[serde(default)]
    pub speed: Option<f64>,
    /// Provider endpoint; a fake in the tests points this at itself.
    #[serde(default = "CartesiaParams::default_base_url")]
    pub base_url: String,
}

impl CartesiaParams {
    fn default_model() -> String {
        crate::voice::providers::cartesia::DEFAULT_MODEL.to_string()
    }
    fn default_language() -> String {
        "de".to_string()
    }
    fn default_rate() -> u32 {
        24000
    }
    fn default_base_url() -> String {
        crate::voice::providers::cartesia::DEFAULT_BASE_URL.to_string()
    }
}

/// Settings of the OpenAI streaming text-to-speech adapter.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenAiTtsParams {
    /// API credential, `${OPENAI_API_KEY}` in a config.
    pub api_key: Secret,
    /// Speech model; default [`crate::voice::providers::openai_tts::DEFAULT_MODEL`].
    #[serde(default = "OpenAiTtsParams::default_model")]
    pub model: String,
    /// Voice name.
    #[serde(default = "OpenAiTtsParams::default_voice")]
    pub voice: String,
    /// Provider endpoint; a fake in the tests points this at itself.
    #[serde(default = "OpenAiTtsParams::default_base_url")]
    pub base_url: String,
}

impl OpenAiTtsParams {
    /// The rate this provider emits, fixed: the cell never resamples (R-V2).
    pub const SAMPLE_RATE: u32 = 24000;

    fn default_model() -> String {
        crate::voice::providers::openai_tts::DEFAULT_MODEL.to_string()
    }
    fn default_voice() -> String {
        "alloy".to_string()
    }
    fn default_base_url() -> String {
        crate::voice::providers::openai_tts::DEFAULT_BASE_URL.to_string()
    }
}

/// Settings of the ElevenLabs text-to-speech adapter (GH #591).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElevenLabsParams {
    /// API credential, `${ELEVENLABS_API_KEY}` in a config.
    pub api_key: Secret,
    /// Voice id (R-V5: a param, never wired into the code). Arrives as
    /// `${ELEVENLABS_VOICE}` in a config and is refused if that stayed
    /// standing. It is a PATH segment on this API rather than a body field, so
    /// an unsubstituted one would not be a wrong voice, it would be a wrong URL.
    pub voice: String,
    /// Streaming model; default [`crate::voice::providers::elevenlabs::DEFAULT_MODEL`].
    #[serde(default = "ElevenLabsParams::default_model")]
    pub model: String,
    /// The rate the cell sends to the client. The cell never resamples (R-V2),
    /// and only the rates the vendor serves are accepted — see
    /// [`crate::voice::providers::elevenlabs::SUPPORTED_SAMPLE_RATES`].
    #[serde(default = "ElevenLabsParams::default_rate")]
    pub sample_rate: u32,
    /// Optional voice stability, `0.0..=1.0`. Unset leaves the vendor default.
    #[serde(default)]
    pub stability: Option<f64>,
    /// Optional similarity boost, `0.0..=1.0`. Unset leaves the vendor default.
    #[serde(default)]
    pub similarity_boost: Option<f64>,
    /// Provider endpoint; a fake in the tests points this at itself.
    #[serde(default = "ElevenLabsParams::default_base_url")]
    pub base_url: String,
}

impl ElevenLabsParams {
    fn default_model() -> String {
        crate::voice::providers::elevenlabs::DEFAULT_MODEL.to_string()
    }
    fn default_rate() -> u32 {
        crate::voice::providers::elevenlabs::DEFAULT_SAMPLE_RATE
    }
    fn default_base_url() -> String {
        crate::voice::providers::elevenlabs::DEFAULT_BASE_URL.to_string()
    }
}

/// A credential. `Debug` prints `<redacted>`, and there is deliberately no
/// `Display`: the only way out is [`Secret::expose`], which is greppable.
#[derive(Clone, serde::Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// The credential itself. Call this where it goes on the wire, nowhere else.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the substitution pass left a `${…}` standing.
    fn is_unresolved(&self) -> bool {
        self.0.starts_with("${") && self.0.ends_with('}')
    }

    /// The variable name inside an unresolved `${…}`, for the refusal message.
    ///
    /// Printing this is not a leak and the distinction is worth stating: an
    /// unresolved placeholder is the *name* of a variable, which is exactly
    /// what the operator has to go and set. A resolved secret never leaves
    /// [`Secret::expose`], and [`std::fmt::Debug`] here prints `<redacted>`
    /// either way.
    fn placeholder_name(&self) -> &str {
        self.0
            .strip_prefix("${")
            .and_then(|r| r.strip_suffix('}'))
            .unwrap_or_default()
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

impl VoiceParams {
    /// Parse + validate. Shares its path with `validate_params` and
    /// `spawn_cell` (parser invariant, `meclaw_colony::CellFactory`).
    ///
    /// Refuses: a non-object, port `0` or a missing `port`, an empty `bind`, a
    /// missing `stt`, a missing `tts` unless `stt.provider == "echo"`, an
    /// unknown provider name, an unknown key anywhere, a `turn_detection`
    /// outside the three the provider knows, and an unresolved `${…}` in any
    /// secret — that last message names the KEY, never the value.
    pub fn parse(v: &JsonValue) -> Result<Self, String> {
        let obj = v.as_object().ok_or("params: must be object")?;

        let port_raw = obj
            .get("port")
            .ok_or("port: required (the port this voice endpoint owns, 1..=65535)")?;
        let port = port_raw
            .as_u64()
            .filter(|p| (1..=MAX_PORT).contains(p))
            .ok_or_else(|| format!("port: must be an integer in 1..={MAX_PORT}, got {port_raw}"))?
            as u16;

        let bind = obj
            .get("bind")
            .map(|b| {
                b.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("bind: must be a string, got {b}"))
            })
            .transpose()?
            .unwrap_or_else(|| "127.0.0.1".to_string());
        if bind.is_empty() {
            return Err("bind: must not be empty".to_string());
        }

        let default_mode = match obj.get("default_mode") {
            None => Mode::Auto,
            Some(m) => match m.as_str() {
                Some("auto") => Mode::Auto,
                Some("hold") => Mode::Hold,
                _ => {
                    return Err(format!(
                        "default_mode: must be \"auto\" or \"hold\", got {m}"
                    ));
                }
            },
        };

        let barge_in = read_bool(obj, "barge_in", true)?;
        let emit_partials = read_bool(obj, "emit_partials", false)?;
        let emit_speak_end = read_bool(obj, "emit_speak_end", false)?;
        let external_timeout_ms = read_positive_u64(obj, "external_timeout_ms", 5000)?;
        let provider_idle_timeout_ms = read_positive_u64(obj, "provider_idle_timeout_ms", 30000)?;
        let audio_out_frame_ms = read_frame_ms(obj, "audio_out_frame_ms")?;
        let speak_plain = read_bool(obj, "speak_plain", DEFAULT_SPEAK_PLAIN)?;
        let release_grace_ms = read_release_grace_ms(obj, "release_grace_ms")?;

        let stt_raw = obj
            .get("stt")
            .ok_or("stt: required (an object with a `provider` key)")?;
        let stt: SttParams = meclaw_core::serde_json::from_value(stt_raw.clone())
            .map_err(|e| format!("stt: {e}"))?;

        // An explicit `null` counts as absent. `override_params` is a flat
        // merge and cannot REMOVE a key, so a template that ships a Cartesia
        // block has no other way to say "this instance is echo, and needs no
        // credential" — and refusing the null would make the shipped echo
        // instance impossible to configure at all.
        let tts = match obj.get("tts").filter(|v| !v.is_null()) {
            Some(raw) => Some(
                meclaw_core::serde_json::from_value::<TtsParams>(raw.clone())
                    .map_err(|e| format!("tts: {e}"))?,
            ),
            None => {
                if !matches!(stt, SttParams::Echo) {
                    return Err(
                        "tts: required unless `stt.provider` is \"echo\" (a voice cell that \
                         transcribes but cannot speak would answer every `in_speak` with an error)"
                            .to_string(),
                    );
                }
                None
            }
        };

        for key in obj.keys() {
            if !KNOWN_PARAMS_KEYS.contains(&key.as_str()) {
                return Err(format!("{key}: unknown params key for a voice cell"));
            }
        }

        let parsed = Self {
            port,
            bind,
            default_mode,
            barge_in,
            emit_partials,
            emit_speak_end,
            external_timeout_ms,
            provider_idle_timeout_ms,
            audio_out_frame_ms,
            speak_plain,
            release_grace_ms,
            stt,
            tts,
        };
        parsed.check_secrets()?;
        parsed.check_provider_values()?;
        Ok(parsed)
    }

    /// Refuse provider settings that parse as a string but name nothing.
    ///
    /// A `turn_detection` the provider does not know is not a slower session,
    /// it is a session that never ends a turn — so it is refused here, where
    /// the message can name the three values, rather than at the socket.
    fn check_provider_values(&self) -> Result<(), String> {
        if let SttParams::Openai(p) = &self.stt
            && !matches!(
                p.turn_detection.as_str(),
                "server_vad" | "semantic_vad" | "none"
            )
        {
            return Err(
                "stt.turn_detection: must be \"server_vad\", \"semantic_vad\" or \"none\""
                    .to_string(),
            );
        }
        // The voice id is not a credential, but it arrives the same way
        // (R-V5), and an unsubstituted one is a cell that synthesises with a
        // voice named `${CARTESIA_VOICE}` — refused by the vendor on the first
        // sentence somebody actually wanted to hear.
        if let Some(TtsParams::Cartesia(p)) = &self.tts {
            if p.voice.is_empty() {
                return Err("tts.voice: must not be empty".to_string());
            }
            if let Some(name) = p.voice.strip_prefix("${").and_then(|r| r.strip_suffix('}')) {
                return Err(format!(
                    "tts.voice: unresolved placeholder — set {name} in this colony's environment"
                ));
            }
        }
        // The same guard for ElevenLabs, where it protects one thing more: the
        // voice id is a PATH SEGMENT of the endpoint, so a value carrying `?`,
        // `#`, `/` or whitespace would not name a wrong voice, it would name a
        // different URL — with the query this adapter built silently displaced.
        if let Some(TtsParams::Elevenlabs(p)) = &self.tts {
            if p.voice.is_empty() {
                return Err("tts.voice: must not be empty".to_string());
            }
            if let Some(name) = p.voice.strip_prefix("${").and_then(|r| r.strip_suffix('}')) {
                return Err(format!(
                    "tts.voice: unresolved placeholder — set {name} in this colony's environment"
                ));
            }
            if p.voice
                .contains(|c: char| c.is_whitespace() || "/?#&%".contains(c))
            {
                return Err(
                    "tts.voice: must be a plain voice id (it is a path segment of the \
                     endpoint, so `/`, `?`, `#`, `&`, `%` and whitespace are refused)"
                        .to_string(),
                );
            }
            if !crate::voice::providers::elevenlabs::SUPPORTED_SAMPLE_RATES.contains(&p.sample_rate)
            {
                return Err(format!(
                    "tts.sample_rate: ElevenLabs serves {}, got {}",
                    crate::voice::providers::elevenlabs::SUPPORTED_SAMPLE_RATES
                        .map(|r| r.to_string())
                        .join(", "),
                    p.sample_rate
                ));
            }
        }
        Ok(())
    }

    /// Refuse any credential the substitution pass left as a `${…}`. The
    /// message names the params key, so an operator knows what to fix, and
    /// never the value, so a half-substituted config cannot leak through a
    /// spawn error into a log.
    fn check_secrets(&self) -> Result<(), String> {
        let stt_secret = match &self.stt {
            SttParams::Echo => None,
            SttParams::Deepgram(p) => Some(("stt.api_key", &p.api_key)),
            SttParams::Openai(p) => Some(("stt.api_key", &p.api_key)),
        };
        let tts_secret = match &self.tts {
            None => None,
            Some(TtsParams::Cartesia(p)) => Some(("tts.api_key", &p.api_key)),
            Some(TtsParams::Openai(p)) => Some(("tts.api_key", &p.api_key)),
            Some(TtsParams::Elevenlabs(p)) => Some(("tts.api_key", &p.api_key)),
        };
        for (key, secret) in [stt_secret, tts_secret].into_iter().flatten() {
            if secret.expose().is_empty() {
                return Err(format!("{key}: must not be empty"));
            }
            if secret.is_unresolved() {
                let name = secret.placeholder_name();
                return Err(format!(
                    "{key}: unresolved placeholder — set {name} in this colony's environment"
                ));
            }
        }
        Ok(())
    }
}

/// Read an optional boolean with a default, refusing anything that is not one.
fn read_bool(
    obj: &meclaw_core::serde_json::Map<String, JsonValue>,
    key: &str,
    default: bool,
) -> Result<bool, String> {
    match obj.get(key) {
        None => Ok(default),
        Some(v) => v
            .as_bool()
            .ok_or_else(|| format!("{key}: must be a boolean, got {v}")),
    }
}

/// Read the outbound frame length: `0` is legal and means "send what the
/// provider produced", anything above [`MAX_AUDIO_OUT_FRAME_MS`] is refused.
///
/// Its own reader rather than [`read_positive_u64`], because `0` is a *value*
/// here — the passthrough — and not the "unset" that a port `0` is.
fn read_frame_ms(
    obj: &meclaw_core::serde_json::Map<String, JsonValue>,
    key: &str,
) -> Result<u32, String> {
    match obj.get(key) {
        None => Ok(DEFAULT_AUDIO_OUT_FRAME_MS),
        Some(v) => v
            .as_u64()
            .filter(|n| *n <= MAX_AUDIO_OUT_FRAME_MS)
            .map(|n| n as u32)
            .ok_or_else(|| {
                format!(
                    "{key}: must be an integer in 0..={MAX_AUDIO_OUT_FRAME_MS} \
                     (0 sends each synthesis chunk unchanged), got {v}"
                )
            }),
    }
}

/// Read the release grace: `0` is legal and means "cut on the `release` frame",
/// anything above [`MAX_RELEASE_GRACE_MS`] is refused.
///
/// Its own reader for the same reason [`read_frame_ms`] has one: `0` is a
/// *value* here — the behaviour this cell had before the grace existed — and
/// not the "unset" that [`read_positive_u64`] refuses.
fn read_release_grace_ms(
    obj: &meclaw_core::serde_json::Map<String, JsonValue>,
    key: &str,
) -> Result<u64, String> {
    match obj.get(key) {
        None => Ok(DEFAULT_RELEASE_GRACE_MS),
        Some(v) => v
            .as_u64()
            .filter(|n| *n <= MAX_RELEASE_GRACE_MS)
            .ok_or_else(|| {
                format!(
                    "{key}: must be an integer in 0..={MAX_RELEASE_GRACE_MS} \
                     (0 cuts the turn on the release frame), got {v}"
                )
            }),
    }
}

/// Read an optional positive integer with a default.
fn read_positive_u64(
    obj: &meclaw_core::serde_json::Map<String, JsonValue>,
    key: &str,
    default: u64,
) -> Result<u64, String> {
    match obj.get(key) {
        None => Ok(default),
        Some(v) => v
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(|| format!("{key}: must be a positive integer, got {v}")),
    }
}

/// The runtime params-update overlay of a `voice` cell.
///
/// It carries the ten mutable keys **and** the two provider sub-objects
/// verbatim, and the second half is a consequence of how `apply_update` works
/// rather than a choice: the merge base is the *serialised current params*, so
/// a key that is missing here is missing from the merge. An update naming only
/// `port` would then be re-parsed against a document with no `stt` in it and
/// refused with `stt: required` — a refusal about a key the operator never
/// touched (the `web` cell learned this the same way, GH #410).
///
/// The sub-objects travel as raw JSON, not as parsed structs. [`Secret`] has no
/// `Serialize` on purpose, and giving it one so an overlay could round-trip a
/// credential would be exactly the wrong trade: what is needed here is the
/// document that came in, not a re-rendering of the values inside it.
///
/// Immutability of `stt` and `tts` is expressed by leaving them out of
/// [`OverlayParams::KNOWN_KEYS`]: an update naming either is refused as
/// `Unknown`. That is the same mechanic the `web` cell uses for its empty
/// `IMMUTABLE_KEYS`, and it puts the credential and format identity of a voice
/// endpoint where `bot_token` sits for a proxy — settled at birth.
///
/// # When an accepted update takes effect
///
/// Persistence is immediate for all ten; **effect** is not, and the split is
/// worth knowing before an operator waits for one that will not come.
///
/// - `port` / `bind` move the listener during the update itself, and only a
///   bind that took is written (see [`crate::voice::cell::VoiceCell`]).
/// - `barge_in` / `emit_partials` are handed to every open session as part of
///   the update; a call in progress changes behaviour on its next event. This
///   is how the `partial` lane is turned on at all: it is off by default
///   (R-V8'), so a colony that grows a listener for interim transcripts orders
///   them here or in `override_params`, and one without a listener never
///   dead-letters an interim it nobody asked for.
/// - `external_timeout_ms` takes effect immediately where the handler owns the
///   deadline — the wait on a rebind acknowledgement, and the `cell.db` query
///   timeout.
/// - `audio_out_frame_ms` reaches the wire on the next respawn, like both
///   timeouts: the I/O half reads it once, when it is built from the effective
///   params of the life about to start, and no message in this cell rebuilds a
///   live one. The value is written down at once and the next life frames on
///   it.
/// - `release_grace_ms` is read at the next `release`, a call in progress
///   included. A boundary that is already draining keeps the deadline it was
///   armed with: the timer is asleep in the other half, and a turn cut by a
///   number nobody sent it with is worse than one that finishes on the old one.
/// - `speak_plain` is in force from the next `in_speak` on, a call in progress
///   included: the handler is the half that rewrites the text, and it reads the
///   field every time. What DOES wait for the next respawn is the *declaration* —
///   `hello` and `GET /info` report the value the I/O half was built with,
///   because the I/O half is a separate task and the two do not share state.
///   An operator who moved the flag and wants the declaration to agree restarts
///   the cell; nothing about what is spoken depends on that.
/// - `default_mode` only ever applied to *new* connections, so nothing about it
///   is deferred; the ones already up keep the mode they were opened with.
/// - `external_timeout_ms` and `provider_idle_timeout_ms` reach the **provider
///   adapters** on the next respawn. The adapters are built in the factory's
///   build closure from the effective params, and there is no message in this
///   wave that rebuilds a live one — a running speech session holds its socket
///   and its deadlines until it ends. The value is remembered, and the next
///   life runs on it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VoiceOverlay {
    /// The TCP port this instance listens on. Mutable.
    pub port: u16,
    /// The address this instance binds. Mutable (rebind).
    pub bind: String,
    /// The mode new connections start in. Mutable.
    pub default_mode: Mode,
    /// Whether speech cancels a running synthesis. Mutable.
    pub barge_in: bool,
    /// Whether interim transcripts reach the `partial` lane. Mutable, and off
    /// until somebody orders it (R-V8').
    pub emit_partials: bool,
    /// Whether the end of a synthesis reaches the `speak_end` lane. Mutable,
    /// and off until somebody orders it, for the `partial` lane's reason.
    pub emit_speak_end: bool,
    /// Operation-timeout around provider I/O. Mutable.
    pub external_timeout_ms: u64,
    /// Idle deadline per provider socket. Mutable.
    pub provider_idle_timeout_ms: u64,
    /// Outbound frame length in milliseconds. Mutable; the I/O half frames on
    /// the value its life was built with, so a new one takes effect on the next
    /// respawn.
    pub audio_out_frame_ms: u32,
    /// Whether markdown is turned into speech text before synthesis. Mutable,
    /// and in force from the next `in_speak` on: the handler is the half that
    /// does the work, so nothing has to be rebuilt for it.
    pub speak_plain: bool,
    /// How long a released boundary waits for the provider's end. Mutable, and
    /// read at the next `release`: a boundary already draining finishes on the
    /// deadline it was armed with.
    pub release_grace_ms: u64,
    /// The `stt` sub-object as it came in. Not in `KNOWN_KEYS`: not updatable.
    pub stt: JsonValue,
    /// The `tts` sub-object as it came in, absent with the echo provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tts: Option<JsonValue>,
}

impl OverlayParams for VoiceOverlay {
    /// Every key an update may name. `stt` and `tts` are deliberately absent.
    const KNOWN_KEYS: &'static [&'static str] = &[
        "port",
        "bind",
        "default_mode",
        "barge_in",
        "emit_partials",
        "emit_speak_end",
        "external_timeout_ms",
        "provider_idle_timeout_ms",
        "audio_out_frame_ms",
        "speak_plain",
        "release_grace_ms",
    ];

    /// **Empty**, like the `web` cell's. Nothing is refused here for being the
    /// key it is; the provider blocks are out of reach because they are not
    /// *known* keys, which is a different sentence and a better one — an update
    /// naming `stt` gets `unknown param 'stt'` rather than an invitation to
    /// wonder which half of it might have been accepted.
    const IMMUTABLE_KEYS: &'static [&'static str] = &[];

    fn parse(raw: &JsonValue) -> Result<Self, String> {
        // Through the same parser as everything else (parser invariant).
        let p = VoiceParams::parse(raw)?;
        let obj = raw.as_object().ok_or("params: must be object")?;
        Ok(Self {
            port: p.port,
            bind: p.bind,
            default_mode: p.default_mode,
            barge_in: p.barge_in,
            emit_partials: p.emit_partials,
            emit_speak_end: p.emit_speak_end,
            external_timeout_ms: p.external_timeout_ms,
            provider_idle_timeout_ms: p.provider_idle_timeout_ms,
            audio_out_frame_ms: p.audio_out_frame_ms,
            speak_plain: p.speak_plain,
            release_grace_ms: p.release_grace_ms,
            stt: obj.get("stt").cloned().unwrap_or(JsonValue::Null),
            tts: obj.get("tts").cloned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meclaw_core::serde_json::json;

    fn minimal() -> JsonValue {
        json!({
            "port": 7900,
            "stt": {"provider": "deepgram", "api_key": "k"},
            "tts": {"provider": "cartesia", "api_key": "k", "voice": "v"}
        })
    }

    #[test]
    fn parse_minimal_deepgram_cartesia() {
        let p = VoiceParams::parse(&minimal()).expect("a minimal config parses");
        assert_eq!(p.port, 7900);
        assert_eq!(p.bind, "127.0.0.1");
        assert_eq!(p.default_mode, Mode::Auto);
        assert!(p.barge_in);
        assert!(
            !p.emit_partials,
            "R-V8': the partial lane is ordered, not assumed"
        );
        assert!(
            !p.emit_speak_end,
            "R-V8' again: the speak_end lane is ordered, not assumed"
        );
        assert_eq!(p.external_timeout_ms, 5000);
        assert_eq!(p.provider_idle_timeout_ms, 30000);
        assert_eq!(
            p.audio_out_frame_ms, DEFAULT_AUDIO_OUT_FRAME_MS,
            "20 ms outbound frames, because a phone edge dies on a long one"
        );
        assert!(
            p.speak_plain,
            "markdown becomes speech text unless a config says otherwise"
        );
        assert_eq!(
            p.release_grace_ms, DEFAULT_RELEASE_GRACE_MS,
            "a release waits for the provider's end of turn by default"
        );
        let SttParams::Deepgram(d) = &p.stt else {
            panic!("expected the deepgram provider");
        };
        assert_eq!(d.eot_threshold, 0.7, "R-V4: the July value, as a default");
        assert_eq!(d.eager_eot_threshold, 0.3);
        assert_eq!(d.eot_timeout_ms, 5000);
        assert_eq!(d.sample_rate, 16000);
        assert_eq!(d.language, "de");
        let Some(TtsParams::Cartesia(c)) = &p.tts else {
            panic!("expected the cartesia provider");
        };
        assert_eq!(c.sample_rate, 24000);
        assert_eq!(c.voice, "v");
    }

    #[test]
    fn parse_refuses_port_zero() {
        let mut raw = minimal();
        raw["port"] = json!(0);
        assert!(VoiceParams::parse(&raw).is_err(), "0 means 'pick one'");
        let mut missing = minimal();
        missing.as_object_mut().unwrap().remove("port");
        let err = VoiceParams::parse(&missing).unwrap_err();
        assert!(err.starts_with("port: required"), "got {err}");
    }

    #[test]
    fn parse_refuses_missing_tts_unless_echo() {
        let echo = json!({"port": 7900, "stt": {"provider": "echo"}});
        let p = VoiceParams::parse(&echo).expect("echo needs no voice to speak with");
        assert!(p.tts.is_none());

        let mut deaf = minimal();
        deaf.as_object_mut().unwrap().remove("tts");
        let err = VoiceParams::parse(&deaf).unwrap_err();
        assert!(err.starts_with("tts:"), "the refusal names the key: {err}");
    }

    /// R-V21: `"tts": null` is how a flat override says "no text-to-speech".
    #[test]
    fn tts_null_counts_as_absent_for_echo() {
        let echo = json!({"port": 7900, "stt": {"provider": "echo"}, "tts": null});
        let p = VoiceParams::parse(&echo).expect("an echo instance overridden to silence");
        assert!(p.tts.is_none());

        let deaf = json!({
            "port": 7900,
            "stt": {"provider": "deepgram", "api_key": "k"},
            "tts": null
        });
        let err = VoiceParams::parse(&deaf).unwrap_err();
        assert!(
            err.starts_with("tts:"),
            "a transcribing cell that cannot speak is still refused: {err}"
        );
    }

    #[test]
    fn parse_refuses_unresolved_secret() {
        let mut raw = minimal();
        raw["stt"]["api_key"] = json!("${DEEPGRAM_API_KEY}");
        let err = VoiceParams::parse(&raw).unwrap_err();
        assert!(err.starts_with("stt.api_key:"), "names the key: {err}");
        assert!(
            err.contains("DEEPGRAM_API_KEY"),
            "and names the variable the operator has to set, which is what the \
             unresolved placeholder IS: {err}"
        );

        // The other half of the same rule: a credential that resolved never
        // appears in a refusal, because there is nothing left to refuse it for.
        let mut resolved = minimal();
        resolved["stt"]["api_key"] = json!("dg-supersecret");
        resolved["port"] = json!(0);
        let err = VoiceParams::parse(&resolved).unwrap_err();
        assert!(!err.contains("supersecret"), "got {err}");
    }

    #[test]
    fn parse_refuses_unknown_provider() {
        let mut raw = minimal();
        raw["stt"] = json!({"provider": "whisper.cpp", "api_key": "k"});
        assert!(VoiceParams::parse(&raw).is_err());

        let mut typo = minimal();
        typo["nope"] = json!(true);
        let err = VoiceParams::parse(&typo).unwrap_err();
        assert!(err.starts_with("nope:"), "a typo'd key is named: {err}");

        let mut inner = minimal();
        inner["stt"]["nope"] = json!(true);
        assert!(
            VoiceParams::parse(&inner).is_err(),
            "a typo inside a provider block is a setting nobody applies"
        );
    }

    /// R-V5: a voice id that never got substituted is refused by name.
    #[test]
    fn parse_refuses_an_unresolved_voice_id() {
        let mut raw = minimal();
        raw["tts"]["voice"] = json!("${CARTESIA_VOICE}");
        let err = VoiceParams::parse(&raw).unwrap_err();
        assert!(err.starts_with("tts.voice:"), "names the key: {err}");
        assert!(
            err.contains("CARTESIA_VOICE"),
            "and the variable to set: {err}"
        );

        let mut empty = minimal();
        empty["tts"]["voice"] = json!("");
        assert!(VoiceParams::parse(&empty).is_err());
    }

    /// R-V18: the three the provider knows, and nothing else.
    #[test]
    fn parse_refuses_an_unknown_turn_detection() {
        for good in ["server_vad", "semantic_vad", "none"] {
            let raw = json!({
                "port": 7900,
                "stt": {"provider": "openai", "api_key": "k", "turn_detection": good},
                "tts": {"provider": "openai", "api_key": "k"}
            });
            assert!(VoiceParams::parse(&raw).is_ok(), "{good} is a real value");
        }
        let raw = json!({
            "port": 7900,
            "stt": {"provider": "openai", "api_key": "k", "turn_detection": "vad"},
            "tts": {"provider": "openai", "api_key": "k"}
        });
        let err = VoiceParams::parse(&raw).unwrap_err();
        assert!(err.starts_with("stt.turn_detection:"), "got {err}");
        assert!(
            err.contains("semantic_vad"),
            "the refusal names them: {err}"
        );
    }

    #[test]
    fn debug_redacts_secrets() {
        let mut raw = minimal();
        raw["stt"]["api_key"] = json!("dg-supersecret");
        raw["tts"]["api_key"] = json!("ct-supersecret");
        let p = VoiceParams::parse(&raw).expect("parses");
        let rendered = format!("{p:?}");
        assert!(
            !rendered.contains("supersecret"),
            "a credential must not survive a Debug rendering"
        );
        assert!(rendered.contains("<redacted>"), "got {rendered}");
    }

    /// R-V14: the model name and the endpoint belong to the adapter. These four
    /// assertions are what keeps the params defaults from becoming a second
    /// copy that drifts — and they live here, so a provider strand that moves
    /// its constant does not have to touch this file to stay green.
    #[test]
    fn every_default_model_is_the_adapters_own() {
        use crate::voice::providers::{cartesia, deepgram, openai_stt, openai_tts};
        let p = VoiceParams::parse(&minimal()).expect("parses");
        let SttParams::Deepgram(d) = &p.stt else {
            panic!("expected deepgram");
        };
        assert_eq!(d.model, deepgram::DEFAULT_MODEL);
        assert_eq!(d.base_url, deepgram::DEFAULT_BASE_URL);
        let Some(TtsParams::Cartesia(c)) = &p.tts else {
            panic!("expected cartesia");
        };
        assert_eq!(c.model, cartesia::DEFAULT_MODEL);
        assert_eq!(c.base_url, cartesia::DEFAULT_BASE_URL);

        let openai = VoiceParams::parse(&json!({
            "port": 7900,
            "stt": {"provider": "openai", "api_key": "k"},
            "tts": {"provider": "openai", "api_key": "k"}
        }))
        .expect("the openai pair parses");
        let SttParams::Openai(os) = &openai.stt else {
            panic!("expected the openai transcription provider");
        };
        assert_eq!(os.model, openai_stt::DEFAULT_MODEL);
        assert_eq!(os.base_url, openai_stt::DEFAULT_BASE_URL);
        let Some(TtsParams::Openai(ot)) = &openai.tts else {
            panic!("expected the openai speech provider");
        };
        assert_eq!(ot.model, openai_tts::DEFAULT_MODEL);
        assert_eq!(ot.base_url, openai_tts::DEFAULT_BASE_URL);
    }

    /// GH #591: the third text-to-speech provider parses, and every default it
    /// does not carry itself comes out of the adapter (R-V14).
    #[test]
    fn elevenlabs_tts_parses_with_its_own_defaults() {
        use crate::voice::providers::elevenlabs;
        let raw = json!({
            "port": 7900,
            "stt": {"provider": "echo"},
            "tts": {"provider": "elevenlabs", "api_key": "k", "voice": "v"}
        });
        let p = VoiceParams::parse(&raw).expect("the elevenlabs block parses");
        let Some(TtsParams::Elevenlabs(e)) = &p.tts else {
            panic!("expected the elevenlabs provider");
        };
        assert_eq!(e.model, elevenlabs::DEFAULT_MODEL);
        assert_eq!(e.base_url, elevenlabs::DEFAULT_BASE_URL);
        assert_eq!(e.sample_rate, elevenlabs::DEFAULT_SAMPLE_RATE);
        assert_eq!(e.voice, "v");
        assert!(
            e.stability.is_none() && e.similarity_boost.is_none(),
            "an unset knob stays unset, so the vendor's own default stands"
        );

        let mut unknown = raw.clone();
        unknown["tts"]["speed"] = json!(1.2);
        assert!(
            VoiceParams::parse(&unknown).is_err(),
            "deny_unknown_fields: a knob this provider has no wire for is a typo"
        );
    }

    /// R-V5, sharpened: the voice id is a PATH SEGMENT of the endpoint here, so
    /// an unsubstituted or URL-shaped one is refused by name in the parser.
    #[test]
    fn elevenlabs_refuses_an_unresolved_or_url_shaped_voice() {
        let base = json!({
            "port": 7900,
            "stt": {"provider": "echo"},
            "tts": {"provider": "elevenlabs", "api_key": "k", "voice": "v"}
        });
        for bad in [
            "${ELEVENLABS_VOICE}",
            "",
            "a/b",
            "v?output_format=mp3_44100_128",
            "v#x",
            "v&x",
            "v id",
        ] {
            let mut raw = base.clone();
            raw["tts"]["voice"] = json!(bad);
            let err =
                VoiceParams::parse(&raw).expect_err("a voice that reshapes the URL is refused");
            assert!(err.starts_with("tts.voice:"), "names the key: {err}");
        }
    }

    /// The cell never resamples (R-V2), so a rate the vendor does not serve is
    /// a cell that announces one format in `hello` and then never speaks. It is
    /// refused here, where the message can name the rates that exist.
    #[test]
    fn elevenlabs_refuses_a_rate_the_vendor_does_not_serve() {
        use crate::voice::providers::elevenlabs;
        let base = json!({
            "port": 7900,
            "stt": {"provider": "echo"},
            "tts": {"provider": "elevenlabs", "api_key": "k", "voice": "v"}
        });
        for good in elevenlabs::SUPPORTED_SAMPLE_RATES {
            let mut raw = base.clone();
            raw["tts"]["sample_rate"] = json!(good);
            assert!(
                VoiceParams::parse(&raw).is_ok(),
                "{good} is one of the vendor's pcm_* formats"
            );
        }
        let mut raw = base;
        raw["tts"]["sample_rate"] = json!(11025);
        let err = VoiceParams::parse(&raw).expect_err("11025 is not served");
        assert!(err.starts_with("tts.sample_rate:"), "names the key: {err}");
        assert!(err.contains("24000"), "the refusal names the rates: {err}");
    }

    /// `0` is a value here (the passthrough), a frame longer than a second is
    /// a typo, and both are decided in the parser rather than at the socket.
    #[test]
    fn audio_out_frame_ms_takes_zero_and_refuses_a_buffer() {
        let mut zero = minimal();
        zero["audio_out_frame_ms"] = json!(0);
        assert_eq!(
            VoiceParams::parse(&zero)
                .expect("0 is the passthrough, not an unset value")
                .audio_out_frame_ms,
            0
        );

        let mut long = minimal();
        long["audio_out_frame_ms"] = json!(2000);
        let err = VoiceParams::parse(&long).unwrap_err();
        assert!(err.starts_with("audio_out_frame_ms:"), "got {err}");
        assert!(err.contains("1000"), "the refusal names the bound: {err}");

        let mut wrong = minimal();
        wrong["audio_out_frame_ms"] = json!("20ms");
        assert!(
            VoiceParams::parse(&wrong).is_err(),
            "a string is not a length"
        );
    }

    /// It is on the update surface: an operator who finds out what an edge can
    /// swallow says so without a rebuild.
    #[test]
    fn audio_out_frame_ms_is_mutable() {
        use crate::params_overlay::apply_update;
        let current = <VoiceOverlay as OverlayParams>::parse(&minimal()).expect("parses");
        let mut update = meclaw_core::serde_json::Map::new();
        update.insert("audio_out_frame_ms".into(), json!(40));
        let (merged, overlay) = apply_update(&current, &update).expect("an accepted update");
        assert_eq!(merged.audio_out_frame_ms, 40);
        assert_eq!(overlay, vec![("audio_out_frame_ms".to_string(), json!(40))]);
    }

    /// `0` is a value here — cut on the frame — and the upper bound is what
    /// turns a typo into a refusal instead of a call that goes quiet.
    #[test]
    fn release_grace_ms_takes_zero_and_refuses_a_wedge() {
        let mut zero = minimal();
        zero["release_grace_ms"] = json!(0);
        assert_eq!(
            VoiceParams::parse(&zero)
                .expect("0 is legal")
                .release_grace_ms,
            0,
            "0 says: cut on the release frame, the behaviour before the grace"
        );

        let mut long = minimal();
        long["release_grace_ms"] = json!(15000);
        let err = VoiceParams::parse(&long).expect_err("a wedge is not a grace");
        assert!(err.starts_with("release_grace_ms:"), "got {err}");

        let mut wrong = minimal();
        wrong["release_grace_ms"] = json!("1500ms");
        assert!(
            VoiceParams::parse(&wrong).is_err(),
            "a duration string is not a number of milliseconds"
        );
    }

    #[test]
    fn release_grace_ms_is_mutable() {
        use crate::params_overlay::apply_update;
        let current = <VoiceOverlay as OverlayParams>::parse(&minimal()).expect("parses");
        let mut update = meclaw_core::serde_json::Map::new();
        update.insert("release_grace_ms".into(), json!(300));
        let (merged, overlay) = apply_update(&current, &update).expect("300 ms is a grace");
        assert_eq!(merged.release_grace_ms, 300);
        assert_eq!(overlay, vec![("release_grace_ms".to_string(), json!(300))]);
    }

    /// The knob that keeps a synthesis provider from reading markdown aloud is
    /// on by default and off by naming it — and it is on the update surface,
    /// because a colony that finds its answers already plain says so without a
    /// rebuild.
    #[test]
    fn speak_plain_ships_on_and_is_mutable() {
        use crate::params_overlay::apply_update;
        assert!(
            VoiceParams::parse(&minimal())
                .expect("a minimal config parses")
                .speak_plain,
            "an assistant writes markdown whether or not anybody asked it to"
        );

        let mut off = minimal();
        off["speak_plain"] = json!(false);
        assert!(
            !VoiceParams::parse(&off)
                .expect("the flag parses")
                .speak_plain,
            "false hands the text to the provider exactly as it arrived"
        );

        let current = <VoiceOverlay as OverlayParams>::parse(&minimal()).expect("parses");
        let mut update = meclaw_core::serde_json::Map::new();
        update.insert("speak_plain".into(), json!(false));
        let (merged, overlay) = apply_update(&current, &update).expect("an accepted update");
        assert!(!merged.speak_plain);
        assert_eq!(overlay, vec![("speak_plain".to_string(), json!(false))]);
    }

    #[test]
    fn overlay_refuses_stt_update() {
        use crate::params_overlay::{ParamUpdateError, apply_update};
        let current = <VoiceOverlay as OverlayParams>::parse(&minimal()).expect("parses");
        let mut update = meclaw_core::serde_json::Map::new();
        update.insert("stt".into(), json!({"provider": "echo"}));
        let err = apply_update(&current, &update).expect_err("the provider is settled at birth");
        assert!(
            matches!(err, ParamUpdateError::Unknown(ref k) if k == "stt"),
            "got {err:?}"
        );
    }

    #[test]
    fn a_port_only_update_does_not_lose_the_provider() {
        // The merge base is the serialised current params, so the overlay has
        // to carry `stt` even though no update may name it.
        use crate::params_overlay::apply_update;
        let current = <VoiceOverlay as OverlayParams>::parse(&minimal()).expect("parses");
        let mut update = meclaw_core::serde_json::Map::new();
        update.insert("port".into(), json!(7901));
        let (merged, overlay) = apply_update(&current, &update).expect("a port update applies");
        assert_eq!(merged.port, 7901);
        assert_eq!(overlay, vec![("port".to_string(), json!(7901))]);
        let back = VoiceParams::parse(
            &meclaw_core::serde_json::to_value(&merged).expect("the overlay serializes"),
        )
        .expect("and round-trips through the one parser");
        assert!(matches!(back.stt, SttParams::Deepgram(_)));
    }
}
