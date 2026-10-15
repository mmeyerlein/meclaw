//! meclaw-os 1 -- the collector hive assembles the running round (GitHub #27).
//!
//! The collector grew from an example pattern (a code cell that fans tool
//! results back in) into a hive class: the fan-in of an agent's round. Two
//! claims are pinned here, one per group:
//!
//! 1. ASSEMBLY -- a turn is written BEFORE its round is read back, so the round
//!    a brain gets carries the turn it is answering, and every leg the round
//!    declared (window, memory, brief) is waited for and nothing else.
//! 2. THE SEAM -- everything the brain sees of a round leaves through ONE
//!    message on ONE route. Turn, memory bundle and tool round meet there and
//!    nowhere else, which is what lets the curator stand behind the seam.
//!
//! GH #889 (`collector@5.0.0`) took the third claim away: the rolling window,
//! its turn and byte caps and the eviction policy over them now belong to the
//! curator hive behind `curate` (R-27-1), so the seam is named `curate` and
//! carries the running round uncut, without an earlier turn. The close, prune
//! and episode lanes went with the history they read.
//!
//! Everything runs the shipped `params.script_inline` against real stdin
//! documents, so nothing is mocked and nothing is spent.

use std::io::Write;
use std::process::{Command, Stdio};

const ASSEMBLE_CONFIG: &str = "../../templates/collector/assemble/config.json";

/// The shipped `config.json` of `./assemble`, parsed.
fn assemble_config() -> serde_json::Value {
    let raw = std::fs::read_to_string(ASSEMBLE_CONFIG).expect("assemble config");
    serde_json::from_str(&raw).expect("config json")
}

fn assemble_script() -> String {
    assemble_config()["params"]["script_inline"]
        .as_str()
        .expect("script_inline")
        .to_string()
}

/// The `params` object the substrate puts on this cell's stdin: the SHIPPED
/// values of the template with the case's overrides merged over them, minus the
/// script's own source (`build_stdin_json` withholds it).
///
/// Reading the defaults out of the config instead of restating them here is
/// what makes a case that names no knob a test of the shipped value; and the
/// assertion on the key makes a typo in a knob name a failure instead of a
/// silently ignored override.
fn assemble_params(over: &[(&str, &str)]) -> serde_json::Value {
    let mut p = assemble_config()["params"]
        .as_object()
        .cloned()
        .expect("params object");
    p.remove("script_inline");
    for (k, v) in over {
        assert!(p.contains_key(*k), "no such collector param: {k}");
        p.insert((*k).to_string(), serde_json::json!(v));
    }
    serde_json::Value::Object(p)
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped
/// scripts have grown to within a few KB of that line, so `python3 -c <whole
/// script>` is a harness that breaks on size rather than on behaviour (GH #279,
/// precedent 89a522e4). stdin carries the program, so the document rides inside
/// it and is put under `sys.stdin` before the script runs. From there the script
/// executes exactly as `python3 -c` ran it: same `__main__` globals, same
/// stdout, same exit status.
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(stdin_doc).unwrap(),
    );
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run the real script against a real stdin document and return the emitted
/// messages.
fn emit_with(over: &[(&str, &str)], doc: serde_json::Value) -> Vec<serde_json::Value> {
    let mut doc = doc;
    // Last, exactly like `build_stdin_json` -- a body slot cannot shadow it.
    doc["params"] = assemble_params(over);
    let out = run_script_on_stdin(
        &assemble_script(),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
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
    if op == "bundle" {
        // GH #419: the phases that used to be a fan-in of two and three messages
        // are ONE bundle now -- the leg parks and the table is read back in the
        // same message, and the trailing select is what elects. A fixture that
        // used to name the firing phase names the bundle's phase instead and
        // puts its rows under the read-back's `tool_call_id`; what it measures
        // is the assembly, not the message boundary around it.
        let cid = read_back_id(phase);
        return serde_json::json!({
            "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0",
                                   "col_phase": phase, "store_origin": "collector"},
                       "hop": {"operation": "bundle", "rows_affected": rows_affected,
                               "bundle_errors": 0}},
            "messages": [{"origin": "tool", "type": "tool_result", "id": cid,
                          "text": payload.to_string()}],
            "results": [{"tool_call_id": cid, "operation": "select",
                         "rows_affected": rows_affected, "duration_ms": 0}]
        });
    }
    serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0",
                               "col_phase": phase, "store_origin": "collector"},
                   "hop": {"operation": op, "rows_affected": rows_affected}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "x",
                      "text": payload.to_string()}]
    })
}
/// The `tool_call_id` the phase reads its rows out of, for the phases that
/// became ONE bundle with GH #419.
/// How many messages this decision emitted, NOT counting the bookkeeping mark.
///
/// GH #419: a round that assembles emits the seam and, beside it, the `fired`
/// mark that records the round as answered. The mark used to be a guarded update
/// one hop in FRONT of the seam, and its `rows_affected` used to elect; now the
/// read-back elects and the mark only records, so it travels with the emission
/// instead of before it. Every "one message" claim below is about the message
/// that DOES something, which is what it always was.
fn emitted(out: &[serde_json::Value]) -> usize {
    out.iter()
        .filter(|m| m["header"]["phase"] != "collect-done" && m["header"]["phase"] != "round-done")
        .count()
}

/// The reply of the ONE message a turn opens with (GH #419).
///
/// `turn-w` (the row), `turn-open` (the open-round check) and `win` (the
/// window) were three messages for one question: may this turn be assembled,
/// and out of which window. They are one bundle now, so a fixture names the
/// legs it wants to answer. `round` empty means no open round -- the turn
/// assembles; `window` is what the window read came back with.
fn open_reply(round_rows: serde_json::Value, window: serde_json::Value) -> serde_json::Value {
    let legs = [("c-open-round", round_rows), ("c-open-win", window)];
    serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0",
                               "col_phase": "turn-open", "store_origin": "collector"},
                   "hop": {"operation": "bundle", "rows_affected": 1,
                           "bundle_errors": 0}},
        "messages": legs.iter().map(|(id, rows)| serde_json::json!(
            {"origin": "tool", "type": "tool_result", "id": id, "text": rows.to_string()}))
            .collect::<Vec<_>>(),
        "results": legs.iter().map(|(id, _)| serde_json::json!(
            {"tool_call_id": id, "operation": "select", "rows_affected": 1,
             "duration_ms": 0})).collect::<Vec<_>>()
    })
}

fn read_back_id(phase: &str) -> &'static str {
    match phase {
        "collect" => "c-collect-read",
        "round-check" => "c-round-check-read",
        other => panic!("no read-back id for phase `{other}`"),
    }
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

/// A materialised `leg-window` row, as the `turn-open` step writes it.
///
/// GH #889: the leg carries the round's turns and its deferral bookkeeping and
/// nothing of a cap -- `bytes`, `dropped` and `capped` went with the window.
fn leg_window_row(turns: serde_json::Value) -> serde_json::Value {
    let payload = serde_json::json!({"turns": turns, "deferred": 0,
                                     "deferred_turns": []});
    serde_json::json!({"turn_id": "t1", "iter": 0, "role": "leg-window",
                       "turn": payload.to_string(), "fired": 0})
}

/// The readable half of a bundle in the form `memory-hive@2.3.0` renders it: an
/// ASSERTING opening line (#279) and one section per kind (#281).
const READABLE: &str = "WHAT THIS MEMORY HOLDS (as of 2026-08-16)\n\
                        FACTS (extracted, canonical, dated)\n  \
                        alex editor = the editor is helix   since 2026-08-01";

/// The machine-readable half beside it, with the slim payload candidates of
/// #296 — no row ids, no fused scores, no legs.
const BUNDLE_JSON: &str = "{\"answers\": \"direct\", \"as_of\": \"2026-08-16T00:00:00Z\", \
                           \"query\": \"and my editor?\", \"tier\": 0}";

/// The one sentence a bundle with no candidate says about itself (#297).
const EMPTY_STATE: &str = "Nothing in this memory answers this question (as of 2026-08-16).";

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
    assert_eq!(
        emitted(&out),
        1,
        "no memory leg configured: one emission only"
    );
    assert_eq!(out[0]["header"]["route"], "cstore");
    // GH #419: the row, the open-round check and the window in ONE message. The
    // INSERT is the first call, which is what keeps "written before the window
    // is read" true -- a bundle's ops run in order over the store's one
    // connection, so the window contains this very turn.
    assert_eq!(out[0]["header"]["phase"], "turn-open");
    let calls: Vec<serde_json::Value> = out[0]["messages"]
        .as_array()
        .expect("calls")
        .iter()
        .map(|t| serde_json::from_str(t["text"].as_str().expect("op text")).expect("args"))
        .collect();
    // GH #889: the per-turn episode scan left with `turn_write` -- episodes are
    // the curator's to write now -- so the bundle ends on the legend. OR-KX-63/68:
    // the departures' own cut and the two age reads (turns, round) run in front
    // of the open-round check, and the session's departures are read behind the
    // round read.
    assert_eq!(
        calls.iter().map(|a| a["table"].clone()).collect::<Vec<_>>(),
        [
            "turns", "round", "turns", "round", "round", "turns", "round", "session", "roster"
        ],
        "the row, the departures' cut, the two age reads, the open-round check, the \
         round read, the departures, the session row and the legend (GH #845/#847): \
         {calls:?}"
    );
    let op = calls[0].clone();
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
    // answered. The question is asked BEFORE the window is read, and since
    // GH #419 both are calls of the ONE message the turn opens with: the ORDER
    // is what this pin is about, and a bundle's ops run in call order over the
    // store's one connection.
    let out = emit(lane_doc(
        "in_turn",
        serde_json::json!([{"origin": "user", "type": "text", "text": "hello"}]),
    ));
    assert_eq!(emitted(&out), 1, "one message opens the turn: {out:?}");
    assert_eq!(out[0]["header"]["phase"], "turn-open");
    let calls: Vec<serde_json::Value> = out[0]["messages"]
        .as_array()
        .expect("calls")
        .iter()
        .map(|t| serde_json::from_str(t["text"].as_str().expect("op text")).expect("args"))
        .collect();
    // Four calls since GH #298: the row, the round check, the window, and the
    // per-turn episode scan. Six since GH #845/#847: the session row and the
    // legend are read beside the window. Five since GH #889: the episode scan
    // went with `turn_write` (the curator writes episodes now). Nine since
    // OR-KX-63/68: the departures' cut and the two age reads in front of the
    // check and the read of the session's departures behind the window.
    assert_eq!(calls.len(), 9, "{calls:?}");
    let ids: Vec<&str> = out[0]["messages"]
        .as_array()
        .expect("calls")
        .iter()
        .map(|t| t["id"].as_str().expect("call id"))
        .collect();
    let at = |id: &str| {
        ids.iter()
            .position(|i| *i == id)
            .unwrap_or_else(|| panic!("no `{id}` in {ids:?}"))
    };
    assert!(
        at("c-open-turn") < at("c-open-round") && at("c-open-round") < at("c-open-win"),
        "the row, then the check, then the window: {ids:?}"
    );
    let check = calls[at("c-open-round")].clone();
    assert_eq!(check["operation"], "select");
    assert_eq!(check["table"], "round");
    assert_eq!(check["where"]["session_id"], "s1");
    assert_eq!(check["where"]["role"], "assistant");
    assert_eq!(
        check["where"]["fired"], 0,
        "open means: the round has not answered"
    );
}

#[test]
fn the_round_read_is_session_scoped_and_carries_no_turn_cap() {
    // No open round: the chain continues exactly as before #103.
    //
    // GH #889: this read used to carry `window_turns` into the store as a
    // `limit`, newest first. The window is the curator's now (R-27-1), so the
    // read takes the session's rows oldest first and uncut, and the script keeps
    // the round's own turn and the deferred ones riding in it (`round_turns`).
    let out = emit(lane_doc(
        "in_turn",
        serde_json::json!([{"origin": "user", "type": "text", "text": "hello"}]),
    ));
    // Found by its id, not its index: OR-KX-68 put three ops in front.
    let op: serde_json::Value = serde_json::from_str(
        out[0]["messages"]
            .as_array()
            .expect("calls")
            .iter()
            .find(|t| t["id"] == "c-open-win")
            .expect("the window call")["text"]
            .as_str()
            .expect("the window call"),
    )
    .expect("op args");
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "turns");
    assert!(
        op["columns"]
            .as_array()
            .expect("columns")
            .iter()
            .any(|c| c == "deferred"),
        "the round read carries the deferral stamp along"
    );
    assert_eq!(
        op["where"]["session_id"], "s1",
        "the round read is session-scoped"
    );
    assert_eq!(op["order_by"][0]["col"], "id");
    assert_eq!(op["order_by"][0]["dir"], "asc");
    assert!(
        op["limit"].is_null(),
        "no turn cap reaches the store -- the collector cuts nothing (GH #889): {op}"
    );
}

