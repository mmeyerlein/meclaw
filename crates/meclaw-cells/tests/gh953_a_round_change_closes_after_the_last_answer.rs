//! GH #953 -- a round change closes the old generation after the last answer
//! of the turn before it.
//!
//! WHY this file exists: since GH #940 (ADR-0002 E8) a turn whose round
//! differs from the open generation of its channel SEALS that generation, and
//! the keeper's `close` for it runs on that turn's chain. The close pass reads
//! the old generation's wall at the curator's writer (`wall_select`) and hands
//! the batch to the memory. The final answer of the turn BEFORE the round
//! change reaches the wall on a branch of its own (`./brain -> ./curator`,
//! `in_llm`, the intake's insert) beside the branch that carries the answer
//! to the person (`./brain -> ./splitter -> ... -> ./collector`). Nothing up
//! to #953 had asked whether a person who speaks the instant the answer
//! reaches them -- in a new round -- can have the close read the wall before
//! that answer stands on it. The round-change locks (`session_keeper.rs`,
//! `gh946_*`) wait for the wall after every turn, which is the question this
//! file does NOT wait for.
//!
//! Asked of a real colony (the member road of GH #929: the shipped generation
//! under its container, the member's memory, every `llm` cell on a local
//! stub), 20 times, each run on a channel of its own: a turn in round A, its
//! answer at the surface, and AT ONCE a turn in round B. Measured at the
//! receiver -- the `write` batch the writer built from what `wall_select`
//! returned -- the batch of the sealed generation carries the final answer of
//! the turn before the round change. Every run prints
//! `gh953 run <i>: wall insert @<n>, close read @<m>, lead <m-n>` -- the place
//! in the colony's own `message_log` of the intake's wall insert of that
//! answer and of the writer's close read.
//!
//! The second lock, `a_round_change_waits_for_a_late_last_answer_*`, holds the
//! answer back until the generation is sealed (Review I-3): the order of the
//! close behind the last answer is an event the keeper waits for, not a lead
//! the answer usually has.

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/round_race.rs"]
mod race;
#[path = "support/gh929_member_road.rs"]
mod road;

use race::{LEDGER, WRITER, answer, close_reports, generations, reply, turn};

const RUNS: usize = 20;

/// The runs of a lock, cut into parts: one `#[tokio::test]` per part, each on a
/// colony of its own, over the runs `from..to` of that lock.
///
/// WHY split (GH #1048): each lock drove all its runs as ONE test on ONE
/// colony, one run after the other -- `a_round_change_closes_after_the_last_answer`
/// 100.1-101.0 s, `a_round_change_waits_for_a_late_last_answer` 99.8-101.8 s,
/// `a_double_message_without_ids_closes_after_the_answer_to_the_last_turn`
/// 80.4 s of the 240 s budget (mark 80 s), measured without the lane load of a
/// gate. No sleep of this file drives that time: every wait is on an event
/// (an answer at the surface, a row, a held request, a close report), so the
/// time is the runs themselves, about 5 s (20 runs) and 8 s (10 runs) each.
/// A part keeps the fixtures of its runs unchanged (run `i` keeps its channel
/// and its tags) and makes EVERY assertion per run, as before; nothing of a
/// lock judges across runs -- its summaries only count the runs that failed
/// their own assertion -- so no aggregate is lost by the cut.
/// `the_parts_cover_every_run_once` holds that the parts are all the runs.
macro_rules! parts {
    ($list:ident, $lock:ident: $($name:ident => $from:literal .. $to:literal;)+) => {
        /// Every part the macro made of this lock, in the order it made them.
        const $list: &[std::ops::Range<usize>] = &[$($from..$to),+];
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $name() {
                $lock($from..$to).await;
            }
        )+
    };
}

parts! {
    CLOSES_PARTS, a_round_change_closes_after_the_last_answer:
    a_round_change_closes_after_the_last_answer_runs_0_to_4 => 0..5;
    a_round_change_closes_after_the_last_answer_runs_5_to_9 => 5..10;
    a_round_change_closes_after_the_last_answer_runs_10_to_14 => 10..15;
    a_round_change_closes_after_the_last_answer_runs_15_to_19 => 15..20;
}

