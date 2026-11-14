//! d54g weblive, contract 6 -- the guarantees of the phone hold for a page
//! session: everything of `callseam` (no turn after the end, no takeover into
//! running speech, no control marks in a turn, the stamp of the first audio)
//! and the append ceiling of `deleg`.
//!
//! The page reaches the same connection code as a call, but by another door:
//! the in-process link in `hold`, a session that opens with the first hold
//! and carries the name `<connection>-<n>`, and an end of its own, the idle
//! end after `hold_idle_ms` (contract 4) while the page stays. Each lock here
//! is the statement of one lock of `gh_d54g_callseam` or `gh_d54g_deleg_appends`,
//! written once as a scenario over [`Access`] and driven through both doors:
//! the phone as the original drives it, the page through its own door -- and,
//! where the lock is about the end of a session, through both of its ends.
//! The original locks stay as they are; these say the same of the page.
//!
//! Topology tests: markers and orders, no time windows. Where a page waits
//! for its idle end, `hold_idle_ms` is the stimulus that ends the session,
//! never a verdict.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::keeper;
use duplex_cell::{
    Access, CAP_MS, CLOSE_GRACE_MS, End, FRAME_BYTES, FakeSession, Line, Live, PageFake, RATE,
    Seen, advise_msg, boot_fake, boot_fake_clocked, boot_fake_timed, boot_idle, boot_lingering,
    boot_recovering_with, boot_renewing, boot_renewing_timed, call_end_from_usage, duplex_params,
    emission_route, handover_msg, header, marker,
};
use futures_util::FutureExt;
use meclaw_cells::voice::contract::{
    AppendKind, DuplexControl, DuplexEvent, DuplexProvider, ProviderTimeouts, Speaker,
};
use meclaw_cells::voice::params::GptLiveParams;
use meclaw_cells::voice::providers::gpt_live::GptLiveDuplex;
use meclaw_core::serde_json::{self, Map, Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use meclaw_testing::deadline;
use meclaw_testing::mock_gpt_live::{LiveAction, LiveScript, MockGptLive};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

/// The rounds of a lock about the end of a session: the caller hangs up, the
/// page goes away, the page stays and its session ends idle.
const ENDS: [(Access, End); 3] = [
    (Access::Phone, End::Gone),
    (Access::Page, End::Gone),
    (Access::Page, End::Idle),
];

/// The idle end of a page session in the rounds with [`End::Idle`]: the
/// stimulus that ends the session after the key came up.
const PAGE_IDLE_MS: u64 = 300;

/// A renewal opens this long after a session took charge of the call.
const RENEW_AFTER_MS: u64 = 300;

/// A grace no test in this file reaches.
const NO_GRACE_MS: u64 = 20_000;

/// The line counts as quiet this long after the last word (shorter than the
/// turn gap of one second).
const QUIET_MS: u64 = 300;

/// The session clock of the renewal locks.
const TICK_MS: u64 = 100;

/// The block a colony writes in answer to the `renewed` lane.
const HANDOVER: &str = "Handover: the caller asked for the opening hours of the library; \
                        the answer was nine to six.";

/// Which round is running, ahead of every assertion of the round: a failure
/// names its door in the captured output.
fn round(lock: &str, access: Access, end: Option<End>) {
    eprintln!("round: {lock} -- door {access:?}, end {end:?}");
}

/// One round of a lock: a door, or a door and the end its session takes.
trait Round: Copy {
    fn door(self) -> String;
}

impl Round for Access {
    fn door(self) -> String {
        format!("door {self:?}")
    }
}

impl Round for (Access, End) {
    fn door(self) -> String {
        format!("door {:?}, end {:?}", self.0, self.1)
    }
}

/// The first line a red round panicked with.
fn first_line(payload: &(dyn std::any::Any + Send)) -> String {
    let text = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("(no message)");
    text.lines().next().unwrap_or_default().to_string()
}

/// Drives `scenario` through every round of `cases`. A red round does not
/// stop the rounds after it: each round gives its own result, and the lock
/// holds when every round holds. A red lock names each red round with its
/// door and the first line it failed with; the full message of a red round
/// stands above in the captured output, after its `round:` line.
async fn rounds<C, F, Fut>(lock: &str, cases: impl IntoIterator<Item = C>, scenario: F)
where
    C: Round,
    F: Fn(C) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut red = Vec::new();
    for case in cases {
        if let Err(payload) = AssertUnwindSafe(scenario(case)).catch_unwind().await {
            red.push(format!("{}: {}", case.door(), first_line(payload.as_ref())));
        }
    }
    assert!(red.is_empty(), "{lock}: red rounds -- {}", red.join("; "));
}

/// The paused clock of a stamp round: it runs again when the round lets it
/// go, and also when the round fails while it is paused, so the next round
/// starts on a running clock.
struct PausedClock;

impl PausedClock {
    fn pause() -> Self {
        tokio::time::pause();
        PausedClock
    }
}

impl Drop for PausedClock {
    fn drop(&mut self) {
        tokio::time::resume();
    }
}

/// A duplex cell that reports its usage; the door sets the mode.
fn metered() -> Value {
    duplex_params(json!({"default_mode": "auto", "emit_usage": true}))
}

/// A duplex cell; the door sets the mode.
fn plain() -> Value {
    duplex_params(json!({"default_mode": "auto"}))
}

/// One 20 ms frame whose every byte is `n`.
fn frame(n: u8) -> Vec<u8> {
    vec![n; FRAME_BYTES]
}

/// `len` bytes of PCM16 at a steady 8 000: audible speech.
fn speech(len: usize) -> Vec<u8> {
    (0..len / 2).flat_map(|_| 8_000i16.to_le_bytes()).collect()
}

/// One transcript fragment of `session`.
async fn says(session: &FakeSession, speaker: Speaker, text: &str, start_ms: u64, end_ms: u64) {
    session
        .say(DuplexEvent::Transcript {
            speaker,
            delta: text.to_string(),
            start_ms,
            end_ms,
        })
        .await;
}

/// The routes of a list of emissions, for a failure message.
fn routes(seen: &[Value]) -> Vec<String> {
    seen.iter()
        .map(|e| emission_route(e).unwrap_or_default().to_string())
        .collect()
}

/// Every `turn` among `seen`, in order.
fn turns(seen: &[Value]) -> Vec<&Value> {
    seen.iter()
        .filter(|e| emission_route(e) == Some("turn"))
        .collect()
}

/// What `origin` said in an emission, its messages joined.
fn said_by(emission: &Value, origin: &str) -> String {
    emission
        .get("messages")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .filter(|m| m.get("origin").and_then(Value::as_str) == Some(origin))
                .filter_map(|m| m.get("text").and_then(Value::as_str))
                .collect::<String>()
        })
        .unwrap_or_default()
}

/// Whether an emission is a session's final `usage`.
fn is_final_usage(emission: &Value) -> bool {
    emission_route(emission) == Some("usage") && header(emission, "final") == Some(&json!(true))
}

/// The next emission of the handler, under the hang mark.
async fn next_emission(live: &mut Live, waiting_for: &str, seen: &[Value]) -> Value {
    match tokio::time::timeout(marker(), live.emissions.recv()).await {
        Ok(Some(e)) => e.content,
        Ok(None) => panic!("the emission channel closed while waiting for {waiting_for}"),
        Err(_) => panic!(
            "HANG: no {waiting_for} within {} s; last state: saw {:?}",
            marker().as_secs(),
            routes(seen)
        ),
    }
}

/// Every emission up to and including a session's final `usage`.
async fn until_final_usage(live: &mut Live) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    loop {
        let e = next_emission(live, "the final `usage`", &seen).await;
        let last = is_final_usage(&e);
        seen.push(e);
        if last {
            return seen;
        }
    }
}

/// Every emission up to and including the next `turn`.
async fn until_turn(live: &mut Live) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    loop {
        let e = next_emission(live, "a `turn`", &seen).await;
        let last = emission_route(&e) == Some("turn");
        seen.push(e);
        if last {
            return seen;
        }
    }
}

/// Every emission up to and including the next `partial` of the session
/// `session_id`: the marker for "everything an earlier session still had to
/// say has been said" (the handler takes events in order).
async fn until_partial_of(live: &mut Live, session_id: &str) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    loop {
        let e = next_emission(live, "a `partial` of the next session", &seen).await;
        let last = emission_route(&e) == Some("partial")
            && header(&e, "session_id") == Some(&json!(session_id));
        seen.push(e);
        if last {
            return seen;
        }
    }
}

