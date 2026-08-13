//! meclaw-os -- the archive bridge template `archive-bridge@1` (night wave 3,
//! track U, GH #4).
//!
//! Stores are driven by tool_call args, not by headers -- an `llm`'s final text
//! turn cannot reach a `store` as it is. This template is ONE `code` cell that
//! translates: the final assistant text turn becomes a store-native
//! `{operation: insert, table, row}` tool_call, and the store's reply echo is
//! swallowed on purpose so nothing dead-letters. The pattern existed inline in
//! the example colonies (`examples/telegram-research/main/archive`); this
//! template lifts it into a clean, reusable cell. Three claims are pinned:
//!
//! 1. TRANSLATION -- the LAST non-empty assistant text turn becomes
//!    `{operation: insert, table: ${ARCHIVE_TABLE:-archive}, row: {id, text,
//!    recorded_at}}` on route 'store', with a fresh `archive-` tool_call id.
//! 2. THE ECHO DIES HERE, QUIETLY AND ON PURPOSE -- the store answers every
//!    insert with a tool_result under the same id; that echo has nowhere to go
//!    and would dead-letter (or loop) in every wiring that does not drain it.
//!    The bridge IS the drain: any input without an assistant text turn is an
//!    empty multi-send, and the colony half proves the DLQ stays empty.
//! 3. THE ROW IS REAL -- the colony half reads the store's cell.db and finds
//!    the answer text, an id and a recorded_at timestamp in the table.
//!
//! The script half runs the shipped `params.script_inline` against real stdin
//! documents; the colony half boots the shipped template file next to a real
//! `store` cell. Nothing is mocked and no provider is called.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

#[path = "support_14b.rs"]
mod support;

use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path};
use support::boot;

const BRIDGE_CONFIG: &str = "../../builder/templates/archive-bridge/config.json";

// ======================================================================= SCRIPT

/// `${VAR:-default}` becomes the default (or the override, when the case names
/// one) -- the same substitution the colony performs at boot.
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

fn bridge_script(over: &[(&str, &str)]) -> String {
    let raw = std::fs::read_to_string(BRIDGE_CONFIG).expect("archive-bridge config");
    let v: Value = meclaw_core::serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
        over,
    )
}

fn emit_with(over: &[(&str, &str)], doc: Value) -> Vec<Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(bridge_script(over))
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
        "bridge script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    meclaw_core::serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn emit(doc: Value) -> Vec<Value> {
    emit_with(&[], doc)
}

fn doc(messages: Vec<Value>) -> Value {
    json!({"header": {"context": {}, "hop": {}}, "messages": messages})
}

fn assistant_text(text: &str) -> Value {
    json!({"origin": "assistant", "type": "text", "text": text})
}

/// Parses the store args out of the single emitted tool_call turn.
fn insert_args(out: &[Value]) -> Value {
    assert_eq!(out.len(), 1, "exactly one store emission: {out:?}");
    let turn = &out[0]["messages"][0];
    assert_eq!(turn["type"], "tool_call", "{turn}");
    assert_eq!(turn["origin"], "assistant", "{turn}");
    meclaw_core::serde_json::from_str(turn["text"].as_str().expect("args string"))
        .expect("tool_call text is the JSON args object")
}

#[test]
fn a_final_answer_becomes_a_store_native_insert() {
    let out = emit(doc(vec![assistant_text("CEL is an expression language.")]));

    assert_eq!(out[0]["header"]["route"], "store", "{out:?}");
    let args = insert_args(&out);
    assert_eq!(args["operation"], "insert");
    assert_eq!(args["table"], "archive", "the default table");
    assert_eq!(args["row"]["text"], "CEL is an expression language.");
    assert!(
        !args["row"]["id"].as_str().unwrap_or("").is_empty(),
        "the row carries its own id: {args}"
    );
    let recorded = args["row"]["recorded_at"].as_str().unwrap_or("");
    assert!(
        recorded.contains('T') && recorded.len() >= 19,
        "recorded_at is an ISO-8601 timestamp: {recorded:?}"
    );

    // The correlation contract: turn id and hop.tool_call_id are the same
    // 'archive-' id, so the store's reply echo is recognizable downstream.
    let turn_id = out[0]["messages"][0]["id"].as_str().unwrap_or("");
    assert!(turn_id.starts_with("archive-"), "{turn_id:?}");
    assert_eq!(out[0]["header"]["tool_call_id"], turn_id);
}

#[test]
fn the_last_assistant_text_turn_wins() {
    // A conversation can carry earlier assistant turns (tool rounds, partial
    // sentences). The FINAL answer is the last non-empty assistant text turn.
    let out = emit(doc(vec![
        assistant_text("thinking out loud"),
        json!({"origin": "tool", "type": "tool_result", "id": "c1", "text": "42"}),
        assistant_text("The answer is 42."),
    ]));
    let args = insert_args(&out);
    assert_eq!(args["row"]["text"], "The answer is 42.");
}

#[test]
fn the_table_is_a_knob() {
    let out = emit_with(
        &[("ARCHIVE_TABLE", "notes")],
        doc(vec![assistant_text("remember this")]),
    );
    assert_eq!(insert_args(&out)["table"], "notes");
}

