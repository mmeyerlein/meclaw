//! Wave 11 -- the curator: continuous context-window management in `collector`.
//!
//! The collector could bound one ITEM (`tool_chars`) and it could
//! drop whole ITERATIONS (`round_bytes`), but nothing in it ever knew
//! how large the window it was building actually was. That is the gap this wave
//! closes, and it closes it with the opposite of the compaction every coding CLI
//! ships: no threshold, no single shot, no model call, no prose.
//!
//! Four claims are pinned here, one per group:
//!
//! 1. THE LEDGER KNOWS WHAT IT HOLDS (B1) -- the recoverability of a tool result
//!    is DECLARED per tool name and looked up, never inferred from the payload.
//!    Everything undeclared is `unique` and is therefore never touched: an
//!    undeclared tool costs context, never correctness.
//! 2. BUDGET, NOT THRESHOLD (B2) -- the trigger is the fill of the window the
//!    cell is about to send, measured against a model budget, with the
//!    provider's own token count preferred wherever it reaches the cell. A
//!    missing usage field estimates and says so; it never parks the turn.
//! 3. THE CURATOR (B3) -- staged, deterministic, model-free elision with a
//!    recovery key in every stub. The conversation, `system.*`, the tool_call
//!    NAMES and the newest rounds are out of reach at every stage and every
//!    budget.
//! 4. THE WAY BACK (B5) -- `thread_recall` is the difference between "condensed"
//!    and "lost": the collector serves the call itself, out of its own slate,
//!    uncapped, behind a per-turn budget wall that answers instead of truncating.
//!
//! Plus the gate nobody in the field runs: the CURATION-INVARIANCE GATE. The
//! same probe questions before and after N curations must have the same answer
//! basis in the window. It holds here for a structural reason rather than a
//! hopeful one -- the curator rewrites no text at all, so there is nothing for a
//! summary to drift away from.
//!
//! Everything runs the shipped `params.script_inline` against real stdin
//! documents. Nothing is mocked, no provider is called, nothing is spent.

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
/// script's own source (`build_stdin_json` withholds it). The key assertion
/// turns a typo in a knob name into a failure instead of a silent no-op.
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

// ───────────────────────────────────────────────────────────── document shapes

/// The knobs every curation case shares: the two per-item caps are lifted out
/// of the way so the CURATOR is what the assertion measures, not the pre-#91
/// preview caps that run before it.
fn base(window: &str, recoverability: &str) -> Vec<(&'static str, String)> {
    vec![
        ("context_window", window.to_string()),
        ("tool_chars", "1000000".into()),
        ("round_bytes", "10000000".into()),
        ("window_bytes", "1000000".into()),
        ("turn_chars", "1000000".into()),
        ("recoverability", recoverability.to_string()),
    ]
}

fn over<'a>(v: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    v.iter().map(|(k, s)| (*k, s.as_str())).collect()
}

fn call_turn(id: &str, name: &str, args: &str) -> serde_json::Value {
    serde_json::json!({
        "origin": "assistant", "type": "tool_call", "id": id,
        "text": serde_json::json!({"name": name, "arguments": args}).to_string()
    })
}

fn assistant_row(iter: i64, calls: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"turn_id": "t1", "iter": iter, "role": "assistant",
                       "turn": calls.to_string(), "fired": 0,
                       "recorded_at": "2026-08-15T00:00:00.000000Z"})
}

fn result_row(iter: i64, id: &str, text: &str) -> serde_json::Value {
    let turn = serde_json::json!({"origin": "tool", "type": "tool_result",
                                  "id": id, "text": text});
    serde_json::json!({"turn_id": "t1", "iter": iter, "role": "tool",
                       "turn": turn.to_string(), "fired": 0,
                       "recorded_at": "2026-08-15T00:00:00.000000Z"})
}

fn leg_window(turns: serde_json::Value) -> serde_json::Value {
    let payload = serde_json::json!({"turns": turns, "bytes": 0,
                                     "dropped": 0, "capped": 0});
    serde_json::json!({"turn_id": "t1", "iter": 0, "role": "leg-window",
                       "turn": payload.to_string(), "fired": 0,
                       "recorded_at": "2026-08-15T00:00:00.000000Z"})
}

