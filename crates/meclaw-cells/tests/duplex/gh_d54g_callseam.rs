//! d54g callseam -- the seams of a live session: nothing after the end, and no
//! takeover into running speech.
//!
//! Measured on a live colony (2026-11-07, three calls in a row): the tail of
//! one call opened the next conversation, a renewed session took over in the
//! middle of the caller's sentence, and a turn carried a control mark of the
//! model. The rule behind every lock here: what the person says is not lost,
//! and nothing foreign stands at the beginning of a conversation.
//!
//! **The end of a session.** When the connection ends, the cell closes the open
//! turn at once from the transcript it holds and emits it before the call's
//! final `usage`. What the provider still delivers during the close grace is
//! not a turn. A sentence of the model that the end cut off carries
//! `cut: true` in the turn's header. Outside the cell, the `session-keeper`
//! opens no conversation session for a turn whose handle `live` belongs to a
//! session that has ended.
//!
//! **A renewal.** A renewal that is due waits while the caller's turn is open
//! and takes over when that turn closes. Only past the hard cap does it take
//! over inside a turn; then the new session is handed what was heard so far,
//! and the turn the takeover falls into carries the words of both sessions in
//! the order they were spoken.
//!
//! **The stamp.** With `emit_first_audio` set, the first audible audio
//! of an answer is one emission on the `first_audio` lane, with the time the
//! closed turn later carries as `t_first_audio_ms`.
//!
//! How it is measured: the handler's lanes are read off its emissions and the
//! provider's side off the fake's journal. Every claim is an order of events.
//! Where the caller or the model "talks", a task says something every few
//! milliseconds; that pace is the stimulus and never the verdict.

// harness_fold: #[path = "support/duplex_cell.rs"]
#[allow(unused_imports)]
use crate::duplex_cell; // harness_fold: mod duplex_cell;

use duplex_cell::keeper;
use duplex_cell::{
    CAP_MS, FRAME_BYTES, FakeSession, LinkClient, Live, RATE, Seen, boot_fake, boot_fake_clocked,
    boot_fake_timed, boot_lingering, boot_renewing, boot_renewing_timed, duplex_params,
    emission_route, handover_msg, header, marker,
};
use meclaw_cells::voice::contract::{DuplexControl, DuplexEvent, Speaker};
use meclaw_colony::mutation::subtree::{CellNode, check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{Value, json};
use meclaw_testing::deadline;
use meclaw_testing::voice_client::VoiceClient;
use std::time::Duration;
use tokio::time::Instant;

/// A renewal opens this long after a session took charge of the call.
const RENEW_AFTER_MS: u64 = 300;

/// A grace no test in this file reaches.
const NO_GRACE_MS: u64 = 20_000;

/// The line counts as quiet this long after the last word. Shorter than the
/// turn gap of one second, so "quiet" comes before the caller's turn closes.
const QUIET_MS: u64 = 300;

/// The session clock of the renewal locks: a turn closes on a tick.
const TICK_MS: u64 = 100;

/// The block a colony writes in answer to the `renewed` lane.
const HANDOVER: &str = "Handover: the caller asked for the opening hours of the library; \
                        the answer was nine to six.";

/// A duplex cell in `auto` that reports the call's usage.
fn metered() -> Value {
    duplex_params(json!({"default_mode": "auto", "emit_usage": true}))
}

/// A duplex cell in `auto`.
fn plain() -> Value {
    duplex_params(json!({"default_mode": "auto"}))
}

/// One 20 ms frame whose every byte is `n`, so a frame names itself.
fn frame(n: u8) -> Vec<u8> {
    vec![n; FRAME_BYTES]
}

/// `len` bytes of PCM16 at a steady 8 000 (about -12 dBFS): audible speech.
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

/// Whether an emission is the call's final `usage`.
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

/// Every emission up to and including the call's final `usage`, in order.
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

/// Every emission up to and including the next `turn`, in order.
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

/// Every emission up to and including the next `partial` of the call
/// `session_id`, in order. The handler takes events in order, so this is the
/// marker for "everything an earlier call still had to say has been said".
async fn until_partial_of(live: &mut Live, session_id: &str) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    loop {
        let e = next_emission(live, "a `partial` of the next call", &seen).await;
        let last = emission_route(&e) == Some("partial")
            && header(&e, "session_id") == Some(&json!(session_id));
        seen.push(e);
        if last {
            return seen;
        }
    }
}

/// The index of the next session whose way in was shut: the provider's view
/// of "the connection let go of me".
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

// ------------------------------------------------------------ the end of a session

