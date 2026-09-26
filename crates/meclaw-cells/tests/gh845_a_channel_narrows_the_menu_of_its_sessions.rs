//! GH #845 -- a channel narrows the tool menu of the sessions it carries, and the
//! menu itself stays whole.
//!
//! Measured before this lock (collector@4.3.0): the menu lived DURABLY in the
//! brain (`params.tools` -> route `menu` -> `system.tools` in its `cell.db`), and
//! nothing a turn carried could narrow it for one channel. A filter in front of
//! the brain would have left the narrowed menu standing for every other channel
//! of the same brain, and rewriting it per turn would break the provider's prefix
//! cache on every turn.
//!
//! Ruling R-SN-1: the scope is a fixed property of the CHANNEL, stamped by its
//! entry edge as `context.tools_allow` / `context.tools_deny`, constant per
//! session. The collector reads the two keys on `in_turn` and nowhere else,
//! keeps them in the `session` row of its own store, and sends them with every
//! brain call of the session as the body key `tool_scope` (the `llm` cell filters
//! that one request, L1). Advice and delegation never state one; the round they
//! open still carries the scope of the session it belongs to. A change inside a
//! session is taken over and SAID -- a stderr line and `hop.scope_changed`.
//!
//! What is pinned here, over the SHIPPED `script_inline` on stdin:
//!
//! 1. `in_turn` with `tools_deny` writes the session row and the brain call of
//!    the turn carries `tool_scope.deny`; the durable menu (route `menu`) is not
//!    written;
//! 2. a tool result's re-entry and a second turn WITHOUT the keys carry the same
//!    `tool_scope`, out of the session row;
//! 3. `in_advice` carrying the keys states nothing -- no session write, and with
//!    no session scope no `tool_scope`;
//! 4. a second `in_turn` with another scope: the new one wins, stderr says so,
//!    and the turn's first assembly carries `hop.scope_changed = "1"`;
//! 5. a session without a scope carries no `tool_scope` at all;
//! 6. the shipped wiring: the consult edge into the core deletes both keys, and
//!    the window store has the `session` table;
//! 7. a scope first stated in a RUNNING session is a change from `none`
//!    (fix round 1, I-2);
//! 8. an advice round of a scoped session carries the session's scope (m-7);
//! 9. on the shipped road from the member to the spoken collector only the two
//!    consult edges drop the keys (m-7);
//! 10. the session row outlives the prune: a session that goes on past an aged
//!     day close restates the same scope as no change -- no `none -> X` out of a
//!     row the prune took (fix round 2, m-3).

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const TURN: &str = "chan#0001";
const TURN2: &str = "chan#0002";

fn turn(tid: &str, ctx: Value, text: &str) -> Value {
    let mut c = json!({"turn_id": tid, "channel": "room-1"});
    for (k, v) in ctx.as_object().expect("ctx") {
        c[k] = v.clone();
    }
    lane(
        "in_turn",
        json!({"turn_id": tid}),
        c,
        json!([{"origin": "user", "type": "text", "text": text}]),
    )
}

/// The ops of the turn-open bundle that touch the `session` table.
fn session_ops(out: &[Value]) -> Vec<(String, Value)> {
    calls_of(in_phase(out, "turn-open"))
        .into_iter()
        .filter(|(_, a)| a["table"] == "session")
        .collect()
}

fn session_writes(out: &[Value]) -> Vec<(String, Value)> {
    session_ops(out)
        .into_iter()
        .filter(|(_, a)| a["operation"] != "select")
        .collect()
}

fn session_row(allow: Value, deny: Value) -> Value {
    json!({"session_id": SESSION, "tools_allow": allow, "tools_deny": deny})
}