parts! {
    LATE_PARTS, a_round_change_waits_for_a_late_last_answer:
    a_round_change_waits_for_a_late_last_answer_runs_0_to_4 => 0..5;
    a_round_change_waits_for_a_late_last_answer_runs_5_to_9 => 5..10;
    a_round_change_waits_for_a_late_last_answer_runs_10_to_14 => 10..15;
    a_round_change_waits_for_a_late_last_answer_runs_15_to_19 => 15..20;
}

parts! {
    IDLESS_PARTS, a_double_message_without_ids_closes_after_the_answer_to_the_last_turn:
    a_double_message_without_ids_closes_after_the_answer_to_the_last_turn_runs_0_to_2 => 0..3;
    a_double_message_without_ids_closes_after_the_answer_to_the_last_turn_runs_3_to_5 => 3..6;
    a_double_message_without_ids_closes_after_the_answer_to_the_last_turn_runs_6_to_7 => 6..8;
    a_double_message_without_ids_closes_after_the_answer_to_the_last_turn_runs_8_to_9 => 8..10;
}

/// The split loses no run: the parts of each lock follow each other without a
/// gap or an overlap, none is empty, and together they are the runs `0..RUNS`
/// (`0..IDLESS_RUNS`) the lock took as one test -- a run count raised without
/// a part, or a part dropped, is red here, not silently untested.
#[test]
fn the_parts_cover_every_run_once() {
    for (lock, parts, runs) in [
        ("closes", CLOSES_PARTS, RUNS),
        ("late", LATE_PARTS, RUNS),
        ("idless", IDLESS_PARTS, IDLESS_RUNS),
    ] {
        let mut next = 0;
        for part in parts {
            assert!(!part.is_empty(), "{lock}: the part {part:?} takes no run");
            assert_eq!(
                part.start, next,
                "{lock}: the part {part:?} does not start where the one before it ended ({next})"
            );
            next = part.end;
        }
        assert_eq!(
            next, runs,
            "{lock}: the parts end at run {next}, the lock has {runs} runs"
        );
    }
}

/// The round the generation is opened in, and the round that ends it.
const ROUND_A: &str = road::AUDIENCE;
const ROUND_B: &str = r#"["member:owner","agent:scribe","member:guest"]"#;

