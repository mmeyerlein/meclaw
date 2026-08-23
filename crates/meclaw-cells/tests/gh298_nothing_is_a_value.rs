//! Wave 5 -- "nothing new" is an OUTCOME, not a rejection (GitHub #298).
//!
//! Per-turn extraction asks the front model about every single turn, and the
//! honest answer to most turns is "nothing worth remembering". Until this
//! package that answer left the hive through the `reject` port, on the same lane
//! as a payload that was not JSON -- and a queue row that stayed `pending`
//! either way. Two consequences, both bad:
//!
//! * the batched extractor bought a second opinion on a question the front model
//!   had already answered, which is exactly the duplicate GitHub #52 exists to
//!   stop;
//! * an explicit "nothing" and a broken annotation were indistinguishable from
//!   the outside, so nobody could tell a turn nobody annotated from a turn
//!   annotated as empty.
//!
//! So the queue gets a third status. `pending` = no annotation ever arrived (the
//! exception worth looking at), `inline` = annotated and carried content,
//! `nothing` = annotated as explicitly nothing. The distinction the lane must
//! keep is the one this file measures: a WELL-FORMED empty verdict covers its
//! turn and emits no reject, while a MALFORMED block -- not JSON, not an object,
//! or without provenance -- still rejects and still covers nothing.
//!
//! Everything here runs the REAL `params.script_inline` of `extract-glue`
//! against injected replies, so nothing costs anything.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/extract-glue/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty string --
/// the same substitution the colony performs when it instantiates the template.
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

fn glue_script() -> String {
    let raw = std::fs::read_to_string(GLUE_CONFIG).expect("extract-glue config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv (GH #279: a single argv string is
/// capped at 128 KiB and the shipped scripts have grown to within a few KB of
/// that line).
fn run_script_on_stdin(script: &str, stdin_doc: &[u8]) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(&String::from_utf8_lossy(stdin_doc).to_string()).unwrap(),
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

/// Run the real script with a real stdin document and return the emitted messages.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let out = run_script_on_stdin(&glue_script(), &meclaw_testing::code_stdin_bytes(&doc));
    assert!(
        out.status.success(),
        "extract-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The conversation these blocks are written in.
const SESSION: &str = "s-gh298n";

/// The room the blocks are written in.
const CHANNEL: &str = "c-gh298n";

/// Who was present. Not `["*"]` -- a universal set would let every case here
/// pass against a write path with no gate at all (#244).
const AUDIENCE: &str = r#"["member:user","agent:assistant"]"#;

/// One annotation block as the port edge delivers it, with the provenance the
/// gate requires next to it.
fn annotation(payload: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "inline", "mem_phase": "inline",
                        "session_id": SESSION,
                        "audience_set": AUDIENCE, "channel": CHANNEL},
            "hop": {}
        },
        "messages": [{"origin": "user", "type": "text", "text": payload}]
    })
}

/// The same block, but delivered without the cast of the turn -- the one thing
/// this ingress refuses before it even reads the payload (#244).
fn annotation_without_audience(payload: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "inline", "mem_phase": "inline",
                        "session_id": SESSION, "channel": CHANNEL},
            "hop": {}
        },
        "messages": [{"origin": "user", "type": "text", "text": payload}]
    })
}

/// The `remember`/annotation call that names no turn: a `tool_call` whose text
/// is the raw arguments string. The hive binds it to the newest `user` episode
/// of the session.
fn tool_call(arguments: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "inline", "mem_phase": "inline",
                        "session_id": SESSION,
                        "audience_set": AUDIENCE, "channel": CHANNEL},
            "hop": {"route": "tool", "tool_name": "remember", "async": "1"}
        },
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-1",
                      "text": arguments}]
    })
}

/// A store answer on the inline chain: the phase it answers for, the batch key
/// it carries and the provenance that rides along the whole round trip.
fn echo(phase: &str, batch_id: &str, operation: &str, rows: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "extract", "mem_phase": phase,
                        "batch_id": batch_id, "session_id": SESSION,
                        "audience_set": AUDIENCE, "channel": CHANNEL},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows}]
    })
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

fn store_ops(msgs: &[serde_json::Value]) -> Vec<serde_json::Value> {
    msgs.iter()
        .filter(|m| m["header"]["route"] == "xstore")
        .map(args_of)
        .collect()
}

fn rejects(msgs: &[serde_json::Value]) -> Vec<&serde_json::Value> {
    msgs.iter()
        .filter(|m| m["header"]["route"] == "reject")
        .collect()
}

fn batch_id_of(msgs: &[serde_json::Value]) -> String {
    msgs.iter()
        .find(|m| m["header"]["route"] == "xstore")
        .and_then(|m| m["header"]["batch_id"].as_str())
        .expect("every store op of this lane names its batch")
        .to_string()
}

