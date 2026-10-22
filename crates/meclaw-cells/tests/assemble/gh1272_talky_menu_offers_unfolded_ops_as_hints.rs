//! GH #1272 (K3) -- talky's menu offers the unfolded op tools as hints.
//!
//! WHY this file exists: with the fold on (R-WM-2) a tool with an `op` of its
//! own (the tools hive's `file`, `edit`) stays single and went to the model
//! whole, its full schema in every request -- the hint level (name, one
//! line, `{op}`, the rest through `help`) was reachable only for a family:
//! `family_scope` dropped every other hint name, and `help` was answered only
//! for a folded family, so a `file {op: "help"}` went to the tools hive.
//!
//! Now the collector's `hint_own_op` ("1" on talky, "0" shipped) keeps every
//! unfolded op tool at hint level and names them on the seam
//! (`tools_hinted`); the composite's curate edge lifts the list into the
//! context, the dispatcher answers `{op: "help"}` of such a tool in place
//! (`help_for`), and the collector answers it from its menu store: the
//! tool's full schema. No call reaches the tools hive.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const TURN: &str = "chan#0001";
const DISPATCHER: &str = "templates/dispatcher/config.json";

fn tool(name: &str, description: &str, props: Value) -> Value {
    json!({"type": "function",
           "function": {"name": name, "description": description,
                        "parameters": {"type": "object", "properties": props}}})
}

/// The menu store: the tools hive's `file` with its own `op`, and a
/// two-member `web` family.
fn menu() -> Value {
    json!([{"answerer": "tools", "tools": [
        tool("file", "Work on the files of the workspace. Read, list or write.",
             json!({"op": {"type": "string", "enum": ["read", "list", "write"]},
                    "path": {"type": "string"}})),
        tool("web_search", "Search the web.", json!({"query": {"type": "string"}})),
        tool("web_fetch", "Fetch one page.", json!({"url": {"type": "string"}}))]}])
}

/// The seam of a turn on a collector with `over`: the turn opens, the
/// store's reply (window, no session scope, the menu) parks the window leg,
/// the collect step leaves on `curate`.
fn seam(over: &[(&str, Value)]) -> Value {
    let opened = run_cell(
        ASSEMBLE,
        over,
        lane(
            "in_turn",
            json!({"turn_id": TURN}),
            json!({"turn_id": TURN, "channel": "room-1"}),
            json!([{"origin": "user", "type": "text", "text": "hello"}]),
        ),
    )
    .0;
    assert_eq!(
        call(in_phase(&opened, "turn-open"), "c-open-menu")["table"],
        "menu"
    );
    let mut reply = bundle_reply(
        "turn-open",
        TURN,
        &[
            ("c-open-turn", Value::Null),
            ("c-open-round", json!([])),
            (
                "c-open-win",
                json!([{"id": "0001", "session_id": SESSION, "turn_id": TURN,
                        "role": "user", "content": "hello", "deferred": 0,
                        "consult_id": ""}]),
            ),
            ("c-open-scope", json!([])),
            ("c-open-roster", json!([])),
            ("c-open-menu", menu()),
        ],
    );
    reply["header"]["context"]["channel"] = json!("room-1");
    let (parked, stderr) = run_cell(ASSEMBLE, over, reply);
    let mut w = calls_of(in_phase(&parked, "collect"))
        .into_iter()
        .filter(|(_, a)| a["operation"] == "insert")
        .map(|(_, a)| a["row"].clone())
        .find(|r| r["role"] == "leg-window")
        .unwrap_or_else(|| panic!("no leg-window parked: {parked:?} {stderr}"));
    w["fired"] = json!(0);
    let out = assemble(
        &[],
        bundle_reply("collect", TURN, &[("c-collect-read", json!([w]))]),
    );
    on_route(&out, "curate").clone()
}

#[test]
fn talky_menu_offers_unfolded_ops_as_hints() {
    let on = [("fold_tools", json!("1")), ("hint_own_op", json!("1"))];
    let c = seam(&on);
    assert_eq!(
        c["tool_scope"],
        json!({"hint": ["file"]}),
        "the op tool goes at hint level, the family stays whole: {c}"
    );
    assert_eq!(hop_str(&c, "tools_hinted"), "file", "{c}");
    assert_eq!(hop_str(&c, "tools_folded"), "web", "{c}");

    // Shipped off: no hint, an empty list on the seam.
    let c = seam(&[("fold_tools", json!("1"))]);
    assert!(c.get("tool_scope").is_none(), "{c}");
    assert_eq!(hop_str(&c, "tools_hinted"), "", "{c}");
}