/// The index of the next session whose way in was shut.
async fn next_ended(live: &mut Live) -> usize {
    match tokio::time::timeout(marker(), live.ended.recv()).await {
        Ok(Some(index)) => index,
        Ok(None) => panic!("the fake is gone"),
        Err(_) => panic!(
            "HANG: no session's way in was shut within {} s",
            marker().as_secs()
        ),
    }
}

/// End the session of `line` the way `end` says: the person goes (the line
/// is gone after it), or the key comes up and the page idles until its idle
/// end (the line stays for the next hold).
async fn end_session(line: Line, end: End) -> Option<Line> {
    match end {
        End::Gone => {
            line.hang_up().await;
            None
        }
        End::Idle => {
            let mut line = line;
            line.release().await;
            Some(line)
        }
    }
}

/// A cell for the end of a session: for the idle end a page cell with
/// `hold_idle_ms`, around the same fake as for the other ends.
async fn boot_for_end(raw: Value, fake: PageFake, close_grace_ms: u64, end: End) -> Live {
    match (end, fake) {
        (End::Idle, fake) => boot_idle(raw, fake, close_grace_ms, PAGE_IDLE_MS).await,
        (End::Gone, PageFake::Holding) => boot_renewing(raw, 0, NO_GRACE_MS).await,
        (End::Gone, PageFake::Lingering) => boot_lingering(raw, close_grace_ms).await,
        (End::Gone, PageFake::Plain) => boot_fake(raw).await,
    }
}

// ------------------------------------------------------------ the end of a session