/// The turn-open reply: the window, and the session rows as the store read them.
fn turn_open(tid: &str, now: Value, was: Option<Value>, text: &str) -> Value {
    let mut legs = vec![
        ("c-open-turn", Value::Null),
        ("c-open-round", json!([])),
        (
            "c-open-win",
            json!([{"id": "0001", "session_id": SESSION, "turn_id": tid,
                    "role": "user", "content": text, "deferred": 0,
                    "consult_id": ""}]),
        ),
    ];
    if let Some(w) = was {
        legs.push(("c-open-scope-was", w));
    }
    legs.push(("c-open-scope", now));
    legs.push(("c-open-roster", json!([])));
    bundle_reply("turn-open", tid, &legs)
}

/// The leg-window row a turn-open reply parks.
fn window_row(out: &[Value]) -> Value {
    let park = in_phase(out, "collect");
    calls_of(park)
        .into_iter()
        .filter(|(_, a)| a["operation"] == "insert")
        .map(|(_, a)| a["row"].clone())
        .find(|r| r["role"] == "leg-window")
        .unwrap_or_else(|| panic!("no leg-window parked: {out:?}"))
}

fn collect(tid: &str, window: Value) -> Vec<Value> {
    let mut w = window;
    w["fired"] = json!(0);
    assemble(
        &[],
        bundle_reply("collect", tid, &[("c-collect-read", json!([w]))]),
    )
}

fn brain(out: &[Value]) -> &Value {
    on_route(out, "brain")
}

fn no_menu_write(out: &[Value]) {
    assert!(
        out.iter()
            .all(|m| m["header"]["route"].as_str() != Some("menu")),
        "the durable menu is never rewritten by a scope: {out:?}"
    );
    for m in out {
        assert!(
            m["system"].get("tools").is_none(),
            "no `system.tools` write rides on a scoped turn: {m}"
        );
    }
}

/// A whole turn: in_turn, turn-open with the given session rows, collect.
fn round_of(tid: &str, ctx: Value, now: Value, was: Option<Value>) -> (Vec<Value>, Value) {
    let a = assemble(&[], turn(tid, ctx, "what is the weather"));
    let (b, _) = run_cell(
        ASSEMBLE,
        &[],
        turn_open(tid, now, was, "what is the weather"),
    );
    let win = window_row(&b);
    let c = collect(tid, win.clone());
    let mut all = a;
    all.extend(b);
    all.extend(c.clone());
    no_menu_write(&all);
    (c, win)
}

// ═══════════════════════════════════════════ 1. the turn states the scope

#[test]
fn a_turn_on_a_scoped_channel_carries_the_scope_to_the_brain() {
    let out = assemble(&[], turn(TURN, json!({"tools_deny": "x"}), "hello"));
    let writes = session_writes(&out);
    let put = writes
        .iter()
        .find(|(_, a)| a["operation"] == "insert")
        .unwrap_or_else(|| panic!("the stated scope is written to the session row: {out:?}"));
    assert_eq!(put.1["row"]["tools_deny"], json!(["x"]), "{put:?}");
    assert_eq!(put.1["row"]["tools_allow"], json!([]), "{put:?}");
    assert!(
        writes.iter().any(|(_, a)| a["operation"] == "delete"),
        "the old row is replaced, not doubled: {writes:?}"
    );
    no_menu_write(&out);

    let (c, _) = round_of(
        TURN,
        json!({"tools_deny": "x"}),
        json!([session_row(json!([]), json!(["x"]))]),
        Some(json!([])),
    );
    let b = brain(&c);
    assert_eq!(b["tool_scope"], json!({"deny": ["x"]}), "{b}");
    assert_eq!(
        hop_str(b, "scope_changed"),
        "0",
        "a first statement is not a change"
    );
}

#[test]
fn a_comma_string_and_an_array_read_the_same_in_their_order() {
    for v in [
        json!("b, a,b"),
        json!(["b", "a", "b"]),
        json!("[\"b\",\"a\"]"),
    ] {
        let out = assemble(&[], turn(TURN, json!({"tools_allow": v.clone()}), "hi"));
        let put = session_writes(&out)
            .into_iter()
            .find(|(_, a)| a["operation"] == "insert")
            .expect("insert");
        assert_eq!(
            put.1["row"]["tools_allow"],
            json!(["b", "a"]),
            "input order, repeats dropped: {v}"
        );
    }
}