/// The block the lane parked while it resolves the turn, as the store would
/// hand it back.
fn parked_payload(msgs: &[serde_json::Value]) -> String {
    store_ops(msgs)
        .into_iter()
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == "inline")
        .map(|a| {
            a["row"]["payload"]
                .as_str()
                .expect("parked string")
                .to_string()
        })
        .expect("the block is parked while its turn is resolved")
}

/// The verdict itself: the model read the turn, found nothing worth remembering
/// and says where the conversation stands anyway. `continue` on purpose -- it
/// writes no topic op, so what remains is exactly the coverage op and the
/// "exactly one" below measures the verdict rather than the thread.
fn nothing_block(episode: Option<&str>) -> String {
    let mut v = serde_json::json!({
        "nothing_new": true,
        "facts": [],
        "topic": {"movement": "continue"}
    });
    if let Some(e) = episode {
        v["episode_id"] = serde_json::json!(e);
    }
    v.to_string()
}

/// The payload the lane staged for the dedup phase -- the content lane's own op.
fn staged(msgs: &[serde_json::Value]) -> Option<serde_json::Value> {
    store_ops(msgs)
        .into_iter()
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == "payload")
}

/// The queue op, whatever status it carries.
fn queue_op(msgs: &[serde_json::Value]) -> Option<serde_json::Value> {
    store_ops(msgs)
        .into_iter()
        .find(|a| a["table"] == "pending_extraction")
}

/// A block whose whole content is ONE EDGE: no fact, no entity, an empty `facts`
/// list next to it. It is not an empty answer -- an edge is the graph leg of
/// tier 1 and the apply phase writes it without looking at a single fact.
fn edge_only_block(episode: Option<&str>) -> String {
    let mut v = serde_json::json!({
        "facts": [],
        "edges": [{"src_entity": "Alex", "dst_entity": "Aiden",
                   "edge_kind": "works_on", "weight": 1}]
    });
    if let Some(e) = episode {
        v["episode_id"] = serde_json::json!(e);
    }
    v.to_string()
}

/// The coverage op, and the assertion that it is the WHOLE emission.
fn assert_sole_nothing_coverage(msgs: &[serde_json::Value], episode: &str) {
    let ops = store_ops(msgs);
    assert_eq!(
        ops.len(),
        1,
        "an explicit nothing writes the coverage op and nothing else: {msgs:?}"
    );
    let op = &ops[0];
    assert_eq!(op["table"], "pending_extraction");
    assert_eq!(op["operation"], "update");
    assert_eq!(
        op["set"]["status"], "nothing",
        "the queue row says the turn was annotated as empty, not that nobody looked"
    );
    assert_eq!(
        op["where"]["status"], "pending",
        "a row a batch already claimed is left to the batch that owns it"
    );
    let covered: Vec<String> = op["where"]["episode_id"]["in"]
        .as_array()
        .expect("the covered turns")
        .iter()
        .map(|v| v.as_str().expect("id").to_string())
        .collect();
    assert_eq!(covered, vec![episode.to_string()]);
}

#[test]
fn an_explicit_nothing_covers_its_turn_with_its_own_status() {
    // (a) The whole package in one emission: one op, on the queue, saying
    // `nothing`. Not `inline` -- a turn that produced a fact and a turn that
    // produced a considered silence are different books -- and not `done`, which
    // is what a batch writes after paying a model.
    let msgs = emit(annotation(&nothing_block(Some("e1"))));
    assert_sole_nothing_coverage(&msgs, "e1");
}

#[test]
fn an_explicit_nothing_does_not_leave_through_the_reject_port() {
    // (b) The indistinguishability #298 closes. A verdict that travels on the
    // refusal lane is indistinguishable from a refusal: the same route, the same
    // absence of a write, and nothing downstream can tell "the model read this
    // turn and found nothing" from "this block was garbage".
    let msgs = emit(annotation(&nothing_block(Some("e2"))));
    assert!(
        rejects(&msgs).is_empty(),
        "an answer is not a refusal: {msgs:?}"
    );
}

#[test]
fn a_bound_nothing_covers_the_turn_it_was_bound_to() {
    // (c) The tool form. It cannot name its turn -- an `episodes.id` is a uuid
    // the hive mints and no front model has ever seen it -- so the block is
    // parked, the turn is resolved, and the verdict is written where the id
    // exists. Same status, one round trip later, and still no reject: a
    // `remember` call that honestly reports nothing is not a broken call.
    let first = emit(tool_call(&nothing_block(None)));
    let bid = batch_id_of(&first);
    assert!(
        rejects(&first).is_empty(),
        "an empty verdict is parked, not refused: {first:?}"
    );

    let second = emit(echo("inline-turn", &bid, "insert", "[]"));
    let bid2 = batch_id_of(&second);

    let rows = serde_json::json!([
        {"id": "ep-88", "turn_id": format!("{SESSION}#0"), "session_id": SESSION,
         "sender": "user", "recorded_at": "2026-08-23T09:00:00.000000Z"}
    ])
    .to_string();
    let third = emit(echo("inline-bind", &bid2, "select", &rows));
    let bid3 = batch_id_of(&third);

    let parked = serde_json::json!([{"key": bid, "kind": "inline",
                                     "payload": parked_payload(&first)}])
    .to_string();
    let fourth = emit(echo("inline-apply", &bid3, "select", &parked));

    assert!(
        rejects(&fourth).is_empty(),
        "and no refusal at the far end either: {fourth:?}"
    );
    assert_sole_nothing_coverage(&fourth, "ep-88");
}