#[test]
fn the_window_leg_is_chronological_and_carries_both_roles() {
    // Whatever order the store answers in, a round is read oldest first, and
    // BOTH roles are in it -- an assistant turn the agent cannot see is how a
    // conversation loses its own thread. (GH #889: the read is ascending now,
    // and the script still sorts by id rather than trust the reply's order.)
    let rows = serde_json::json!([
        turn_row("3", "user", "third"),
        turn_row("2", "assistant", "second"),
        turn_row("1", "user", "first")
    ]);
    let out = emit(open_reply(serde_json::json!([]), rows));
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
    let rows = serde_json::json!([leg_window_row(serde_json::json!([]))]);
    let out = emit(reply_doc("collect", "bundle", 1, rows));
    assert_eq!(emitted(&out), 1);
    // GH #419: what a complete read-back produces is the ASSEMBLY itself, not a
    // guarded update to win the right to produce it. The election is the
    // trailing select of this very bundle -- of two legs parking concurrently
    // exactly one reads a complete set -- so the three messages the decision
    // used to cost (`gate`, `fire-guard`, `fire`) are gone with it.
    // GH #889: the seam is `curate` -- the curator stands between the
    // collector and the brain.
    assert_eq!(
        out[0]["header"]["route"], "curate",
        "a complete round assembles: {}",
        out[0]
    );
}

#[test]
fn a_configured_memory_leg_is_waited_for() {
    let over = [("memory_tier", "0")];
    let rows = serde_json::json!([leg_window_row(serde_json::json!([]))]);
    let out = emit_with(&over, reply_doc("collect", "bundle", 1, rows));
    assert!(
        out.is_empty(),
        "with the memory leg on, a window-only gate is incomplete and terminal"
    );
    let both = serde_json::json!([
        leg_window_row(serde_json::json!([])),
        {"turn_id": "t1", "iter": 0, "role": "leg-memory", "turn": "{}", "fired": 0}
    ]);
    let out = emit_with(&over, reply_doc("collect", "bundle", 2, both));
    assert_eq!(emitted(&out), 1, "both legs in: the round assembles");
}

#[test]
fn a_lost_election_emits_nothing() {
    // GH #419: the same property, elected differently. What `rows_affected` on a
    // guarded update used to say -- "somebody else owns the fire" -- the
    // trailing select of a bundle says by coming back INCOMPLETE: the other leg
    // had not parked yet when this hop read. Whoever reads a complete set is the
    // one that assembles, and there is exactly one of those.
    let out = emit(reply_doc("collect", "bundle", 0, serde_json::json!([])));
    assert!(
        out.is_empty(),
        "an incomplete read-back means another hop owns the fire"
    );
    let rows = serde_json::json!([leg_window_row(serde_json::json!([]))]);
    let out = emit(reply_doc("collect", "bundle", 1, rows.clone()));
    assert_eq!(emitted(&out), 1, "a complete read-back assembles");
    // GH #889: the seam is `curate`, not `brain`.
    assert_eq!(out[0]["header"]["route"], "curate");

    // ... and exactly once: a leg the store handed back TWICE is a redelivery,
    // not a complete set, and firing on it would assemble the same turn twice.
    let twice = serde_json::json!([
        leg_window_row(serde_json::json!([])),
        leg_window_row(serde_json::json!([]))
    ]);
    assert!(
        emit(reply_doc("collect", "bundle", 2, twice)).is_empty(),
        "a redelivered leg must park"
    );
}

// ========================================================================= SEAM

#[test]
fn the_brain_is_handed_one_assembled_context_over_one_route() {
    let turns = serde_json::json!([
        {"role": "user", "text": "first"},
        {"role": "assistant", "text": "second"},
        {"role": "user", "text": "what did i say first?"}
    ]);
    let rows = serde_json::json!([leg_window_row(turns)]);
    let out = emit(reply_doc("collect", "bundle", 1, rows));
    assert_eq!(
        emitted(&out),
        1,
        "ONE seam: one message, one route, one curator edge"
    );
    let msg = &out[0];
    // GH #889: the seam is `curate`; the curator behind it calls the brain.
    assert_eq!(msg["header"]["route"], "curate");
    assert_eq!(
        texts_of(msg),
        vec!["first", "second", "what did i say first?"]
    );
    assert_eq!(msg["messages"][1]["origin"], "assistant");
    // GH #889: there is no eviction policy here any more, so nothing is cut --
    // but the marks stay on the seam, zero-valued, because a CEL modifier that
    // reads a missing key fails and a failed modifier skips the edge.
    assert_eq!(msg["header"]["window_turns"], "3");
    assert_eq!(msg["header"]["window_dropped"], "0");
    assert_eq!(msg["header"]["window_capped"], "0");
    assert_eq!(msg["header"]["iter"], "0", "the first call of the turn");
}

/// The memory leg end to end: asked once per turn, filed as a leg, and handed
/// on VERBATIM (GH #889 removed its cap, `memory_chars`: the curator owns the
/// window).
///
/// The last third moved with GH #278. The bundle used to reach the brain in
/// `system.memory` — durable state, upserted per slot path, sitting where the
/// agent's instructions live. It now reaches it as what it is: the result of a
/// `memory_recall` call, at the end of the round, in the round's own budget.
/// The two halves of the claim are unchanged — the collector renders nothing of
/// its own, and `memory_form` chooses which form travels — only the channel is
/// different, and `gh278_the_ambient_recall_is_a_tool_result.rs` is where the
/// channel itself is pinned.
#[test]
fn the_memory_bundle_enters_through_the_collector_and_verbatim() {
    let over = [("memory_tier", "0")];
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
        "system": {"memory": {"bundle": {"text": BUNDLE_JSON}}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                      "text": READABLE}]
    });
    let out = emit_with(&over, bundle);
    let op = op_of(&out[0]);
    assert_eq!(op["row"]["role"], "leg-memory");

    // And it reaches the brain as the answer to a call, not as durable state.
    let leg_memory = serde_json::json!({
        "turn_id": "t1", "iter": 0, "role": "leg-memory",
        "turn": op["row"]["turn"], "fired": 0
    });
    let rows = serde_json::json!([
        leg_window_row(serde_json::json!([{"role": "user", "text": "and my editor?"}])),
        leg_memory
    ]);
    let out = emit_with(&over, reply_doc("collect", "bundle", 2, rows.clone()));
    assert_eq!(emitted(&out), 1);
    let msg = &out[0];
    let msgs = msg["messages"].as_array().expect("messages");
    assert_eq!(msgs.len(), 3, "the conversation, then the pair: {msg}");
    assert_eq!(msgs[0]["text"], "and my editor?");
    assert_eq!(msgs[1]["type"], "tool_call");
    assert_eq!(msgs[2]["type"], "tool_result");
    assert_eq!(msgs[2]["id"], msgs[1]["id"], "a result answers its call");
    assert_eq!(
        msgs[2]["text"], READABLE,
        "the readable form the memory hive rendered, byte for byte: {msg}"
    );
    assert_eq!(
        msg["system"]["memory"]["recall"]["text"], "",
        "and nothing of it stays behind in durable state (GH #278): {msg}"
    );

    // The machine-readable half is a configuration choice, not a second render.
    let out = emit_with(
        &[("memory_tier", "0"), ("memory_form", "json")],
        reply_doc("collect", "bundle", 2, rows),
    );
    let msgs = out[0]["messages"].as_array().expect("messages");
    let last: serde_json::Value =
        serde_json::from_str(msgs[msgs.len() - 1]["text"].as_str().expect("result text"))
            .expect("the json form is json");
    assert_eq!(
        last,
        serde_json::json!({"bundle": {"text": BUNDLE_JSON}}),
        "under `json` the same channel carries the other form: {}",
        out[0]
    );
}

/// GH #259, re-pointed by GH #278: a recall that came back with nothing must
/// not leave the previous turn's bundle standing in the prompt.
///
/// The bug #259 named is closed twice over now. The bundle no longer LIVES in
/// `system.memory`, so a stale one cannot survive there in the first place —
/// and the fixed path is sent empty every turn regardless, because a brain that
/// was written to by an older collector still carries whatever stood there
/// last (`system.*` is upserted per slot path; a path nobody sends is a path
/// nobody touches).
///
/// What replaces the old assertion is the honest one: an empty leg still
/// produces a PAIR, and its result says so in words. A turn where memory was
/// asked and answered nothing is a different fact from a turn where memory was
/// never asked, and the model has to be able to tell them apart.
///
/// Two rounds, and the second one is the test.
#[test]
fn a_recall_that_found_nothing_overwrites_the_bundle_of_the_turn_before() {
    let over = [("memory_tier", "0")];
    let window = leg_window_row(serde_json::json!([{"role": "user", "text": "and my editor?"}]));
    let leg = |payload: serde_json::Value| {
        serde_json::json!({"turn_id": "t1", "iter": 0, "role": "leg-memory",
                           "turn": payload.to_string(), "fired": 0})
    };
    let last_text = |out: &[serde_json::Value]| {
        let msgs = out[0]["messages"].as_array().expect("messages").clone();
        msgs[msgs.len() - 1]["text"]
            .as_str()
            .expect("result text")
            .to_string()
    };

    let full = leg(serde_json::json!({
        "system": {},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                      "text": READABLE}]
    }));
    let first = emit_with(
        &over,
        reply_doc(
            "collect",
            "bundle",
            2,
            serde_json::json!([window.clone(), full]),
        ),
    );
    assert_eq!(last_text(&first), READABLE);

    // Round two: the leg fired and came back with nothing at all.
    let empty = leg(serde_json::json!({"system": {}, "messages": []}));
    let second = emit_with(
        &over,
        reply_doc(
            "collect",
            "bundle",
            2,
            serde_json::json!([window.clone(), empty]),
        ),
    );
    assert_eq!(
        last_text(&second),
        "memory recall returned nothing",
        "an empty leg still answers its call -- a tool_call left unanswered is \
         a malformed turn for every provider: {}",
        second[0]
    );
    assert_eq!(
        second[0]["system"]["memory"]["recall"]["text"], "",
        "and the fixed path is still revoked, whatever an older collector left \
         standing there: {}",
        second[0]
    );

    // Round three: the empty state a `memory-hive@2.3.0` actually emits — one
    // sentence in the payload, `answers: none` beside it. It travels as itself;
    // the collector renders nothing of its own here either.
    let sentence = leg(serde_json::json!({
        "system": {"memory": {"bundle": {"text":
            "{\"answers\": \"none\", \"as_of\": \"2026-08-16T00:00:00Z\", \
              \"candidates\": [], \"query\": \"and my editor?\"}"}}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                      "text": EMPTY_STATE}]
    }));
    let third = emit_with(
        &over,
        reply_doc(
            "collect",
            "bundle",
            2,
            serde_json::json!([window, sentence]),
        ),
    );
    assert_eq!(last_text(&third), EMPTY_STATE, "{}", third[0]);
}