// ════════════════════════════════ 2. the session row carries it everywhere

#[test]
fn a_tool_result_and_the_next_turn_carry_the_scope_of_the_session() {
    let (_, win) = round_of(
        TURN,
        json!({"tools_deny": ["x"]}),
        json!([session_row(json!([]), json!(["x"]))]),
        Some(json!([])),
    );

    // The tool result parks under the round and reads the slate back; the
    // re-entry is assembled from the ROUND rows, so the scope rides along
    // without a context key.
    let tool = assemble(
        &[],
        lane(
            "in_tool",
            json!({}),
            json!({"turn_id": TURN, "iter": "0"}),
            json!([{"origin": "tool", "type": "tool_result", "id": "call-1",
                    "text": "sunny"}]),
        ),
    );
    assert_eq!(tool.len(), 1, "{tool:?}");
    assert!(
        calls_of(&tool[0])
            .iter()
            .all(|(_, a)| a["table"] == "round"),
        "a tool result neither reads nor writes the session row: {tool:?}"
    );
    let call = json!([{"origin": "assistant", "type": "tool_call", "id": "call-1",
                       "text": "{\"name\":\"web_search\",\"arguments\":\"{}\"}"}]);
    let res = json!({"origin": "tool", "type": "tool_result", "id": "call-1",
                     "text": "sunny"});
    let mut w = win.clone();
    w["fired"] = json!(1);
    let rows = json!([
        w,
        {"turn_id": TURN, "iter": 0, "role": "assistant", "turn": call.to_string(),
         "fired": 0, "recorded_at": "2026-09-25T10:00:00.000000Z"},
        {"turn_id": TURN, "iter": 0, "role": "tool", "turn": res.to_string(),
         "fired": 0, "recorded_at": "2026-09-25T10:00:01.000000Z"}
    ]);
    let back = assemble(
        &[],
        bundle_reply("round-check", TURN, &[("c-round-check-read", rows)]),
    );
    let b = brain(&back);
    assert_eq!(b["tool_scope"], json!({"deny": ["x"]}), "{b}");
    assert_eq!(hop_str(b, "iter"), "1", "this is the re-entry: {b}");
    no_menu_write(&back);

    // A second turn of the same session whose channel stamps nothing states
    // nothing -- and still carries the session's scope.
    let second = assemble(&[], turn(TURN2, json!({}), "and tomorrow?"));
    assert!(
        session_writes(&second).is_empty(),
        "an unstated scope writes nothing: {second:?}"
    );
    assert!(
        session_ops(&second)
            .iter()
            .any(|(id, a)| id == "c-open-scope" && a["operation"] == "select"),
        "every opening reads the session row: {second:?}"
    );
    let (c, _) = round_of(
        TURN2,
        json!({}),
        json!([session_row(json!([]), json!(["x"]))]),
        None,
    );
    assert_eq!(brain(&c)["tool_scope"], json!({"deny": ["x"]}));
}

// ═══════════════════════════════════════ 3. advice states nothing

#[test]
fn an_advice_never_states_a_scope() {
    let a = assemble(
        &[],
        lane(
            "in_advice",
            json!({"turn_id": "cogny-round-1"}),
            json!({"consult_id": "k-1", "turn_id": "cogny-round-1",
                   "tools_deny": "x", "tools_allow": "y"}),
            json!([{"origin": "assistant", "type": "text", "text": "berlin: 21C"}]),
        ),
    );
    for m in &a {
        for (_, args) in calls_of_or_empty(m) {
            assert_ne!(args["table"], "session", "stage A: {m}");
        }
    }
    let (key, b) = open_advice("k-2", json!([]));
    assert!(
        session_writes(&b).is_empty(),
        "the round an advice opens reads the session row and never writes it: {b:?}"
    );
    let (reply, _) = run_cell(
        ASSEMBLE,
        &[],
        turn_open(&key, json!([]), None, "berlin: 21C"),
    );
    let c = collect(&key, window_row(&reply));
    assert!(
        brain(&c).get("tool_scope").is_none(),
        "no session scope, no tool_scope -- whatever the advice's context says: {c:?}"
    );
}

