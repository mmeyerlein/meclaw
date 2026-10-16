//! GH #1225 (A3) -- a ledger read the store refused is never an empty answer.
//!
//! WHY this file exists: `./policy` read every leg of a store answer as rows,
//! and a refused leg reads as none. A refused wall read sent the model a call
//! with no history and nothing said; a refused parked turn dropped the call
//! (silence in the conversation); a refused probe started a rebuild over an
//! empty plan or parked as "stale"; a refused claim parked as a lost race; a
//! refused tap overwrote the model's own limits with the fallback row's. Now
//! the window reads once more, and a second refusal sends the call with its
//! system part and its round, without what is missing, said on the hop
//! (`cuts`) and as one `store_error` mark; a rebuild stops with the mark and
//! frees its claim; a refused tap writes nothing it would derive from the
//! answer. Driven through the curator hive: `refuse_phase` refuses one
//! bundle, `sql_at_phase` takes a table away for every read until restored.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const S: &str = "s-a3";

fn hive() -> Hive {
    let mut h = Hive::with(&[("policy", "keep_recent", json!(1))]);
    h.ledger_may_refuse = true;
    h
}

/// The values of the `store_error` marks.
fn store_errors(h: &Hive) -> Vec<String> {
    h.rows("SELECT value FROM marks WHERE kind = 'store_error' ORDER BY seq")
        .into_iter()
        .map(|r| r[0].as_str().unwrap_or("").to_string())
        .collect()
}

fn has_step(h: &Hive, step: &str) -> bool {
    store_errors(h)
        .iter()
        .any(|v| v.contains(&format!("\"step\":\"{step}\"")))
}

fn cut_whats(call: &Msg) -> Vec<String> {
    call.hop
        .get("cuts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|c| c["what"].as_str().unwrap_or("").to_string())
        .collect()
}

fn says(call: &Msg, words: &str) -> bool {
    texts(call).iter().any(|t| t.contains(words))
}

/// Two turns of the round, the second ordering the strike of its cold cache.
fn ordered(h: &mut Hive) -> Msg {
    turn(
        h,
        S,
        "t-1",
        "cwA3 1: the parcel left.",
        "cwA3 1 reply.",
        json!({}),
    );
    turn(
        h,
        S,
        "t-2",
        "cwA3 2: the lease is signed.",
        "cwA3 2 reply.",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.clock_order_for(ROUND_E)
        .unwrap_or_else(|| panic!("no strike ordered: {:?}", h.clock))
}

#[test]
fn a_window_read_refused_once_is_read_again() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    turn(
        &mut h,
        S,
        "t-1",
        "cwA3 first words.",
        "cwA3 first reply.",
        json!({}),
    );
    h.refuse_phase = Some("win".to_string());
    let call = h.curate(
        S,
        "t-2",
        0,
        json!([user("cwA3 the second question")]),
        mode("Be brief."),
    );
    assert!(h.refuse_phase.is_none(), "the window read was refused");
    assert!(
        says(&call, "cwA3 the second question") && says(&call, "cwA3 first words."),
        "the call after a second read carries the window: {:?} {:?}",
        texts(&call),
        h.stderr
    );
    assert!(
        cut_whats(&call).iter().all(|w| w != "wall"),
        "{:?}",
        call.hop
    );
}

#[test]
fn a_wall_refused_twice_goes_without_it_and_says_so() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    turn(
        &mut h,
        S,
        "t-1",
        "cwA3 first words.",
        "cwA3 first reply.",
        json!({}),
    );
    h.sql_at_phase = Some((
        "win".to_string(),
        "ALTER TABLE wall RENAME TO wall_off".to_string(),
    ));
    let call = h.curate(
        S,
        "t-2",
        0,
        json!([user("cwA3 the second question")]),
        mode("Be brief."),
    );
    h.db.execute_batch("ALTER TABLE wall_off RENAME TO wall")
        .expect("the wall back");
    assert!(
        cut_whats(&call).iter().any(|w| w == "wall"),
        "a call without its wall says so on the hop: {:?} {:?}",
        call.hop,
        h.stderr
    );
    assert!(
        has_step(&h, "win"),
        "one store_error mark: {:?}",
        store_errors(&h)
    );
    assert!(
        call.body.get("system").is_some_and(Value::is_object),
        "the system part goes: {:?}",
        call.body
    );
}

#[test]
fn a_parked_turn_refused_once_on_its_way_back_from_handover_is_read_again() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    h.refuse_phase = Some("win-plan".to_string());
    let call = h.curate(
        S,
        "t-1",
        0,
        json!([user("cwA3 the very first words")]),
        mode("Be brief."),
    );
    assert!(
        h.refuse_phase.is_none(),
        "the first call went past ./handover"
    );
    assert!(
        says(&call, "cwA3 the very first words"),
        "{:?}",
        texts(&call)
    );
}

#[test]
fn a_refused_step_b_of_the_intake_is_said() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    turn(
        &mut h,
        S,
        "t-1",
        "cwA3 first words.",
        "cwA3 first reply.",
        json!({}),
    );
    h.refuse_phase = Some("win-hashes".to_string());
    h.curate(
        S,
        "t-2",
        0,
        json!([user("cwA3 the second question")]),
        mode("Be brief."),
    );
    assert!(h.refuse_phase.is_none(), "step B was refused");
    assert!(
        has_step(&h, "win-hashes"),
        "the refused writes of step B are a mark, not a stderr line alone: {:?} {:?}",
        store_errors(&h),
        h.stderr
    );
}

