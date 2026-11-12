//! GH #1463 (D58 form) -- the plan keeps no orphan answer.
//!
//! WHY this file exists: the plan of a round is written ahead, right after
//! the answer, from the elements the call put out. The wall can hold another
//! element for the same answer: after a first sentence was already shown,
//! the answer that follows is only the continuation, and the wall books the
//! shown sentence in front of it. The block planned ahead then names a hash
//! no wall row has. The next call adds the block of the real row, the old
//! one is never looked at again and stays: an answer block with the source
//! `wall` that is never rendered and whose text the ledger does not hold. In
//! the measured windows one or two of them stood per long conversation, and
//! a reader of the plan counted tokens for blocks nobody was sent. A plan
//! that is the window must not list what the window cannot show.
//!
//! The setup is the shipped flow that replaces an answer on its way to the
//! wall (the fallback after a shown first sentence); the rule pinned is the
//! plan's and holds for every block of the wall.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Map, Value, json};

const TIMEOUT: &str = r#"{"error_code": "timeout", "error_source": "wire"}"#;
const ASK: &str = "how much rice do I need for four people";
const SHOWN: &str = "For four people you need about 300 grams of rice.";
const REST: &str = "Rinse it first.";

fn hive() -> Hive {
    Hive::with(&[
        ("policy", "recall_in_wall", json!("1")),
        ("lead", "quick_speculative", json!("1")),
        ("lead", "spec_tools", json!("web,weather")),
    ])
    .reporting_composite("talky")
}

fn early_ctx(call: &Msg) -> Map<String, Value> {
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert("spec".into(), json!("1"));
    ctx
}

/// The turn `t1` up to its answer: the early call shows [`SHOWN`] and then
/// fails, the stored call goes and answers with the continuation [`REST`]
/// alone. The wall books the shown sentence in front of it.
fn a_replaced_answer(h: &mut Hive) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                     "path": "", "spec": "1"});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": ""});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(ASK)]}));
    let early = h.routed("quick");
    assert_eq!(early.len(), 1, "the early call: {:?} {:?}", h.out, h.stderr);
    let early = early[0].clone();

    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
                     "path": "simple", "needs": "", "families": "web"});
    let ctx = json!({"session_id": "s1", "turn_id": "t1", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": "simple"});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(ASK)]}));

    h.out.clear();
    let hop = json!({"route": "partial", "seq": 1, "final": false,
                     "partial_of": "0190aaaa-0000-7000-8000-000000000001"});
    h.lane(
        "in_spec_partial",
        Value::Object(early_ctx(&early)),
        hop,
        json!({"messages": [said(SHOWN)]}),
    );

    h.out.clear();
    let mut hop = obj(json!({"finish_reason": "error", "model": "test-model",
                             "tokens_prompt": 10}));
    for (k, v) in obj(sj::from_str(TIMEOUT).expect("the error")) {
        hop.insert(k, v);
    }
    h.lane(
        "in_spec_llm",
        Value::Object(early_ctx(&early)),
        Value::Object(hop),
        json!({"messages": []}),
    );
    let stored = h.routed("quick");
    assert_eq!(
        stored.len(),
        1,
        "the stored call goes: {:?} {:?}",
        h.out,
        h.stderr
    );
    let stored = stored[0].clone();
    h.out.clear();
    h.tap(&stored, "stop", json!({}), json!([said(REST)]));
}

/// The hashes of the wall rows.
fn wall_hashes(h: &Hive) -> Vec<String> {
    h.rows("SELECT hash FROM wall")
        .into_iter()
        .map(|r| r[0].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The blocks of `blocks` with the source `wall` whose hash no wall row has.
fn orphans(h: &Hive, blocks: &[Value]) -> Vec<Value> {
    let wall = wall_hashes(h);
    blocks
        .iter()
        .filter(|b| {
            b["origin"]["source"] == "wall"
                && !wall.contains(&b["origin"]["hash"].as_str().unwrap_or_default().to_string())
        })
        .cloned()
        .collect()
}

/// Pin: after the next turn the plan lists the answer the wall holds and no
/// block of the wall without a wall row.
#[test]
fn the_plan_keeps_no_orphan_answer() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    a_replaced_answer(&mut h);
    let walled = said(&format!("{SHOWN} {REST}"));
    assert!(
        wall_hashes(&h).contains(&sha256_hex(&canonical(&walled))),
        "check of the setup: the wall holds the shown sentence in front of the answer: {:?}",
        h.stderr
    );
    assert!(
        !wall_hashes(&h).contains(&sha256_hex(&canonical(&said(REST)))),
        "check of the setup: the continuation alone is no wall row"
    );

    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t2", "iter": "0", "phase": ""});
    let ctx = json!({"session_id": "s1", "turn_id": "t2", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("And for six?")], "system": mode("Be brief.")}),
    );
    let (c2, _) = calls_out(&h);
    let text = sj::to_string(&c2.messages()).expect("messages");
    assert_eq!(
        text.matches(REST).count(),
        1,
        "check of the setup: the window shows the answer of the wall once: {text}"
    );
    h.tap(&c2, "stop", json!({}), json!([said("About 450 grams.")]));

    let blocks = plan_blocks(&h, ROUND_E);
    let ids = block_ids(&blocks);
    assert!(
        ids.contains(&block_id("answer", &walled)),
        "the plan lists the answer the wall holds: {ids:?}"
    );
    assert_eq!(
        orphans(&h, &blocks),
        Vec::<Value>::new(),
        "every block of the wall names a wall row: {ids:?}"
    );
    assert!(
        !ids.contains(&block_id("answer", &said(REST))),
        "the block planned ahead for the replaced answer left the plan: {ids:?}"
    );
}

/// Pin: the snapshot of the next call lists no orphan either -- its plan
/// half names the blocks of the call, and a block without a wall row is none.
#[test]
fn the_snapshot_of_the_next_call_lists_no_orphan() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    a_replaced_answer(&mut h);
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": "t2", "iter": "0", "phase": ""});
    let ctx = json!({"session_id": "s1", "turn_id": "t2", "iter": "0",
                     "channel": "test", "audience_set": ROUND_E});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("And for six?")], "system": mode("Be brief.")}),
    );
    let (c2, _) = calls_out(&h);
    let listed = snapshot_of(&h, &c2).expect("the snapshot row of the call");
    let ids = block_ids(&listed);
    assert!(
        ids.contains(&block_id("answer", &said(&format!("{SHOWN} {REST}")))),
        "check of the setup: the snapshot lists the answer of the wall: {ids:?}"
    );
    assert!(
        !ids.contains(&block_id("answer", &said(REST))),
        "no block for an answer the wall never held: {ids:?}"
    );
}
