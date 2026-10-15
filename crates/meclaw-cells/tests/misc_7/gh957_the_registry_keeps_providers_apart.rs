//! GH #957 -- the llm registry keeps the two protocols apart.
//!
//! A catalogue row whose `wire_dialect` is `decisions` is a typed decisions
//! model; a subscriber says `"protocol": "decisions"` when it is a cell born
//! with `provider: "decisions"`. Pinned on the shipped `hand` and `select`
//! scripts, run as the code cell runs them (stdin document in, messages out):
//!
//! 1. A targeted override onto the decisions model reaches ONLY the decisions
//!    subscriber, and its push carries no `wire_dialect` (the cell would
//!    refuse one); the chat subscriber beside it stays where it is.
//! 2. A global override of the chat model onto the decisions model moves no
//!    chat cell, and a targeted override onto a chat model moves no decisions
//!    cell.
//! 3. `subscribe` takes `protocol`, an unknown one is refused by name, and a
//!    decisions cell may subscribe without a start model.
//! 4. `select` never ranks a decisions row for a chat request, and ranks it
//!    for a decisions request.
//!
//! That the push, once it ARRIVES, moves a decisions cell's next call is
//! pinned on the real cell in `gh957_a_decisions_cell_changes_its_model_by_overlay`.
//!
//! **R2b guard (GH #49 form).** Every read goes through [`shipped_registry`],
//! so a tree without the template skips instead of failing.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::code_wire::{run_shipped_script, shipped_script};

fn shipped_registry() -> Option<std::path::PathBuf> {
    let root =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/llm-registry");
    ["hand/config.json", "select/config.json"]
        .iter()
        .all(|rel| root.join(rel).exists())
        .then_some(root)
}

fn script(cell: &str) -> Option<String> {
    let root = shipped_registry()?;
    Some(shipped_script(
        root.join(cell)
            .join("config.json")
            .to_str()
            .expect("utf-8 path"),
    ))
}

fn run(script: &str, envelope: Value, body: Value) -> Vec<Value> {
    let doc = json!({"envelope": envelope, "body": body, "params": {}});
    let out = run_shipped_script(script, &doc.to_string());
    assert!(
        out.status.success(),
        "the script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    match meclaw_core::serde_json::from_slice(&out.stdout).expect("json out") {
        Value::Array(a) => a,
        other => vec![other],
    }
}

const CHAT_A: &str = "vendor/chat-a";
const CHAT_B: &str = "vendor/chat-b";
const DECIDER_A: &str = "vendor/decider-a";
const DECIDER_B: &str = "vendor/decider-b";
const CHAT_CELL: &str = "/g/assistant/brain";
const DECIDE_CELL: &str = "/g/presenter/decide";

fn models() -> Value {
    json!([
        {"model_id": CHAT_A, "status": "active", "wire_dialect": "chat_completions",
         "base_url": "", "cost_in": 10, "cost_out": 50, "context_window": 100000},
        {"model_id": CHAT_B, "status": "active", "wire_dialect": "chat_completions",
         "base_url": "", "cost_in": 20, "cost_out": 60, "context_window": 100000},
        {"model_id": DECIDER_A, "status": "active", "wire_dialect": "decisions",
         "base_url": "https://decide.example/api", "cost_in": 4, "cost_out": 0,
         "context_window": 32000},
        {"model_id": DECIDER_B, "status": "active", "wire_dialect": "decisions",
         "base_url": "https://decide.example/api", "cost_in": 4, "cost_out": 0,
         "context_window": 32000}
    ])
}

fn subscribers() -> Value {
    json!([
        {"cell_path": CHAT_CELL, "tier": "", "pinned": 0, "start_model": CHAT_A,
         "package_hash": "", "model_id": CHAT_A, "rank": "start", "reason": "start_value",
         "since": "t", "protocol": ""},
        {"cell_path": DECIDE_CELL, "tier": "", "pinned": 0, "start_model": DECIDER_A,
         "package_hash": "", "model_id": DECIDER_A, "rank": "start", "reason": "start_value",
         "since": "t", "protocol": "decisions"}
    ])
}

/// The store's echo of the `world` bundle of one op: models, tiers,
/// overrides, subscribers, then the op's own extra reads.
fn world(hand: &str, cmd: Value, extras: Vec<Value>) -> Vec<Value> {
    let mut rows = vec![models(), json!([]), json!([]), subscribers()];
    rows.extend(extras);
    let messages: Vec<Value> = rows
        .iter()
        .map(|r| json!({"origin": "tool", "type": "tool_result", "id": "x", "text": r.to_string()}))
        .collect();
    run(
        hand,
        json!({"header": {"hop": {"route": "rstore", "operation": "bundle"},
                          "context": {"registry_origin": "hand", "lr_phase": "world",
                                      "lr_carry": json!({"cmd": cmd}).to_string()}}}),
        json!({"messages": messages}),
    )
}

