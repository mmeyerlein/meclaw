//! The turn state machine, one per session: `(state, input) -> actions`, pure
//! and testable without a socket, a provider or a runtime.
//!
//! # What this is a port of, and what it deliberately is not (R-V11)
//!
//! Two references were read before this file was written. Cartesia's turn-based
//! ASR wire protocol as pipecat speaks it
//! (`pipecat/services/cartesia/turns/stt.py`) runs
//! `turn.start → turn.update* → (turn.eager_end → turn.resume?)* → turn.end`,
//! and three things there are the same shape the substrate needs, so they are
//! taken over:
//!
//! - **A transcript is cumulative, not a delta.** Every update carries the whole
//!   turn so far and replaces its predecessor. [`crate::voice::contract::SttEvent::Partial`]
//!   says the same, and nothing here concatenates two partials.
//! - **An eager end is not a turn.** `turn.eager_end` is surfaced and nothing
//!   downstream is finalized by it; `turn.resume` withdraws it and changes
//!   nothing that was already sent. Here that is `Partial { eager: true }` and
//!   [`crate::voice::contract::SttEvent::TurnResumed`] producing no actions.
//! - **An empty turn is not a message.** pipecat skips the transcription frame
//!   when the turn ended with an empty transcript — a watchdog can force a turn
//!   end over silence — while still closing the turn on the lifecycle path. Here
//!   the empty turn produces the client frame and **no lane emission**, in both
//!   modes, which is the spec's rule for an empty `release` generalised to the
//!   one case that has the same cause.
//!
//! From livekit-agents' `InterruptionOptions` only `enabled` is ported — it is
//! `params.barge_in`. What is **not** ported, and why: `min_duration` /
//! `min_words` gate an interruption behind a measured threshold, and this wave
//! has no measurement to set one from (R-V4 forbids a constant with meaning);
//! `resume_false_interruption` / `false_interruption_timeout` would re-start a
//! synthesis the substrate already told the client had ended, which needs a
//! frame this protocol does not have; `discard_audio_if_uninterruptible` is
//! moot, because audio never queues in the handler. Their
//! `preemptive_generation` defaults to `preemptive_tts: false`, which is the
//! same ruling this wave made for itself.
//!
//! The spec's own rules win wherever they differ: exactly one `turn` per
//! `EndOfTurn` in `auto` and per `release` in `hold`, the hold buffer, and
//! barge-in only in `auto` mode.
//!
//! # The three phases of a boundary in `hold` mode
//!
//! `Idle -> Holding -> Draining -> Idle`, and the middle arrow is the one that
//! is not obvious. `release` does not end a turn; it says that **no new audio**
//! belongs to it. What the provider still owes for the audio already sent does,
//! and that owing is measured in hundreds of milliseconds — long enough that
//! cutting on the frame is how the last words of every take went missing. So a
//! released boundary DRAINS: it stays open, keeps taking the provider's events
//! for the turn that is closing, and ends at whichever comes first — the
//! provider's own `EndOfTurn`, or the cap `release_grace_ms` (`0` = cut on the
//! frame, the behaviour before this existed). A `hold` arriving mid-drain ends
//! the old boundary at once, with what it has, and opens an empty new one; a
//! mode switch mid-drain closes it the same way, rather than refusing for as
//! long as the grace runs.
//!
//! Whenever a boundary is closed by something OTHER than the provider — the key
//! again, the cap, a mode switch — **the session remembers a provider end it is
//! still owed** ([`SessionState::pending_provider_end`]), because the provider
//! is still inside the turn the old audio started and its next `EndOfTurn`
//! carries the take that has just closed. That one event pays the debt and is
//! thrown away, wherever it lands; the debt is written off if the recognition
//! session dies first, since a provider that is gone owes nothing. Every close
//! also moves [`SessionState::grace_token`], which is what makes the cap that
//! wakes up afterwards a no-op instead of a second, empty turn.

use crate::voice::contract::SttEvent;
use crate::voice::wire::{ClientFrame, Mode, ServerFrame, SpeakEndReason, WireErrorCode};
use std::collections::VecDeque;

/// What one session knows about itself. Lives in the handler task; there is one
/// of these per live connection and no lock anywhere near it.
#[derive(Debug, Clone)]
pub struct SessionState {
    /// `auto` (the provider ends turns) or `hold` (the client does).
    pub mode: Mode,
    /// The open turn boundary in `hold` mode, if there is one.
    pub hold: Option<HoldState>,
    /// The synthesis currently running, if any.
    pub speaking: Option<String>,
    /// Syntheses waiting behind it, in arrival order.
    pub queue: VecDeque<(String, String)>,
    /// How many turns this session has produced; the source of `turn_id`.
    pub turn_seq: u64,
    /// Whether speech cancels a running synthesis (`auto` only).
    pub barge_in: bool,
    /// Whether interim transcripts reach the `partial` lane.
    pub emit_partials: bool,
    /// How long a released boundary waits for the provider's own end before it
    /// cuts with what it has. `0` cuts on the `release` frame itself.
    pub release_grace_ms: u64,
    /// Which draining boundary a [`Input::ReleaseGraceExpired`] is about.
    ///
    /// A `release` moves it on to name the boundary that starts draining, and
    /// every close of one — by the provider's end, by the cap, by a key pressed
    /// again or by a mode switch — moves it on again, so the timer that was
    /// armed for a boundary which has since ended names a generation that no
    /// longer exists and is ignored. The starting value is minted by the CELL
    /// and not by this type: a session identity outlives a connection, and two
    /// lives of one identity must not share a number
    /// (see `VoiceCell::grace_seq`). A cancellable timer would be the other way to say this;
    /// a number the handler cannot get wrong is the cheaper one, because the
    /// half that sleeps is not the half that knows what a turn is.
    pub grace_token: u64,
    /// Whether the provider still owes the end of a turn that belongs to a
    /// boundary this session has already closed.
    ///
    /// Set whenever a DRAINING boundary is cut by something other than the
    /// provider itself — the key pressed again, the cap, a mode switch. The
    /// provider does not know the boundary is gone: it is still inside the turn
    /// the old audio started, so its next `EndOfTurn` (and the interims leading
    /// up to it) carry the take that has just been closed. Paid by that one
    /// event, wherever it lands — inside the next boundary, or outside every
    /// boundary.
    ///
    /// It lives on the SESSION and not on a boundary because the two events are
    /// in a race: whether the client presses the key again before or after the
    /// provider answers is not something either side decides, and a debt that
    /// only one of the two orders could collect would be a leak in the other.
    ///
    /// Written off when the recognition session dies (`Closed` / `Failed`): a
    /// provider that is gone owes nothing, and a debt carried over a reconnect
    /// would eat the first real end of turn of the NEXT take — the very loss
    /// this whole boundary phase exists against.
    pub pending_provider_end: bool,
}

