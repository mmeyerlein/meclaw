//! Wire protocol `meclaw-voice/1` between a client and the cell. Text frames
//! are JSON objects tagged by `type`; binary frames are raw audio in the
//! format the `hello` frame declared. Documented in docs/voice-wire-protocol.en.md.

use crate::voice::contract::AudioFormat;

/// Protocol name and version, as sent in `hello.protocol`.
pub const PROTOCOL: &str = "meclaw-voice/1";
/// Close code: a second connection claimed this session; the first is dropped.
pub const CLOSE_SESSION_REPLACED: u16 = 4409;

/// Per-connection mode. `auto` = provider endpointing, `hold` = client frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// The provider decides where a turn ends.
    Auto,
    /// The client decides, by `hold` and `release`.
    Hold,
}

/// Frames the client sends.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientFrame {
    /// Open the turn boundary (`hold` mode only).
    Hold,
    /// Close the turn boundary: exactly one turn follows.
    Release,
    /// Drop the running synthesis and the session's queue.
    Cancel,
    /// Switch mode.
    Mode {
        /// The mode to switch to.
        mode: Mode,
    },
}

/// Frames the cell sends.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    /// First frame on every connection.
    Hello {
        /// Always [`PROTOCOL`].
        protocol: &'static str,
        /// The session this connection speaks for.
        session_id: String,
        /// The call this connection carries.
        ///
        /// The same string as `session_id` on this cell — a connection IS the
        /// session, and for a telephony edge the session IS the call. The
        /// second name exists because the two are owned by different things
        /// once a call reaches a colony: `session_id` is also what a member's
        /// `session-keeper` mints for its own generation, and `call_id` is
        /// what this cell answers to and stamps on everything it emits
        /// (GH #620). A client reads whichever of the two its own vocabulary
        /// uses.
        call_id: String,
        /// The mode this connection starts in.
        mode: Mode,
        /// What the client must send. The cell never resamples (R-V2).
        audio_in: AudioFormat,
        /// What the cell will send back; `None` when there is no TTS provider.
        audio_out: Option<AudioFormat>,
        /// Name of the speech-to-text provider.
        stt: &'static str,
        /// Name of the text-to-speech provider, or `None`.
        tts: Option<&'static str>,
        /// How much audio one outbound binary frame carries, in milliseconds.
        ///
        /// Always present, `0` included: `0` says the cell sends every chunk
        /// exactly as its provider produced it, so a client that has to know
        /// its worst-case frame size — a phone edge whose playback half is not
        /// robust against a long one — reads a number either way rather than
        /// an absence it has to interpret.
        audio_out_frame_ms: u32,
        /// Whether the cell turns a written answer into speech text before it
        /// is synthesised — markdown emphasis, headings, list markers, links,
        /// code fences and table pipes removed.
        ///
        /// Declared because it changes what a client HEARS: with `false` a
        /// provider is handed the answer exactly as it was written, stars
        /// included. The value is the one this cell's I/O half was built with;
        /// a `params` update takes effect on the next synthesis and reaches
        /// this declaration on the next respawn.
        speak_plain: bool,
        /// How long a released `hold` boundary waits for the recognition
        /// provider's own end of turn before the cell cuts the turn with what
        /// it has, in milliseconds; `0` cuts on the `release` frame.
        ///
        /// Declared because it is the only number that tells a `hold` client
        /// how long its `turn` may take: `release` is not the moment the turn
        /// arrives, and a client with a spinner or a push-to-talk key that
        /// re-arms itself has to know the upper bound rather than guess one.
        /// Like `speak_plain`, the value is the one this cell's I/O half was
        /// built with; a `params` update is in force at the next `release` and
        /// reaches this declaration on the next respawn.
        release_grace_ms: u64,
    },
    /// Interim transcript (mirror of the `partial` lane).
    Partial {
        /// The interim transcript, replacing the previous one.
        text: String,
        /// `true` for a preflight transcript.
        eager: bool,
    },
    /// A turn (mirror of the `turn` lane). Empty `text` on an empty release.
    Turn {
        /// The final transcript of this turn.
        text: String,
        /// `"<session_id>#<n>"`, counted per session.
        turn_id: String,
    },
    /// Audio for `speak_id` starts with the next binary frame.
    SpeakStart {
        /// The synthesis this audio belongs to.
        speak_id: String,
    },
    /// Audio for `speak_id` is over.
    SpeakEnd {
        /// The synthesis that ended.
        speak_id: String,
        /// Why it ended.
        reason: SpeakEndReason,
        /// Cause, on `failed`.
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// Mode switch acknowledged.
    Mode {
        /// The mode now in force.
        mode: Mode,
    },
    /// A protocol error on this connection.
    Error {
        /// Closed vocabulary, see [`WireErrorCode`].
        code: WireErrorCode,
        /// Human-readable cause. Never carries a credential.
        detail: String,
        /// How many audio frames this connection has sent that were not whole
        /// sample frames, counting this one (R-V6'). Present only on
        /// [`WireErrorCode::BadAudioFrame`]; every other error frame serialises
        /// exactly as it did before this field existed.
        #[serde(skip_serializing_if = "Option::is_none")]
        bad_frames: Option<u32>,
    },
}

