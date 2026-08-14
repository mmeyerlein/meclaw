//! meclaw-os 1 -- the collector hive assembles a context window (GitHub #27).
//!
//! The collector grew from an example pattern (a code cell that fans tool
//! results back in) into a hive class: the one place that decides what enters
//! an agent's context window. Three claims are pinned here, one per group:
//!
//! 1. ASSEMBLY -- a turn is written into the rolling window BEFORE the window
//!    is read, so the context a brain gets ends with the turn it is answering
//!    and begins with what was said before it. That is the whole "an agent
//!    knows only its current turn" gap, closed by a table instead of by luck.
//! 2. EVICTION -- what leaves the window is deterministic policy: a turn cap
//!    that runs in the store and a byte cap that runs here, both configuration,
//!    neither a model judgement. Whole turns leave, never halves, and the turn
//!    being answered is never the one evicted.
//! 3. THE SEAM -- everything the brain sees leaves through ONE message on ONE
//!    route. Window, memory bundle and tool round meet there and nowhere else,
//!    which is what lets a later agent split happen behind the seam.
//!
//! Everything runs the shipped `params.script_inline` against real stdin
//! documents, so nothing is mocked and nothing is spent.

use std::io::Write;
use std::process::{Command, Stdio};

const ASSEMBLE_CONFIG: &str = "../../builder/templates/collector/assemble/config.json";

/// `${VAR:-default}` becomes the default (or the override, when the case names
/// one), a bare `${VAR}` becomes the empty string -- the same substitution the
/// colony performs when it instantiates the template.
fn resolve_vars(script: &str, over: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        let inner = &tail[..end];
        let (name, default) = match inner.split_once(":-") {
            Some((n, d)) => (n, d),
            None => (inner, ""),
        };
        let value = over
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| *v)
            .unwrap_or(default);
        out.push_str(value);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn assemble_script(over: &[(&str, &str)]) -> String {
    let raw = std::fs::read_to_string(ASSEMBLE_CONFIG).expect("assemble config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
        over,
    )
}

/// Run the real script against a real stdin document and return the emitted
/// messages.
fn emit_with(over: &[(&str, &str)], doc: serde_json::Value) -> Vec<serde_json::Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(assemble_script(over))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(doc.to_string().as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "assemble exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    emit_with(&[], doc)
}

/// The store args of an emitted `cstore` message.
fn op_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op json")
}

/// A message as a port edge delivers it: the lane on the hop, the session in
/// context.
fn lane_doc(route: &str, messages: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0"},
                   "hop": {"route": route}},
        "messages": messages
    })
}

/// A store reply as the hive's own edge delivers it back: the step in context,
/// the operation and the guard signal on the hop.
fn reply_doc(
    phase: &str,
    op: &str,
    rows_affected: i64,
    payload: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0",
                               "col_phase": phase, "store_origin": "collector"},
                   "hop": {"operation": op, "rows_affected": rows_affected}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "x",
                      "text": payload.to_string()}]
    })
}

/// The same reply, but at a chosen iteration of the tool round.
fn reply_at(
    phase: &str,
    op: &str,
    rows_affected: i64,
    payload: serde_json::Value,
    iter: i64,
) -> serde_json::Value {
    let mut doc = reply_doc(phase, op, rows_affected, payload);
    doc["header"]["context"]["iter"] = serde_json::json!(iter.to_string());
    doc
}

fn turn_row(id: &str, role: &str, content: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "session_id": "s1", "turn_id": "t1", "role": role,
                       "content": content, "recorded_at": id})
}

/// A materialised `leg-window` row, as the `win` step writes it.
fn leg_window_row(turns: serde_json::Value, dropped: i64, capped: i64) -> serde_json::Value {
    let payload = serde_json::json!({"turns": turns, "bytes": 0,
                                     "dropped": dropped, "capped": capped});
    serde_json::json!({"turn_id": "t1", "iter": 0, "role": "leg-window",
                       "turn": payload.to_string(), "fired": 0})
}

fn texts_of(msg: &serde_json::Value) -> Vec<String> {
    msg["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| m["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

// ===================================================================== ASSEMBLY

#[test]
fn an_inbound_turn_is_written_before_the_window_is_read() {
    let out = emit(lane_doc(
        "in_turn",
        serde_json::json!([{"origin": "user", "type": "text", "text": "hello"}]),
    ));
    assert_eq!(out.len(), 1, "no memory leg configured: one emission only");
    assert_eq!(out[0]["header"]["route"], "cstore");
    assert_eq!(out[0]["header"]["phase"], "turn-w");
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "insert");
    assert_eq!(op["table"], "turns");
    assert_eq!(op["row"]["role"], "user");
    assert_eq!(op["row"]["content"], "hello");
    assert_eq!(op["row"]["session_id"], "s1");
    // The turn id is MINTED here and promoted by the edge -- the rest of the
    // turn correlates on it, so it must be on the hop of the very first hop.
    let minted = out[0]["header"]["turn_id"].as_str().expect("turn_id");
    assert!(!minted.is_empty(), "the fresh lane mints the turn id");
    assert_eq!(op["row"]["turn_id"], minted);
}

#[test]
fn the_turn_chain_asks_for_open_rounds_before_it_reads_the_window() {
    // GH #103: whether this turn may assemble depends on whether a tool round
    // of the session is still open -- an assistant row whose guard has not
    // fired. The question is asked BEFORE the window is read; the deliberately
    // evolved pin of the pre-#103 chain (turn-w went straight to the window).
    let out = emit(reply_doc("turn-w", "insert", 1, serde_json::json!("ok")));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["phase"], "turn-open");
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "round");
    assert_eq!(op["where"]["session_id"], "s1");
    assert_eq!(op["where"]["role"], "assistant");
    assert_eq!(
        op["where"]["fired"], 0,
        "open means: the guard has not fired"
    );
}

#[test]
fn the_window_read_carries_the_turn_cap_into_the_store() {
    // No open round: the chain continues exactly as before #103.
    let out = emit(reply_doc("turn-open", "select", 0, serde_json::json!([])));
    assert_eq!(out.len(), 1);
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "turns");
    assert!(
        op["columns"]
            .as_array()
            .expect("columns")
            .iter()
            .any(|c| c == "deferred"),
        "the window read carries the deferral stamp along"
    );
    assert_eq!(
        op["where"]["session_id"], "s1",
        "the window is session-scoped"
    );
    assert_eq!(op["order_by"][0]["col"], "id");
    assert_eq!(op["order_by"][0]["dir"], "desc");
    assert_eq!(op["limit"], 12, "COLLECTOR_WINDOW_TURNS default");
}

#[test]
fn the_window_leg_is_chronological_and_carries_both_roles() {
    // The store answers newest first (order by id desc); a conversation is read
    // oldest first, and BOTH roles are in it -- an assistant turn the agent
    // cannot see is how a conversation loses its own thread.
    let rows = serde_json::json!([
        turn_row("3", "user", "third"),
        turn_row("2", "assistant", "second"),
        turn_row("1", "user", "first")
    ]);
    let out = emit(reply_doc("win", "select", 3, rows));
    let op = op_of(&out[0]);
    assert_eq!(op["table"], "round");
    assert_eq!(op["row"]["role"], "leg-window");
    let payload: serde_json::Value =
        serde_json::from_str(op["row"]["turn"].as_str().expect("turn")).expect("payload");
    let turns = payload["turns"].as_array().expect("turns");
    assert_eq!(turns.len(), 3);
    assert_eq!(turns[0]["text"], "first");
    assert_eq!(turns[1]["text"], "second");
    assert_eq!(turns[1]["role"], "assistant");
    assert_eq!(turns[2]["text"], "third");
}

#[test]
fn the_gate_waits_for_every_declared_leg_and_only_for_those() {
    // Without a memory tier the window leg is the whole expectation, so a gate
    // that sees it fires. The counter-direction is the next test.
    let rows = serde_json::json!([leg_window_row(serde_json::json!([]), 0, 0)]);
    let out = emit(reply_doc("gate", "select", 1, rows));
    assert_eq!(out.len(), 1);
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "update");
    assert_eq!(op["set"]["fired"], 1);
    assert_eq!(op["where"]["role"], "leg-window");
    assert_eq!(op["where"]["fired"], 0, "the guard only ever wins once");
}

#[test]
fn a_configured_memory_leg_is_waited_for() {
    let over = [("COLLECTOR_MEMORY_TIER", "0")];
    let rows = serde_json::json!([leg_window_row(serde_json::json!([]), 0, 0)]);
    let out = emit_with(&over, reply_doc("gate", "select", 1, rows));
    assert!(
        out.is_empty(),
        "with the memory leg on, a window-only gate is incomplete and terminal"
    );
    let both = serde_json::json!([
        leg_window_row(serde_json::json!([]), 0, 0),
        {"turn_id": "t1", "iter": 0, "role": "leg-memory", "turn": "{}", "fired": 0}
    ]);
    let out = emit_with(&over, reply_doc("gate", "select", 2, both));
    assert_eq!(out.len(), 1, "both legs in: the gate fires");
}