/// The slate coming back on `round-check`: the assembly is about to leave for
/// the brain, and this is the last moment anything may be taken out of it.
///
/// GH #419: the result parks and the slate is read back in ONE message, so the
/// slate arrives under the read-back's `tool_call_id` and the round fires out of
/// that same reply. The curator runs where it always ran -- at the seam.
fn fire_at(iter: i64, rows: serde_json::Value) -> serde_json::Value {
    // A slate with an assistant row is a TOOL ROUND and fires out of the
    // `round-check` bundle; a slate that is only a window is the first assembly
    // of the turn and fires out of the `collect` bundle. Before GH #419 both
    // arrived at a phase whose only job was to render (`fire` / `round-fire`),
    // reached through a guarded update; now the render happens in the reply the
    // read-back came back on, so the fixture names the bundle it belongs to.
    let is_round = rows
        .as_array()
        .map(|rs| rs.iter().any(|r| r["role"] == "assistant"))
        .unwrap_or(false);
    let (phase, cid) = if is_round {
        ("round-check", "c-round-check-read")
    } else {
        ("collect", "c-collect-read")
    };
    // A round fires out of its own read-back now, so the slate has to BE
    // complete at `iter`. A fixture that names a later iteration than its own
    // rounds -- which is how a case says "these rounds are the old ones the
    // curator may spare or elide" -- gets the closing pair it implied, and it is
    // deliberately tiny: nothing about the curation under test may hang on it.
    let mut rows = rows;
    if is_round
        && !rows
            .as_array()
            .map(|rs| {
                rs.iter()
                    .any(|r| r["iter"] == iter && r["role"] == "assistant")
            })
            .unwrap_or(false)
    {
        let rs = rows.as_array_mut().expect("rows");
        rs.push(assistant_row(
            iter,
            serde_json::json!([call_turn("_close", "read_file", "{}")]),
        ));
        rs.push(result_row(iter, "_close", "ok"));
    }
    serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1",
                               "iter": iter.to_string(),
                               "col_phase": phase, "store_origin": "collector"},
                   "hop": {"operation": "bundle", "rows_affected": 0,
                           "bundle_errors": 0}},
        "messages": [{"origin": "tool", "type": "tool_result",
                      "id": cid, "text": rows.to_string()}],
        "results": [{"tool_call_id": cid, "operation": "select",
                     "rows_affected": 0, "duration_ms": 0}]
    })
}

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