/// `gh_d54g_callseam_no_turn_leaves_after_the_final_usage`, through every
/// end: what the provider still delivers after the session ended is not a
/// turn, and after the session's final `usage` no turn of that session leaves
/// at all. The marker behind the first session is the next one: a new call,
/// a new page, or the next hold of the same page.
async fn no_turn_after_the_final_usage(access: Access, end: End) {
    const REST: &str = "And one more thing.";
    const TAG: &str = "wseam-1";
    round("no turn after the final usage", access, Some(end));
    let mut live = boot_for_end(metered(), PageFake::Holding, CLOSE_GRACE_MS, end).await;
    let (line, mut session) = live.open_line(access, TAG).await;
    let first_name = access.session_name(TAG, 1);

    says(&session, Speaker::User, "Until tomorrow then.", 0, 900).await;
    let _ = live.emission("partial").await;
    let line = end_session(line, end).await;
    assert_eq!(next_ended(&mut live).await, 1, "the session ended");

    // The session is over; the provider is not done yet.
    says(&session, Speaker::User, REST, 1_000, 1_800).await;
    session.finish();
    drop(session);

    let first = until_final_usage(&mut live).await;
    let last = first.last().expect("the final usage");
    assert_eq!(
        header(last, "call_id"),
        Some(&json!(first_name)),
        "the final `usage` names the session that ended: {last}"
    );
    for turn in turns(&first) {
        assert!(
            !said_by(turn, "user").contains(REST) && !said_by(turn, "assistant").contains(REST),
            "what the provider delivered after the session ended left as a turn: {turn}"
        );
    }

    // The marker: the next session.
    let (next, next_name, _keep) = match line {
        Some(mut line) => {
            let next = live.key_down(&mut line).await;
            (next, access.session_name(TAG, 2), Some(line))
        }
        None => {
            let (line, next) = live.open_line(access, "wseam-1b").await;
            (next, access.session_name("wseam-1b", 1), Some(line))
        }
    };
    says(&next, Speaker::User, "Hello again.", 0, 600).await;
    let after = until_partial_of(&mut live, &next_name).await;
    let late: Vec<&Value> = after
        .iter()
        .filter(|e| {
            emission_route(e) == Some("turn") && header(e, "session_id") == Some(&json!(first_name))
        })
        .collect();
    assert!(
        late.is_empty(),
        "a turn of a session left after that session's final `usage`: {late:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_seam_no_turn_leaves_after_the_final_usage() {
    rounds(
        "gh_d54g_weblive_seam_no_turn_leaves_after_the_final_usage",
        ENDS,
        |(access, end)| no_turn_after_the_final_usage(access, end),
    )
    .await;
}

/// `gh_d54g_callseam_the_open_turn_closes_with_the_connection_before_the_final_usage`,
/// through every end: the open turn leaves with the end of the session,
/// before its final `usage` and without waiting for the provider, which here
/// never finalises.
async fn the_open_turn_closes_with_the_end(access: Access, end: End) {
    const LAST: &str = "Please remind me at six.";
    const GRACE_MS: u64 = 400;
    const TAG: &str = "wseam-2";
    round("the open turn closes with the end", access, Some(end));
    let mut live = boot_for_end(metered(), PageFake::Lingering, GRACE_MS, end).await;
    let (line, session) = live.open_line(access, TAG).await;

    says(&session, Speaker::User, LAST, 0, 900).await;
    let _ = live.emission("partial").await;
    let _line = end_session(line, end).await;

    let turn = deadline::hang("the person's last turn leaves", async {
        loop {
            tokio::select! {
                biased;

                e = live.emissions.recv() => {
                    let e = e.expect("the handler is running").content;
                    if emission_route(&e) == Some("turn") {
                        return e;
                    }
                    assert!(
                        !is_final_usage(&e),
                        "the session's final `usage` left and the person's last sentence \
                         never did -- it is lost"
                    );
                }
                waited = live.let_go.recv() => {
                    panic!(
                        "the connection let go of the model after {waited:?} and the \
                         person's last turn had not left: the turn waits for the \
                         provider's close"
                    );
                }
            }
        }
    })
    .await;
    assert_eq!(said_by(&turn, "user"), LAST, "{turn}");
    assert_eq!(
        header(&turn, "turn_id"),
        Some(&json!(format!("{}#0", access.session_name(TAG, 1)))),
        "{turn}"
    );
    assert_ne!(
        header(&turn, "cut"),
        Some(&json!(true)),
        "a turn without a sentence of the model's has nothing that was cut: {turn}"
    );

    let rest = until_final_usage(&mut live).await;
    assert!(
        turns(&rest).is_empty(),
        "one turn for one sentence, and it left before the final `usage`: {:?}",
        routes(&rest)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_seam_the_open_turn_closes_with_the_end_before_the_final_usage() {
    rounds(
        "gh_d54g_weblive_seam_the_open_turn_closes_with_the_end_before_the_final_usage",
        ENDS,
        |(access, end)| the_open_turn_closes_with_the_end(access, end),
    )
    .await;
}

/// How many chunks of the model's answer the person hears before going.
const SPOKEN_CHUNKS: usize = 3;

/// `gh_d54g_callseam_a_sentence_cut_by_the_end_is_marked_cut`: the person
/// goes while the model is in the middle of its answer, and the turn says the
/// model's sentence was not said to its end. The page asked with the key down
/// and released it, as a page does. The idle end has no such round: it comes
/// only once the model has been quiet for `hold_idle_ms`, so it cuts nothing.
async fn a_sentence_cut_by_the_end(access: Access) {
    const ASKED: &str = "What is on tomorrow?";
    const ANSWER: &str = "Tomorrow at ten you have the";
    round("a sentence cut by the end", access, Some(End::Gone));
    // Quiet, cap and turn gap at the hang mark, as in the original: only the
    // end may close this turn.
    let quiet_ms = u64::try_from(marker().as_millis()).unwrap_or(u64::MAX);
    let params = duplex_params(json!({
        "default_mode": "auto",
        "emit_usage": true,
        "duplex": {
            "provider": "gpt_live",
            "api_key": "test-key",
            "instructions": "A test line.",
            "sample_rate": RATE,
            "turn_gap_ms": quiet_ms,
        },
    }));
    let mut live = boot_fake_timed(params, quiet_ms, quiet_ms.saturating_mul(2)).await;
    let (mut line, session) = live.open_line(access, "wseam-3").await;

    says(&session, Speaker::User, ASKED, 0, 900).await;
    let _ = live.emission("partial").await;
    if access == Access::Page {
        line.release().await;
    }
    says(&session, Speaker::Assistant, ANSWER, 1_000, 2_400).await;
    let _ = live.emission("spoken").await;

    // Chunk after chunk, each sent once the person heard the one before.
    for _ in 0..SPOKEN_CHUNKS {
        session
            .audio_out
            .send(speech(FRAME_BYTES))
            .await
            .expect("the model's audio goes out");
        let _ = line.hear().await;
    }
    line.hang_up().await;

    let seen = until_final_usage(&mut live).await;
    let closed = turns(&seen);
    assert_eq!(closed.len(), 1, "one turn: {:?}", routes(&seen));
    let turn = closed[0];
    assert_eq!(said_by(turn, "user"), ASKED, "{turn}");
    assert_eq!(said_by(turn, "assistant"), ANSWER, "{turn}");
    assert_eq!(
        header(turn, "cut"),
        Some(&json!(true)),
        "the end cut the model's sentence, and the turn says so: {turn}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_seam_a_sentence_cut_by_the_end_is_marked_cut() {
    rounds(
        "gh_d54g_weblive_seam_a_sentence_cut_by_the_end_is_marked_cut",
        Access::BOTH,
        a_sentence_cut_by_the_end,
    )
    .await;
}

// ------------------------------------------------- the second safeguard (keeper)

/// What the cell said of one ended session: the handle its turn carries, the
/// turn's id and words, and its final `usage`.
struct Ended {
    handle: String,
    turn_id: String,
    said: String,
    usage: Value,
}

/// Two sessions of one person through `access`, each ended the way that door
/// ends it -- the phone hangs up and calls again, the page releases the key,
/// idles out and holds again -- with what the cell emitted for each. The
/// keeper locks below read the handles off these emissions, so they make
/// their claim about the names the door really gives, not about hand-set
/// ones: the turn and the end of a session carry the same handle, and the
/// next session carries another.
async fn two_ended_sessions(access: Access, tag: &str) -> [Ended; 2] {
    let (mut live, end) = match access {
        Access::Phone => (boot_fake(metered()).await, End::Gone),
        Access::Page => (
            boot_idle(metered(), PageFake::Plain, CLOSE_GRACE_MS, PAGE_IDLE_MS).await,
            End::Idle,
        ),
    };
    let mut out: Vec<Ended> = Vec::new();
    // The page keeps its connection and holds again; the phone calls again.
    let mut kept: Option<(Line, String)> = None;
    for (n, said) in [(1u32, "hello, it is me"), (2, "me again")] {
        let (line, session, connection, name) = match kept.take() {
            Some((mut line, connection)) => {
                let session = live.key_down(&mut line).await;
                let name = access.session_name(&connection, n);
                (line, session, connection, name)
            }
            None => {
                let connection = format!("{tag}-c{n}");
                let (line, session) = live.open_line(access, &connection).await;
                let name = access.session_name(&connection, 1);
                (line, session, connection, name)
            }
        };
        says(&session, Speaker::User, said, 0, 900).await;
        let _ = live.emission("partial").await;
        kept = end_session(line, end).await.map(|line| (line, connection));
        let seen = until_final_usage(&mut live).await;
        let usage = seen.last().cloned().expect("the final usage");
        let closed = turns(&seen);
        assert_eq!(closed.len(), 1, "one turn per session: {:?}", routes(&seen));
        let turn = closed[0];
        assert_eq!(said_by(turn, "user"), said, "{turn}");
        assert_eq!(
            header(turn, "call_id"),
            Some(&json!(name)),
            "the turn carries the session's handle: {turn}"
        );
        assert_eq!(
            header(&usage, "call_id"),
            Some(&json!(name)),
            "the end carries the same handle as the session's turn: {usage}"
        );
        out.push(Ended {
            handle: name,
            turn_id: header(turn, "turn_id")
                .and_then(Value::as_str)
                .expect("a turn names itself")
                .to_string(),
            said: said.to_string(),
            usage,
        });
        drop(session);
    }
    assert_ne!(
        out[0].handle, out[1].handle,
        "every session has a handle of its own"
    );
    let second = out.pop().expect("two sessions");
    let first = out.pop().expect("two sessions");
    [first, second]
}

/// `gh_d54g_callseam_a_turn_of_an_ended_session_opens_no_session`, on the
/// handles a call and a page get from the cell.
async fn a_turn_of_an_ended_session(access: Access) {
    round("a turn of an ended session", access, None);
    if !keeper::shipped() {
        return;
    }
    let [first, second] = two_ended_sessions(access, "wseam-k1").await;
    const LINE: &str = "line-1";
    let node = keeper::LIVE_NODE;
    let mut k = keeper::boot_keeper().await;

    let (during, said) = k
        .say(keeper::turn(
            LINE,
            node,
            &first.said,
            &first.turn_id,
            Some(&first.handle),
        ))
        .await;
    assert_eq!(said, first.said);
    k.h.send(keeper::answered(&during, &first.turn_id)).await;
    k.h.quiesce().await;

    k.h.send(keeper::live_end(node, &first.handle)).await;
    let (closed, reason) = k.next_close().await;
    assert_eq!(
        (closed.as_str(), reason.as_str()),
        (during.as_str(), "call_ended"),
        "the end of the session closes its generation"
    );

    // The tail: a turn of the session that has just ended.
    k.h.send(keeper::turn(
        LINE,
        node,
        "and one more thing",
        "t-tail",
        Some(&first.handle),
    ))
    .await;
    k.h.quiesce().await;
    assert_eq!(
        k.generations(),
        vec![(during.clone(), node.to_string(), 1)],
        "a turn of an ended session opened a conversation session"
    );
    while k.sink.try_recv().is_ok() {}

    // The next session of the same person is a new conversation.
    let (next, said) = k
        .say(keeper::turn(
            LINE,
            node,
            &second.said,
            &second.turn_id,
            Some(&second.handle),
        ))
        .await;
    assert_eq!(said, second.said);
    assert_ne!(next, during, "the next session runs in its own generation");
    k.h.quiesce().await;
    assert_eq!(
        k.generations(),
        vec![(during, node.to_string(), 1), (next, node.to_string(), 0)],
        "one generation per session"
    );
    k.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_seam_a_turn_of_an_ended_session_opens_no_session() {
    rounds(
        "gh_d54g_weblive_seam_a_turn_of_an_ended_session_opens_no_session",
        Access::BOTH,
        a_turn_of_an_ended_session,
    )
    .await;
}

/// `gh_d54g_callseam_a_session_that_ended_without_a_generation_opens_none_after_it`,
/// on the handles a call and a page get from the cell.
async fn a_session_that_ended_without_a_generation(access: Access) {
    round("a session that ended without a generation", access, None);
    if !keeper::shipped() {
        return;
    }
    let [first, second] = two_ended_sessions(access, "wseam-k2").await;
    const LINE: &str = "line-1";
    let node = keeper::LIVE_NODE;
    let mut k = keeper::boot_keeper().await;

    k.h.send(keeper::live_end(node, &first.handle)).await;
    k.h.quiesce().await;
    assert_eq!(
        k.generations(),
        Vec::<(String, String, i64)>::new(),
        "an end with nothing open seals nothing and opens nothing"
    );

    k.h.send(keeper::turn(
        LINE,
        node,
        &first.said,
        &first.turn_id,
        Some(&first.handle),
    ))
    .await;
    k.h.quiesce().await;
    assert_eq!(
        k.generations(),
        Vec::<(String, String, i64)>::new(),
        "a turn of a session that has ended opened a conversation session"
    );
    while k.sink.try_recv().is_ok() {}

    let (next, said) = k
        .say(keeper::turn(
            LINE,
            node,
            &second.said,
            &second.turn_id,
            Some(&second.handle),
        ))
        .await;
    assert_eq!(said, second.said);
    k.h.quiesce().await;
    assert_eq!(
        k.generations(),
        vec![(next, node.to_string(), 0)],
        "the next session opens its own generation"
    );
    k.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_seam_a_session_that_ended_without_a_generation_opens_none_after_it() {
    rounds(
        "gh_d54g_weblive_seam_a_session_that_ended_without_a_generation_opens_none_after_it",
        Access::BOTH,
        a_session_that_ended_without_a_generation,
    )
    .await;
}

/// `gh_d54g_callseam_a_turn_after_a_call_end_drawn_by_talky_is_refused`, with
/// the end built from the cell's own final `usage` of the session (its hop as
/// sent) and the edge `talky` ships.
async fn a_turn_after_an_end_drawn_by_talky(access: Access) {
    round("a turn after an end drawn by talky", access, None);
    if !keeper::shipped() || !keeper::talky_shipped() {
        return;
    }
    let [first, _second] = two_ended_sessions(access, "wseam-k3").await;
    const LINE: &str = "line-1";
    let node = keeper::LIVE_NODE;
    let mut k = keeper::boot_keeper_wired_like_talky().await;

    let (during, said) = k
        .say(keeper::turn(
            LINE,
            node,
            &first.said,
            &first.turn_id,
            Some(&first.handle),
        ))
        .await;
    assert_eq!(said, first.said);
    k.h.send(keeper::answered(&during, &first.turn_id)).await;
    k.h.quiesce().await;

    let end = call_end_from_usage(node, &first.usage);
    assert!(
        end.headers.context.get("live").is_none(),
        "the end carries no handle of its own"
    );
    k.h.send(end).await;
    let (closed, reason) = k.next_close().await;
    assert_eq!(
        (closed.as_str(), reason.as_str()),
        (during.as_str(), "call_ended"),
        "the end of the session, as the cell sent it, closes its generation"
    );
    k.h.quiesce().await;

    k.h.send(keeper::turn(
        LINE,
        node,
        "and one more thing",
        "t-tail",
        Some(&first.handle),
    ))
    .await;
    let refused = k.next_reject().await;
    assert_eq!(
        refused.headers.hop.get("error_code"),
        Some(&json!("live_ended")),
        "the tail of an ended session is refused as such: {:?}",
        refused.headers.hop
    );
    k.h.quiesce().await;
    assert_eq!(
        k.generations(),
        vec![(during, node.to_string(), 1)],
        "a turn of an ended session opened a conversation session"
    );
    k.h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_seam_a_turn_after_an_end_drawn_by_talky_is_refused() {
    rounds(
        "gh_d54g_weblive_seam_a_turn_after_an_end_drawn_by_talky_is_refused",
        Access::BOTH,
        a_turn_after_an_end_drawn_by_talky,
    )
    .await;
}

// ------------------------------------------------------------ a renewal

/// The next control frame any session was told, with the session it reached.
async fn tagged_control(live: &mut Live) -> (usize, DuplexControl) {
    tokio::time::timeout(marker(), live.tagged_controls.recv())
        .await
        .expect("HANG: no control frame reached a session")
        .expect("the fake is running")
}

/// The next `n` audio frames any session was fed, with the session each
/// reached.
async fn tagged_audio(live: &mut Live, n: usize) -> Vec<(usize, Vec<u8>)> {
    let mut out: Vec<(usize, Vec<u8>)> = Vec::with_capacity(n);
    while out.len() < n {
        let got = tokio::time::timeout(marker(), live.tagged_audio.recv())
            .await
            .unwrap_or_else(|_| panic!("HANG: only {} of {n} frames reached a session", out.len()))
            .expect("the fake is running");
        out.push(got);
    }
    out
}

/// The next journal entry of session `index`, skipping every other session's.
async fn next_seen_at(live: &mut Live, index: usize) -> Seen {
    let deadline = Instant::now() + marker();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let (at, seen) = tokio::time::timeout(left, live.journal.recv())
            .await
            .unwrap_or_else(|_| panic!("HANG: session {index} was told nothing"))
            .expect("the fake is running");
        if at == index {
            return seen;
        }
    }
}

/// What came first on a line where a renewal is due while a turn is open.
#[derive(Debug)]
enum First {
    /// The person's turn closed.
    Turn(Value),
    /// The session with this index lost its way in: the renewal took over.
    Takeover(usize),
}

/// The person's turn closing, or a takeover -- whichever is first.
async fn turn_or_takeover(live: &mut Live) -> First {
    deadline::hang(
        "the person's turn closes or the renewal takes over",
        async {
            loop {
                tokio::select! {
                    biased;

                    e = live.emissions.recv() => {
                        let e = e.expect("the handler is running").content;
                        if emission_route(&e) == Some("turn") {
                            return First::Turn(e);
                        }
                    }
                    index = live.ended.recv() => {
                        return First::Takeover(index.expect("the fake is running"));
                    }
                }
            }
        },
    )
    .await
}

/// `gh_d54g_callseam_a_due_renewal_waits_while_the_caller_speaks`. On the
/// page the key is down: the person is speaking.
async fn a_due_renewal_waits(access: Access) {
    const SENTENCE: &str = "Could you please";
    const TAG: &str = "wseam-5";
    round("a due renewal waits while the person speaks", access, None);
    let mut live = boot_renewing_timed(
        plain(),
        RENEW_AFTER_MS,
        NO_GRACE_MS,
        QUIET_MS,
        8_000,
        TICK_MS,
    )
    .await;
    let (mut line, first) = live.open_line(access, TAG).await;
    let _renewed = live.emission("renewed").await;
    let _second = live.session().await;

    says(&first, Speaker::User, SENTENCE, 5_000, 5_400).await;
    let _ = live.emission("partial").await;
    live.send(handover_msg(&access.session_name(TAG, 1), HANDOVER, 1))
        .await;
    match tagged_control(&mut live).await {
        (2, DuplexControl::Append { .. }) => {}
        other => panic!("the handover reaches session 2 first; got {other:?}"),
    }

    match turn_or_takeover(&mut live).await {
        First::Turn(turn) => assert_eq!(said_by(&turn, "user"), SENTENCE, "{turn}"),
        First::Takeover(index) => panic!(
            "the renewal took the session from model session {index} while the person's \
             turn was open: a sentence is never split between two sessions"
        ),
    }
    assert_eq!(
        next_ended(&mut live).await,
        1,
        "the old session's ear is shut"
    );
    line.send_audio(&frame(1)).await;
    assert_eq!(
        tagged_audio(&mut live, 1).await,
        vec![(2, frame(1))],
        "after the turn the session runs on the renewed model session"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_seam_a_due_renewal_waits_while_the_person_speaks() {
    rounds(
        "gh_d54g_weblive_seam_a_due_renewal_waits_while_the_person_speaks",
        Access::BOTH,
        a_due_renewal_waits,
    )
    .await;
}

/// `gh_d54g_callseam_a_renewal_past_its_grace_takes_over_at_the_turn_close`.
async fn a_renewal_past_its_grace(access: Access) {
    const SENTENCE: &str = "I wanted to ask you";
    const GRACE_MS: u64 = 200;
    round("a renewal past its grace", access, None);
    let mut live =
        boot_renewing_timed(plain(), RENEW_AFTER_MS, GRACE_MS, QUIET_MS, 8_000, TICK_MS).await;
    let (mut line, first) = live.open_line(access, "wseam-6").await;
    let _renewed = live.emission("renewed").await;
    let _second = live.session().await;

    says(&first, Speaker::User, SENTENCE, 5_000, 5_400).await;
    let _ = live.emission("partial").await;

    match turn_or_takeover(&mut live).await {
        First::Turn(turn) => assert_eq!(said_by(&turn, "user"), SENTENCE, "{turn}"),
        First::Takeover(index) => panic!(
            "the grace ran out and the renewal took the session from model session \
             {index} before the person's turn closed"
        ),
    }
    assert_eq!(
        next_ended(&mut live).await,
        1,
        "at the close of the turn the renewal takes over"
    );
    line.send_audio(&frame(1)).await;
    assert_eq!(
        tagged_audio(&mut live, 1).await,
        vec![(2, frame(1))],
        "after the turn the session runs on the renewed model session"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_seam_a_renewal_past_its_grace_takes_over_at_the_turn_close() {
    rounds(
        "gh_d54g_weblive_seam_a_renewal_past_its_grace_takes_over_at_the_turn_close",
        Access::BOTH,
        a_renewal_past_its_grace,
    )
    .await;
}

/// Where the person's words are placed on the session's timeline.
const TALK_BASE_MS: u64 = 5_000;
/// One word of a talking person, and the pause before the next one.
const WORD_MS: u64 = 100;
/// The hard cap: a person still talking this long after the renewal became
/// due is taken over inside the turn.
const HARD_CAP_MS: u64 = 600;

/// What a person who talked until the takeover left behind.
struct Talk {
    words: Vec<String>,
    last_end_ms: u64,
    seen: Vec<Value>,
}

/// The person talks without a pause until the old model session's way in is
/// shut (the pace is the stimulus, never the verdict).
async fn person_talks_until_the_takeover(live: &mut Live, old: &FakeSession) -> Talk {
    let began = Instant::now();
    let mut words: Vec<String> = Vec::new();
    let mut seen: Vec<Value> = Vec::new();
    let mut last_end_ms = TALK_BASE_MS;
    loop {
        let now_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
        let start_ms = TALK_BASE_MS.saturating_add(now_ms).max(last_end_ms);
        let end_ms = start_ms + WORD_MS;
        let word = format!("w{} ", words.len());
        says(old, Speaker::User, &word, start_ms, end_ms).await;
        words.push(word);
        last_end_ms = end_ms;
        let ended = tokio::time::timeout(Duration::from_millis(WORD_MS), live.ended.recv()).await;
        while let Ok(e) = live.emissions.try_recv() {
            seen.push(e.content);
        }
        if let Ok(ended) = ended {
            let index = ended.expect("the cell reports the sessions it ends");
            assert_eq!(
                index, 1,
                "the model session in charge is the one taken over"
            );
            return Talk {
                words,
                last_end_ms,
                seen,
            };
        }
    }
}

/// [`person_talks_until_the_takeover`] under the hang mark.
async fn talks_until_the_takeover(live: &mut Live, old: &FakeSession) -> Talk {
    tokio::time::timeout(marker(), person_talks_until_the_takeover(live, old))
        .await
        .unwrap_or_else(|_| {
            panic!(
                "HANG: the renewal never took over from a person who talked for {} s",
                marker().as_secs()
            )
        })
}

/// A line whose renewal becomes due by its grace and is capped at
/// [`HARD_CAP_MS`], with both model sessions open.
async fn a_line_with_a_waiting_renewal(
    access: Access,
    tag: &str,
) -> (Live, Line, FakeSession, FakeSession) {
    let mut live =
        boot_renewing_timed(plain(), RENEW_AFTER_MS, 200, QUIET_MS, HARD_CAP_MS, TICK_MS).await;
    let (line, first) = live.open_line(access, tag).await;
    let _renewed = live.emission("renewed").await;
    let second = live.session().await;
    (live, line, first, second)
}

/// `gh_d54g_callseam_the_turn_across_a_takeover_keeps_every_word`.
async fn the_turn_across_a_takeover(access: Access) {
    const NEW: &str = "and the rest.";
    const LATE: &str = "late ";
    round("the turn across a takeover", access, None);
    let (mut live, _line, first, second) = a_line_with_a_waiting_renewal(access, "wseam-7").await;
    let talk = talks_until_the_takeover(&mut live, &first).await;
    assert!(
        turns(&talk.seen).is_empty(),
        "no turn closes while the person talks: {:?}",
        routes(&talk.seen)
    );

    says(&second, Speaker::User, NEW, 300, 500).await;
    let mut seen: Vec<Value> = Vec::new();
    loop {
        let e = next_emission(
            &mut live,
            "the new model session's words as a `partial`",
            &seen,
        )
        .await;
        let through = emission_route(&e) == Some("partial") && said_by(&e, "user").contains(NEW);
        seen.push(e);
        if through {
            break;
        }
    }
    says(
        &first,
        Speaker::User,
        LATE,
        talk.last_end_ms,
        talk.last_end_ms + 50,
    )
    .await;

    seen.extend(until_turn(&mut live).await);
    let closed = turns(&seen);
    assert_eq!(
        closed.len(),
        1,
        "the takeover does not cut the turn: {:?}",
        routes(&seen)
    );
    let expected = format!("{}{LATE}{NEW}", talk.words.concat());
    assert_eq!(
        said_by(closed[0], "user"),
        expected,
        "the turn carries the words of both model sessions in the order they were spoken"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_seam_the_turn_across_a_takeover_keeps_every_word() {
    rounds(
        "gh_d54g_weblive_seam_the_turn_across_a_takeover_keeps_every_word",
        Access::BOTH,
        the_turn_across_a_takeover,
    )
    .await;
}

/// The words a control frame tells a model session.
fn told_text(control: &DuplexControl) -> String {
    match control {
        DuplexControl::Append { content, .. } => content.clone(),
        DuplexControl::Seed { messages } => messages
            .iter()
            .map(|m| m.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// `gh_d54g_callseam_the_hard_cap_hands_the_partial_over`.
async fn the_hard_cap_hands_the_partial_over(access: Access) {
    round("the hard cap hands the partial over", access, None);
    let (mut live, mut line, first, _second) =
        a_line_with_a_waiting_renewal(access, "wseam-8").await;

    let talk = talks_until_the_takeover(&mut live, &first).await;
    assert!(
        talk.words.len() >= 3,
        "the person said a few words before the cap: {:?}",
        talk.words
    );
    line.send_audio(&frame(1)).await;

    let mut told: Vec<DuplexControl> = Vec::new();
    loop {
        match next_seen_at(&mut live, 2).await {
            Seen::Control(control) => told.push(control),
            Seen::Audio(bytes) => {
                assert_eq!(bytes, frame(1), "the person's audio after the takeover");
                break;
            }
        }
    }
    let heard_so_far = talk.words[..3].concat();
    let heard_so_far = heard_so_far.trim();
    assert!(
        told.iter().any(|c| told_text(c).contains(heard_so_far)),
        "the model session that took over inside a sentence was not told the part \
         already heard (`{heard_so_far}` ...): it was told {told:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_seam_the_hard_cap_hands_the_partial_over() {
    rounds(
        "gh_d54g_weblive_seam_the_hard_cap_hands_the_partial_over",
        Access::BOTH,
        the_hard_cap_hands_the_partial_over,
    )
    .await;
}

// ------------------------------------------------------------ the stamp

/// A `gpt_live` block the parse accepts, with a literal key that is not one.
fn live_block(extra: Value) -> Value {
    let mut block = json!({
        "provider": "gpt_live",
        "api_key": "test-key",
        "instructions": "A test line."
    });
    if let (Some(obj), Some(more)) = (block.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            obj.insert(k.clone(), v.clone());
        }
    }
    block
}

/// The person's question and the model's answer, on a paused clock: the
/// question ends at 900 on the session's timeline, the page's key comes up
/// behind it, and the first audible audio of the answer goes out 700 ms
/// later. Every emission up to and including the closed `turn`.
async fn one_answered_question(
    live: &mut Live,
    access: Access,
    line: &mut Line,
    session: &FakeSession,
) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    for (delta, start_ms, end_ms) in [("Is it ", 0, 500), ("raining?", 500, 900)] {
        says(session, Speaker::User, delta, start_ms, end_ms).await;
        loop {
            let e = next_emission(live, "the person's `partial`", &seen).await;
            let through = emission_route(&e) == Some("partial");
            seen.push(e);
            if through {
                break;
            }
        }
    }
    if access == Access::Page {
        line.release().await;
    }
    tokio::time::advance(Duration::from_millis(700)).await;
    session
        .audio_out
        .send(speech(9_600))
        .await
        .expect("the connection takes model audio");
    let _ = line.hear().await;
    says(session, Speaker::Assistant, "Not right now.", 1_200, 1_800).await;
    seen.extend(until_turn(live).await);
    seen
}

/// `gh_d54g_callseam_the_first_audio_of_an_answer_is_stamped` (the door of
/// the switch is the original's; this is the runtime half on both doors).
async fn the_first_audio_is_stamped(access: Access) {
    round("the first audio of an answer is stamped", access, None);
    let mut params = json!({"default_mode": "auto", "emit_first_audio": true});
    if let Some(obj) = params.as_object_mut() {
        obj.insert("duplex".into(), live_block(json!({})));
    }
    let mut live = boot_fake(duplex_params(params)).await;
    let clock = PausedClock::pause();
    let (mut line, session) = live.open_linked(access, "wseam-10").await;
    let seen = one_answered_question(&mut live, access, &mut line, &session).await;
    let turn = seen.last().expect("the turn closed");
    assert_eq!(
        header(turn, "t_first_audio_ms"),
        Some(&json!(1_600)),
        "{turn}"
    );
    let stamps: Vec<&Value> = seen
        .iter()
        .filter(|e| emission_route(e) == Some("first_audio"))
        .collect();
    assert_eq!(
        stamps.len(),
        1,
        "one stamp per answer on the `first_audio` lane: saw {:?}",
        routes(&seen)
    );
    let stamp = stamps[0];
    assert_eq!(
        header(stamp, "t_first_audio_ms"),
        header(turn, "t_first_audio_ms"),
        "the stamp carries the time the turn carries: {stamp}"
    );
    assert_eq!(
        header(stamp, "turn_id"),
        header(turn, "turn_id"),
        "the stamp names the turn it belongs to: {stamp}"
    );
    assert_eq!(
        header(stamp, "call_id"),
        Some(&json!(access.session_name("wseam-10", 1))),
        "the stamp names the session: {stamp}"
    );
    assert_eq!(stamp["messages"], json!([]), "no words on this lane");
    let stamp_at = seen
        .iter()
        .position(|e| emission_route(e) == Some("first_audio"))
        .expect("counted above");
    let spoken_at = seen
        .iter()
        .position(|e| emission_route(e) == Some("spoken"))
        .unwrap_or(seen.len() - 1);
    assert!(
        stamp_at < spoken_at,
        "the stamp leaves when the audio does: {:?}",
        routes(&seen)
    );
    drop(line);
    drop(live);
    drop(clock);

    // Off by default: the same session emits nothing on the lane.
    let mut live = boot_fake(duplex_params(json!({
        "default_mode": "auto",
        "duplex": live_block(json!({})),
    })))
    .await;
    let clock = PausedClock::pause();
    let (mut line, session) = live.open_linked(access, "wseam-10b").await;
    let seen = one_answered_question(&mut live, access, &mut line, &session).await;
    let turn = seen.last().expect("the turn closed");
    assert_eq!(
        header(turn, "t_first_audio_ms"),
        Some(&json!(1_600)),
        "the turn carries the time either way: {turn}"
    );
    assert!(
        !seen
            .iter()
            .any(|e| emission_route(e) == Some("first_audio")),
        "without the switch nothing leaves on `first_audio`: {:?}",
        routes(&seen)
    );
    drop(clock);
}

#[tokio::test(flavor = "current_thread")]
async fn gh_d54g_weblive_seam_the_first_audio_of_an_answer_is_stamped() {
    rounds(
        "gh_d54g_weblive_seam_the_first_audio_of_an_answer_is_stamped",
        Access::BOTH,
        the_first_audio_is_stamped,
    )
    .await;
}

/// `gh_d54g_callseam_a_first_answer_ahead_of_its_audio_is_stamped_once`: the
/// words of the answer run ahead of its audio, and the clock passes them
/// before the audio comes; the answer is one turn, stamped once.
async fn a_first_answer_ahead_of_its_audio(access: Access) {
    round("a first answer ahead of its audio", access, None);
    let mut params = json!({"default_mode": "auto", "emit_first_audio": true});
    if let Some(obj) = params.as_object_mut() {
        obj.insert("duplex".into(), live_block(json!({})));
    }
    let mut live = boot_fake_clocked(duplex_params(params), TICK_MS, 256).await;
    let clock = PausedClock::pause();
    let (mut line, session) = live.open_linked(access, "wseam-11").await;
    let mut seen: Vec<Value> = Vec::new();

    let through = |e: &Value, lane: &str| emission_route(e) == Some(lane);
    for (delta, start_ms, end_ms) in [("So the driver ", 0, 600), ("of the vehicle", 600, 1_000)] {
        says(&session, Speaker::User, delta, start_ms, end_ms).await;
        loop {
            let e = next_emission(&mut live, "the person's `partial`", &seen).await;
            let done = through(&e, "partial");
            seen.push(e);
            if done {
                break;
            }
        }
    }
    if access == Access::Page {
        line.release().await;
    }
    says(
        &session,
        Speaker::Assistant,
        "is held responsible",
        1_000,
        2_200,
    )
    .await;
    loop {
        let e = next_emission(&mut live, "the answer's `spoken`", &seen).await;
        let done = through(&e, "spoken");
        seen.push(e);
        if done {
            break;
        }
    }
    tokio::time::advance(Duration::from_millis(300)).await;
    session
        .audio_out
        .send(speech(9_600))
        .await
        .expect("the connection takes model audio");
    let _ = line.hear().await;
    says(
        &session,
        Speaker::Assistant,
        " because he controls the vehicle.",
        2_200,
        3_400,
    )
    .await;
    session
        .audio_out
        .send(speech(9_600))
        .await
        .expect("the connection takes model audio");
    let _ = line.hear().await;
    says(&session, Speaker::User, "And when?", 6_000, 6_600).await;
    let answer = loop {
        let e = next_emission(&mut live, "the `turn` with the whole answer", &seen).await;
        let done = through(&e, "turn") && said_by(&e, "assistant").contains("because");
        seen.push(e);
        if done {
            break seen.last().cloned().expect("pushed above");
        }
    };

    let stamps: Vec<&Value> = seen
        .iter()
        .filter(|e| emission_route(e) == Some("first_audio"))
        .collect();
    assert_eq!(
        stamps.len(),
        1,
        "one stamp for the one answer: saw {:?}",
        routes(&seen)
    );
    assert_eq!(
        turns(&seen).len(),
        1,
        "the answer is one turn: {:?}",
        turns(&seen)
    );
    assert_eq!(
        said_by(&answer, "assistant"),
        "is held responsible because he controls the vehicle.",
        "{answer}"
    );
    assert_eq!(
        header(&answer, "t_person_end_ms"),
        Some(&json!(1_000)),
        "{answer}"
    );
    assert!(
        header(&answer, "t_first_audio_ms").is_some(),
        "the turn carries the answer's first audio: {answer}"
    );
    assert_eq!(
        header(stamps[0], "t_first_audio_ms"),
        header(&answer, "t_first_audio_ms"),
        "the stamp carries the time the turn carries: {}",
        stamps[0]
    );
    assert_eq!(
        header(stamps[0], "turn_id"),
        header(&answer, "turn_id"),
        "the stamp names the turn it belongs to: {}",
        stamps[0]
    );
    drop(clock);
}

#[tokio::test(flavor = "current_thread")]
async fn gh_d54g_weblive_seam_a_first_answer_ahead_of_its_audio_is_stamped_once() {
    rounds(
        "gh_d54g_weblive_seam_a_first_answer_ahead_of_its_audio_is_stamped_once",
        Access::BOTH,
        a_first_answer_ahead_of_its_audio,
    )
    .await;
}

// ------------------------------------------------- a cut text on a busy line

/// Three paragraphs of one horizon, each a part of its own under the cut.
const SPLIT_HORIZON: &str = "Part one: the answer is coming.\n\n\
                             Part two: about {seconds} seconds.\n\n\
                             Part three: keep the talk going.";

/// The cut of one append for [`SPLIT_HORIZON`].
const SPLIT_CHARS: usize = 40;

/// Every append the model is told, up to and including the one carrying
/// `last`.
async fn told_until(live: &mut Live, last: &str) -> Vec<String> {
    let mut out = Vec::new();
    loop {
        let control = tokio::time::timeout(marker(), live.controls.recv())
            .await
            .unwrap_or_else(|_| panic!("HANG: `{last}` never reached the model; saw {out:?}"))
            .expect("the fake is running");
        if let DuplexControl::Append { content, .. } = control {
            let done = content == last;
            out.push(content);
            if done {
                return out;
            }
        }
    }
}

/// `gh_d54g_callseam_the_parts_of_one_cut_wait_together_for_the_quiet_line`:
/// the page released its key, the model answers, and a text cut into parts
/// reaches the model whole at the first quiet moment.
async fn the_parts_of_one_cut_wait_together(access: Access) {
    const TAG: &str = "wcut-1";
    round("the parts of one cut wait together", access, None);
    let params = duplex_params(json!({
        "default_mode": "auto",
        "duplex": {
            "provider": "gpt_live",
            "api_key": "test-key",
            "instructions": "A test line.",
            "sample_rate": 16_000,
            "tick_ms": 100,
            "turn_gap_ms": 300,
            "horizon_text": SPLIT_HORIZON,
            "advise_split_chars": SPLIT_CHARS,
        },
    }));
    let mut live = boot_fake_timed(params, 3_000, CAP_MS * 8).await;
    let (mut line, session) = live.open_line(access, TAG).await;
    if access == Access::Page {
        line.release().await;
    }
    let name = access.session_name(TAG, 1);

    // The model speaks; the person hears it, so the line is busy.
    let audio: Vec<u8> = (0..320).flat_map(|_| 8_000i16.to_le_bytes()).collect();
    session.audio_out.send(audio).await.expect("audio");
    let _ = line.hear().await;

    live.send(duplex_cell::advise_msg_with_body(
        &name,
        "horizon",
        json!({"messages": [], "section": "horizon", "seconds": 6,
               "way": "quick", "family": "web"}),
        None,
    ))
    .await;
    live.send(advise_msg(&name, "context", "Marker.", None))
        .await;
    assert_eq!(
        told_until(&mut live, "Marker.").await,
        vec!["Marker.".to_string()],
        "the horizon waits while the model speaks"
    );
    assert_eq!(
        told_until(&mut live, "Part three: keep the talk going.").await,
        vec![
            "Part one: the answer is coming.".to_string(),
            "Part two: about 6 seconds.".to_string(),
            "Part three: keep the talk going.".to_string(),
        ],
        "at the quiet moment every part of the cut goes, in its order"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_seam_the_parts_of_one_cut_wait_together_for_the_quiet_line() {
    rounds(
        "gh_d54g_weblive_seam_the_parts_of_one_cut_wait_together_for_the_quiet_line",
        Access::BOTH,
        the_parts_of_one_cut_wait_together,
    )
    .await;
}

// ------------------------------------------------- only what was spoken

/// `gh_d54g_callseam_control_marks_never_reach_a_turn_text` (harness `voice`,
/// on the adapter alone), through the whole cell: the real `gpt_live`
/// adapter against the mock socket, a session opened through each door, and
/// the model's transcript with whole, split and broken control marks in it.
/// What leaves as a turn is what the model said, without a piece of a mark.
async fn control_marks_never_reach_a_turn(access: Access) {
    const TAG: &str = "wseam-marks";
    round("control marks never reach a turn", access, None);
    let mock = MockGptLive::start(LiveScript {
        session_id: "sess_marks".to_string(),
        actions: vec![
            LiveAction::Transcript {
                speaker: "assistant",
                delta: "The doctor is booked.<|message|>",
                start_ms: 100,
                end_ms: 900,
            },
            LiveAction::Transcript {
                speaker: "assistant",
                delta: " It is on Friday.<|mes",
                start_ms: 900,
                end_ms: 1_500,
            },
            LiveAction::Transcript {
                speaker: "assistant",
                delta: "sage|> At nine.",
                start_ms: 1_500,
                end_ms: 2_000,
            },
            LiveAction::Transcript {
                speaker: "assistant",
                delta: " That is due.|message|>Today the call is due.",
                start_ms: 2_000,
                end_ms: 3_200,
            },
            LiveAction::CloseWith {
                reason: "drained",
                usage_seconds: 3.2,
            },
        ],
        ..LiveScript::default()
    })
    .await
    .expect("the mock socket binds");
    let block: GptLiveParams = serde_json::from_value(json!({
        "api_key": "test-key",
        "base_url": mock.base_url(),
        "instructions": "A test line.",
        "sample_rate": RATE,
    }))
    .expect("the gpt_live block parses");
    let provider: Arc<dyn DuplexProvider> =
        Arc::new(GptLiveDuplex::new(block).with_timeouts(ProviderTimeouts::default()));
    let mut live = boot_recovering_with(metered(), provider, 0, 0).await;
    let _line = match access {
        Access::Phone => {
            let (client, hello) = live.connect(&format!("session={TAG}&mode=auto")).await;
            assert_eq!(hello["duplex"], true, "got {hello}");
            Line::Socket(client)
        }
        Access::Page => {
            let (client, _hello) = live.link(TAG, "hold").await;
            let mut line = Line::Link(client);
            line.hold().await;
            line
        }
    };

    let seen = until_final_usage(&mut live).await;
    let closed = turns(&seen);
    assert!(
        !closed.is_empty(),
        "what the model said leaves as a turn: {:?}",
        routes(&seen)
    );
    let mut spoken = String::new();
    for turn in &closed {
        assert_eq!(
            header(turn, "call_id"),
            Some(&json!(access.session_name(TAG, 1))),
            "{turn}"
        );
        let said = said_by(turn, "assistant");
        for piece in ["<|", "|>", "|", "message", "mes", "sage"] {
            assert!(
                !said.contains(piece),
                "a piece of a control mark (`{piece}`) is in a turn: {said:?}"
            );
        }
        spoken.push_str(&said);
    }
    let words = |s: &str| s.split_whitespace().collect::<String>();
    assert_eq!(
        words(&spoken),
        words("The doctor is booked. It is on Friday. At nine. That is due.Today the call is due."),
        "the turns without the marks are exactly what the model said: {spoken:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_seam_control_marks_never_reach_a_turn_text() {
    rounds(
        "gh_d54g_weblive_seam_control_marks_never_reach_a_turn_text",
        Access::BOTH,
        control_marks_never_reach_a_turn,
    )
    .await;
}

// ------------------------------------------------- the append ceiling (deleg)

/// The fallback sentence of the deleg fixture.
const FALLBACK: &str = "I cannot look that up right now.";

/// The provider's own words for an append over its ceiling.
const REFUSAL: &str = "Context append text must not exceed 500 tokens.";

/// The ceiling as shipped: 500 tokens at 2.4 characters each, less a tenth.
const SHIPPED_LIMIT: usize = 1_080;

/// The deleg fixture's params; `extra` lands in the `duplex` block.
fn deleg_params(extra: Value) -> Value {
    let mut duplex = json!({
        "provider": "gpt_live",
        "api_key": "test-key",
        "instructions": "A test line.",
        "sample_rate": 16_000,
        "tick_ms": 100,
        "delegation_grace_ms": 600_000,
        "delegation_fallback": FALLBACK,
    });
    if let (Some(obj), Some(more)) = (duplex.as_object_mut(), extra.as_object()) {
        for (k, v) in more {
            obj.insert(k.clone(), v.clone());
        }
    }
    duplex_params(json!({"default_mode": "auto", "duplex": duplex}))
}

/// One append the model was handed.
#[derive(Debug, Clone, PartialEq)]
struct Sent {
    kind: AppendKind,
    event_id: String,
    delegation_id: Option<String>,
    content: String,
}

/// The next append the model is handed.
async fn next_append(live: &mut Live) -> Sent {
    loop {
        let control = tokio::time::timeout(marker(), live.controls.recv())
            .await
            .expect("HANG: no append reached the model")
            .expect("the fake is running");
        if let DuplexControl::Append {
            kind,
            event_id,
            delegation_id,
            content,
        } = control
        {
            return Sent {
                kind,
                event_id,
                delegation_id,
                content,
            };
        }
    }
}

/// Every append before the one carrying `marker`.
async fn appends_before(live: &mut Live, marker: &str) -> Vec<Sent> {
    let mut out = Vec::new();
    loop {
        let sent = next_append(live).await;
        if sent.content == marker {
            return out;
        }
        out.push(sent);
    }
}

/// A `context` advise as the marker that ends a collection.
fn advise_marker(call: &str) -> Message {
    advise_msg(call, "context", "Marker.", None)
}

/// The refusal of one append, as the adapter hands it on.
fn refusal(event_id: &str) -> DuplexEvent {
    DuplexEvent::Warning {
        detail: REFUSAL.to_string(),
        provider_code: Some("invalid_value".to_string()),
        client_event_id: Some(event_id.to_string()),
    }
}

/// `count` numbered sentences with numbers and a time in each.
fn sentences(count: usize) -> String {
    (1..=count)
        .map(|n| format!("Entry {n} of the list starts at 9:30 in room 204 and ends at 11:45."))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The curator's horizon for the session `call`.
fn horizon(call: &str, seconds: u64) -> Message {
    let mut context = Map::new();
    context.insert("call_id".into(), json!(call));
    let mut hop = Map::new();
    hop.insert("route".into(), json!("in_advise"));
    hop.insert("section".into(), json!("horizon"));
    MessageBuilder::new(Path::new("/voice"))
        .context(context)
        .hop(hop)
        .body(Body::Inline(json!({
            "messages": [], "section": "horizon", "seconds": seconds,
            "way": "quick", "family": "web"
        })))
        .build()
}

/// A session through `access` on a cell with the deleg params, the page's key
/// up again (the person has said what they had to say): the session's name,
/// the line and the fake's handle.
async fn a_quiet_session(
    live: &mut Live,
    access: Access,
    tag: &str,
) -> (String, Line, FakeSession) {
    let (mut line, session) = live.open_line(access, tag).await;
    if access == Access::Page {
        line.release().await;
    }
    (access.session_name(tag, 1), line, session)
}

/// One session on a cell of its own: `text` as a `fact`, and every append
/// the model was handed for it.
async fn fact_parts(access: Access, extra: Value, tag: &str, text: &str) -> Vec<Sent> {
    let mut live = boot_fake(deleg_params(extra)).await;
    let (name, _line, _session) = a_quiet_session(&mut live, access, tag).await;
    live.send(advise_msg(&name, "fact", text, None)).await;
    live.send(advise_marker(&name)).await;
    appends_before(&mut live, "Marker.").await
}

/// `gh_d54g_deleg_an_append_over_the_limit_goes_out_in_parts_by_default`.
async fn an_append_over_the_limit(access: Access) {
    round("an append over the limit", access, None);
    let text = sentences(24);
    let chars = text.chars().count();
    assert!(chars > SHIPPED_LIMIT && chars <= 1_800, "{chars}");

    let shipped = fact_parts(access, json!({}), "wparts-1", &text).await;
    assert!(
        shipped.len() >= 2,
        "over the ceiling is more than one append: {shipped:?}"
    );
    for part in &shipped {
        assert!(
            part.content.chars().count() <= SHIPPED_LIMIT,
            "no part over the ceiling: {}",
            part.content.chars().count()
        );
        assert_eq!(part.kind, AppendKind::Commentary, "a fact is said");
    }
    let joined = shipped
        .iter()
        .map(|p| p.content.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(joined, text, "nothing lost, nothing doubled");

    let moved = fact_parts(
        access,
        json!({"advise_max_tokens": 100, "chars_per_token": 2.0}),
        "wparts-2",
        &text,
    )
    .await;
    assert!(
        moved.len() > shipped.len(),
        "a lower ceiling is more parts: {} against {}",
        moved.len(),
        shipped.len()
    );
    for part in &moved {
        assert!(
            part.content.chars().count() <= 180,
            "the instance's ceiling holds: {}",
            part.content.chars().count()
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_deleg_an_append_over_the_limit_goes_out_in_parts_by_default() {
    rounds(
        "gh_d54g_weblive_deleg_an_append_over_the_limit_goes_out_in_parts_by_default",
        Access::BOTH,
        an_append_over_the_limit,
    )
    .await;
}

/// `gh_d54g_deleg_the_parts_keep_their_order_and_cut_at_sentence_ends`.
async fn the_parts_keep_their_order(access: Access) {
    round("the parts keep their order", access, None);
    let text = sentences(48);
    let parts = fact_parts(access, json!({}), "worder-1", &text).await;
    assert!(parts.len() >= 3, "{} parts", parts.len());
    for part in &parts {
        assert!(
            part.content.ends_with("11:45."),
            "every part ends with a whole sentence: {:?}",
            part.content
        );
        assert!(
            part.content.starts_with("Entry "),
            "and begins with one: {:?}",
            part.content
        );
    }
    let joined = parts
        .iter()
        .map(|p| p.content.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(
        joined, text,
        "the order of the text is the order of the parts"
    );
    let ids: std::collections::HashSet<&str> = parts.iter().map(|p| p.event_id.as_str()).collect();
    assert_eq!(
        ids.len(),
        parts.len(),
        "every part has an event id of its own"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_deleg_the_parts_keep_their_order_and_cut_at_sentence_ends() {
    rounds(
        "gh_d54g_weblive_deleg_the_parts_keep_their_order_and_cut_at_sentence_ends",
        Access::BOTH,
        the_parts_keep_their_order,
    )
    .await;
}

/// `gh_d54g_deleg_the_waiting_advise_is_split_like_any_append`.
async fn the_waiting_advise_is_split(access: Access) {
    round("the waiting advise is split", access, None);
    let horizon_text = "The answer takes about {seconds} seconds. Keep the talk going until \
                        then. Say that you are looking it up. Do not promise more than that. \
                        Ask nothing new while you wait.";
    let mut live = boot_fake(deleg_params(json!({
        "horizon_text": horizon_text,
        "advise_split_chars": 80,
    })))
    .await;
    let (name, _line, _session) = a_quiet_session(&mut live, access, "wwait-1").await;
    live.send(horizon(&name, 4)).await;
    live.send(advise_marker(&name)).await;
    let parts = appends_before(&mut live, "Marker.").await;
    assert!(
        parts.len() >= 2,
        "a waiting advise over the ceiling is cut: {parts:?}"
    );
    for part in &parts {
        assert_eq!(part.kind, AppendKind::Instructions, "{part:?}");
        assert!(part.content.chars().count() <= 80, "{part:?}");
        assert!(
            part.content.ends_with('.'),
            "cut at a sentence end: {part:?}"
        );
    }
    let joined = parts
        .iter()
        .map(|p| p.content.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(joined, horizon_text.replace("{seconds}", "4"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_deleg_the_waiting_advise_is_split_like_any_append() {
    rounds(
        "gh_d54g_weblive_deleg_the_waiting_advise_is_split_like_any_append",
        Access::BOTH,
        the_waiting_advise_is_split,
    )
    .await;
}

/// `gh_d54g_deleg_a_refused_append_is_reported_with_section_and_length`.
async fn a_refused_append_is_reported(access: Access) {
    round("a refused append is reported", access, None);
    let mut live = boot_fake(deleg_params(json!({}))).await;
    let (name, _line, session) = a_quiet_session(&mut live, access, "wrefused-1").await;
    let note = "A note of forty-one characters, no more.";
    live.send(advise_msg(&name, "context", note, None)).await;
    let sent = next_append(&mut live).await;
    assert_eq!(sent.content, note);

    session.say(refusal(&sent.event_id)).await;
    let reported = live.emission("error").await;
    assert_eq!(
        header(&reported, "error_code"),
        Some(&json!("advise_append_refused")),
        "{reported}"
    );
    assert_eq!(
        reported["meta"],
        json!({
            "detail": REFUSAL,
            "provider_code": "invalid_value",
            "section": "context",
            "chars": note.chars().count(),
        }),
        "{reported}"
    );

    session.say(refusal("an-event-of-somebody-else")).await;
    let foreign = live.emission("error").await;
    assert_eq!(
        header(&foreign, "error_code"),
        Some(&json!("duplex_warning")),
        "{foreign}"
    );
    assert_eq!(
        foreign["meta"],
        json!({"detail": REFUSAL, "provider_code": "invalid_value"}),
        "{foreign}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_deleg_a_refused_append_is_reported_with_section_and_length() {
    rounds(
        "gh_d54g_weblive_deleg_a_refused_append_is_reported_with_section_and_length",
        Access::BOTH,
        a_refused_append_is_reported,
    )
    .await;
}

/// `gh_d54g_deleg_a_refused_answer_fails_its_delegation_at_once`.
async fn a_refused_answer_fails_its_delegation(access: Access) {
    round("a refused answer fails its delegation", access, None);
    let mut live = boot_fake(deleg_params(json!({}))).await;
    let (name, _line, session) = a_quiet_session(&mut live, access, "wrefused-2").await;
    session
        .say(DuplexEvent::DelegationCreated {
            delegation_id: "dlg-r".to_string(),
            offset_ms: 0,
        })
        .await;
    let delegation = live.emission("delegation").await;
    assert_eq!(
        header(&delegation, "call_id"),
        Some(&json!(name)),
        "the delegation names the session: {delegation}"
    );
    let answer = "The list has three entries for today.";
    live.send(advise_msg(&name, "fact", answer, Some("dlg-r")))
        .await;
    let sent = next_append(&mut live).await;
    assert_eq!(sent.content, answer);
    assert_eq!(sent.delegation_id.as_deref(), Some("dlg-r"));

    session.say(refusal(&sent.event_id)).await;
    let reported = live.emission("error").await;
    assert_eq!(
        header(&reported, "error_code"),
        Some(&json!("advise_append_refused")),
        "{reported}"
    );
    assert_eq!(
        reported["meta"],
        json!({
            "detail": REFUSAL,
            "provider_code": "invalid_value",
            "section": "fact",
            "chars": answer.chars().count(),
            "delegation_id": "dlg-r",
        }),
        "{reported}"
    );
    let fallback = next_append(&mut live).await;
    assert_eq!(
        (
            fallback.kind,
            fallback.delegation_id.as_deref(),
            fallback.content.as_str()
        ),
        (AppendKind::Commentary, Some("dlg-r"), FALLBACK),
        "the fallback on that delegation, at once"
    );

    session.say(refusal(&fallback.event_id)).await;
    let again = live.emission("error").await;
    assert_eq!(
        header(&again, "error_code"),
        Some(&json!("advise_append_refused")),
        "{again}"
    );
    assert_eq!(again["meta"]["chars"], json!(FALLBACK.chars().count()));
    session.say(refusal(&sent.event_id)).await;
    let twice = live.emission("error").await;
    assert_eq!(
        header(&twice, "error_code"),
        Some(&json!("advise_append_refused")),
        "{twice}"
    );
    live.send(advise_marker(&name)).await;
    let more = appends_before(&mut live, "Marker.").await;
    assert!(
        more.is_empty(),
        "a failed delegation hears its fallback once: {more:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_weblive_deleg_a_refused_answer_fails_its_delegation_at_once() {
    rounds(
        "gh_d54g_weblive_deleg_a_refused_answer_fails_its_delegation_at_once",
        Access::BOTH,
        a_refused_answer_fails_its_delegation,
    )
    .await;
}
