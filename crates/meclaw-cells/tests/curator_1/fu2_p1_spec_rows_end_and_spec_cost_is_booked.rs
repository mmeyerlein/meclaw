//! #1279 -- the speculation leaves no rows behind and its money is
//! booked.
//!
//! WHY this file exists: with `quick_speculative` on, every turn left its
//! row `spec:<session>#<turn>` in the ledger for good -- the bare `normal`
//! claim of a turn without speculation too -- and a kept answer that settled
//! an `await` left the stored call's record behind although that call never
//! went. And a dropped spec call booked nothing: its answer was thrown away
//! with its usage, so the curator did not know what the turn cost (R-HK-14).
//! Now the rows go when their turn is decided (an event, never a timer), and
//! every spec answer is booked on arrival like any answer.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

fn spec_hive() -> Hive {
    let mut h = Hive::with(&[
        ("policy", "role", json!("talky")),
        ("policy", "quick_speculative", json!("1")),
        ("policy", "spec_tools", json!("")),
    ]);
    h.check_emits = true;
    h
}

fn spec_curate(h: &mut Hive, turn: &str, ask: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "", "spec": "1"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": ""});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(ask)]}));
}

fn spec_call(h: &mut Hive, turn: &str, ask: &str) -> Msg {
    spec_curate(h, turn, ask);
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "one spec call: {:?} {:?}", h.out, h.stderr);
    quick[0].clone()
}

fn seam(h: &mut Hive, turn: &str, path: &str, ask: &str) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "needs": "", "families": ""});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(ask)]}));
}

fn spec_answer(h: &mut Hive, call: &Msg, finish: &str, text: &str, cost: f64) {
    h.out.clear();
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert("spec".into(), json!("1"));
    let messages = if text.is_empty() {
        json!([])
    } else {
        json!([said(text)])
    };
    h.lane(
        "in_spec_llm",
        Value::Object(ctx),
        json!({"finish_reason": finish, "model": "spec-model", "tokens_prompt": 10,
               "tokens_completion": 5, "cost": cost}),
        json!({"messages": messages}),
    );
}

fn id_of(call: &Msg) -> String {
    call.hop["curator_call"].as_str().unwrap().to_string()
}

/// Every speculation row the ledger still holds.
fn spec_rows(h: &Hive) -> Vec<Vec<Value>> {
    h.rows(
        "SELECT key, value FROM state WHERE key LIKE 'spec:%' OR key LIKE 'spec_held:%' \
         OR key LIKE 'spec_fb:%'",
    )
}

