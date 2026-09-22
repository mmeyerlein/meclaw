//! Turn formation on the **model's** timeline (R-25-9), as a pure function.
//!
//! A duplex model transcribes both voices and stamps every fragment with the
//! clock of the session rather than with the moment the frame arrived. That is
//! what makes turn formation a function here and a state machine with sockets
//! in [`crate::voice::turns`]: there is no endpointing to wait for, no hold to
//! honour, no synthesis queue to drain — only fragments, a gap and two numbers.
//!
//! The cascade machine is NOT touched by any of this. The duplex path never
//! instantiates it, and the two live side by side.
//!
//! # The rules, in one place (R-25-9, OR-L10, OR-L18)
//!
//! 1. A user turn is closed when `gap_ms` passes on the MODEL's clock with no
//!    further user fragment — measured from the `end_ms` of the last user
//!    fragment against the `start_ms` of the next one, or against the `now_ms`
//!    of a [`TurnInput::Tick`]. Strictly greater than the gap, never equal
//!    (OR-L10): the reference run has two fragments exactly `gap_ms` apart and
//!    they are one sentence.
//! 2. Assistant fragments up to the next user fragment belong to the open turn.
//! 3. A user run inside an agent block that stays at or under
//!    `backchannel_max_ms` is a backchannel: it joins the open turn and closes
//!    nothing. A run that exceeds it is a barge-in — [`TurnAction::BargeIn`]
//!    exactly once per agent BLOCK (OR-L18), not once per turn — and closes
//!    the open turn. A block ends where the model falls silent for longer than
//!    `gap_ms`, and the next block brings the right to interrupt back.
//! 4. An agent fragment that starts BEFORE the last user fragment ended
//!    (the two streams overlap by about 400 ms) cuts nothing.
//! 5. A turn with no assistant part passes as it is.
//! 6. [`TurnInput::Close`] closes whatever is open.
//! 7. `happened_at_ms` is the `end_ms` of the turn's last fragment; `index`
//!    counts from 0.
//! 8. Every fragment also yields a [`TurnAction::Partial`] with the CUMULATIVE
//!    text of the part it belongs to — the wire semantics of `partial`, where a
//!    new one replaces the previous one.
//!
//! Rules 1 and 3 both want the same user fragment, and the line between them
//! is the overlap itself: a fragment that starts while the model's transcript
//! is still running (`start_ms` before the block's furthest `end_ms`) is a run
//! inside the block and goes to rule 3; every other one goes to rule 1. Both
//! halves are measured — the reference run closes its first turn on a fragment
//! that begins 800 ms AFTER the model fell silent, and the backchannel of the
//! same call begins in the middle of an answer still being spoken. A grace
//! period on top of the block would swallow the first, no block test at all
//! the second.

use crate::voice::contract::Speaker;

/// One transcript fragment as the model timed it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Frag {
    /// The words.
    text: String,
    /// Where the fragment starts on the model's clock.
    start_ms: u64,
    /// Where it ends.
    end_ms: u64,
}

/// The turn that is currently being formed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct OpenTurn {
    /// What the caller has said in this turn so far.
    user: Vec<Frag>,
    /// What the model has said in it.
    assistant: Vec<Frag>,
    /// Where the last user fragment ended — the anchor rule 1 measures from.
    user_last_end_ms: u64,
    /// When the running agent block started, where one is running.
    agent_block_start_ms: Option<u64>,
    /// How far the running block has spoken — the block is running exactly as
    /// long as it has timeline left, and that is what tells a backchannel from
    /// a reply.
    agent_block_end_ms: u64,
    /// How long the current uninterrupted user run has been, in milliseconds.
    user_run_ms: u64,
    /// How many trailing `user` fragments that run is made of: a barge-in hands
    /// the WHOLE run to the turn it opens, including the part that still looked
    /// like a backchannel when it arrived.
    user_run_frags: usize,
    /// Whether the agent block that is running has already produced its one
    /// `BargeIn`. Cleared when a new block starts, never by a new turn.
    barged: bool,
}