/// What the provider still delivers after the connection ended is not a turn,
/// and after the final `usage` no turn of that call leaves at all.
///
/// The caller says a sentence and hangs up. The provider's session holds its
/// verdict (the renewing fake does until `finish`), delivers one more
/// fragment -- the tail a live model transcribes after the line is gone --
/// and closes. A second call is the marker behind the first one's final
/// `usage`: the handler takes events in order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_callseam_no_turn_leaves_after_the_final_usage() {
    const REST: &str = "And one more thing.";
    let mut live = boot_renewing(metered(), 0, NO_GRACE_MS).await;
    let (client, hello) = live.connect("session=seam-1&mode=auto").await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    let mut session = live.session().await;

    says(&session, Speaker::User, "Until tomorrow then.", 0, 900).await;
    let _ = live.emission("partial").await;
    client.close().await.expect("the caller hangs up");
    assert_eq!(next_ended(&mut live).await, 1, "the connection ended");

    // The connection is over; the provider is not done yet.
    says(&session, Speaker::User, REST, 1_000, 1_800).await;
    session.finish();
    drop(session);

    let first_call = until_final_usage(&mut live).await;
    for turn in turns(&first_call) {
        assert!(
            !said_by(turn, "user").contains(REST) && !said_by(turn, "assistant").contains(REST),
            "what the provider delivered after the connection ended left as a turn: {turn}"
        );
    }

    // The marker: a second call. Whatever the first call still emitted stands
    // before the second call's first `partial`.
    let (_client, _hello) = live.connect("session=seam-1b&mode=auto").await;
    let next = live.session().await;
    says(&next, Speaker::User, "Hello again.", 0, 600).await;
    let after = until_partial_of(&mut live, "seam-1b").await;
    let late: Vec<&Value> = after
        .iter()
        .filter(|e| {
            emission_route(e) == Some("turn") && header(e, "session_id") == Some(&json!("seam-1"))
        })
        .collect();
    assert!(
        late.is_empty(),
        "a turn of a call left after that call's final `usage`: {late:?}"
    );
}

/// The open turn leaves with the end of the connection: before the final
/// `usage`, and without waiting for the provider.
///
/// The model here never finalises (the lingering fake), so the connection
/// waits its whole close grace and the provider never sends a close. The
/// caller's last sentence must not depend on that close: it is emitted before
/// the connection lets go of the model, and so before the final `usage`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_callseam_the_open_turn_closes_with_the_connection_before_the_final_usage() {
    const LAST: &str = "Please remind me at six.";
    const GRACE_MS: u64 = 400;
    let mut live = boot_lingering(metered(), GRACE_MS).await;
    let (client, hello) = live.connect("session=seam-2&mode=auto").await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    let session = live.session().await;

    says(&session, Speaker::User, LAST, 0, 900).await;
    let _ = live.emission("partial").await;
    client.close().await.expect("the caller hangs up");

    // The turn, or the moment the connection let go of the model after its
    // close grace -- whichever is first. The turn's arm is polled first: it
    // loses only where the turn is not there yet.
    let turn = deadline::hang("the caller's last turn leaves", async {
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
                        "the call's final `usage` left and the caller's last sentence \
                         never did -- it is lost"
                    );
                }
                waited = live.let_go.recv() => {
                    panic!(
                        "the connection let go of the model after {waited:?} and the \
                         caller's last turn had not left: the turn waits for the \
                         provider's close"
                    );
                }
            }
        }
    })
    .await;
    assert_eq!(said_by(&turn, "user"), LAST, "{turn}");
    assert_eq!(header(&turn, "turn_id"), Some(&json!("seam-2#0")), "{turn}");
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

/// How many chunks of the model's answer the caller hears before hanging up.
const SPOKEN_CHUNKS: usize = 3;