#[test]
fn help_of_a_hinted_op_tool_is_answered_from_the_menu_store() {
    // The dispatcher: `file {op: "help"}` under `tools_hinted` comes back at
    // once, `help_for` the tool -- no `tool` route, so no tools hive call.
    let doc = json!({
        "header": {"context": {"session_id": SESSION, "turn_id": "t1", "iter": "0",
                               "tools_folded": "web", "tools_hinted": "file"},
                   "hop": {"finish_reason": "tool_calls"}},
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "c1",
                      "text": json!({"name": "file", "arguments": "{\"op\":\"help\"}"})
                          .to_string()}]});
    let (out, _) = run_cell(DISPATCHER, &[], doc);
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "tool"),
        "no call reaches an answerer: {out:?}"
    );
    let res = on_route(&out, "result");
    assert_eq!(hop_str(res, "help_for"), "file", "{res}");
    assert_eq!(hop_str(res, "tool_call_id"), "c1", "{res}");

    // A real op of the same tool still leaves for its answerer.
    let doc = json!({
        "header": {"context": {"session_id": SESSION, "turn_id": "t1", "iter": "0",
                               "tools_hinted": "file"},
                   "hop": {"finish_reason": "tool_calls"}},
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "c2",
                      "text": json!({"name": "file",
                                     "arguments": "{\"op\":\"list\",\"path\":\".\"}"})
                          .to_string()}]});
    let (out, _) = run_cell(DISPATCHER, &[], doc);
    assert_eq!(hop_str(on_route(&out, "tool"), "tool_name"), "file");

    // The collector: the help request is parked and looked up in the menu
    // store; the answer is the tool's full schema under the call's id.
    let arrive = run_cell(
        ASSEMBLE,
        &[("fold_tools", json!("1")), ("hint_own_op", json!("1"))],
        json!({"header": {"hop": {"route": "in_tool", "help_for": "file",
                                  "tool_call_id": "c1"},
                          "context": {"session_id": SESSION, "turn_id": "t1", "iter": "0"}},
               "messages": [{"origin": "tool", "type": "tool_result", "id": "c1",
                             "text": "{}"}]}),
    )
    .0;
    let ask = in_phase(&arrive, "help");
    let row = calls_of(ask)
        .into_iter()
        .find(|(_, a)| a["operation"] == "insert")
        .map(|(_, a)| a["row"].clone())
        .expect("the request is parked");
    let reply = bundle_reply(
        "help",
        "t1",
        &[
            ("c-help-row", Value::Null),
            ("c-help-ask", json!([row])),
            ("c-help-calls", json!([])),
            ("c-help-menu", menu()),
        ],
    );
    let (done, stderr) = run_cell(
        ASSEMBLE,
        &[("fold_tools", json!("1")), ("hint_own_op", json!("1"))],
        reply,
    );
    let park = in_phase(&done, "round-check");
    let parked = calls_of(park)
        .into_iter()
        .find(|(_, a)| a["operation"] == "insert")
        .map(|(_, a)| a["row"].clone())
        .unwrap_or_else(|| panic!("no answer parked: {done:?} {stderr}"));
    let turn: Value = serde_json::from_str(parked["turn"].as_str().unwrap()).unwrap();
    assert_eq!(turn["id"], "c1");
    let text = turn["text"].as_str().unwrap_or_default();
    assert!(
        text.contains("Read, list or write.")
            && text.contains("\"path\"")
            && text.contains("\"enum\""),
        "the tool's full schema from the menu store: {text}"
    );
}

#[test]
fn talky_turns_the_hint_on_and_lifts_the_list() {
    let marker = config_of("templates/talky/collector/config.json");
    assert_eq!(
        marker["override_params"]["assemble"]["hint_own_op"], "1",
        "{marker}"
    );
    let cfg = config_of(ASSEMBLE);
    assert_eq!(cfg["params"]["hint_own_op"], "0", "shipped off");
    let talky = config_of("templates/talky/config.json");
    let edges = talky.to_string();
    assert!(
        edges.contains("\"tools_hinted\":\"has(hop.tools_hinted) ? hop.tools_hinted : ''\""),
        "the curate edge lifts the list into the context"
    );
}