impl OpenTurn {
    /// A turn that begins with something the caller said.
    fn with_user(frag: Frag) -> Self {
        Self {
            user_last_end_ms: frag.end_ms,
            user_run_ms: frag.end_ms.saturating_sub(frag.start_ms),
            user_run_frags: 1,
            user: vec![frag],
            ..Self::default()
        }
    }

    /// The turn a barge-in opens: the run that interrupted, and the block it
    /// interrupted, which is still running and has already had its one
    /// `BargeIn` (OR-L18 — the model needs a moment to stop, and the fragments
    /// of that moment must not buy a second cancel). The flag travels with the
    /// BLOCK, not with the turn: the model's next answer is a new block and is
    /// interruptible again (see `push_assistant`).
    fn from_barge_in(
        run: Vec<Frag>,
        block_start: Option<u64>,
        block_end: u64,
        run_ms: u64,
    ) -> Self {
        Self {
            user_last_end_ms: fragment_end(&run),
            user_run_frags: run.len(),
            user: run,
            assistant: Vec::new(),
            agent_block_start_ms: block_start,
            agent_block_end_ms: block_end,
            user_run_ms: run_ms,
            barged: true,
        }
    }

    /// Everything the caller has said in this turn, in one string.
    fn user_text(&self) -> String {
        self.user.iter().map(|f| f.text.as_str()).collect()
    }

    /// Everything the model has said in it, where it said anything.
    fn assistant_text(&self) -> Option<String> {
        if self.assistant.is_empty() {
            return None;
        }
        Some(self.assistant.iter().map(|f| f.text.as_str()).collect())
    }

    /// Whether the model's transcript is still running at `at_ms`.
    fn model_speaks_at(&self, at_ms: u64) -> bool {
        self.agent_block_start_ms.is_some() && at_ms < self.agent_block_end_ms
    }

    /// Take a user fragment into the open turn and grow the run it belongs to.
    fn push_user(&mut self, frag: Frag) {
        self.user_run_ms += frag.end_ms.saturating_sub(frag.start_ms);
        self.user_run_frags += 1;
        self.user_last_end_ms = frag.end_ms;
        self.user.push(frag);
    }

    /// Take an agent fragment. It opens a block where none is running, never
    /// cuts anything (rule 4), and ends the user's run: a run is the user
    /// fragments with no agent fragment between them.
    ///
    /// A fragment that starts more than `gap_ms` after the block last spoke is
    /// a NEW block — a second answer — and with it the right to interrupt
    /// comes back (OR-L18 counts blocks, not turns). The same number rule 1
    /// measures silence with is the line here, because it is the same silence:
    /// inside one sentence the raster leaves holes of 200–600 ms (fixture
    /// events 44→45 and 51→52), and a tail the model still trickles out after
    /// a cancel is contiguous with the block it belongs to. Without this, a
    /// call has at most one `BargeIn` per turn: the second interruption never
    /// reaches the connection, no `speak_end cancelled`, no `uuid_break`, and
    /// FS02 plays the answer it already holds over the caller — while the
    /// model keeps speaking 4.4–11.0 s after a disturbance (S0).
    fn push_assistant(&mut self, frag: Frag, gap_ms: u64) {
        let new_block = self.agent_block_start_ms.is_none()
            || frag.start_ms.saturating_sub(self.agent_block_end_ms) > gap_ms;
        if new_block {
            self.agent_block_start_ms = Some(frag.start_ms);
            self.agent_block_end_ms = frag.end_ms;
            self.barged = false;
        } else {
            self.agent_block_end_ms = self.agent_block_end_ms.max(frag.end_ms);
        }
        self.user_run_ms = 0;
        self.user_run_frags = 0;
        self.assistant.push(frag);
    }

    /// Cut the current user run off the turn; what is left is the turn the
    /// barge-in closed.
    fn take_run(&mut self) -> Vec<Frag> {
        let at = self.user.len().saturating_sub(self.user_run_frags);
        self.user.split_off(at)
    }

