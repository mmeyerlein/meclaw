//! Wave 5 -- the close pass reads the whole session, records included
//! (GitHub #300, ruling Q9).
//!
//! Task 15 built the lane; this file locks what travels on it: four store
//! reads, one round trip each, parked in `scratch` until they can meet. Every
//! one of the four is a way the pass could look finished and be blind:
//!
//! * **The turns, all of them.** Q9's change is that the pass is not shown a
//!   summary of the session, it is shown the session. Ordered and bounded --
//!   a prefix of an unordered set is not a page, and an unbounded answer puts a
//!   whole table on a mailbox (GH #68) whatever the consumer does with it.
//! * **The facts WITH their record ids.** Points 2, 3 and 4 of the contract are
//!   unobeyable without them: a model that cannot name a record can only
//!   rephrase it. `id` and `episode_id` are therefore asserted by name.
//! * **The open topics with their ids**, for the same reason one step over.
//! * **The exceptions, emitted whether or not anything preceded them.** The
//!   turns whose annotation never arrived are the pass's PRIORITY list, not its
//!   work list, and an empty one is the common case -- so the shape that used
//!   to be a skip is the shape asserted here: zero rows everywhere, and the
//!   chain still reaches the `pending_extraction` read.
//!
//! The last assertion sweeps the whole chain for `limit`, because the way this
//! lane fails quietly is one read out of four forgetting it.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/close-glue/config.json";
const SESSION: &str = "s-300";

/// A shipped `config.json` of this hive, parsed.
fn config(path: &str) -> serde_json::Value {
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

fn glue_script() -> String {
    let v = config(GLUE_CONFIG);
    let src = v["params"]["script_inline"]
        .as_str()
        .expect("close-glue script_inline")
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

/// Run the real script with a real stdin document and return the emitted
/// messages.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let out = run_script_on_stdin(
        &glue_script(),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "close-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The ingress hop: the phase edge 1 stamps, the three keys the lane demands.
fn ingress() -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"mem_phase": "window", "store_origin": "close",
                        "session_id": SESSION, "channel": "c-300",
                        "audience_set": r#"["member:user","agent:assistant"]"#},
            "hop": {"route": "in_close_pass"}
        },
        "messages": [{"origin": "user", "type": "text", "id": "m1", "text": "close"}]
    })
}

/// One store reply, as edge 3 delivers it: the phase the glue asked for comes
/// back in `context.mem_phase`, `hop.operation` says which op answered.
fn store_reply(phase: &str, operation: &str, rows: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "close", "mem_phase": phase,
                        "session_id": SESSION, "channel": "c-300",
                        "audience_set": r#"["member:user","agent:assistant"]"#},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

/// The op arguments of one emitted message.
fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

/// The single store op of one emission -- the chain is sequential by
/// construction (a fan-out would park twice and render twice), so more than one
/// is a finding, not a shape to tolerate.
fn one_op(msgs: &[serde_json::Value]) -> Option<(String, serde_json::Value)> {
    let ops: Vec<&serde_json::Value> = msgs
        .iter()
        .filter(|m| m["header"]["route"] == "cstore")
        .collect();
    assert!(
        ops.len() <= 1,
        "the read chain is sequential: {} store ops in one emission: {msgs:?}",
        ops.len()
    );
    let op = ops.first()?;
    let phase = op["header"]["phase"]
        .as_str()
        .expect("every store op names the phase of its reply")
        .to_string();
    Some((phase, args_of(op)))
}

/// Drive the chain from the ingress hop, answering every read with `rows` (by
/// table) and every write with an empty row set, and collect the ops in order.
///
/// This is the whole point of the file: the phases are not inspected one at a
/// time against a guessed reply, they are walked the way the store walks them.
fn drive(rows: &dyn Fn(&str) -> serde_json::Value) -> Vec<serde_json::Value> {
    let mut collected = Vec::new();
    let mut msgs = emit(ingress());
    for _ in 0..16 {
        let Some((phase, args)) = one_op(&msgs) else {
            return collected;
        };
        let operation = args["operation"].as_str().unwrap_or("").to_string();
        let table = args["table"].as_str().unwrap_or("").to_string();
        collected.push(args);
        let answer = if operation == "select" {
            rows(&table)
        } else {
            serde_json::json!([])
        };
        msgs = emit(store_reply(&phase, &operation, &answer));
    }
    panic!("the read chain did not terminate within 16 round trips: {collected:?}");
}

/// Every read answers empty -- the common case, and the shape that used to be a
/// skip.
fn nothing(_table: &str) -> serde_json::Value {
    serde_json::json!([])
}

/// The one op of the chain that reads `table`.
fn read_of(ops: &[serde_json::Value], table: &str) -> serde_json::Value {
    let found: Vec<&serde_json::Value> = ops
        .iter()
        .filter(|a| a["operation"] == "select" && a["table"] == table)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one select on {table} in the chain, found {found:?}"
    );
    found[0].clone()
}