/// GH #266, the half #259 could not reach: the `json` form of the recall
/// bundle. Its sub-keys are named by the memory hive per bundle, so the
/// collector cannot name the previous turn's paths empty — an upsert per slot
/// path leaves a key nobody sends standing in the prompt. The repair is the GH
/// #264 marker on the ONE node the collector owns, `system.memory`: below it,
/// exactly what this message carries holds.
///
/// GH #278 makes the same guarantee STRONGER and the test says so: no
/// hive-named key reaches `system` at all any more, because the bundle no
/// longer travels there. The marker stays, and it is not redundant — a brain
/// written to by an older collector, or by any other writer that ever put a key
/// under this node, is cleared by exactly this message and by nothing else.
///
/// Two rounds, and the second one is the test — a pin on the first round is
/// green with and without the repair, the same lesson as GH #259.
#[test]
fn a_json_key_the_next_turn_does_not_name_is_revoked_with_the_bundle() {
    let over = [("memory_tier", "0"), ("memory_form", "json")];
    let window = leg_window_row(serde_json::json!([{"role": "user", "text": "and my editor?"}]));
    let leg = |payload: serde_json::Value| {
        serde_json::json!({"turn_id": "t1", "iter": 0, "role": "leg-memory",
                           "turn": payload.to_string(), "fired": 0})
    };

    // Round one: the hive names two keys.
    let two = leg(serde_json::json!({
        "system": {"memory": {"bundle": {"text": BUNDLE_JSON},
                              "answer": {"text": "helix"}}},
        "messages": []
    }));
    let first = emit_with(
        &over,
        reply_doc(
            "collect",
            "bundle",
            2,
            serde_json::json!([window.clone(), two]),
        ),
    );
    assert!(
        first[0]["system"]["memory"]["answer"].is_null(),
        "a hive-named key has no business in durable state (GH #278): {}",
        first[0]
    );

    // Round two: the same leg, one key. The other is not named and cannot be.
    let one = leg(serde_json::json!({
        "system": {"memory": {"bundle": {"text": BUNDLE_JSON}}},
        "messages": []
    }));
    let second = emit_with(
        &over,
        reply_doc("collect", "bundle", 2, serde_json::json!([window, one])),
    );
    let mem = &second[0]["system"]["memory"];
    assert_eq!(
        mem["$replace"],
        serde_json::json!(true),
        "without the marker the brain keeps `memory.answer` from whoever wrote \
         it, and the collector has no path to name it empty: {}",
        second[0]
    );
    assert!(
        mem.get("answer").is_none(),
        "the collector must not invent the key it is revoking: {}",
        second[0]
    );
    // Where the bundle went instead.
    let msgs = second[0]["messages"].as_array().expect("messages");
    assert_eq!(msgs[msgs.len() - 1]["type"], "tool_result", "{}", second[0]);
}

/// GH #266 — the marker covers BOTH legs, because both hang under the same
/// node. One marker, no second one, and the `text` form keeps the fixed path
/// GH #259 gave it.
///
/// GH #278 emptied both legs of their data: under every form the node now holds
/// the revocation and nothing else, and the bundle — in whichever form
/// `memory_form` selects — is the tool result at the end of the round. The
/// marker and the fixed leaf are what remains, and what they cover is the same
/// subtree as before.
#[test]
fn under_both_forms_one_marker_covers_the_whole_memory_subtree() {
    let over = [("memory_tier", "0"), ("memory_form", "both")];
    let window = leg_window_row(serde_json::json!([{"role": "user", "text": "and my editor?"}]));
    let leg = |payload: serde_json::Value| {
        serde_json::json!({"turn_id": "t1", "iter": 0, "role": "leg-memory",
                           "turn": payload.to_string(), "fired": 0})
    };

    let two = leg(serde_json::json!({
        "system": {"memory": {"bundle": {"text": BUNDLE_JSON},
                              "answer": {"text": "helix"}}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                      "text": READABLE}]
    }));
    let first = emit_with(
        &over,
        reply_doc(
            "collect",
            "bundle",
            2,
            serde_json::json!([window.clone(), two]),
        ),
    );
    assert!(
        first[0]["system"]["memory"]["answer"].is_null(),
        "{}",
        first[0]
    );
    assert_eq!(
        first[0]["system"]["memory"]["recall"]["text"], "",
        "the fixed leaf is a revocation now, not a rendering: {}",
        first[0]
    );

    let one = leg(serde_json::json!({
        "system": {"memory": {"bundle": {"text": BUNDLE_JSON}}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                      "text": READABLE}]
    }));
    let second = emit_with(
        &over,
        reply_doc("collect", "bundle", 2, serde_json::json!([window, one])),
    );
    let mem = &second[0]["system"]["memory"];
    assert_eq!(
        mem["$replace"],
        serde_json::json!(true),
        "one marker above both legs, or a stale key from any writer stands: {}",
        second[0]
    );
    assert!(mem.get("answer").is_none(), "{}", second[0]);
    assert!(
        mem["recall"].get("$replace").is_none(),
        "the readable leaf carries no marker of its own: {}",
        second[0]
    );
    // Both forms, one channel: `both` concatenates them into the ONE result.
    let msgs = second[0]["messages"].as_array().expect("messages");
    let result = msgs[msgs.len() - 1]["text"].as_str().expect("result text");
    let tail = result
        .strip_prefix(&format!("{READABLE}\n"))
        .unwrap_or_else(|| panic!("the readable half comes first: {result}"));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(tail).expect("the json half is json"),
        serde_json::json!({"bundle": {"text": BUNDLE_JSON}}),
        "{}",
        second[0]
    );
}

/// GH #266 counter-pin, and the one that matters more than the first: a
/// replace that reaches too far is worse than the defect it cures. The marker
/// sits on `system.memory` — the node the collector fills wholesale every turn
/// — and on nothing else. `system.consult` is a fixed path the collector
/// revokes the GH #259 way and must stay untouched; the `system` node itself
/// carries the EMPTY root, which would revoke every other writer's slot in the
/// brain, `system.instructions` and `system.handover` included.
#[test]
fn the_marker_sits_on_the_memory_node_and_on_nothing_else() {
    let over = [("memory_tier", "0"), ("memory_form", "both")];
    let rows = serde_json::json!([
        leg_window_row(serde_json::json!([{"role": "user", "text": "and my editor?",
                                "consult_id": "c1"}])),
        {"turn_id": "t1", "iter": 0, "role": "leg-memory", "fired": 0,
         "turn": serde_json::json!({
             "system": {"memory": {"bundle": {"text": "{}"}}},
             "messages": [{"origin": "tool", "type": "tool_result",
                           "id": "recall", "text": "MEMORY (tier 0)"}]
         }).to_string()}
    ]);
    let out = emit_with(&over, reply_doc("collect", "bundle", 2, rows));
    let sys = &out[0]["system"];
    assert_eq!(sys["memory"]["$replace"], serde_json::json!(true));
    assert!(
        sys.get("$replace").is_none(),
        "a marker on the `system` node has the EMPTY root and revokes every \
         slot in the brain, including the ones the collector never wrote: {}",
        out[0]
    );
    assert_eq!(
        sys["consult"]["open"],
        serde_json::json!(["c1"]),
        "the consult slot travels unchanged (GH #259): {}",
        out[0]
    );
    assert!(
        sys["consult"].get("$replace").is_none(),
        "a fixed path needs no marker, and one here would revoke a subtree the \
         collector does not own: {}",
        out[0]
    );
}

/// GH #266 — the marker as the honest pure revocation, RE-POINTED by GH #278.
///
/// The `json` form was the case that needed it: no fixed path to send empty, so
/// a leg that found nothing had nothing to write and everything to withdraw.
/// Since #278 that is the case for EVERY form and every leg — the node carries
/// no data at all any more — and the revocation is therefore one literal rather
/// than something assembled out of what the hive returned.
///
/// The empty leaf travels with it under every form, and that is a change: it
/// used to be the `readable` leg's own path, which made its owner depend on a
/// knob. A fixed path does not change owner per instance, and an instance
/// retuned from `readable` to `json` would otherwise carry its last rendering
/// for the rest of its life.
#[test]
fn a_bundle_that_found_nothing_revokes_under_every_form() {
    for form in ["readable", "json", "both"] {
        let over = [("memory_tier", "0"), ("memory_form", form)];
        let rows = serde_json::json!([
            leg_window_row(serde_json::json!([{"role": "user", "text": "and my editor?"}])),
            {"turn_id": "t1", "iter": 0, "role": "leg-memory", "fired": 0,
             "turn": serde_json::json!({"system": {}, "messages": []}).to_string()}
        ]);
        let out = emit_with(&over, reply_doc("collect", "bundle", 2, rows));
        assert_eq!(
            out[0]["system"]["memory"],
            serde_json::json!({"recall": {"text": ""}, "$replace": true}),
            "under `memory_form` {form} the node is a revocation and nothing \
             else: {}",
            out[0]
        );
    }
}