    /// The turn as it goes on the wire.
    fn finish(&self, index: u64) -> FinishedTurn {
        FinishedTurn {
            index,
            user: self.user_text(),
            assistant: self.assistant_text(),
            happened_at_ms: fragment_end(&self.user).max(fragment_end(&self.assistant)),
        }
    }
}

/// Where the last of these fragments ends, or 0 where there are none.
fn fragment_end(frags: &[Frag]) -> u64 {
    frags.iter().map(|f| f.end_ms).max().unwrap_or(0)
}

/// Everything turn formation remembers between two fragments.
///
/// Owned by the handler, one per live duplex session. No clock of its own: the
/// only time it knows is the time a fragment carries.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TurnState {
    /// The turn being formed, if one is.
    open: Option<OpenTurn>,
    /// The user text of the last turn that closed — what a delegation falls
    /// back to when no turn is open.
    last_closed_user_text: Option<String>,
    /// How many turns this session has closed; the next one's `index`.
    index: u64,
}

impl TurnState {
    /// A session that has heard nothing yet.
    pub fn new() -> Self {
        Self::default()
    }
}

/// What goes into the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnInput {
    /// One transcript fragment, on the model's clock.
    Fragment {
        /// Whose words these are.
        speaker: Speaker,
        /// The words.
        text: String,
        /// Where the fragment starts.
        start_ms: u64,
        /// Where it ends.
        end_ms: u64,
    },
    /// Time passed and nothing was said. The session's own clock carries this,
    /// so a turn can close while nobody speaks (R-L7, GH #798).
    Tick {
        /// Where the clock stands now, on the model's timeline: the last
        /// offset the provider stamped, plus the time since it arrived.
        now_ms: u64,
    },
    /// The session is over; close what is open.
    Close,
}

/// What the machine decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnAction {
    /// A turn is finished and belongs on the `turn` lane.
    Emit(FinishedTurn),
    /// The cumulative text of the open turn's part for this speaker. It
    /// REPLACES the previous one, like every `partial` on this wire.
    Partial {
        /// Whose part this is.
        speaker: Speaker,
        /// The whole of it so far.
        text: String,
    },
    /// A user run inside an agent block exceeded `backchannel_max_ms`. Emitted
    /// exactly once per agent block (OR-L18); the connection turns it into a
    /// cancelled `speak_end` where `barge_in` is on.
    BargeIn,
}

/// A closed turn, ready for the `turn` lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedTurn {
    /// Which turn of the session this is, counting from 0.
    pub index: u64,
    /// Everything the caller said in it.
    pub user: String,
    /// Everything the model said in it, where it said anything.
    pub assistant: Option<String>,
    /// The `end_ms` of the turn's last fragment, on the model's clock.
    pub happened_at_ms: u64,
}

/// Feed one input to the machine and read its verdict.
///
/// `gap_ms` and `backchannel_max_ms` are params rather than constants
/// (`params.duplex.turn_gap_ms`, `params.duplex.backchannel_max_ms`): both were
/// measured on one model with one language, and a number like that belongs
/// where an operator can move it.
///
/// The actions come back in the order they happened: a `BargeIn` before the
/// turn it closed, a closed turn before the `Partial` of the turn that took its
/// place.
pub fn step(
    state: &mut TurnState,
    input: TurnInput,
    gap_ms: u64,
    backchannel_max_ms: u64,
) -> Vec<TurnAction> {
    match input {
        TurnInput::Fragment {
            speaker,
            text,
            start_ms,
            end_ms,
        } => {
            let frag = Frag {
                text,
                start_ms,
                end_ms,
            };
            match speaker {
                Speaker::User => user_fragment(state, frag, gap_ms, backchannel_max_ms),
                Speaker::Assistant => assistant_fragment(state, frag, gap_ms),
            }
        }
        TurnInput::Tick { now_ms } => tick(state, now_ms, gap_ms),
        TurnInput::Close => close(state).into_iter().collect(),
    }
}