#[test]
fn a_lost_guard_race_emits_nothing() {
    let out = emit(reply_doc(
        "fire-guard",
        "update",
        0,
        serde_json::json!("ok"),
    ));
    assert!(
        out.is_empty(),
        "rows_affected 0 means another emission owns the fire"
    );
    let out = emit(reply_doc(
        "fire-guard",
        "update",
        1,
        serde_json::json!("ok"),
    ));
    assert_eq!(out.len(), 1, "rows_affected 1 reads the slate back");
    assert_eq!(out[0]["header"]["phase"], "fire");
}

// ===================================================================== EVICTION

#[test]
fn the_byte_cap_drops_whole_turns_from_the_oldest_end() {
    let over = [("COLLECTOR_WINDOW_BYTES", "20")];
    // Four turns of ten characters each: the newest two fit in twenty bytes,
    // the third would be thirty and everything from there is dropped.
    let rows = serde_json::json!([
        turn_row("4", "user", "dddddddddd"),
        turn_row("3", "assistant", "cccccccccc"),
        turn_row("2", "user", "bbbbbbbbbb"),
        turn_row("1", "user", "aaaaaaaaaa")
    ]);
    let out = emit_with(&over, reply_doc("win", "select", 4, rows));
    let payload: serde_json::Value =
        serde_json::from_str(op_of(&out[0])["row"]["turn"].as_str().expect("turn"))
            .expect("payload");
    let turns = payload["turns"].as_array().expect("turns");
    assert_eq!(turns.len(), 2, "two whole turns fit");
    assert_eq!(
        turns[0]["text"], "cccccccccc",
        "the oldest SURVIVOR, not a half"
    );
    assert_eq!(
        turns[1]["text"], "dddddddddd",
        "the newest turn is still last"
    );
    assert_eq!(
        payload["dropped"], 2,
        "what left is counted, not silently gone"
    );
}

#[test]
fn the_turn_being_answered_is_never_the_one_evicted() {
    let over = [("COLLECTOR_WINDOW_BYTES", "5")];
    let rows = serde_json::json!([
        turn_row("2", "user", "a turn far larger than the whole byte cap"),
        turn_row("1", "user", "older")
    ]);
    let out = emit_with(&over, reply_doc("win", "select", 2, rows));
    let payload: serde_json::Value =
        serde_json::from_str(op_of(&out[0])["row"]["turn"].as_str().expect("turn"))
            .expect("payload");
    let turns = payload["turns"].as_array().expect("turns");
    assert_eq!(turns.len(), 1);
    assert_eq!(
        turns[0]["text"],
        "a turn far larger than the whole byte cap"
    );
    assert_eq!(payload["dropped"], 1);
}

#[test]
fn a_single_pathological_turn_cannot_eat_the_window() {
    let over = [
        ("COLLECTOR_TURN_CHARS", "8"),
        ("COLLECTOR_WINDOW_BYTES", "24"),
    ];
    let rows = serde_json::json!([
        turn_row("2", "user", "0123456789abcdef"),
        turn_row("1", "user", "short")
    ]);
    let out = emit_with(&over, reply_doc("win", "select", 2, rows));
    let payload: serde_json::Value =
        serde_json::from_str(op_of(&out[0])["row"]["turn"].as_str().expect("turn"))
            .expect("payload");
    let turns = payload["turns"].as_array().expect("turns");
    assert_eq!(
        turns.len(),
        2,
        "the truncated turn leaves room for its predecessor"
    );
    assert_eq!(
        turns[1]["text"], "01234567",
        "per-turn cap applied before the byte cap"
    );
}

#[test]
fn a_full_window_says_that_it_is_full() {
    let over = [("COLLECTOR_WINDOW_TURNS", "2")];
    // The store honoured the limit, so the reader cannot tell from the rows
    // alone whether older turns exist. The marker says it did cut.
    let rows = serde_json::json!([turn_row("2", "user", "b"), turn_row("1", "user", "a")]);
    let out = emit_with(&over, reply_doc("win", "select", 2, rows));
    let payload: serde_json::Value =
        serde_json::from_str(op_of(&out[0])["row"]["turn"].as_str().expect("turn"))
            .expect("payload");
    assert_eq!(payload["capped"], 1);
    assert_eq!(payload["dropped"], 0, "the turn cap is not a byte-cap drop");

    let rows = serde_json::json!([turn_row("1", "user", "a")]);
    let out = emit_with(&over, reply_doc("win", "select", 1, rows));
    let payload: serde_json::Value =
        serde_json::from_str(op_of(&out[0])["row"]["turn"].as_str().expect("turn"))
            .expect("payload");
    assert_eq!(payload["capped"], 0, "a window under the cap says so too");
}

#[test]
fn eviction_never_deletes() {
    // The whole read path of a turn, step by step: not one emission is a delete
    // or touches a row of `turns` other than by appending to it. The durable
    // record of a conversation belongs to the memory hive; this window is a cut.
    let rows = serde_json::json!([turn_row("2", "user", "b"), turn_row("1", "user", "a")]);
    let steps = vec![
        emit(lane_doc(
            "in_turn",
            serde_json::json!([{"origin": "user", "type": "text", "text": "hi"}]),
        )),
        emit(reply_doc("turn-w", "insert", 1, serde_json::json!("ok"))),
        emit_with(
            &[("COLLECTOR_WINDOW_TURNS", "1")],
            reply_doc("win", "select", 2, rows),
        ),
        emit(reply_doc("collect", "insert", 1, serde_json::json!("ok"))),
    ];
    for step in steps {
        for msg in step {
            if msg["header"]["route"] != "cstore" {
                continue;
            }
            let op = op_of(&msg);
            assert_ne!(
                op["operation"], "delete",
                "no step of the read path deletes"
            );
            if op["table"] == "turns" {
                assert!(
                    op["operation"] == "insert" || op["operation"] == "select",
                    "the turn table is append-only and read-only: {}",
                    op["operation"]
                );
            }
        }
    }
}

// ========================================================================= SEAM

#[test]
fn the_brain_is_handed_one_assembled_context_over_one_route() {
    let turns = serde_json::json!([
        {"role": "user", "text": "first"},
        {"role": "assistant", "text": "second"},
        {"role": "user", "text": "what did i say first?"}
    ]);
    let rows = serde_json::json!([leg_window_row(turns, 1, 1)]);
    let out = emit(reply_doc("fire", "select", 1, rows));
    assert_eq!(
        out.len(),
        1,
        "ONE seam: one message, one route, one brain edge"
    );
    let msg = &out[0];
    assert_eq!(msg["header"]["route"], "brain");
    assert_eq!(
        texts_of(msg),
        vec!["first", "second", "what did i say first?"]
    );
    assert_eq!(msg["messages"][1]["origin"], "assistant");
    // What the eviction policy did travels WITH the context, so a router or an
    // operator can see a cut without reading the store.
    assert_eq!(msg["header"]["window_turns"], "3");
    assert_eq!(msg["header"]["window_dropped"], "1");
    assert_eq!(msg["header"]["window_capped"], "1");
    assert_eq!(msg["header"]["iter"], "0", "the first call of the turn");
}

#[test]
fn the_memory_bundle_enters_through_the_collector_and_verbatim() {
    let over = [("COLLECTOR_MEMORY_TIER", "0")];
    // A turn with the memory leg on asks exactly once, next to the write.
    let out = emit_with(
        &over,
        lane_doc(
            "in_turn",
            serde_json::json!([{"origin": "user", "type": "text", "text": "what do you know?"}]),
        ),
    );
    assert_eq!(out.len(), 2);
    let ask = out
        .iter()
        .find(|m| m["header"]["route"] == "recall")
        .expect("a recall request per turn");
    assert_eq!(ask["header"]["memory_tier"], "0");
    assert_eq!(ask["header"]["recall_query"], "what do you know?");
    assert_eq!(
        ask["header"]["turn_id"], out[0]["header"]["turn_id"],
        "the bundle has to find its way back to THIS turn"
    );

    // The bundle arrives on its own lane and becomes a leg.
    let bundle = serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0"},
                   "hop": {"route": "in_bundle"}},
        "system": {"memory": {"bundle": {"text": "{\"beliefs\":[]}"}}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                      "text": "MEMORY (tier 0)\n- belief: the editor is helix"}]
    });
    let out = emit_with(&over, bundle);
    let op = op_of(&out[0]);
    assert_eq!(op["row"]["role"], "leg-memory");

    // And it reaches the brain in the system slot, not in the conversation.
    let leg_memory = serde_json::json!({
        "turn_id": "t1", "iter": 0, "role": "leg-memory",
        "turn": op["row"]["turn"], "fired": 0
    });
    let rows = serde_json::json!([
        leg_window_row(
            serde_json::json!([{"role": "user", "text": "and my editor?"}]),
            0,
            0
        ),
        leg_memory
    ]);
    let out = emit_with(&over, reply_doc("fire", "select", 2, rows.clone()));
    assert_eq!(out.len(), 1);
    let msg = &out[0];
    assert_eq!(
        msg["system"]["memory"]["recall"]["text"], "MEMORY (tier 0)\n- belief: the editor is helix",
        "the readable form the memory hive rendered, byte for byte"
    );
    assert_eq!(
        texts_of(msg),
        vec!["and my editor?"],
        "a tool_result without its tool_call has no business in a chat thread"
    );
    // The machine-readable half is a configuration choice, not a second render.
    let out = emit_with(
        &[
            ("COLLECTOR_MEMORY_TIER", "0"),
            ("COLLECTOR_MEMORY_FORM", "json"),
        ],
        reply_doc("fire", "select", 2, rows),
    );
    assert_eq!(
        out[0]["system"]["memory"]["bundle"]["text"],
        "{\"beliefs\":[]}"
    );
    assert!(out[0]["system"]["memory"]["recall"].is_null());
}