fn calls_of_or_empty(m: &Value) -> Vec<(String, Value)> {
    if m["header"]["route"].as_str() != Some("cstore") {
        return vec![];
    }
    calls_of(m)
}

// ═════════════════════════════════════════ 4. a change is taken and said

#[test]
fn a_changed_scope_is_taken_over_and_said() {
    let out = assemble(&[], turn(TURN2, json!({"tools_deny": "y"}), "again"));
    let ids: Vec<String> = session_ops(&out).into_iter().map(|(id, _)| id).collect();
    assert_eq!(
        ids,
        vec![
            "c-open-scope-was",
            "c-open-scope-del",
            "c-open-scope-put",
            "c-open-scope"
        ],
        "the old row is read BEFORE it is replaced, the new one after"
    );
    let (b, stderr) = run_cell(
        ASSEMBLE,
        &[],
        turn_open(
            TURN2,
            json!([session_row(json!([]), json!(["y"]))]),
            Some(json!([session_row(json!([]), json!(["x"]))])),
            "again",
        ),
    );
    assert!(
        stderr.contains("collector: tool scope of session s1 changed"),
        "the change is SAID: {stderr}"
    );
    assert!(
        stderr.contains("[\"x\"]") && stderr.contains("[\"y\"]"),
        "old and new: {stderr}"
    );
    let c = collect(TURN2, window_row(&b));
    let brain = brain(&c);
    assert_eq!(
        brain["tool_scope"],
        json!({"deny": ["y"]}),
        "the new scope wins"
    );
    assert_eq!(hop_str(brain, "scope_changed"), "1", "{brain}");

    // A repeat of the same scope is not a change.
    let (_, quiet) = run_cell(
        ASSEMBLE,
        &[],
        turn_open(
            TURN2,
            json!([session_row(json!([]), json!(["y"]))]),
            Some(json!([session_row(json!([]), json!(["y"]))])),
            "again",
        ),
    );
    assert!(!quiet.contains("tool scope"), "{quiet}");
}

// ════════════════════════════════════════ 5. no scope, no key

#[test]
fn a_session_without_a_scope_carries_no_tool_scope() {
    let (c, _) = round_of(TURN, json!({}), json!([]), None);
    let b = brain(&c);
    assert!(b.get("tool_scope").is_none(), "{b}");
    assert_eq!(hop_str(b, "scope_changed"), "0");
}

// ══════════════════════════════════════════ 6. the shipped wiring