#[test]
fn the_store_reply_echo_is_swallowed_silently() {
    // The store answers the insert with a tool_result under the bridge's own
    // 'archive-' id. Routed back here (the echo lane), it must die QUIETLY --
    // an empty multi-send, no emission, no dead-letter. This is deliberate
    // behavior, documented in the README, not an accident.
    let out = emit(doc(vec![json!({
        "origin": "tool", "type": "tool_result",
        "id": "archive-0a1b2c3d", "text": "{\"rows_affected\":1}"
    })]));
    assert!(out.is_empty(), "the echo is swallowed, terminal: {out:?}");
}

#[test]
fn an_input_without_an_assistant_answer_is_terminal() {
    let out = emit(doc(vec![
        json!({"origin": "user", "type": "text", "text": "what is CEL?"}),
    ]));
    assert!(
        out.is_empty(),
        "nothing to archive, nothing leaves: {out:?}"
    );

    let empty = emit(doc(vec![assistant_text("")]));
    assert!(
        empty.is_empty(),
        "an empty answer is not an archive row: {empty:?}"
    );
}

// ======================================================================= COLONY

/// The shipped template, copied cell by cell: only `config.json` files travel,
/// so the cell under test IS the template and nothing else.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).unwrap();
        }
    }
}

fn template_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../builder/templates/archive-bridge")
}

/// A real `store` cell owning the archive table -- the bridge inserts into it,
/// and its reply echo travels back over the echo lane.
fn store_config(table: &str) -> Value {
    json!({
        "cell": {"type": "store"},
        "params": {
            "schema": {table: {"id": "text", "text": "text", "recorded_at": "text"}},
            "query_timeout_ms": 5000
        },
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {
                    "operation": {"type": "string", "required": true},
                    "rows_affected": {"type": "number", "required": false},
                    "error_code": {"type": "string", "required": false}
                }
            },
            "consumes": {"body": {"messages": {"type": "array", "required": true}}},
            "capabilities": ["db:own"]
        },
        "description": {
            "purpose": "Test archive store for the bridge colony test.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

/// The wiring the README documents: one store lane out, one unconditional echo
/// lane back in. No sink -- the echo must end INSIDE the bridge.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./archive", "to": "./keep",
         "condition": "has(hop.route) && hop.route == 'store'"},
        {"from": "./keep", "to": "./archive"}
    ]}}})
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn build_tree(td: &tempfile::TempDir, env: &str, table: &str) {
    let root = td.path();
    std::fs::write(root.join(".env"), env).unwrap();
    write(root, "main/config.json", &main_config());
    copy_cells(&template_dir(), &root.join("main/archive"));
    write(root, "main/keep/config.json", &store_config(table));
}

fn answer_message(text: &str) -> Message {
    MessageBuilder::new(Path::new("/archive"))
        .body(Body::Inline(
            json!({"messages": [{"origin": "assistant", "type": "text", "text": text}]}),
        ))
        .ttl(64)
        .build()
}

/// Polls the store's cell.db until the archived row exists; returns
/// (id, recorded_at). 30 s failure marker, robust under cargo parallel load.
fn await_row(store_db: &std::path::Path, table: &str, text: &str) -> (String, String) {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(conn) = rusqlite::Connection::open(store_db) {
            let q = format!("SELECT id, recorded_at FROM {table} WHERE text = ?1");
            if let Ok(row) = conn.query_row(&q, [text], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            }) {
                return row;
            }
        }
        if std::time::Instant::now() > deadline {
            panic!("no {table} row with text {text:?} within 30s");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn dlq_count(root: &std::path::Path) -> i64 {
    let conn = rusqlite::Connection::open(root.join("colony.db")).expect("colony.db");
    conn.query_row("SELECT COUNT(*) FROM dead_letters", [], |r| r.get(0))
        .expect("dead_letters count")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_answer_is_archived_and_the_echo_leaves_no_dead_letter() {
    let td = tempfile::TempDir::new().unwrap();
    build_tree(&td, "", "archive");
    let (h, _sink_rx, _park_rx) = boot(&td).await;

    h.send(answer_message("The answer is 42.")).await;

    // The positive receipt: the row is IN the store's own cell.db.
    let store_db = td.path().join("main/keep/cell.db");
    let (id, recorded_at) = await_row(&store_db, "archive", "The answer is 42.");
    assert!(!id.is_empty(), "the row carries an id");
    assert!(
        recorded_at.contains('T'),
        "recorded_at is a timestamp: {recorded_at:?}"
    );

    // The echo proof: the store HAS replied by the time the row is visible
    // (reply and write leave the same handle() call). Give the echo two hops
    // of headroom, then require a clean DLQ -- the bridge swallowed it.
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        dlq_count(td.path()),
        0,
        "the echo lane ends in the bridge, not in the DLQ"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_archive_table_knob_reaches_the_store_args() {
    let td = tempfile::TempDir::new().unwrap();
    // ARCHIVE_TABLE=notes, and the store owns a `notes` table: the knob is
    // boot-substituted into the shipped script.
    build_tree(&td, "ARCHIVE_TABLE=notes\n", "notes");
    let (h, _sink_rx, _park_rx) = boot(&td).await;

    h.send(answer_message("remember this")).await;

    let store_db = td.path().join("main/keep/cell.db");
    let (id, _) = await_row(&store_db, "notes", "remember this");
    assert!(!id.is_empty());

    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(dlq_count(td.path()), 0, "the echo dies in the bridge");

    h.shutdown().await;
}
