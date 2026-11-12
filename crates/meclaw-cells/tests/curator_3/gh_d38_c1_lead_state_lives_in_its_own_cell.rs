//! GH #1447 (D38 C1, R-38-4 Nr. 4) -- the lead's state lives in its own cell.
//!
//! WHY this file exists: the switch of the curator kept its memory in the
//! ledger beside the window -- `escalated:<session>` (the quick voice gave a
//! turn up, `win` 7039/7045), `consult_pending:<session>` (an errand runs,
//! 7677) and the speculation rows `spec:`/`spec_held:`/`spec_fb:<s>#<t>`
//! (claimed in `win-hashes` 6728). With the lead a cell of its own, that
//! state moves with it into the lead's own store (`./lead_state`): the
//! ledger holds the window and its record, the lead holds the switch. Each
//! case runs over two messages, so the second one can only decide right
//! from the row the first one left -- and the row is looked for where it
//! must stand and where it must not.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The lead's keys: none of them may ever reach the ledger again.
const LEAD_KEYS: [&str; 5] = [
    "escalated:",
    "consult_pending:",
    "spec:",
    "spec_held:",
    "spec_fb:",
];

fn hive(over: &[(&str, &str, Value)]) -> Hive {
    assert!(
        lead_shipped(),
        "templates/lead/lead/config.json is missing: the lead is not shipped (GH #1447, GH #1474)"
    );
    let mut all = Vec::<(&str, &str, Value)>::new();
    all.extend_from_slice(over);
    Hive::with(&all).reporting_composite("talky")
}

/// One curate of turn `turn_id`, iteration `iter`, stamped `path`/`needs`.
fn curate(
    h: &mut Hive,
    turn_id: &str,
    iter: u32,
    path: &str,
    needs: Value,
    round: Value,
    extra_hop: Value,
) {
    h.out.clear();
    let mut hop = json!({"session_id": "s1", "turn_id": turn_id, "iter": iter.to_string(),
                         "phase": "", "path": path, "needs": needs, "families": []});
    for (k, v) in extra_hop.as_object().cloned().unwrap_or_default() {
        hop[k] = v;
    }
    let ctx = json!({"session_id": "s1", "turn_id": turn_id, "iter": iter.to_string(),
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": round, "system": mode("Be brief.")}),
    );
}

fn one(h: &Hive, route: &str) -> Msg {
    let calls = h.routed(route);
    assert_eq!(
        calls.len(),
        1,
        "one `{route}` call: {:?} {:?}",
        h.out,
        h.stderr
    );
    calls[0].clone()
}

/// No ledger op of any cell names a key of the lead.
fn ledger_holds_no_lead_key(h: &Hive) {
    for (sender, args) in &h.ledger_ops {
        let text = args.to_string();
        for k in LEAD_KEYS {
            assert!(
                !text.contains(&format!("\"{k}")) && !text.contains(&format!("\"key\":\"{k}")),
                "{sender} put a key of the lead (`{k}`) into the ledger: {args}"
            );
        }
    }
    let rows = h.rows("SELECT key FROM state");
    for r in rows {
        let key = r[0].as_str().unwrap_or_default();
        assert!(
            !LEAD_KEYS.iter().any(|k| key.starts_with(k)),
            "the ledger holds the lead's row {key}"
        );
    }
}

#[test]
fn an_escalation_is_kept_by_the_lead_for_the_next_iteration() {
    if !shipped() {
        return;
    }
    let mut h = hive(&[]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));
    // Iteration 1: the quick voice called a tool and no result is in -- it
    // gives the turn up (`gap`), the brain speaks.
    curate(
        &mut h,
        "t1",
        1,
        "simple",
        json!(["tool"]),
        json!([user("weather?"), tool_call("c1", "weather")]),
        json!({}),
    );
    one(&h, "brain");
    assert_eq!(
        h.lead_state("escalated:s1"),
        "t1",
        "the lead keeps the escalation"
    );
    // Iteration 2: the result is in and speakable -- the quick voice would
    // speak it, but the turn was given up: only the lead's row says so.
    curate(
        &mut h,
        "t1",
        2,
        "simple",
        json!(["tool"]),
        json!([
            user("weather?"),
            tool_call("c1", "weather"),
            tool_result("c1", "sunny, 21 degrees")
        ]),
        json!({}),
    );
    one(&h, "brain");
    assert!(
        h.routed("quick").is_empty(),
        "no quick call after the escalation: {:?}",
        h.out
    );
    ledger_holds_no_lead_key(&h);
}

