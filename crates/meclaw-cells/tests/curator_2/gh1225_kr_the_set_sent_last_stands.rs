//! GH #1225 (A4) -- the pin set and the candidate sent last stand.
//!
//! WHY this file exists: `./intake` read the pins of a source in step A and
//! decided in step B which to end, which to insert and which to update; a
//! candidate the same way. Two replacements of one source in flight both
//! read the old set: the first set stayed alive beside the second, and a hash
//! new to both was inserted twice. The receipt of a candidate push left with
//! `stored` before anything was written. Now `replace_sources` is ONE bundle
//! without a read (every live pin of the source ends, the new set stands
//! again by `insert` with `replace` on (hash, source) under the unique index
//! `pins_round`), a new candidate is `insert` with `replace` on its key, and
//! the receipt leaves after the write, a refused write as `store_error`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

const SOURCE: &str = "probe-app";

fn pins_lane(texts: &[&str]) -> (&'static str, Value, Value, Value) {
    let pins: Vec<Value> = texts
        .iter()
        .map(|t| json!({"text": t, "source": SOURCE}))
        .collect();
    (
        "in_pin",
        json!({}),
        json!({}),
        json!({"pins": pins, "replace_sources": [SOURCE]}),
    )
}

/// The live pins of the source, by text.
fn live(h: &Hive) -> Vec<String> {
    let mut out: Vec<String> = h
        .rows(&format!(
            "SELECT b.body FROM pins p JOIN blocks b ON b.hash = p.hash \
             WHERE p.source = '{SOURCE}' AND (p.until IS NULL OR p.until = '')"
        ))
        .into_iter()
        .map(|r| {
            let body: Value =
                meclaw_core::serde_json::from_str(r[0].as_str().unwrap_or("{}")).unwrap();
            body["text"].as_str().unwrap_or("").to_string()
        })
        .collect();
    out.sort();
    out
}

#[test]
fn two_replacements_in_flight_leave_the_set_sent_last() {
    if !shipped() {
        return;
    }
    let sets: [&[&str]; 3] = [&["one", "two"], &["two", "three"], &["four"]];
    for rep in 0..50 {
        let mut h = Hive::new();
        let first = sets[rep % 3];
        let last = sets[(rep + 1) % 3];
        h.lane(
            "in_pin",
            json!({}),
            json!({}),
            json!({"pins": [
            {"text": "old", "source": SOURCE}]}),
        );
        h.lanes_together(vec![pins_lane(first), pins_lane(last)]);
        let mut want: Vec<String> = last.iter().map(|s| s.to_string()).collect();
        want.sort();
        assert_eq!(
            live(&h),
            want,
            "rep {rep}: the set sent last: {:?}",
            h.stderr
        );
        let twice = h.rows(
            "SELECT hash, source, audience_set FROM pins \
             GROUP BY hash, source, audience_set HAVING count(*) > 1",
        );
        assert!(twice.is_empty(), "rep {rep}: no pin row twice: {twice:?}");
    }
}

fn cand_lane(text: &str, hop: Value) -> (&'static str, Value, Value, Value) {
    (
        "in_candidate",
        json!({"audience_set": ROUND_E}),
        hop,
        json!({"source": SOURCE, "candidates": [{"id": "c1", "text": text}]}),
    )
}

#[test]
fn two_pushes_of_one_candidate_leave_the_text_sent_last() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.lanes_together(vec![
        cand_lane("the first text", json!({})),
        cand_lane("the second text", json!({})),
    ]);
    let rows = h.rows(&format!(
        "SELECT b.body FROM candidates c JOIN blocks b ON b.hash = c.hash \
         WHERE c.source = '{SOURCE}' AND c.cand_id = 'c1'"
    ));
    assert_eq!(rows.len(), 1, "{:?}", h.stderr);
    assert!(
        rows[0][0]
            .as_str()
            .unwrap_or("")
            .contains("the second text"),
        "{rows:?}"
    );
}

#[test]
fn a_receipt_never_says_stored_without_the_row() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("bc-i*".to_string(), "store_error".to_string())];
    h.out.clear();
    let (route, ctx, hop, body) = cand_lane("a text", json!({"ack": "1"}));
    h.lane(route, ctx, hop, body);
    assert!(h.refuse_legs.is_empty(), "the write was refused");
    let acks = h.routed("candidate_ack");
    assert_eq!(acks.len(), 1, "one receipt: {:?} {:?}", h.out, h.stderr);
    assert_eq!(acks[0].body["stored"], json!(0), "{:?}", acks[0].body);
    assert_eq!(
        acks[0].body["refused"],
        json!([{"id": "c1", "error_code": "store_error"}])
    );
    assert!(h.rows("SELECT 1 FROM candidates").is_empty());
    // Written, it says so.
    h.out.clear();
    let (route, ctx, hop, body) = cand_lane("a text", json!({"ack": "1"}));
    h.lane(route, ctx, hop, body);
    let acks = h.routed("candidate_ack");
    assert_eq!(acks.len(), 1);
    assert_eq!(acks[0].body["stored"], json!(1), "{:?}", acks[0].body);
    assert_eq!(h.rows("SELECT 1 FROM candidates").len(), 1);
}