#[test]
fn the_tool_round_fires_once_and_re_enters_through_the_same_seam() {
    let calls = serde_json::json!([
        {"origin": "assistant", "type": "tool_call", "id": "c1", "text": "{}"},
        {"origin": "assistant", "type": "tool_call", "id": "c2", "text": "{}"}
    ]);
    let out = emit(lane_doc("in_calls", calls.clone()));
    assert_eq!(op_of(&out[0])["row"]["role"], "assistant");
    let out = emit(lane_doc(
        "in_tool",
        serde_json::json!([{"origin": "tool", "type": "tool_result", "id": "c1", "text": "a"}]),
    ));
    assert_eq!(op_of(&out[0])["row"]["role"], "tool");

    let asst = serde_json::json!({"turn_id": "t1", "iter": 0, "role": "assistant",
                                  "turn": calls.to_string(), "fired": 0});
    let res1 = serde_json::json!({"turn_id": "t1", "iter": 0, "role": "tool",
        "turn": "{\"origin\":\"tool\",\"type\":\"tool_result\",\"id\":\"c1\",\"text\":\"a\"}",
        "fired": 0});
    let res2 = serde_json::json!({"turn_id": "t1", "iter": 0, "role": "tool",
        "turn": "{\"origin\":\"tool\",\"type\":\"tool_result\",\"id\":\"c2\",\"text\":\"b\"}",
        "fired": 0});

    // One of two answers back: the round is incomplete and terminal.
    let partial = serde_json::json!([asst.clone(), res1.clone()]);
    assert!(
        emit(reply_doc("round-check", "select", 2, partial)).is_empty(),
        "a round with an open call does not re-enter the brain"
    );

    let full = serde_json::json!([asst.clone(), res1.clone(), res2.clone()]);
    let out = emit(reply_doc("round-check", "select", 3, full.clone()));
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "update");
    assert_eq!(op["where"]["role"], "assistant");
    assert_eq!(
        op["where"]["iter"], 0,
        "the guard is per ITERATION, not per turn"
    );

    // Re-entry: the window travels with the tool round, through the same seam.
    let mut rows = full.as_array().expect("rows").clone();
    rows.push(leg_window_row(
        serde_json::json!([{"role": "user", "text": "the question"}]),
        0,
        0,
    ));
    let out = emit(reply_doc(
        "round-fire",
        "select",
        4,
        serde_json::Value::Array(rows),
    ));
    assert_eq!(out.len(), 1);
    let msg = &out[0];
    assert_eq!(
        msg["header"]["route"], "brain",
        "the same route as the first call"
    );
    assert_eq!(msg["header"]["iter"], "1", "the loop counter moved");
    let texts = texts_of(msg);
    assert_eq!(
        texts.len(),
        5,
        "window turn + assistant turn (2 calls) + 2 results: {texts:?}"
    );
    assert_eq!(texts[0], "the question", "the conversation still leads");
    assert_eq!(msg["messages"][1]["type"], "tool_call");
    assert_eq!(msg["messages"][3]["type"], "tool_result");
    assert_eq!(
        msg["messages"][3]["id"], "c1",
        "PLAIN order: asked, then answered"
    );
}

// ======================================================== CAPS AT THE SEAM (#91)

/// One assistant `tool_call` row and one `tool_result` row of the same
/// iteration, as the round table holds them.
fn round_pair(iter: i64, id: &str, result: &str) -> Vec<serde_json::Value> {
    let call = serde_json::json!([
        {"origin": "assistant", "type": "tool_call", "id": id, "text": "{}"}
    ]);
    let res = serde_json::json!(
        {"origin": "tool", "type": "tool_result", "id": id, "text": result}
    );
    vec![
        serde_json::json!({"turn_id": "t1", "iter": iter, "role": "assistant",
                           "turn": call.to_string(), "fired": 0}),
        serde_json::json!({"turn_id": "t1", "iter": iter, "role": "tool",
                           "turn": res.to_string(), "fired": 0}),
    ]
}

#[test]
fn a_huge_tool_result_reaches_the_seam_capped_and_stays_whole_in_the_store() {
    // The environment keeps the value, the context window gets a bounded
    // preview -- the truncated-output discipline of #91. First half: what the
    // lane writes is NOT cut, so the full text stays addressable.
    let huge = "x".repeat(100_000);
    let out = emit(lane_doc(
        "in_tool",
        serde_json::json!([{"origin": "tool", "type": "tool_result", "id": "c1",
                            "text": huge}]),
    ));
    let stored: serde_json::Value =
        serde_json::from_str(op_of(&out[0])["row"]["turn"].as_str().expect("turn"))
            .expect("stored turn");
    assert_eq!(
        stored["text"].as_str().expect("text").len(),
        100_000,
        "the round store keeps the whole result: a cap is a preview, not a delete"
    );

    // Second half: the same row on its way to the brain is bounded.
    let mut rows = round_pair(0, "c1", &huge);
    rows.push(leg_window_row(
        serde_json::json!([{"role": "user", "text": "q"}]),
        0,
        0,
    ));
    let out = emit_with(
        &[("COLLECTOR_TOOL_CHARS", "50")],
        reply_doc("round-fire", "select", 3, serde_json::Value::Array(rows)),
    );
    assert_eq!(out.len(), 1);
    let texts = texts_of(&out[0]);
    assert_eq!(texts.len(), 3, "window turn + call + result: {texts:?}");
    assert_eq!(
        texts[2].len(),
        50,
        "the per-item cap ran before the seam, not after it"
    );
    assert_eq!(
        out[0]["header"]["round_capped"], "1",
        "a capped preview says that it was capped"
    );
}

#[test]
fn the_round_byte_cap_drops_whole_iterations_from_the_oldest_end() {
    // Three iterations of twelve characters each (a two-character call plus a
    // ten-character result). A budget of twenty-five buys two of them and
    // cannot afford the third, so the OLDEST iteration falls -- whole, with
    // its own call.
    let mut rows = vec![leg_window_row(serde_json::json!([]), 0, 0)];
    for i in 0..3 {
        rows.extend(round_pair(i, &format!("c{i}"), "bbbbbbbbbb"));
    }
    let out = emit_with(
        &[("COLLECTOR_ROUND_BYTES", "25")],
        reply_doc("round-fire", "select", 7, serde_json::Value::Array(rows)),
    );
    let msg = &out[0];
    let texts = texts_of(msg);
    assert_eq!(
        texts.len(),
        4,
        "two whole iterations survive, calls included: {texts:?}"
    );
    assert_eq!(
        msg["messages"][0]["id"], "c1",
        "the oldest survivor is a CALL"
    );
    assert_eq!(msg["messages"][3]["id"], "c2", "the newest round is last");
    assert_eq!(
        msg["header"]["round_dropped"], "2",
        "what left is counted, not silently gone"
    );
    assert_eq!(msg["header"]["round_capped"], "1");
}

#[test]
fn an_uncapped_round_says_so() {
    let mut rows = vec![leg_window_row(serde_json::json!([]), 0, 0)];
    rows.extend(round_pair(0, "c1", "short"));
    let out = emit(reply_doc(
        "round-fire",
        "select",
        3,
        serde_json::Value::Array(rows),
    ));
    assert_eq!(out[0]["header"]["round_dropped"], "0");
    assert_eq!(out[0]["header"]["round_capped"], "0");
    assert_eq!(out[0]["header"]["memory_capped"], "0");
}

// =============================================== THE SEAM'S OWN BOUND (#77)

#[test]
fn the_iteration_cap_ends_the_round_at_the_seam_and_not_at_the_dispatcher() {
    let mut rows = vec![leg_window_row(
        serde_json::json!([{"role": "user", "text": "look it up"}]),
        0,
        0,
    )];
    rows.extend(round_pair(1, "c1", "found"));
    let rows = serde_json::Value::Array(rows);

    // Under the cap the seam is what it always was.
    let out = emit_with(
        &[("COLLECTOR_MAX_ITER", "2")],
        reply_at("round-fire", "select", 3, rows.clone(), 1),
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "brain");
    assert_eq!(out[0]["header"]["iter"], "2");

    // At the cap the SAME phase leaves through the answer lane instead. The
    // round began at the seam, so the seam is what ends it -- no dispatcher
    // and no edge condition is needed to stop the loop (R-OS-2).
    let out = emit_with(
        &[("COLLECTOR_MAX_ITER", "2")],
        reply_at("round-fire", "select", 3, rows, 2),
    );
    assert_eq!(out.len(), 1, "one emission, and it is not a brain call");
    assert_eq!(out[0]["header"]["route"], "answer");
    assert_eq!(out[0]["header"]["round_capped"], "1");
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "brain"),
        "a capped turn asks nothing more"
    );
    let texts = texts_of(&out[0]);
    assert_eq!(
        texts[0], "look it up",
        "what was collected leaves with it: {texts:?}"
    );
    assert_eq!(texts.len(), 3, "window turn + the round so far: {texts:?}");
}