fn texts(msg: &serde_json::Value) -> Vec<String> {
    msg["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|m| m["text"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn big(tag: &str, n: usize) -> String {
    format!("{tag}:{}", "x".repeat(n))
}

/// Six iterations of a tool loop over one large read and one large write, plus
/// a conversation that carries the three things a compaction is measured on: a
/// constraint, an obscure detail, and a time marker.
fn long_turn(iters: i64) -> serde_json::Value {
    let mut rows = vec![leg_window(serde_json::json!([
        {"role": "user", "text": "Rule: never touch /etc/shadow. The build id is 7734-QX. \
                                  I need this before 2026-09-03T14:00Z.", "consult_id": ""},
        {"role": "assistant", "text": "understood", "consult_id": ""}
    ]))];
    for i in 0..iters {
        rows.push(assistant_row(
            i,
            serde_json::json!([
                call_turn(
                    &format!("r{i}"),
                    "read_file",
                    &format!("{{\"path\":\"/a/{i}\"}}")
                ),
                call_turn(
                    &format!("w{i}"),
                    "write_file",
                    &big(&format!("{{\"path\":\"/b/{i}\",\"body\":\""), 3000)
                )
            ]),
        ));
        rows.push(result_row(i, &format!("r{i}"), &big("READ", 4000)));
        rows.push(result_row(i, &format!("w{i}"), &big("WROTE", 4000)));
    }
    serde_json::Value::Array(rows)
}

const DECLARED: &str = "read_file:repeatable,write_file:env";

// ══════════════════════════════════════════════ 1. THE LEDGER KNOWS WHAT IT HOLDS

#[test]
fn an_undeclared_tool_is_unique_and_is_never_elided() {
    // The default that makes the whole scheme safe: what nobody declared costs
    // context, never correctness. `web_search` is exactly the class the dossier
    // names -- a result nothing in the environment holds and no re-run
    // reproduces.
    let knobs = base("100", "");
    let out = emit_with(&over(&knobs), fire_at(5, long_turn(6)));
    let joined = texts(&out[0]).join("\n");
    assert!(
        !joined.contains("[elided tool_result"),
        "nothing was declared recoverable, so nothing may be elided:\n{joined}"
    );
    assert_eq!(
        out[0]["header"]["curate_mark"], "hard",
        "the window is far over both marks -- the curator TRIED and found nothing it may take"
    );
}

#[test]
fn the_env_class_goes_before_the_repeatable_class() {
    // Stage order is not cosmetic: an `env` result is a receipt for an effect
    // that is already in the world, a `repeatable` one still has to be re-run
    // to come back. The cheaper loss goes first, and the curator stops as soon
    // as it fits -- so with a budget that only needs one stage, the reads
    // survive and the writes do not.
    let knobs = base("30000", DECLARED);
    let out = emit_with(&over(&knobs), fire_at(5, long_turn(6)));
    let joined = texts(&out[0]).join("\n");
    assert!(
        joined.contains("kind=env"),
        "stage 1 must have run:\n{joined}"
    );
    assert!(
        !joined.contains("kind=repeatable"),
        "stage 1 was enough -- stage 2 must not have run:\n{joined}"
    );
    assert_eq!(out[0]["header"]["curate_stage"], "1");
}

#[test]
fn a_tighter_budget_walks_on_to_the_next_stage() {
    // Same slate, less room: the curator does MORE, in the declared order, and
    // reports which stage it had to reach. That number is the operator's whole
    // early-warning system.
    let knobs = base("400", DECLARED);
    let out = emit_with(&over(&knobs), fire_at(5, long_turn(6)));
    let joined = texts(&out[0]).join("\n");
    assert!(joined.contains("kind=env"), "stage 1 ran:\n{joined}");
    assert!(joined.contains("kind=repeatable"), "stage 2 ran:\n{joined}");
    assert!(
        joined.contains("[elided arguments"),
        "stage 3 ran:\n{joined}"
    );
    assert_eq!(out[0]["header"]["curate_stage"], "3");
}

// ═══════════════════════════════════════════════════ 2. BUDGET, NOT THRESHOLD

#[test]
fn a_window_under_the_soft_mark_is_left_alone() {
    // The curator is continuous, not eager: it runs at every assembly and does
    // NOTHING while the window is comfortable. A procedure that shaves context
    // it does not need to shave pays cache misses for free.
    let knobs = base("10000000", DECLARED);
    let out = emit_with(&over(&knobs), fire_at(5, long_turn(6)));
    let joined = texts(&out[0]).join("\n");
    assert!(!joined.contains("[elided"), "nothing to do:\n{joined}");
    assert_eq!(out[0]["header"]["curate_mark"], "none");
    assert_eq!(out[0]["header"]["curate_stage"], "0");
    assert_eq!(out[0]["header"]["curate_elided"], "0");
}

#[test]
fn curation_is_off_until_a_window_budget_is_named() {
    // The whole wave is dark by default: without `context_window`
    // every byte of behaviour is the pre-wave-11 behaviour, which is what makes
    // the change revertible in production by emptying one variable.
    let mut knobs = base("", DECLARED);
    knobs.retain(|(k, _)| *k != "context_window");
    let out = emit_with(&over(&knobs), fire_at(5, long_turn(6)));
    let joined = texts(&out[0]).join("\n");
    assert!(!joined.contains("[elided"), "curation is off:\n{joined}");
    assert_eq!(out[0]["header"]["curate_mark"], "none");
}

#[test]
fn the_two_marks_are_reported_apart() {
    // MemGPT's shape: a courtesy mark before the emergency one. `soft` says the
    // curator is working, `hard` says it is out of stages and the next thing to
    // give is prose -- the one signal a fold lane will subscribe to.
    let soft = emit_with(&over(&base("30000", DECLARED)), fire_at(5, long_turn(6)));
    let hard = emit_with(&over(&base("20000", DECLARED)), fire_at(5, long_turn(6)));
    assert_eq!(soft[0]["header"]["curate_mark"], "soft");
    assert_eq!(hard[0]["header"]["curate_mark"], "hard");
}

#[test]
fn the_curator_never_blocks_the_round() {
    // The reason this is not a fold: curation costs no extra message, no extra
    // hop, no model call and no round trip. The assembly it curated leaves in
    // the SAME emission it would have left in uncurated -- there is nothing for
    // the turn to wait on, at any mark including `hard`.
    let plain = emit_with(&over(&base("", DECLARED)), fire_at(5, long_turn(6)));
    let curated = emit_with(&over(&base("50", DECLARED)), fire_at(5, long_turn(6)));
    assert_eq!(emitted(&plain), 1);
    assert_eq!(emitted(&curated), 1, "one seam, curated or not");
    assert_eq!(curated[0]["header"]["route"], plain[0]["header"]["route"]);
    assert_eq!(curated[0]["header"]["iter"], plain[0]["header"]["iter"]);
    assert_eq!(curated[0]["header"]["curate_mark"], "hard");
    let n = |v: &serde_json::Value| v["messages"].as_array().expect("messages").len();
    assert_eq!(
        n(&curated[0]),
        n(&plain[0]),
        "the same turns leave -- payloads were replaced, no row was removed"
    );
}

#[test]
fn a_conversation_without_tool_results_is_never_damaged() {
    // The talky shape, and the safety proof for a colony-global knob: a window
    // that is only conversation is over every mark at a small budget, the
    // curator walks all three stages, finds nothing it may take, and hands the
    // turns on byte for byte. A channel voice cannot be hurt by a budget that
    // was meant for a tool-loop core.
    let rows = serde_json::json!([leg_window(serde_json::json!([
        {"role": "user", "text": "a".repeat(3000), "consult_id": ""},
        {"role": "assistant", "text": "b".repeat(3000), "consult_id": ""}
    ]))]);
    let out = emit_with(&over(&base("100", DECLARED)), fire_at(0, rows));
    assert_eq!(out[0]["header"]["curate_mark"], "hard");
    assert_eq!(out[0]["header"]["curate_stage"], "3", "it tried everything");
    assert_eq!(out[0]["header"]["curate_elided"], "0", "and took nothing");
    assert_eq!(texts(&out[0]), vec!["a".repeat(3000), "b".repeat(3000)]);
}

#[test]
fn a_missing_usage_field_estimates_and_says_so_instead_of_parking() {
    // L2's failure class, and it is the expensive one: when `tokens_prompt` is
    // absent the CEL evaluation of a threshold edge fails, BOTH branches of an
    // exact partition drop out, and the turn parks in silence. Here the trigger
    // is computed inside the cell, so the absence costs an estimate and a flag
    // -- never a turn.
    let out = emit_with(&over(&base("4000", DECLARED)), fire_at(5, long_turn(6)));
    assert_eq!(emitted(&out), 1, "the turn left; nothing parked");
    assert_eq!(out[0]["header"]["route"], "brain");
    assert_eq!(
        out[0]["header"]["tokens_estimated"], "1",
        "an estimate must declare itself an estimate"
    );
    let w: i64 = out[0]["header"]["tokens_window"]
        .as_str()
        .expect("tokens_window")
        .parse()
        .expect("number");
    assert!(w > 0, "the estimate is a number, not a shrug");
}

#[test]
fn the_provider_token_count_wins_over_the_estimate_when_it_reaches_the_cell() {
    // `hop.tokens_prompt` is the one number in the system that is not a guess.
    // It is stored when it arrives on the calls lane and preferred from then on;
    // `tokens_estimated` flips to 0 so an operator can see WHICH number a
    // decision was taken on.
    let stored = emit_with(
        &over(&base("4000", DECLARED)),
        serde_json::json!({
            "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0"},
                       "hop": {"route": "in_calls", "tokens_prompt": 31337}},
            "messages": [call_turn("c1", "read_file", "{}")]
        }),
    );
    let op: serde_json::Value =
        serde_json::from_str(stored[0]["messages"][0]["text"].as_str().expect("op")).expect("json");
    let turn: serde_json::Value =
        serde_json::from_str(op["row"]["turn"].as_str().expect("turn")).expect("json");
    assert_eq!(
        turn[0]["tp"], 31337,
        "the provider count is kept with the round it belongs to"
    );

    // ... and read back at the seam.
    let rows = serde_json::json!([
        leg_window(serde_json::json!([{"role": "user", "text": "hi", "consult_id": ""}])),
        {"turn_id": "t1", "iter": 0, "role": "assistant",
         "turn": serde_json::json!([{
             "origin": "assistant", "type": "tool_call", "id": "c1", "tp": 31337,
             "text": serde_json::json!({"name": "read_file", "arguments": "{}"}).to_string()
         }]).to_string(),
         "fired": 0, "recorded_at": "2026-08-15T00:00:00.000000Z"},
        result_row(0, "c1", "small")
    ]);
    let out = emit_with(&over(&base("100000", DECLARED)), fire_at(0, rows));
    assert_eq!(out[0]["header"]["tokens_window"], "31337");
    assert_eq!(out[0]["header"]["tokens_estimated"], "0");
    assert!(
        !texts(&out[0]).join("").contains("31337"),
        "the marker is bookkeeping and never reaches the wire"
    );
}