/// GH #266 — the marker is the collector's own statement about a node it owns,
/// so a bundle key of the same name cannot overwrite it with data. `$` is the
/// substrate's reserved namespace inside a system subtree.
///
/// GH #278 turned the protection from an ORDERING into a structural one, and
/// that is worth keeping a test on: the marker used to be stamped after the
/// legs, so a hive emitting `$replace: false` was beaten by one line of
/// sequencing. Now nothing a hive emits reaches this node at all — the slot is
/// a literal — and a bundle that names the marker cannot even be considered.
#[test]
fn a_bundle_key_cannot_overwrite_the_marker() {
    let over = [("memory_tier", "0"), ("memory_form", "json")];
    let rows = serde_json::json!([
        leg_window_row(serde_json::json!([{"role": "user", "text": "and my editor?"}])),
        {"turn_id": "t1", "iter": 0, "role": "leg-memory", "fired": 0,
         "turn": serde_json::json!({
             "system": {"memory": {"$replace": false,
                                   "bundle": {"text": "{}"}}},
             "messages": []
         }).to_string()}
    ]);
    let out = emit_with(&over, reply_doc("collect", "bundle", 2, rows));
    assert_eq!(
        out[0]["system"]["memory"]["$replace"],
        serde_json::json!(true),
        "a bundle that names the marker must not be able to disarm it: {}",
        out[0]
    );
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
    let out = emit(reply_doc("round-check", "bundle", 3, full.clone()));
    assert_eq!(emitted(&out), 1, "the completed round fires: {out:?}");
    // GH #419: the closing mark travels WITH the seam. It is per ITERATION, as
    // the guard it replaced was -- a later iteration of the same turn is a
    // different round and closes itself.
    let op = op_of(out.last().expect("the closing mark"));
    assert_eq!(op["operation"], "update");
    assert_eq!(op["where"]["role"], "assistant");
    assert_eq!(
        op["where"]["iter"], 0,
        "the mark is per ITERATION, not per turn"
    );

    // Re-entry: the window travels with the tool round, through the same seam.
    let mut rows = full.as_array().expect("rows").clone();
    rows.push(leg_window_row(
        serde_json::json!([{"role": "user", "text": "the question"}]),
    ));
    let out = emit(reply_doc(
        "round-check",
        "bundle",
        4,
        serde_json::Value::Array(rows),
    ));
    assert_eq!(emitted(&out), 1);
    let msg = &out[0];
    // GH #889: the seam is `curate`, for the first call and the re-entry alike.
    assert_eq!(
        msg["header"]["route"], "curate",
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

// ============================================ WHAT A RESULT MAY CARRY (#252)

/// A tool result is its `messages[]` -- all of it.
///
/// Two `tool_result` turns in ONE message answer two calls, and the fan-in has
/// to see both. The lane used to keep `messages[0]` and nothing else, so the
/// second call stayed open and the round waited for a result that had already
/// arrived until `round_idle_ms` expired and a synthetic stand-in closed it.
/// The row the lane writes is fed straight back into the round check here: what
/// the lane records IS what the fan-in reads, so the two cannot disagree.
#[test]
fn a_result_that_answers_two_calls_in_one_message_closes_both() {
    let calls = serde_json::json!([
        {"origin": "assistant", "type": "tool_call", "id": "c1", "text": "{}"},
        {"origin": "assistant", "type": "tool_call", "id": "c2", "text": "{}"}
    ]);
    let out = emit(lane_doc(
        "in_tool",
        serde_json::json!([
            {"origin": "tool", "type": "tool_result", "id": "c1", "text": "a"},
            {"origin": "tool", "type": "tool_result", "id": "c2", "text": "b"}
        ]),
    ));
    assert_eq!(emitted(&out), 1, "one result, one row: {out:?}");
    let row = op_of(&out[0])["row"].clone();
    assert_eq!(row["role"], "tool");
    let stored: serde_json::Value =
        serde_json::from_str(row["turn"].as_str().expect("turn")).expect("stored turn");
    assert_eq!(
        stored.as_array().map(|a| a.len()),
        Some(2),
        "both turns of the result are in the row: {stored}"
    );

    let asst = serde_json::json!({"turn_id": "t1", "iter": 0, "role": "assistant",
                                  "turn": calls.to_string(), "fired": 0});
    let out = emit(reply_doc(
        "round-check",
        "select",
        2,
        serde_json::json!([asst, row]),
    ));
    assert!(
        !out.is_empty(),
        "the round parked on a call that was answered in the same breath"
    );
    assert_eq!(emitted(&out), 1, "the round fired: {out:?}");
    // GH #419: the closing mark travels WITH the seam instead of one hop in
    // front of it. It is no longer a guard -- nothing reads its `rows_affected`
    // -- but it still records that this round has answered.
    let op = op_of(out.last().expect("the closing mark"));
    assert_eq!(op["operation"], "update");
    assert_eq!(op["where"]["role"], "assistant");
    assert_eq!(op["set"]["fired"], 1, "the round fired: {op}");
}

/// And nothing else: a `system` slot on a tool result stays at the door.
///
/// `in_bundle` keeps `system` and is not the precedent it looks like. What
/// leaves the seam in `system.*` is UPSERTed into the brain cell's own `cell.db`
/// and stands in the prompt until something overwrites that exact slot path, so
/// it is durable state of the agent rather than evidence of one round. The
/// recall bundle survives that treatment because it is re-sent under a fixed
/// path on every turn; a single tool result gets no second chance to correct
/// itself, and a brief about one subject would still be there three subjects
/// later. A tool with something to say says it in the text of its result.
#[test]
fn a_tool_result_leaves_its_system_slot_at_the_door() {
    let mut doc = lane_doc(
        "in_tool",
        serde_json::json!([{"origin": "tool", "type": "tool_result", "id": "c1",
                            "text": "the receipt line"}]),
    );
    doc["system"] = serde_json::json!({"identity": {"text": "a durable claim"}});
    let out = emit(doc);
    assert_eq!(emitted(&out), 1);
    let stored: serde_json::Value =
        serde_json::from_str(op_of(&out[0])["row"]["turn"].as_str().expect("turn"))
            .expect("stored turn");
    assert_eq!(
        stored["text"], "the receipt line",
        "the result itself is untouched: {stored}"
    );
    assert!(
        !stored.to_string().contains("a durable claim"),
        "the system slot rode into the round row: {stored}"
    );
}

// ================================================ NO CAP AT THE SEAM (#91, #889)
//
// GH #91 capped what reached the brain; GH #889 gave the window to the curator
// behind `curate` (R-27-1), so the collector hands every item on whole.

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

/// The same pair with a tool NAME on the call, in the shape a provider writes
/// one: the `function` object IS the text of a `tool_call` turn, so the name is
/// read out of it and nowhere else.
fn named_round_pair(iter: i64, id: &str, name: &str, result: &str) -> Vec<serde_json::Value> {
    let call = serde_json::json!([
        {"origin": "assistant", "type": "tool_call", "id": id,
         "text": serde_json::json!({"name": name, "arguments": "{}"}).to_string()}
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
fn a_huge_tool_result_stays_whole_in_the_store_and_on_the_seam() {
    // First half, unchanged since #91: what the lane writes is NOT cut, so the
    // full text stays addressable.
    //
    // GH #889: the second half used to pin the `tool_chars` preview on the way
    // to the brain. The knob is gone -- the curator owns the window (R-27-1) --
    // so the same row now reaches `curate` whole.
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

    // Second half: the same row on its way to the curator is whole.
    let mut rows = round_pair(0, "c1", &huge);
    rows.push(leg_window_row(
        serde_json::json!([{"role": "user", "text": "q"}]),
    ));
    let out = emit(reply_doc(
        "round-check",
        "bundle",
        3,
        serde_json::Value::Array(rows),
    ));
    assert_eq!(emitted(&out), 1);
    assert_eq!(out[0]["header"]["route"], "curate");
    let texts = texts_of(&out[0]);
    assert_eq!(texts.len(), 3, "round turn + call + result: {texts:?}");
    assert_eq!(
        texts[2].len(),
        100_000,
        "no per-item cap runs in front of the seam any more (GH #889)"
    );
    assert_eq!(
        out[0]["header"]["round_capped"], "0",
        "nothing was capped, and the seam says so"
    );
}

#[test]
fn an_uncapped_round_says_so() {
    let mut rows = vec![leg_window_row(serde_json::json!([]))];
    rows.extend(round_pair(0, "c1", "short"));
    let out = emit(reply_doc(
        "round-check",
        "bundle",
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
    )];
    rows.extend(round_pair(1, "c1", "found"));
    let rows = serde_json::Value::Array(rows);

    // Under the cap the seam is what it always was.
    let out = emit_with(
        &[("max_iter", "2")],
        reply_at("round-check", "bundle", 3, rows.clone(), 1),
    );
    assert_eq!(emitted(&out), 1);
    // GH #889: the seam under the cap is `curate`.
    assert_eq!(out[0]["header"]["route"], "curate");
    assert_eq!(out[0]["header"]["iter"], "2");

    // At the cap the SAME phase leaves through the answer lane instead. The
    // round began at the seam, so the seam is what ends it -- no dispatcher
    // and no edge condition is needed to stop the loop (R-OS-2).
    // The cap bites at the iteration whose NEXT one would exceed it, so the
    // fixture carries a round that completes there.
    let mut capped = rows.as_array().expect("rows").clone();
    capped.extend(round_pair(2, "c2", "found again"));
    let out = emit_with(
        &[("max_iter", "2")],
        reply_at(
            "round-check",
            "bundle",
            4,
            serde_json::Value::Array(capped),
            2,
        ),
    );
    // GH #889 (OR-KX-V2): the answer ends the round, so its rows leave with it
    // -- one `round-drop` bundle beside the seam, bookkeeping like the mark.
    assert_eq!(
        emitted(&out),
        2,
        "one seam, and it is not a curator call -- plus the drop: {out:?}"
    );
    assert_eq!(out[0]["header"]["route"], "answer");
    assert_eq!(out[0]["header"]["round_capped"], "1");
    assert_eq!(out[1]["header"]["phase"], "round-drop", "{out:?}");
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "curate"),
        "a capped turn asks nothing more"
    );
    let texts = texts_of(&out[0]);
    assert_eq!(
        texts[0], "look it up",
        "what was collected leaves with it: {texts:?}"
    );
    assert_eq!(
        texts.len(),
        6,
        "window turn + both iterations of the round so far + the partial \
         answer that closes it (GH #570): {texts:?}"
    );
}

/// GH #570: a capped round ends on a NAMED partial answer, not on the raw end
/// of the tool round -- and says so with a hop key of its own.
///
/// Measured on e18: `cogny` capped at its bound and its `answer` body ended
/// with a raw `web_search` `tool_result`; the surface in front of it takes the
/// LAST text (`any_text`) and writes it into the conversation, so the person
/// was shown a search payload. Nothing is lost by fixing it here -- the raw
/// round travelled to the curator on every `curate` of it (GH #889: the
/// collector drops its round rows when the answer leaves, and `thread_recall`
/// is gone). What changes is the last WORD of the projection, because that is
/// the one a consumer reads.
///
/// THE ZERO-CALL SHAPE IS NOT PINNED HERE, and that is a finding rather than a
/// gap: a spent round whose thread is empty cannot be reached through any
/// shipped lane. `round-check` is the only phase that assembles the seam, it
/// only fires on a round it read rows for, and with no round rows the fan-in
/// PARKS and emits nothing at all (measured against the shipped script: an
/// `iter=2`/`max_iter=2` reply carrying only the window row emits zero
/// messages). Nothing cuts a non-empty thread either -- GH #889 removed the
/// byte cap. The script still answers the shape
/// honestly (`PARTIAL_ANSWER_EMPTY`, "No tool call was made.", no invented
/// "last result") rather than printing three empty clauses, because an
/// unreachable branch is exactly the one nobody will read again.
#[test]
fn the_capped_round_ends_on_a_partial_answer_not_on_a_raw_tool_result() {
    let mut rows = vec![leg_window_row(
        serde_json::json!([{"role": "user", "text": "look it up"}]),
    )];
    rows.extend(named_round_pair(1, "c1", "web_search", "found"));
    rows.extend(named_round_pair(
        2,
        "c2",
        "web_search",
        "{\"results\": [\"raw json the person must not see\"]}",
    ));
    let out = emit_with(
        &[("max_iter", "2")],
        reply_at(
            "round-check",
            "bundle",
            4,
            serde_json::Value::Array(rows),
            2,
        ),
    );
    // GH #889 (OR-KX-V2): the answer and the `round-drop` bundle that takes the
    // round's rows with it.
    assert_eq!(emitted(&out), 2, "{out:?}");
    assert_eq!(out[1]["header"]["phase"], "round-drop", "{out:?}");
    assert_eq!(out[0]["header"]["route"], "answer");
    assert_eq!(out[0]["header"]["round_capped"], "1");
    assert_eq!(
        out[0]["header"]["partial"], "1",
        "the round ended early, and a guard can tell that from trimmed bytes"
    );

    let texts = texts_of(&out[0]);
    let last = texts.last().expect("a last turn");
    assert!(
        last.contains("iteration cap") && last.contains("max_iter=2"),
        "the partial answer names its own bound: {last}"
    );
    assert!(
        !last.starts_with("{\"results\""),
        "the raw tool result is not the last word: {last}"
    );
    assert!(
        last.contains("web_search"),
        "the digest names what the round called: {last}"
    );
    assert!(
        last.contains("2 tool call(s) -- web_search"),
        "how many times it called, and the SEPARATOR: the digest is 7-bit ASCII \
         like the rest of this script, so the dash is `--` and not an em dash \
         a surface might render its own way: {last}"
    );
    let msgs = out[0]["messages"].as_array().expect("messages");
    let closing = msgs.last().expect("a last message");
    assert_eq!(closing["origin"], "assistant");
    assert_eq!(closing["type"], "text");
}

/// GH #894 (R-27-3, OR-KY-68): the digest of a capped round goes to a READER --
/// straight past the splitter that takes the curator's short block ids out of a
/// model's prose -- and a history tool's result quotes those ids by design. So
/// the head of the last result loses them here; the raw round in front of the
/// digest keeps them, because it is the colony's own record.
#[test]
fn the_partial_answer_quotes_no_short_block_id() {
    let mut rows = vec![leg_window_row(
        serde_json::json!([{"role": "user", "text": "what did I say about Lisbon?"}]),
    )];
    rows.extend(named_round_pair(1, "c1", "history_search", "nothing yet"));
    rows.extend(named_round_pair(
        2,
        "c2",
        "history_search",
        "[#0123456789ab] Lisbon in May  [#abcdefABCDEF] a flat near the river",
    ));
    let out = emit_with(
        &[("max_iter", "2")],
        reply_at(
            "round-check",
            "bundle",
            4,
            serde_json::Value::Array(rows),
            2,
        ),
    );
    assert_eq!(out[0]["header"]["partial"], "1", "{out:?}");
    let texts = texts_of(&out[0]);
    let last = texts.last().expect("a last turn");
    assert!(
        !last.contains("[#"),
        "a block id reached the reader: {last}"
    );
    assert!(
        last.contains("Lisbon in May a flat near the river"),
        "the evidence stays, only the ids go: {last}"
    );
    assert!(
        texts[..texts.len() - 1]
            .iter()
            .any(|t| t.contains("[#0123456789ab]")),
        "the raw round keeps what the tool said: {texts:?}"
    );
}

/// OR-KY-80: a history tool answers in JSON and writes its ids BARE
/// (`"id": "#<12 hex>"`), and the window's forms of a released or an expired block
/// carry one inside their brackets, and an ambiguous id is answered with sixteen-digit
/// `candidates` (OR-KY-81), and `history_read` answers with the full sixty-four-digit
/// `hash` (OR-KY-84) -- the digest quotes the head of exactly such a result, so it loses
/// those too, while a colour or a step number stays.
#[test]
fn the_partial_answer_quotes_no_bare_short_block_id() {
    let mut rows = vec![leg_window_row(
        serde_json::json!([{"role": "user", "text": "what did I say about Lisbon?"}]),
    )];
    rows.extend(named_round_pair(1, "c1", "history_outline", "nothing yet"));
    let result = r##"{"hits": [{"id": "#0123456789ab", "text": "Lisbon in May, step #3, colour #1a2b3c"}, {"id": "#abcdefabcdef", "text": "[#abcdefabcdef expired] [#0123456789ab released — history_read(\"#0123456789ab\")]"}], "candidates": ["#abcdefabcdef0123"], "hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", "order": "#123456789012"}"##;
    rows.extend(named_round_pair(2, "c2", "history_search", result));
    let out = emit_with(
        &[("max_iter", "2")],
        reply_at(
            "round-check",
            "bundle",
            4,
            serde_json::Value::Array(rows),
            2,
        ),
    );
    assert_eq!(out[0]["header"]["partial"], "1", "{out:?}");
    let texts = texts_of(&out[0]);
    let last = texts.last().expect("a last turn");
    assert!(
        !last.contains("0123456789ab") && !last.contains("abcdefabcdef"),
        "a block id reached the reader: {last}"
    );
    assert!(
        last.contains("Lisbon in May, step #3, colour #1a2b3c"),
        "the evidence stays, only the ids go: {last}"
    );
    assert!(
        !last.contains("0123456789abcdef0123456789abcdef"),
        "the full block id of a history_read answer reached the reader (OR-KY-84): {last}"
    );
    assert!(
        texts[..texts.len() - 1]
            .iter()
            .any(|t| t.contains("#abcdefabcdef")),
        "the raw round keeps what the tool said: {texts:?}"
    );
}

#[test]
fn the_first_assembly_of_a_turn_is_never_the_capped_one() {
    // iter 0 against the default of 8: the cap is a bound on the ROUND, not a
    // tax on every turn.
    let rows = serde_json::json!([leg_window_row(
        serde_json::json!([{"role": "user", "text": "hi"}])
    )]);
    let out = emit(reply_doc("collect", "bundle", 1, rows));
    // GH #889: the seam is `curate`.
    assert_eq!(out[0]["header"]["route"], "curate");
    assert_eq!(out[0]["header"]["round_capped"], "0");
}

#[test]
fn the_answer_leaves_and_takes_its_round_with_it() {
    // GH #889: the answer used to be written into the window before it left.
    // The history is the curator's now (it hears the brain's answer on its own
    // tap, R-27-1), so no turn row is written here; what leaves beside the
    // answer is the `round-drop` bundle -- the round's rows fall when its
    // answer leaves (OR-KX-V2), and the `depart` rows stay for a late consult
    // (GH #728).
    let out = emit(lane_doc(
        "in_answer",
        serde_json::json!([{"origin": "assistant", "type": "text", "text": "you said first"}]),
    ));
    assert_eq!(out.len(), 2, "the way out and the drop, in one multi-send");
    let dropped = out
        .iter()
        .find(|m| m["header"]["route"] == "cstore")
        .expect("the round is dropped");
    assert_eq!(dropped["header"]["phase"], "round-drop");
    let calls: Vec<serde_json::Value> = dropped["messages"]
        .as_array()
        .expect("calls")
        .iter()
        .map(|t| serde_json::from_str(t["text"].as_str().expect("op text")).expect("args"))
        .collect();
    assert!(
        calls.iter().all(|a| a["operation"] == "delete"),
        "nothing is written, only dropped: {calls:?}"
    );
    assert_eq!(calls[0]["table"], "turns");
    assert_eq!(
        calls[0]["where"]["turn_id"], "t1",
        "the turn it answers, and only that turn"
    );
    assert_eq!(calls[1]["table"], "round");
    assert_eq!(
        calls[1]["where"]["role"],
        serde_json::json!({"neq": "depart"}),
        "the departures stay for a late consult (GH #728)"
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
// progress is older than round_idle_ms and a message reaches this
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
            msg["header"]["phase"], "round-check",
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
    // behaviour (park and wait) -- a row the policy cannot date is a row the
    // policy never touches (the R-P3 direction of the prune lane GH #889
    // removed).
    let rows = serde_json::json!([asst_row(0, &["c1", "c2"], ""), tool_row(0, "c1", "a", "")]);
    assert!(emit(reply_doc("round-check", "select", 2, rows)).is_empty());
}

#[test]
fn a_stale_closed_round_fires_with_round_stale_on_the_seam() {
    let rows = serde_json::json!([
        leg_window_row(serde_json::json!([{"role": "user", "text": "q"}])),
        asst_row(0, &["c1", "c2"], STALE),
        tool_row(0, "c1", "found", STALE),
        lost_row(0, "c2")
    ]);
    let out = emit(reply_doc("round-check", "bundle", 4, rows));
    assert_eq!(emitted(&out), 1);
    let msg = &out[0];
    // GH #889: the regular route is `curate`.
    assert_eq!(
        msg["header"]["route"], "curate",
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
        leg_window_row(serde_json::json!([{"role": "user", "text": "q"}])),
        asst_row(0, &["c1", "c2"], STALE),
        tool_row(0, "c1", "found", STALE),
        tool_row(0, "c2", "also found", STALE)
    ]);
    let out = emit(reply_doc("round-check", "bundle", 4, rows));
    assert_eq!(out[0]["header"]["round_stale"], "0");
}

#[test]
fn a_lost_marker_from_an_older_iteration_does_not_stick() {
    // Iteration 0 was closed stale, iteration 1 completed on its own. The
    // fire of iteration 1 carries the old stand-in as history, but does not
    // call ITSELF stale.
    let rows = serde_json::json!([
        leg_window_row(serde_json::json!([{"role": "user", "text": "q"}])),
        asst_row(0, &["c1"], STALE),
        lost_row(0, "c1"),
        asst_row(1, &["c2"], STALE),
        tool_row(1, "c2", "found", STALE)
    ]);
    let out = emit(reply_at("round-check", "bundle", 5, rows, 1));
    // GH #889: the seam is `curate`.
    assert_eq!(out[0]["header"]["route"], "curate");
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
        leg_window_row(serde_json::json!([{"role": "user", "text": "q"}])),
        asst_row(0, &["c1"], STALE),
        lost_row(0, "c1"),
        tool_row(0, "c1", "late but real", STALE)
    ]);
    let out = emit(reply_doc("round-check", "bundle", 4, rows));
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
    let out = emit(open_reply(open, serde_json::json!([])));
    assert_eq!(
        emitted(&out),
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
    // GH #889: EVERY row of a deferred turn is a deferred row -- with no window
    // behind this cell the stamp is the only way the next round finds them, so
    // the update names no role (it used to name the user and peer rows only).
    assert!(
        op["where"].get("role").is_none(),
        "every row of the deferred turn is stamped, whatever its role: {op}"
    );
}

#[test]
fn a_mid_round_turn_with_a_stale_round_also_triggers_the_close() {
    // The parked arrival is itself the next OCCASION: a round whose start
    // already lies behind the idle window gets its re-check in the same
    // multi-send that defers the turn.
    let open = serde_json::json!([{"turn_id": "t9", "iter": 1, "recorded_at": STALE}]);
    let out = emit(open_reply(open, serde_json::json!([])));
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

/// GH #894 (OR-KY-75): `defer_turns` "0" -- a core's collector -- opens a round
/// of its own for a turn that arrives in another round's tool round, instead of
/// stamping it deferred: the stamp waited for a round nobody opened, and the next
/// errand's round carried it under that errand's `consult_id`. The open round's
/// idle re-check still rides on the arrival.
#[test]
fn a_mid_round_turn_opens_its_own_round_where_turns_are_not_deferred() {
    let open = serde_json::json!([{"turn_id": "t9", "iter": 1, "recorded_at": FRESH}]);
    let out = emit_with(
        &[("defer_turns", "0")],
        open_reply(open, serde_json::json!([])),
    );
    assert_eq!(emitted(&out), 1, "the window leg and nothing else: {out:?}");
    assert!(
        !out.iter().any(|m| m["header"]["phase"] == "defer-w"),
        "no deferral stamp: {out:?}"
    );
    let op = op_of(&out[0]);
    assert_eq!(op["table"], "round", "the window leg parks: {op}");
    assert_eq!(op["operation"], "insert");
    assert_eq!(op["row"]["role"], "leg-window");
    assert_eq!(op["row"]["turn_id"], "t1", "under the NEW turn: {op}");

    let stale = serde_json::json!([{"turn_id": "t9", "iter": 1, "recorded_at": STALE}]);
    let out = emit_with(
        &[("defer_turns", "0")],
        open_reply(stale, serde_json::json!([])),
    );
    assert_eq!(out.len(), 2, "the window leg and the re-check: {out:?}");
    assert_eq!(out[1]["header"]["phase"], "round-check", "{out:?}");
    assert_eq!(out[1]["header"]["turn_id"], "t9", "{out:?}");

    // The shipped value and a blank knob keep the telephone model.
    for v in ["1", ""] {
        let open = serde_json::json!([{"turn_id": "t9", "iter": 1, "recorded_at": FRESH}]);
        let out = emit_with(
            &[("defer_turns", v)],
            open_reply(open, serde_json::json!([])),
        );
        assert_eq!(out[0]["header"]["phase"], "defer-w", "{v:?}: {out:?}");
    }
}

#[test]
fn an_undatable_open_round_does_not_defer_the_turn() {
    // An open assistant row without recorded_at predates the policy; it can
    // neither be closed (undatable) nor block the session forever. Such a
    // session keeps the pre-#103 behaviour: the turn assembles normally.
    let open = serde_json::json!([{"turn_id": "t9", "iter": 1, "recorded_at": ""}]);
    let out = emit(open_reply(open, serde_json::json!([])));
    assert_eq!(emitted(&out), 1);
    // GH #419: the window is already IN this reply, so what an assembling turn
    // emits is the parked window leg -- not a second read of it.
    let op = op_of(&out[0]);
    assert_eq!(op["table"], "round", "the window leg parks, not a deferral");
    assert_eq!(op["operation"], "insert");
    assert_eq!(op["row"]["role"], "leg-window");
}

#[test]
fn a_deferred_turn_rides_with_the_next_assembly_and_is_cleared() {
    // 1. The round read sees the stamp and counts it into the leg. GH #889: the
    //    read keeps the round's own turn and the deferred rows riding in it, and
    //    NAMES the turns those rows belong to, so the seam can drop them once
    //    they rode -- the deferred turn is an earlier one (`t0`), not this one.
    let mut rows = serde_json::json!([
        turn_row("2", "user", "the new one"),
        turn_row("1", "user", "the deferred one")
    ]);
    rows[1]["turn_id"] = serde_json::json!("t0");
    rows[1]["deferred"] = serde_json::json!(1);
    let out = emit(open_reply(serde_json::json!([]), rows));
    let payload: serde_json::Value =
        serde_json::from_str(op_of(&out[0])["row"]["turn"].as_str().expect("turn"))
            .expect("payload");
    assert_eq!(
        payload["deferred"], 1,
        "the leg carries how many deferred turns it holds"
    );
    assert_eq!(
        payload["deferred_turns"],
        serde_json::json!(["t0"]),
        "and which turns they belong to: {payload}"
    );
    let texts: Vec<&str> = payload["turns"]
        .as_array()
        .expect("turns")
        .iter()
        .map(|t| t["text"].as_str().expect("text"))
        .collect();
    assert_eq!(
        texts,
        ["the deferred one", "the new one"],
        "in the order they arrived: {payload}"
    );

    // 2. The fire that carries a deferred turn says so on the seam and, in the
    //    same multi-send, DROPS the deferred turn's rows -- GH #889 (OR-KX-V2):
    //    its words travelled on `curate`, the curator keeps them from here on.
    //    It used to only clear the stamp, since the window read them again.
    let leg = serde_json::json!({"turn_id": "t1", "iter": 0, "role": "leg-window",
        "turn": "{\"turns\":[{\"role\":\"user\",\"text\":\"the deferred one\"}],\"deferred\":1,\"deferred_turns\":[\"t0\"]}",
        "fired": 0});
    let out = emit(reply_doc("collect", "bundle", 1, serde_json::json!([leg])));
    assert_eq!(
        out.len(),
        3,
        "the seam, the clear and the closing mark: {out:?}"
    );
    let seam = &out[0];
    assert_eq!(seam["header"]["route"], "curate");
    assert_eq!(seam["header"]["round_deferred"], "1");
    let clear = &out[1];
    assert_eq!(clear["header"]["phase"], "defer-clear");
    let calls: Vec<serde_json::Value> = clear["messages"]
        .as_array()
        .expect("calls")
        .iter()
        .map(|t| serde_json::from_str(t["text"].as_str().expect("op text")).expect("args"))
        .collect();
    assert_eq!(calls.len(), 2, "the turns and their legs: {calls:?}");
    assert!(
        calls.iter().all(|a| a["operation"] == "delete"),
        "{calls:?}"
    );
    assert_eq!(calls[0]["table"], "turns");
    assert_eq!(calls[0]["where"]["session_id"], "s1");
    assert_eq!(
        calls[0]["where"]["turn_id"],
        serde_json::json!({"in": ["t0"]}),
        "the deferred turn's rows and no other turn's"
    );
    assert_eq!(calls[1]["table"], "round");
    assert_eq!(
        calls[1]["where"]["turn_id"],
        serde_json::json!({"in": ["t0"]})
    );

    // 3. An assembly without a deferred turn is a single emission, flag 0.
    let out = emit(reply_doc(
        "collect",
        "bundle",
        1,
        serde_json::json!([leg_window_row(
            serde_json::json!([{"role": "user", "text": "hi"}])
        )]),
    ));
    assert_eq!(emitted(&out), 1);
    assert_eq!(out[0]["header"]["round_deferred"], "0");
}

#[test]
fn the_fresh_turn_write_stamps_the_deferred_column() {
    // The writer of `turns` stamps the flag explicitly, so a row is never
    // ambiguous between "not deferred" and "predates the column". GH #889 left
    // one writer: `in_answer` no longer writes the answer (the history is the
    // curator's), so only the turn's own row is measured.
    let out = emit(lane_doc(
        "in_turn",
        serde_json::json!([{"origin": "user", "type": "text", "text": "hello"}]),
    ));
    assert_eq!(op_of(&out[0])["row"]["deferred"], 0);
}

#[test]
fn the_sweep_lane_asks_for_every_open_round_in_every_session() {
    // The re-check occasion a parent tree provides: a timer or an operator
    // asks "anything stuck?". The template never fires this itself (the
    // session-keeper discipline), and the question is session-agnostic on
    // purpose -- a timer knows no session; the rows do.
    let out = emit(lane_doc("in_round_sweep", serde_json::json!([])));
    assert_eq!(emitted(&out), 1);
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
        emitted(&out),
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

// ============================================== THE ROUND ROWS AND THE ECHO GUARD
//
// GH #889: the close lane (R-OS-6) and the prune lane (GH #76) left with the
// history they read -- the close batch is the curator's `write` now, and a
// collector that drops a round's rows when its answer leaves has nothing left
// to prune. What stays is what the round rows carry for #103.

#[test]
fn every_round_row_carries_the_session_it_belongs_to() {
    // Every writer of the round table stamps the session -- from the first
    // assembly leg to the last tool result -- because the idle sweep's re-check
    // runs under the session it reads off the row (GH #103). (It used to be
    // the close lane that read it; GH #889 removed that lane.)
    let out = emit(lane_doc(
        "in_calls",
        serde_json::json!([{"origin": "assistant", "type": "tool_call", "id": "c1",
                            "text": "{}"}]),
    ));
    assert_eq!(op_of(&out[0])["row"]["session_id"], "s1");
    let out = emit(open_reply(serde_json::json!([]), serde_json::json!([])));
    assert_eq!(op_of(&out[0])["row"]["session_id"], "s1");
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
                               "col_phase": "collect"},
                   "hop": {"route": "in_answer"}},
        "messages": [{"origin": "assistant", "type": "text", "text": "done"}]
    });
    let out = emit(stale);
    // The answer and, since GH #889, the `round-drop` beside it (it used to be
    // the answer's write into the window).
    assert_eq!(out.len(), 2, "the lane wins over the stale step");
    assert!(out.iter().any(|m| m["header"]["route"] == "answer"));
}

