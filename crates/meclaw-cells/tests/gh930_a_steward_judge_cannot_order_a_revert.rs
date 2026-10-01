//! GH #930 — in the `steward` hive, only the meter and the probe order a revert.
//!
//! The mutator's revert path replays a plan the cycle stored before its change
//! was applied. The edges `./meter -> ./mutator` and `./probe -> ./mutator`
//! carry that order on the lane `revert`; the judge's edge carries no lane. The
//! path used to branch on `op: "revert"` in the text alone, whoever sent it, so
//! a judge answering with a revert order swapped a model or moved a param
//! without the step limit, the radius or the probe the decide path asks for.
//!
//! Pinned here at the script, table by table: a judge's revert order (flat, or
//! as the provider's function object a model actually sends) is refused as
//! `judge_cannot_revert` with its receipt as the only emission; the meter's
//! order on its lane still lands; and that lane carries revert orders and
//! nothing else. The same guard in the `argus` hive is pinned by
//! `gh927/mutator.rs`.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{emit_all, shipped_script};

const MUTATOR: &str = "../../templates/steward/mutator/config.json";

/// The cell a change is addressed to, and the plan a revert order carries.
const TARGET: &str = "/main/assistant/brain";

fn shipped() -> bool {
    if std::path::Path::new(MUTATOR).is_file() {
        return true;
    }
    eprintln!("steward did not travel into this tree -- skipped (GH #49)");
    false
}

fn a_revert_order(cycle_id: &str) -> Value {
    json!({
        "op": "revert",
        "cycle_id": cycle_id,
        "plan": {"target": TARGET, "kind": "model", "to": "vendor/other-model"}
    })
}

/// One tool-call turn carrying `args` as its text, on a hop and a context.
fn a_turn(args: &Value, hop: Value, context: Value) -> Value {
    json!({
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                      "text": args.to_string()}],
        "header": {"hop": hop, "context": context}
    })
}

fn routes(out: &[Value]) -> Vec<String> {
    out.iter()
        .map(|m| m["header"]["route"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The store operations among `out`, parsed.
fn store_ops(out: &[Value]) -> Vec<Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == "rstore")
        .filter_map(|m| m["messages"][0]["text"].as_str())
        .map(|t| meclaw_core::serde_json::from_str(t).expect("a store call is json"))
        .collect()
}

#[test]
fn a_revert_order_from_the_judge_moves_nothing() {
    if !shipped() {
        return;
    }
    let script = shipped_script(MUTATOR);
    let order = a_revert_order("cycle:1");
    // The flat form, and the provider's function object -- the only form a
    // model's answer ever reaches this cell in.
    let wrapped = json!({"name": "steward_change", "arguments": order.to_string()});
    for (form, args) in [("flat", &order), ("wrapped", &wrapped)] {
        let out = emit_all(
            &script,
            &a_turn(
                args,
                json!({"model": "judge/thinker", "finish_reason": "tool_calls"}),
                json!({"st_cycle": "cycle:9", "st_goal": "goal:x"}),
            ),
        );
        assert_eq!(
            routes(&out),
            vec!["rstore"],
            "{form}: the receipt is the only emission, no params update: {out:?}"
        );
        let ops = store_ops(&out);
        assert_eq!(ops.len(), 1, "{form}: {ops:?}");
        assert_eq!(ops[0]["operation"], "insert", "{form}: {ops:?}");
        let row = &ops[0]["row"];
        assert_eq!(row["status"], "closed", "{form}: {row}");
        assert_eq!(row["outcome"], "refused", "{form}: {row}");
        assert_eq!(row["reason_code"], "judge_cannot_revert", "{form}: {row}");
        assert_eq!(
            row["id"], "cycle:9",
            "{form}: the row is this cycle's, not the one the order names: {row}"
        );
    }
}

#[test]
fn a_revert_order_on_its_lane_still_lands() {
    if !shipped() {
        return;
    }
    let out = emit_all(
        &shipped_script(MUTATOR),
        &a_turn(
            &a_revert_order("cycle:1"),
            json!({"route": "revert", "cycle_id": "cycle:1"}),
            json!({"st_cycle": "cycle:1"}),
        ),
    );
    assert_eq!(routes(&out), vec!["mutate", "rstore"], "{out:?}");
    assert_eq!(out[0]["params"], json!({"model": "vendor/other-model"}));
    assert_eq!(out[0]["header"]["target"], TARGET);
    let ops = store_ops(&out);
    assert_eq!(ops[0]["set"]["outcome"], "reverted", "{ops:?}");
    assert_eq!(ops[0]["where"]["id"], "cycle:1", "{ops:?}");
}

#[test]
fn the_revert_lane_carries_revert_orders_and_nothing_else() {
    if !shipped() {
        return;
    }
    let decision = json!({
        "cycle_id": "cycle:2", "action": "change", "reasoning": "r", "simulated": {},
        "change": {"target": TARGET, "kind": "model", "from": "vendor/model",
                   "to": "vendor/other-model"},
        "revert_plan": {"target": TARGET, "kind": "model", "to": "vendor/model"}
    });
    let out = emit_all(
        &shipped_script(MUTATOR),
        &a_turn(
            &decision,
            json!({"route": "revert", "cycle_id": "cycle:2"}),
            json!({}),
        ),
    );
    assert!(
        out.is_empty(),
        "a decision on the revert lane is nobody's and writes nothing: {out:?}"
    );
}