#[test]
fn a_pending_consult_is_kept_by_the_lead_across_turns() {
    if !shipped() {
        return;
    }
    let mut h = hive(&[]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));
    curate(
        &mut h,
        "t2",
        0,
        "complex",
        json!([]),
        json!([user("plan my week")]),
        json!({}),
    );
    let errand = one(&h, "consult");
    let cid = errand.hop["consult_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let mark: Value =
        meclaw_core::serde_json::from_str(&h.lead_state("consult_pending:s1")).unwrap_or_default();
    assert_eq!(
        mark["consult_id"],
        json!(cid),
        "the lead holds the errand's mark: {mark}"
    );
    // The next turn: the brain is told the consult runs and cannot consult.
    curate(
        &mut h,
        "t3",
        0,
        "talk",
        json!([]),
        json!([user("and tomorrow?")]),
        json!({}),
    );
    let brain = one(&h, "brain");
    let deny = brain
        .body
        .get("tool_scope")
        .and_then(|s| s.get("deny"))
        .cloned()
        .unwrap_or(json!([]));
    assert!(
        deny.as_array()
            .is_some_and(|d| d.contains(&json!("consult_cogny"))),
        "while the lead holds the mark, the consult tool is taken away: {deny}"
    );
    // The advice comes under the errand's id: the mark goes, in the lead.
    curate(
        &mut h,
        "t4",
        0,
        "talk",
        json!([]),
        json!([user("so?")]),
        json!({"consult_id": cid}),
    );
    one(&h, "brain");
    assert_eq!(
        h.lead_state("consult_pending:s1"),
        "",
        "the advice ends the lead's mark"
    );
    ledger_holds_no_lead_key(&h);
}

#[test]
fn a_speculation_row_is_claimed_in_the_lead() {
    if !shipped() {
        return;
    }
    let mut h = hive(&[("lead", "quick_speculative", json!("1"))]);
    turn(&mut h, "s1", "t0", "hello", "hi", json!({}));
    // The spec curate of t5 claims the turn's row first ...
    curate(
        &mut h,
        "t5",
        0,
        "",
        json!([]),
        json!([user("weather?")]),
        json!({"spec": "1"}),
    );
    let claim: Value =
        meclaw_core::serde_json::from_str(&h.lead_state("spec:s1#t5")).unwrap_or_default();
    assert_eq!(
        claim["state"],
        json!("fired"),
        "the spec curate claims in the lead: {claim}"
    );
    // ... and the turn's normal curate under its simple stamp finds it there.
    curate(
        &mut h,
        "t5",
        0,
        "simple",
        json!([]),
        json!([user("weather?")]),
        json!({}),
    );
    let row: Value =
        meclaw_core::serde_json::from_str(&h.lead_state("spec:s1#t5")).unwrap_or_default();
    assert!(
        row.get("state").is_some(),
        "the turn's speculation row stands in the lead after both curates: {row}"
    );
    ledger_holds_no_lead_key(&h);
}

/// The state line the summarizer is handed (`summary_state_cond`, GH #1440)
/// names a consult the lead still waits for, as it did while the row lived
/// in the ledger. WHY: the move of `consult_pending:<session>` took the
/// ledger read of the rebuild (`n-sstate`) with it -- the window's line got
/// the row from the lead's verdict, the summarizer's line lost "open: a
/// consult" without a word and no test saw it (`gh_d38_z3_state_cond_keeps_
/// the_listed_lines` sows a delegation only). The rebuild asks the lead now.
#[test]
fn the_summary_names_the_consult_the_lead_waits_for() {
    if !shipped() {
        return;
    }
    clear_of_midnight();
    // Without a role, as `gh_d38_z3_state_cond_keeps_the_listed_lines`: the
    // summary goes to `./summarizer` (a talky role summarizes itself).
    assert!(lead_shipped(), "the lead cell is shipped (GH #1447)");
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!("brief")),
        ("policy", "summary_state_cond", json!("1")),
        ("policy", "state_lines", json!(8)),
    ]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    let iso = |at: chrono::DateTime<chrono::Utc>| at.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string();
    h.lead_db
        .as_ref()
        .expect("the tree ships the lead")
        .execute(
            "INSERT INTO state (key, value) VALUES (?1, ?2)",
            rusqlite::params![
                "consult_pending:s1",
                canonical(&json!({"consult_id": "c-1", "at": iso(backdated(5)),
                                  "wait_ms": 120000,
                                  "until": iso(chrono::Utc::now() + chrono::Duration::hours(1))}))
            ],
        )
        .expect("a lead state row");
    turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    turn(
        &mut h,
        "s1",
        "t3",
        "q3",
        "a3",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    let req = h.answer("- person (2026-11-05) garden/state: In bloom.", "stop");
    let transcript = req.messages()[0]["text"]
        .as_str()
        .expect("a transcript")
        .to_string();
    let line = transcript
        .lines()
        .find(|l| l.starts_with("[state -- "))
        .unwrap_or_else(|| panic!("the state line rides behind the transcript: {transcript}"))
        .to_string();
    assert!(
        line.contains("open: a consult"),
        "the open consult in {line:?} (stderr: {:?})",
        h.stderr
    );
    ledger_holds_no_lead_key(&h);
}