impl SessionState {
    /// A fresh session in `mode`, with the cell's behaviour flags and the
    /// release grace this colony configured.
    pub fn new(mode: Mode, barge_in: bool, emit_partials: bool, release_grace_ms: u64) -> Self {
        Self {
            mode,
            hold: None,
            speaking: None,
            queue: VecDeque::new(),
            turn_seq: 0,
            barge_in,
            emit_partials,
            release_grace_ms,
            grace_token: 0,
            pending_provider_end: false,
        }
    }
}

/// An open turn boundary in `hold` mode.
#[derive(Debug, Clone, Default)]
pub struct HoldState {
    /// Every `EndOfTurn` the provider produced while the boundary was open. The
    /// provider keeps ending turns on its own schedule; in `hold` mode those are
    /// pieces of the one turn the client will ask for.
    pub buffer: Vec<String>,
    /// The most recent interim transcript, which is what the provider has heard
    /// since its last `EndOfTurn`. `release` has to include it or everything
    /// said after the last provider-side boundary would be dropped.
    pub last_partial: String,
    /// Whether the client has let the key go and this boundary is only waiting
    /// for the provider to finish the audio it was already given.
    ///
    /// `release` used to cut here and then, and that lost the end of every
    /// take (found 06.09.2026 on the built-in test page). A recognition
    /// provider reports the end of the last words a few hundred milliseconds
    /// after the audio carrying them was sent — Deepgram Flux takes 400–700 ms
    /// for its `EndOfTurn` — so the frame arrives while the last sentence is
    /// still in flight. Cutting on it dropped those words, and the late
    /// `EndOfTurn` then landed in whatever boundary happened to be open next,
    /// which is why the previous take turned up as a prefix of the following
    /// one. Draining is the fix and the whole of it: no new audio belongs to
    /// this turn, but everything the provider still owes for the audio already
    /// sent does.
    pub draining: bool,
}

/// Everything that can move a session.
#[derive(Debug, Clone)]
pub enum Input {
    /// The speech-to-text provider said something.
    Stt(SttEvent),
    /// The client sent a control frame.
    Control(ClientFrame),
    /// A synthesis this session started has ended.
    SpeakEnded {
        /// Which one.
        speak_id: String,
        /// Why it ended.
        reason: SpeakEndReason,
    },
    /// The grace a `release` armed has run out (see
    /// [`SessionState::release_grace_ms`]). Carries the generation it was armed
    /// for; anything older is a timer for a turn that has already closed.
    ReleaseGraceExpired {
        /// The [`SessionState::grace_token`] this timer was armed with.
        token: u64,
    },
    /// The topology asked for something to be spoken.
    Speak {
        /// The identity minted for this synthesis.
        speak_id: String,
        /// What to say.
        text: String,
    },
}

/// What the handler has to do about it. Nothing here performs I/O; the caller
/// turns these into lane emissions and [`crate::voice::cell::VoiceReconfig`]
/// messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Emit an interim transcript on the `partial` lane.
    EmitPartial {
        /// The transcript.
        text: String,
        /// Whether it was a preflight one.
        eager: bool,
    },
    /// Emit a finished turn on the `turn` lane.
    EmitTurn {
        /// The transcript.
        text: String,
        /// `"<session_id>#<n>"`.
        turn_id: String,
    },
    /// Send a text frame to this session's client.
    ToClient(ServerFrame),
    /// Start a synthesis.
    StartSpeak {
        /// The identity of this synthesis.
        speak_id: String,
        /// What to say.
        text: String,
    },
    /// Stop the running synthesis of this session.
    CancelSpeak,
}

/// Advance one session by one input.
///
/// The whole cell's turn semantics are here, and deliberately nowhere else: a
/// state machine that can be driven from a `Vec` in a unit test is the only
/// version of this that anyone can argue with.
pub fn step(state: &mut SessionState, session_id: &str, input: Input) -> Vec<Action> {
    match input {
        Input::Stt(event) => step_stt(state, session_id, event),
        Input::Control(frame) => step_control(state, session_id, frame),
        Input::ReleaseGraceExpired { token } => {
            // Two ways to be about nothing: the boundary closed already (the
            // provider beat the cap, or a new `hold` took over), or this timer
            // belongs to a generation before the one now draining.
            let current = state.grace_token == token;
            let draining = matches!(&state.hold, Some(h) if h.draining);
            if current && draining {
                // The provider never answered in time. It still owes the end of
                // the audio it was given, and that end is not this take's any
                // more.
                cut_hold(state, session_id, ProviderDebt::Owed)
            } else {
                Vec::new()
            }
        }
        Input::Speak { speak_id, text } => {
            state.queue.push_back((speak_id, text));
            start_next_if_idle(state)
        }
        Input::SpeakEnded { speak_id, reason } => {
            let _ = reason;
            if state.speaking.as_deref() == Some(speak_id.as_str()) {
                state.speaking = None;
            }
            // A cancel emptied the queue before the provider got here, so the
            // `cancelled` case starts nothing without needing to say so.
            start_next_if_idle(state)
        }
    }
}