// ═══════════════════════════════════════════════════════════════ 3. THE CURATOR

#[test]
fn an_elided_result_carries_its_own_way_back() {
    // Manus' restorable compression, in one line: the payload leaves, the key
    // stays. Four things have to be in the stub or it is just a loss with
    // better manners -- the call id, the size, a content hash (so the same
    // payload elided twice reads as the same reference) and the literal recall
    // call.
    let out = emit_with(&over(&base("30000", DECLARED)), fire_at(5, long_turn(6)));
    let stub = texts(&out[0])
        .into_iter()
        .find(|t| t.starts_with("[elided tool_result"))
        .expect("a stub");
    assert!(stub.contains("w0"), "the call id: {stub}");
    assert!(stub.contains("tool=write_file"), "the tool name: {stub}");
    assert!(stub.contains("kind=env"), "the class: {stub}");
    assert!(stub.contains("sha256:"), "the content hash: {stub}");
    assert!(
        stub.contains("thread_recall(call_id=\"w0\")"),
        "the way back: {stub}"
    );
}

#[test]
fn the_same_payload_elides_to_the_same_reference() {
    // A content hash instead of a serial number buys deduplication for free:
    // two identical results anywhere in the window print the same key.
    let rows = serde_json::json!([
        leg_window(serde_json::json!([{"role": "user", "text": "go", "consult_id": ""}])),
        assistant_row(0, serde_json::json!([call_turn("a", "write_file", "{}")])),
        result_row(0, "a", &big("SAME", 6000)),
        assistant_row(1, serde_json::json!([call_turn("b", "write_file", "{}")])),
        result_row(1, "b", &big("SAME", 6000)),
        assistant_row(2, serde_json::json!([call_turn("c", "write_file", "{}")])),
        result_row(2, "c", &big("OTHER", 6000)),
    ]);
    let out = emit_with(&over(&base("300", DECLARED)), fire_at(4, rows));
    let hashes: Vec<String> = texts(&out[0])
        .into_iter()
        .filter(|t| t.contains("sha256:"))
        .map(|t| {
            let i = t.find("sha256:").expect("hash");
            t[i..i + 19].to_string()
        })
        .collect();
    assert_eq!(hashes.len(), 3, "three results were elided");
    assert_eq!(hashes[0], hashes[1], "same payload, same reference");
    assert_ne!(
        hashes[1], hashes[2],
        "different payload, different reference"
    );
}