#[test]
fn a_block_carrying_only_an_edge_is_not_nothing() {
    // The trap the new status opens if "empty" is read as "no facts". An edge is
    // CONTENT: it is the graph leg of tier 1 and the apply phase inserts it
    // without ever looking at a fact. A block that carried one and was booked as
    // `nothing` would do two wrong things at once -- lose the edge without a
    // word, and write into the queue that a model had read this turn and found
    // nothing in it. The second is worse than the first: the fact lane can be
    // re-run, a queue row that lies about having been judged cannot.
    let msgs = emit(annotation(&edge_only_block(Some("e9"))));
    assert!(
        rejects(&msgs).is_empty(),
        "an edge is not a broken block: {msgs:?}"
    );
    let staged =
        staged(&msgs).unwrap_or_else(|| panic!("the edge travels the content lane: {msgs:?}"));
    let payload: serde_json::Value =
        serde_json::from_str(staged["row"]["payload"].as_str().expect("staged payload"))
            .expect("staged json");
    assert_eq!(
        payload["edges"].as_array().map(|e| e.len()),
        Some(1),
        "and it is the edge that was written: {payload}"
    );
    assert_eq!(
        queue_op(&msgs).expect("the turn is still covered")["set"]["status"],
        "inline",
        "a turn that produced an edge was not annotated as empty"
    );
}

#[test]
fn a_bound_block_carrying_only_an_edge_is_not_nothing_either() {
    // Same rule on the form that names no turn, where the mistake would be
    // hidden one round trip deeper: the verdict is decided in the `inline` phase
    // and rides in the parked block, so an edge misread as "nothing" there would
    // be discarded by `inline-apply` with nothing left to see.
    let first = emit(tool_call(&edge_only_block(None)));
    let bid = batch_id_of(&first);
    assert!(
        rejects(&first).is_empty(),
        "an edge-only remember call is parked, not refused: {first:?}"
    );

    let second = emit(echo("inline-turn", &bid, "insert", "[]"));
    let bid2 = batch_id_of(&second);
    let rows = serde_json::json!([
        {"id": "ep-99", "turn_id": format!("{SESSION}#0"), "session_id": SESSION,
         "sender": "user", "recorded_at": "2026-08-23T09:00:00.000000Z"}
    ])
    .to_string();
    let third = emit(echo("inline-bind", &bid2, "select", &rows));
    let bid3 = batch_id_of(&third);
    let parked = serde_json::json!([{"key": bid, "kind": "inline",
                                     "payload": parked_payload(&first)}])
    .to_string();
    let fourth = emit(echo("inline-apply", &bid3, "select", &parked));

    let staged = staged(&fourth).expect("the bound edge is staged");
    let payload: serde_json::Value =
        serde_json::from_str(staged["row"]["payload"].as_str().expect("staged payload"))
            .expect("staged json");
    assert_eq!(
        payload["edges"][0]["episode_id"], "ep-99",
        "and it hangs on the turn the block was bound to: {payload}"
    );
    assert_eq!(
        queue_op(&fourth).expect("the bound turn is covered")["set"]["status"],
        "inline",
        "a turn that produced an edge was not annotated as empty"
    );
}

#[test]
fn a_block_that_is_not_json_still_rejects_as_invalid() {
    // (d) The other side of the distinction, and the reason the two must not
    // share a lane. Garbage names no turn, proves nothing about one, and must
    // leave the turn in the queue for the batched extractor.
    let msgs = emit(annotation("not json at all"));
    let refusals = rejects(&msgs);
    assert_eq!(refusals.len(), 1, "garbage is refused: {msgs:?}");
    assert_eq!(refusals[0]["header"]["reject_reason"], "inline_invalid");
    assert!(
        store_ops(&msgs).is_empty(),
        "and writes nothing at all: {msgs:?}"
    );
}

#[test]
fn an_explicit_nothing_without_a_cast_still_rejects_fail_closed() {
    // (e) The provenance gate runs BEFORE the block is read (#244), and the new
    // status does not open a hole in it: a verdict without the cast of the turn
    // would mark a queue row on the word of a message that cannot say who was
    // present, and who was present is the gate on every later read.
    let msgs = emit(annotation_without_audience(&nothing_block(Some("e3"))));
    let refusals = rejects(&msgs);
    assert_eq!(refusals.len(), 1, "no cast, no write: {msgs:?}");
    assert_eq!(refusals[0]["header"]["reject_reason"], "missing_audience");
    assert!(
        store_ops(&msgs).is_empty(),
        "and the turn stays pending for the batch: {msgs:?}"
    );
}