/// Provider events.
fn step_stt(state: &mut SessionState, session_id: &str, event: SttEvent) -> Vec<Action> {
    match event {
        SttEvent::SpeechStarted => {
            // Barge-in is an `auto`-mode affordance: in `hold` the client's own
            // key is the interruption, and cancelling on speech there would
            // make the agent unable to finish a sentence over a cough.
            if state.mode == Mode::Auto && state.barge_in && state.speaking.is_some() {
                state.queue.clear();
                vec![Action::CancelSpeak]
            } else {
                Vec::new()
            }
        }
        SttEvent::Partial { text, eager } => match state.mode {
            Mode::Auto => partial_actions(state, text, eager),
            Mode::Hold => {
                let Some(hold) = state.hold.as_mut() else {
                    // Audio keeps flowing so the provider session stays warm,
                    // but outside an open boundary nobody asked for a turn.
                    return Vec::new();
                };
                hold.last_partial = text;
                let combined = join_hold(hold);
                partial_actions(state, combined, eager)
            }
        },
        SttEvent::EndOfTurn { text } => {
            // A debt is paid by the very next end of turn, before anything else
            // looks at it and whatever is open at the time: the provider is
            // finishing the take that was cut out from under it, and none of
            // that belongs to a boundary opened since. Clearing the interim
            // with it drops whatever of the old take had already leaked in as a
            // partial. In `auto` this is what keeps a mode switch mid-drain
            // from delivering the same take a second time.
            if state.pending_provider_end {
                state.pending_provider_end = false;
                if let Some(hold) = state.hold.as_mut() {
                    hold.last_partial.clear();
                }
                return Vec::new();
            }
            match state.mode {
                Mode::Auto => turn_actions(state, session_id, text),
                Mode::Hold => {
                    let Some(hold) = state.hold.as_mut() else {
                        return Vec::new();
                    };
                    if !text.is_empty() {
                        hold.buffer.push(text);
                    }
                    // The provider started a new turn; what it heard before is
                    // settled, so the interim it was building is gone with it.
                    hold.last_partial.clear();
                    let draining = hold.draining;
                    if draining {
                        // This is exactly what `release` was waiting for: the
                        // provider has finished the audio it was given, so the turn
                        // is complete, closes here rather than at the cap, and
                        // leaves nothing outstanding.
                        cut_hold(state, session_id, ProviderDebt::Settled)
                    } else {
                        Vec::new()
                    }
                }
            }
        }
        // The cell does not un-emit. A turn that was already sent stays sent,
        // and the continuation becomes the next turn.
        SttEvent::TurnResumed => Vec::new(),
        // Reported and survived. The handler puts it on the error lane as
        // `stt_failed` with this session's id; the machine changes nothing,
        // because nothing about the turn changed.
        SttEvent::Warning { detail } => vec![Action::ToClient(ServerFrame::Error {
            code: WireErrorCode::SttFailed,
            detail,
            bad_frames: None,
        })],
        // The I/O half owns reconnecting; the only thing the session state has
        // to say is that a provider which is gone owes nothing. A debt kept
        // across the reconnect would be paid by the first real end of turn of
        // the NEXT take, which is the loss this whole boundary phase exists
        // against.
        SttEvent::Closed => {
            state.pending_provider_end = false;
            Vec::new()
        }
        SttEvent::Failed { detail } => {
            state.pending_provider_end = false;
            vec![Action::ToClient(ServerFrame::Error {
                code: WireErrorCode::SttFailed,
                detail,
                bad_frames: None,
            })]
        }
    }
}

/// Client control frames.
fn step_control(state: &mut SessionState, session_id: &str, frame: ClientFrame) -> Vec<Action> {
    match frame {
        ClientFrame::Hold => {
            if state.mode != Mode::Hold {
                return vec![wrong_mode("hold is only a frame in `hold` mode")];
            }
            let draining = matches!(&state.hold, Some(h) if h.draining);
            if state.hold.is_some() && !draining {
                return vec![error_frame(
                    WireErrorCode::AlreadyHolding,
                    "a turn boundary is already open",
                )];
            }
            // The key pressed again while the previous boundary is still
            // waiting for the provider ends that boundary NOW, with what it
            // has. Both halves of that are the point: the old take must not
            // disappear because somebody was quick, and the new one must not
            // inherit a word of it.
            let mut actions = if draining {
                // What the provider still owes belongs to the take that just
                // closed, not to the one opening here — recorded on the session
                // by `cut_hold`, because the answer may arrive before or after
                // this frame and both orders have to be right.
                cut_hold(state, session_id, ProviderDebt::Owed)
            } else {
                Vec::new()
            };
            state.hold = Some(HoldState::default());
            state.queue.clear();
            // Pressing the key is the interruption in this mode.
            if state.speaking.is_some() {
                actions.push(Action::CancelSpeak);
            }
            actions
        }
        ClientFrame::Release => {
            if state.mode != Mode::Hold {
                return vec![wrong_mode("release is only a frame in `hold` mode")];
            }
            // A boundary that is already draining is one the client has
            // released: from its side nothing is open, and a second `release`
            // is told the same thing a `release` without a `hold` is.
            if !matches!(&state.hold, Some(h) if !h.draining) {
                return vec![error_frame(
                    WireErrorCode::NotHolding,
                    "no turn boundary is open",
                )];
            }
            if state.release_grace_ms == 0 {
                // The behaviour before the grace existed, kept as a VALUE
                // rather than as history: a client whose provider ends turns
                // synchronously, or one that would rather have the last words
                // missing than the turn late, says so in `params`.
                return cut_hold(state, session_id, ProviderDebt::Settled);
            }
            // No new audio belongs to this turn from here on, but the provider
            // still owes the end of what it already has. The handler reads the
            // new generation off the session and arms the cap with it; whichever
            // of the two arrives first closes the turn.
            if let Some(hold) = state.hold.as_mut() {
                hold.draining = true;
            }
            state.grace_token = state.grace_token.wrapping_add(1);
            Vec::new()
        }
        ClientFrame::Cancel => {
            state.queue.clear();
            if state.speaking.is_some() {
                vec![Action::CancelSpeak]
            } else {
                Vec::new()
            }
        }
        ClientFrame::Mode { mode } => {
            let draining = matches!(&state.hold, Some(h) if h.draining);
            if state.hold.is_some() && !draining {
                return vec![error_frame(
                    WireErrorCode::AlreadyHolding,
                    "a turn boundary is open; release it before switching mode",
                )];
            }
            // A DRAINING boundary is not one the client can release — it
            // already did, and a second `release` is `not_holding` — so
            // refusing here would be a dead end for as long as the grace runs.
            // It also has to be closed rather than abandoned: in `auto` the
            // `EndOfTurn` still on its way would become a turn of its own, and
            // the take would arrive twice. So the switch cuts it first, with
            // what it has, exactly as a `hold` mid-drain does.
            let mut actions = if draining {
                cut_hold(state, session_id, ProviderDebt::Owed)
            } else {
                Vec::new()
            };
            state.mode = mode;
            actions.push(Action::ToClient(ServerFrame::Mode { mode }));
            actions
        }
    }
}