#[test]
fn a_refused_probe_starts_no_rebuild_and_says_so() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let order = ordered(&mut h);
    h.refuse_phase = Some("rb-probe".to_string());
    h.fire(&order);
    assert!(h.refuse_phase.is_none(), "the probe was refused");
    assert!(h.summ.is_empty(), "no rebuild from a refused probe");
    assert!(
        has_step(&h, "rb-probe"),
        "{:?} {:?}",
        store_errors(&h),
        h.stderr
    );
    assert_eq!(h.state_in("rebuild_running", ROUND_E), "");
    // The next trigger rebuilds.
    h.fire(&order);
    assert_eq!(h.summ.len(), 1, "{:?}", h.stderr);
}

#[test]
fn a_refused_claim_is_no_lost_race() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let order = ordered(&mut h);
    h.refuse_phase = Some("rb-claim".to_string());
    h.fire(&order);
    assert!(h.refuse_phase.is_none(), "the claim was refused");
    assert!(
        h.stderr
            .iter()
            .all(|l| !l.contains("another trigger claimed")),
        "a refusal is said as a refusal: {:?}",
        h.stderr
    );
    assert!(has_step(&h, "rb-claim"), "{:?}", store_errors(&h));
    assert_eq!(h.state_in("rebuild_running", ROUND_E), "");
    h.fire(&order);
    assert_eq!(h.summ.len(), 1, "the next trigger rebuilds: {:?}", h.stderr);
}

#[test]
fn a_refused_question_frees_the_claim_and_says_so() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let order = ordered(&mut h);
    h.refuse_phase = Some("rb-send".to_string());
    h.fire(&order);
    assert!(h.refuse_phase.is_none(), "the note was refused");
    assert!(h.summ.is_empty());
    assert_eq!(h.state_in("rebuild_running", ROUND_E), "", "{:?}", h.stderr);
    assert!(has_step(&h, "rb-send"), "{:?}", store_errors(&h));
}

#[test]
fn a_refused_answer_read_frees_the_claim() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let order = ordered(&mut h);
    h.fire(&order);
    assert_eq!(h.summ.len(), 1, "{:?}", h.stderr);
    h.refuse_phase = Some("sum".to_string());
    h.answer("cwA3 summary.", "stop");
    assert!(h.refuse_phase.is_none(), "the answer's bundle was refused");
    assert_eq!(
        h.state_in("rebuild_running", ROUND_E),
        "",
        "a refused read of the claim never holds it until it goes stale: {:?}",
        h.stderr
    );
    assert!(has_step(&h, "sum"), "{:?}", store_errors(&h));
}

#[test]
fn a_refused_tap_keeps_the_models_limits() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let package = json!({"input_soft": 200000, "input_hard": 400000,
                         "context_window": 400000, "tokens_prompt": 1000});
    turn(
        &mut h,
        S,
        "t-1",
        "cwA3 first words.",
        "cwA3 first reply.",
        package.clone(),
    );
    assert_eq!(h.state("input_hard"), "400000", "{:?}", h.stderr);
    let call = h.curate(
        S,
        "t-2",
        0,
        json!([user("cwA3 the second question")]),
        mode("Be brief."),
    );
    h.refuse_phase = Some("tap".to_string());
    h.tap(
        &call,
        "stop",
        json!({}),
        json!([said("cwA3 second reply.")]),
    );
    assert!(h.refuse_phase.is_none(), "the tap was refused");
    assert_eq!(
        h.state("input_hard"),
        "400000",
        "a refused tap is no answer without a package: {:?}",
        h.stderr
    );
    assert!(has_step(&h, "tap"), "{:?}", store_errors(&h));
}

/// The round's plan and leaf fell together (`drop_ops`), so its next call
/// lays the leaf again (phase `relay`, GH #949). A refused relay bundle read
/// as no parked turn and dropped the call; now it is read once more and the
/// call goes, without the leaf (fail-closed, the mark keeps it at most once).
#[test]
fn a_refused_relay_read_is_read_again() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "keep_recent", json!(1)),
    ]);
    h.ledger_may_refuse = true;
    let base = chrono::Utc::now() - chrono::Duration::seconds(50);
    for i in 0..4i64 {
        let at = base + chrono::Duration::milliseconds(100 * i);
        h.row(
            at,
            "s",
            &format!("r{i}"),
            "user",
            &user(&"z".repeat(1000)),
            0,
        );
    }
    let pkg = json!({"input_soft": 300, "input_hard": 100000, "cost_in": 10.0,
                     "cost_cached_in": 0.0, "cache_expires_at": "2099-01-01T00:00:00Z",
                     "tokens_prompt": 5000});
    turn(&mut h, "s", "t1", &"y".repeat(1200), "a", pkg);
    h.fire(&last_add(&h));
    assert_eq!(h.summ.len(), 1, "{:?}", h.stderr);
    h.answer("Earlier, briefly.", "stop");
    let slot = summary_slot(ROUND_E);
    h.db.execute(
        &format!(
            "DELETE FROM state WHERE key = 'window_plan:{}'",
            round_key(ROUND_E)
        ),
        [],
    )
    .unwrap();
    h.db.execute(&format!("DELETE FROM slots WHERE path = '{slot}'"), [])
        .unwrap();
    h.refuse_phase = Some("relay".to_string());
    let call = h.curate("s", "t2", 0, json!([user("cwA3 again")]), mode("Be brief."));
    assert!(h.refuse_phase.is_none(), "the relay bundle was refused");
    assert!(
        says(&call, "cwA3 again"),
        "{:?} {:?}",
        texts(&call),
        h.stderr
    );
}
