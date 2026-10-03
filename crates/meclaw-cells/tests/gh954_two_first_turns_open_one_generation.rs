//! GH #954 -- two first turns of a new round open ONE generation.
//!
//! WHY this file exists: the session keeper opens a generation lazily -- the
//! first turn of a round that has no open generation on its channel inserts
//! one (`stamp`, phase `open`, `sid = channel-now`). Two turns of that round
//! that arrive together both look before either inserts, and up to #954 both
//! opened one: two open generations of one round on one channel. Measured
//! here on a real colony (the member road of GH #929), 20 times, each run on
//! a channel of its own:
//!
//! * a turn in round A (the generation the round change seals);
//! * two turns of round B sent together -- the race;
//! * a third turn of round B (the conversation goes on);
//! * a turn in round A again, which seals whatever round B opened.
//!
//! What round B's conversation must come to, read at the receivers: one
//! generation of round B on the keeper's store, ONE close batch for it at the
//! curator's writer, and in it all three turns of round B and their answers;
//! the third turn answered with both racing turns in its window. A second
//! generation of the same round splits the episode: two closes, the turns
//! divided between them, and the memory's close pass reading each half
//! without the other. Every run prints
//! `gh954 run <i>: generations of round b <n>, closes <c>, ...`.
//!
//! That the race RAN is measured too (review M-2): the loser of a race meets
//! the unique index `sessions_open_round`, and the keeper's store answers the
//! stamp's `open` with `unique_violation`. Without one such answer in the
//! `message_log`, the result above would say nothing -- two turns that never
//! met also open one generation.

#[path = "mock_openai.rs"]
mod mock_openai;
#[path = "support/round_race.rs"]
mod race;
#[path = "support/gh929_member_road.rs"]
mod road;

use race::{answer, close_reports, generations, reply, say, turn};

const RUNS: usize = 20;

/// The keeper's store and the cell whose `open` it answers.
const SESSIONS: &str = "/assistants/scribe/talky/session-keeper/sessions";
const STAMP: &str = "/assistants/scribe/talky/session-keeper/stamp";