#[test]
fn the_window_read_is_one_ordered_bounded_select_on_the_session() {
    // (a) -- the first emission is the session read and nothing else.
    let msgs = emit(ingress());
    let (phase, args) = one_op(&msgs).expect("the window phase emits a store op");
    assert_eq!(msgs.len(), 1, "and only that one message: {msgs:?}");
    assert!(!phase.is_empty());
    assert_eq!(args["operation"], "select");
    assert_eq!(args["table"], "episodes");
    assert_eq!(
        args["where"]["session_id"], SESSION,
        "only of the session it was handed: {args}"
    );
    assert!(
        args["order_by"].is_array() && !args["order_by"].as_array().unwrap().is_empty(),
        "ordered -- a prefix of an unordered set is not a page: {args}"
    );
    assert!(
        args["limit"].as_u64().is_some_and(|n| n > 0),
        "and bounded: {args}"
    );
    // Ruling: the window stays NEWEST-first. A session longer than the page
    // loses its OLDEST turns rather than its last ones; the prompt phase
    // re-sorts for rendering.
    assert_eq!(
        args["order_by"][0]["dir"], "desc",
        "newest first, so the page that is dropped is the one least likely to \
         hold the correction the pass exists for: {args}"
    );
}

#[test]
fn the_facts_of_the_session_are_read_with_their_record_ids() {
    // (b) -- the reply to the window leads to the facts read, and the ids are
    // the whole point of it.
    let ops = drive(&nothing);
    let read = read_of(&ops, "facts");
    assert_eq!(read["where"]["session_id"], SESSION);
    assert_eq!(
        read["where"]["expired_at"],
        serde_json::json!({"is_null": true}),
        "the OPEN facts -- an expired row is not what the session left standing: {read}"
    );
    let columns: Vec<&str> = read["columns"]
        .as_array()
        .expect("the facts read names its columns")
        .iter()
        .filter_map(|c| c.as_str())
        .collect();
    for needed in [
        "id",
        "episode_id",
        "canonical_subject",
        "canonical_predicate",
        "canonical_claim",
        "recorded_at",
    ] {
        assert!(
            columns.contains(&needed),
            "the facts read carries `{needed}`: {columns:?}"
        );
    }
    assert!(
        read["order_by"].is_array() && !read["order_by"].as_array().unwrap().is_empty(),
        "ordered: {read}"
    );
}

#[test]
fn the_open_topics_are_read_by_the_emptiness_that_defines_them() {
    // (c) -- `closed_at == ''` IS what open means in this table.
    let ops = drive(&nothing);
    let read = read_of(&ops, "topics");
    assert_eq!(read["where"]["session_id"], SESSION);
    assert_eq!(read["where"]["closed_at"], "");
    let columns: Vec<&str> = read["columns"]
        .as_array()
        .expect("the topics read names its columns")
        .iter()
        .filter_map(|c| c.as_str())
        .collect();
    assert!(
        columns.contains(&"id"),
        "with their ids -- a topic the pass cannot name it cannot close: {columns:?}"
    );
}

#[test]
fn the_exception_list_is_read_on_a_session_that_has_none() {
    // (d) -- the shape that used to be a skip. Every read before this one
    // answered empty and the chain still gets here: the exceptions are a
    // priority list, not a gate.
    let ops = drive(&nothing);
    let read = read_of(&ops, "pending_extraction");
    assert_eq!(read["where"]["session_id"], SESSION);
    assert_eq!(read["where"]["status"], "pending");
    let episodes = ops
        .iter()
        .position(|a| a["table"] == "episodes")
        .expect("the window read is in the chain");
    let exceptions = ops
        .iter()
        .position(|a| a["table"] == "pending_extraction")
        .expect("checked above");
    assert!(
        episodes < exceptions,
        "and it comes after the session it belongs to: {ops:?}"
    );
}

#[test]
fn every_select_in_the_chain_is_bounded() {
    // (e) -- the way this lane fails quietly: one read out of four forgetting
    // its `limit`. GH #68 -- what the store ANSWERS travels the mailbox.
    let ops = drive(&nothing);
    let selects: Vec<&serde_json::Value> =
        ops.iter().filter(|a| a["operation"] == "select").collect();
    assert!(
        selects.len() >= 5,
        "the chain reads the four points and meets them again: {ops:?}"
    );
    for s in selects {
        assert!(
            s["limit"].as_u64().is_some_and(|n| n > 0),
            "an unbounded select in the read chain: {s}"
        );
        assert!(
            s["order_by"].is_array() && !s["order_by"].as_array().unwrap().is_empty(),
            "an unordered select in the read chain: {s}"
        );
    }
}