#[test]
fn every_round_row_carries_its_write_time() {
    // A row the idle policy cannot date is a round it will never close
    // (GH #103), so every writer of the round table stamps recorded_at --
    // assembly legs and tool rounds alike. (GH #889 removed the prune lane,
    // the other reader of the stamp.)
    let out = emit(open_reply(serde_json::json!([]), serde_json::json!([])));
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

// ================================== THE MEMORY LEG (GH #78, narrowed by #552)
//
// The ambient leg is fired before the model has seen the turn, so nothing in an
// agent can DECIDE to ask memory about a TIME RANGE -- the half of #27 that did
// not fall out naturally. The TOOL closes that half, and until #552 it closed it
// inside this cell: the collector owned the recall port, so it answered the call
// as well and typed the schema by hand. It does not any more -- the memory hive
// declares the name and answers it, because the hive is where the rules a recall
// obeys are enforced. What is measured below is the AMBIENT leg, and the round
// fan-in a memory result joins like any other tool result.

#[test]
fn the_ambient_ask_names_no_window_and_says_so() {
    // The P15 finding was that the recall cell has understood
    // recall_window_from/_to for a long time and nobody ever DERIVES one. The
    // producer is the MODEL, and since GH #552 it names them to the memory hive
    // directly (`memory-hive/tool`), not to this cell. What is left here is the
    // ambient leg, which asks no window at all -- and it still carries both keys,
    // empty rather than absent, because a missing hop key makes the promoting CEL
    // modifier fail and a failed modifier skips the edge.
    let out = emit_with(
        &[("memory_tier", "0")],
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
    assert!(
        ask["header"]["memory_call_id"].is_null(),
        "the port has ONE meaning again: no call asked for this, and no \
         correlation key rides along to say a call might have: {ask:#?}"
    );
}

#[test]
fn a_memory_result_fans_in_beside_a_normal_tool_result_and_the_round_fires() {
    // The bundle the issue describes: one memory_recall and one ordinary tool
    // in ONE brain answer. The memory call is a normal member of the
    // expectation set -- it is counted and waited for like any other (and, since
    // GH #889, passed on uncut like any other).
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
    // GH #419: a complete round emits the SEAM and the closing mark together --
    // the mark is not a guard any more (nothing reads its `rows_affected`), it
    // is what tells `turn-open` and the idle sweep that this round has answered.
    let out = emit(reply_doc("round-check", "bundle", 3, full.clone()));
    // GH #889: the seam is `curate`.
    assert_eq!(out[0]["header"]["route"], "curate", "the round completed");
    let mark = out.last().expect("the closing mark travels with the seam");
    assert_eq!(op_of(mark)["operation"], "update", "{mark}");
    assert_eq!(op_of(mark)["set"]["fired"], 1, "{mark}");

    let mut rows = full.as_array().expect("rows").clone();
    rows.push(leg_window_row(
        serde_json::json!([{"role": "user", "text": "what about the roof?"}]),
    ));
    let out = emit(reply_doc(
        "round-check",
        "bundle",
        4,
        serde_json::Value::Array(rows),
    ));
    assert_eq!(emitted(&out), 1);
    let texts = texts_of(&out[0]);
    assert!(
        texts.contains(&"the weather".to_string())
            && texts.contains(&"MEMORY: the roof".to_string()),
        "both results reach the curator through the ONE seam: {texts:?}"
    );
    assert_eq!(out[0]["header"]["route"], "curate");
    assert_eq!(out[0]["header"]["iter"], "1");
}

// ===================================== THE ADVISOR CONNECTION (GH #28, R-CG-3)

/// A lane document with extra hop keys -- the dispatcher's `async_calls` marker
/// and the correlation id an advice event carries back.
fn lane_with(
    route: &str,
    hop: serde_json::Value,
    ctx: serde_json::Value,
    messages: serde_json::Value,
) -> serde_json::Value {
    let mut doc = lane_doc(route, messages);
    for (k, v) in hop.as_object().expect("hop object") {
        doc["header"]["hop"][k] = v.clone();
    }
    for (k, v) in ctx.as_object().expect("ctx object") {
        doc["header"]["context"][k] = v.clone();
    }
    doc
}

fn call_bundle(ids: &[&str]) -> serde_json::Value {
    serde_json::Value::Array(
        ids.iter()
            .map(|id| {
                serde_json::json!({"origin": "assistant", "type": "tool_call",
                                   "id": id, "text": "{}"})
            })
            .collect(),
    )
}

#[test]
fn an_all_async_bundle_closes_its_round_at_once_and_waits_for_nothing() {
    // Delta 2 of R-CG-3: the collector opens NO fan-in expectation for a call
    // the dispatcher classified as async. The assistant row is filed as already
    // fired, so no guard can ever fire it, no sweep can ever find it open, and
    // round_idle_ms never races the advisor's thinking time. The turn
    // is over; the answer comes back later as an EVENT, not as a fan-in.
    //
    // Since GH #372 "the turn is over" is a CONDITION, and this bundle meets it
    // twice over: the model spoke beside the calls (so the dispatcher already
    // sent that sentence as the interim answer) and the call is a declared
    // HANDOFF. Either alone would do -- the two neighbouring tests take them
    // apart -- and neither, and the round stays open.
    let mut messages = call_bundle(&["c1"]);
    messages
        .as_array_mut()
        .expect("bundle")
        .push(serde_json::json!({"origin": "assistant", "type": "text",
                           "text": "one moment, I am asking"}));
    let out = emit(lane_with(
        "in_calls",
        serde_json::json!({"async_calls": "c1", "handoff_calls": "c1"}),
        serde_json::json!({}),
        messages,
    ));

    // GH #889 (OR-KX-V2): a round filed as fired is over on the spot, so its
    // rows leave with it -- the `round-drop` bundle rides behind the ack.
    assert_eq!(
        out.len(),
        3,
        "the assistant row, one ack and the drop: {out:?}"
    );
    assert_eq!(out[2]["header"]["phase"], "round-drop", "{out:?}");
    let asst = op_of(&out[0]);
    assert_eq!(asst["row"]["role"], "assistant");
    assert_eq!(
        asst["row"]["fired"], 1,
        "a round with nothing to wait for is not an open round"
    );
    // The ack keeps the wire well-formed: a provider rejects an assistant turn
    // whose tool_call has no tool_result beside it.
    let ack = op_of(&out[1]);
    assert_eq!(ack["row"]["role"], "tool");
    let turn: serde_json::Value =
        serde_json::from_str(ack["row"]["turn"].as_str().expect("turn")).expect("turn json");
    assert_eq!(turn["id"], "c1", "under the original tool_call_id");
    assert_eq!(turn["type"], "tool_result");
    assert!(
        turn["text"].as_str().unwrap_or_default().contains("later"),
        "and it says what happened: {turn}"
    );
}

/// GH #372, half one: a bare async call that is NOT a handoff leaves the round
/// OPEN. Nothing has answered this turn -- the dispatcher sends an interim only
/// when a sentence stands beside the bundle, and a fire-and-forget write never
/// comes back -- so filing the row as fired ended the turn in silence. The
/// acknowledgement still travels, so the fan-in completes on the spot and the
/// regular guard re-enters the brain for the iteration the model has not spent.
#[test]
fn a_bare_async_call_that_hands_nothing_over_leaves_the_round_open() {
    let out = emit(lane_with(
        "in_calls",
        serde_json::json!({"async_calls": "c1", "handoff_calls": ""}),
        serde_json::json!({}),
        call_bundle(&["c1"]),
    ));

    assert_eq!(out.len(), 2, "the assistant row plus one ack: {out:?}");
    assert_eq!(
        op_of(&out[0])["row"]["fired"],
        0,
        "nobody has answered this turn, so the round is not over"
    );
    // The ack is unchanged: what makes the round complete is what makes the
    // guard fire, and that is the same machinery as every other round.
    let turn: serde_json::Value =
        serde_json::from_str(op_of(&out[1])["row"]["turn"].as_str().expect("turn"))
            .expect("turn json");
    assert_eq!(turn["id"], "c1");
    assert_eq!(turn["type"], "tool_result");
}

/// GH #372, half two: the handoff mark alone closes the round, without a word.
/// That is the escalation shape (`escalate_to_deep`, whose seed prompt says "say
/// nothing else") and the consult shape -- the answer comes from a later turn,
/// so this round is over and must not re-enter the brain.
#[test]
fn a_handoff_call_closes_its_round_without_a_word() {
    let out = emit(lane_with(
        "in_calls",
        serde_json::json!({"async_calls": "c1", "handoff_calls": "c1"}),
        serde_json::json!({}),
        call_bundle(&["c1"]),
    ));

    // GH #889 (OR-KX-V2): the round is over, so the `round-drop` rides along.
    assert_eq!(out.len(), 3, "{out:?}");
    assert_eq!(out[2]["header"]["phase"], "round-drop", "{out:?}");
    assert_eq!(
        op_of(&out[0])["row"]["fired"],
        1,
        "the turn was handed over: no guard to win, no open round for a sweep"
    );
}

/// And the text alone closes it too -- the sentence the dispatcher already put
/// on the channel IS the answer of this turn (R-CG-3, delta 1).
#[test]
fn a_sentence_beside_the_bundle_closes_the_round_without_a_handoff() {
    let mut messages = call_bundle(&["c1"]);
    messages
        .as_array_mut()
        .expect("bundle")
        .push(serde_json::json!({"origin": "assistant", "type": "text", "text": "one moment."}));
    let out = emit(lane_with(
        "in_calls",
        serde_json::json!({"async_calls": "c1", "handoff_calls": ""}),
        serde_json::json!({}),
        messages,
    ));

    // GH #889 (OR-KX-V2): the round is over, so the `round-drop` rides along.
    assert_eq!(out.len(), 3, "{out:?}");
    assert_eq!(out[2]["header"]["phase"], "round-drop", "{out:?}");
    assert_eq!(op_of(&out[0])["row"]["fired"], 1);
}

/// An EMPTY text turn is not a sentence. A provider that sends `content: ""`
/// beside a bundle has said nothing, and reading it as an answer would put the
/// silence straight back -- which is why this reads the turns exactly as the
/// dispatcher does when it decides whether to send an interim answer at all.
#[test]
fn an_empty_text_turn_is_not_an_answer() {
    let mut messages = call_bundle(&["c1"]);
    messages
        .as_array_mut()
        .expect("bundle")
        .push(serde_json::json!({"origin": "assistant", "type": "text", "text": ""}));
    let out = emit(lane_with(
        "in_calls",
        serde_json::json!({"async_calls": "c1", "handoff_calls": ""}),
        serde_json::json!({}),
        messages,
    ));

    assert_eq!(op_of(&out[0])["row"]["fired"], 0, "{out:?}");
}

#[test]
fn a_mixed_bundle_still_waits_for_the_calls_that_do_answer() {
    let out = emit(lane_with(
        "in_calls",
        serde_json::json!({"async_calls": "c1"}),
        serde_json::json!({}),
        call_bundle(&["c1", "c2"]),
    ));

    assert_eq!(out.len(), 2, "{out:?}");
    assert_eq!(
        op_of(&out[0])["row"]["fired"],
        0,
        "c2 is still expected, so the round is open"
    );
    let turn: serde_json::Value =
        serde_json::from_str(op_of(&out[1])["row"]["turn"].as_str().expect("turn"))
            .expect("turn json");
    assert_eq!(turn["id"], "c1", "only the async call is acknowledged");
}

#[test]
fn without_the_marker_a_bundle_behaves_exactly_as_before() {
    let out = emit(lane_doc("in_calls", call_bundle(&["c1", "c2"])));
    assert_eq!(emitted(&out), 1, "one assistant row, no acks: {out:?}");
    assert_eq!(op_of(&out[0])["row"]["fired"], 0);
}

/// GH #541 -- a turn OPENS a round, so it starts with the whole budget. The
/// `in_advice` lane is the answer lane of ANOTHER hive's round, and its `iter`
/// rides along on the arrival: a core that spent nine iterations handed the
/// surface a turn that was over before it began, the seam left on `answer`
/// instead of `brain`, and the reader got the raw assembled round where an
/// answer belonged. Every emission of this invocation carries the round's
/// number, so the store bundle is where it is measured -- it is what stamps the
/// reply the seam is later assembled out of.
/// GH #728 -- an advice with a correlation opens in TWO stages: the first
/// parks its row and looks up the departure (`advice-look`), the second opens
/// the round. Both carry the round's own number; the second is fed the reply
/// the store hands back, with the `iter` the cstore edge promoted off the first.
#[test]
fn an_advice_turn_opens_its_round_with_the_whole_budget() {
    let out = emit(lane_with(
        "in_advice",
        serde_json::json!({"iter": "9", "round_capped": "1"}),
        serde_json::json!({"consult_id": "k-7", "iter": "9"}),
        serde_json::json!([{"origin": "assistant", "type": "text",
                            "text": "cheap: flight 180, hostel 3x40"}]),
    ));
    assert_eq!(
        out[0]["header"]["iter"], "0",
        "the advisor's spent budget is not this round's: {out:?}"
    );
    assert_eq!(
        out[0]["header"]["phase"], "advice-look",
        "stage A: the departure is looked up first"
    );
    let opened = emit(advice_looked(&out[0], serde_json::json!([])));
    assert_eq!(opened[0]["header"]["iter"], "0", "{opened:?}");
    assert_eq!(
        opened[0]["header"]["phase"], "turn-open",
        "and it is still assembled as a turn"
    );
}

/// Stage B of an advice (GH #728): the store's reply to the `advice-look`
/// bundle `stage_a` emitted, with `departs` as what the departure lookup found
/// and the advice row read back as stage A wrote it.
fn advice_looked(stage_a: &serde_json::Value, departs: serde_json::Value) -> serde_json::Value {
    let prov = stage_a["header"]["turn_id"]
        .as_str()
        .expect("provisional key");
    let row = stage_a["messages"]
        .as_array()
        .expect("bundle")
        .iter()
        .find(|m| m["id"] == "c-look-turn")
        .map(|m| {
            serde_json::from_str::<serde_json::Value>(m["text"].as_str().expect("op"))
                .expect("op json")["row"]
                .clone()
        })
        .expect("the advice row");
    let legs = [
        ("c-look-depart", departs),
        ("c-look-turn", serde_json::Value::Null),
        ("c-look-self", serde_json::json!([row])),
    ];
    serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": prov, "iter": "0",
                               "consult_id": row["consult_id"],
                               "col_phase": "advice-look", "store_origin": "collector"},
                   "hop": {"operation": "bundle", "rows_affected": 1,
                           "bundle_errors": 0}},
        "messages": legs.iter().map(|(id, rows)| serde_json::json!(
            {"origin": "tool", "type": "tool_result", "id": id, "text": rows.to_string()}))
            .collect::<Vec<_>>(),
        "results": legs.iter().map(|(id, _)| serde_json::json!(
            {"tool_call_id": id, "operation": "select", "rows_affected": 1,
             "duration_ms": 0})).collect::<Vec<_>>()
    })
}

