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
//!    they are one sentence. A tick closes it only once the model's block is
//!    over too (rule 3): a model that answered is silent for the gap first.
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
    /// Where the caller's last words before the answer ended (D48, LC2):
    /// moved by a caller fragment outside a running block only. A backchannel
    /// inside the answer is no "last words before it", and the run a barge-in
    /// cuts off leaves with the turn it opens (review M3).
    person_end_ms: Option<u64>,
    /// Which agent fragment counts as the answer to `person_end_ms`.
    awaiting: Awaiting,
    /// When the first audible audio frame of that answer went out to the client, on
    /// the call's clock ([`TurnInput::AudioOut`]); never before
    /// `person_end_ms`.
    first_audio_ms: Option<u64>,
}

/// Which agent fragment answers the caller's last words (review M3).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Awaiting {
    /// None: nothing was said to answer, or the answer is in.
    #[default]
    Nothing,
    /// The next agent fragment.
    NextFragment,
    /// The next agent BLOCK: after a barge-in the block that was interrupted
    /// keeps trickling for seconds (S0: 4.4-11.0 s), and its tail is no answer.
    NextBlock,
}

impl OpenTurn {
    /// A turn that begins with something the caller said.
    fn with_user(frag: Frag) -> Self {
        Self {
            user_last_end_ms: frag.end_ms,
            user_run_ms: frag.end_ms.saturating_sub(frag.start_ms),
            user_run_frags: 1,
            person_end_ms: Some(frag.end_ms),
            awaiting: Awaiting::NextFragment,
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
            person_end_ms: Some(fragment_end(&run)),
            awaiting: Awaiting::NextBlock,
            user_run_frags: run.len(),
            user: run,
            assistant: Vec::new(),
            agent_block_start_ms: block_start,
            agent_block_end_ms: block_end,
            user_run_ms: run_ms,
            barged: true,
            first_audio_ms: None,
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
    /// Outside a running block it is the caller's latest word to answer.
    ///
    /// The caller's words stand in the order they were spoken, not the order
    /// their transcripts arrived (d54g callseam). One session stamps in order;
    /// across a takeover two do not: the session that was replaced still
    /// reports a word it heard before the takeover after the new one reported
    /// the next (a live model's transcript lags its audio). Such a late word
    /// goes where it lies on the call's timeline and moves no anchor: it is
    /// not the caller's latest word.
    fn push_user(&mut self, frag: Frag, in_block: bool) {
        let at = self.user.partition_point(|f| f.start_ms <= frag.start_ms);
        if at < self.user.len() {
            let run_from = self.user.len().saturating_sub(self.user_run_frags);
            if at > run_from {
                self.user_run_ms += frag.end_ms.saturating_sub(frag.start_ms);
                self.user_run_frags += 1;
            }
            self.user_last_end_ms = self.user_last_end_ms.max(frag.end_ms);
            self.user.insert(at, frag);
            return;
        }
        if !in_block {
            self.person_end_ms = Some(frag.end_ms);
            self.awaiting = Awaiting::NextFragment;
            self.first_audio_ms = None;
        }
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
        let new_block = opens_new_block(
            self.agent_block_start_ms.map(|_| self.agent_block_end_ms),
            frag.start_ms,
            gap_ms,
        );
        if new_block {
            self.agent_block_start_ms = Some(frag.start_ms);
            self.agent_block_end_ms = frag.end_ms;
            self.barged = false;
        } else {
            self.agent_block_end_ms = self.agent_block_end_ms.max(frag.end_ms);
        }
        self.user_run_ms = 0;
        self.user_run_frags = 0;
        let answers = match self.awaiting {
            Awaiting::Nothing => false,
            Awaiting::NextFragment => true,
            Awaiting::NextBlock => new_block,
        };
        if answers {
            self.awaiting = Awaiting::Nothing;
        }
        self.assistant.push(frag);
    }

    /// Audio went out to the client at `at_ms` on the call's clock. The first
    /// frame after the caller's last words is the answer's first audio --
    /// except after a barge-in, where the interrupted block keeps trickling
    /// (S0: 4.4-11.0 s) and only audio after the new block's first
    /// transcript fragment counts (`Awaiting::NextBlock`).
    ///
    /// Why audio and not the answer's transcript (BAT R1e, C3): a duplex
    /// model stamps its answer's words where the caller's ended, on its own
    /// 200 ms raster, so 12 of 15 telephone turns read a time to the first
    /// word of exactly 0 ms.
    fn audio_out(&mut self, at_ms: u64) {
        let Some(person_end) = self.person_end_ms else {
            return;
        };
        if self.first_audio_ms.is_none() && self.awaiting != Awaiting::NextBlock {
            self.first_audio_ms = Some(at_ms.max(person_end));
        }
    }

    /// Cut the current user run off the turn; what is left is the turn the
    /// barge-in closed.
    fn take_run(&mut self) -> Vec<Frag> {
        let at = self.user.len().saturating_sub(self.user_run_frags);
        let run = self.user.split_off(at);
        // The anchor never names words that left with the run.
        let left = fragment_end(&self.user);
        if self.person_end_ms.is_some_and(|end| end > left) {
            self.person_end_ms = (!self.user.is_empty()).then_some(left);
        }
        run
    }

    /// The turn as it goes on the wire.
    fn finish(&self, index: u64) -> FinishedTurn {
        FinishedTurn {
            index,
            user: self.user_text(),
            assistant: self.assistant_text(),
            happened_at_ms: fragment_end(&self.user).max(fragment_end(&self.assistant)),
            person_end_ms: self.person_end_ms,
            first_audio_ms: self.first_audio_ms,
            cut: false,
        }
    }
}

/// Whether a fragment of the model's that starts at `start_ms` opens a NEW
/// block: no block is running (`block_end_ms` is `None`), or the model was
/// silent for longer than `gap_ms` since the running one last spoke (rule 3,
/// OR-L18). Strictly greater than the gap, like rule 1.
///
/// One function for its two readers: the open turn, and a session parked
/// while its line rings, where the second block is what the model says
/// after its greeting to a caller who is not there yet (GH #1055).
pub(crate) fn opens_new_block(block_end_ms: Option<u64>, start_ms: u64, gap_ms: u64) -> bool {
    match block_end_ms {
        None => true,
        Some(end_ms) => start_ms.saturating_sub(end_ms) > gap_ms,
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
    /// The closed turns of the call, kept for a delegation's history where
    /// `duplex.delegation_history` asks for it; `None` keeps nothing.
    history: Option<CallHistory>,
}

/// The closed turns of one call, as fragments. Whole: this cell produces the
/// history and cuts none of it; what fits the target's window is the
/// curator's to decide, on the way to the target.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct CallHistory {
    /// The closed turns, oldest first.
    turns: Vec<HistoryTurn>,
}

/// One closed turn of the call, as the history keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HistoryTurn {
    /// The turn's `index` in the session ([`FinishedTurn::index`]).
    index: u64,
    /// The id the turn carried on the `turn` lane, once it was reported
    /// ([`name_reported_turn`]); `None` before that.
    turn_id: Option<String>,
    /// The caller's part.
    user: Vec<Frag>,
    /// The model's part.
    assistant: Vec<Frag>,
}

impl TurnState {
    /// A session that has heard nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// The same, keeping the call's closed turns for a delegation's history
    /// (`duplex.delegation_history`), all of them.
    pub fn with_history() -> Self {
        Self {
            history: Some(CallHistory::default()),
            ..Self::default()
        }
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
    /// The connection sent the model's audio to the client, the first audible frame
    /// since the last transcript fragment it passed on (BAT R1e, C3).
    AudioOut {
        /// When that frame went out, on the call's clock: the clock the
        /// connection's tick reads (the last stamp a fragment set, plus the
        /// monotonic time since it arrived), so on the same timeline as the
        /// fragments' own stamps, to the millisecond.
        at_ms: u64,
    },
    /// The session is over; close what is open.
    Close,
    /// The connection is over: close what is open from what is held now,
    /// without waiting for the provider (d54g callseam). Measured on
    /// 2026-11-07: the turn a provider's late close produced left 330 ms and
    /// more after the line was gone and opened the next conversation.
    End {
        /// Where the call's clock stood when the connection ended.
        at_ms: u64,
        /// Whether the model's audio was still going out to the client then.
        model_audible: bool,
    },
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
    /// Where the caller's last words before the model's answer ended, on the
    /// model's clock: a backchannel inside the answer does not move it, and a
    /// barge-in's run belongs to the turn it opens. `None` for a turn the
    /// caller said nothing to answer in (D48, LC2; review M3).
    pub person_end_ms: Option<u64>,
    /// When the first audible audio frame of the model's answer to those words went
    /// out to the client, on the call's clock ([`TurnInput::AudioOut`]);
    /// after a barge-in the first new block, not the tail of the interrupted
    /// one. Never before `person_end_ms`. `None` where no answer audio went
    /// out inside the turn.
    pub first_audio_ms: Option<u64>,
    /// Whether the end of the connection cut the model off inside this turn
    /// ([`TurnInput::End`]): the model's part was not said to its end, so a
    /// reader does not take it for a statement that was made.
    pub cut: bool,
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
        TurnInput::AudioOut { at_ms } => {
            if let Some(open) = state.open.as_mut() {
                open.audio_out(at_ms);
            }
            Vec::new()
        }
        TurnInput::Close => close(state).into_iter().collect(),
        TurnInput::End {
            at_ms,
            model_audible,
        } => end(state, at_ms, model_audible).into_iter().collect(),
    }
}

/// When the first audio of the answer in the open turn went out, where it
/// has: what the `first_audio` lane stamps while the turn is still open.
pub fn open_first_audio_ms(state: &TurnState) -> Option<u64> {
    state.open.as_ref().and_then(|open| open.first_audio_ms)
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

/// The call up to a delegation's handoff (A75-A78), where the state keeps a
/// history ([`TurnState::with_history`]); `None` where it does not.
///
/// The closed turns in order, each speaker's part of a turn as one entry (the
/// caller's first), with every fragment that starts after `offset_ms` left
/// out; last, the text the `delegation` lane always carried
/// ([`open_user_text`]): the task. Nothing is cut here, whatever the length.
///
/// Each entry of a closed turn carries the `turn_id` that turn was reported
/// under on the `turn` lane, where it was ([`name_reported_turn`]); the task
/// carries none, because its turn is still open and no lane has seen it yet
/// (OR-54g-33 b).
pub fn delegation_history(
    state: &TurnState,
    offset_ms: u64,
) -> Option<Vec<(Speaker, String, Option<String>)>> {
    let history = state.history.as_ref()?;
    let joined = |frags: &[Frag]| -> String {
        frags
            .iter()
            .filter(|f| f.start_ms <= offset_ms)
            .map(|f| f.text.as_str())
            .collect()
    };
    let mut entries: Vec<(Speaker, String, Option<String>)> = Vec::new();
    for turn in &history.turns {
        for (speaker, text) in [
            (Speaker::User, joined(&turn.user)),
            (Speaker::Assistant, joined(&turn.assistant)),
        ] {
            if !text.trim().is_empty() {
                entries.push((speaker, text, turn.turn_id.clone()));
            }
        }
    }
    entries.push((
        Speaker::User,
        open_user_text(state).unwrap_or_default(),
        None,
    ));
    Some(entries)
}

/// Remember the id a closed turn was reported under on the `turn` lane, so
/// the history can name it (OR-54g-33 b): a reader that already holds that
/// turn can tell the second copy in a delegation from a new one. Does nothing
/// where the state keeps no history or no turn has that index.
pub fn name_reported_turn(state: &mut TurnState, index: u64, turn_id: String) {
    if let Some(turn) = state
        .history
        .as_mut()
        .and_then(|h| h.turns.iter_mut().rev().find(|t| t.index == index))
    {
        turn.turn_id = Some(turn_id);
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
            open.push_user(frag, in_block);
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
    // neither is a pause while the model is still talking. The model talks
    // until its block is over -- silent for longer than the gap, the line rule
    // 3 draws between two blocks -- not until the clock passes the end of its
    // last word (d54g weblive, lab run of 2026-11-13): a live model stamps the
    // first words of an answer where the caller's ended and sends them ahead
    // of their audio, the clock overtook them, and the turn closed 4 ms before
    // the answer's first audible frame, in the middle of its sentence. Four of
    // five first answers of a session lost their `first_audio` that way.
    let closes = state.open.as_ref().is_some_and(|open| {
        !open.user.is_empty()
            && opens_new_block(
                open.agent_block_start_ms.map(|_| open.agent_block_end_ms),
                now_ms,
                gap_ms,
            )
            && now_ms.saturating_sub(open.user_last_end_ms) > gap_ms
    });
    if closes {
        close(state).into_iter().collect()
    } else {
        Vec::new()
    }
}

/// The end of the connection: the open turn closes now. Its model part is
/// `cut` where the model was still speaking -- its audio was still going out,
/// or its transcript reaches past the clock.
fn end(state: &mut TurnState, at_ms: u64, model_audible: bool) -> Option<TurnAction> {
    let cut = state.open.as_ref().is_some_and(|open| {
        !open.assistant.is_empty() && (model_audible || open.model_speaks_at(at_ms))
    });
    close_as(state, cut)
}

/// Rules 6 and 7: hand over what is open and count it.
fn close(state: &mut TurnState) -> Option<TurnAction> {
    close_as(state, false)
}

/// [`close`], saying whether the end of the connection cut the model off.
fn close_as(state: &mut TurnState, cut: bool) -> Option<TurnAction> {
    let open = state.open.take()?;
    let mut finished = open.finish(state.index);
    finished.cut = cut;
    state.index += 1;
    if !finished.user.is_empty() {
        state.last_closed_user_text = Some(finished.user.clone());
    }
    if let Some(history) = state.history.as_mut() {
        history.turns.push(HistoryTurn {
            index: finished.index,
            turn_id: None,
            user: open.user,
            assistant: open.assistant,
        });
    }
    Some(TurnAction::Emit(finished))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAP: u64 = 700;
    const BACK: u64 = 900;

    fn say(state: &mut TurnState, speaker: Speaker, text: &str, start_ms: u64, end_ms: u64) {
        let _ = step(
            state,
            TurnInput::Fragment {
                speaker,
                text: text.to_string(),
                start_ms,
                end_ms,
            },
            GAP,
            BACK,
        );
    }

    fn entries(list: &[(Speaker, &str)]) -> Vec<(Speaker, String, Option<String>)> {
        list.iter()
            .map(|(s, t)| (*s, (*t).to_string(), None))
            .collect()
    }

    /// OR-54g-33 b: a closed turn's entries carry the id it was reported
    /// under, a turn not reported yet carries none, and the task never does.
    #[test]
    fn a_reported_turn_is_named_in_the_history_and_the_task_is_not() {
        let mut state = TurnState::with_history();
        say(&mut state, Speaker::User, "First.", 0, 1_000);
        say(&mut state, Speaker::Assistant, "One.", 1_100, 1_800);
        say(&mut state, Speaker::User, "Second.", 5_000, 6_000);
        say(&mut state, Speaker::User, "Third.", 9_000, 10_000);
        name_reported_turn(&mut state, 0, "s#0".to_string());
        name_reported_turn(&mut state, 7, "s#7".to_string());
        assert_eq!(
            delegation_history(&state, 10_100),
            Some(vec![
                (Speaker::User, "First.".to_string(), Some("s#0".to_string())),
                (
                    Speaker::Assistant,
                    "One.".to_string(),
                    Some("s#0".to_string())
                ),
                (Speaker::User, "Second.".to_string(), None),
                (Speaker::User, "Third.".to_string(), None),
            ])
        );
    }

    /// d54g callseam: a word whose transcript arrives late stands where it
    /// was spoken, and the turn still closes on the caller's latest word.
    #[test]
    fn a_late_fragment_stands_where_it_was_spoken() {
        let mut state = TurnState::new();
        say(&mut state, Speaker::User, "one ", 1_000, 1_100);
        say(&mut state, Speaker::User, "three.", 1_400, 1_600);
        say(&mut state, Speaker::User, "two ", 1_100, 1_200);
        assert_eq!(open_user_text(&state).as_deref(), Some("one two three."));
        // 700 ms after the latest word, not after the late one.
        assert!(step(&mut state, TurnInput::Tick { now_ms: 2_300 }, GAP, BACK).is_empty());
        let closed = step(&mut state, TurnInput::Tick { now_ms: 2_301 }, GAP, BACK);
        match closed.as_slice() {
            [TurnAction::Emit(turn)] => {
                assert_eq!(turn.user, "one two three.");
                assert_eq!(turn.happened_at_ms, 1_600);
                assert_eq!(turn.person_end_ms, Some(1_600));
            }
            other => panic!("one closed turn, got {other:?}"),
        }
    }

    /// The one turn an input closed.
    fn closed(actions: Vec<TurnAction>) -> FinishedTurn {
        match actions.as_slice() {
            [TurnAction::Emit(turn)] => turn.clone(),
            other => panic!("expected one closed turn, got {other:?}"),
        }
    }

    /// d54g callseam: the end of the connection closes the open turn at
    /// once, and a turn without a part of the model's has nothing cut.
    #[test]
    fn the_end_of_the_connection_closes_the_open_turn() {
        let mut state = TurnState::new();
        say(&mut state, Speaker::User, "Remind me at six.", 0, 900);
        let turn = closed(step(
            &mut state,
            TurnInput::End {
                at_ms: 950,
                model_audible: true,
            },
            GAP,
            BACK,
        ));
        assert_eq!(turn.user, "Remind me at six.");
        assert!(!turn.cut, "no sentence of the model's, nothing cut");
        assert!(
            step(&mut state, TurnInput::Close, GAP, BACK).is_empty(),
            "the provider's late close finds nothing open"
        );
    }

    /// d54g callseam: a model part is cut where the model's audio was still
    /// going out, or its transcript reaches past the clock; a model that had
    /// finished is not cut.
    #[test]
    fn a_model_part_is_cut_only_where_the_model_still_spoke() {
        let ended = |at_ms: u64, model_audible: bool| {
            let mut state = TurnState::new();
            say(&mut state, Speaker::User, "What is on?", 0, 900);
            say(
                &mut state,
                Speaker::Assistant,
                "At ten you have",
                1_000,
                2_400,
            );
            closed(step(
                &mut state,
                TurnInput::End {
                    at_ms,
                    model_audible,
                },
                GAP,
                BACK,
            ))
        };
        assert!(ended(2_500, true).cut, "its audio was still going out");
        assert!(
            ended(2_000, false).cut,
            "its transcript reaches past the clock"
        );
        let said = ended(2_500, false);
        assert!(!said.cut, "the model had finished");
        assert_eq!(said.assistant.as_deref(), Some("At ten you have"));
        assert!(
            !closed({
                let mut state = TurnState::new();
                say(&mut state, Speaker::User, "What is on?", 0, 900);
                say(&mut state, Speaker::Assistant, "At ten.", 1_000, 2_400);
                step(&mut state, TurnInput::Close, GAP, BACK)
            })
            .cut,
            "a provider's own close cuts nothing"
        );
    }

    /// d54g callseam: the open turn's first-audio time is readable while the
    /// turn is open, and only once audio went out.
    #[test]
    fn the_open_turn_says_when_its_first_audio_went_out() {
        let mut state = TurnState::new();
        assert_eq!(open_first_audio_ms(&state), None);
        say(&mut state, Speaker::User, "Is it raining?", 0, 900);
        assert_eq!(open_first_audio_ms(&state), None);
        let _ = step(&mut state, TurnInput::AudioOut { at_ms: 1_600 }, GAP, BACK);
        assert_eq!(open_first_audio_ms(&state), Some(1_600));
    }

    /// POC gpt-live-1 round 2 (A75-A78): without the switch there is no
    /// history at all -- the lane carries what it always carried.
    #[test]
    fn no_history_without_the_switch() {
        let mut state = TurnState::new();
        say(&mut state, Speaker::User, "Hello", 0, 500);
        assert_eq!(delegation_history(&state, 600), None);
    }

    /// The closed turns of the call in order, each speaker's part as one
    /// entry, and the open turn's caller text last.
    #[test]
    fn the_history_is_the_call_up_to_the_handoff() {
        let mut state = TurnState::with_history();
        say(
            &mut state,
            Speaker::Assistant,
            "Hello, this is the assistant.",
            0,
            1_000,
        );
        say(&mut state, Speaker::User, "Book the doctor", 2_000, 3_000);
        say(&mut state, Speaker::User, " for Friday.", 3_000, 3_600);
        say(
            &mut state,
            Speaker::Assistant,
            "Friday it is.",
            3_800,
            4_600,
        );
        say(
            &mut state,
            Speaker::User,
            "Thursday, not Friday.",
            6_000,
            7_000,
        );
        assert_eq!(
            delegation_history(&state, 7_100),
            Some(entries(&[
                (Speaker::Assistant, "Hello, this is the assistant."),
                (Speaker::User, "Book the doctor for Friday."),
                (Speaker::Assistant, "Friday it is."),
                (Speaker::User, "Thursday, not Friday."),
            ]))
        );
    }

    /// Fragments that start after the delegation's offset stay out (A78).
    #[test]
    fn fragments_after_the_offset_stay_out() {
        let mut state = TurnState::with_history();
        say(&mut state, Speaker::User, "First question.", 0, 1_000);
        say(&mut state, Speaker::Assistant, "Answer", 1_100, 1_800);
        say(&mut state, Speaker::Assistant, " and more.", 5_000, 5_600);
        say(&mut state, Speaker::User, "Second question.", 7_000, 8_000);
        assert_eq!(
            delegation_history(&state, 4_000),
            Some(entries(&[
                (Speaker::User, "First question."),
                (Speaker::Assistant, "Answer"),
                (Speaker::User, "Second question."),
            ]))
        );
    }

    /// The history is never cut in this cell: a call far over 50 KB reaches
    /// the delegation whole, oldest turn included.
    #[test]
    fn a_history_over_50_kb_is_carried_whole() {
        let mut state = TurnState::with_history();
        let mut expected: Vec<(Speaker, String, Option<String>)> = Vec::new();
        let mut at = 0_u64;
        for n in 0..40 {
            let asked = format!("question {n} {}", "q".repeat(900));
            let told = format!("answer {n} {}", "a".repeat(900));
            say(&mut state, Speaker::User, &asked, at, at + 500);
            say(&mut state, Speaker::Assistant, &told, at + 600, at + 1_000);
            expected.push((Speaker::User, asked, None));
            expected.push((Speaker::Assistant, told, None));
            at += 3_000;
        }
        say(&mut state, Speaker::User, "the task", at, at + 500);
        expected.push((Speaker::User, "the task".to_string(), None));
        let got = delegation_history(&state, at + 600);
        let bytes: usize = got.iter().flatten().map(|(_, t, _)| t.len()).sum();
        assert!(bytes > 50_000, "the fixture is over 50 KB, got {bytes}");
        assert_eq!(got, Some(expected));
    }
}