#[test]
fn the_newest_rounds_stay_verbatim_at_any_budget() {
    // CompactionRL keeps k=2 and so does this: whatever the budget says, the
    // rounds an agent is actually working in are never touched. A curator that
    // elides what the model is holding in its hands right now is not saving
    // context, it is causing a retry.
    let out = emit_with(&over(&base("50", DECLARED)), fire_at(5, long_turn(6)));
    let all = texts(&out[0]).join("\n");
    for keep in ["r4", "w4", "r5", "w5"] {
        assert!(
            !all.contains(&format!("[elided tool_result {keep} ")),
            "iteration 4 and 5 are the newest two and must be verbatim:\n{all}"
        );
    }
    assert!(
        all.contains("[elided tool_result w0 "),
        "the old ones did go:\n{all}"
    );
}

#[test]
fn the_tool_call_name_survives_every_stage() {
    // Anthropic's `clear_tool_inputs: false`, and the reason for it: the record
    // of WHAT was done is what stops an agent from doing it a second time. The
    // arguments may go; the name is the action protocol and stays.
    let out = emit_with(&over(&base("50", DECLARED)), fire_at(5, long_turn(6)));
    let all = texts(&out[0]).join("\n");
    assert!(
        all.contains("[elided arguments"),
        "stage 3 ran at this budget:\n{all}"
    );
    assert!(
        all.contains("\"name\": \"write_file\"") || all.contains("\"name\":\"write_file\""),
        "the name is still there:\n{all}"
    );
}