#[test]
fn the_first_assembly_of_a_turn_is_never_the_capped_one() {
    // iter 0 against the default of 8: the cap is a bound on the ROUND, not a
    // tax on every turn.
    let rows = serde_json::json!([leg_window_row(
        serde_json::json!([{"role": "user", "text": "hi"}]),
        0,
        0
    )]);
    let out = emit(reply_doc("fire", "select", 1, rows));
    assert_eq!(out[0]["header"]["route"], "brain");
    assert_eq!(out[0]["header"]["round_capped"], "0");
}

#[test]
fn the_rendered_memory_bundle_is_capped_before_it_enters_the_system_slot() {
    let big = "m".repeat(500);
    let bundle = serde_json::json!({
        "system": {"memory": {"bundle": {"text": big}}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": big}]
    });
    let rows = serde_json::json!([
        leg_window_row(serde_json::json!([]), 0, 0),
        {"turn_id": "t1", "iter": 0, "role": "leg-memory",
         "turn": bundle.to_string(), "fired": 0}
    ]);
    let out = emit_with(
        &[
            ("COLLECTOR_MEMORY_TIER", "0"),
            ("COLLECTOR_MEMORY_CHARS", "20"),
        ],
        reply_doc("fire", "select", 2, rows.clone()),
    );
    assert_eq!(
        out[0]["system"]["memory"]["recall"]["text"]
            .as_str()
            .expect("recall")
            .len(),
        20,
        "an oversized bundle cannot flood the window past every other knob"
    );
    assert_eq!(out[0]["header"]["memory_capped"], "1");

    // The machine-readable half answers to the same knob.
    let out = emit_with(
        &[
            ("COLLECTOR_MEMORY_TIER", "0"),
            ("COLLECTOR_MEMORY_CHARS", "20"),
            ("COLLECTOR_MEMORY_FORM", "json"),
        ],
        reply_doc("fire", "select", 2, rows),
    );
    assert_eq!(
        out[0]["system"]["memory"]["bundle"]["text"]
            .as_str()
            .expect("bundle")
            .len(),
        20
    );
}

#[test]
fn the_answer_is_written_into_the_window_before_it_leaves() {
    let out = emit(lane_doc(
        "in_answer",
        serde_json::json!([{"origin": "assistant", "type": "text", "text": "you said first"}]),
    ));
    assert_eq!(out.len(), 2, "the write and the way out, in one multi-send");
    let write = out
        .iter()
        .find(|m| m["header"]["route"] == "cstore")
        .expect("the assistant turn is persisted");
    let op = op_of(write);
    assert_eq!(op["table"], "turns");
    assert_eq!(op["row"]["role"], "assistant");
    assert_eq!(op["row"]["content"], "you said first");
    assert_eq!(
        op["row"]["turn_id"], "t1",
        "written under the turn it answers"
    );
    let answer = out
        .iter()
        .find(|m| m["header"]["route"] == "answer")
        .expect("the answer leaves");
    assert_eq!(texts_of(answer), vec!["you said first"]);
}

// ================================================ ROUND ROBUSTNESS (GH #103)
//
// A round whose fan-in can never complete must not park forever: when its last
// progress is older than COLLECTOR_ROUND_IDLE_MS and a message reaches this
// cell anyway (the next result, the next turn, a sweep), the missing calls get
// synthetic error results and the round fires through its regular route.

/// Old enough to be behind any idle window a test could configure.
const STALE: &str = "2000-01-01T00:00:00.000000Z";
/// Newer than any cutoff minted from the real clock.
const FRESH: &str = "2999-01-01T00:00:00.000000Z";

/// One assistant `tool_call` row expecting `ids`, dated `rec`.
fn asst_row(iter: i64, ids: &[&str], rec: &str) -> serde_json::Value {
    let calls: Vec<serde_json::Value> = ids
        .iter()
        .map(|id| {
            serde_json::json!({"origin": "assistant", "type": "tool_call",
                                     "id": id, "text": "{}"})
        })
        .collect();
    serde_json::json!({"turn_id": "t1", "iter": iter, "role": "assistant",
                       "turn": serde_json::Value::Array(calls).to_string(),
                       "fired": 0, "recorded_at": rec})
}

/// One real tool result row, dated `rec`.
fn tool_row(iter: i64, id: &str, text: &str, rec: &str) -> serde_json::Value {
    let res = serde_json::json!({"origin": "tool", "type": "tool_result", "id": id, "text": text});
    serde_json::json!({"turn_id": "t1", "iter": iter, "role": "tool",
                       "turn": res.to_string(), "fired": 0, "recorded_at": rec})
}

/// A synthetic stand-in as the stale close writes it: the `lost` marker stays
/// in the store row and never reaches the wire.
fn lost_row(iter: i64, id: &str) -> serde_json::Value {
    let res = serde_json::json!({"origin": "tool", "type": "tool_result", "id": id,
                                 "text": "tool result lost: the round went idle before this call's result arrived",
                                 "lost": 1});
    serde_json::json!({"turn_id": "t1", "iter": iter, "role": "tool",
                       "turn": res.to_string(), "fired": 0, "recorded_at": STALE})
}

#[test]
fn an_incomplete_round_with_fresh_progress_parks() {
    // The round started long ago, but a result just arrived: progress resets
    // the clock, so the round is still someone's to answer.
    let rows = serde_json::json!([
        asst_row(0, &["c1", "c2"], STALE),
        tool_row(0, "c1", "a", FRESH)
    ]);
    assert!(
        emit(reply_doc("round-check", "select", 2, rows)).is_empty(),
        "progress within the idle window: park, exactly as before #103"
    );
}

#[test]
fn a_stale_round_is_closed_with_synthetic_error_results_for_the_missing_calls() {
    let rows = serde_json::json!([
        asst_row(0, &["c1", "c2", "c3"], STALE),
        tool_row(0, "c1", "a", STALE)
    ]);
    let out = emit(reply_doc("round-check", "select", 2, rows));
    assert_eq!(
        out.len(),
        2,
        "one synthetic result per missing call, nothing else: {out:?}"
    );
    for (msg, id) in out.iter().zip(["c2", "c3"]) {
        assert_eq!(msg["header"]["route"], "cstore");
        assert_eq!(
            msg["header"]["phase"], "round-w",
            "the stand-in re-enters the REGULAR fan-in, no second machinery"
        );
        let op = op_of(msg);
        assert_eq!(op["operation"], "insert");
        assert_eq!(op["table"], "round");
        assert_eq!(op["row"]["role"], "tool");
        assert_eq!(op["row"]["session_id"], "s1");
        assert_eq!(
            op["row"]["iter"], 0,
            "the stand-in belongs to the round it closes"
        );
        assert!(
            !op["row"]["recorded_at"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "dated like every other round row"
        );
        let turn: serde_json::Value =
            serde_json::from_str(op["row"]["turn"].as_str().expect("turn")).expect("turn json");
        assert_eq!(turn["type"], "tool_result");
        assert_eq!(
            turn["id"], *id,
            "the tool_call_id is kept, so the fan-in completes (dispatcher lid pattern)"
        );
        assert!(
            turn["text"]
                .as_str()
                .expect("text")
                .starts_with("tool result lost"),
            "{turn}"
        );
        assert_eq!(
            turn["lost"], 1,
            "the row-level marker the seam later reports as round_stale"
        );
    }
}

#[test]
fn an_undatable_round_never_goes_stale() {
    // Rows from before `recorded_at` cannot be dated; they keep the pre-#103
    // behaviour (park and wait) -- symmetric to the prune lane's R-P3, where
    // a row the policy cannot date is a row the policy never touches.
    let rows = serde_json::json!([asst_row(0, &["c1", "c2"], ""), tool_row(0, "c1", "a", "")]);
    assert!(emit(reply_doc("round-check", "select", 2, rows)).is_empty());
}

#[test]
fn a_stale_closed_round_fires_with_round_stale_on_the_seam() {
    let rows = serde_json::json!([
        leg_window_row(serde_json::json!([{"role": "user", "text": "q"}]), 0, 0),
        asst_row(0, &["c1", "c2"], STALE),
        tool_row(0, "c1", "found", STALE),
        lost_row(0, "c2")
    ]);
    let out = emit(reply_doc("round-fire", "select", 4, rows));
    assert_eq!(out.len(), 1);
    let msg = &out[0];
    assert_eq!(
        msg["header"]["route"], "brain",
        "the round fires through its regular route, stale or not"
    );
    assert_eq!(
        msg["header"]["round_stale"], "1",
        "a stale-closed round says so on the hop"
    );
    let texts = texts_of(msg);
    assert_eq!(
        texts.len(),
        5,
        "window turn + 2 calls + real result + stand-in: {texts:?}"
    );
    assert!(texts[4].starts_with("tool result lost"), "{texts:?}");
    assert!(
        msg["messages"][4]["lost"].is_null(),
        "the marker is store bookkeeping and never reaches the wire"
    );

    // The control direction: a round that completed on its own is not stale.
    let rows = serde_json::json!([
        leg_window_row(serde_json::json!([{"role": "user", "text": "q"}]), 0, 0),
        asst_row(0, &["c1", "c2"], STALE),
        tool_row(0, "c1", "found", STALE),
        tool_row(0, "c2", "also found", STALE)
    ]);
    let out = emit(reply_doc("round-fire", "select", 4, rows));
    assert_eq!(out[0]["header"]["round_stale"], "0");
}

#[test]
fn a_lost_marker_from_an_older_iteration_does_not_stick() {
    // Iteration 0 was closed stale, iteration 1 completed on its own. The
    // fire of iteration 1 carries the old stand-in as history, but does not
    // call ITSELF stale.
    let rows = serde_json::json!([
        leg_window_row(serde_json::json!([{"role": "user", "text": "q"}]), 0, 0),
        asst_row(0, &["c1"], STALE),
        lost_row(0, "c1"),
        asst_row(1, &["c2"], STALE),
        tool_row(1, "c2", "found", STALE)
    ]);
    let out = emit(reply_at("round-fire", "select", 5, rows, 1));
    assert_eq!(out[0]["header"]["route"], "brain");
    assert_eq!(
        out[0]["header"]["round_stale"], "0",
        "the flag belongs to the round being fired, not to the whole thread"
    );
    assert!(
        texts_of(&out[0])
            .iter()
            .any(|t| t.starts_with("tool result lost")),
        "the old stand-in still travels as history"
    );
}

#[test]
fn a_late_real_result_wins_over_its_synthetic_stand_in() {
    // The race the store cannot prevent: the real result arrives between the
    // stale detection and the fire. Both rows exist; the wire carries the
    // real one and only the real one.
    let rows = serde_json::json!([
        leg_window_row(serde_json::json!([{"role": "user", "text": "q"}]), 0, 0),
        asst_row(0, &["c1"], STALE),
        lost_row(0, "c1"),
        tool_row(0, "c1", "late but real", STALE)
    ]);
    let out = emit(reply_doc("round-fire", "select", 4, rows));
    let texts = texts_of(&out[0]);
    assert_eq!(
        texts,
        vec!["q", "{}", "late but real"],
        "one result per call id, and it is the real one"
    );
    assert_eq!(
        out[0]["header"]["round_stale"], "0",
        "no stand-in travelled, so the context is not stale"
    );
}

#[test]
fn a_mid_round_turn_is_deferred_not_assembled() {
    // A user turn while a round of the session is open: the turn is in the
    // window already (its insert happened on the lane) and stays there, but it
    // starts NO second assembly -- one open brain call per session, the
    // telephone model (R-OS-3). The deferral stamp is the one emission.
    let open = serde_json::json!([{"turn_id": "t9", "iter": 1, "recorded_at": FRESH}]);
    let out = emit(reply_doc("turn-open", "select", 1, open));
    assert_eq!(
        out.len(),
        1,
        "the stamp and nothing else: no window read, no brain: {out:?}"
    );
    let msg = &out[0];
    assert_eq!(msg["header"]["route"], "cstore");
    assert_eq!(msg["header"]["phase"], "defer-w");
    assert_eq!(
        msg["header"]["round_deferred"], "1",
        "the parked arrival says on its hop that it was deferred"
    );
    let op = op_of(msg);
    assert_eq!(op["operation"], "update");
    assert_eq!(op["table"], "turns");
    assert_eq!(op["set"]["deferred"], 1);
    assert_eq!(
        op["where"]["turn_id"], "t1",
        "the NEW turn is the one stamped, not the round's turn"
    );
    assert_eq!(op["where"]["role"], "user");
}

#[test]
fn a_mid_round_turn_with_a_stale_round_also_triggers_the_close() {
    // The parked arrival is itself the next OCCASION: a round whose start
    // already lies behind the idle window gets its re-check in the same
    // multi-send that defers the turn.
    let open = serde_json::json!([{"turn_id": "t9", "iter": 1, "recorded_at": STALE}]);
    let out = emit(reply_doc("turn-open", "select", 1, open));
    assert_eq!(out.len(), 2, "the stamp and the re-check: {out:?}");
    assert_eq!(out[0]["header"]["phase"], "defer-w");
    let check = &out[1];
    assert_eq!(check["header"]["phase"], "round-check");
    assert_eq!(
        check["header"]["turn_id"], "t9",
        "the re-check runs under the OPEN round's turn"
    );
    assert_eq!(
        check["header"]["iter"], "1",
        "and under the open round's iteration -- the edge promotes both"
    );
    let op = op_of(check);
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "round");
    assert_eq!(op["where"]["turn_id"], "t9");
}

#[test]
fn an_undatable_open_round_does_not_defer_the_turn() {
    // An open assistant row without recorded_at predates the policy; it can
    // neither be closed (undatable) nor block the session forever. Such a
    // session keeps the pre-#103 behaviour: the turn assembles normally.
    let open = serde_json::json!([{"turn_id": "t9", "iter": 1, "recorded_at": ""}]);
    let out = emit(reply_doc("turn-open", "select", 1, open));
    assert_eq!(out.len(), 1);
    let op = op_of(&out[0]);
    assert_eq!(op["table"], "turns", "the window read, not a deferral");
    assert_eq!(op["operation"], "select");
}

#[test]
fn a_deferred_turn_rides_with_the_next_assembly_and_is_cleared() {
    // 1. The window read sees the stamp and counts it into the leg.
    let rows = serde_json::json!([
        turn_row("2", "user", "the deferred one"),
        turn_row("1", "user", "first")
    ]);
    let mut rows = rows;
    rows[0]["deferred"] = serde_json::json!(1);
    let out = emit(reply_doc("win", "select", 2, rows));
    let payload: serde_json::Value =
        serde_json::from_str(op_of(&out[0])["row"]["turn"].as_str().expect("turn"))
            .expect("payload");
    assert_eq!(
        payload["deferred"], 1,
        "the leg carries how many deferred turns it holds"
    );

    // 2. The fire that carries a deferred turn says so on the seam and clears
    //    the stamp in the same multi-send -- round_deferred marks the ARRIVAL,
    //    not every later window that still contains the turn.
    let leg = serde_json::json!({"turn_id": "t1", "iter": 0, "role": "leg-window",
        "turn": "{\"turns\":[{\"role\":\"user\",\"text\":\"the deferred one\"}],\"dropped\":0,\"capped\":0,\"deferred\":1}",
        "fired": 0});
    let out = emit(reply_doc("fire", "select", 1, serde_json::json!([leg])));
    assert_eq!(out.len(), 2, "the seam and the clear: {out:?}");
    let brain = &out[0];
    assert_eq!(brain["header"]["route"], "brain");
    assert_eq!(brain["header"]["round_deferred"], "1");
    let clear = &out[1];
    assert_eq!(clear["header"]["phase"], "defer-clear");
    let op = op_of(clear);
    assert_eq!(op["operation"], "update");
    assert_eq!(op["table"], "turns");
    assert_eq!(op["set"]["deferred"], 0);
    assert_eq!(op["where"]["session_id"], "s1");
    assert_eq!(op["where"]["deferred"], 1);

    // 3. An assembly without a deferred turn is a single emission, flag 0.
    let out = emit(reply_doc(
        "fire",
        "select",
        1,
        serde_json::json!([leg_window_row(
            serde_json::json!([{"role": "user", "text": "hi"}]),
            0,
            0
        )]),
    ));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["round_deferred"], "0");
}

