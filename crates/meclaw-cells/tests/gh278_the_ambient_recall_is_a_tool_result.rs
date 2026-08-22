//! GH #278 — the ambient recall arrives as the RESULT OF A CALL, not as
//! durable state.
//!
//! Until this wave the collector put the recall bundle into `system.memory` and
//! left it there. Three things were wrong with that, and all three are the same
//! mistake in different clothes:
//!
//! 1. `system.*` is DURABLE state in the brain cell — upserted per slot path,
//!    never expiring on its own. A bundle written under a fixed path is
//!    re-stated every turn as a standing truth about the agent, in the same
//!    place its instructions live, with no marker that it is the answer to one
//!    question asked once.
//! 2. A model reading it cannot tell WHERE it came from. Evidence produced by a
//!    lookup, presented as configuration, is evidence whose provenance the
//!    reader has to guess.
//! 3. It was un-attributable. The bytes were never outside the budget —
//!    `curate` is handed `len(json.dumps(sysm))` and counts it into every
//!    projection — but they arrived as an anonymous lump of `sys_chars` in a
//!    subtree the curator may not touch. The one payload that grows with an
//!    agent's memory could be measured and never accounted for.
//!
//! The repair is not a new mechanism: the collector already serves a
//! `memory_recall` tool on its `in_memory_call` lane, and a tool result is
//! exactly what a bundle IS. So the ambient leg now leaves as a synthetic
//! tool_call / tool_result pair at the end of `messages[]` — evidence of THIS
//! round, counted item by item in the round it belongs to, under a call id
//! derived from the bundle so a retry is the same call rather than a second
//! one. What stays behind in `system.memory` is the revocation and nothing
//! else.
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

/// A message as a port edge delivers it: the lane on the hop, the session in
/// context.
fn lane_doc(route: &str, ctx_extra: &[(&str, &str)], body: serde_json::Value) -> serde_json::Value {
    let mut doc = body;
    let mut ctx = serde_json::json!({"session_id": "s1", "turn_id": "t1", "iter": "0"});
    for (k, v) in ctx_extra {
        ctx[*k] = serde_json::json!(v);
    }
    doc["header"] = serde_json::json!({"context": ctx, "hop": {"route": route}});
    doc
}

/// A store reply as the hive's own edge delivers it back: the step in context,
/// the operation and the guard signal on the hop.
fn reply_doc(phase: &str, rows_affected: i64, payload: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0",
                               "col_phase": phase, "store_origin": "collector"},
                   "hop": {"operation": "select", "rows_affected": rows_affected}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "x",
                      "text": payload.to_string()}]
    })
}

/// A materialised `leg-window` row, as the `win` step writes it.
fn leg_window_row(turns: serde_json::Value) -> serde_json::Value {
    let payload = serde_json::json!({"turns": turns, "bytes": 0,
                                     "dropped": 0, "capped": 0});
    serde_json::json!({"turn_id": "t1", "iter": 0, "role": "leg-window",
                       "turn": payload.to_string(), "fired": 0})
}

/// A materialised `leg-memory` row, as the `in_bundle` lane writes it: the
/// bundle's `system` and `messages[]`, and nothing else of the body.
fn leg_memory_row(payload: serde_json::Value) -> serde_json::Value {
    serde_json::json!({"turn_id": "t1", "iter": 0, "role": "leg-memory",
                       "turn": payload.to_string(), "fired": 0})
}

const QUERY: &str = "and my editor?";
const AS_OF: &str = "2026-08-16T00:00:00Z";

/// The readable half of a `memory-hive@2.3.0` bundle, in the form the recall
/// script renders it since #279/#281/#296: an ASSERTING opening line, then one
/// section per kind.
const READABLE: &str = "WHAT THIS MEMORY HOLDS (as of 2026-08-16)\n\
                        FACTS (extracted, canonical, dated)\n  \
                        alex editor = the editor is helix   since 2026-08-01\n\
                        WHAT WAS SAID (verbatim, not interpreted)\n  \
                        alex on 2026-08-14: \"i finally switched to helix\"";

/// The machine-readable half, with the slim payload candidates of #296: no row
/// ids, no fused scores, no legs.
fn bundle_json(as_of: &str, query: &str) -> String {
    serde_json::json!({
        "answers": "direct",
        "as_of": as_of,
        "beliefs": [],
        "candidates": [{"kind": "fact", "predicate": "editor", "subject": "alex",
                        "text": "the editor is helix", "valid_from": "2026-08-01"}],
        "complete": true,
        "query": query,
        "tier": 1,
        "token_estimate": 61
    })
    .to_string()
}

