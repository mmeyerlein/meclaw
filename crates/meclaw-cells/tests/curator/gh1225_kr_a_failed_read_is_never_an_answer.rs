//! GH #1225 (A3, intake) -- a read that failed is never an answer.
//!
//! WHY this file exists: `./intake` took a read leg that came back with an
//! error as an empty read. Under the rules `turn`/`turn+recall` (GH #1079) a
//! round that could not be read refuted every quote of the person
//! (`quote_not_in_source`), a wall that could not be read was written a second
//! time by a child that had lost its RAM, and a window plan or a cover that
//! could not be read became cover 0 -- the model's call over the whole wall.
//! Now a failed read of the round is `store_error`, a wall row is ONE step by
//! its own key whatever the read said, and a call without its plan shows its
//! own round only, marked `store_error` and parked with `wall_unread`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const RULE: &str = r#"{"items":"facts","quote":"turn"}"#;
const WORDS: &str = "My daughter Anna moves to Hamburg in May.";

fn section_in(h: &mut Hive, call: &Msg, payload: Value) {
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    h.out.clear();
    h.lane(
        "in_section",
        Value::Object(ctx),
        json!({"section": "memory", "turn_id": call.hop["turn_id"], "sidecar_verify": RULE}),
        json!({"messages": [], "section": "memory", "payload": payload}),
    );
}

fn reasons(h: &Hive) -> Vec<String> {
    h.routed("reject")
        .iter()
        .map(|m| m.hop["reject_reason"].as_str().unwrap_or("").to_string())
        .collect()
}

#[test]
fn a_failed_read_of_the_round_is_store_error() {
    if !shipped() {
        return;
    }
    // Step A cannot read the round's rows (`a-src`, a query timeout), or
    // step v cannot fetch their words (`v-blocks`).
    for leg in ["a-src", "v-blocks"] {
        let mut h = Hive::with(&[("intake", "pass_sections", json!("memory"))]);
        let call = h.curate("s-ea3", "r1", 0, json!([user(WORDS)]), mode("Be brief."));
        h.ledger_may_refuse = true;
        h.refuse_legs = vec![(leg.to_string(), "query_timeout".to_string())];
        section_in(
            &mut h,
            &call,
            json!({"facts": [
                {"predicate": "moves", "value": "Hamburg", "quote": "Anna moves to Hamburg in May"},
                {"predicate": "moves", "value": "May", "quote": "moves to Hamburg"},
            ]}),
        );
        assert!(h.refuse_legs.is_empty(), "{leg}: the read failed");
        assert_eq!(
            reasons(&h),
            ["store_error", "store_error"],
            "{leg}: never `quote_not_in_source`: {:?}",
            h.stderr
        );
    }
}

#[test]
fn a_failed_read_of_the_wall_writes_no_row_twice() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.curate("s-ea3w", "t1", 0, json!([user(WORDS)]), mode("Be brief."));
    // A new child, and its step A cannot read the wall.
    h.restart("intake");
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("a-wall".to_string(), "query_timeout".to_string())];
    h.curate(
        "s-ea3w",
        "t1",
        1,
        json!([
            user(WORDS),
            tool_call("c1", "lookup"),
            tool_result("c1", "{\"ok\": true}")
        ]),
        mode("Be brief."),
    );
    assert!(h.refuse_legs.is_empty(), "the read failed");
    let twice = h.rows(
        "SELECT turn_id, hash, nth, count(*) FROM wall GROUP BY session_id, turn_id, hash, nth \
         HAVING count(*) > 1",
    );
    assert!(
        twice.is_empty(),
        "no wall row twice: {twice:?} {:?}",
        h.stderr
    );
    assert_eq!(
        h.rows("SELECT 1 FROM wall WHERE kind = 'tool_result'")
            .len(),
        1,
        "the new rows stand"
    );
}

#[test]
fn a_failed_read_of_the_plan_never_shows_the_whole_wall() {
    if !shipped() {
        return;
    }
    for leg in ["a-plan", "a-cover"] {
        let mut h = Hive::new();
        turn(
            &mut h,
            "s-ea3p",
            "r1",
            "I grew up in Leipzig.",
            "Nice.",
            json!({}),
        );
        h.ledger_ops.clear();
        h.ledger_may_refuse = true;
        h.refuse_legs = vec![(leg.to_string(), "store_error".to_string())];
        let call = h.curate("s-ea3p", "r2", 0, json!([user(WORDS)]), mode("Be brief."));
        assert!(h.refuse_legs.is_empty(), "{leg}: the read failed");
        let wall_reads: Vec<&Value> = h
            .ledger_ops
            .iter()
            .filter(|(from, a)| {
                from == "intake"
                    && a["operation"] == "select"
                    && a["table"] == "wall"
                    && a["columns"] == json!(["hash"])
            })
            .map(|(_, a)| a)
            .collect();
        assert!(!wall_reads.is_empty(), "{leg}: the window's rows are read");
        for a in &wall_reads {
            assert_ne!(
                a["where"]["seq"],
                json!({"gt": 0}),
                "{leg}: never the wall from its first row: {a}"
            );
        }
        let marks = h.rows("SELECT value FROM marks WHERE kind = 'store_error'");
        assert!(
            marks.iter().any(|r| {
                meclaw_core::serde_json::from_str::<Value>(r[0].as_str().unwrap_or("")).ok()
                    == Some(json!({"step": "a", "legs": {leg: "store_error"}}))
            }),
            "{leg}: said with a mark: {marks:?} {:?}",
            h.stderr
        );
        assert!(
            texts(&call).iter().any(|t| t.contains("Hamburg")),
            "{leg}: the call carries its own round"
        );
    }
}
