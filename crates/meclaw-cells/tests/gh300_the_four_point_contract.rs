//! Wave 5 -- the four-point contract, and the writes that obey it
//! (GitHub #300, ruling Q9).
//!
//! Task 16 gave the pass the whole session to look at. This file locks what it
//! is allowed to DO with it, and the four points are obligations rather than
//! advice because each of them has a mechanical half:
//!
//! * **Add only what is missing.** An `add` leaves as an inline block on
//!   `close_write` -- through the ingress, never beside it. The ingress is what
//!   canonicalises, hashes, deduplicates, mints the entity rows and enqueues the
//!   embedding; a close pass writing `facts` rows itself would produce
//!   statements vector recall cannot see. Asserted by the ABSENCE of an insert
//!   into `facts` from this lane.
//! * **Correct only by superseding the record you name**, and **a sharpening
//!   points at a record**: both put the named id into `replaces`, and both carry
//!   the `shown` array that guard rail 3 checks the reference against. The
//!   report counts them apart, because the operator wants to know whether the
//!   per-turn path is producing WRONG records or merely ROUGH ones.
//! * **Do not restate what is already there.** A proposal that repeats an open
//!   record of the session is counted (`restated`) and never written -- the
//!   honest measurement of how well point 4 is obeyed.
//!
//! And the health signal: `close_report` with every count zero is a SUCCESS,
//! not an empty result. That is the shape a well-annotated session produces,
//! and the harness reads it as invariant 6.

use std::io::Write;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

const CLOSE_GLUE: &str = "../../templates/memory-hive/close-glue/config.json";
const EXTRACT_GLUE: &str = "../../templates/memory-hive/extract-glue/config.json";
const DREAM_GLUE: &str = "../../templates/memory-hive/dream-glue/config.json";
const SESSION: &str = "s-300";
const AUDIENCE: &str = r#"["member:user","agent:assistant"]"#;

/// A shipped `config.json` of this hive, parsed.
fn config(path: &str) -> Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{path} is not json: {e}"))
}

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string -- the same substitution the colony performs when it instantiates the
/// template.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

