//! A store refusal is not a result (GitHub #343).
//!
//! Since GH #331 the `store` cell stamps `hop.operation` on its error paths as
//! well: `emit_invalid_input`, `emit_query_timeout` and `emit_write_denied` all
//! name the refused op (or the literal `error` when nothing parseable arrived).
//! That closed a contract hole — the shipped store contract declares
//! `emits.hop.operation` as `required: true`, and a return edge conditioned on
//! `hop.operation` no longer loses exactly the replies that report a failure.
//!
//! It also changed what the lanes downstream of a store see. A `code` lane that
//! dispatches on `(context.<phase>, hop.operation)` used to fall through on a
//! store error reply — such a reply carried no `operation` at all, so no branch
//! matched and the lane parked. Since #331 the same reply matches the SUCCESS
//! branch, and the lane reads the error sentence as a result set.
//!
//! The four lanes in this file are the ones with double-digit dispatch sites,
//! and all four were measured advancing on a refusal before the fix:
//!
//! | lane | phase | what a refusal did |
//! |---|---|---|
//! | `collector/assemble` | `win` | wrote an EMPTY window leg into the round table; the turn then fires and the model answers with no conversation at all |
//! | `memory-hive/recall` | `t1-kw-ep` | recorded the failed keyword leg as a leg with zero hits; the bundle then reads "memory knows nothing" (this is #308, one hive over) |
//! | `memory-hive/extract-glue` | `vocab` | went on to prompt the extractor with an EMPTY known-predicate vocabulary (the phase is retired with the batch lane, #298 — the pair below drives `known` instead, see `extract_known_context`) |
//! | `memory-hive/dream-glue` | `scope` | booked the run `status: done, facts_in_window: 0`; the next night derives `delta_from` from that row, so the whole window is skipped forever |
//!
//! The fix is the shape `builder-librarian/retrieve` was hardened into for
//! #308: `operation` says WHICH op answered, `error_code` says WHETHER it
//! worked, and both are read. The success branches gain `and not err`, and each
//! lane gained a terminal branch that says the store refused instead of parsing
//! the refusal as rows.
//!
//! Every test here drives the SHIPPED `script_inline` of the real template.

use std::io::Write;
use std::process::{Command, Stdio};

const ASSEMBLE: &str = "../../templates/collector/assemble/config.json";
const RECALL: &str = "../../templates/memory-hive/recall/config.json";
const EXTRACT_GLUE: &str = "../../templates/memory-hive/extract-glue/config.json";
const DREAM_GLUE: &str = "../../templates/memory-hive/dream-glue/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string — the substitution the colony performs at instantiation.
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

