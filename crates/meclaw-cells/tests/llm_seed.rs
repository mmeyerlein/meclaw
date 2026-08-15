//! GH #99 — the `llm` cell's JSONL seed path (`seed/system.jsonl`).
//!
//! The seed concept is generic in the overview (§ Seed-Konzept) but was
//! implemented for the `store` cell only. Without it a template cannot ship a
//! default identity: the agent has no self until the first `system.*` update
//! arrives as a message. These tests pin the four halves of the behaviour:
//!
//! * a fresh `cell.db` (`OpenStatus::Created`) is seeded, and the seeded
//!   `system.identity` reaches the provider on the FIRST call (the receipt is
//!   the mock wire, not a `cell.db` row — a row nobody reads proves nothing);
//! * a resume never re-seeds (the accumulated identity survives a restart —
//!   the overview's double-row rule);
//! * a `{text_id}` leaf in the seed rejects the spawn loudly, and leaves no
//!   `cell.db` behind that a later spawn would resume unseeded;
//! * a missing seed file is the normal case, not an error.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::llm::LlmCellFactory;
use meclaw_colony::{CellFactory, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, Path};
use mock_openai::{MockOpenAI, canned_chat_completion};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::mpsc;

/// The seed file a template would ship with an `llm` cell.
const SEED: &str = r#"{"schema":{"slot_path":"text","value":"json"}}
{"slot_path":"identity","value":{"text":"I am the seeded assistant."}}
"#;

fn write_seed(cell_dir: &std::path::Path, body: &str) {
    std::fs::create_dir_all(cell_dir.join("seed")).unwrap();
    std::fs::write(cell_dir.join("seed/system.jsonl"), body).unwrap();
}

fn params(base_url: &str) -> Value {
    json!({
        "provider": "openai",
        "model": "gpt-x",
        "api_key": "sk-test",
        "base_url": format!("{base_url}/v1"),
    })
}

/// Spawn the cell through the real factory. Returns the `Dormant` wiring, or
/// the factory's error string.
#[allow(clippy::type_complexity)]
fn spawn(
    cell_dir: &std::path::Path,
    raw: Value,
) -> Result<
    (
        mpsc::Sender<Message>,
        mpsc::Receiver<Message>,
        meclaw_colony::WakeFn,
        mpsc::Receiver<CellEmission>,
    ),
    String,
> {
    let (otx, orx) = mpsc::channel::<CellEmission>(8);
    let (itx, irx) = mpsc::channel(8);
    // The colony inbox receiver must outlive the cell task (the watcher sends
    // `CellDied` into it); leaking it keeps the channel open for the test.
    std::mem::forget(irx);
    let kind = Arc::new(LlmCellFactory).spawn_cell(
        Path::new("/llm"),
        raw,
        otx,
        cell_dir.to_path_buf(),
        meclaw_colony::ContractView::default(),
        itx,
        None,
        32,
        None,
        None,
        16,
    )?;
    match kind {
        SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            ..
        } => Ok((sender, receiver, wake, orx)),
        SpawnedCellKind::Active { .. } => unreachable!("llm spawns Dormant"),
    }
}

/// A fresh spawn seeds `cell.db`, and the seeded leaf is in the system message
/// of the very first provider call — no `system.*` update message needed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seeded_identity_reaches_the_first_provider_call() {
    let td = TempDir::new().unwrap();
    write_seed(td.path(), SEED);
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;

    let (sender, receiver, wake, mut orx) = spawn(td.path(), params(&mock.base_url)).unwrap();
    wake(receiver);
    sender
        .send(
            MessageBuilder::new(Path::new("/llm"))
                .reply_to(Path::new("/observer"))
                .body(Body::Inline(json!({
                    "messages": [{"origin": "user", "type": "text", "text": "Hi"}]
                })))
                .build(),
        )
        .await
        .unwrap();
    orx.recv().await.expect("the cell must emit its answer");

    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "exactly one provider call per inference");
    let messages = snaps[0].messages().expect("request must carry messages[]");
    assert_eq!(
        messages[0]["role"], "system",
        "the seeded identity must lead the request: {messages:?}"
    );
    assert_eq!(
        messages[0]["content"], "I am the seeded assistant.",
        "the seeded system.identity must reach the provider: {messages:?}"
    );
    assert_eq!(messages[1]["role"], "user");
}