/// The lane emission and the client mirror of one interim transcript.
fn partial_actions(state: &SessionState, text: String, eager: bool) -> Vec<Action> {
    let mut actions = Vec::with_capacity(2);
    if state.emit_partials {
        actions.push(Action::EmitPartial {
            text: text.clone(),
            eager,
        });
    }
    actions.push(Action::ToClient(ServerFrame::Partial { text, eager }));
    actions
}

/// The lane emission and the client mirror of one finished turn.
///
/// An empty turn gets the frame and no lane emission: the client has to see its
/// boundary close, and the topology has no business receiving an empty user
/// message (the pipecat rule, R-V11).
fn turn_actions(state: &mut SessionState, session_id: &str, text: String) -> Vec<Action> {
    state.turn_seq += 1;
    let turn_id = format!("{session_id}#{}", state.turn_seq);
    let mut actions = Vec::with_capacity(2);
    if !text.is_empty() {
        actions.push(Action::EmitTurn {
            text: text.clone(),
            turn_id: turn_id.clone(),
        });
    }
    actions.push(Action::ToClient(ServerFrame::Turn { text, turn_id }));
    actions
}

/// Start the next queued synthesis if nothing is running.
fn start_next_if_idle(state: &mut SessionState) -> Vec<Action> {
    if state.speaking.is_some() {
        return Vec::new();
    }
    match state.queue.pop_front() {
        Some((speak_id, text)) => {
            state.speaking = Some(speak_id.clone());
            vec![Action::StartSpeak { speak_id, text }]
        }
        None => Vec::new(),
    }
}

/// What closing a boundary leaves the recognition provider owing.
///
/// Not a detail of the caller: it is the difference between a take that ended
/// because the provider said so and one that was cut out from under it, and
/// only the caller knows which of the two just happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderDebt {
    /// The provider has just delivered the end of this turn, or was never
    /// inside one of it — nothing is outstanding.
    Settled,
    /// The boundary was cut while the provider was still inside its turn, so
    /// its next `EndOfTurn` belongs to the take that has just closed.
    Owed,
}

/// Close the open boundary, whatever phase it is in, and hand back its one turn.
///
/// Everything that ends a boundary goes through here, so that the generation
/// counter moves exactly once per close — a timer still asleep for the boundary
/// that just ended names the generation before this one and does nothing when
/// it wakes — and so that [`SessionState::pending_provider_end`] is written in
/// exactly one place.
fn cut_hold(state: &mut SessionState, session_id: &str, debt: ProviderDebt) -> Vec<Action> {
    let Some(hold) = state.hold.take() else {
        return Vec::new();
    };
    state.grace_token = state.grace_token.wrapping_add(1);
    state.pending_provider_end = debt == ProviderDebt::Owed;
    let text = join_hold(&hold);
    turn_actions(state, session_id, text)
}

/// The hold buffer plus the interim that has not been ended yet, as one turn.
fn join_hold(hold: &HoldState) -> String {
    let mut parts: Vec<&str> = hold.buffer.iter().map(String::as_str).collect();
    if !hold.last_partial.is_empty() {
        parts.push(hold.last_partial.as_str());
    }
    parts.join(" ")
}

/// A `wrong_mode` refusal.
fn wrong_mode(detail: &str) -> Action {
    error_frame(WireErrorCode::WrongMode, detail)
}