/// A whole bundle as it reaches the `in_bundle` lane, minus the
/// `recall_diagnostic` slot (which the lane drops — see the test that pins it).
fn bundle_body(as_of: &str, query: &str, readable: &str) -> serde_json::Value {
    serde_json::json!({
        "system": {"memory": {"bundle": {"text": bundle_json(as_of, query)}}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                      "text": readable}]
    })
}

/// The OTHER shipped bundle shape, and the one that broke the first cut of this
/// package: tier 0 is a deterministic projection, not a dated lookup, so its
/// JSON carries `query` and NO `as_of` at all (`memory-hive`'s `fire` phase,
/// `bundle = {"query": …, "tier": 0, "beliefs": …, "foresight": …, "episodes": …}`).
fn tier_zero_body(query: &str) -> serde_json::Value {
    let bundle = serde_json::json!({
        "beliefs": [{"statement": "the editor is helix"}],
        "episodes": [],
        "foresight": [],
        "query": query,
        "tier": 0,
        "token_estimate": 7
    })
    .to_string();
    serde_json::json!({
        "system": {"memory": {"bundle": {"text": bundle}}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                      "text": "MEMORY (tier 0, deterministic bundle)\n\
                               - belief: the editor is helix"}]
    })
}

/// The assembly of one turn whose memory leg came back with `payload`.
fn assemble(over: &[(&str, &str)], payload: serde_json::Value) -> serde_json::Value {
    let rows = serde_json::json!([
        leg_window_row(serde_json::json!([{"role": "user", "text": QUERY}])),
        leg_memory_row(payload)
    ]);
    let out = emit_with(over, reply_doc("fire", 2, rows));
    assert_eq!(out.len(), 1, "ONE seam: {out:?}");
    out.into_iter().next().expect("the seam message")
}

/// The last two entries of the assembled `messages[]`.
fn pair(msg: &serde_json::Value) -> (serde_json::Value, serde_json::Value) {
    let msgs = msg["messages"].as_array().expect("messages");
    assert!(msgs.len() >= 2, "no pair to read: {msg}");
    (msgs[msgs.len() - 2].clone(), msgs[msgs.len() - 1].clone())
}

// ═══════════════════════════════════════════════════ 1. THE BUNDLE IS EVIDENCE

/// The whole of #278 in one assertion: the ambient bundle leaves the seam as a
/// tool_call answered by a tool_result, at the END of the conversation, under
/// one shared id — the shape a provider already has a meaning for.
#[test]
fn the_ambient_bundle_leaves_as_a_tool_result_pair() {
    let msg = assemble(&[("memory_tier", "0")], bundle_body(AS_OF, QUERY, READABLE));
    let (call, result) = pair(&msg);

    assert_eq!(call["origin"], "assistant", "{msg}");
    assert_eq!(call["type"], "tool_call", "{msg}");
    assert_eq!(result["origin"], "tool", "{msg}");
    assert_eq!(result["type"], "tool_result", "{msg}");

    let id = call["id"].as_str().expect("a call id").to_string();
    assert_eq!(
        result["id"], call["id"],
        "a result answers its call or it is a malformed turn for every \
         provider: {msg}"
    );
    let hex = id
        .strip_prefix("call_recall_")
        .unwrap_or_else(|| panic!("the id names what it is: {id}"));
    assert_eq!(hex.len(), 16, "16 hex of a digest: {id}");
    assert!(
        hex.chars().all(|c| c.is_ascii_hexdigit()),
        "16 hex of a digest: {id}"
    );

    // The name is the one this collector ALREADY serves on `in_memory_call`
    // (GH #78). A model that reacts to the synthetic call by calling the tool
    // itself therefore reaches a real edge instead of a void.
    let fun: serde_json::Value =
        serde_json::from_str(call["text"].as_str().expect("call text")).expect("the call is json");
    assert_eq!(fun["name"], "memory_recall", "{msg}");
    let args: serde_json::Value =
        serde_json::from_str(fun["arguments"].as_str().expect("arguments string"))
            .expect("arguments are json");
    assert_eq!(
        args["query"], QUERY,
        "the call says what it asked, out of the bundle's own record of it: {msg}"
    );

    // VERBATIM, up to the cap: the collector renders nothing of its own here.
    assert_eq!(
        result["text"], READABLE,
        "the readable form the memory hive rendered, byte for byte: {msg}"
    );
}