/// A sentence of the model that the end of the call cut off is marked `cut`.
///
/// The model is in the middle of its answer -- its transcript runs, its audio
/// flows and the caller hears it -- when the caller hangs up. The turn carries
/// both voices, as ever, and the header says that the model's sentence was not
/// said to its end: a reader does not treat it as a statement that was made.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_callseam_a_sentence_cut_by_the_end_is_marked_cut() {
    const ASKED: &str = "What is on tomorrow?";
    const ANSWER: &str = "Tomorrow at ten you have the";
    // The model counts as speaking for as long as the test can run after its
    // audio went out: the hang-up below comes after the caller HEARD that
    // audio, and no load between the two can make the model fall silent. The
    // quiet is the hang mark, never a verdict; the cap is the ceiling of the
    // same heuristic and stands above it, as the shipped pair does (1 500 ms
    // under 8 000 ms). Measured on build02 (2026-11-13, five chunks, twice
    // each): with the quiet above the cap a turn closed before the model's
    // first audio and nothing was cut; with it below, every run was cut.
    //
    // Only the end may close this turn. The session clock ticks, and a tick
    // after the model's fragment closes a turn whose caller has been quiet
    // longer than `turn_gap_ms` (measured on build02, 2026-11-13: one run in
    // three closed it that way, uncut, before the hang-up). So the gap is the
    // hang mark too: the caller's silence closes nothing within the test.
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
    let (mut client, hello) = live.connect("session=seam-3&mode=auto").await;
    assert_eq!(hello["duplex"], true, "got {hello}");
    let session = live.session().await;

    says(&session, Speaker::User, ASKED, 0, 900).await;
    let _ = live.emission("partial").await;
    says(&session, Speaker::Assistant, ANSWER, 1_000, 2_400).await;
    let _ = live.emission("spoken").await;

    // The model speaks on: chunk after chunk, each sent once the caller heard
    // the one before -- the caller's ear is the pace, no clock is. The caller
    // hangs up on hearing the last one, while it is still the model's latest
    // word on the line.
    for chunk in 0..SPOKEN_CHUNKS {
        session
            .audio_out
            .send(speech(FRAME_BYTES))
            .await
            .expect("the model's audio goes out");
        let heard = client
            .collect_until(|f| f.as_audio().is_some(), marker())
            .await;
        assert!(
            heard.iter().any(|(_, f)| f.as_audio().is_some()),
            "HANG: the caller never heard chunk {chunk} of the model's answer within {} s",
            marker().as_secs()
        );
    }
    client.close().await.expect("the caller hangs up");

    let seen = until_final_usage(&mut live).await;
    let closed = turns(&seen);
    assert_eq!(closed.len(), 1, "one turn: {:?}", routes(&seen));
    let turn = closed[0];
    assert_eq!(said_by(turn, "user"), ASKED, "{turn}");
    assert_eq!(said_by(turn, "assistant"), ANSWER, "{turn}");
    assert_eq!(
        header(turn, "cut"),
        Some(&json!(true)),
        "the end of the call cut the model's sentence, and the turn says so: {turn}"
    );
}

/// The second safeguard, outside the cell: a turn whose handle `live` belongs
/// to a session that has ended opens no conversation session in the
/// `session-keeper`.
///
/// On the shipped keeper, wired as a parent wires it. A live session runs in
/// one generation; its end closes that generation (GH #1401). A turn that
/// still carries the handle of that session arrives afterwards -- the tail of
/// the session -- and must not open the next generation: the next conversation
/// would begin with the tail of the last one. The turn of a NEW live session
/// opens a generation as ever. No channel is named by the rule, and none here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_callseam_a_turn_of_an_ended_session_opens_no_session() {
    if !keeper::shipped() {
        return;
    }
    const LINE: &str = "line-1";
    let node = keeper::LIVE_NODE;
    let mut k = keeper::boot_keeper().await;

    let (during, said) = k
        .say(keeper::turn(
            LINE,
            node,
            "hello, it is me",
            "t-1",
            Some("live-1"),
        ))
        .await;
    assert_eq!(said, "hello, it is me");
    k.h.send(keeper::answered(&during, "t-1")).await;
    k.h.quiesce().await;

    k.h.send(keeper::live_end(node, "live-1")).await;
    let (closed, reason) = k.next_close().await;
    assert_eq!(
        (closed.as_str(), reason.as_str()),
        (during.as_str(), "call_ended"),
        "the end of the live session closes its generation"
    );

    // The tail: a turn of the session that has just ended.
    k.h.send(keeper::turn(
        LINE,
        node,
        "and one more thing",
        "t-2",
        Some("live-1"),
    ))
    .await;
    k.h.quiesce().await;
    let after_tail = k.generations();
    assert_eq!(
        after_tail,
        vec![(during.clone(), node.to_string(), 1)],
        "a turn of an ended live session opened a conversation session"
    );
    // Whatever became of the tail itself is not this lock's subject.
    while k.sink.try_recv().is_ok() {}

    // A new live session is a new conversation, as ever.
    let (next, said) = k
        .say(keeper::turn(LINE, node, "me again", "t-3", Some("live-2")))
        .await;
    assert_eq!(said, "me again");
    assert_ne!(next, during, "the next session runs in its own generation");
    k.h.quiesce().await;
    let after_next = k.generations();
    assert_eq!(
        after_next,
        vec![(during, node.to_string(), 1), (next, node.to_string(), 0)],
        "one generation per live session"
    );
    k.h.shutdown().await;
}