/// The user text of the turn that is open, or the last one that closed.
///
/// This is what a `delegation` carries: the model asked for backend work in the
/// middle of a sentence, and what the backend needs is the sentence — the
/// delegation event itself carries no task text at all. An open turn with
/// nothing in the caller's part yet — the model greeting first — is no
/// sentence, so that case falls back too.
pub fn open_user_text(state: &TurnState) -> Option<String> {
    match state.open.as_ref().map(OpenTurn::user_text) {
        Some(text) if !text.is_empty() => Some(text),
        _ => state.last_closed_user_text.clone(),
    }
}

/// Rules 1, 3, 4 and 8 for a fragment of the caller's.
fn user_fragment(
    state: &mut TurnState,
    frag: Frag,
    gap_ms: u64,
    backchannel_max_ms: u64,
) -> Vec<TurnAction> {
    // Rule 3 takes the fragment while the model is still talking; rule 1 takes
    // every other one, and where its gap ran out the turn closes BEFORE the
    // fragment lands, so the fragment belongs to the turn it opens.
    let in_block = state
        .open
        .as_ref()
        .is_some_and(|open| open.model_speaks_at(frag.start_ms));
    let cuts = !in_block
        && state
            .open
            .as_ref()
            .is_some_and(|open| frag.start_ms.saturating_sub(open.user_last_end_ms) > gap_ms);

    let mut out = Vec::new();
    if cuts {
        out.extend(close(state));
    }
    let mut barge = None;
    match state.open.as_mut() {
        None => state.open = Some(OpenTurn::with_user(frag)),
        Some(open) => {
            open.push_user(frag);
            if in_block && open.user_run_ms > backchannel_max_ms && !open.barged {
                barge = Some((
                    open.take_run(),
                    open.agent_block_start_ms,
                    open.agent_block_end_ms,
                    open.user_run_ms,
                ));
            }
        }
    }
    if let Some((run, block_start, block_end, run_ms)) = barge {
        // The run stopped being a listener and became a speaker. The
        // announcement comes first: it is what cancels the audio already on its
        // way to the caller.
        out.push(TurnAction::BargeIn);
        out.extend(close(state));
        state.open = Some(OpenTurn::from_barge_in(run, block_start, block_end, run_ms));
    }
    if let Some(open) = state.open.as_ref() {
        out.push(TurnAction::Partial {
            speaker: Speaker::User,
            text: open.user_text(),
        });
    }
    out
}

/// Rules 2, 4 and 8 for a fragment of the model's.
fn assistant_fragment(state: &mut TurnState, frag: Frag, gap_ms: u64) -> Vec<TurnAction> {
    // The model speaking with nothing open is the greeting: it opens a turn of
    // its own rather than being dropped, because a greeting the memory never
    // sees is a call that starts with a hole in it.
    let open = state.open.get_or_insert_with(OpenTurn::default);
    open.push_assistant(frag, gap_ms);
    vec![TurnAction::Partial {
        speaker: Speaker::Assistant,
        text: open.assistant_text().unwrap_or_default(),
    }]
}

/// Rule 1 against the clock instead of against a fragment.
fn tick(state: &mut TurnState, now_ms: u64, gap_ms: u64) -> Vec<TurnAction> {
    // Silence measured from a fragment that does not exist is not silence, and
    // neither is a pause while the model is still talking.
    let closes = state.open.as_ref().is_some_and(|open| {
        !open.user.is_empty()
            && !open.model_speaks_at(now_ms)
            && now_ms.saturating_sub(open.user_last_end_ms) > gap_ms
    });
    if closes {
        close(state).into_iter().collect()
    } else {
        Vec::new()
    }
}

/// Rules 6 and 7: hand over what is open and count it.
fn close(state: &mut TurnState) -> Option<TurnAction> {
    let open = state.open.take()?;
    let finished = open.finish(state.index);
    state.index += 1;
    if !finished.user.is_empty() {
        state.last_closed_user_text = Some(finished.user.clone());
    }
    Some(TurnAction::Emit(finished))
}