/// The conversation stays a conversation: the window turns come first, the pair
/// is appended after them, and nothing of the bundle is smuggled into a user or
/// assistant text turn.
#[test]
fn the_pair_sits_behind_the_window_and_not_inside_it() {
    let msg = assemble(&[("memory_tier", "0")], bundle_body(AS_OF, QUERY, READABLE));
    let msgs = msg["messages"].as_array().expect("messages");
    assert_eq!(msgs.len(), 3, "one window turn plus the pair: {msg}");
    assert_eq!(msgs[0]["text"], QUERY);
    assert_eq!(msgs[0]["type"], "text");
}

// ═════════════════════════════════════════════════════ 2. A RETRY IS ONE CALL

/// The id is DERIVED from the bundle, so the same bundle is the same call and a
/// re-assembly of the same turn (a tool round re-entering the seam, a retried
/// delivery) is not a second question the model has to explain to itself.
#[test]
fn the_call_id_is_the_same_for_the_same_bundle_and_different_for_another() {
    let over = [("memory_tier", "0")];
    let once = assemble(&over, bundle_body(AS_OF, QUERY, READABLE));
    let twice = assemble(&over, bundle_body(AS_OF, QUERY, READABLE));
    assert_eq!(
        pair(&once).0["id"],
        pair(&twice).0["id"],
        "a retry must not look like a second call"
    );

    // A different recall is a different call. `as_of` is what tells two runs of
    // the SAME question apart — the query alone would collide across turns.
    let later = assemble(&over, bundle_body("2026-08-17T00:00:00Z", QUERY, READABLE));
    assert_ne!(
        pair(&once).0["id"],
        pair(&later).0["id"],
        "a second recall of the same question is a second call"
    );
}

/// The regression that the first cut of this package shipped, reproduced: a
/// TIER-0 bundle carries `query` and no `as_of`, so gating the QUESTION on
/// `as_of` — as the identity has to be gated — emitted a call that asked
/// `{"query": ""}` on every tier-0 turn. A synthetic call whose arguments are
/// empty is worse than no call: it tells the model memory was asked nothing.
///
/// Two reads over one slot: the query comes from the first parsing object that
/// carries one, the identity still needs `as_of` and falls back to the rendered
/// block when there is none — which is stable for the same bundle, and that is
/// what an id has to be.
#[test]
fn a_tier_zero_bundle_still_names_the_question_it_answered() {
    let over = [("memory_tier", "0")];
    let once = assemble(&over, tier_zero_body(QUERY));
    let (call, result) = pair(&once);

    let fun: serde_json::Value =
        serde_json::from_str(call["text"].as_str().expect("call text")).expect("the call is json");
    let args: serde_json::Value =
        serde_json::from_str(fun["arguments"].as_str().expect("arguments string"))
            .expect("arguments are json");
    assert_eq!(
        args["query"], QUERY,
        "a tier-0 bundle names its question in the JSON and nowhere else: {once}"
    );
    assert_eq!(
        result["text"], "MEMORY (tier 0, deterministic bundle)\n- belief: the editor is helix",
        "{once}"
    );

    // No `as_of` to hash, so the identity is the rendered block — and it is
    // stable, which is the whole requirement.
    let twice = assemble(&over, tier_zero_body(QUERY));
    assert_eq!(
        call["id"],
        pair(&twice).0["id"],
        "the same tier-0 bundle is the same call"
    );
    assert!(
        call["id"]
            .as_str()
            .expect("a call id")
            .starts_with("call_recall_"),
        "{once}"
    );
    assert_ne!(
        call["id"],
        pair(&assemble(&over, bundle_body(AS_OF, QUERY, READABLE))).0["id"],
        "a different bundle is a different call, whatever it is keyed on"
    );
}

// ══════════════════════════════════════════════════ 3. THE OLD SLOT IS REVOKED