#[test]
fn every_tool_call_still_has_a_result() {
    // The invariant every provider enforces and every compaction breaks by
    // accident: a `tool_call` without its `tool_result` is a malformed turn.
    // Elision replaces payloads; it never removes a row.
    let out = emit_with(&over(&base("50", DECLARED)), fire_at(5, long_turn(6)));
    let msgs = out[0]["messages"].as_array().expect("messages");
    let calls: Vec<String> = msgs
        .iter()
        .filter(|m| m["type"] == "tool_call")
        .map(|m| m["id"].as_str().unwrap_or_default().to_string())
        .collect();
    let results: Vec<String> = msgs
        .iter()
        .filter(|m| m["type"] == "tool_result")
        .map(|m| m["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(calls.len(), 12, "six iterations, two calls each");
    for c in &calls {
        assert!(results.contains(c), "call {c} lost its result");
    }
}

// ══════════════════════════════════════════ THE CURATION-INVARIANCE GATE (L10)

/// The three classes the field measures compaction on -- and Anthropic's own
/// number for the third one after a single prose fold is 0/3.
const PROBES: &[(&str, &str)] = &[
    ("constraint", "never touch /etc/shadow"),
    ("detail", "7734-QX"),
    ("time marker", "2026-09-03T14:00Z"),
];

#[test]
fn the_invariance_gate_holds_at_every_stage() {
    // The gate nobody in the field runs, because everybody's compaction is a
    // model call and a model call cannot promise this. Here it holds for a
    // structural reason: the curator never rewrites a word. Constraints,
    // obscure details and time markers live in the conversation window and in
    // `unique` results -- and the curator has no path to either, at any stage.
    let mut seen = 0;
    for budget in ["10000000", "4000", "400", "50"] {
        let out = emit_with(&over(&base(budget, DECLARED)), fire_at(5, long_turn(6)));
        let all = texts(&out[0]).join("\n");
        for (class, probe) in PROBES {
            assert!(
                all.contains(probe),
                "{class} lost at budget {budget} (stage {}):\n{all}",
                out[0]["header"]["curate_stage"]
            );
        }
        seen += 1;
    }
    assert_eq!(seen, 4, "all four budgets were probed");
}

#[test]
fn two_curations_in_a_row_do_not_drift() {
    // Summary drift is accumulated distortion over N folds -- the thing the
    // dossier says nobody has ever plotted. Here the second curation is fed the
    // OUTPUT of the first, and the answer basis is identical, because eliding
    // an already-elided payload is a no-op rather than a second lossy pass.
    let first = emit_with(&over(&base("400", DECLARED)), fire_at(5, long_turn(6)));
    let one = texts(&first[0]).join("\n");

    // Feed the curated window back in as the slate of the next round.
    let mut rows = vec![leg_window(serde_json::json!([
        {"role": "user", "text": "Rule: never touch /etc/shadow. The build id is 7734-QX. \
                                  I need this before 2026-09-03T14:00Z.", "consult_id": ""},
        {"role": "assistant", "text": "understood", "consult_id": ""}
    ]))];
    let msgs = first[0]["messages"].as_array().expect("messages");
    for i in 0..6 {
        let calls: Vec<serde_json::Value> = msgs
            .iter()
            .filter(|m| m["type"] == "tool_call")
            .filter(|m| {
                let id = m["id"].as_str().unwrap_or_default();
                id == format!("r{i}") || id == format!("w{i}")
            })
            .cloned()
            .collect();
        rows.push(assistant_row(i, serde_json::Value::Array(calls)));
        for m in msgs.iter().filter(|m| m["type"] == "tool_result") {
            let id = m["id"].as_str().unwrap_or_default().to_string();
            if id == format!("r{i}") || id == format!("w{i}") {
                rows.push(result_row(i, &id, m["text"].as_str().unwrap_or_default()));
            }
        }
    }
    let second = emit_with(
        &over(&base("400", DECLARED)),
        fire_at(5, serde_json::Value::Array(rows)),
    );
    let two = texts(&second[0]).join("\n");

    for (class, probe) in PROBES {
        assert!(
            two.contains(probe),
            "{class} lost on the SECOND curation:\n{two}"
        );
    }
    assert_eq!(
        one, two,
        "a second curation over an already curated window is a fixed point"
    );
}

// ══════════════════════════════════════════════════════════════ 4. THE WAY BACK

#[test]
fn thread_recall_asks_this_turns_own_slate() {
    let out = emit_with(
        &over(&base("4000", DECLARED)),
        serde_json::json!({
            "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "3"},
                       "hop": {"route": "in_thread_call"}},
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "tr1",
                          "text": "{\"call_id\":\"w0\"}"}]
        }),
    );
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["header"]["route"], "cstore");
    assert_eq!(out[0]["header"]["phase"], "tr-sel");
    let op: serde_json::Value =
        serde_json::from_str(out[0]["messages"][0]["text"].as_str().expect("op")).expect("json");
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "round");
    assert_eq!(
        op["where"]["turn_id"], "t1",
        "a thread recall never leaves its own turn"
    );
    assert_eq!(
        out[0]["header"]["turn_id"], "t1|tr1|call|w0",
        "the chain state rides in the hop id -- this script keeps none between hops"
    );
}