#[test]
fn the_fresh_turn_write_stamps_the_deferred_column() {
    // Both writers of `turns` stamp the flag explicitly, so a row is never
    // ambiguous between "not deferred" and "predates the column".
    let out = emit(lane_doc(
        "in_turn",
        serde_json::json!([{"origin": "user", "type": "text", "text": "hello"}]),
    ));
    assert_eq!(op_of(&out[0])["row"]["deferred"], 0);
    let out = emit(lane_doc(
        "in_answer",
        serde_json::json!([{"origin": "assistant", "type": "text", "text": "hi"}]),
    ));
    let write = out
        .iter()
        .find(|m| m["header"]["route"] == "cstore")
        .expect("the answer write");
    assert_eq!(op_of(write)["row"]["deferred"], 0);
}

#[test]
fn the_sweep_lane_asks_for_every_open_round_in_every_session() {
    // The re-check occasion a parent tree provides: a timer or an operator
    // asks "anything stuck?". The template never fires this itself (the
    // session-keeper discipline), and the question is session-agnostic on
    // purpose -- a timer knows no session; the rows do.
    let out = emit(lane_doc("in_round_sweep", serde_json::json!([])));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["phase"], "sweep");
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "round");
    assert_eq!(op["where"]["role"], "assistant");
    assert_eq!(op["where"]["fired"], 0);
    assert!(
        op["where"]["session_id"].is_null(),
        "no session filter: the sweep covers the whole store"
    );
}

#[test]
fn the_sweep_spawns_a_re_check_for_stale_rounds_and_only_for_those() {
    let rows = serde_json::json!([
        {"turn_id": "t7", "iter": 2, "session_id": "s7", "recorded_at": STALE},
        {"turn_id": "t8", "iter": 0, "session_id": "s8", "recorded_at": FRESH},
        {"turn_id": "t9", "iter": 1, "session_id": "s9", "recorded_at": ""}
    ]);
    let out = emit(reply_doc("sweep", "select", 3, rows));
    assert_eq!(
        out.len(),
        1,
        "one re-check for the stale round alone -- fresh waits, undatable is left in its pre-#103 behaviour: {out:?}"
    );
    let msg = &out[0];
    assert_eq!(msg["header"]["phase"], "round-check");
    assert_eq!(
        msg["header"]["turn_id"], "t7",
        "the re-check runs under the round's own turn"
    );
    assert_eq!(msg["header"]["iter"], "2");
    assert_eq!(
        msg["header"]["session_id"], "s7",
        "and under the round's own session -- a later stand-in must stamp it"
    );
    assert_eq!(op_of(msg)["where"]["turn_id"], "t7");

    // A sweep that finds nothing stale emits nothing: its observable effect
    // is the round it fires. This lane is an occasion, not a report lane.
    assert!(emit(reply_doc("sweep", "select", 0, serde_json::json!([]))).is_empty());
}