/// The second safeguard holds where the ended session never had a generation.
///
/// A live session can end before any of its turns opened one: the caller hung
/// up before the first turn was stamped, or the session's generation was
/// sealed already (the night, a round change) and none followed. Then the end
/// seals nothing, and a safeguard that rode on the sealed generation knew
/// nothing of the end (report section "Bauer 3b", OR-d54g.callseam.11): the
/// tail of that session opened a generation, and the next conversation began
/// with it. The end itself is what the keeper records, not a generation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_callseam_a_session_that_ended_without_a_generation_opens_none_after_it() {
    if !keeper::shipped() {
        return;
    }
    const LINE: &str = "line-1";
    let node = keeper::LIVE_NODE;
    let mut k = keeper::boot_keeper().await;

    // The end of a live session that no turn ever reached the keeper for.
    k.h.send(keeper::live_end(node, "live-0")).await;
    k.h.quiesce().await;
    assert_eq!(
        k.generations(),
        Vec::<(String, String, i64)>::new(),
        "an end with nothing open seals nothing and opens nothing"
    );

    // Its tail.
    k.h.send(keeper::turn(
        LINE,
        node,
        "and one more thing",
        "t-1",
        Some("live-0"),
    ))
    .await;
    k.h.quiesce().await;
    assert_eq!(
        k.generations(),
        Vec::<(String, String, i64)>::new(),
        "a turn of a live session that has ended opened a conversation session"
    );
    while k.sink.try_recv().is_ok() {}

    // A new live session is a new conversation, as ever.
    let (next, said) = k
        .say(keeper::turn(LINE, node, "me again", "t-2", Some("live-1")))
        .await;
    assert_eq!(said, "me again");
    k.h.quiesce().await;
    assert_eq!(
        k.generations(),
        vec![(next, node.to_string(), 0)],
        "the next live session opens its own generation"
    );
    k.h.shutdown().await;
}

// ------------------------------------------------------------ a renewal

/// The next control frame any session was told, with the session it reached.
async fn tagged_control(live: &mut Live) -> (usize, DuplexControl) {
    tokio::time::timeout(marker(), live.tagged_controls.recv())
        .await
        .expect("HANG: no control frame reached a session")
        .expect("the fake is running")
}

/// The next `n` audio frames any session of the fake was fed, with the session
/// each one reached.
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

/// What came first on a line where a renewal is due while the caller's turn is
/// open.
#[derive(Debug)]
enum First {
    /// The caller's turn closed.
    Turn(Value),
    /// The session with this index lost its way in: the renewal took over.
    Takeover(usize),
}

/// The caller's turn closing, or a takeover -- whichever is first. The turn's
/// arm is polled first, so where the takeover follows the turn's close the
/// turn is what this returns.
async fn turn_or_takeover(live: &mut Live) -> First {
    deadline::hang(
        "the caller's turn closes or the renewal takes over",
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

/// A renewal that is due waits while the caller speaks.
///
/// The handover has reached the new session, so the renewal is due. The caller
/// is in the middle of a sentence: the turn is open, and the pause after the
/// last fragment is already longer than the quiet the takeover used to wait
/// for (300 ms) and shorter than the gap that closes a turn (one second). The
/// call stays with the session that hears the sentence until the turn closes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_callseam_a_due_renewal_waits_while_the_caller_speaks() {
    const SENTENCE: &str = "Could you please";
    let mut live = boot_renewing_timed(
        plain(),
        RENEW_AFTER_MS,
        NO_GRACE_MS,
        QUIET_MS,
        8_000,
        TICK_MS,
    )
    .await;
    let (mut client, _hello) = live.connect("session=seam-5&mode=auto").await;
    let first = live.session().await;
    let _renewed = live.emission("renewed").await;
    let _second = live.session().await;

    says(&first, Speaker::User, SENTENCE, 5_000, 5_400).await;
    let _ = live.emission("partial").await;
    live.send(handover_msg("seam-5", HANDOVER, 1)).await;
    match tagged_control(&mut live).await {
        (2, DuplexControl::Append { .. }) => {}
        other => panic!("the handover reaches session 2 first; got {other:?}"),
    }

    match turn_or_takeover(&mut live).await {
        First::Turn(turn) => assert_eq!(said_by(&turn, "user"), SENTENCE, "{turn}"),
        First::Takeover(index) => panic!(
            "the renewal took the call from session {index} while the caller's turn \
             was open: a sentence is never split between two sessions"
        ),
    }
    // And then it does take over: the turn is closed, the line is quiet.
    assert_eq!(
        next_ended(&mut live).await,
        1,
        "the old session's ear is shut"
    );
    client
        .send_audio(&frame(1))
        .await
        .expect("the socket is open");
    assert_eq!(
        tagged_audio(&mut live, 1).await,
        vec![(2, frame(1))],
        "after the turn the call runs on the renewed session"
    );
}

/// A renewal whose grace ran out while the caller speaks takes over when the
/// caller's turn closes, and not before.
///
/// No handover: the renewal becomes due by its grace alone (200 ms), and the
/// caller's turn is open at that moment.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_callseam_a_renewal_past_its_grace_takes_over_at_the_turn_close() {
    const SENTENCE: &str = "I wanted to ask you";
    const GRACE_MS: u64 = 200;
    let mut live =
        boot_renewing_timed(plain(), RENEW_AFTER_MS, GRACE_MS, QUIET_MS, 8_000, TICK_MS).await;
    let (mut client, _hello) = live.connect("session=seam-6&mode=auto").await;
    let first = live.session().await;
    let _renewed = live.emission("renewed").await;
    let _second = live.session().await;

    says(&first, Speaker::User, SENTENCE, 5_000, 5_400).await;
    let _ = live.emission("partial").await;

    match turn_or_takeover(&mut live).await {
        First::Turn(turn) => assert_eq!(said_by(&turn, "user"), SENTENCE, "{turn}"),
        First::Takeover(index) => panic!(
            "the grace ran out and the renewal took the call from session {index} \
             before the caller's turn closed"
        ),
    }
    assert_eq!(
        next_ended(&mut live).await,
        1,
        "at the close of the turn the renewal takes over"
    );
    client
        .send_audio(&frame(1))
        .await
        .expect("the socket is open");
    assert_eq!(
        tagged_audio(&mut live, 1).await,
        vec![(2, frame(1))],
        "after the turn the call runs on the renewed session"
    );
}