#[test]
fn thread_recall_brings_the_payload_back_uncapped() {
    // The difference between "condensed" and "lost", and the reason the stubs
    // above are honest: the ORIGINAL text comes back, under the original
    // tool_call_id, filed as a result of the running round.
    let doc = serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1|tr1|call|w0",
                               "iter": "3", "col_phase": "tr-sel",
                               "store_origin": "collector"},
                   "hop": {"operation": "select", "rows_affected": 0}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "x",
                      "text": long_turn(6).to_string()}]
    });
    let out = emit_with(&over(&base("100000", DECLARED)), doc);
    let op: serde_json::Value =
        serde_json::from_str(out[0]["messages"][0]["text"].as_str().expect("op")).expect("json");
    assert_eq!(op["table"], "round");
    assert_eq!(
        op["row"]["turn_id"], "t1",
        "the real turn id is split back out"
    );
    let turn: serde_json::Value =
        serde_json::from_str(op["row"]["turn"].as_str().expect("turn")).expect("json");
    assert_eq!(
        turn["id"], "tr1",
        "answered under the ORIGINAL tool_call_id"
    );
    assert_eq!(
        turn["tr"], 1,
        "marked as recall spend for this turn's budget"
    );
    let text = turn["text"].as_str().expect("text");
    assert!(
        text.contains(&big("WROTE", 4000)),
        "the whole payload, uncapped"
    );
    assert!(
        !text.contains(&big("READ", 4000)),
        "and only what was asked for"
    );
}

#[test]
fn thread_recall_over_budget_answers_with_a_typed_error() {
    // The wall against a model that recalls everything back in and eats the
    // saving. It REFUSES and says why -- a recall silently halved is a lie
    // about what the model was shown.
    let doc = serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1|tr1|round|0",
                               "iter": "3", "col_phase": "tr-sel",
                               "store_origin": "collector"},
                   "hop": {"operation": "select", "rows_affected": 0}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "x",
                      "text": long_turn(6).to_string()}]
    });
    let out = emit_with(&over(&base("100", DECLARED)), doc);
    let op: serde_json::Value =
        serde_json::from_str(out[0]["messages"][0]["text"].as_str().expect("op")).expect("json");
    let turn: serde_json::Value =
        serde_json::from_str(op["row"]["turn"].as_str().expect("turn")).expect("json");
    let text = turn["text"].as_str().expect("text");
    assert!(text.contains("recall budget"), "it says which wall: {text}");
    assert!(
        !text.contains(&big("WROTE", 4000)),
        "and nothing came through"
    );
}