/// The counter-pin: a lane that is NOT turn-opening still reads the iteration
/// it was handed, or a tool round would restart its budget on every fan-in.
#[test]
fn a_tool_result_still_carries_the_round_it_belongs_to() {
    let out = emit(lane_with(
        "in_tool",
        serde_json::json!({}),
        serde_json::json!({"iter": "3"}),
        serde_json::json!([{"origin": "tool", "type": "tool_result", "id": "c1",
                            "text": "21C"}]),
    ));
    assert_eq!(
        out[0]["header"]["iter"], "3",
        "a fan-in is inside a round, not the start of one: {out:?}"
    );
}

#[test]
fn an_advice_event_is_assembled_like_a_turn_and_keeps_its_correlation() {
    // Delta 3 of R-CG-3: the advisor's result comes back as an EVENT on its own
    // lane and starts a fresh round -- the turn it belongs to ended long ago.
    // Since GH #728 in two stages: the row is written first, under a
    // provisional key, beside the lookup of its departure.
    let out = emit(lane_with(
        "in_advice",
        serde_json::json!({}),
        serde_json::json!({"consult_id": "k-7"}),
        serde_json::json!([{"origin": "assistant", "type": "text",
                            "text": "berlin: 21C"}]),
    ));

    assert_eq!(
        out.len(),
        1,
        "one bundle, the memory leg waits for stage B: {out:?}"
    );
    let row = out[0]["messages"]
        .as_array()
        .expect("bundle")
        .iter()
        .find(|m| m["id"] == "c-look-turn")
        .map(|m| {
            serde_json::from_str::<serde_json::Value>(m["text"].as_str().expect("op"))
                .expect("op json")
        })
        .expect("the advice row");
    assert_eq!(row["table"], "turns");
    assert_eq!(
        row["row"]["role"], "advice",
        "an event is not a user turn and not the agent's own words"
    );
    assert_eq!(row["row"]["content"], "berlin: 21C");
    assert_eq!(
        row["row"]["consult_id"], "k-7",
        "the correlation is what makes the exchange bilateral"
    );
    let prov = out[0]["header"]["turn_id"].as_str().expect("turn_id");
    assert!(!prov.is_empty(), "a fresh turn id: this is a new round");
    assert_eq!(row["row"]["turn_id"], prov);

    // Stage B, nothing departed: the SAME chain as a turn, under the key the row
    // already has.
    let opened = emit(advice_looked(&out[0], serde_json::json!([])));
    assert_eq!(emitted(&opened), 1, "no memory leg configured: {opened:?}");
    assert_eq!(
        opened[0]["header"]["phase"], "turn-open",
        "the SAME chain as a turn"
    );
    let rekey = op_of(&opened[0]);
    assert_eq!(rekey["operation"], "update");
    assert_eq!(rekey["set"]["turn_id"], prov);
    assert_eq!(opened[0]["header"]["turn_id"], prov);
}