async fn a_round_change_closes_after_the_last_answer(runs: std::ops::Range<usize>) {
    if !road::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let (td, h, mut ports, _brain) = race::boot_road().await;
    let root = td.path().to_path_buf();

    let mut sealed = Vec::new();
    for i in runs.clone() {
        let channel = format!("talky:953-{i}");
        let a = format!("953-{i}-a");
        let b = format!("953-{i}-b");
        h.send(turn(&channel, &a, ROUND_A)).await;
        let got = answer(&mut ports, "the turn of round a").await;
        assert!(
            race::text_of(&got).contains(&reply(&a)),
            "run {i}: the answer at the surface is the answer to the turn of round a: {}",
            race::text_of(&got)
        );
        // No pause: the person speaks in the new round the instant the answer
        // reaches them -- no wait for the wall.
        h.send(turn(&channel, &b, ROUND_B)).await;
        answer(&mut ports, "the turn of round b").await;
        let gens = race::until_rows(
            &race::sessions_db(&root),
            &format!("SELECT session_id FROM sessions WHERE channel = '{channel}' AND closed = 1"),
            1,
            "the turn of round b seals the generation of round a",
        )
        .await;
        let sid = gens[0][0].clone();
        close_reports(&mut ports, &root, std::slice::from_ref(&sid)).await;
        sealed.push((i, sid, a));
    }
    let all: Vec<_> = runs
        .clone()
        .map(|i| (i, generations(&root, &format!("talky:953-{i}"))))
        .collect();
    h.shutdown().await;

    let writes = race::writes(&root);
    let reads: Vec<race::Logged> = race::logged(&root, WRITER, Some(LEDGER))
        .into_iter()
        .filter(|l| l.hop["phase"] == "close-wall")
        .collect();
    let inserts = race::logged(&root, race::INTAKE, Some(LEDGER));
    let mut missing = Vec::new();
    for (i, sid, a) in &sealed {
        let batch = writes
            .get(sid)
            .unwrap_or_else(|| panic!("run {i}: no write batch of {sid}"));
        assert_eq!(batch.len(), 1, "run {i}: one close batch of {sid}");
        let has = batch[0].body.contains(&reply(a));
        let insert = inserts
            .iter()
            // The bundle that enters the answer's block (a `blocks` row has
            // `first_seen`; the parked tap before it does not).
            .find(|l| l.body.contains(&reply(a)) && l.body.contains("first_seen"))
            .map(|l| l.at);
        let read = reads
            .iter()
            .find(|l| l.hop["cur_call"] == sid.as_str())
            .map(|l| l.at);
        eprintln!(
            "gh953 run {i}: wall insert @{insert:?}, close read @{read:?}, lead {:?}, \
             answer in the close: {has}",
            insert.zip(read).map(|(w, r)| r - w)
        );
        if !has {
            missing.push((*i, sid.clone()));
        }
    }
    eprintln!(
        "gh953 summary: {} of {} closes (runs {runs:?}) miss the last answer before the round \
         change",
        missing.len(),
        runs.len()
    );
    assert!(
        missing.is_empty(),
        "the close of a round change read the wall before the last answer stood on it in \
         {} of {} runs ({runs:?}): {missing:?}",
        missing.len(),
        runs.len()
    );
    for (i, gens) in &all {
        assert_eq!(
            gens.iter()
                .map(|g| (g.1.as_str(), g.2.as_str()))
                .collect::<Vec<_>>(),
            vec![(ROUND_A, "1"), (ROUND_B, "0")],
            "run {i}: one sealed generation of round a, one open of round b: {gens:?}"
        );
    }
}

/// The keeper's store and stamp of the generation, for the race signal read
/// off the colony's own `message_log`.
const SESSIONS: &str = "/assistants/scribe/talky/session-keeper/sessions";
const STAMP: &str = "/assistants/scribe/talky/session-keeper/stamp";

