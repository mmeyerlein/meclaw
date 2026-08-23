//! Wave 5 -- the inline ingress records the MOVEMENT of the conversation
//! (GitHub #298).
//!
//! Per-turn extraction gives the front model one annotation block per turn, and
//! the block says two things at once: what was worth remembering (`facts`) and
//! where the conversation stands (`topic`). The second half is new. It is not a
//! fact -- "we are talking about the sailing trip" is not a claim about the
//! world, it has no subject and no predicate and it would poison the axes if it
//! were minted as one -- so it gets its own table (`topics`, wave 5 task 1) and
//! its own op.
//!
//! Three movements, at most one op each:
//!
//! * `start` opens a row on the turn the block speaks for, with `closed_at`
//!   empty -- that emptiness IS what "open" means in this table;
//! * `end` closes the row this session still holds open under that name. The
//!   model names the topic it ends, so no round trip is needed to find it. A
//!   name that matches nothing updates no row and leaves the topic open, which
//!   is the safe direction: a topic wrongly still open costs a later close, a
//!   topic wrongly closed loses the thread of the conversation;
//! * `continue` -- and an absent, malformed or unknown `topic` -- writes
//!   nothing.
//!
//! And the property that outranks all three: the topic op travels **next to**
//! the fact lane, never instead of it. Coverage (GitHub #52) already had to
//! learn that rule; a block that opens a topic still stages its facts, and a
//! block whose topic is garbage still stages them too.
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
/// that line, so `python3 -c <whole script>` breaks on size rather than on
/// behaviour). From stdin the script executes exactly as `python3 -c` ran it:
/// same `__main__` globals, same stdout, same exit status.
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

/// The conversation these blocks are written in. A topic lives in a session --
/// it is the thread of ONE conversation -- so the session is in every context
/// here, exactly as the seam edge promotes it in a real colony.
const SESSION: &str = "s-gh298";

/// The room the blocks are written in.
const CHANNEL: &str = "c-gh298";

/// Who was present: the person whose turn is annotated and the agent that
/// annotated it. Not `["*"]` -- a universal set would let every case here pass
/// against a write path with no gate at all (#244).
const AUDIENCE: &str = r#"["member:user","agent:assistant"]"#;

/// One annotation block as the port edge delivers it: the two keys the
/// `inline-extraction` port prescribes, the session the turn belongs to, and the
/// provenance the gate requires next to them.
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

/// The one op this block wrote about the movement of the conversation, or
/// `None` if it wrote none.
fn topic_op(msgs: &[serde_json::Value]) -> Option<serde_json::Value> {
    let ops: Vec<serde_json::Value> = store_ops(msgs)
        .into_iter()
        .filter(|a| a["table"] == "topics")
        .collect();
    assert!(
        ops.len() <= 1,
        "at most ONE topic op per annotation, got {ops:?}"
    );
    ops.into_iter().next()
}

/// The payload the lane staged for the dedup phase -- the fact lane's own op.
fn staged(msgs: &[serde_json::Value]) -> Option<serde_json::Value> {
    store_ops(msgs)
        .into_iter()
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == "payload")
}

/// An annotation block in the wave-5 shape: one fact about the turn `episode`,
/// plus whatever the model said about the thread.
fn block(episode: &str, topic: serde_json::Value) -> String {
    serde_json::json!({
        "episode_id": episode,
        "nothing_new": false,
        "facts": [{"episode_id": episode, "subject": "user", "predicate": "favorite_color",
                   "claim": "Blau", "fact_kind": "world", "confidence": 90}],
        "topic": topic
    })
    .to_string()
}

/// The name of the thread, verbatim as a model would write it -- spaces, capital
/// and umlaut included. Nothing normalises it: a topic name is a label the model
/// wrote, not a retrieval axis (wave 5 task 1 declared it without `fts` and
/// without `canonical` for exactly that reason).
const NAME: &str = "Segeltörn in Kroatien";

#[test]
fn a_started_topic_is_opened_on_the_turn_the_block_covers() {
    // (a) The row the conversation gets a thread from. `opened_episode_id` is
    // the turn the block speaks for -- the same turn coverage takes out of the
    // queue -- because a topic that cannot be read back to a turn cannot be
    // read back at all.
    let msgs = emit(annotation(&block(
        "e1",
        serde_json::json!({"movement": "start", "name": NAME}),
    )));
    let op = topic_op(&msgs).expect("a started topic opens a row");
    assert_eq!(op["operation"], "insert");
    assert_eq!(op["row"]["opened_episode_id"], "e1");
    assert_eq!(op["row"]["name"], NAME, "the name travels byte for byte");
    assert_eq!(op["row"]["session_id"], SESSION);
    assert_eq!(
        op["row"]["closed_at"], "",
        "an empty closed_at IS what open means in this table"
    );
    // #244: a topic carries a room and a cast, or the porter refuses the part.
    // The cast is the NORMALISED set (deduplicated, sorted) the whole hive
    // speaks -- an ordering difference between a write and the gate that reads
    // it is a difference nobody can see.
    assert_eq!(op["row"]["channel"], CHANNEL);
    assert_eq!(
        op["row"]["audience_set"], r#"["agent:assistant", "member:user"]"#,
        "the cast of the turn, in the set form the affinity gate normalises to"
    );
}