/// Why a synthesis ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SpeakEndReason {
    /// The last chunk was sent.
    Done,
    /// `cancel`, or a barge-in, cut it short.
    Cancelled,
    /// The provider failed.
    Failed,
}

/// Closed list of per-connection error codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireErrorCode {
    /// A text frame was not a frame of this protocol.
    BadFrame,
    /// A frame that only exists in the other mode.
    WrongMode,
    /// `release` without an open `hold`.
    NotHolding,
    /// `hold` while one is already open.
    AlreadyHolding,
    /// The speech-to-text session of this connection failed.
    SttFailed,
    /// A synthesis for this connection failed.
    TtsFailed,
    /// A binary frame was not whole sample frames (odd length for PCM16 mono).
    ///
    /// The frame is dropped and the connection stays open (R-V6'): a client
    /// that mis-frames one buffer is a client with a bug, not an attacker, and
    /// closing the socket would take a live call down over a lost 20 ms. The
    /// count on the frame is what makes a systematic mis-framing visible
    /// without a threshold anybody has to justify.
    BadAudioFrame,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::contract::{AudioFormat, Encoding};
    use meclaw_core::serde_json;

    #[test]
    fn hello_frame_serializes_with_protocol_tag() {
        let hello = ServerFrame::Hello {
            protocol: PROTOCOL,
            session_id: "s1".to_string(),
            call_id: "s1".to_string(),
            mode: Mode::Auto,
            audio_in: AudioFormat::pcm16_mono(16000),
            audio_out: Some(AudioFormat::pcm16_mono(24000)),
            stt: "echo",
            tts: None,
            audio_out_frame_ms: 20,
            speak_plain: true,
            release_grace_ms: 1500,
        };
        let json = serde_json::to_string(&hello).expect("hello serializes");
        assert!(json.contains(r#""type":"hello""#), "got {json}");
        assert!(
            json.contains(r#""protocol":"meclaw-voice/1""#),
            "got {json}"
        );
        assert!(
            json.contains(
                r#""audio_in":{"encoding":"pcm_s16le","sample_rate":16000,"channels":1}"#
            ),
            "the declared input format is the one the client has to adapt to: {json}"
        );
        assert!(
            json.contains(r#""audio_out_frame_ms":20"#),
            "the frame length is declared, never inferred: {json}"
        );
        assert!(
            json.contains(r#""speak_plain":true"#),
            "what happens to the text before it is spoken is declared too: {json}"
        );
        assert!(
            json.contains(r#""call_id":"s1""#),
            "the first frame names the call this connection carries: {json}"
        );
        assert!(
            json.contains(r#""release_grace_ms":1500"#),
            "how long a released boundary may take is a number the client reads,              not one it guesses: {json}"
        );
        assert_eq!(AudioFormat::pcm16_mono(16000).encoding, Encoding::PcmS16Le);
    }

    #[test]
    fn client_frame_rejects_unknown_type() {
        let err = serde_json::from_str::<ClientFrame>(r#"{"type":"nope"}"#);
        assert!(err.is_err(), "an unknown frame type is not a frame");
    }

    #[test]
    fn client_frame_mode_parses() {
        let frame: ClientFrame =
            serde_json::from_str(r#"{"type":"mode","mode":"hold"}"#).expect("a mode frame parses");
        assert_eq!(frame, ClientFrame::Mode { mode: Mode::Hold });
    }
}