// ================================================== THE CLOSE LANE (R-OS-6)

#[test]
fn every_round_row_carries_the_session_it_belongs_to() {
    // The close lane finds a whole session's rounds by one column, so every
    // writer of the round table stamps it -- from the first assembly leg to
    // the last tool result.
    let out = emit(lane_doc(
        "in_calls",
        serde_json::json!([{"origin": "assistant", "type": "tool_call", "id": "c1",
                            "text": "{}"}]),
    ));
    assert_eq!(op_of(&out[0])["row"]["session_id"], "s1");
    let out = emit(reply_doc("win", "select", 0, serde_json::json!([])));
    assert_eq!(op_of(&out[0])["row"]["session_id"], "s1");
}

#[test]
fn the_close_request_reads_the_whole_session_oldest_first() {
    let out = emit(lane_doc("in_close", serde_json::json!([])));
    assert_eq!(out.len(), 1);
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "turns");
    assert_eq!(op["where"]["session_id"], "s1");
    assert_eq!(
        op["order_by"][0]["dir"], "asc",
        "a day is handed on in the order it happened"
    );
    assert!(
        op["limit"].is_null(),
        "the batch is the whole session; the caps above bound a CONTEXT window and this is not one"
    );
    assert_eq!(out[0]["header"]["phase"], "close-turns");
}

#[test]
fn the_close_batch_carries_the_session_and_its_rounds_in_one_emission() {
    // 1. The turns are parked as their own leg, so the round select can meet
    //    them again in the same reply -- the script holds no state between hops.
    let rows = serde_json::json!([
        turn_row("1", "user", "first"),
        turn_row("2", "assistant", "second")
    ]);
    let out = emit(reply_doc("close-turns", "select", 2, rows));
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "insert");
    assert_eq!(op["table"], "round");
    assert_eq!(op["row"]["role"], "leg-close");
    assert_eq!(op["row"]["session_id"], "s1");
    assert_eq!(
        op["row"]["turn_id"], "close-s1",
        "the bookkeeping row belongs to no turn, so it pollutes no turn's slate"
    );
    assert_eq!(out[0]["header"]["phase"], "close-w");
    let parked = op["row"]["turn"].as_str().expect("turn").to_string();

    // 2. Then the whole round table of the session, in one select.
    let out = emit(reply_doc("close-w", "insert", 1, serde_json::json!("ok")));
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "round");
    assert_eq!(op["where"]["session_id"], "s1");
    assert_eq!(out[0]["header"]["phase"], "close-fire");

    // 3. And ONE batch leaves on route write: append-all, no judgement. Since
    //    GH #76 the delivery ledger row travels in the same multi-send -- the
    //    batch is still ONE message, the evidence rides beside it.
    let slate = serde_json::json!([
        {"turn_id": "close-s1", "iter": 0, "role": "leg-close",
         "turn": parked, "fired": 0},
        {"turn_id": "t1", "iter": 0, "role": "leg-window",
         "turn": "{\"turns\":[],\"dropped\":3,\"capped\":1}", "fired": 1},
        {"turn_id": "t1", "iter": 0, "role": "tool",
         "turn": "{\"origin\":\"tool\",\"type\":\"tool_result\",\"id\":\"c1\",\"text\":\"r\"}",
         "fired": 0}
    ]);
    let out = emit(reply_doc("close-fire", "select", 3, slate));
    assert_eq!(
        out.len(),
        2,
        "ONE batch plus its ledger row, never one message per turn"
    );
    let msg = &out[0];
    assert_eq!(msg["header"]["route"], "write");
    assert_eq!(msg["header"]["session_id"], "s1");
    assert_eq!(msg["header"]["turn_count"], "2");
    assert_eq!(msg["header"]["round_count"], "2");
    assert_eq!(texts_of(msg), vec!["first", "second"]);
    assert_eq!(msg["messages"][1]["origin"], "assistant");
    assert_eq!(
        msg["rounds"][0]["turn"]["dropped"], 3,
        "the eviction reports of the day travel raw, next to the tool rounds"
    );
    assert_eq!(msg["rounds"][1]["turn"]["id"], "c1");
    assert!(
        !msg["rounds"]
            .as_array()
            .expect("rounds")
            .iter()
            .any(|r| r["role"] == "leg-close"),
        "the bookkeeping row is the collector's, not the batch's"
    );
}

#[test]
fn a_close_request_is_read_as_a_close_even_with_a_stale_step_in_context() {
    // The echo guard again: a close request arrives over a port edge and
    // carries whatever col_phase the sending chain left behind.
    let stale = serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0",
                               "col_phase": "fire"},
                   "hop": {"route": "in_close"}},
        "messages": []
    });
    let out = emit(stale);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["phase"], "close-turns");
}

#[test]
fn a_message_without_a_lane_and_without_a_step_is_terminal() {
    // The echo guard. A collector sits in a loop; anything it cannot name is
    // parked rather than answered, or the loop feeds itself.
    let stray = serde_json::json!({
        "header": {"context": {"session_id": "s1"}, "hop": {}},
        "messages": [{"origin": "user", "type": "text", "text": "stray"}]
    });
    assert!(emit(stray).is_empty());
    // A lane message that arrives with a stale step in its context is still
    // read as the LANE it is -- context is carried along a chain, and a port
    // edge cannot clear it.
    let stale = serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0",
                               "col_phase": "fire"},
                   "hop": {"route": "in_answer"}},
        "messages": [{"origin": "assistant", "type": "text", "text": "done"}]
    });
    let out = emit(stale);
    assert_eq!(out.len(), 2, "the lane wins over the stale step");
    assert!(out.iter().any(|m| m["header"]["route"] == "answer"));
}

// ==================================================== THE PRUNE LANE (GH #76)
//
// Housekeeping over the two session tables, gated on EVIDENCE: a session is
// prunable only when a close batch left the collector (the `batched` ledger
// row written beside the write emission) and that delivery is older than
// COLLECTOR_PRUNE_AFTER_MS. Without a ledger row nothing is ever pruned --
// rather grow than silently lose a turn.

/// A store reply on a chosen chain: the prune and close chains carry their
/// state (boundary, then counts) in the promoted hop id, because the script
/// keeps no state between hops.
fn reply_as(
    phase: &str,
    op: &str,
    rows_affected: i64,
    payload: serde_json::Value,
    session: &str,
    turn_id: &str,
) -> serde_json::Value {
    let mut doc = reply_doc(phase, op, rows_affected, payload);
    doc["header"]["context"]["session_id"] = serde_json::json!(session);
    doc["header"]["context"]["turn_id"] = serde_json::json!(turn_id);
    doc
}

#[test]
fn the_prune_boundary_is_minted_when_the_close_arrives() {
    // The hop id of the close chain carries the ARRIVAL time of the close
    // request. Every turn this cell processed before the close is in the batch
    // (one actor, ordered mailbox); every later one is stamped younger than
    // this boundary and survives a prune.
    let out = emit(lane_doc("in_close", serde_json::json!([])));
    let tid = out[0]["header"]["turn_id"].as_str().expect("turn_id");
    assert!(
        tid.starts_with("close-s1|"),
        "the boundary rides behind the bookkeeping id: {tid}"
    );
    assert!(
        tid.len() > "close-s1|".len(),
        "an empty boundary would date nothing: {tid}"
    );

    // The parked day is BACKDATED to that boundary -- its content is exactly
    // the session up to the close, so the next prune takes the copy along with
    // the day it copies. The row itself stays under the plain id (R-C5).
    let rows = serde_json::json!([turn_row("1", "user", "first")]);
    let out = emit(reply_as(
        "close-turns",
        "select",
        1,
        rows,
        "s1",
        "close-s1|2026-01-01T00:00:00.000000Z",
    ));
    let op = op_of(&out[0]);
    assert_eq!(op["row"]["turn_id"], "close-s1");
    assert_eq!(op["row"]["recorded_at"], "2026-01-01T00:00:00.000000Z");
    assert_eq!(
        out[0]["header"]["turn_id"], "close-s1|2026-01-01T00:00:00.000000Z",
        "the boundary keeps travelling to the ledger write"
    );

    // And the next step passes it on unchanged.
    let out = emit(reply_as(
        "close-w",
        "insert",
        1,
        serde_json::json!("ok"),
        "s1",
        "close-s1|2026-01-01T00:00:00.000000Z",
    ));
    assert_eq!(
        out[0]["header"]["turn_id"],
        "close-s1|2026-01-01T00:00:00.000000Z"
    );
}