/// GH #953, Review I-3 (OR-NL-163): the lock above passes whenever the answer
/// to the person outruns the close on its way to the wall -- it measured 0 of
/// 20 misses with a lead of 47 to 65 deliveries, and that is a probability,
/// not an order. This lock takes the probability away: the brain's answer to
/// the turn of round A is HELD until the turn of round B has sealed round A's
/// generation (the sealed row is the event the test waits for), and only then
/// released. The close of that generation must still carry the answer: the
/// keeper may hand a sealed generation over only once the final answer of its
/// last turn is acknowledged (`./curator` -> `./session-keeper`, lane
/// `in_answered`, the writer's `turn_write` behind the wall insert), never
/// because the answer usually wins.
///
/// Every turn carries a channel turn id (`turn_with_id`): the keeper owes an
/// answer to a turn it knows by id, and a turn without one owes nothing.
///
/// The race is proven per run, not assumed: in the `message_log` the store's
/// answer to the seal of round A's generation stands BEFORE the intake's wall
/// insert of the held answer. Measured at the receiver -- the curator writer's
/// `write` batch of that generation -- the answer must be in it. Every run
/// prints `gh953 late run <i>: seal answered @<s>, wall insert @<w>, close read
/// @<r>, owed while held <o>, answer in the close: <bool>`.
async fn a_round_change_waits_for_a_late_last_answer(runs: std::ops::Range<usize>) {
    if !road::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let brain = race::start_held_brain().await;
    let (td, h, mut ports) = race::boot_road_with(&brain.base_url).await;
    let root = td.path().to_path_buf();

    let mut sealed = Vec::new();
    for i in runs.clone() {
        let channel = format!("talky:953-late-{i}");
        let a = format!("953-late-{i}-a");
        let b = format!("953-late-{i}-b");
        brain.hold(&a);
        h.send(race::turn_with_id(&channel, &a, ROUND_A)).await;
        // The turn of round A is at the brain and its answer is not out.
        brain.until_asked(&a).await;
        // The person speaks in round B before the answer to round A exists.
        h.send(race::turn_with_id(&channel, &b, ROUND_B)).await;
        let gens = race::until_rows(
            &race::sessions_db(&root),
            &format!("SELECT session_id FROM sessions WHERE channel = '{channel}' AND closed = 1"),
            1,
            "the turn of round b seals the generation of round a while its answer is held",
        )
        .await;
        let sid = gens[0][0].clone();
        // What the keeper holds the sealed generation to while the answer is
        // out (a diagnostic: the column does not exist before GH #953).
        let owed = race::rows(
            &race::sessions_db(&root),
            &format!(
                "SELECT COALESCE(owed_turn, '<null>') FROM sessions WHERE session_id = '{sid}'"
            ),
        )
        .first()
        .map_or_else(|| "<no column>".to_string(), |r| r[0].clone());
        // Only now does the answer to round A leave the brain.
        brain.release(&a);
        // The brain answers one turn after the other, so the turn of round b
        // is answered after the released one; both leave at the surface.
        let first = race::text_of(&answer(&mut ports, "the released answer").await);
        let second = race::text_of(&answer(&mut ports, "the turn of round b").await);
        for (tag, what) in [(&a, "round a"), (&b, "round b")] {
            assert!(
                first.contains(&reply(tag)) || second.contains(&reply(tag)),
                "run {i}: the turn of {what} was answered at the surface: {first} / {second}"
            );
        }
        close_reports(&mut ports, &root, std::slice::from_ref(&sid)).await;
        sealed.push((i, sid, a, owed));
    }
    let all: Vec<_> = runs
        .clone()
        .map(|i| (i, generations(&root, &format!("talky:953-late-{i}"))))
        .collect();
    h.shutdown().await;

    let writes = race::writes(&root);
    let reads: Vec<race::Logged> = race::logged(&root, WRITER, Some(LEDGER))
        .into_iter()
        .filter(|l| l.hop["phase"] == "close-wall")
        .collect();
    let inserts = race::logged(&root, race::INTAKE, Some(LEDGER));
    let seals: Vec<race::Logged> = race::logged(&root, SESSIONS, Some(STAMP))
        .into_iter()
        .filter(|l| {
            l.context["ses_phase"]
                .as_str()
                .is_some_and(|p| p.starts_with("seal"))
        })
        .collect();
    let mut missing = Vec::new();
    let mut no_race = Vec::new();
    for (i, sid, a, owed) in &sealed {
        let batch = writes
            .get(sid)
            .unwrap_or_else(|| panic!("run {i}: no write batch of {sid}"));
        assert_eq!(batch.len(), 1, "run {i}: one close batch of {sid}");
        let has = batch[0].body.contains(&reply(a));
        let seal = seals
            .iter()
            .find(|l| l.context["keeper_session"] == sid.as_str())
            .map(|l| l.at);
        let insert = inserts
            .iter()
            .find(|l| l.body.contains(&reply(a)) && l.body.contains("first_seen"))
            .map(|l| l.at);
        let read = reads
            .iter()
            .find(|l| l.hop["cur_call"] == sid.as_str())
            .map(|l| l.at);
        eprintln!(
            "gh953 late run {i}: seal answered @{seal:?}, wall insert @{insert:?}, close read \
             @{read:?}, owed while held {owed}, answer in the close: {has}"
        );
        // The positive signal that the race this lock is about took place:
        // the generation was sealed before its last answer reached the wall.
        if !seal.zip(insert).is_some_and(|(s, w)| s < w) {
            no_race.push((*i, seal, insert));
        }
        if !has {
            missing.push((*i, sid.clone()));
        }
    }
    assert!(
        no_race.is_empty(),
        "the seal did not precede the wall insert of the held answer -- the race this lock \
         is about did not happen (run, seal answered @, wall insert @): {no_race:?}"
    );
    eprintln!(
        "gh953 late summary: {} of {} closes (runs {runs:?}) miss the late last answer",
        missing.len(),
        runs.len()
    );
    assert!(
        missing.is_empty(),
        "a sealed generation was handed over before the held last answer of its turn \
         stood on the wall in {} of {} runs ({runs:?}): {missing:?}",
        missing.len(),
        runs.len()
    );
    for (i, gens) in &all {
        assert_eq!(
            gens.iter()
                .map(|g| (g.1.as_str(), g.2.as_str()))
                .collect::<Vec<_>>(),
            vec![(ROUND_A, "1"), (ROUND_B, "0")],
            "run {i}: one sealed generation of round a, one open of round b: {gens:?}"
        );
    }
}