/// Where the caller's words are placed on the session's timeline when a test
/// lets the caller talk.
const TALK_BASE_MS: u64 = 5_000;
/// One word of a talking caller, and the pause before the next one starts.
const WORD_MS: u64 = 100;
/// The hard cap of the two locks below: a caller who is still talking this
/// long after the renewal became due is taken over inside the turn.
const HARD_CAP_MS: u64 = 600;

/// What a caller who talked until the takeover left behind.
struct Talk {
    /// The words, in the order they were said: `w0 `, `w1 `, ...
    words: Vec<String>,
    /// Where the last of them ended on the old session's timeline.
    last_end_ms: u64,
    /// Everything the handler emitted meanwhile.
    seen: Vec<Value>,
}

/// The caller talks without a pause -- one word every [`WORD_MS`], stamped
/// where it was said -- until the old session's way in is shut. The loop
/// waits for the takeover; each word length that passes without it is one
/// more word (the stimulus, never the verdict). The hang mark is the caller's.
async fn caller_talks_until_the_takeover(live: &mut Live, old: &FakeSession) -> Talk {
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
            assert_eq!(index, 1, "the session in charge is the one taken over");
            return Talk {
                words,
                last_end_ms,
                seen,
            };
        }
    }
}

/// [`caller_talks_until_the_takeover`] under the hang mark.
async fn talks_until_the_takeover(live: &mut Live, old: &FakeSession) -> Talk {
    tokio::time::timeout(marker(), caller_talks_until_the_takeover(live, old))
        .await
        .unwrap_or_else(|_| {
            panic!(
                "HANG: the renewal never took over from a caller who talked for {} s",
                marker().as_secs()
            )
        })
}

/// A line whose renewal becomes due by its grace and is capped at
/// [`HARD_CAP_MS`], with both sessions open.
async fn a_line_with_a_waiting_renewal(tag: &str) -> (Live, VoiceClient, FakeSession, FakeSession) {
    let mut live =
        boot_renewing_timed(plain(), RENEW_AFTER_MS, 200, QUIET_MS, HARD_CAP_MS, TICK_MS).await;
    let (client, _hello) = live.connect(&format!("session={tag}&mode=auto")).await;
    let first = live.session().await;
    let _renewed = live.emission("renewed").await;
    let second = live.session().await;
    (live, client, first, second)
}

/// The turn a takeover falls into keeps every word, in the order the words
/// were spoken.
///
/// The caller talks past the hard cap, so the renewal takes over inside the
/// turn. The new session hears the end of the sentence; the old session's
/// transcript of a word spoken BEFORE the takeover arrives after that -- a
/// live model's transcript lags its audio, and a retiring session still
/// reports what it heard. One turn carries all of it, ordered by where the
/// words lie on the call's timeline, not by when their transcripts arrived.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_callseam_the_turn_across_a_takeover_keeps_every_word() {
    const NEW: &str = "and the rest.";
    const LATE: &str = "late ";
    // The socket stays open for the length of the test.
    let (mut live, _client, first, second) = a_line_with_a_waiting_renewal("seam-7").await;
    let talk = talks_until_the_takeover(&mut live, &first).await;
    assert!(
        turns(&talk.seen).is_empty(),
        "no turn closes while the caller talks: {:?}",
        routes(&talk.seen)
    );

    // The new session hears the end of the sentence. Its clock starts where
    // the call's clock stood at the takeover, which is at most one word
    // before the end of the last word said above; 300 ms on it is therefore
    // behind the late word below and inside the turn gap.
    says(&second, Speaker::User, NEW, 300, 500).await;
    let mut seen: Vec<Value> = Vec::new();
    loop {
        let e = next_emission(&mut live, "the new session's words as a `partial`", &seen).await;
        let through = emission_route(&e) == Some("partial") && said_by(&e, "user").contains(NEW);
        seen.push(e);
        if through {
            break;
        }
    }
    // The old session's transcript of a word spoken before the takeover.
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
        "the turn carries the words of both sessions in the order they were spoken"
    );
}