fn number(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

#[test]
fn spec_rows_are_cleared_at_the_end() {
    if !shipped() {
        return;
    }
    // Kept on a quick turn.
    let mut h = spec_hive();
    let c = spec_call(&mut h, "t1", "hi");
    spec_answer(&mut h, &c, "stop", "SPECHI", 0.001);
    seam(&mut h, "t1", "simple", "hi");
    assert_eq!(
        h.routed("spec_release").len(),
        1,
        "{:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(spec_rows(&h).is_empty(), "kept: {:?}", spec_rows(&h));

    // Dropped on talk, the answer already in -- and the answer still on its way.
    let c = spec_call(&mut h, "t2", "how are you?");
    spec_answer(&mut h, &c, "stop", "SPECFINE", 0.001);
    seam(&mut h, "t2", "talk", "how are you?");
    assert_eq!(h.routed("brain").len(), 1);
    assert!(spec_rows(&h).is_empty(), "dropped: {:?}", spec_rows(&h));
    let c = spec_call(&mut h, "t3", "and you?");
    seam(&mut h, "t3", "talk", "and you?");
    spec_answer(&mut h, &c, "stop", "SPECLATE", 0.001);
    assert!(h.out.is_empty(), "a late answer goes nowhere: {:?}", h.out);
    assert!(spec_rows(&h).is_empty(), "late: {:?}", spec_rows(&h));

    // A failed spec answer before the stamp: its own row goes, the turn runs normally.
    let c = spec_call(&mut h, "t4", "hello");
    spec_answer(&mut h, &c, "error", "", 0.0);
    seam(&mut h, "t4", "simple", "hello");
    assert_eq!(h.routed("quick").len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(spec_rows(&h).is_empty(), "failed: {:?}", spec_rows(&h));

    // A turn without a spec curate: its `normal` claim goes with its call.
    seam(&mut h, "t5", "simple", "weather?");
    assert_eq!(h.routed("quick").len(), 1);
    assert!(spec_rows(&h).is_empty(), "normal: {:?}", spec_rows(&h));
    // A spec curate after that call makes no call and leaves nothing.
    spec_curate(&mut h, "t5", "weather?");
    assert!(
        h.routed("quick").is_empty() && h.routed("brain").is_empty(),
        "after the decision no call: {:?}",
        h.out
    );
    assert!(
        spec_rows(&h).is_empty(),
        "late spec curate: {:?}",
        spec_rows(&h)
    );

    // await -> keep: the stored call never went, and nothing of it stays.
    let c = spec_call(&mut h, "t6", "hey");
    let spec_id = id_of(&c);
    seam(&mut h, "t6", "simple", "hey");
    let fb = h.rows(&format!(
        "SELECT call_id FROM calls WHERE turn_id = 't6' AND call_id != '{spec_id}'"
    ));
    assert_eq!(fb.len(), 1, "the stored call's record: {fb:?}");
    let fb_id = fb[0][0].as_str().unwrap().to_string();
    spec_answer(&mut h, &c, "stop", "SPECHEY", 0.001);
    assert_eq!(
        h.routed("spec_release").len(),
        1,
        "{:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(spec_rows(&h).is_empty(), "await: {:?}", spec_rows(&h));
    for sql in [
        format!("SELECT call_id FROM calls WHERE call_id = '{fb_id}'"),
        format!("SELECT call_id FROM call_blocks WHERE call_id = '{fb_id}'"),
        format!("SELECT key FROM state WHERE key IN ('pending:{fb_id}-esc', 'window_est:{fb_id}')"),
    ] {
        assert!(h.rows(&sql).is_empty(), "left of the stored call: {sql}");
    }
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}

#[test]
fn dropped_spec_call_books_its_cost() {
    if !shipped() {
        return;
    }
    let mut h = spec_hive();
    let c = spec_call(&mut h, "t1", "how are you?");
    let spec_id = id_of(&c);
    spec_answer(&mut h, &c, "stop", "SPECFINE", 0.0021);
    seam(&mut h, "t1", "talk", "how are you?");
    assert!(h.routed("spec_release").is_empty(), "dropped");
    let row = h.rows(&format!(
        "SELECT model, tokens_prompt, tokens_completion, cost FROM calls WHERE call_id = '{spec_id}'"
    ));
    assert_eq!(row.len(), 1, "the spec call has its record");
    assert_eq!(
        row[0][0],
        json!("spec-model"),
        "booked like any call: {row:?}"
    );
    assert_eq!(row[0][1], json!(10));
    assert_eq!(row[0][2], json!(5));
    assert_eq!(number(&row[0][3]), Some(0.0021), "{row:?}");
    let marks = h.rows("SELECT value FROM marks WHERE kind = 'quick_spec'");
    assert_eq!(marks.len(), 1, "{marks:?}");
    let m: Value = sj::from_str(marks[0][0].as_str().unwrap()).unwrap();
    assert_eq!(m["verdict"], json!("dropped"));
    assert_eq!(
        number(&m["cost"]),
        Some(0.0021),
        "the mark carries the cost: {m}"
    );

    // An answer that arrives after its turn was decided is booked as well.
    let c = spec_call(&mut h, "t2", "and you?");
    let late_id = id_of(&c);
    seam(&mut h, "t2", "talk", "and you?");
    spec_answer(&mut h, &c, "stop", "SPECLATE", 0.0007);
    let row = h.rows(&format!(
        "SELECT model, cost FROM calls WHERE call_id = '{late_id}'"
    ));
    assert_eq!(row[0][0], json!("spec-model"), "{row:?}");
    assert_eq!(number(&row[0][1]), Some(0.0007), "{row:?}");
}