/// What stays behind under `system.memory` is the revocation and nothing else.
///
/// Both halves are needed and neither carries data any more. Without the empty
/// leaf the previous turn's bundle stands in the prompt forever — `system.*` is
/// upserted per slot path, and a path nobody sends is a path nobody touches.
/// Without `$replace` the hive-named keys of the `json` form stand forever for
/// the same reason (GH #264/#266), and the collector has no path it could name
/// empty. The data half moved into the round; the revocation is what is left.
#[test]
fn the_old_system_slot_is_revoked_every_turn() {
    for form in ["readable", "json", "both"] {
        let msg = assemble(
            &[("memory_tier", "0"), ("memory_form", form)],
            bundle_body(AS_OF, QUERY, READABLE),
        );
        assert_eq!(
            msg["system"]["memory"],
            serde_json::json!({"recall": {"text": ""}, "$replace": true}),
            "the revocation and nothing else, under `memory_form` {form}: {msg}"
        );
        let sys = serde_json::to_string(&msg["system"]).expect("system json");
        assert!(
            !sys.contains("WHAT THIS MEMORY HOLDS") && !sys.contains("the editor is helix"),
            "no byte of the bundle may reach durable state: {sys}"
        );
    }
}

/// The counter-pin: the revocation reaches `system.memory` and no further. The
/// `system` node itself carries the EMPTY root and would revoke every other
/// writer's slot in the brain — instructions, handover, the affinity push lane.
#[test]
fn the_revocation_stays_on_the_node_the_collector_owns() {
    let rows = serde_json::json!([
        leg_window_row(serde_json::json!([{"role": "user", "text": QUERY,
                                           "consult_id": "c1"}])),
        leg_memory_row(bundle_body(AS_OF, QUERY, READABLE))
    ]);
    let out = emit_with(&[("memory_tier", "0")], reply_doc("fire", 2, rows));
    let sys = &out[0]["system"];
    assert!(sys.get("$replace").is_none(), "{}", out[0]);
    assert_eq!(
        sys["consult"]["open"],
        serde_json::json!(["c1"]),
        "{}",
        out[0]
    );
    assert!(sys["consult"].get("$replace").is_none(), "{}", out[0]);
}

// ══════════════════════════════════════════════ 4. THE PAIR IS IN THE BUDGET

/// The third consequence #278 names, stated precisely: the bundle was never
/// OUTSIDE the budget. `curate` receives `len(json.dumps(sysm))` as `sys_chars`
/// and adds it to every projection, so the bytes were always counted — but they
/// were counted as an anonymous lump in a subtree the curator may not touch.
///
/// What changes here is WHERE they are counted: the pair sits in `msgs`, so the
/// projection grows through the conversation rather than through `system`, and
/// every byte is attributable to the item that produced it. The two assertions
/// below are the discriminating ones — the projection grows with the bundle
/// AND the `system` half stays the same size, which is exactly the move from
/// `sys_chars` into the round.
///
/// It is still not a curation CANDIDATE, and that is deliberate: curation only
/// touches items tagged with an iteration, and this pair belongs to no round.
#[test]
fn the_pair_counts_towards_the_curator_budget() {
    let over = [("memory_tier", "0"), ("context_window", "200")];
    let small = assemble(&over, bundle_body(AS_OF, QUERY, READABLE));
    let large = assemble(
        &over,
        bundle_body(
            AS_OF,
            QUERY,
            &format!("{READABLE}\n{}", "  more said\n".repeat(80)),
        ),
    );

    let projected = |m: &serde_json::Value| {
        m["header"]["tokens_projected"]
            .as_str()
            .expect("tokens_projected")
            .parse::<i64>()
            .expect("a number")
    };
    assert!(
        projected(&large) > projected(&small),
        "a bigger bundle has to make a bigger projection: {} vs {}",
        projected(&small),
        projected(&large)
    );
    // The move, made visible: the growth is in the ROUND, not in `sys_chars`.
    // The system half is a literal now and cannot grow with the bundle at all.
    let sys_chars = |m: &serde_json::Value| serde_json::to_string(&m["system"]).unwrap().len();
    assert_eq!(
        sys_chars(&small),
        sys_chars(&large),
        "the bundle must no longer reach the projection through `system`: {large}"
    );
    assert_ne!(
        large["header"]["curate_mark"], "none",
        "and the budget still marks on it: {large}"
    );
    // And it is never a curation candidate: an ambient recall the curator
    // elided would be a bundle the model was told it had and cannot read.
    let (_, result) = pair(&large);
    assert!(
        !result["text"]
            .as_str()
            .unwrap_or_default()
            .starts_with("[elided"),
        "{large}"
    );
}

// ══════════════════════════════════════ 5. THE EXPLICIT PATH IS UNTOUCHED