const ROUND_A: &str = road::AUDIENCE;
const ROUND_B: &str = r#"["member:owner","agent:scribe","member:guest"]"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_first_turns_open_one_generation() {
    if !road::shipped() {
        eprintln!("a template of this road did not travel into this tree -- skipped (GH #49)");
        return;
    }
    let (td, h, mut ports, brain) = race::boot_road().await;
    let root = td.path().to_path_buf();

    let mut runs = Vec::new();
    for i in 0..RUNS {
        let channel = format!("talky:954-{i}");
        let tag = |t: &str| format!("954-{i}-{t}");
        h.send(turn(&channel, &tag("a"), ROUND_A)).await;
        answer(&mut ports, "the turn of round a").await;
        // The race: two first turns of round B, together.
        h.send(turn(&channel, &tag("x"), ROUND_B)).await;
        h.send(turn(&channel, &tag("y"), ROUND_B)).await;
        answer(&mut ports, "the first racing turn").await;
        answer(&mut ports, "the second racing turn").await;
        // The conversation goes on in round B.
        h.send(turn(&channel, &tag("z"), ROUND_B)).await;
        answer(&mut ports, "the third turn of round b").await;
        // Round A again: every open generation of round B is sealed.
        h.send(turn(&channel, &tag("w"), ROUND_A)).await;
        answer(&mut ports, "round a again").await;
        let b_rows = format!(
            "SELECT session_id FROM sessions WHERE channel = '{channel}' \
             AND audience_set = '{ROUND_B}'"
        );
        let opened = race::rows(&race::sessions_db(&root), &b_rows).len();
        race::until_rows(
            &race::sessions_db(&root),
            &format!("{b_rows} AND closed = 1"),
            opened.max(1),
            "round a seals every generation of round b",
        )
        .await;
        let gens = generations(&root, &channel);
        // The pass of round a's first generation (sealed by the race) and of
        // every generation of round b.
        let closing: Vec<String> = gens
            .iter()
            .enumerate()
            .filter(|(k, g)| g.1 == ROUND_B || (*k == 0 && g.1 == ROUND_A))
            .map(|(_, g)| g.0.clone())
            .collect();
        close_reports(&mut ports, &root, &closing).await;
        runs.push((i, gens));
    }
    let seen = brain.seen.lock().unwrap().clone();
    h.shutdown().await;

    // Review M-2: the race must have happened, or the lock below is green
    // for nothing. Asked over the 20 runs, not of each run: two turns sent
    // together race only when both look before either inserts, and a run
    // whose second turn looks after the first's insert is a valid order of
    // the same road, not a defect -- a per-run demand would make the lock
    // flaky on timing. Before #954 the race split round b in 20 of 20 runs,
    // so zero losers in 20 runs means the timing of the road changed and
    // this file no longer measures #954.
    let lost_opens: Vec<race::Logged> = race::logged(&root, SESSIONS, Some(STAMP))
        .into_iter()
        .filter(|l| l.hop["error_code"] == "unique_violation" && l.context["ses_phase"] == "open")
        .collect();
    for i in 0..RUNS {
        let channel = format!("talky:954-{i}");
        let n = lost_opens
            .iter()
            .filter(|l| l.context["channel"] == channel.as_str())
            .count();
        eprintln!("gh954 run {i}: opens that lost the race {n}");
    }
    assert!(
        !lost_opens.is_empty(),
        "no `open` of the stamp met `unique_violation` in {RUNS} runs -- the two turns \
         never raced, and the lock below proves nothing"
    );

    let writes = race::writes(&root);
    let mut bad = Vec::new();
    for (i, gens) in &runs {
        let tag = |t: &str| format!("954-{i}-{t}");
        let b: Vec<&String> = gens
            .iter()
            .filter(|g| g.1 == ROUND_B)
            .map(|g| &g.0)
            .collect();
        let batches: Vec<&race::Logged> = b
            .iter()
            .flat_map(|sid| writes.get(*sid).into_iter().flatten())
            .collect();
        // A turn is carried when its words stand in a close batch, and so
        // does its answer if the brain was ever asked for one under its tag
        // (two racing turns may be answered in one call that names the later).
        let carried = |t: &str| {
            let said = batches.iter().any(|w| w.body.contains(&say(&tag(t))));
            let answered = !seen.contains_key(&tag(t))
                || batches.iter().any(|w| w.body.contains(&reply(&tag(t))));
            said && answered
        };
        let lost: Vec<&str> = ["x", "y", "z"]
            .into_iter()
            .filter(|t| !carried(t))
            .collect();
        let asked: Vec<&str> = ["x", "y", "z"]
            .into_iter()
            .filter(|t| seen.contains_key(&tag(t)))
            .collect();
        eprintln!("gh954 run {i}: the brain was asked under {asked:?}");
        let window = seen.get(&tag("z")).cloned().unwrap_or_default();
        let blind: Vec<&str> = ["x", "y"]
            .into_iter()
            .filter(|t| !window.contains(&say(&tag(t))))
            .collect();
        eprintln!(
            "gh954 run {i}: generations of round b {}, closes {}, turns lost {lost:?}, \
             the third turn blind to {blind:?}",
            b.len(),
            batches.len()
        );
        if b.len() != 1 || batches.len() != 1 || !lost.is_empty() || !blind.is_empty() {
            bad.push((*i, b.len(), batches.len(), lost, blind));
        }
    }
    eprintln!(
        "gh954 summary: {} of {RUNS} runs split round b's conversation",
        bad.iter().filter(|r| r.1 > 1).count()
    );
    assert!(
        bad.is_empty(),
        "round b's conversation is not one generation with one close holding every turn \
         in {} of {RUNS} runs (run, generations, closes, lost, blind): {bad:?}",
        bad.len()
    );
}