fn script_of(config: &str) -> String {
    let raw = std::fs::read_to_string(config).unwrap_or_else(|e| panic!("{config}: {e}"));
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv (GH #279: a single argv string is
/// capped at 128 KiB and these scripts sit within a few KB of that line).
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

fn emit(script: &str, doc: serde_json::Value) -> Vec<serde_json::Value> {
    let out = run_script_on_stdin(script, &meclaw_testing::code_stdin(&doc).to_string());
    assert!(
        out.status.success(),
        "the script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// One store reply as the return edge delivers it: the chain's phase in the
/// context, the op in the hop — and, when `error_code` is `Some`, the refusal
/// stamp #331 added beside it.
fn store_reply(
    context: serde_json::Value,
    operation: &str,
    error_code: Option<&str>,
    text: &str,
) -> serde_json::Value {
    let mut hop = serde_json::json!({"operation": operation, "rows_affected": 0});
    if let Some(code) = error_code {
        hop["error_code"] = serde_json::json!(code);
    }
    serde_json::json!({
        "header": {"context": context, "hop": hop},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "", "text": text}]
    })
}

fn routes(msgs: &[serde_json::Value]) -> Vec<String> {
    msgs.iter()
        .filter_map(|m| m["header"]["route"].as_str().map(str::to_string))
        .collect()
}

fn phases(msgs: &[serde_json::Value]) -> Vec<String> {
    msgs.iter()
        .filter_map(|m| m["header"]["phase"].as_str().map(str::to_string))
        .collect()
}

/// Everything the emission says, in one string — the assertion "the reply names
/// the store's error_code" has to hold over the whole message, not over one
/// field we happened to pick.
fn spoken(msgs: &[serde_json::Value]) -> String {
    serde_json::to_string(msgs).expect("serialise")
}

/// The one thing every terminal branch must NOT do: hand the store another op.
fn asserts_no_store_op(msgs: &[serde_json::Value], store_route: &str, lane: &str) {
    assert!(
        !routes(msgs).iter().any(|r| r == store_route),
        "{lane}: the chain advanced on a refusal — it sent the store another \
         op instead of stopping: {}",
        spoken(msgs)
    );
}

// ------------------------------------------------------- collector/assemble

fn assemble_window_context() -> serde_json::Value {
    serde_json::json!({
        "col_phase": "win", "session_id": "s1", "turn_id": "t1", "iter": "0"
    })
}

#[test]
fn the_collector_does_not_read_a_refused_window_read_as_an_empty_window() {
    let script = script_of(ASSEMBLE);
    let out = emit(
        &script,
        store_reply(
            assemble_window_context(),
            "select",
            Some("query_timeout"),
            "query exceeded query_timeout_ms",
        ),
    );
    asserts_no_store_op(&out, "cstore", "collector/assemble");
    assert!(
        !phases(&out).iter().any(|p| p == "collect"),
        "collector/assemble: the phase advanced to `collect` on a refusal: {}",
        spoken(&out)
    );
    assert!(
        !out.is_empty(),
        "collector/assemble: a refusal must be SAID, not swallowed"
    );
    assert!(
        spoken(&out).contains("query_timeout"),
        "collector/assemble: the reply does not name the store's error_code: {}",
        spoken(&out)
    );
}

#[test]
fn the_collector_still_assembles_a_window_that_came_back() {
    // The control. A fix that answers every message equally is not a fix.
    let script = script_of(ASSEMBLE);
    let rows = serde_json::json!([{
        "id": "a", "session_id": "s1", "turn_id": "t1", "role": "user",
        "content": "hi", "recorded_at": "2026-01-01T00:00:00.000000Z",
        "deferred": 0, "consult_id": ""
    }])
    .to_string();
    let out = emit(
        &script,
        store_reply(assemble_window_context(), "select", None, &rows),
    );
    assert!(
        phases(&out).iter().any(|p| p == "collect"),
        "collector/assemble: a clean window read must still open the fan-in: {}",
        spoken(&out)
    );
}

// ------------------------------------------------------- memory-hive/recall

fn recall_keyword_context() -> serde_json::Value {
    serde_json::json!({
        "mem_phase": "t1-kw-ep", "recall_id": "r1", "memory_tier": "1",
        "recall_query": "radiator valve",
        "audience_now": r#"["member:user","agent:assistant"]"#,
        "channel": "c-343"
    })
}

#[test]
fn recall_does_not_read_a_refused_keyword_leg_as_a_leg_with_no_hits() {
    let script = script_of(RECALL);
    let out = emit(
        &script,
        store_reply(
            recall_keyword_context(),
            "search",
            Some("invalid_input"),
            "no such column: digest",
        ),
    );
    asserts_no_store_op(&out, "rstore", "memory-hive/recall");
    assert_eq!(
        routes(&out),
        vec!["reject".to_string()],
        "memory-hive/recall: a refused leg has to leave on the reject lane the \
         hive already declares: {}",
        spoken(&out)
    );
    assert!(
        spoken(&out).contains("invalid_input"),
        "memory-hive/recall: the reply does not name the store's error_code: {}",
        spoken(&out)
    );
}

#[test]
fn recall_still_records_a_keyword_leg_that_came_back() {
    let script = script_of(RECALL);
    let out = emit(
        &script,
        store_reply(recall_keyword_context(), "search", None, "[]"),
    );
    assert!(
        routes(&out).iter().any(|r| r == "rstore"),
        "memory-hive/recall: a clean (empty) leg must still be recorded: {}",
        spoken(&out)
    );
}

// ------------------------------------------------- memory-hive/extract-glue

/// The dedup leg's known-facts read, which is where this lane's guarded chain
/// begins now.
///
/// It used to be the VOCABULARY read: the measured damage was a refused
/// vocabulary read that walked on and prompted the extractor with an empty list
/// of known predicates. Per-turn extraction (#298) retired that read together
/// with the batch prompt, and the pair below moved to the first guarded read
/// that survives it -- the known-facts set the dedup diff is taken against. The
/// damage is the same one dimension over: read as an empty answer, a refused
/// `known` read makes every fact of the block look new, and the duplicates it
/// writes are indelible.
fn extract_known_context() -> serde_json::Value {
    serde_json::json!({
        "mem_phase": "known", "batch_id": "b1", "store_origin": "extract",
        "session_id": "s1"
    })
}

#[test]
fn extraction_does_not_read_a_refused_dedup_read_as_an_empty_known_set() {
    let script = script_of(EXTRACT_GLUE);
    let out = emit(
        &script,
        store_reply(
            extract_known_context(),
            "select",
            Some("query_timeout"),
            "query exceeded query_timeout_ms",
        ),
    );
    asserts_no_store_op(&out, "xstore", "memory-hive/extract-glue");
    assert!(
        !routes(&out).iter().any(|r| r == "extract"),
        "memory-hive/extract-glue: it went on with an empty known-facts set: {}",
        spoken(&out)
    );
    assert_eq!(
        routes(&out),
        vec!["reject".to_string()],
        "memory-hive/extract-glue: a refusal has to leave on the reject lane: {}",
        spoken(&out)
    );
    assert!(
        spoken(&out).contains("query_timeout"),
        "memory-hive/extract-glue: the reply does not name the error_code: {}",
        spoken(&out)
    );
}

#[test]
fn extraction_still_walks_on_from_a_dedup_read_that_came_back() {
    let script = script_of(EXTRACT_GLUE);
    let out = emit(
        &script,
        store_reply(extract_known_context(), "select", None, "[]"),
    );
    assert!(
        routes(&out).iter().any(|r| r == "xstore"),
        "memory-hive/extract-glue: a clean (empty) known-facts read must still \
         walk on: {}",
        spoken(&out)
    );
}

// --------------------------------------------------- memory-hive/dream-glue

fn dream_scope_context() -> serde_json::Value {
    serde_json::json!({
        "mem_phase": "scope", "dream_run": "run1",
        "dream_to": "2026-01-02T00:00:00.000000Z", "store_origin": "dream"
    })
}

#[test]
fn the_nightly_run_does_not_book_a_window_it_never_read() {
    let script = script_of(DREAM_GLUE);
    let out = emit(
        &script,
        store_reply(
            dream_scope_context(),
            "select",
            Some("query_timeout"),
            "query exceeded query_timeout_ms",
        ),
    );
    asserts_no_store_op(&out, "dstore", "memory-hive/dream-glue");
    // The specific damage: `close_run` writes status "done" plus the delta
    // boundary, and every later night derives `delta_from` from the newest done
    // row. A window booked as consolidated is never looked at again.
    assert!(
        !spoken(&out).contains("\\\"status\\\": \\\"done\\\""),
        "memory-hive/dream-glue: it booked the run as done on a refused read — \
         that window is now skipped forever: {}",
        spoken(&out)
    );
    assert_eq!(
        routes(&out),
        vec!["reject".to_string()],
        "memory-hive/dream-glue: a refusal has to leave on the reject lane: {}",
        spoken(&out)
    );
    assert!(
        spoken(&out).contains("query_timeout"),
        "memory-hive/dream-glue: the reply does not name the error_code: {}",
        spoken(&out)
    );
}

#[test]
fn a_refusal_that_lost_the_run_identity_is_still_reported() {
    // Review finding, fix round 1: the terminal branch first sat BELOW
    // `if not run_id or not delta_to: park()`. That guard is right for a stray
    // echo and wrong for a refusal — a store reply whose context lost
    // `dream_run` is still a store reply saying a read did not happen, and
    // parking it swallows exactly the news worth having. Reproduced as `[]`.
    let script = script_of(DREAM_GLUE);
    let out = emit(
        &script,
        store_reply(
            serde_json::json!({"mem_phase": "scope", "store_origin": "dream"}),
            "select",
            Some("query_timeout"),
            "query exceeded query_timeout_ms",
        ),
    );
    assert_eq!(
        routes(&out),
        vec!["reject".to_string()],
        "memory-hive/dream-glue: a refusal without run keys was swallowed: {}",
        spoken(&out)
    );
    assert!(
        spoken(&out).contains("query_timeout"),
        "memory-hive/dream-glue: the reply does not name the error_code: {}",
        spoken(&out)
    );
}

#[test]
fn a_stray_echo_without_an_operation_still_parks() {
    // The guard the move had to leave standing: a hop that carries neither an
    // op nor an error_code is not a store reply at all, and it must still park
    // rather than report a refusal that did not happen.
    let script = script_of(DREAM_GLUE);
    let out = emit(
        &script,
        serde_json::json!({
            "header": {"context": {"mem_phase": "window", "dream_run": "", "dream_to": ""},
                       "hop": {}},
            "messages": []
        }),
    );
    assert!(
        out.is_empty(),
        "memory-hive/dream-glue: a stray echo must still park, got {}",
        spoken(&out)
    );
}

#[test]
fn the_nightly_run_still_closes_a_window_it_did_read() {
    let script = script_of(DREAM_GLUE);
    let out = emit(
        &script,
        store_reply(dream_scope_context(), "select", None, "[]"),
    );
    assert!(
        routes(&out).iter().any(|r| r == "dstore"),
        "memory-hive/dream-glue: an empty but ANSWERED window must still close \
         the run: {}",
        spoken(&out)
    );
}