#[test]
fn an_ended_topic_closes_the_row_this_session_still_holds_open() {
    // (b) The close needs no round trip: the model names the topic it ends, so
    // the `where` is the name plus the session plus the openness. A name that
    // matches nothing affects no row and the topic stays open -- the safe
    // direction, and the one the nightly close pass can still repair.
    let msgs = emit(annotation(&block(
        "e2",
        serde_json::json!({"movement": "end", "name": NAME}),
    )));
    let op = topic_op(&msgs).expect("an ended topic closes its row");
    assert_eq!(op["operation"], "update");
    assert_eq!(op["where"]["name"], NAME, "the model names what it ends");
    assert_eq!(op["where"]["session_id"], SESSION);
    assert_eq!(
        op["where"]["closed_at"], "",
        "only a topic that is still open can be closed"
    );
    assert_eq!(op["set"]["closed_episode_id"], "e2");
    assert_eq!(
        op["set"]["closure_source"], "turn:e2",
        "the closure names the turn that ended the thread"
    );
    assert_ne!(
        op["set"]["closed_at"], "",
        "and it stamps the instant, or the row stays open by its own definition"
    );
}

#[test]
fn a_continued_topic_writes_nothing_about_topics() {
    // (c) The common case, and the one that must stay free: most turns continue
    // the thread they are in. A row per turn would make the table a second
    // episode log.
    let msgs = emit(annotation(&block(
        "e3",
        serde_json::json!({"movement": "continue", "name": NAME}),
    )));
    assert!(
        topic_op(&msgs).is_none(),
        "a continued topic is a topic nothing happened to: {msgs:?}"
    );
}

#[test]
fn a_topic_the_lane_cannot_read_writes_nothing_about_topics() {
    // Same silence for everything that is not one of the three movements: a
    // missing `topic`, a `topic` that is not an object, an unknown movement and
    // a movement without a name. The block is model-written and the store is
    // not the place to find out what a model meant.
    for topic in [
        serde_json::Value::Null,
        serde_json::json!("Segeltörn"),
        serde_json::json!({"movement": "pause", "name": NAME}),
        serde_json::json!({"movement": "start", "name": ""}),
        serde_json::json!({"name": NAME}),
    ] {
        let msgs = emit(annotation(&block("e4", topic.clone())));
        assert!(
            topic_op(&msgs).is_none(),
            "no op for topic {topic}: {msgs:?}"
        );
    }
}

#[test]
fn the_fact_lane_is_untouched_whatever_the_movement_is() {
    // (d) The rule the topic op must never break. The annotation is ONE block
    // with two halves and the halves are not alternatives: the facts are the
    // memory, the topic is bookkeeping about the conversation, and a lane that
    // dropped the first to write the second would have traded the whole point
    // of per-turn extraction for a label.
    for movement in ["start", "continue", "end"] {
        let msgs = emit(annotation(&block(
            "e5",
            serde_json::json!({"movement": movement, "name": NAME}),
        )));
        assert!(
            staged(&msgs).is_some(),
            "the validated payload is still staged on `{movement}`: {msgs:?}"
        );
        assert!(
            store_ops(&msgs)
                .iter()
                .any(|a| a["table"] == "pending_extraction"),
            "and the turn is still taken out of the queue on `{movement}`: {msgs:?}"
        );
    }
}

// ------------------------------------------------------- the tool form (W10b)

/// The `remember`/annotation call that names no turn: a `tool_call` whose text
/// is the raw arguments string. The hive binds it to the newest `user` episode
/// of the session, so its ops -- coverage, facts AND topic -- are emitted one
/// round trip later, in `inline-apply`, where the episode id exists.
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
/// it carries and the provenance that rides along the whole round trip (the
/// store echoes the context it was called with; only `mem_phase` and `batch_id`
/// are re-stamped by the edge).
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

fn batch_id_of(msgs: &[serde_json::Value]) -> String {
    msgs.iter()
        .find(|m| m["header"]["route"] == "xstore")
        .and_then(|m| m["header"]["batch_id"].as_str())
        .expect("every store op of this lane names its batch")
        .to_string()
}

#[test]
fn a_bound_block_opens_its_topic_on_the_turn_it_was_bound_to() {
    // The form that names no turn. Its topic cannot be written in the `inline`
    // phase for the same reason its facts cannot: there is no episode id yet.
    // It is parked with the payload and emitted where the id exists -- and it
    // opens on exactly the turn the facts hang on, or the thread and the memory
    // would disagree about which turn they came from.
    let arguments = serde_json::json!({
        "nothing_new": false,
        "facts": [{"subject": "user", "predicate": "favorite_color", "claim": "Blau",
                   "fact_kind": "world", "confidence": 90}],
        "topic": {"movement": "start", "name": NAME}
    })
    .to_string();

    let first = emit(tool_call(&arguments));
    let bid = batch_id_of(&first);
    assert!(
        topic_op(&first).is_none(),
        "nothing is written before the turn is known: {first:?}"
    );

    let second = emit(echo("inline-turn", &bid, "insert", "[]"));
    let bid2 = batch_id_of(&second);

    let rows = serde_json::json!([
        {"id": "ep-77", "turn_id": format!("{SESSION}#0"), "session_id": SESSION,
         "sender": "user", "recorded_at": "2026-08-23T09:00:00.000000Z"}
    ])
    .to_string();
    let third = emit(echo("inline-bind", &bid2, "select", &rows));
    let bid3 = batch_id_of(&third);

    let parked = serde_json::json!([{"key": bid, "kind": "inline",
                                     "payload": parked_payload(&first)}])
    .to_string();
    let fourth = emit(echo("inline-apply", &bid3, "select", &parked));

    let op = topic_op(&fourth).expect("the bound block opens its topic");
    assert_eq!(op["operation"], "insert");
    assert_eq!(
        op["row"]["opened_episode_id"], "ep-77",
        "the thread opens on the turn the facts were bound to"
    );
    assert_eq!(op["row"]["name"], NAME);
    assert_eq!(op["row"]["closed_at"], "");
    assert!(
        staged(&fourth).is_some(),
        "and the fact lane is untouched here too: {fourth:?}"
    );
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