/// The words a control frame tells a session.
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

/// At the hard cap the new session is handed what was heard so far.
///
/// The caller talks past the hard cap and the renewal takes over inside the
/// sentence. Before the new session hears the caller it is told the part of
/// the sentence the old session had heard: it does not start in the middle of
/// a sentence it knows nothing of.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_callseam_the_hard_cap_hands_the_partial_over() {
    let (mut live, mut client, first, _second) = a_line_with_a_waiting_renewal("seam-8").await;

    let talk = talks_until_the_takeover(&mut live, &first).await;
    assert!(
        talk.words.len() >= 3,
        "the caller said a few words before the cap: {:?}",
        talk.words
    );
    client
        .send_audio(&frame(1))
        .await
        .expect("the socket is open");

    // Everything session 2 was told before it heard the caller.
    let mut told: Vec<DuplexControl> = Vec::new();
    loop {
        match next_seen_at(&mut live, 2).await {
            Seen::Control(control) => told.push(control),
            Seen::Audio(bytes) => {
                assert_eq!(bytes, frame(1), "the caller's audio after the takeover");
                break;
            }
        }
    }
    let heard_so_far = talk.words[..3].concat();
    let heard_so_far = heard_so_far.trim();
    assert!(
        told.iter().any(|c| told_text(c).contains(heard_so_far)),
        "the session that took over inside a sentence was not told the part \
         already heard (`{heard_so_far}` ...): it was told {told:?}"
    );
}

// ------------------------------------------------------------ the stamp

/// A `gpt_live` block the parse accepts, with a literal key that is not one:
/// the fixture runs the fake provider and reads only the knobs of the handler.
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

/// Boot, pause the clock, open the call on the link.
async fn open(tag: &str, duplex: Value) -> (Live, LinkClient, FakeSession) {
    open_cell(tag, json!({"duplex": duplex})).await
}

/// [`open`] for a cell whose own params are named, the `duplex` block among
/// them: what a set's `override_params` on the instance amounts to.
async fn open_cell(tag: &str, over: Value) -> (Live, LinkClient, FakeSession) {
    let mut params = json!({"default_mode": "auto"});
    if let (Some(obj), Some(more)) = (params.as_object_mut(), over.as_object()) {
        for (k, v) in more {
            obj.insert(k.clone(), v.clone());
        }
    }
    let mut live = boot_fake(duplex_params(params)).await;
    tokio::time::pause();
    let (client, _hello) = live.link(tag, "auto").await;
    let session = live.session().await;
    (live, client, session)
}

/// The caller's question and the model's answer: the question ends at 900 on
/// the call's timeline, the first audible audio of the answer goes out 700 ms
/// later, and the answer's transcript follows. Returns every emission up to
/// and including the closed `turn`.
async fn one_answered_question(
    live: &mut Live,
    client: &mut LinkClient,
    session: &FakeSession,
) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    for (delta, start_ms, end_ms) in [("Is it ", 0, 500), ("raining?", 500, 900)] {
        says(session, Speaker::User, delta, start_ms, end_ms).await;
        // The fragment is through the connection (its clock stamped) once
        // the handler has turned it into a partial.
        loop {
            let e = next_emission(live, "the caller's `partial`", &seen).await;
            let through = emission_route(&e) == Some("partial");
            seen.push(e);
            if through {
                break;
            }
        }
    }
    tokio::time::advance(Duration::from_millis(700)).await;
    session
        .audio_out
        .send(speech(9_600))
        .await
        .expect("the connection takes model audio");
    let _ = client.next_audio().await;
    says(session, Speaker::Assistant, "Not right now.", 1_200, 1_800).await;
    seen.extend(until_turn(live).await);
    seen
}

/// The shipped `templates/` directory as a registry.
fn shipped_registry() -> TemplatesRegistry {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scanned = scan_templates_dir(&root.join("templates")).unwrap_or_default();
    TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
}