#[test]
fn thread_recall_switched_off_answers_instead_of_parking() {
    // The dispatcher-lid pattern, one lane further in: a call that cannot be
    // served is ANSWERED, because asking into a port that is not there hangs
    // the fan-in until the idle exit.
    let mut knobs = base("4000", DECLARED);
    knobs.push(("thread_recall", String::new()));
    let out = emit_with(
        &over(&knobs),
        serde_json::json!({
            "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "3"},
                       "hop": {"route": "in_thread_call"}},
            "messages": [{"origin": "assistant", "type": "tool_call", "id": "tr1",
                          "text": "{\"call_id\":\"w0\"}"}]
        }),
    );
    assert_eq!(out.len(), 1);
    let op: serde_json::Value =
        serde_json::from_str(out[0]["messages"][0]["text"].as_str().expect("op")).expect("json");
    assert_eq!(op["operation"], "insert", "an answer, not a question");
    let turn: serde_json::Value =
        serde_json::from_str(op["row"]["turn"].as_str().expect("turn")).expect("json");
    assert_eq!(turn["id"], "tr1");
    assert!(
        turn["text"]
            .as_str()
            .unwrap_or_default()
            .contains("not configured")
    );
}

#[test]
fn a_recalled_payload_is_not_shown_twice() {
    // "One representation per prompt": what a recall brought back must not
    // stand next to the stub that pointed at it. The stub stays as the ANSWER
    // to its call -- dropping the row would leave a tool_call unanswered -- but
    // it stops repeating the content it no longer owns.
    let rows = serde_json::json!([
        leg_window(serde_json::json!([{"role": "user", "text": "go", "consult_id": ""}])),
        assistant_row(0, serde_json::json!([call_turn("a", "web_search", "{}")])),
        result_row(0, "a", &big("UNIQUE", 6000)),
        assistant_row(
            1,
            serde_json::json!([call_turn("tr1", "thread_recall", "{\"call_id\":\"a\"}")])
        ),
        result_row(1, "tr1", &big("UNIQUE", 6000)),
        assistant_row(2, serde_json::json!([call_turn("z", "read_file", "{}")])),
        result_row(2, "z", "tiny"),
    ]);
    let out = emit_with(&over(&base("300", DECLARED)), fire_at(4, rows));
    let msgs = out[0]["messages"].as_array().expect("messages");
    let a = msgs
        .iter()
        .find(|m| m["id"] == "a" && m["type"] == "tool_result")
        .expect("the original result row is still there");
    assert!(
        a["text"]
            .as_str()
            .unwrap_or_default()
            .contains("kind=recalled"),
        "a `unique` result IS elided once it has been recalled: {a}"
    );
    let tr = msgs
        .iter()
        .find(|m| m["id"] == "tr1" && m["type"] == "tool_result")
        .expect("the recall result");
    assert!(
        tr["text"]
            .as_str()
            .unwrap_or_default()
            .contains(&big("UNIQUE", 6000)),
        "and the recall keeps the one representation"
    );
}

// ═══════════════════════════════════════════════════════════════════════ pins

#[test]
fn the_private_markers_never_reach_the_wire() {
    // `lost`, `tr`, `tp` and `_it` are this cell's bookkeeping. A provider that
    // sees them gets a malformed turn, and an operator who greps for them in a
    // request log should find nothing.
    let out = emit_with(&over(&base("400", DECLARED)), fire_at(5, long_turn(6)));
    for m in out[0]["messages"].as_array().expect("messages") {
        for key in ["lost", "tr", "tp", "_it"] {
            assert!(
                m.get(key).is_none(),
                "private marker {key} leaked onto the wire: {m}"
            );
        }
    }
}

#[test]
fn the_shipped_contract_declares_every_new_knob_and_key() {
    // The contract is the operator's only map of this cell. A knob that works
    // but is not declared is a knob nobody finds.
    let raw = std::fs::read_to_string(ASSEMBLE_CONFIG).expect("assemble config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    for knob in [
        "context_window",
        "curate_soft",
        "curate_hard",
        "keep_rounds",
        "recoverability",
        "thread_recall",
        "thread_recall_budget",
    ] {
        assert!(
            v["contract"]["settings"][knob].is_object(),
            "contract.settings.{knob} is not declared"
        );
    }
    for key in [
        "tokens_window",
        "tokens_projected",
        "tokens_estimated",
        "curate_mark",
        "curate_stage",
        "curate_elided",
        "curate_saved",
    ] {
        assert!(
            v["contract"]["emits"]["hop"][key].is_object(),
            "contract.emits.hop.{key} is not declared"
        );
    }
    assert_eq!(
        v["contract"]["version"], "1.3.0",
        "wave 11 added a lane and seven hop keys; wave 13 moved every setting \
         off the environment onto params; GH #372 added `consumes.hop.handoff_calls` \
         -- all three are minor versions, because none of them takes anything away"
    );
}