/// A refusal frame, addressed to this connection only.
fn error_frame(code: WireErrorCode, detail: &str) -> Action {
    Action::ToClient(ServerFrame::Error {
        code,
        detail: detail.to_string(),
        bad_frames: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "s1";

    /// The shipped default grace, as the tests use it.
    const GRACE: u64 = 1500;

    fn auto() -> SessionState {
        SessionState::new(Mode::Auto, true, true, GRACE)
    }

    fn hold_mode() -> SessionState {
        SessionState::new(Mode::Hold, true, true, GRACE)
    }

    /// Drive a `release` and hand back the actions it produced.
    fn release(st: &mut SessionState) -> Vec<Action> {
        step(st, S, Input::Control(ClientFrame::Release))
    }

    /// The turn text of the one [`Action::EmitTurn`] in `actions`.
    fn emitted_turn(actions: &[Action]) -> Option<String> {
        let mut found = None;
        for a in actions {
            if let Action::EmitTurn { text, .. } = a {
                assert!(found.is_none(), "more than one turn in {actions:?}");
                found = Some(text.clone());
            }
        }
        found
    }

    fn partial(text: &str, eager: bool) -> Input {
        Input::Stt(SttEvent::Partial {
            text: text.to_string(),
            eager,
        })
    }

    fn eot(text: &str) -> Input {
        Input::Stt(SttEvent::EndOfTurn {
            text: text.to_string(),
        })
    }

    #[test]
    fn auto_partial_mirrors_and_emits() {
        let mut st = auto();
        let actions = step(&mut st, S, partial("hallo", false));
        assert_eq!(
            actions,
            vec![
                Action::EmitPartial {
                    text: "hallo".into(),
                    eager: false
                },
                Action::ToClient(ServerFrame::Partial {
                    text: "hallo".into(),
                    eager: false
                }),
            ]
        );

        let mut quiet = SessionState::new(Mode::Auto, true, false, GRACE);
        let actions = step(&mut quiet, S, partial("hallo", false));
        assert_eq!(
            actions,
            vec![Action::ToClient(ServerFrame::Partial {
                text: "hallo".into(),
                eager: false
            })],
            "emit_partials:false stops the lane, never the client mirror"
        );
    }

    #[test]
    fn auto_eager_partial_carries_flag() {
        let mut st = auto();
        let actions = step(&mut st, S, partial("hallo", true));
        assert_eq!(
            actions[0],
            Action::EmitPartial {
                text: "hallo".into(),
                eager: true
            }
        );
        assert_eq!(
            actions[1],
            Action::ToClient(ServerFrame::Partial {
                text: "hallo".into(),
                eager: true
            })
        );
    }

    #[test]
    fn auto_end_of_turn_emits_exactly_one_turn() {
        let mut st = auto();
        let actions = step(&mut st, S, eot("guten tag"));
        assert_eq!(
            actions,
            vec![
                Action::EmitTurn {
                    text: "guten tag".into(),
                    turn_id: "s1#1".into()
                },
                Action::ToClient(ServerFrame::Turn {
                    text: "guten tag".into(),
                    turn_id: "s1#1".into()
                }),
            ]
        );
        let second = step(&mut st, S, eot("and goodbye"));
        assert_eq!(
            second[0],
            Action::EmitTurn {
                text: "and goodbye".into(),
                turn_id: "s1#2".into()
            },
            "the counter is per session"
        );
        assert_eq!(
            second
                .iter()
                .filter(|a| matches!(a, Action::EmitTurn { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn auto_turn_resumed_changes_nothing() {
        let mut st = auto();
        step(&mut st, S, eot("guten tag"));
        assert_eq!(
            step(&mut st, S, Input::Stt(SttEvent::TurnResumed)),
            Vec::new(),
            "the cell does not un-emit; the continuation is the next turn"
        );
        assert_eq!(st.turn_seq, 1);
    }

    #[test]
    fn auto_speech_started_cancels_when_speaking() {
        let mut st = auto();
        st.speaking = Some("sp1".into());
        assert_eq!(
            step(&mut st, S, Input::Stt(SttEvent::SpeechStarted)),
            vec![Action::CancelSpeak]
        );

        let mut polite = SessionState::new(Mode::Auto, false, true, GRACE);
        polite.speaking = Some("sp1".into());
        assert_eq!(
            step(&mut polite, S, Input::Stt(SttEvent::SpeechStarted)),
            Vec::new(),
            "barge_in:false lets the agent finish"
        );

        let mut idle = auto();
        assert_eq!(
            step(&mut idle, S, Input::Stt(SttEvent::SpeechStarted)),
            Vec::new(),
            "nothing to interrupt"
        );
    }

    #[test]
    fn hold_outside_window_drops_events() {
        let mut st = hold_mode();
        assert_eq!(step(&mut st, S, partial("hallo", false)), Vec::new());
        assert_eq!(step(&mut st, S, eot("hallo")), Vec::new());
        assert_eq!(st.turn_seq, 0, "no turn happened");
    }

    #[test]
    fn hold_accumulates_end_of_turn_without_emitting() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        assert_eq!(step(&mut st, S, eot("a")), Vec::new());
        assert_eq!(step(&mut st, S, eot("b")), Vec::new());
        let actions = step(&mut st, S, partial("c", false));
        assert_eq!(
            actions[0],
            Action::EmitPartial {
                text: "a b c".into(),
                eager: false
            },
            "an interim in hold shows the whole boundary so far"
        );
        assert!(
            !actions.iter().any(|a| matches!(a, Action::EmitTurn { .. })),
            "no turn crosses an open boundary"
        );
    }

    #[test]
    fn release_emits_one_turn_with_buffer_and_last_partial() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        step(&mut st, S, eot("a"));
        step(&mut st, S, eot("b"));
        step(&mut st, S, partial("c", false));
        assert_eq!(
            release(&mut st),
            Vec::new(),
            "the frame no longer cuts; the turn waits for the provider or the cap"
        );
        let token = st.grace_token;
        let actions = step(&mut st, S, Input::ReleaseGraceExpired { token });
        assert_eq!(
            actions,
            vec![
                Action::EmitTurn {
                    text: "a b c".into(),
                    turn_id: "s1#1".into()
                },
                Action::ToClient(ServerFrame::Turn {
                    text: "a b c".into(),
                    turn_id: "s1#1".into()
                }),
            ]
        );
        assert!(st.hold.is_none(), "the boundary is closed");
    }

    #[test]
    fn release_with_nothing_said_emits_no_lane_but_a_frame() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        release(&mut st);
        let token = st.grace_token;
        let actions = step(&mut st, S, Input::ReleaseGraceExpired { token });
        assert_eq!(
            actions,
            vec![Action::ToClient(ServerFrame::Turn {
                text: String::new(),
                turn_id: "s1#1".into()
            })],
            "the client sees its boundary close; the topology sees no empty user message"
        );
    }

    /// The defect found on the test page, first half: the last real line
    /// of a take was missing. `release` cut on the frame, and Flux reports the
    /// end of the last words 400-700 ms later.
    #[test]
    fn release_waits_for_the_providers_end_of_turn() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        step(&mut st, S, partial("this is", false));
        assert_eq!(release(&mut st), Vec::new(), "nothing is cut yet");
        assert!(
            st.hold
                .as_ref()
                .expect("the boundary is still open")
                .draining,
            "released means draining, not closed"
        );

        let actions = step(&mut st, S, eot("this is the last sentence"));
        assert_eq!(
            emitted_turn(&actions).as_deref(),
            Some("this is the last sentence"),
            "exactly one turn, and it carries the words that were still in flight"
        );
        assert!(st.hold.is_none(), "the provider's end closed the boundary");
    }

    /// The other half of the same defect: the previous take turned up at the
    /// front of the next one. A late end-of-turn belongs to the boundary that
    /// was open when its audio was sent, and to no other.
    #[test]
    fn a_late_end_of_turn_does_not_leak_into_the_next_hold() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        step(&mut st, S, partial("erster take", false));
        release(&mut st);
        let token = st.grace_token;
        let first = step(&mut st, S, Input::ReleaseGraceExpired { token });
        assert_eq!(emitted_turn(&first).as_deref(), Some("erster take"));

        // The provider answers after the cap already cut. Too late is too late:
        // the turn it belongs to is gone, and it is not the next one's.
        assert_eq!(
            step(&mut st, S, eot("erster take vollstaendig")),
            Vec::new(),
            "an event outside a boundary is dropped, exactly as it always was"
        );

        step(&mut st, S, Input::Control(ClientFrame::Hold));
        let held = st.hold.as_ref().expect("the new boundary is open");
        assert!(held.buffer.is_empty(), "the new take starts empty");
        assert!(held.last_partial.is_empty());
        step(&mut st, S, partial("second take", false));
        release(&mut st);
        let second = step(&mut st, S, eot("second take"));
        assert_eq!(
            emitted_turn(&second).as_deref(),
            Some("second take"),
            "nothing of the first take is in the second"
        );
    }

    /// A provider that never reports an end must not hold a turn open for ever.
    #[test]
    fn the_grace_cap_cuts_with_the_last_partial() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        step(&mut st, S, eot("a"));
        step(&mut st, S, partial("b", false));
        release(&mut st);
        let token = st.grace_token;
        let actions = step(&mut st, S, Input::ReleaseGraceExpired { token });
        assert_eq!(
            emitted_turn(&actions).as_deref(),
            Some("a b"),
            "the cap cuts with the buffer plus the interim that was never ended"
        );
        assert!(st.hold.is_none());
    }

    /// Partials keep arriving while a boundary drains, and they belong to the
    /// turn that is closing: a client watching its own transcript would
    /// otherwise see it freeze exactly where the last words are still coming in.
    #[test]
    fn partials_during_draining_still_reach_the_client() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        step(&mut st, S, partial("halb", false));
        release(&mut st);
        let actions = step(&mut st, S, partial("halb gesagt", false));
        assert_eq!(
            actions,
            vec![
                Action::EmitPartial {
                    text: "halb gesagt".into(),
                    eager: false
                },
                Action::ToClient(ServerFrame::Partial {
                    text: "halb gesagt".into(),
                    eager: false
                }),
            ]
        );
        let token = st.grace_token;
        let cut = step(&mut st, S, Input::ReleaseGraceExpired { token });
        assert_eq!(
            emitted_turn(&cut).as_deref(),
            Some("halb gesagt"),
            "and the interim they carried is what the cap cuts with"
        );
    }

    /// Somebody who presses again before the previous take has closed gets two
    /// turns, in the right order, with nothing shared between them.
    #[test]
    fn a_hold_during_draining_closes_the_old_turn_first() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        step(&mut st, S, partial("erster", false));
        release(&mut st);

        let actions = step(&mut st, S, Input::Control(ClientFrame::Hold));
        assert_eq!(
            emitted_turn(&actions).as_deref(),
            Some("erster"),
            "the old boundary closes with what it had"
        );
        let held = st.hold.as_ref().expect("the new boundary is open");
        assert!(!held.draining, "and the new one is held, not draining");
        assert!(held.buffer.is_empty() && held.last_partial.is_empty());

        // The timer of the boundary that just closed wakes up and finds a
        // generation that is over.
        assert_eq!(
            step(&mut st, S, Input::ReleaseGraceExpired { token: 1 }),
            Vec::new(),
            "a stale token cuts nothing"
        );
        assert!(st.hold.is_some(), "and it did not take the new boundary");
    }

    /// The half of "the new take starts empty" that the buffer alone does not
    /// give: the provider is still inside the turn the OLD audio started, so
    /// its next `EndOfTurn` — and the interims on the way to it — carry the
    /// take that has just been cut. They belong to no boundary and must reach
    /// none.
    #[test]
    fn a_hold_during_draining_discards_the_old_takes_end_of_turn() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        step(&mut st, S, partial("erster", false));
        release(&mut st);
        let closed = step(&mut st, S, Input::Control(ClientFrame::Hold));
        assert_eq!(emitted_turn(&closed).as_deref(), Some("erster"));

        // The provider finally ends the turn the FIRST take began.
        assert_eq!(
            step(&mut st, S, eot("first one whole")),
            Vec::new(),
            "the late end of the old take is not a turn of the new one"
        );
        let held = st.hold.as_ref().expect("the new boundary is open");
        assert!(
            held.buffer.is_empty() && held.last_partial.is_empty(),
            "and it left nothing behind in the new take: {held:?}"
        );

        // From here the provider is in the new take, and it counts normally.
        step(&mut st, S, partial("second", false));
        release(&mut st);
        let second = step(&mut st, S, eot("second one whole"));
        assert_eq!(
            emitted_turn(&second).as_deref(),
            Some("second one whole"),
            "only the SECOND end of turn is discarded, and only once"
        );
    }

    /// The cap has the same debt as the key pressed again, and the client may
    /// press in either order.
    ///
    /// Here the cap cuts first and the client opens a new take BEFORE the
    /// provider answers. The old end then arrives inside a boundary that has
    /// nothing to do with it — which is exactly the leak, and exactly why the
    /// debt is on the session and not on the boundary that was cut.
    #[test]
    fn a_late_end_after_the_cap_does_not_reach_the_next_hold() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        step(&mut st, S, partial("erster", false));
        release(&mut st);
        let token = st.grace_token;
        let first = step(&mut st, S, Input::ReleaseGraceExpired { token });
        assert_eq!(emitted_turn(&first).as_deref(), Some("erster"));
        assert!(
            st.pending_provider_end,
            "the cap cut a take the provider had not finished"
        );

        step(&mut st, S, Input::Control(ClientFrame::Hold));
        assert_eq!(
            step(&mut st, S, eot("first one whole")),
            Vec::new(),
            "the old take's end is not a turn of the new one"
        );
        let held = st.hold.as_ref().expect("the new boundary is open");
        assert!(
            held.buffer.is_empty() && held.last_partial.is_empty(),
            "and it left nothing behind: {held:?}"
        );

        step(&mut st, S, partial("second", false));
        release(&mut st);
        let second = step(&mut st, S, eot("second one whole"));
        assert_eq!(
            emitted_turn(&second).as_deref(),
            Some("second one whole"),
            "the take after the debt counts normally"
        );
    }

    /// The other order of the same race: the provider answers while no boundary
    /// is open at all. The debt is paid there too — it has to be, or the next
    /// `hold` would collect it.
    #[test]
    fn a_debt_is_paid_with_no_boundary_open() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        step(&mut st, S, partial("erster", false));
        release(&mut st);
        let token = st.grace_token;
        step(&mut st, S, Input::ReleaseGraceExpired { token });

        assert_eq!(
            step(&mut st, S, eot("first one whole")),
            Vec::new(),
            "outside a boundary it produces nothing, as it always did"
        );
        assert!(
            !st.pending_provider_end,
            "but it is what the session was owed, and the debt is now settled"
        );

        step(&mut st, S, Input::Control(ClientFrame::Hold));
        step(&mut st, S, partial("second", false));
        release(&mut st);
        let second = step(&mut st, S, eot("second one whole"));
        assert_eq!(
            emitted_turn(&second).as_deref(),
            Some("second one whole"),
            "so the next take's own end is not eaten by a debt already paid"
        );
    }

    /// A provider that dies inside the window owes nothing, and a debt kept
    /// over the reconnect would eat the first real end of the NEXT take — the
    /// loss this whole boundary phase exists against, reintroduced by its own
    /// fix. Both ways a recognition session can end are written off.
    #[test]
    fn a_dead_provider_session_writes_the_debt_off() {
        for ending in [
            SttEvent::Closed,
            SttEvent::Failed {
                detail: "socket closed".into(),
            },
        ] {
            let mut st = hold_mode();
            step(&mut st, S, Input::Control(ClientFrame::Hold));
            step(&mut st, S, partial("erster", false));
            release(&mut st);
            step(&mut st, S, Input::Control(ClientFrame::Hold));
            assert!(st.pending_provider_end, "the old take was cut mid-turn");

            // The socket the debt was owed on is gone; the I/O half reconnects.
            step(&mut st, S, Input::Stt(ending.clone()));
            assert!(
                !st.pending_provider_end,
                "a provider that is gone owes nothing"
            );

            // The fresh session's first end of turn is the new take's own.
            step(&mut st, S, partial("neuer take", false));
            release(&mut st);
            let actions = step(&mut st, S, eot("new take whole"));
            assert_eq!(
                emitted_turn(&actions).as_deref(),
                Some("new take whole"),
                "and it must not be swallowed by a debt nobody can pay"
            );
        }
    }

    /// Switching mode mid-drain closes the boundary rather than refusing it.
    ///
    /// Two failures at once if it refused: the client cannot release a boundary
    /// it has already released (`not_holding`), so it would be stuck for as
    /// long as the grace runs — and in `auto` the `EndOfTurn` still on its way
    /// would have become a turn of its own, so the take would arrive twice.
    #[test]
    fn a_mode_switch_during_draining_closes_the_turn_first() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        step(&mut st, S, partial("letztes wort", false));
        release(&mut st);

        let actions = step(
            &mut st,
            S,
            Input::Control(ClientFrame::Mode { mode: Mode::Auto }),
        );
        assert_eq!(
            emitted_turn(&actions).as_deref(),
            Some("letztes wort"),
            "the take that was closing is not thrown away by a mode switch"
        );
        assert_eq!(
            actions.last(),
            Some(&Action::ToClient(ServerFrame::Mode { mode: Mode::Auto })),
            "and the switch is acknowledged after it, not instead of it: {actions:?}"
        );
        assert_eq!(st.mode, Mode::Auto);
        assert!(st.hold.is_none());

        assert_eq!(
            step(&mut st, S, Input::ReleaseGraceExpired { token: 1 }),
            Vec::new(),
            "the cap that was armed for it wakes into a turn that is over"
        );

        // And the take does not arrive twice: in `auto` the end of turn still
        // on its way would otherwise become a turn of its own.
        assert!(st.pending_provider_end);
        assert_eq!(
            step(&mut st, S, eot("last word whole")),
            Vec::new(),
            "the provider's answer to audio from before the switch is not a \
             second delivery of the same take"
        );
        assert_eq!(st.turn_seq, 1, "exactly one turn came out of that take");
    }

    /// A second `release` while the first is still draining is not a boundary
    /// being closed twice; from the client's side there is nothing open.
    #[test]
    fn a_second_release_while_draining_is_not_holding() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        release(&mut st);
        let actions = release(&mut st);
        assert!(matches!(
            actions.as_slice(),
            [Action::ToClient(ServerFrame::Error {
                code: WireErrorCode::NotHolding,
                ..
            })]
        ));
        assert!(
            st.hold.as_ref().expect("still draining").draining,
            "the refusal changed nothing about the turn that is closing"
        );
    }

    /// `release_grace_ms: 0` is the behaviour this cell had before the grace
    /// existed, and it is a value rather than a piece of history.
    #[test]
    fn grace_zero_keeps_the_old_behaviour() {
        let mut st = SessionState::new(Mode::Hold, true, true, 0);
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        step(&mut st, S, partial("sofort", false));
        let actions = release(&mut st);
        assert_eq!(
            emitted_turn(&actions).as_deref(),
            Some("sofort"),
            "the frame itself cuts"
        );
        assert!(st.hold.is_none());
        assert_eq!(
            step(&mut st, S, eot("at once and more")),
            Vec::new(),
            "and what comes after is outside every boundary, as before"
        );
    }

    /// A timer for a turn the provider already closed is a no-op — not a second
    /// turn, and not an empty one.
    #[test]
    fn a_stale_grace_expiry_after_a_providers_end_changes_nothing() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        release(&mut st);
        let armed = st.grace_token;
        let closed = step(&mut st, S, eot("fertig"));
        assert_eq!(emitted_turn(&closed).as_deref(), Some("fertig"));
        let seq = st.turn_seq;

        assert_eq!(
            step(&mut st, S, Input::ReleaseGraceExpired { token: armed }),
            Vec::new(),
            "the timer wakes into a turn that is over"
        );
        assert_eq!(st.turn_seq, seq, "and counts nothing");
    }

    #[test]
    fn hold_in_auto_mode_is_wrong_mode() {
        let mut st = auto();
        let actions = step(&mut st, S, Input::Control(ClientFrame::Hold));
        assert!(matches!(
            actions.as_slice(),
            [Action::ToClient(ServerFrame::Error {
                code: WireErrorCode::WrongMode,
                ..
            })]
        ));
        assert!(st.hold.is_none());
    }

    #[test]
    fn double_hold_is_already_holding() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        let actions = step(&mut st, S, Input::Control(ClientFrame::Hold));
        assert!(matches!(
            actions.as_slice(),
            [Action::ToClient(ServerFrame::Error {
                code: WireErrorCode::AlreadyHolding,
                ..
            })]
        ));
    }

    #[test]
    fn release_without_hold_is_not_holding() {
        let mut st = hold_mode();
        let actions = step(&mut st, S, Input::Control(ClientFrame::Release));
        assert!(matches!(
            actions.as_slice(),
            [Action::ToClient(ServerFrame::Error {
                code: WireErrorCode::NotHolding,
                ..
            })]
        ));
        assert_eq!(st.turn_seq, 0);
    }

    #[test]
    fn hold_cancels_running_speak() {
        let mut st = hold_mode();
        st.speaking = Some("sp1".into());
        st.queue.push_back(("sp2".into(), "next".into()));
        let actions = step(&mut st, S, Input::Control(ClientFrame::Hold));
        assert_eq!(actions, vec![Action::CancelSpeak]);
        assert!(st.queue.is_empty(), "barge-in by key empties the queue too");
        assert!(st.hold.is_some());
    }

    #[test]
    fn mode_switch_during_hold_is_refused() {
        let mut st = hold_mode();
        step(&mut st, S, Input::Control(ClientFrame::Hold));
        let actions = step(
            &mut st,
            S,
            Input::Control(ClientFrame::Mode { mode: Mode::Auto }),
        );
        assert!(matches!(
            actions.as_slice(),
            [Action::ToClient(ServerFrame::Error {
                code: WireErrorCode::AlreadyHolding,
                ..
            })]
        ));
        assert_eq!(st.mode, Mode::Hold, "the refusal changed nothing");
    }

    #[test]
    fn mode_switch_acknowledged() {
        let mut st = auto();
        let actions = step(
            &mut st,
            S,
            Input::Control(ClientFrame::Mode { mode: Mode::Hold }),
        );
        assert_eq!(
            actions,
            vec![Action::ToClient(ServerFrame::Mode { mode: Mode::Hold })]
        );
        assert_eq!(st.mode, Mode::Hold);
    }

    #[test]
    fn speak_queues_fifo_and_starts_when_idle() {
        let mut st = auto();
        let a = step(
            &mut st,
            S,
            Input::Speak {
                speak_id: "A".into(),
                text: "erst".into(),
            },
        );
        assert_eq!(
            a,
            vec![Action::StartSpeak {
                speak_id: "A".into(),
                text: "erst".into()
            }]
        );
        let b = step(
            &mut st,
            S,
            Input::Speak {
                speak_id: "B".into(),
                text: "dann".into(),
            },
        );
        assert_eq!(b, Vec::new(), "no preemptive TTS, and no second voice");
        let done = step(
            &mut st,
            S,
            Input::SpeakEnded {
                speak_id: "A".into(),
                reason: SpeakEndReason::Done,
            },
        );
        assert_eq!(
            done,
            vec![Action::StartSpeak {
                speak_id: "B".into(),
                text: "dann".into()
            }]
        );
    }

    #[test]
    fn cancel_clears_queue() {
        let mut st = auto();
        step(
            &mut st,
            S,
            Input::Speak {
                speak_id: "A".into(),
                text: "erst".into(),
            },
        );
        step(
            &mut st,
            S,
            Input::Speak {
                speak_id: "B".into(),
                text: "dann".into(),
            },
        );
        let actions = step(&mut st, S, Input::Control(ClientFrame::Cancel));
        assert_eq!(actions, vec![Action::CancelSpeak]);
        assert!(st.queue.is_empty());
        let after = step(
            &mut st,
            S,
            Input::SpeakEnded {
                speak_id: "A".into(),
                reason: SpeakEndReason::Cancelled,
            },
        );
        assert_eq!(after, Vec::new(), "a cancelled queue starts nothing");
        assert!(st.speaking.is_none());
    }

    /// Deepgram Flux sends a `StartOfTurn` that already carries a transcript,
    /// so the adapter splits it into `SpeechStarted` + `Partial`. Both halves
    /// arrive back to back, and the machine has to take them in that order: the
    /// barge-in first, the transcript behind it, and — outside an open `hold` —
    /// neither.
    #[test]
    fn speech_started_then_partial_in_one_turn() {
        let mut st = auto();
        st.speaking = Some("sp1".into());
        assert_eq!(
            step(&mut st, S, Input::Stt(SttEvent::SpeechStarted)),
            vec![Action::CancelSpeak],
            "the interruption comes first"
        );
        let actions = step(&mut st, S, partial("guten", false));
        assert_eq!(
            actions,
            vec![
                Action::EmitPartial {
                    text: "guten".into(),
                    eager: false
                },
                Action::ToClient(ServerFrame::Partial {
                    text: "guten".into(),
                    eager: false
                }),
            ],
            "and the transcript the same event carried arrives behind it"
        );

        let mut held = hold_mode();
        held.speaking = Some("sp1".into());
        assert_eq!(
            step(&mut held, S, Input::Stt(SttEvent::SpeechStarted)),
            Vec::new(),
            "in hold the key is the interruption, not the voice"
        );
        assert_eq!(
            step(&mut held, S, partial("guten", false)),
            Vec::new(),
            "and outside an open boundary nobody asked for a transcript"
        );
    }

    /// R-V17: a warning is loud and harmless. The one failure mode worth a
    /// test is the opposite pair — a warning that ends a call, or one that is
    /// swallowed and leaves a caller wondering why a sentence went missing.
    #[test]
    fn warning_reports_without_ending_the_session() {
        let mut st = auto();
        step(&mut st, S, eot("guten tag"));
        let before = st.turn_seq;
        let actions = step(
            &mut st,
            S,
            Input::Stt(SttEvent::Warning {
                detail: "item 3 could not be transcribed".into(),
            }),
        );
        assert_eq!(
            actions,
            vec![Action::ToClient(ServerFrame::Error {
                code: WireErrorCode::SttFailed,
                detail: "item 3 could not be transcribed".into(),
                bad_frames: None
            })],
            "reported, not swallowed"
        );
        assert_eq!(st.turn_seq, before, "and nothing about the turn moved");
        // The session is still usable.
        let after = step(&mut st, S, partial("go on", false));
        assert_eq!(after.len(), 2, "the call goes on: {after:?}");
    }

    #[test]
    fn a_failed_provider_session_reaches_the_client() {
        let mut st = auto();
        let actions = step(
            &mut st,
            S,
            Input::Stt(SttEvent::Failed {
                detail: "socket closed".into(),
            }),
        );
        assert_eq!(
            actions,
            vec![Action::ToClient(ServerFrame::Error {
                code: WireErrorCode::SttFailed,
                detail: "socket closed".into(),
                bad_frames: None
            })],
            "never silent"
        );
    }
}