fn override_cmd(scope: &str, matching: &str, model: &str) -> Value {
    json!({"op": "override_set", "tier": "", "model_id": model, "reason": "",
           "scope": scope, "match": matching, "id": "", "cell_path": "", "start_model": "",
           "pinned": null, "announce": false, "entries": [], "actor": "operator",
           "call_id": "c1"})
}

/// `(subscriber, params)` of every push in the output.
fn pushes(out: &[Value]) -> Vec<(String, Value)> {
    out.iter()
        .filter(|m| m["header"]["route"] == "update")
        .map(|m| {
            (
                m["header"]["subscriber"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                m["params"].clone(),
            )
        })
        .collect()
}

/// The two extra reads of a targeted override: the path itself, the first
/// path below it -- here the presenter's cell, so the target is not empty.
fn target_extras() -> Vec<Value> {
    vec![json!([]), json!([{"cell_path": DECIDE_CELL}])]
}

#[test]
fn a_targeted_decisions_model_reaches_only_the_decisions_cell() {
    let Some(hand) = script("hand") else { return };
    let out = world(
        &hand,
        override_cmd("target", "/g", DECIDER_B),
        target_extras(),
    );
    let p = pushes(&out);
    assert_eq!(p.len(), 1, "one push, to the decisions cell only: {out:?}");
    assert_eq!(p[0].0, DECIDE_CELL);
    assert_eq!(p[0].1["model"], DECIDER_B);
    assert_eq!(p[0].1["base_url"], "https://decide.example/api");
    assert!(
        p[0].1.get("wire_dialect").is_none(),
        "a decisions cell is never sent a dialect: {}",
        p[0].1
    );
}

#[test]
fn a_targeted_chat_model_never_reaches_the_decisions_cell() {
    let Some(hand) = script("hand") else { return };
    let mut extras = target_extras();
    extras[1] = json!([{"cell_path": CHAT_CELL}]);
    let out = world(&hand, override_cmd("target", "/g", CHAT_B), extras);
    let p = pushes(&out);
    assert_eq!(p.len(), 1, "{out:?}");
    assert_eq!(p[0].0, CHAT_CELL);
    assert_eq!(p[0].1["model"], CHAT_B);
}

#[test]
fn a_global_swap_of_a_chat_model_onto_a_decisions_model_moves_no_chat_cell() {
    let Some(hand) = script("hand") else { return };
    let out = world(&hand, override_cmd("global", CHAT_A, DECIDER_B), vec![]);
    assert!(pushes(&out).is_empty(), "no cell moves: {out:?}");
}

fn command(hand: &str, cmd: Value) -> Vec<Value> {
    run(
        hand,
        json!({"header": {"hop": {"route": "in_hand"}, "context": {"actor": "operator"}}}),
        json!({"messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                             "text": cmd.to_string()}]}),
    )
}

#[test]
fn subscribe_takes_a_protocol_and_refuses_an_unknown_one() {
    let Some(hand) = script("hand") else { return };
    let out = command(
        &hand,
        json!({"op": "subscribe", "cell_path": DECIDE_CELL, "protocol": "decisions"}),
    );
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["header"]["phase"], "world", "{out:?}");
    let carry: Value =
        meclaw_core::serde_json::from_str(out[0]["header"]["carry"].as_str().unwrap_or("{}"))
            .unwrap();
    assert_eq!(
        carry["cmd"]["entries"],
        json!([{"cell_path": DECIDE_CELL, "start_model": "", "protocol": "decisions"}]),
        "a decisions cell subscribes without a start model"
    );

    let out = command(
        &hand,
        json!({"op": "subscribe", "cell_path": DECIDE_CELL, "start_model": DECIDER_A,
               "protocol": "telepathy"}),
    );
    let ack = out
        .iter()
        .find(|m| m["header"]["route"] == "ack")
        .expect("an ack");
    assert_eq!(ack["header"]["outcome"], "rejected");
    assert_eq!(ack["header"]["reason_code"], "unknown_protocol");
}

fn select_models(select: &str, req: Value) -> Value {
    let rows = models();
    let out = run(
        select,
        json!({"header": {"hop": {"route": "rstore", "operation": "select"},
                          "context": {"lr_phase": "models",
                                      "lr_carry": json!({"req": req, "tiers": {}}).to_string()}}}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "x",
                             "text": rows.to_string()}]}),
    );
    out.into_iter()
        .find(|m| m["header"]["route"] == "answer")
        .expect("an answer")
}

#[test]
fn select_ranks_a_decisions_row_only_for_a_decisions_request() {
    let Some(select) = script("select") else {
        return;
    };
    let chat = select_models(
        &select,
        json!({"tier": "", "caps": [], "max_cost": null, "min_context": null,
               "asker": "/a", "call_id": "c", "protocol": ""}),
    );
    assert_eq!(
        chat["header"]["model_id"], CHAT_A,
        "the cheaper decisions rows are never a chat answer: {chat}"
    );
    let decide = select_models(
        &select,
        json!({"tier": "", "caps": [], "max_cost": null, "min_context": null,
               "asker": "/a", "call_id": "c", "protocol": "decisions"}),
    );
    assert_eq!(decide["header"]["model_id"], DECIDER_A, "{decide}");
}