#[test]
fn the_close_emission_writes_the_delivery_ledger_beside_the_batch() {
    let slate = serde_json::json!([
        {"turn_id": "close-s1", "iter": 0, "role": "leg-close",
         "turn": "[{\"role\":\"user\",\"text\":\"first\"}]", "fired": 0},
        {"turn_id": "t1", "iter": 0, "role": "leg-window",
         "turn": "{\"turns\":[],\"dropped\":0,\"capped\":0}", "fired": 1}
    ]);
    let out = emit(reply_as(
        "close-fire",
        "select",
        2,
        slate,
        "s1",
        "close-s1|2026-01-01T00:00:00.000000Z",
    ));
    assert_eq!(
        out.len(),
        2,
        "the delivery and its evidence leave in ONE multi-send"
    );
    assert_eq!(out[0]["header"]["route"], "write", "the batch goes first");
    let ledger = &out[1];
    assert_eq!(ledger["header"]["route"], "cstore");
    assert_eq!(ledger["header"]["phase"], "close-ledger");
    let op = op_of(ledger);
    assert_eq!(op["operation"], "insert");
    assert_eq!(op["table"], "batched");
    assert_eq!(op["row"]["session_id"], "s1");
    assert_eq!(
        op["row"]["batched_at"], "2026-01-01T00:00:00.000000Z",
        "the evidence is dated to the close ARRIVAL, not to this emission"
    );
}

#[test]
fn every_round_row_carries_its_write_time() {
    // A row the prune lane cannot date is a row it will never cut, so every
    // writer of the round table stamps recorded_at -- assembly legs and tool
    // rounds alike.
    let out = emit(reply_doc("win", "select", 0, serde_json::json!([])));
    let leg = op_of(&out[0]);
    assert!(
        !leg["row"]["recorded_at"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "the window leg is dated"
    );
    let out = emit(lane_doc(
        "in_calls",
        serde_json::json!([{"origin": "assistant", "type": "tool_call", "id": "c1",
                            "text": "{}"}]),
    ));
    assert!(
        !op_of(&out[0])["row"]["recorded_at"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "the tool round is dated"
    );
}

#[test]
fn a_prune_request_reads_the_ledger_and_only_the_ledger() {
    let out = emit(lane_doc("in_prune", serde_json::json!([])));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "cstore");
    assert_eq!(out[0]["header"]["phase"], "prune-ledger");
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "select");
    assert_eq!(
        op["table"], "batched",
        "eligibility is the ledger, nothing else"
    );
    let cut = op["where"]["batched_at"]["lte"]
        .as_str()
        .expect("age gate")
        .to_string();
    assert!(!cut.is_empty());
    assert_eq!(
        op["where"]["pruned_at"]["is_null"], true,
        "evidence already used does not fire twice"
    );

    // The default gate is seven days; a zero gate cuts at (approximately) now.
    // Both cutoffs are minted from the same clock, so they order.
    let out = emit_with(
        &[("COLLECTOR_PRUNE_AFTER_MS", "0")],
        lane_doc("in_prune", serde_json::json!([])),
    );
    let cut0 = op_of(&out[0])["where"]["batched_at"]["lte"]
        .as_str()
        .expect("zero gate")
        .to_string();
    assert!(
        cut < cut0,
        "the seven-day default cutoff lies before the zero-gate cutoff: {cut} vs {cut0}"
    );
}

#[test]
fn a_prune_without_ledger_evidence_deletes_nothing_and_says_so() {
    let out = emit(reply_doc(
        "prune-ledger",
        "select",
        0,
        serde_json::json!([]),
    ));
    assert_eq!(out.len(), 1, "no evidence: no delete op leaves at all");
    let msg = &out[0];
    assert_eq!(
        msg["header"]["route"], "prune",
        "the operator lane asked, the operator lane gets an answer"
    );
    assert_eq!(msg["header"]["pruned_turns"], "0");
    assert_eq!(msg["header"]["pruned_rounds"], "0");
    assert_eq!(msg["header"]["session_id"], "");
}

#[test]
fn an_aged_batched_session_is_cut_exactly_at_its_evidence() {
    // Three ledger rows, two sessions -- s7 was closed twice. One turn cut per
    // SESSION, scoped to its own youngest delivered boundary; the chains run
    // in parallel and never name a session the ledger did not.
    let rows = serde_json::json!([
        {"session_id": "s7", "batched_at": "2026-01-01T00:00:00.000000Z"},
        {"session_id": "s7", "batched_at": "2026-01-05T00:00:00.000000Z"},
        {"session_id": "s9", "batched_at": "2026-01-03T00:00:00.000000Z"}
    ]);
    let out = emit(reply_doc("prune-ledger", "select", 3, rows));
    assert_eq!(
        out.len(),
        2,
        "one cut per session, whatever the ledger row count"
    );
    for msg in &out {
        assert_eq!(msg["header"]["phase"], "prune-t");
        assert_eq!(op_of(msg)["operation"], "delete");
        assert_eq!(op_of(msg)["table"], "turns");
    }
    let s7 = out
        .iter()
        .find(|m| m["header"]["session_id"] == "s7")
        .expect("s7 chain");
    let op = op_of(s7);
    assert_eq!(op["where"]["session_id"], "s7");
    assert_eq!(
        op["where"]["recorded_at"]["lte"], "2026-01-05T00:00:00.000000Z",
        "a re-closed session is cut at its YOUNGEST delivered boundary"
    );
    assert_eq!(
        s7["header"]["turn_id"], "prune|2026-01-05T00:00:00.000000Z",
        "the boundary rides in the hop id to the round cut"
    );
    let s9 = out
        .iter()
        .find(|m| m["header"]["session_id"] == "s9")
        .expect("s9 chain");
    assert_eq!(
        op_of(s9)["where"]["recorded_at"]["lte"],
        "2026-01-03T00:00:00.000000Z"
    );
}

#[test]
fn the_prune_chain_cuts_rounds_with_the_same_boundary_and_reports_the_cut() {
    // Step 2 of a chain: the turns fell (rows_affected 4), the round rows
    // follow under the SAME boundary. Strictly `lte`, never or_null: a row
    // without a write time predates the policy and is never pruned.
    let out = emit(reply_as(
        "prune-t",
        "delete",
        4,
        serde_json::json!("ok"),
        "s7",
        "prune|2026-01-05T00:00:00.000000Z",
    ));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["phase"], "prune-r");
    let op = op_of(&out[0]);
    assert_eq!(op["operation"], "delete");
    assert_eq!(op["table"], "round");
    assert_eq!(op["where"]["session_id"], "s7");
    let by_age = op["where"]["recorded_at"]
        .as_object()
        .expect("operator object");
    assert_eq!(
        by_age.len(),
        1,
        "exactly lte -- an undatable row never falls"
    );
    assert_eq!(by_age["lte"], "2026-01-05T00:00:00.000000Z");
    assert_eq!(
        out[0]["header"]["turn_id"], "prune|2026-01-05T00:00:00.000000Z|4",
        "the turn count rides to the report"
    );

    // Step 3: the evidence is marked used and the cut is reported, in ONE
    // multi-send. The report is rows_affected of the deletes themselves, not a
    // re-read of what is no longer there.
    let out = emit(reply_as(
        "prune-r",
        "delete",
        3,
        serde_json::json!("ok"),
        "s7",
        "prune|2026-01-05T00:00:00.000000Z|4",
    ));
    assert_eq!(out.len(), 2, "the mark and the report");
    let mark = out
        .iter()
        .find(|m| m["header"]["route"] == "cstore")
        .expect("mark");
    assert_eq!(mark["header"]["phase"], "prune-mark");
    let op = op_of(mark);
    assert_eq!(op["operation"], "update");
    assert_eq!(op["table"], "batched");
    assert!(
        !op["set"]["pruned_at"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "used evidence is dated, not deleted"
    );
    assert_eq!(op["where"]["session_id"], "s7");
    assert_eq!(
        op["where"]["batched_at"]["lte"],
        "2026-01-05T00:00:00.000000Z"
    );
    let report = out
        .iter()
        .find(|m| m["header"]["route"] == "prune")
        .expect("report");
    assert_eq!(report["header"]["session_id"], "s7");
    assert_eq!(report["header"]["pruned_turns"], "4");
    assert_eq!(report["header"]["pruned_rounds"], "3");
    assert_eq!(
        report["header"]["prune_boundary"],
        "2026-01-05T00:00:00.000000Z"
    );
}

// ==================================================== THE MEMORY TOOL (GH #78)
//
// The ambient leg is fired before the model has seen the turn, so nothing in an
// agent can DECIDE to ask memory about a TIME RANGE -- the half of #27 that did
// not fall out naturally. The tool closes it, and it closes it INSIDE the
// collector: from the dispatcher's side `memory_recall` is a tool like any
// other (it names the tool, an edge knows the cell), and the cell behind that
// edge is the collector itself, because the collector already owns the recall
// port (R-OS-5). The round therefore ends where it began (R-OS-2), and memory
// never learns a word of dispatcher vocabulary.

/// The tool call as `dispatcher@1` hands it on: ONE tool_call turn whose `text`
/// is the raw arguments string and whose id is the one the round's expectation
/// set was built from.
fn memory_call(args: serde_json::Value) -> serde_json::Value {
    lane_doc(
        "in_memory_call",
        serde_json::json!([{"origin": "assistant", "type": "tool_call", "id": "m1",
                            "text": args.to_string()}]),
    )
}