#[test]
fn the_consult_edge_leaves_the_scope_behind_and_the_store_has_the_row() {
    let level = config_of(ASSISTANT);
    let edges = level["params"]["graph"]["edges"].as_array().expect("edges");
    let consults: Vec<&Value> = edges
        .iter()
        .filter(|e| {
            e["to"] == "./cogny"
                && e["condition"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("consult_cogny")
        })
        .collect();
    assert_eq!(consults.len(), 2, "talky and talky-chat consult the core");
    for e in consults {
        let del: Vec<&str> = e["modifier"]["delete_context"]
            .as_array()
            .expect("delete_context")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for k in ["tools_allow", "tools_deny"] {
            assert!(
                del.contains(&k),
                "the core keeps its full menu: {k} must not reach its in_turn ({e})"
            );
        }
    }
    let window = config_of("templates/collector/window/config.json");
    let t = &window["params"]["schema"]["session"];
    for col in ["session_id", "tools_allow", "tools_deny"] {
        assert!(t.get(col).is_some(), "session.{col}: {t}");
    }
}

// ═════ 7. a scope that appears in a running session is a change (rev-T2 I-2)
//
// Measured on `404e3b46`: a session with turns but no session row -- a channel
// that gains `tools_deny` by a deploy, a session begun before the collector
// kept scopes -- took the new scope over in silence, although the prompt cache
// breaks exactly there. R-SN-1 asks for the warning with its receipt.

#[test]
fn a_scope_first_stated_in_a_running_session_is_a_change() {
    let older = json!({"id": "0000", "session_id": SESSION, "turn_id": "chan#0000",
                       "role": "user", "content": "earlier", "deferred": 0,
                       "consult_id": ""});
    let own = json!({"id": "0001", "session_id": SESSION, "turn_id": TURN2,
                     "role": "user", "content": "again", "deferred": 0,
                     "consult_id": ""});
    let reply = bundle_reply(
        "turn-open",
        TURN2,
        &[
            ("c-open-turn", Value::Null),
            ("c-open-round", json!([])),
            ("c-open-win", json!([own, older])),
            ("c-open-scope-was", json!([])),
            (
                "c-open-scope",
                json!([session_row(json!([]), json!(["x"]))]),
            ),
            ("c-open-roster", json!([])),
        ],
    );
    let (b, stderr) = run_cell(ASSEMBLE, &[], reply);
    assert!(
        stderr
            .contains("collector: tool scope of session s1 changed (none -> {\"deny\": [\"x\"]})"),
        "a first statement in a running session is a change from none: {stderr}"
    );
    let c = collect(TURN2, window_row(&b));
    assert_eq!(hop_str(brain(&c), "scope_changed"), "1", "{c:?}");
    assert_eq!(brain(&c)["tool_scope"], json!({"deny": ["x"]}));

    // An empty statement where none stood is no change either.
    let quiet_reply = bundle_reply(
        "turn-open",
        TURN2,
        &[
            ("c-open-turn", Value::Null),
            ("c-open-round", json!([])),
            (
                "c-open-win",
                json!([{"id": "0001", "session_id": SESSION, "turn_id": TURN2,
                        "role": "user", "content": "again", "deferred": 0,
                        "consult_id": ""},
                       {"id": "0000", "session_id": SESSION, "turn_id": "chan#0000",
                        "role": "user", "content": "earlier", "deferred": 0,
                        "consult_id": ""}]),
            ),
            ("c-open-scope-was", json!([])),
            ("c-open-scope", json!([session_row(json!([]), json!([]))])),
            ("c-open-roster", json!([])),
        ],
    );
    let (_, quiet) = run_cell(ASSEMBLE, &[], quiet_reply);
    assert!(!quiet.contains("tool scope"), "{quiet}");
}

// ═══════ 8. an advice in a scoped session inherits the scope (rev-T2 m-7 b)

#[test]
fn an_advice_round_of_a_scoped_session_carries_the_sessions_scope() {
    let (key, b) = open_advice("k-3", json!([]));
    assert!(session_writes(&b).is_empty(), "{b:?}");
    let (reply, _) = run_cell(
        ASSEMBLE,
        &[],
        turn_open(
            &key,
            json!([session_row(json!([]), json!(["x"]))]),
            None,
            "berlin: 21C",
        ),
    );
    let c = collect(&key, window_row(&reply));
    let brain = brain(&c);
    assert_eq!(
        brain["tool_scope"],
        json!({"deny": ["x"]}),
        "R-SN-1: an insertion inherits the channel's scope: {brain}"
    );
    assert_eq!(hop_str(brain, "scope_changed"), "0");
}

// ═════ 9. nothing on the road from the channel to the collector drops it (m-7 a)

/// Every edge of the shipped levels a channel turn crosses on its way to the
/// spoken collector: the member, its firewall, the assistant, the talky. The
/// only edges that may delete the two keys are the consults into the core.
#[test]
fn only_the_consult_edges_on_the_road_drop_the_scope() {
    let mut droppers = vec![];
    for rel in [
        "templates/member/config.json",
        "templates/firewall/config.json",
        ASSISTANT,
        TALKY_REF,
        TALKY_CHAT_REF,
        "templates/talky/config.json",
    ] {
        let level = config_of(rel);
        let edges = level["params"]["graph"]["edges"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for e in edges {
            let m = &e["modifier"];
            let deletes = m["delete_context"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<_>>();
            let sets = m["set_context"]
                .as_object()
                .map(|o| o.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            for k in ["tools_allow", "tools_deny"] {
                if deletes.iter().any(|d| d == k) || sets.iter().any(|s| s == k) {
                    droppers.push(format!("{rel}: {} -> {}", e["from"], e["to"]));
                }
            }
        }
    }
    droppers.sort();
    droppers.dedup();
    assert_eq!(
        droppers,
        vec![
            format!("{ASSISTANT}: \"./talky\" -> \"./cogny\""),
            format!("{ASSISTANT}: \"./talky-chat\" -> \"./cogny\""),
        ],
        "a channel's scope reaches the spoken collector untouched; only the \
         consult into the core leaves it behind"
    );
}

// ═══ 10. the session row outlives the prune (rev-T2 fix round 1, m-3)
//
// Measured on `18dcaf62`: the prune chain deleted the session row with
// `recorded_at <= boundary` while window rows younger than the boundary stayed.
// The next turn restating the SAME scope then met no row and an older turn in
// the window -- the shape of a first statement in a running session -- and
// reported `none -> X`, a change that never happened. Without the stamp on the
// next turn the session would even have lost its scope in silence.

#[test]
fn a_session_past_its_prune_keeps_its_scope_and_reports_no_change() {
    let ledger = json!({
        "header": {"context": {"session_id": "", "turn_id": "",
                               "col_phase": "prune-ledger", "store_origin": "collector"},
                   "hop": {"operation": "select", "rows_affected": 1}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "c-prune-ledger",
                      "text": json!([{"session_id": SESSION,
                                      "batched_at": "2026-09-12T00:00:00.000000Z"}])
                                  .to_string()}]
    });
    let cut = assemble(&[], ledger);
    let tables: Vec<Value> = calls_of(in_phase(&cut, "prune-cut"))
        .into_iter()
        .map(|(_, a)| a["table"].clone())
        .collect();
    assert_eq!(
        tables,
        ["turns", "round"],
        "the scope is the channel's policy, not the window's: {cut:?}"
    );

    // The row is still there when the session goes on; a late row of an older
    // turn outlived the cut beside it. The same scope again is a repeat.
    let late = json!({"id": "0000", "session_id": SESSION, "turn_id": "chan#0000",
                      "role": "advice", "content": "late advice", "deferred": 0,
                      "consult_id": ""});
    let own = json!({"id": "0001", "session_id": SESSION, "turn_id": TURN2,
                     "role": "user", "content": "next day", "deferred": 0,
                     "consult_id": ""});
    let scope = json!([session_row(json!([]), json!(["x"]))]);
    let reply = bundle_reply(
        "turn-open",
        TURN2,
        &[
            ("c-open-turn", Value::Null),
            ("c-open-round", json!([])),
            ("c-open-win", json!([own, late])),
            ("c-open-scope-was", scope.clone()),
            ("c-open-scope", scope),
            ("c-open-roster", json!([])),
        ],
    );
    let (b, stderr) = run_cell(ASSEMBLE, &[], reply);
    assert!(!stderr.contains("tool scope"), "{stderr}");
    let c = collect(TURN2, window_row(&b));
    assert_eq!(hop_str(brain(&c), "scope_changed"), "0", "{c:?}");
    assert_eq!(brain(&c)["tool_scope"], json!({"deny": ["x"]}));
}
