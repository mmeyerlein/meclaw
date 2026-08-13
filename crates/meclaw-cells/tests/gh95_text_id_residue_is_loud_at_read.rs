//! GH #95 — a pre-#86 `{text_id}` row in `cell.db.system` fails the call
//! LOUDLY instead of silently dropping out of the system prompt.
//!
//! Since GH #86 the substrate resolves the `{text_id}` pointer class at the
//! delivery boundary, so an `llm` cell only ever persists `{"text": …}`
//! leaves. A `cell.db` written before that boundary existed can still hold a
//! pointer row, and nothing resolves it any more: `concat_system_prompt`
//! stops at `text`, so the row's content would vanish from the prompt with
//! no report — the silent-wrong failure class.
//!
//! Ruling (GH #95, night-wave 3 track G): **loud-at-read**. When the handle
//! path reads the system tree back out of `cell.db`, a leaf still carrying
//! `text_id` is a regular cell error (no panic, no restart) naming the
//! slot_path and the pre-#86 residue origin. The provider is never called
//! with a shortened prompt.
//!
//! The fixture writes the residue row BY HAND into a fresh `cell.db` —
//! exactly how such a row exists in the wild (persisted verbatim by a
//! pre-#86 `handle()`, unreachable through any post-#86 delivery).

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::LlmCell;
use meclaw_cells::llm::params::LlmParams;
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use mock_openai::{MockOpenAI, canned_chat_completion};
use tempfile::TempDir;
use tokio::sync::mpsc;

fn mk_sink() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (tx, rx) = mpsc::channel::<CellEmission>(8);
    let sink = OutputSink::new(
        tx,
        Path::new("/llm"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        32,
        meclaw_core::Headers::new(),
        None,
    );
    (sink, rx)
}

/// Open a fresh `cell.db` in `td` and hand-write the given `system` rows —
/// the pre-#86 state, created without any delivery-boundary involvement.
fn cell_db_with_rows(td: &TempDir, rows: &[(&str, &str)]) -> rusqlite::Connection {
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    for (slot_path, value) in rows {
        conn.execute(
            "INSERT INTO system (slot_path, value, updated_at) VALUES (?, ?, 1)",
            rusqlite::params![slot_path, value],
        )
        .unwrap();
    }
    conn
}

/// Drive one message-only inference through a real `LlmCell` sitting on the
/// prepared `conn`, against `mock`. Returns the emitted content.
async fn drive(mock: &MockOpenAI, conn: rusqlite::Connection) -> Value {
    let params = LlmParams::parse(&json!({
        "provider": "openai", "model": "gpt-x", "api_key": "sk-test",
        "base_url": format!("{}/v1", mock.base_url),
        "system_order": ["identity"],
    }))
    .expect("params");
    let mut cell = LlmCell::new(params, reqwest::Client::builder().build().unwrap());
    let mut db = DbConn::wrap(conn, None);
    let (sink, mut rx) = mk_sink();
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(json!({
            "messages": [{"origin": "user", "type": "text", "text": "Hi"}]
        })))
        .build();
    cell.handle(msg, &sink, &mut db).await;
    rx.recv()
        .await
        .expect("the cell must emit something")
        .content
}

/// The core pin: a hand-written `{text_id}` row fails the call with a regular
/// error naming the slot and the pre-#86 origin — and the provider is never
/// called with a prompt missing that leaf.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_text_id_residue_row_fails_the_call_loudly() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    let conn = cell_db_with_rows(
        &td,
        &[
            ("identity.soul", r#"{"text":"inline"}"#),
            (
                "identity.body",
                r#"{"text_id":"01H0000000000000000000000A"}"#,
            ),
        ],
    );
    let out = drive(&mock, conn).await;

    assert_eq!(out["header"]["finish_reason"], "error", "got: {out}");
    assert_eq!(out["header"]["error_code"], "provider_error");
    assert_eq!(out["meta"]["error"]["source"], "translate");
    let detail = out["meta"]["error"]["detail"].as_str().unwrap();
    assert!(
        detail.contains("'identity.body'"),
        "detail must name the slot_path: {detail}"
    );
    assert!(
        detail.contains("pre-#86 residue") && detail.contains("GH #95"),
        "detail must name the origin: {detail}"
    );
    // Gate-1 pass-through: the input messages travel unchanged.
    assert_eq!(out["messages"][0]["text"], "Hi");
    // The loud error REPLACES the shortened prompt — no provider call at all.
    assert!(
        mock.recorded_requests().await.is_empty(),
        "the provider must never see a silently shortened prompt"
    );
}

/// Negative pin, half 1: a mixed leaf (`text` next to a `text_id` rest) is
/// residue too — silently preferring the `text` half would hide the rest.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mixed_leaf_with_a_text_id_rest_is_loud_too() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    let conn = cell_db_with_rows(
        &td,
        &[(
            "identity.body",
            r#"{"text":"half","text_id":"01H0000000000000000000000A"}"#,
        )],
    );
    let out = drive(&mock, conn).await;

    assert_eq!(out["header"]["finish_reason"], "error", "got: {out}");
    let detail = out["meta"]["error"]["detail"].as_str().unwrap();
    assert!(
        detail.contains("'identity.body'") && detail.contains("pre-#86 residue"),
        "detail must name slot and origin: {detail}"
    );
    assert!(mock.recorded_requests().await.is_empty());
}

/// Negative pin, half 2: pure `{"text": …}` rows are untouched by the guard —
/// the call succeeds and the provider sees the exact pre-guard prompt bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pure_text_rows_reach_the_wire_byte_identical() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    let conn = cell_db_with_rows(
        &td,
        &[
            ("identity.body", r#"{"text":"the long persona"}"#),
            ("identity.soul", r#"{"text":"inline"}"#),
        ],
    );
    let out = drive(&mock, conn).await;

    assert_eq!(out["header"]["finish_reason"], "stop", "got: {out}");
    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "exactly one provider call");
    let messages = snaps[0].messages().expect("request must carry messages[]");
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(
        messages[0]["content"], "the long persona\n\ninline",
        "pure text rows must produce the exact pre-#95 prompt: {messages:?}"
    );
}