#[test]
fn an_advice_event_fires_the_memory_leg_like_any_other_turn() {
    let out = emit_with(
        &[("memory_tier", "1")],
        lane_with(
            "in_advice",
            serde_json::json!({}),
            serde_json::json!({"consult_id": "k-7"}),
            serde_json::json!([{"origin": "assistant", "type": "text", "text": "berlin: 21C"}]),
        ),
    );
    assert_eq!(
        out.len(),
        1,
        "stage A asks for the departure and nothing else"
    );
    // GH #728: the leg leaves with the round, under the round's key -- which is
    // only known once the departure came back.
    let opened = emit_with(
        &[("memory_tier", "1")],
        advice_looked(
            &out[0],
            serde_json::json!([{"turn_id": "chat#1", "deadline_ms": 9_999_999_999_999_i64}]),
        ),
    );
    assert_eq!(opened.len(), 2, "the gate waits for the leg it configured");
    assert_eq!(opened[1]["header"]["route"], "recall");
    assert_eq!(
        opened[1]["header"]["turn_id"], opened[0]["header"]["turn_id"],
        "the leg parks on the round it belongs to"
    );
}

#[test]
fn the_open_consults_of_the_window_reach_the_brain_as_data() {
    // The reply half of the bilateral lane: the model can only pass a consult
    // id back if it was shown one. The collector renders nothing -- it hands
    // over the raw ids and lets the persona decide what to do with them.
    let turns = serde_json::json!([
        {"role": "user", "text": "what is the weather?"},
        {"role": "assistant", "text": "one moment, asking"},
        {"role": "advice", "text": "which city?", "consult_id": "k-7"}
    ]);
    let rows = serde_json::json!([leg_window_row(turns)]);
    let out = emit(reply_doc("collect", "bundle", 1, rows));

    // GH #889: the seam is `curate`; the curator passes the slot on.
    assert_eq!(out[0]["header"]["route"], "curate");
    assert_eq!(
        out[0]["system"]["consult"]["open"],
        serde_json::json!(["k-7"]),
        "verbatim ids, no prose: {}",
        out[0]
    );
    // The system tree only travels through `text` leaves (walk_collect), so
    // the ids need one -- three words of label, the ids themselves, and since
    // GH #540 the one rule that says what an advice IS.
    let text = out[0]["system"]["consult"]["text"]
        .as_str()
        .expect("consult text");
    assert!(
        text.starts_with("open consults: k-7\n"),
        "the ids stay the first line, verbatim: {text}"
    );
    let msgs = out[0]["messages"].as_array().expect("messages");
    assert_eq!(msgs.len(), 3);
    assert_eq!(
        msgs[2]["origin"], "user",
        "an event is inbound on the wire -- the two roles a provider knows"
    );
    assert_eq!(
        msgs[2]["text"], "[advice from your reasoning core, consult k-7]\nwhich city?",
        "and it says which of the two it is (GH #540)"
    );
}