/// The root cell of the shipped `voice` template, or `None` where the
/// template did not travel (R2b / GH #49).
fn shipped_voice_cell() -> Option<CellNode> {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("templates/voice");
    if !root.join("config.json").is_file() {
        return None;
    }
    let parsed = parse_subtree(&root, &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped template 'voice' parses: {e:?}"));
    parsed.cells.into_iter().find(|c| c.rel_path.is_empty())
}

/// The first audible audio of an answer is stamped the moment it goes out.
///
/// Until now the time stood only in the closed `turn` (`t_first_audio_ms`), so
/// a measurement had it at the END of the turn. With `emit_first_audio`
/// set, the same time leaves as one emission on the `first_audio` lane as soon
/// as the audio is on its way -- before the answer's transcript and before the
/// turn -- on the session's clock, never a wall clock. Off by default: a
/// composition that draws no edge for the lane emits nothing new.
///
/// The switch is a param of the cell, not of the `duplex` block (a cell that
/// speaks through a speech synthesis has no such block and stamps under the
/// same switch). It is set here as the override of an instance, as a set
/// writes it, not in the template: the door admits it on the shipped
/// template, and the parse reads it.
#[tokio::test(flavor = "current_thread")]
async fn gh_d54g_callseam_the_first_audio_of_an_answer_is_stamped() {
    let over = json!({"duplex": live_block(json!({})), "emit_first_audio": true});
    if let Some(cell) = shipped_voice_cell()
        && let Err(why) = check_override_params(&cell, None, "voice", &over)
    {
        panic!("a set's `emit_first_audio` on 'voice' must pass the door: {why:?}");
    }

    let (mut live, mut client, session) = open_cell("seam-10", over).await;
    let seen = one_answered_question(&mut live, &mut client, &session).await;
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
        "the stamp leaves when the audio does, before the answer's transcript and \
         before the turn: {:?}",
        routes(&seen)
    );
    drop(client);
    drop(live);

    // Off by default: the same call emits nothing on the lane.
    tokio::time::resume();
    let (mut live, mut client, session) = open("seam-10b", live_block(json!({}))).await;
    let seen = one_answered_question(&mut live, &mut client, &session).await;
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
}

/// The first answer of a session is stamped once, although its words run
/// ahead of its audio.
///
/// Measured on a live model (d54g weblive, lab run of 2026-11-13, four of five
/// first holds): the model stamps the first words of its answer right where
/// the caller's ended, and its transcript arrives before its first audible
/// audio. The session's clock passed the end of those words while the model
/// was still talking -- its next words lay within the turn gap, in the
/// middle of the sentence -- and the turn closed
/// there, 4 ms before the answer's first audible frame reached the page. That
/// frame found no open turn, the rest of the answer opened a turn with nothing
/// of the caller's in it, and the answer was never stamped.
///
/// The call here is that sequence, as an order of events on a paused clock:
/// the caller's words, the first words of the answer, a few ticks of the
/// session's clock past them (less than the turn gap), then the audio, then
/// the rest of the answer and its audio, and only then the caller again.
#[tokio::test(flavor = "current_thread")]
async fn gh_d54g_callseam_a_first_answer_ahead_of_its_audio_is_stamped_once() {
    let mut params = json!({"default_mode": "auto", "emit_first_audio": true});
    if let Some(obj) = params.as_object_mut() {
        obj.insert("duplex".into(), live_block(json!({})));
    }
    // A clock that ticks inside the turn gap, so the ticks between the
    // answer's words and its audio are ticks the turn sees.
    let mut live = boot_fake_clocked(duplex_params(params), TICK_MS, 256).await;
    tokio::time::pause();
    let (mut client, _hello) = live.link("seam-11", "auto").await;
    let session = live.session().await;
    let mut seen: Vec<Value> = Vec::new();

    // Each fragment is through the connection (its clock stamped) once the
    // handler has turned it into an emission of its own lane.
    let through = |e: &Value, lane: &str| emission_route(e) == Some(lane);
    for (delta, start_ms, end_ms) in [("So the driver ", 0, 600), ("of the vehicle", 600, 1_000)] {
        says(&session, Speaker::User, delta, start_ms, end_ms).await;
        loop {
            let e = next_emission(&mut live, "the caller's `partial`", &seen).await;
            let done = through(&e, "partial");
            seen.push(e);
            if done {
                break;
            }
        }
    }
    // The first words of the answer, stamped from where the caller's ended
    // and 1 200 ms long: more than the turn gap after the caller's last word.
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
    // The session's clock runs past those words; the model has not fallen
    // silent for a turn gap, its audio is still on the way.
    tokio::time::advance(Duration::from_millis(300)).await;
    session
        .audio_out
        .send(speech(9_600))
        .await
        .expect("the connection takes model audio");
    let _ = client.next_audio().await;
    // The rest of the sentence, contiguous with its first words.
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
    let _ = client.next_audio().await;
    // The caller again, long after: whatever is open closes before it.
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
        "one stamp for the one answer on the `first_audio` lane: saw {:?}",
        routes(&seen)
    );
    assert_eq!(
        turns(&seen).len(),
        1,
        "the answer is one turn, not cut where the clock overtook its words: {:?}",
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
}