/// A recall bundle on its way back, correlated to the call that asked for it.
fn memory_answer(call_id: &str, readable: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0",
                               "memory_call_id": call_id},
                   "hop": {"route": "in_bundle"}},
        "system": {"memory": {"bundle": {"text": "{\"beliefs\":[]}"}}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                      "text": readable}]
    })
}

#[test]
fn a_memory_recall_call_is_served_on_the_collectors_own_recall_port() {
    let out = emit(memory_call(
        serde_json::json!({"query": "what did I say about the roof?"}),
    ));
    assert_eq!(out.len(), 1, "one request, no store round-trip: {out:?}");
    let ask = &out[0];
    assert_eq!(
        ask["header"]["route"], "recall",
        "the same port the per-turn leg uses -- no second door into memory"
    );
    assert_eq!(
        ask["header"]["recall_query"],
        "what did I say about the roof?"
    );
    assert_eq!(
        ask["header"]["memory_tier"], "1",
        "the tier is configuration, never a model argument"
    );
    assert_eq!(
        ask["header"]["memory_call_id"], "m1",
        "the correlation key that turns the answer back into THIS call's result"
    );
    assert_eq!(
        ask["header"]["turn_id"], "t1",
        "and it belongs to the running turn, like every other leg"
    );
    assert_eq!(texts_of(ask), vec!["what did I say about the roof?"]);
}

#[test]
fn the_window_arguments_of_the_call_reach_the_recall_request() {
    // The P15 finding was that the recall cell has understood
    // recall_window_from/_to for a long time and nobody ever DERIVES one. This
    // is the first producer, and it sits at the consumer: the model asked.
    let out = emit(memory_call(serde_json::json!({
        "query": "what did we decide?",
        "window_from": "2026-08-01T00:00:00Z",
        "window_to": "2026-08-02T00:00:00Z"
    })));
    assert_eq!(
        out[0]["header"]["recall_window_from"],
        "2026-08-01T00:00:00Z"
    );
    assert_eq!(out[0]["header"]["recall_window_to"], "2026-08-02T00:00:00Z");

    // The ambient ask travels the SAME edge, so it carries the same keys --
    // empty rather than absent, because a missing hop key makes the promoting
    // CEL modifier fail and a failed modifier skips the edge.
    let out = emit_with(
        &[("COLLECTOR_MEMORY_TIER", "0")],
        lane_doc(
            "in_turn",
            serde_json::json!([{"origin": "user", "type": "text", "text": "hello"}]),
        ),
    );
    let ask = out
        .iter()
        .find(|m| m["header"]["route"] == "recall")
        .expect("the ambient request");
    assert_eq!(ask["header"]["recall_window_from"], "");
    assert_eq!(ask["header"]["recall_window_to"], "");
    assert_eq!(
        ask["header"]["memory_call_id"], "",
        "no call asked for it: the ambient leg is the free floor under every turn"
    );
}

#[test]
fn the_bundle_of_a_tool_call_becomes_a_tool_result_of_the_round() {
    let out = emit(memory_answer(
        "m1",
        "MEMORY (tier 1)\n- the roof was fixed in May",
    ));
    assert_eq!(out.len(), 1);
    let op = op_of(&out[0]);
    assert_eq!(op["table"], "round");
    assert_eq!(
        op["row"]["role"], "tool",
        "an answered call is a tool RESULT of the round, not a leg of the turn"
    );
    assert_eq!(out[0]["header"]["phase"], "round-w", "the ordinary fan-in");
    let turn: serde_json::Value =
        serde_json::from_str(op["row"]["turn"].as_str().expect("turn")).expect("turn json");
    assert_eq!(turn["type"], "tool_result");
    assert_eq!(
        turn["id"], "m1",
        "under the ORIGINAL tool_call_id, or the round could never complete"
    );
    assert_eq!(turn["text"], "MEMORY (tier 1)\n- the roof was fixed in May");

    // And the ambient bundle is untouched by all of this: no call id, no round.
    let mut ambient = memory_answer("", "MEMORY (tier 0)\n- the editor is helix");
    ambient["header"]["context"]
        .as_object_mut()
        .expect("ctx")
        .remove("memory_call_id");
    let out = emit(ambient);
    assert_eq!(op_of(&out[0])["row"]["role"], "leg-memory");
}

#[test]
fn a_memory_result_fans_in_beside_a_normal_tool_result_and_the_round_fires() {
    // The bundle the issue describes: one memory_recall and one ordinary tool
    // in ONE brain answer. The memory call is a normal member of the
    // expectation set -- it is counted, waited for and capped like any other.
    let calls = serde_json::json!([
        {"origin": "assistant", "type": "tool_call", "id": "c1", "text": "{}"},
        {"origin": "assistant", "type": "tool_call", "id": "m1", "text": "{}"}
    ]);
    let asst = serde_json::json!({"turn_id": "t1", "iter": 0, "role": "assistant",
                                  "turn": calls.to_string(), "fired": 0});
    let tool = serde_json::json!({"turn_id": "t1", "iter": 0, "role": "tool",
        "turn": "{\"origin\":\"tool\",\"type\":\"tool_result\",\"id\":\"c1\",\"text\":\"the weather\"}",
        "fired": 0});
    let memo = serde_json::json!({"turn_id": "t1", "iter": 0, "role": "tool",
        "turn": "{\"origin\":\"tool\",\"type\":\"tool_result\",\"id\":\"m1\",\"text\":\"MEMORY: the roof\"}",
        "fired": 0});

    assert!(
        emit(reply_doc(
            "round-check",
            "select",
            2,
            serde_json::json!([asst.clone(), tool.clone()])
        ))
        .is_empty(),
        "the round waits for the memory call exactly as it waits for a tool"
    );

    let full = serde_json::json!([asst, tool, memo]);
    let out = emit(reply_doc("round-check", "select", 3, full.clone()));
    assert_eq!(op_of(&out[0])["operation"], "update", "the round completed");

    let mut rows = full.as_array().expect("rows").clone();
    rows.push(leg_window_row(
        serde_json::json!([{"role": "user", "text": "what about the roof?"}]),
        0,
        0,
    ));
    let out = emit(reply_doc(
        "round-fire",
        "select",
        4,
        serde_json::Value::Array(rows),
    ));
    assert_eq!(out.len(), 1);
    let texts = texts_of(&out[0]);
    assert!(
        texts.contains(&"the weather".to_string())
            && texts.contains(&"MEMORY: the roof".to_string()),
        "both results reach the brain through the ONE seam: {texts:?}"
    );
    assert_eq!(out[0]["header"]["route"], "brain");
    assert_eq!(out[0]["header"]["iter"], "1");
}

#[test]
fn a_memory_call_without_a_configured_tier_is_answered_instead_of_parked() {
    // The tool switched off is a FAILED call, not a hung round: asking into a
    // void would park the fan-in until the idle exit. The lid pattern again --
    // the brain sees the failure and has to answer it.
    let out = emit_with(
        &[("COLLECTOR_MEMORY_CALL_TIER", "")],
        memory_call(serde_json::json!({"query": "anything?"})),
    );
    assert_eq!(out.len(), 1);
    assert_eq!(
        out[0]["header"]["route"], "cstore",
        "no request leaves for a port that would not answer it"
    );
    let op = op_of(&out[0]);
    assert_eq!(op["row"]["role"], "tool");
    let turn: serde_json::Value =
        serde_json::from_str(op["row"]["turn"].as_str().expect("turn")).expect("turn json");
    assert_eq!(turn["id"], "m1");
    assert!(
        turn["text"]
            .as_str()
            .unwrap_or_default()
            .contains("not configured"),
        "and it says why: {turn}"
    );
}

#[test]
fn the_memory_result_is_capped_like_every_other_tool_result() {
    // GH #91's discipline, unchanged: the recall bundle of a TOOL call enters
    // the window through the round, so the round's per-item cap runs on it.
    let big = "m".repeat(9000);
    let calls = serde_json::json!([
        {"origin": "assistant", "type": "tool_call", "id": "m1", "text": "{}"}
    ]);
    let res = serde_json::json!(
        {"origin": "tool", "type": "tool_result", "id": "m1", "text": big}
    );
    let rows = serde_json::json!([
        {"turn_id": "t1", "iter": 0, "role": "assistant",
         "turn": calls.to_string(), "fired": 0},
        {"turn_id": "t1", "iter": 0, "role": "tool",
         "turn": res.to_string(), "fired": 0}
    ]);
    let out = emit(reply_doc("round-fire", "select", 2, rows));
    let texts = texts_of(&out[0]);
    assert_eq!(
        texts[1].len(),
        4000,
        "COLLECTOR_TOOL_CHARS, exactly as for a web search result"
    );
    assert_eq!(
        out[0]["header"]["round_capped"], "1",
        "and the cut is reported"
    );
}

#[test]
fn the_machine_readable_form_of_a_called_bundle_is_a_configuration_choice() {
    let out = emit_with(
        &[("COLLECTOR_MEMORY_FORM", "json")],
        memory_answer("m1", "MEMORY (tier 1)\n- the roof"),
    );
    let turn: serde_json::Value =
        serde_json::from_str(op_of(&out[0])["row"]["turn"].as_str().expect("turn"))
            .expect("turn json");
    assert_eq!(
        turn["text"], "{\"bundle\": {\"text\": \"{\\\"beliefs\\\":[]}\"}}",
        "the collector renders nothing of its own; it chooses a form"
    );
}