/// GH #540 -- the role confusion the frame closes. An advisor's answer used to
/// leave this seam as a bare `origin: user` text: byte for byte the shape of a
/// new sentence by the person, and a model read it as one -- it consulted a
/// second time quoting the core back as the person's own figures, then answered
/// the person a question she had never asked.
#[test]
fn an_advice_turn_says_on_the_wire_that_it_is_one() {
    let turns = serde_json::json!([
        {"role": "user", "text": "plan me three days in athens"},
        {"role": "assistant", "text": "one moment, I am putting two options together"},
        {"role": "advice", "text": "cheap: flight 180, hostel 3x40", "consult_id": "k-7"}
    ]);
    let rows = serde_json::json!([leg_window_row(turns)]);
    let out = emit(reply_doc("collect", "bundle", 1, rows));
    let msgs = out[0]["messages"].as_array().expect("messages");
    assert_eq!(msgs.len(), 3);
    // The person's word and the agent's own are untouched: this repair adds a
    // frame to ONE role and reads every other row exactly as before.
    assert_eq!(msgs[0]["origin"], "user");
    assert_eq!(msgs[0]["text"], "plan me three days in athens");
    assert_eq!(msgs[1]["origin"], "assistant");
    assert_eq!(
        msgs[1]["text"], "one moment, I am putting two options together",
        "no frame on the agent's own voice"
    );
    assert_eq!(
        msgs[2]["origin"], "user",
        "still the only inbound role a provider accepts mid-conversation"
    );
    assert_eq!(
        msgs[2]["text"],
        "[advice from your reasoning core, consult k-7]\ncheap: flight 180, hostel 3x40",
        "the frame names what it is and which consultation it answers"
    );
}

/// The same row with no correlation on it. An advice can only be replied to
/// through an id it was shown, so a frame that PRINTED an empty one would
/// invite a consult call carrying `consult_id: ""`. It names the sender and
/// stops there.
#[test]
fn an_advice_without_a_correlation_is_still_framed_but_names_no_id() {
    let turns = serde_json::json!([
        {"role": "advice", "text": "the fare is 180 EUR", "consult_id": ""}
    ]);
    let rows = serde_json::json!([leg_window_row(turns)]);
    let out = emit(reply_doc("collect", "bundle", 1, rows));
    let msgs = out[0]["messages"].as_array().expect("messages");
    assert_eq!(
        msgs[0]["text"], "[advice from your reasoning core]\nthe fare is 180 EUR",
        "no id in the frame when there is none to pass back"
    );
    assert_eq!(
        out[0]["system"]["consult"]["text"], "",
        "and nothing is open, so the slot is the empty rendering"
    );
}

/// The other half of GH #540: knowing an advice is an advice does not yet say
/// what to do with one. The rule travels with the open ids -- in the slot this
/// cell RE-DERIVES every round, because a seed charter is read once at birth and
/// a grown, imported or rebuilt brain never receives it (GH #512, GH #525).
///
/// GH #894: the question the core asks back is answered with `reply_to_consult`
/// under its id -- and when only the person knows the answer, the person is
/// asked first. Until then the rule ended on "never the person", which stood in
/// every request of the round that carried the core's question.
#[test]
fn the_open_consult_slot_says_what_an_advice_is_for() {
    let turns = serde_json::json!([
        {"role": "advice", "text": "which city?", "consult_id": "k-7"}
    ]);
    let rows = serde_json::json!([leg_window_row(turns)]);
    let out = emit(reply_doc("collect", "bundle", 1, rows));
    let text = out[0]["system"]["consult"]["text"]
        .as_str()
        .expect("consult text");
    for phrase in [
        "the answer to YOUR consultation",
        "in your own words",
        "Do not consult again",
        "consult_id",
        "reply_to_consult",
        "if only the person knows, ask the person first",
    ] {
        assert!(
            text.contains(phrase),
            "the rule is missing {phrase:?}: {text}"
        );
    }
    assert!(
        !text.contains("never the person"),
        "the rule forbids the one person who can answer the core's question: {text}"
    );
}

/// GH #259 -- the second half of the same lane: a correlation that was handed
/// out has to be taken back, and `system.*` is the one slot family where
/// "stop sending it" does not do that. The receiving `llm` cell UPSERTS per
/// slot path into its own `cell.db`, so a path that is not sent is a path that
/// is not touched: an id set once outlives every round that no longer holds
/// the event it belongs to.
///
/// OR-KX-63 (GH #889): the round no longer reads a history, so an id outlives
/// the round that showed it on the session's `depart` row instead -- the
/// question the core asks back is answered by the person in a LATER round, and
/// the window leg carries the id there (`consults`). The revocation moved with
/// it: the id goes when its departure falls.
///
/// Three rounds, and the LAST one is the test.
#[test]
fn a_consult_whose_departure_fell_is_revoked_in_the_next_projection() {
    let with_advice = serde_json::json!([
        {"role": "user", "text": "what is the weather?"},
        {"role": "advice", "text": "which city?", "consult_id": "k-7"}
    ]);
    let first = emit(reply_doc(
        "collect",
        "bundle",
        1,
        serde_json::json!([leg_window_row(with_advice)]),
    ));
    assert_eq!(
        first[0]["system"]["consult"]["open"],
        serde_json::json!(["k-7"]),
        "round one: the id travels while its event is in the round"
    );

    // Round two: the person answers the core's question. No advice turn in
    // the round -- the answered departure still stands, and the id with it.
    let held = serde_json::json!({
        "turns": [{"role": "user", "text": "berlin"}],
        "deferred": 0, "deferred_turns": [], "consults": ["k-7"]});
    let second = emit(reply_doc(
        "collect",
        "bundle",
        1,
        serde_json::json!([{"turn_id": "t1", "iter": 0, "role": "leg-window",
                             "turn": held.to_string(), "fired": 0}]),
    ));
    assert_eq!(
        second[0]["system"]["consult"]["open"],
        serde_json::json!(["k-7"]),
        "round two: the id rides on its departure into the next round: {}",
        second[0]
    );

    // Round three: the departure fell (a newer one replaced it, or the frist
    // passed). The consultation is closed, and the projection has to SAY so.
    let without_advice = serde_json::json!([
        {"role": "user", "text": "and tomorrow?"},
        {"role": "assistant", "text": "sunny"}
    ]);
    let third = emit(reply_doc(
        "collect",
        "bundle",
        1,
        serde_json::json!([leg_window_row(without_advice)]),
    ));
    assert_eq!(
        third[0]["system"]["consult"]["open"],
        serde_json::json!([]),
        "an omitted path is an untouched path -- the empty slot must be SENT: {}",
        third[0]
    );
    // `flatten_to_leaves` stops at `text`: a slot offered WITHOUT one produces
    // no leaf, hence no upsert, and the stale row would stand exactly as
    // before. The empty rendering is what makes the overwrite happen.
    assert_eq!(
        third[0]["system"]["consult"]["text"], "",
        "the emptied slot still needs its `text` leaf -- that is what gets overwritten"
    );
}

#[test]
fn a_window_without_advice_carries_the_consult_slot_emptied() {
    let turns = serde_json::json!([{"role": "user", "text": "hi"}]);
    let rows = serde_json::json!([leg_window_row(turns)]);
    let out = emit(reply_doc("collect", "bundle", 1, rows));
    assert_eq!(
        out[0]["system"],
        serde_json::json!({"consult": {"open": [], "text": ""},
                           "instructions": {"mode": {"text": ""},
                                            "peer": {"text": ""}},
                           "roster": {"text": ""}}),
        "no memory leg, so the two UNCONDITIONAL slots are the whole tree -- and \
         both are the EMPTY ones: a slot that is never sent empty is never \
         revoked. `instructions.mode` joined the pair with the duplex advise \
         mode (welle-live) and is written on the same argument, and so are \
         `instructions.peer` and `roster` (GH #847): {}",
        out[0]
    );
}

#[test]
fn an_interim_answer_does_not_travel_twice_and_never_splits_a_round() {
    // The sentence that stood next to the bundle already went to the channel
    // and was written into the WINDOW by the in_answer lane. Repeating it
    // inside the round would show it twice -- and on the wire it would stand
    // between an assistant turn and the tool results that answer it, which
    // every provider rejects.
    let calls = serde_json::json!([
        {"origin": "assistant", "type": "tool_call", "id": "c1", "text": "{}"},
        {"origin": "assistant", "type": "text", "text": "one moment, asking"}
    ]);
    let res = serde_json::json!(
        {"origin": "tool", "type": "tool_result", "id": "c1", "text": "42"}
    );
    let rows = serde_json::json!([
        {"turn_id": "t1", "iter": 0, "role": "assistant",
         "turn": calls.to_string(), "fired": 0},
        {"turn_id": "t1", "iter": 0, "role": "tool",
         "turn": res.to_string(), "fired": 0}
    ]);
    let out = emit(reply_doc("round-check", "bundle", 2, rows));
    let msgs = out[0]["messages"].as_array().expect("messages");
    assert_eq!(
        msgs.len(),
        2,
        "the call and its result, nothing else: {msgs:?}"
    );
    assert_eq!(msgs[0]["type"], "tool_call");
    assert_eq!(msgs[1]["type"], "tool_result");
}

#[test]
fn an_interim_answer_stays_an_interim_answer_on_its_way_out() {
    let out = emit(lane_with(
        "in_answer",
        serde_json::json!({"interim": "1"}),
        serde_json::json!({}),
        serde_json::json!([{"origin": "assistant", "type": "text",
                            "text": "one moment, asking"}]),
    ));
    // GH #889: `in_answer` no longer writes the answer into a window, and an
    // interim ends no round (its calls are still out), so the reply is all.
    assert_eq!(out.len(), 1, "the reply alone: {out:?}");
    assert_eq!(out[0]["header"]["route"], "answer");
    assert_eq!(
        out[0]["header"]["interim"], "1",
        "a channel must be able to tell the two apart: {}",
        out[0]
    );
    // A final answer ends its round: the reply first, the `round-drop` behind it.
    let plain = emit(lane_doc(
        "in_answer",
        serde_json::json!([{"origin": "assistant", "type": "text", "text": "42"}]),
    ));
    assert_eq!(plain[0]["header"]["route"], "answer", "{plain:?}");
    assert!(
        plain[0]["header"].get("interim").is_none(),
        "a final answer carries no marker: {}",
        plain[0]
    );
}

#[test]
fn an_errand_that_arrives_as_a_tool_call_is_still_a_turn() {
    // The advisor connection (R-CG-3): the talky's dispatcher addresses the
    // agent core by tool NAME, so what reaches the core's in_turn lane is a
    // `tool_call` turn whose text is the raw arguments. That IS the question --
    // a turn written with empty content would make the core answer nothing.
    let out = emit(lane_doc(
        "in_turn",
        serde_json::json!([{"origin": "assistant", "type": "tool_call", "id": "c1",
                            "text": "{\"question\":\"weather in berlin\"}"}]),
    ));
    assert_eq!(
        op_of(&out[0])["row"]["content"],
        "{\"question\":\"weather in berlin\"}"
    );
    assert_eq!(
        op_of(&out[0])["row"]["role"],
        "user",
        "the asker is the user"
    );
}

#[test]
fn a_real_user_turn_still_wins_over_anything_beside_it() {
    let out = emit(lane_doc(
        "in_turn",
        serde_json::json!([
            {"origin": "user", "type": "text", "text": "the question"},
            {"origin": "assistant", "type": "text", "text": "some echo"}
        ]),
    ));
    assert_eq!(op_of(&out[0])["row"]["content"], "the question");
}