/// How many runs the id-less lock takes: its order is forced by events
/// (held brain, logged store replies), so every run is the race, not a draw.
const IDLESS_RUNS: usize = 10;

/// Until the keeper's store answered `phase` at least `n` times on a message
/// whose context `key` is `value` (the channel, or the generation as
/// `keeper_session`) -- read off the colony's own `message_log` while it runs.
async fn until_store_answered(
    root: &std::path::Path,
    key: &str,
    value: &str,
    phase: &str,
    n: usize,
) {
    race::until_rows(
        &root.join("colony.db"),
        &format!(
            "SELECT rowid FROM message_log WHERE from_path = '{SESSIONS}' \
             AND to_path = '{STAMP}' \
             AND json_extract(headers, '$.context.ses_phase') = '{phase}' \
             AND json_extract(headers, '$.context.{key}') = '{value}'"
        ),
        n,
        &format!("the keeper's store answered `{phase}` on {key} {value}"),
    )
    .await;
}

/// GH #953, Re-Review R-1 (Fix-Runde 2): the Telegram road. A text turn there
/// reaches the keeper WITHOUT a channel turn id, and up to Fix-Runde 1 every
/// such turn owed its answer under the one mark `'*'` -- so the answer to an
/// EARLIER turn acknowledged the debt of a later one. A double message (two
/// id-less turns of one round, the second one still unanswered) followed by a
/// round change then sealed the generation as owing nothing and closed it at
/// once: the answer to the second turn was missing from the close -- #953
/// again, on the main text channel.
///
/// The order is forced by events, never by a lead: the brain holds the answer
/// to the first turn until the second turn's `touch` stands in the log, the
/// first answer is released and its acknowledgement (`ack`) is awaited while
/// the second answer is held, and only then does the person speak in round B.
/// Measured at the receiver -- the curator writer's `write` batch of the
/// sealed generation -- the answer to the LAST turn must be in it.
async fn a_double_message_without_ids_closes_after_the_answer_to_the_last_turn(
    runs: std::ops::Range<usize>,
) {
    if !road::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let brain = race::start_held_brain().await;
    let (td, h, mut ports) = race::boot_road_with(&brain.base_url).await;
    let root = td.path().to_path_buf();

    let mut sealed = Vec::new();
    for i in runs.clone() {
        let channel = format!("talky:953-idless-{i}");
        let a1 = format!("953-idless-{i}-a1");
        let a2 = format!("953-idless-{i}-a2");
        let b = format!("953-idless-{i}-b");
        // Two id-less turns of round A, the first one held at the brain.
        brain.hold(&a1);
        brain.hold(&a2);
        h.send(turn(&channel, &a1, ROUND_A)).await;
        brain.until_asked(&a1).await;
        h.send(turn(&channel, &a2, ROUND_A)).await;
        // The second turn is stamped into the running generation (its touch).
        until_store_answered(&root, "channel", &channel, "touch", 1).await;
        // The answer to the first turn goes out and is acknowledged while the
        // answer to the second one is held.
        brain.release(&a1);
        let first = race::text_of(&answer(&mut ports, "the answer to the first turn").await);
        assert!(first.contains(&reply(&a1)), "run {i}: {first}");
        brain.until_asked(&a2).await;
        let open = race::until_rows(
            &race::sessions_db(&root),
            &format!("SELECT session_id FROM sessions WHERE channel = '{channel}' AND closed = 0"),
            1,
            "the generation of round a is open",
        )
        .await;
        until_store_answered(&root, "keeper_session", &open[0][0], "ack", 1).await;
        let owed = race::rows(
            &race::sessions_db(&root),
            &format!(
                "SELECT COALESCE(owed_turn, '<null>') FROM sessions \
                 WHERE channel = '{channel}' AND closed = 0"
            ),
        )
        .first()
        .map_or_else(|| "<no row>".to_string(), |r| r[0].clone());
        // The person speaks in round B; the answer to the second turn is out.
        h.send(turn(&channel, &b, ROUND_B)).await;
        let gens = race::until_rows(
            &race::sessions_db(&root),
            &format!("SELECT session_id FROM sessions WHERE channel = '{channel}' AND closed = 1"),
            1,
            "the turn of round b seals the generation of round a",
        )
        .await;
        let sid = gens[0][0].clone();
        // The deterministic half of the lock: while the answer to the last
        // turn is held, the sealed generation must still OWE it -- a seal that
        // found the mark cleared (`seal-done`) has already sent its close,
        // and whether that close then misses the answer is only a race
        // (measured red: 10 of 10 runs sealed as owing nothing, 1 of 10
        // closes missed the answer).
        let sealed_owes = race::rows(
            &race::sessions_db(&root),
            &format!(
                "SELECT COALESCE(owed_turn, '<null>') FROM sessions WHERE session_id = '{sid}'"
            ),
        )
        .first()
        .map_or_else(|| "<no row>".to_string(), |r| r[0].clone());
        brain.release(&a2);
        let x = race::text_of(&answer(&mut ports, "the released second answer").await);
        let y = race::text_of(&answer(&mut ports, "the turn of round b").await);
        for (tag, what) in [(&a2, "the second turn"), (&b, "round b")] {
            assert!(
                x.contains(&reply(tag)) || y.contains(&reply(tag)),
                "run {i}: {what} was answered at the surface: {x} / {y}"
            );
        }
        close_reports(&mut ports, &root, std::slice::from_ref(&sid)).await;
        sealed.push((i, sid, a2, owed, sealed_owes));
    }
    let all: Vec<_> = runs
        .clone()
        .map(|i| (i, generations(&root, &format!("talky:953-idless-{i}"))))
        .collect();
    h.shutdown().await;

    let writes = race::writes(&root);
    let mut missing = Vec::new();
    let mut closed_at_once = Vec::new();
    for (i, sid, a2, owed, sealed_owes) in &sealed {
        let batch = writes
            .get(sid)
            .unwrap_or_else(|| panic!("run {i}: no write batch of {sid}"));
        assert_eq!(batch.len(), 1, "run {i}: one close batch of {sid}");
        let has = batch[0].body.contains(&reply(a2));
        eprintln!(
            "gh953 idless run {i}: owed after the first answer {owed:?}, owed at the seal \
             {sealed_owes:?}, answer to the last turn in the close: {has}"
        );
        if sealed_owes.is_empty() || sealed_owes == "<null>" {
            closed_at_once.push((*i, sid.clone(), owed.clone()));
        }
        if !has {
            missing.push((*i, sid.clone(), owed.clone()));
        }
    }
    eprintln!(
        "gh953 idless summary: {} of {} closes (runs {runs:?}) miss the answer to the last turn",
        missing.len(),
        runs.len()
    );
    assert!(
        closed_at_once.is_empty(),
        "the round change sealed a generation as owing nothing while the answer to its last \
         id-less turn was held -- its close left at once (run, session, owed after the first \
         answer): {closed_at_once:?}"
    );
    assert!(
        missing.is_empty(),
        "an earlier answer acknowledged the debt of the last id-less turn, and the round \
         change closed the generation before that turn's answer stood on the wall in {} of \
         {} runs ({runs:?}) (run, session, owed after the first answer): {missing:?}",
        missing.len(),
        runs.len()
    );
    for (i, gens) in &all {
        assert_eq!(
            gens.iter()
                .map(|g| (g.1.as_str(), g.2.as_str()))
                .collect::<Vec<_>>(),
            vec![(ROUND_A, "1"), (ROUND_B, "0")],
            "run {i}: one sealed generation of round a, one open of round b: {gens:?}"
        );
    }
}