/// The accumulated identity survives a restart: a second spawn against the same
/// `cell_dir` opens the `cell.db` as `Resumed` and must NOT replay the template
/// default over what the agent has learned since.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resume_does_not_re_seed() {
    let td = TempDir::new().unwrap();
    write_seed(td.path(), SEED);
    let mock = MockOpenAI::start(vec![]).await;

    // Cycle 1 (fresh): the seed lands.
    let (s1, r1, w1, _o1) = spawn(td.path(), params(&mock.base_url)).unwrap();
    drop((s1, r1, w1));
    let conn = rusqlite::Connection::open(td.path().join("cell.db")).unwrap();
    let seeded: String = conn
        .query_row(
            "SELECT value FROM system WHERE slot_path='identity'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(seeded, r#"{"text":"I am the seeded assistant."}"#);
    // The agent learns: a `system.identity` update arrives by message and
    // replaces the seeded default (the normal accumulation path).
    conn.execute(
        "UPDATE system SET value=? WHERE slot_path='identity'",
        [r#"{"text":"I am the seeded assistant, and I learned a name."}"#],
    )
    .unwrap();
    drop(conn);

    // Cycle 2 (resume): same cell_dir, same seed file on disk.
    let (s2, r2, w2, _o2) = spawn(td.path(), params(&mock.base_url)).unwrap();
    drop((s2, r2, w2));
    let conn = rusqlite::Connection::open(td.path().join("cell.db")).unwrap();
    let after: String = conn
        .query_row(
            "SELECT value FROM system WHERE slot_path='identity'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        after, r#"{"text":"I am the seeded assistant, and I learned a name."}"#,
        "a resume must not replay the seed over the accumulated identity"
    );
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM system", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1, "no second row either");
}

/// A `{text_id}` leaf would sit in `cell.db` unresolved: the substrate expands
/// that class at the DELIVERY boundary (GH #86), which a seeded leaf never
/// passes. Loud reject at spawn — and no `cell.db` left behind, so the fixed
/// seed is not silently skipped as a resume on the next attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seeded_text_id_leaf_rejects_the_spawn() {
    let td = TempDir::new().unwrap();
    write_seed(
        td.path(),
        r#"{"schema":{"slot_path":"text","value":"json"}}
{"slot_path":"identity","value":{"text_id":"01H"}}
"#,
    );
    // (`expect_err` is out: the Ok-half of the tuple holds a `WakeFn`, which is
    // not `Debug`.)
    let err = match spawn(td.path(), params("http://127.0.0.1:1")) {
        Err(e) => e,
        Ok(_) => panic!("a seeded {{text_id}} leaf must reject the spawn"),
    };
    assert!(
        err.contains("text_id"),
        "the error must name the offending class: {err}"
    );
    assert!(
        !td.path().join("cell.db").exists(),
        "a rejected seed must not leave a fresh cell.db behind — the next spawn \
         would resume it and skip the (then fixed) seed"
    );
    // Same verdict on the static path, so `--validate` catches it too.
    assert!(
        LlmCellFactory
            .validate_cell_dir(&params("http://127.0.0.1:1"), td.path())
            .is_err()
    );
}

/// The pre-#99 normal case: no `seed/` directory at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_seed_file_is_not_an_error() {
    let td = TempDir::new().unwrap();
    let (s, r, w, _o) = spawn(td.path(), params("http://127.0.0.1:1")).expect("spawn must succeed");
    drop((s, r, w));
    let conn = rusqlite::Connection::open(td.path().join("cell.db")).unwrap();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM system", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0, "nothing to seed, nothing seeded");
    LlmCellFactory
        .validate_cell_dir(&params("http://127.0.0.1:1"), td.path())
        .expect("a missing seed file stays legal");
}