fn script_of(path: &str) -> String {
    let v = config(path);
    let src = v["params"]["script_inline"]
        .as_str()
        .unwrap_or_else(|| panic!("{path}: params.script_inline"))
        .to_string();
    resolve_vars(&src)
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv (GH #279).
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
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

/// Run one of the shipped scripts with a real stdin document and return the
/// emitted messages.
fn emit(path: &str, doc: Value) -> Vec<Value> {
    let out = run_script_on_stdin(
        &script_of(path),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "{path} exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The op arguments of one emitted message.
fn args_of(msg: &Value) -> Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

/// The base context of the close lane: the three keys `in_close_pass` declares.
fn close_ctx(phase: &str) -> Value {
    json!({"store_origin": "close", "mem_phase": phase, "session_id": SESSION,
           "channel": "c-300", "audience_set": AUDIENCE})
}

/// The ingress hop, as edge 1 stamps it.
fn ingress() -> Value {
    json!({
        "header": {"context": close_ctx("window"), "hop": {"route": "in_close_pass"}},
        "messages": [{"origin": "user", "type": "text", "id": "m1", "text": "close"}]
    })
}

/// One store reply, as edge 3 delivers it.
fn store_reply(phase: &str, operation: &str, rows: &Value) -> Value {
    json!({
        "header": {"context": close_ctx(phase),
                   "hop": {"operation": operation, "rows_affected": 1}},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

/// The closer's answer, as edge 5 delivers it: `mem_phase` is stamped
/// `verdict`, and the verdict is INJECTED -- no model is called anywhere in
/// this file.
fn verdict_reply(verdict: &Value) -> Value {
    json!({
        "header": {"context": close_ctx("verdict"), "hop": {"finish_reason": "stop"}},
        "messages": [{"origin": "assistant", "type": "text", "id": "v",
                      "text": verdict.to_string()}]
    })
}

/// What one whole close pass did.
struct Pass {
    /// Every store op the chain issued, in order.
    ops: Vec<Value>,
    /// The payload the prompt phase rendered for the closer.
    prompt: Value,
    /// The instructions the prompt phase sent with it.
    instructions: String,
    /// The last emission -- the writes, the sweep and the report.
    last: Vec<Value>,
}

impl Pass {
    /// The `close_report` message of the pass.
    fn report(&self) -> &Value {
        self.last
            .iter()
            .find(|m| m["header"]["route"] == "close_report")
            .unwrap_or_else(|| panic!("the pass emits a close_report: {:?}", self.last))
    }

    /// One count off the report hop.
    fn count(&self, key: &str) -> u64 {
        self.report()["header"][key]
            .as_u64()
            .unwrap_or_else(|| panic!("hop.{key} is a count: {}", self.report()))
    }

    /// The inline blocks that left on `close_write`, parsed.
    fn blocks(&self) -> Vec<Value> {
        self.last
            .iter()
            .filter(|m| m["header"]["route"] == "close_write")
            .map(args_of)
            .collect()
    }

    /// The ops of the last emission -- the writes this lane makes ITSELF.
    fn own_writes(&self) -> Vec<Value> {
        self.last
            .iter()
            .filter(|m| m["header"]["route"] == "cstore")
            .map(args_of)
            .collect()
    }
}

/// A store that is enough to walk the chain: `scratch` is real (the lane parks
/// in it and reads it back), every other table answers from `rows`.
///
/// Rows come back NEWEST FIRST, which is what `created_at desc` means and the
/// whole reason the consuming phase must take the FIRST row per kind.
struct Store {
    scratch: Vec<Value>,
}

impl Store {
    fn new() -> Self {
        Store {
            scratch: Vec::new(),
        }
    }

    fn answer(&mut self, args: &Value, rows: &dyn Fn(&str) -> Value) -> Value {
        let table = args["table"].as_str().unwrap_or("");
        match (args["operation"].as_str().unwrap_or(""), table) {
            ("insert", "scratch") => {
                self.scratch.push(args["row"].clone());
                json!([])
            }
            ("select", "scratch") => {
                let key = args["where"]["key"].clone();
                Value::Array(
                    self.scratch
                        .iter()
                        .rev()
                        .filter(|r| r["key"] == key)
                        .cloned()
                        .collect(),
                )
            }
            ("select", t) => rows(t),
            _ => json!([]),
        }
    }
}

/// Walk one close pass from the ingress hop to the report, answering every read
/// and injecting `verdict` where the closer would answer.
fn run_pass(store: &mut Store, rows: &dyn Fn(&str) -> Value, verdict: &Value) -> Pass {
    let mut ops = Vec::new();
    let mut prompt = Value::Null;
    let mut instructions = String::new();
    let mut msgs = emit(CLOSE_GLUE, ingress());
    for _ in 0..40 {
        for m in &msgs {
            if m["header"]["route"] == "cstore" {
                ops.push(args_of(m));
            }
        }
        if msgs.iter().any(|m| m["header"]["route"] == "close_report")
            || msgs.iter().any(|m| m["header"]["route"] == "reject")
        {
            return Pass {
                ops,
                prompt,
                instructions,
                last: msgs,
            };
        }
        if let Some(ask) = msgs.iter().find(|m| m["header"]["route"] == "close") {
            prompt = serde_json::from_str(
                ask["messages"][0]["text"]
                    .as_str()
                    .expect("the prompt payload is text"),
            )
            .expect("the prompt payload is json");
            instructions = ask["system"]["instructions"]["text"]
                .as_str()
                .expect("the prompt carries its instructions")
                .to_string();
            msgs = emit(CLOSE_GLUE, verdict_reply(verdict));
            continue;
        }
        let store_ops: Vec<&Value> = msgs
            .iter()
            .filter(|m| m["header"]["route"] == "cstore")
            .collect();
        assert_eq!(store_ops.len(), 1, "the read chain is sequential: {msgs:?}");
        let op = store_ops[0];
        let args = args_of(op);
        let phase = op["header"]["phase"].as_str().expect("phase").to_string();
        let operation = args["operation"].as_str().unwrap_or("").to_string();
        let answer = store.answer(&args, rows);
        msgs = emit(CLOSE_GLUE, store_reply(&phase, &operation, &answer));
    }
    panic!("the close pass did not terminate within 40 round trips: {ops:?}");
}

/// A session of two turns, no facts, no topics, no exceptions.
fn plain(table: &str) -> Value {
    match table {
        "episodes" => json!([
            {"id": "e-2", "session_id": SESSION, "sender": "assistant", "speaker": "assistant",
             "content": "blue it is", "happened_at": "2026-08-21T10:00:10Z",
             "recorded_at": "2026-08-21T10:00:11Z"},
            {"id": "e-1", "session_id": SESSION, "sender": "user", "speaker": "user",
             "content": "my favourite colour is blue", "happened_at": "2026-08-21T10:00:00Z",
             "recorded_at": "2026-08-21T10:00:01Z"},
        ]),
        _ => json!([]),
    }
}

/// One open fact of the session, in the columns task 16 reads.
fn fact(id: &str, episode: &str, subject: &str, predicate: &str, claim: &str) -> Value {
    json!({"id": id, "episode_id": episode, "subject": subject,
           "canonical_subject": subject, "predicate": predicate,
           "canonical_predicate": predicate, "claim": claim, "canonical_claim": claim,
           "fact_kind": "world", "confidence": 70,
           "valid_from": "2026-08-21T10:00:00Z", "recorded_at": "2026-08-21T10:00:05Z"})
}

/// The plain session plus two open facts.
fn with_facts(table: &str) -> Value {
    match table {
        "facts" => json!([
            fact("f-1", "e-1", "Alex", "favorite_color", "blue"),
            fact("f-2", "e-2", "Alex", "lives_in", "Berlin"),
        ]),
        other => plain(other),
    }
}

/// Every id the `shown` array of a block covers.
fn shown_ids(block: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for axis in block["shown"].as_array().into_iter().flatten() {
        for s in axis["statements"].as_array().into_iter().flatten() {
            out.push(s["id"].as_str().unwrap_or("").to_string());
        }
    }
    out
}

// ---------------------------------------------------------------- (a) the add

#[test]
fn an_add_leaves_through_the_ingress_and_never_beside_it() {
    let verdict = json!({"nothing_to_add": false,
        "add": [{"episode_id": "e-1", "subject": "Alex", "predicate": "favorite_color",
                 "claim": "blue", "fact_kind": "world", "confidence": 80}]});
    let pass = run_pass(&mut Store::new(), &plain, &verdict);

    let blocks = pass.blocks();
    assert_eq!(
        blocks.len(),
        1,
        "one block per covered episode: {:?}",
        pass.last
    );
    assert_eq!(
        blocks[0]["episode_id"], "e-1",
        "and it NAMES the turn it comes from -- `covered_episodes`' own form: {}",
        blocks[0]
    );
    let facts = blocks[0]["facts"]
        .as_array()
        .expect("the block carries facts");
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0]["claim"], "blue");
    assert_eq!(facts[0]["fact_kind"], "world");

    // The write path IS the ingress, asserted by the absence: a `facts` insert
    // from this lane would be a statement vector recall cannot see.
    for op in pass.ops.iter().chain(pass.own_writes().iter()) {
        assert!(
            !(op["operation"] == "insert" && op["table"] == "facts"),
            "the close pass wrote a fact row itself: {op}"
        );
    }
    assert_eq!(pass.count("added"), 1);
}

// ------------------------------------------- (b) the sharpening and the correction

#[test]
fn a_sharpening_and_a_correction_both_supersede_the_record_they_name() {
    let verdict = json!({
        "sharpen": [{"fact_id": "f-1", "subject": "Alex", "predicate": "favorite_color",
                     "claim": "cobalt blue", "why": "the turn said the shade"}],
        "correct": [{"fact_id": "f-2", "subject": "Alex", "predicate": "lives_in",
                     "claim": "Potsdam", "why": "a later turn corrected it"}]});
    let pass = run_pass(&mut Store::new(), &with_facts, &verdict);

    let blocks = pass.blocks();
    assert_eq!(blocks.len(), 2, "one per covered episode: {:?}", pass.last);
    for (id, claim) in [("f-1", "cobalt blue"), ("f-2", "Potsdam")] {
        let block = blocks
            .iter()
            .find(|b| {
                b["facts"]
                    .as_array()
                    .is_some_and(|fs| fs.iter().any(|f| f["claim"] == claim))
            })
            .unwrap_or_else(|| panic!("a block carrying {claim}: {blocks:?}"));
        let f = block["facts"]
            .as_array()
            .and_then(|fs| fs.iter().find(|f| f["claim"] == claim))
            .expect("checked above");
        assert_eq!(
            f["replaces"], id,
            "the reference travels WITH the fact that ends the record: {f}"
        );
        assert!(
            shown_ids(block).contains(&id.to_string()),
            "and the block carries the window guard rail 3 checks it against: {block}"
        );
    }
    assert_eq!(pass.count("sharpened"), 1);
    assert_eq!(pass.count("corrected"), 1);
}

// ------------------------------------------------------------- (c) the topic

#[test]
fn a_closed_topic_is_a_guarded_update_that_says_who_closed_it() {
    let rows = |table: &str| match table {
        "topics" => json!([{"id": "t-1", "name": "the sailing trip", "session_id": SESSION,
                            "opened_episode_id": "e-1", "opened_at": "2026-08-21T10:00:00Z"}]),
        other => plain(other),
    };
    let verdict = json!({"close_topics": [{"id": "t-1", "ended_episode_id": "e-2"}]});
    let pass = run_pass(&mut Store::new(), &rows, &verdict);

    let update = pass
        .own_writes()
        .into_iter()
        .find(|a| a["operation"] == "update" && a["table"] == "topics")
        .unwrap_or_else(|| panic!("the pass closes the topic: {:?}", pass.last));
    assert_eq!(update["where"]["id"], "t-1");
    assert_eq!(
        update["where"]["closed_at"], "",
        "guarded on the emptiness that means open -- a closed topic keeps its \
         first closure: {update}"
    );
    assert_eq!(update["set"]["closed_episode_id"], "e-2");
    assert!(
        update["set"]["closure_source"]
            .as_str()
            .is_some_and(|s| s.starts_with("close:")),
        "and it says WHO closed it, so the night can tell the two roles apart: {update}"
    );
    assert_eq!(pass.count("closed"), 1);
}

// ---------------------------------------------------- (d) nothing to add

#[test]
fn nothing_to_add_is_a_report_a_sweep_and_nothing_else() {
    // The acceptance bullet: a session whose turns were all annotated correctly
    // produces "nothing to add" -- and that is a SUCCESS with every count zero,
    // not an empty result.
    for verdict in [json!({"nothing_to_add": true}), json!({})] {
        let pass = run_pass(&mut Store::new(), &plain, &verdict);
        assert!(
            pass.blocks().is_empty(),
            "nothing was proposed, so nothing leaves on close_write: {:?}",
            pass.last
        );
        let writes = pass.own_writes();
        assert_eq!(
            writes.len(),
            1,
            "the sweep of the priority list, and nothing else: {writes:?}"
        );
        assert_eq!(writes[0]["operation"], "update");
        assert_eq!(writes[0]["table"], "pending_extraction");
        assert_eq!(writes[0]["set"]["status"], "close");
        assert_eq!(writes[0]["where"]["session_id"], SESSION);
        assert_eq!(writes[0]["where"]["status"], "pending");
        assert_eq!(
            pass.last.len(),
            2,
            "the sweep and the report, and nothing else: {:?}",
            pass.last
        );
        for key in [
            "added",
            "sharpened",
            "corrected",
            "closed",
            "exceptions",
            "restated",
            "unseen_refs",
            "truncated",
        ] {
            assert_eq!(
                pass.count(key),
                0,
                "hop.{key} on a well-annotated session: {}",
                pass.report()
            );
        }
        assert!(
            pass.report()["messages"][0]["text"]
                .as_str()
                .is_some_and(|t| !t.is_empty()),
            "and it says in one sentence what happened: {}",
            pass.report()
        );
    }
}

// ------------------------------------------------------- (e) an unseen reference

#[test]
fn a_reference_nobody_showed_is_dropped_rather_than_written() {
    let verdict = json!({"correct": [{"fact_id": "f-ghost", "subject": "Alex",
                                      "predicate": "lives_in", "claim": "Rome",
                                      "why": "invented"}]});
    let pass = run_pass(&mut Store::new(), &with_facts, &verdict);
    assert!(
        pass.blocks().is_empty(),
        "a reference outside the facts read is not a write: {:?}",
        pass.last
    );
    assert_eq!(pass.count("corrected"), 0);
    assert_eq!(
        pass.count("unseen_refs"),
        1,
        "and the guard rail firing is COUNTED: {}",
        pass.report()
    );
}

// ------------------------------------------------------------ (point 4) restated

#[test]
fn a_restatement_is_counted_instead_of_quietly_succeeding() {
    let verdict = json!({"add": [{"episode_id": "e-1", "subject": "Alex",
                                  "predicate": "favorite_color", "claim": "blue",
                                  "fact_kind": "world"}]});
    let pass = run_pass(&mut Store::new(), &with_facts, &verdict);
    assert!(
        pass.blocks().is_empty(),
        "point 4: what already stands is not added again: {:?}",
        pass.last
    );
    assert_eq!(pass.count("added"), 0);
    assert_eq!(
        pass.count("restated"),
        1,
        "the honest measurement of how well point 4 is obeyed: {}",
        pass.report()
    );
}

// ------------------------------------------------------------------ the prompt

#[test]
fn the_prompt_states_the_four_points_and_shows_what_may_be_referenced() {
    let rows = |table: &str| match table {
        "pending_extraction" => json!([{"id": "q-1", "episode_id": "e-2",
                                        "session_id": SESSION, "status": "pending",
                                        "enqueued_at": "2026-08-21T10:00:11Z"}]),
        "topics" => json!([{"id": "t-1", "name": "colours", "session_id": SESSION,
                            "opened_episode_id": "e-1"}]),
        other => with_facts(other),
    };
    let pass = run_pass(&mut Store::new(), &rows, &json!({"nothing_to_add": true}));

    // The turns, oldest first for reading -- the page was READ newest first so
    // that a long session loses its oldest turns, and the two orders are
    // different jobs.
    let turns = pass.prompt["turns"]
        .as_array()
        .expect("the turns block: {pass.prompt}");
    assert_eq!(turns.len(), 2, "{}", pass.prompt);
    // The turn nobody annotated comes FIRST and says so: the exception list is a
    // priority, not a fence.
    assert_eq!(turns[0]["episode_id"], "e-2");
    assert_eq!(turns[0]["annotated"], false);
    assert_eq!(turns[1]["episode_id"], "e-1");
    assert_eq!(turns[1]["annotated"], true);

    // The facts, each with the record id the contract's points 2 and 3 need.
    let ids: Vec<String> = pass.prompt["facts"]
        .as_array()
        .expect("the facts block")
        .iter()
        .flat_map(|a| {
            a["statements"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|s| s["id"].as_str().unwrap_or("").to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(ids.contains(&"f-1".to_string()), "{}", pass.prompt);
    assert!(ids.contains(&"f-2".to_string()), "{}", pass.prompt);

    // The topics, with their ids.
    assert_eq!(pass.prompt["topics"][0]["id"], "t-1", "{}", pass.prompt);

    // And the four points, in the ruling's words.
    for phrase in [
        "Add only what is missing",
        "Correct only by superseding the record you name",
        "sharpening points at a record",
        "nothing to add",
    ] {
        assert!(
            pass.instructions.contains(phrase),
            "the instruction states `{phrase}`: {}",
            pass.instructions
        );
    }
    assert_eq!(
        pass.count("exceptions"),
        1,
        "and the un-annotated turn is reported: {}",
        pass.report()
    );
}

// ------------------------------------------------- the scratch-read trap (T16 #3)

#[test]
fn a_second_close_of_one_session_renders_its_own_sets() {
    // The key of this lane is the SESSION, and `insert` is not `upsert`: a
    // second close parks four more rows under the same key. The meeting read is
    // ordered `created_at desc`, so the consuming phase must take the FIRST row
    // per kind. Taking the last -- the loop the ingress chain can afford,
    // because its key is a fresh batch id every time -- renders the FIRST run's
    // session on the second run.
    let mut store = Store::new();
    run_pass(&mut store, &plain, &json!({"nothing_to_add": true}));
    let second = |table: &str| match table {
        "episodes" => json!([{"id": "e-9", "session_id": SESSION, "sender": "user",
                              "speaker": "user", "content": "a later turn",
                              "happened_at": "2026-08-21T11:00:00Z",
                              "recorded_at": "2026-08-21T11:00:01Z"}]),
        _ => json!([]),
    };
    let pass = run_pass(&mut store, &second, &json!({"nothing_to_add": true}));
    let turns = pass.prompt["turns"].as_array().expect("the turns block");
    assert_eq!(
        turns.len(),
        1,
        "the second close reads its OWN parking, not the first run's: {}",
        pass.prompt
    );
    assert_eq!(turns[0]["episode_id"], "e-9", "{}", pass.prompt);
}

// ------------------------------------------------------- (f) the ingress side

/// One `inline` hop into `extract-glue`, with or without the close lane's
/// stamp.
fn inline_hop(close_pass: bool, block: &Value) -> Value {
    let mut ctx = json!({"mem_phase": "inline", "session_id": SESSION, "channel": "c-300",
                         "audience_set": AUDIENCE});
    if close_pass {
        ctx["close_pass"] = json!("1");
    }
    json!({
        "header": {"context": ctx, "hop": {}},
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "b",
                      "text": block.to_string()}]
    })
}

#[test]
fn the_close_lane_supplies_the_window_the_apply_phase_guards_against() {
    let shown = json!([{"subject": "Alex", "predicate": "favorite_color",
                        "statements": [{"id": "f-1", "claim": "blue",
                                        "since": "2026-08-21T10:00:00Z",
                                        "last_asserted": "2026-08-21T10:00:00Z"}]}]);
    let block = json!({"episode_id": "e-1",
                       "facts": [{"episode_id": "e-1", "subject": "Alex",
                                  "predicate": "favorite_color", "claim": "cobalt blue",
                                  "fact_kind": "world", "confidence": 80,
                                  "replaces": "f-1"}],
                       "shown": shown});

    // WITH the stamp: the window is parked under the block's own batch id,
    // before the payload it guards, and the turn is covered as `close`.
    let msgs = emit(EXTRACT_GLUE, inline_hop(true, &block));
    let ops: Vec<Value> = msgs
        .iter()
        .filter(|m| m["header"]["route"] == "xstore")
        .map(args_of)
        .collect();
    let window = ops
        .iter()
        .position(|a| {
            a["operation"] == "insert" && a["table"] == "scratch" && a["row"]["kind"] == "window"
        })
        .unwrap_or_else(|| panic!("the block's window is parked: {ops:?}"));
    let payload = ops
        .iter()
        .position(|a| {
            a["operation"] == "insert" && a["table"] == "scratch" && a["row"]["kind"] == "payload"
        })
        .unwrap_or_else(|| panic!("the payload is staged: {ops:?}"));
    assert!(
        window < payload,
        "the window is parked BEFORE the payload it guards: {ops:?}"
    );
    assert_eq!(
        ops[window]["row"]["key"], ops[payload]["row"]["key"],
        "under the SAME batch id -- the apply phase reads both back by that one \
         key: {ops:?}"
    );
    let parked: Value = serde_json::from_str(
        ops[window]["row"]["payload"]
            .as_str()
            .expect("the window payload is a string"),
    )
    .expect("the window payload is json");
    assert_eq!(parked, shown, "byte for byte what the close pass rendered");
    let cover = ops
        .iter()
        .find(|a| a["table"] == "pending_extraction")
        .unwrap_or_else(|| panic!("the turn is covered: {ops:?}"));
    assert_eq!(
        cover["set"]["status"], "close",
        "and the books say WHICH reader handled the turn: {cover}"
    );
    // WITHOUT it: no window row, and the ordinary inline status. The front model
    // does not get to declare what it was shown -- edge truth, never a body
    // claim.
    let msgs = emit(EXTRACT_GLUE, inline_hop(false, &block));
    let ops: Vec<Value> = msgs
        .iter()
        .filter(|m| m["header"]["route"] == "xstore")
        .map(args_of)
        .collect();
    assert!(
        !ops.iter().any(|a| a["row"]["kind"] == "window"),
        "a block without the stamp parks no window: {ops:?}"
    );
    assert!(
        ops.iter()
            .any(|a| a["operation"] == "insert" && a["row"]["kind"] == "payload"),
        "and is otherwise parsed exactly as today: {ops:?}"
    );
    let cover = ops
        .iter()
        .find(|a| a["table"] == "pending_extraction")
        .unwrap_or_else(|| panic!("the turn is covered: {ops:?}"));
    assert_eq!(cover["set"]["status"], "inline", "{cover}");
}

// --------------------------------------------------------------- the night

#[test]
fn the_night_reviews_both_closure_authors() {
    // Without the tuple the night reviews the ingress's closures and silently
    // ignores the close pass's -- a second policy by omission. Q9 rules the two
    // ROLES apart, not the policies.
    let script = script_of(DREAM_GLUE);
    assert!(
        script.contains("CLOSURE_SOURCE_PREFIXES = (\"extract:\", \"close:\")"),
        "the night knows both authors of a closure"
    );
    assert!(
        !script.contains("EXTRACT_SOURCE_PREFIX"),
        "and the single-author name is gone rather than shadowed"
    );
    assert_eq!(
        script
            .matches("startswith(CLOSURE_SOURCE_PREFIXES)")
            .count(),
        2,
        "both call sites -- the axis review AND the revert guard"
    );
}

// -------------------------------------------------------------- the contract

#[test]
fn the_report_declares_every_count_it_carries() {
    // A hop value that is not declared is a failed emit, and a count nobody
    // declared is a health signal nobody can read.
    let hop = config(CLOSE_GLUE)["contract"]["emits"]["hop"].clone();
    for key in [
        "added",
        "sharpened",
        "corrected",
        "closed",
        "exceptions",
        "restated",
        "unseen_refs",
        "truncated",
    ] {
        assert_eq!(
            hop[key]["type"], "number",
            "hop.{key} is declared as a count: {hop}"
        );
    }
    let reasons: Vec<&str> = hop["reject_reason"]["values"]
        .as_array()
        .expect("the refusal list")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        reasons.contains(&"closer_failed"),
        "and every refusal this lane can make is in the list: {reasons:?}"
    );
}