// ------------------------------------------------- a cut text on a busy line

/// Three paragraphs of one horizon, each over the half of the cut below, so
/// every paragraph leaves the cell as a part of its own.
const SPLIT_HORIZON: &str = "Part one: the answer is coming.\n\n\
                             Part two: about {seconds} seconds.\n\n\
                             Part three: keep the talk going.";

/// The cut of one append for [`SPLIT_HORIZON`]: one paragraph fits, two do not.
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

/// A text cut into parts reaches the model whole on a busy line.
///
/// A text over the append ceiling leaves the cell in parts, all of one cut
/// (`split_id`). The horizon waits for the line's first quiet moment, and the
/// connection held ONE waiting append, a newer one replacing it (GH #1400):
/// right for a newer horizon, wrong for the next part of the same text, which
/// threw away every part but the last (strand deleg, finding `quiet_held`,
/// decision oc1). The parts of one cut queue in their order; a new text still
/// replaces the waiting one
/// (`d53_v3_a_horizon_advise_reaches_the_voice::a_horizon_advise_becomes_one_corrected_instruction_at_the_turn_boundary`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_callseam_the_parts_of_one_cut_wait_together_for_the_quiet_line() {
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
    let (mut client, _hello) = live.connect("session=cut-1&mode=auto").await;
    let session = live.session().await;

    // The model speaks; the caller hears it, so the line is busy.
    let audio: Vec<u8> = (0..320).flat_map(|_| 8_000i16.to_le_bytes()).collect();
    session.audio_out.send(audio.clone()).await.expect("audio");
    loop {
        let frame = client
            .next_frame(marker())
            .await
            .expect("HANG: the caller gets the model's audio");
        if frame.as_audio().is_some() {
            break;
        }
    }

    live.send(duplex_cell::advise_msg_with_body(
        "cut-1",
        "horizon",
        json!({"messages": [], "section": "horizon", "seconds": 6,
               "way": "quick", "family": "web"}),
        None,
    ))
    .await;
    live.send(duplex_cell::advise_msg("cut-1", "context", "Marker.", None))
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

/// The second safeguard on the edges a colony ships, not on a hand-set handle.
///
/// The keeper records the end of a live session only under the handle the
/// call end carries in `context.live`, and the voice cell's final `usage`
/// names the session on the hop alone (`hop.call_id`, `hop.engine`). So the
/// safeguard holds in a built colony only if the edge that hands the call end
/// to the keeper promotes the handle. Here the end reaches the shipped keeper
/// over the edge `talky` ships to it, read from the template unchanged, in
/// the form the voice cell sends it -- no `context.live`. The tail of that
/// session must be refused with `live_ended` and open no generation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh_d54g_callseam_a_turn_after_a_call_end_drawn_by_talky_is_refused() {
    if !keeper::shipped() || !keeper::talky_shipped() {
        return;
    }
    const LINE: &str = "line-1";
    let node = keeper::LIVE_NODE;
    let mut k = keeper::boot_keeper_wired_like_talky().await;

    let (during, said) = k
        .say(keeper::turn(
            LINE,
            node,
            "hello, it is me",
            "t-1",
            Some("live-1"),
        ))
        .await;
    assert_eq!(said, "hello, it is me");
    k.h.send(keeper::answered(&during, "t-1")).await;
    k.h.quiesce().await;

    let end = keeper::call_end_as_talky_gets_it(node, "live-1");
    assert!(
        end.headers.context.get("live").is_none(),
        "the call end carries no handle of its own"
    );
    k.h.send(end).await;
    let (closed, reason) = k.next_close().await;
    assert_eq!(
        (closed.as_str(), reason.as_str()),
        (during.as_str(), "call_ended"),
        "the end of the live session closes its generation"
    );
    k.h.quiesce().await;

    // The tail: a turn of the session that has just ended.
    k.h.send(keeper::turn(
        LINE,
        node,
        "and one more thing",
        "t-2",
        Some("live-1"),
    ))
    .await;
    let refused = k.next_reject().await;
    assert_eq!(
        refused.headers.hop.get("error_code"),
        Some(&json!("live_ended")),
        "the tail of an ended live session is refused as such: {:?}",
        refused.headers.hop
    );
    k.h.quiesce().await;
    assert_eq!(
        k.generations(),
        vec![(during, node.to_string(), 1)],
        "a turn of an ended live session opened a conversation session"
    );
    k.h.shutdown().await;
}
