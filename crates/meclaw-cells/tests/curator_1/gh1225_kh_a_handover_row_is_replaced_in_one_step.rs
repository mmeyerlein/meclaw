//! GH #1225 (K-5/A5, EA8) -- every row `./handover` keeps is written in ONE step.
//!
//! WHY this file exists: `./handover` wrote each of its rows as a `delete`
//! and an `insert` in two legs of one bundle -- the summarizer's window
//! `summarizer_input_soft`, the parked note `pending:handover:<s>`, the leaf
//! slot `history.handover`, its binding `handover_audience`, the session served
//! last `handover_for` and `renewed_last:<s>`. A bundle is no transaction: an
//! insert the store refused left the key without any row, so the next session
//! started without a leaf, or `./policy` sent the same session to the handover
//! again and again. Blocks were checked by a read before they were written.
//! Now a row is one `insert` with `replace` on its key (`{path}` for the slot,
//! one row per path), a block one `insert` with `if_absent` on its hash (form
//! of `curator::gh1225_a_policy_row_is_replaced_in_one_step`). The behaviour:
//! every insert of these keys refused by a trigger, a close, a new session and
//! a renewal run -- every row keeps its old value, none goes missing.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

fn collapsed(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn the_handover_writes_its_rows_in_one_step() {
    if !shipped() {
        return;
    }
    let text = collapsed(&script_of("handover"));
    for gone in [
        r#"("p-sdel", {"operation": "delete""#,
        r#"("p-clear", {"operation": "delete""#,
        r#"("w-unaud", {"operation": "delete""#,
        r#"("w-unfor", {"operation": "delete""#,
        r#"("w-unren", {"operation": "delete""#,
        r#"check = ("x-" + h[:12], sel("blocks", ["hash"], {"hash": h}))"#,
    ] {
        assert!(
            !text.contains(&collapsed(gone)),
            "still a delete and an insert, or a check read: {gone}"
        );
    }
    for one_step in [
        r#"ins("state", {"key": key, "value": value}, replace={"key": key})"#,
        r#"ins("blocks", row, if_absent={"hash": row["hash"]})"#,
        r#"put("p-sput", "summarizer_input_soft", str(sum_soft))"#,
        r#"put("p-put", "pending:handover:" + session, payload)"#,
        r#"put("w-aud", "handover_audience", held)"#,
        r#"put("w-for", "handover_for", session)"#,
        r#"put("w-ren", "renewed_last:" + session,"#,
        r#"replace={"path": LEAF_PATH}"#,
    ] {
        assert!(
            text.contains(&collapsed(one_step)),
            "the one-step form is missing: {one_step}"
        );
    }
}

fn state_value(h: &Hive, key: &str) -> Option<String> {
    h.rows(&format!(
        "SELECT value FROM state WHERE key = '{}'",
        key.replace('\'', "''")
    ))
    .first()
    .map(|r| r[0].as_str().unwrap_or("").to_string())
}

fn leaf_slot(h: &Hive) -> Vec<Vec<Value>> {
    h.rows("SELECT hash, owner FROM slots WHERE path = 'history.handover' ORDER BY hash")
}

fn renew(h: &mut Hive, session: &str, call_id: &str, n: u32) {
    h.out.clear();
    h.lane(
        "in_renewed",
        json!({"session_id": session, "call_id": call_id, "channel": "test",
               "audience_set": ROUND_E}),
        json!({"call_id": call_id, "session_id": session, "renewal_n": n}),
        json!({"messages": []}),
    );
}

fn close(h: &mut Hive, session: &str) {
    h.summ.clear();
    h.lane(
        "in_close",
        json!({"session_id": session, "audience_set": ROUND_E}),
        json!({}),
        json!({"messages": []}),
    );
}

#[test]
fn no_handover_row_goes_missing_when_its_insert_is_refused() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    // A closed session with a note, the summarizer's window learnt from it.
    turn(
        &mut h,
        "s-a",
        "a1",
        "kh5 a: the boat is in the shed.",
        "kh5 a noted.",
        json!({}),
    );
    close(&mut h, "s-a");
    h.answer_with(
        "kh5 note: the boat is in the shed.",
        "stop",
        json!({"input_soft": 1000}),
    );
    // The next session starts with the leaf of that note.
    turn(
        &mut h,
        "s-b",
        "b1",
        "kh5 b: where were we?",
        "kh5 b noted.",
        json!({}),
    );
    renew(&mut h, "s-b", "call-b", 1);
    // A note of `s-b` parked and on its way (young).
    let parked = canonical(&json!({"text": "kh5 parked note of s-b", "model": "m",
        "parked_at": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()}));
    h.db.execute(
        "INSERT INTO state (key, value) VALUES ('pending:handover:s-b', ?1)",
        [&parked],
    )
    .expect("park a note");

    let keys = [
        "summarizer_input_soft",
        "pending:handover:s-b",
        "handover_audience",
        "handover_for",
        "renewed_last:s-b",
    ];
    let before: Vec<Option<String>> = keys.iter().map(|k| state_value(&h, k)).collect();
    for (k, v) in keys.iter().zip(&before) {
        assert!(
            v.as_deref().is_some_and(|v| !v.is_empty()),
            "{k} stands before: {v:?} {:?}",
            h.stderr
        );
    }
    let slot = leaf_slot(&h);
    assert_eq!(slot.len(), 1, "the leaf of s-b stands: {:?}", h.stderr);

    // From here on the store refuses every insert of these keys, and the
    // drop of the parked note (so it can be seen after its note step).
    h.ledger_may_refuse = true;
    h.db.execute_batch(
        "CREATE TRIGGER kh5_state BEFORE INSERT ON state \
           WHEN NEW.key IN ('summarizer_input_soft', 'handover_audience', 'handover_for') \
             OR NEW.key LIKE 'pending:handover:%' OR NEW.key LIKE 'renewed_last:%' \
           BEGIN SELECT RAISE(ABORT, 'refused by the test'); END; \
         CREATE TRIGGER kh5_slots BEFORE INSERT ON slots \
           WHEN NEW.path = 'history.handover' \
           BEGIN SELECT RAISE(ABORT, 'refused by the test'); END;",
    )
    .expect("the triggers");
    h.refuse_legs
        .push(("w-drop".to_string(), "store_error".to_string()));
    // A close of s-b with a new window and a new note, a new session, a
    // renewal: every one of them puts its rows again.
    close(&mut h, "s-b");
    h.answer_with(
        "kh5 second note of s-b.",
        "stop",
        json!({"input_soft": 2000}),
    );
    // A topic, so the new session has a leaf to put.
    h.mark(
        chrono::Utc::now(),
        "s-b",
        "b1",
        "topic",
        &canonical(&json!({"movement": "start", "name": "kh5 the shed"})),
    );
    turn(
        &mut h,
        "s-c",
        "c1",
        "kh5 c: hello again.",
        "kh5 c noted.",
        json!({}),
    );
    renew(&mut h, "s-b", "call-b", 2);

    let after: Vec<Option<String>> = keys.iter().map(|k| state_value(&h, k)).collect();
    for ((k, b), a) in keys.iter().zip(&before).zip(&after) {
        assert!(a.is_some(), "{k} went missing to a refused insert");
        assert_eq!(a, b, "{k} keeps its old value");
    }
    assert_eq!(
        leaf_slot(&h),
        slot,
        "the leaf slot keeps its old row: {:?}",
        h.stderr
    );
}