#[test]
fn each_result_set_is_parked_before_the_next_read_leaves() {
    // The park-and-meet of `extract-glue`'s surviving chain: a `code` cell is
    // stateless and holds ONE store result at a time, so the four sets have to
    // meet inside `scratch` before anything can be rendered from them.
    let ops = drive(&nothing);
    let parked: Vec<&serde_json::Value> = ops
        .iter()
        .filter(|a| a["operation"] == "insert" && a["table"] == "scratch")
        .collect();
    assert_eq!(parked.len(), 4, "one park per read, under one key: {ops:?}");
    let key = format!("close:{SESSION}");
    let mut kinds: Vec<&str> = Vec::new();
    for p in &parked {
        assert_eq!(
            p["row"]["key"], key,
            "the session IS the identity of the pass -- no run id beside it: {p}"
        );
        kinds.push(
            p["row"]["kind"]
                .as_str()
                .expect("every park names its kind"),
        );
    }
    kinds.sort_unstable();
    assert_eq!(kinds, ["exceptions", "facts", "topics", "turns"]);
    // And the meet: the last op reads the four back under that one key.
    let meet = ops.last().expect("a non-empty chain");
    assert_eq!(meet["operation"], "select");
    assert_eq!(meet["table"], "scratch");
    assert_eq!(meet["where"]["key"], key);
}

#[test]
fn a_full_page_is_recognised_as_one_rather_than_guessed() {
    // Truncation has to be EXACT, not probable: a page that comes back exactly
    // full is indistinguishable from a session that happens to end there. The
    // chain asks for `limit + 1`, so the extra row is the proof -- it is
    // dropped before parking and its presence is what `hop.truncated` (task 17)
    // reports.
    let ops = drive(&nothing);
    let window = read_of(&ops, "episodes");
    let bound = config(GLUE_CONFIG)["contract"]["settings"]["close_turn_rows"]["default"]
        .as_u64()
        .expect("the declared page bound");
    assert_eq!(
        window["limit"].as_u64(),
        Some(bound + 1),
        "the window asks for one row more than it keeps: {window}"
    );

    // Driven with an over-full page, the park keeps the bound and says so.
    let rows: Vec<serde_json::Value> = (0..=bound)
        .map(|i| serde_json::json!({"id": format!("e-{i}"), "session_id": SESSION}))
        .collect();
    let full = serde_json::Value::Array(rows);
    let msgs = emit(store_reply("turns", "select", &full));
    let (_, park) = one_op(&msgs).expect("the turns phase parks its page");
    assert_eq!(park["operation"], "insert");
    assert_eq!(park["table"], "scratch");
    let payload: serde_json::Value = serde_json::from_str(
        park["row"]["payload"]
            .as_str()
            .expect("payload is a string"),
    )
    .expect("payload is json");
    assert_eq!(
        payload["truncated"], true,
        "an over-full page is reported as truncated: {payload}"
    );
    assert_eq!(
        payload["rows"].as_array().map(Vec::len),
        Some(bound as usize),
        "and the proof row is dropped rather than parked: {payload}"
    );
}

#[test]
fn the_facts_bound_has_one_declared_default() {
    // The knob task 16 adds. Since GH #138 it is a PARAM of this cell rather
    // than a `${MEMORY_CLOSE_FACT_ROWS}` substitution, so the pair the old
    // gh204 convention compared (a declaration and its env twin) is a triple:
    // `params`, `contract.settings` and the script's own fallback literal.
    // `gh204_declared_defaults_match_the_inline` compares all three; this one
    // only insists the knob exists and is reachable, because a bound nobody
    // declared is a bound nobody can change -- and an `override_params` entry
    // may only name a key that is under `params` at all (GH #294).
    let cfg = config(GLUE_CONFIG);
    let settings = cfg["contract"]["settings"].clone();
    let fact_rows = &settings["close_fact_rows"];
    assert_eq!(
        fact_rows["default"], 256,
        "the facts page bound is declared: {settings}"
    );
    assert_eq!(
        cfg["params"]["close_fact_rows"], 256,
        "and it is a param, so an instance can be given a different one"
    );
    assert!(
        cfg["params"]["script_inline"]
            .as_str()
            .is_some_and(|s| s.contains("_int(\"close_fact_rows\", 256)")),
        "and the script spends the same number as its own fallback"
    );
}