/// Only the AMBIENT leg moves. A bundle that answers a `memory_recall` the
/// model itself emitted still comes back under the ORIGINAL `tool_call_id`
/// (GH #78) and gets no synthetic pair of its own — two results for one call is
/// a malformed round, and a second call nobody made is a lie about the turn.
#[test]
fn a_model_initiated_memory_recall_still_answers_under_its_own_call_id() {
    let out = emit_with(
        &[("memory_tier", "0")],
        lane_doc(
            "in_bundle",
            &[("memory_call_id", "call_abc123")],
            bundle_body(AS_OF, QUERY, READABLE),
        ),
    );
    assert_eq!(
        out.len(),
        1,
        "the answer to the call and nothing else -- a bundle with a call id is \
         a RESULT of the running round, never the ambient leg of the turn: {out:?}"
    );

    let op: serde_json::Value =
        serde_json::from_str(out[0]["messages"][0]["text"].as_str().expect("op text"))
            .expect("op json");
    assert_eq!(op["table"], "round");
    assert_eq!(op["row"]["role"], "tool");
    let turn: serde_json::Value =
        serde_json::from_str(op["row"]["turn"].as_str().expect("turn text")).expect("turn json");
    assert_eq!(
        turn["id"], "call_abc123",
        "the answer keeps the id the model asked under: {out:?}"
    );
    assert_eq!(turn["type"], "tool_result");
    assert!(
        !turn["id"]
            .as_str()
            .unwrap_or_default()
            .starts_with("call_recall_"),
        "the explicit path does not get a synthetic id: {out:?}"
    );
    assert_eq!(
        turn["text"], READABLE,
        "and it carries the bundle in the configured form, as before: {out:?}"
    );
}

// ═════════════════════════════ 6. THE TRACE DOES NOT RIDE INTO THE PROMPT

/// The guarantee the `in_bundle` lane has always made, pinned here because this
/// is the wave that put the lane's output into `messages[]`.
///
/// Since memory-hive 2.3.0 the full retrieval record travels in a `recall_diagnostic`
/// TOP-LEVEL body slot (#296) — not `system`, not a message. The lane keeps
/// `system` and `messages` and nothing else, so the record survives in the
/// message log while never reaching the next prompt. Now that the bundle is
/// re-rendered into the conversation, a lane that kept the slot would put the
/// whole trace in front of the model.
#[test]
fn the_recall_diagnostic_never_reaches_the_assembled_prompt() {
    let mut body = bundle_body(AS_OF, QUERY, READABLE);
    body["recall_diagnostic"] = serde_json::json!({
        "as_of": AS_OF, "query": QUERY, "tier": 0, "recall_id": "r1",
        "text": "MEMORY (tier 1, 1 candidates, RRF over kw,sem)\n- [fact kw,sem] \
                 the editor is helix",
        "legs_present": ["kw", "sem"], "leg_sizes": {"kw": 1, "sem": 1},
        "leg_sizes_raw": {"kw": 3, "sem": 4}, "leg_capped": {},
        "semantic_degraded": false,
        "candidates": [{"id": "row-9", "score": 0.031, "legs": ["kw", "sem"],
                        "rank": 1, "session": "s0", "kind": "fact",
                        "text": "the editor is helix"}]
    });

    // Through the real lane, so the leg row is the one the lane writes.
    let out = emit_with(&[("memory_tier", "0")], lane_doc("in_bundle", &[], body));
    let leg: serde_json::Value =
        serde_json::from_str(out[0]["messages"][0]["text"].as_str().expect("op text"))
            .expect("op json");
    assert_eq!(leg["row"]["role"], "leg-memory");

    let rows = serde_json::json!([
        leg_window_row(serde_json::json!([{"role": "user", "text": QUERY}])),
        {"turn_id": "t1", "iter": 0, "role": "leg-memory", "fired": 0,
         "turn": leg["row"]["turn"]}
    ]);
    let msg = emit_with(&[("memory_tier", "0")], reply_doc("fire", 2, rows))
        .into_iter()
        .next()
        .expect("the seam message");

    let sys = serde_json::to_string(&msg["system"]).expect("system json");
    assert!(
        !sys.contains("recall_diagnostic") && !sys.contains("RRF over"),
        "the trace is not durable state: {sys}"
    );
    for m in msg["messages"].as_array().expect("messages") {
        let text = serde_json::to_string(m).expect("turn json");
        assert!(
            !text.contains("recall_diagnostic") && !text.contains("RRF over"),
            "the trace is not prompt: {text}"
        );
    }
}
