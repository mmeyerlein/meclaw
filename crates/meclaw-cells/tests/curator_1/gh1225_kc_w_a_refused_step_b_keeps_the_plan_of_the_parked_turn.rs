//! GH #1225 (close KC, win-hashes) -- a step B the store refuses whole keeps
//! the plan of the parked turn.
//!
//! WHY this file exists: `./intake` parks a turn in step A without its plan
//! and writes the plan into it in step B (`b-slim`), in the same bundle as
//! the hash reads `./policy` takes the call on with (`win-hashes`). Refused
//! whole, the read again (`win-plan`) found the parked turn of step A: no
//! `plan`, so no kept rows and the cover of the newest summary instead of the
//! plan's -- a window other than the one the round's rebuild decided, said
//! only as the `store_error` of `b-slim`. Now `win-plan` reads the round's
//! plan beside the parked turn and writes it back into it; a plan it cannot
//! read either is a `store_error` naming the read, and the call shows its own
//! turn only, as `./intake` does for an unread plan (`wall_unread`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const S: &str = "s-kcw";
// The turn's own rows went with the refused step B (written by the next
// message, `gh1225_kr_a_refused_write_is_written_by_the_next_message`), so
// only the rows of earlier turns can show which window the call got.
const WORDS: &str = "My daughter Anna moves to Hamburg in May.";

fn marks(h: &Hive, kind: &str) -> Vec<String> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| r[0].as_str().unwrap_or("").to_string())
    .collect()
}

fn seqs(h: &Hive) -> Vec<i64> {
    h.rows(&format!(
        "SELECT seq FROM wall WHERE session_id = '{S}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| r[0].as_i64().unwrap_or(0))
    .collect()
}

/// Two turns; the round's plan keeps the rows of the first and covers both,
/// so the second (`Rex`) is in no window the plan cuts.
fn two_turns_and_a_plan(h: &mut Hive) {
    turn(h, S, "r1", "I grew up in Leipzig.", "Nice.", json!({}));
    let kept = seqs(h);
    turn(h, S, "r1b", "My dog is called Rex.", "Good dog.", json!({}));
    let cover = *seqs(h).last().expect("wall rows");
    let plan = json!({"v": 1, "cover": cover, "keep": kept});
    let key = format!("window_plan:{}", round_key(ROUND_E));
    h.db.execute("DELETE FROM state WHERE key = ?1", [&key])
        .expect("plan dropped");
    h.db.execute(
        "INSERT INTO state (key, value) VALUES (?1, ?2)",
        [&key, &plan.to_string()],
    )
    .expect("plan written");
}

fn step_b_refused_whole(h: &mut Hive) -> Msg {
    h.ledger_may_refuse = true;
    h.refuse_phase = Some("win-hashes".to_string());
    let call = h.curate(S, "r2", 0, json!([user(WORDS)]), mode("Be brief."));
    assert!(h.refuse_phase.is_none(), "step B was refused");
    call
}

fn says(call: &Msg, word: &str) -> bool {
    texts(call).iter().any(|t| t.contains(word))
}

#[test]
fn a_step_b_refused_whole_cuts_the_window_by_the_rounds_plan() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    two_turns_and_a_plan(&mut h);
    let call = step_b_refused_whole(&mut h);
    assert!(
        says(&call, "Leipzig"),
        "the plan's kept rows go: {:?} {:?}",
        texts(&call),
        h.stderr
    );
    assert!(
        !says(&call, "Rex"),
        "the plan's cover holds -- no wall from the newest summary: {:?} {:?}",
        texts(&call),
        h.stderr
    );
    assert!(
        marks(&h, "store_error")
            .iter()
            .any(|v| v.contains("\"step\":\"win-hashes\"") && v.contains("b-slim")),
        "the refused write of the parked turn is said: {:?}",
        marks(&h, "store_error")
    );
}

#[test]
fn a_plan_read_refused_after_a_refused_step_b_is_said_and_shows_the_turn_only() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    two_turns_and_a_plan(&mut h);
    h.refuse_legs = vec![("h-plan".to_string(), "store_error".to_string())];
    let call = step_b_refused_whole(&mut h);
    assert!(
        h.refuse_legs.is_empty(),
        "the read again reads the round's plan"
    );
    assert!(
        !says(&call, "Leipzig") && !says(&call, "Rex"),
        "an unread plan is never cover 0: {:?} {:?}",
        texts(&call),
        h.stderr
    );
    assert!(
        marks(&h, "store_error")
            .iter()
            .any(|v| v.contains("\"step\":\"win-plan\"") && v.contains("h-plan")),
        "the refused plan read is said: {:?}",
        marks(&h, "store_error")
    );
}
