//! GH #118 — writes into the persistent `system` tree are gated.
//!
//! The `system` tree is the `llm` cell's long-term state: rebuilt into the
//! prompt on every `handle()`, carrier of the tool menu (`system.tools.*`),
//! durable across restarts. Before this gate, any cell with an edge to an
//! `llm` cell could set any slot, in any size, in any number — whoever could
//! route to the cell owned its prompt and its tools, permanently.
//!
//! The gate has two halves, and these tests pin both plus the compatibility
//! contract that makes it deployable:
//!
//! * **Bounds, always on** — an oversized leaf and a write that would blow the
//!   slot budget are LOUD rejects (`error_code: "invalid_input"`), never a
//!   truncation and never a silent drop, and they write NOTHING (the
//!   `messages[]` half of the same transaction included).
//! * **Allowlist, opt-in** — `system_writable` pins which subtrees a MESSAGE
//!   may write. A cell that declares nothing keeps its pre-#118 behaviour, so
//!   the operator's direct `@external` system update (the E3/E5 production
//!   form) and every in-topology writer keep working.
//! * **Seed is configuration, not message** — a pinned cell still seeds its own
//!   identity at boot, and that identity then reaches the provider while a
//!   message can no longer overwrite it. That is the whole point of pinning.

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

/// A cell sitting on a fresh `cell.db` in `td`, with `extra` merged over the
/// minimal params. `base_url` points at the mock so a call that should NOT
/// happen is observable.
fn cell_with(td: &TempDir, base_url: &str, extra: Value) -> (LlmCell, DbConn) {
    let mut raw = json!({
        "provider": "openai", "model": "gpt-x", "api_key": "sk-test",
        "base_url": format!("{base_url}/v1"),
        "system_order": ["identity", "handover"],
    });
    for (k, v) in extra.as_object().expect("extra must be an object") {
        raw[k] = v.clone();
    }
    let params = LlmParams::parse(&raw).expect("params must parse");
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    (
        LlmCell::new(params, reqwest::Client::builder().build().unwrap()),
        DbConn::wrap(conn, None),
    )
}

/// Send one body into the cell. Returns the emitted content, or `None` when the
/// cell stayed silent (the system-only path).
async fn send(cell: &mut LlmCell, db: &mut DbConn, body: Value) -> Option<Value> {
    let (sink, mut rx) = mk_sink();
    let msg = MessageBuilder::new(Path::new("/llm"))
        .reply_to(Path::new("/observer"))
        .body(Body::Inline(body))
        .build();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    rx.recv().await.map(|e| e.content)
}

/// Read one slot back out of the cell's `cell.db`.
async fn slot(db: &mut DbConn, slot_path: &str) -> Option<String> {
    let p = slot_path.to_string();
    db.call(move |conn| -> rusqlite::Result<Option<String>> {
        Ok(conn
            .query_row(
                "SELECT value FROM system WHERE slot_path = ?",
                rusqlite::params![p],
                |r| r.get::<_, String>(0),
            )
            .ok())
    })
    .await
    .unwrap()
}

async fn slot_count(db: &mut DbConn) -> i64 {
    db.call(|conn| conn.query_row("SELECT COUNT(*) FROM system", [], |r| r.get(0)))
        .await
        .unwrap()
}

// ───────────────────────── the default lane stays open ─────────────────────

/// The operator lane (E3/E5): a system update addressed straight at the cell
/// over HTTP, no `messages[]`. A cell that declares no allowlist must accept it
/// exactly as before GH #118 — persist, stay silent, no provider call.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_operator_system_update_still_lands_by_default() {
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = cell_with(&td, &mock.base_url, json!({}));

    let out = send(
        &mut cell,
        &mut db,
        json!({"system": {"identity": {"text": "I am Egon."}}}),
    )
    .await;

    assert!(out.is_none(), "a system-only update stays silent: {out:?}");
    assert_eq!(
        slot(&mut db, "identity").await.as_deref(),
        Some(r#"{"text":"I am Egon."}"#),
        "the default gate must let the operator lane through"
    );
    assert!(
        mock.recorded_requests().await.is_empty(),
        "a system update never calls the provider"
    );
}

/// The inventory of every legitimate `system` writer in the repository, in the
/// SHAPE each one actually sends, pushed through the default gate in one batch.
///
/// Sources (one leaf per writer family):
/// * `identity.soul` / `instructions.style` — the persona cells of slack-agent,
///   egon and the coder-pipeline preps;
/// * `tools.web_search` — the prep cells of research-assistant /
///   coder-pipeline / the swarm and telegram-research examples (one level);
/// * `tools.main_mcp.calc` — MCP discovery (`mcp::emit_system_tools_listing`),
///   which nests the provider key under `tools` (two levels);
/// * `handover` — the summarizer hive's prep, sole writer of that slot (R-OS-1);
/// * `memory.bundle` / `memory.recall` — the memory hive's recall and the
///   collector's assemble seam;
/// * `consult` — the collector's advisor seam. **The only leaf in the whole
///   repository that is not pure text**: it carries a JSON ARRAY next to its
///   `text`, and the leaf definition stops at `text`, so the array rides along
///   inside the one leaf and has to survive the gate's size check as such;
/// * `context` — the 14c lexical-RAG retrieve cell;
/// * `facts.user_name` — the Phase-8 demo topology.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_known_topology_writers_all_pass_the_default_gate() {
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = cell_with(&td, &mock.base_url, json!({}));

    let out = send(
        &mut cell,
        &mut db,
        json!({"system": {
            "identity":     {"soul":  {"text": "I am Egon."}},
            "instructions": {"style": {"text": "Be brief."}},
            "tools":        {
                "web_search": {"text": "{\"name\":\"web_search\"}"},
                "main_mcp":   {"calc": {"text": "{\"name\":\"calc\"}"}},
            },
            "handover":     {"text": "yesterday's session"},
            "memory":       {
                "bundle": {"text": "[fact] alex | likes | blue"},
                "recall": {"text": "You remember that alex likes blue."},
            },
            "consult":      {"open": ["c-1", "c-2"],
                             "text": "open consults: c-1, c-2"},
            "context":      {"text": "retrieved chunk"},
            "facts":        {"user_name": {"text": "Alex"}},
        }}),
    )
    .await;

    assert!(out.is_none(), "system-only stays silent: {out:?}");
    assert_eq!(slot_count(&mut db).await, 10, "all ten leaves must land");
    // The array-carrying leaf survives intact — the gate measures it, it does
    // not reshape it.
    let consult = slot(&mut db, "consult").await.expect("consult must land");
    assert!(
        consult.contains(r#""open":["c-1","c-2"]"#),
        "the non-text half of the consult leaf must survive verbatim: {consult}"
    );
}

// ───────────────────────── the allowlist half ──────────────────────────────

/// The core case: a cell that pins its writable surface refuses an identity
/// overwrite that arrives by message — loudly, by name, and without writing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slot_outside_the_declaration_is_refused_loudly_and_writes_nothing() {
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = cell_with(
        &td,
        &mock.base_url,
        json!({"system_writable": ["handover"]}),
    );

    let out = send(
        &mut cell,
        &mut db,
        json!({"system": {"identity": {"text": "I am someone else now."}}}),
    )
    .await
    .expect("a refused write must ANSWER, never drop silently");

    assert_eq!(out["header"]["finish_reason"], "error", "got: {out}");
    assert_eq!(out["header"]["error_code"], "invalid_input");
    assert_eq!(out["meta"]["error"]["source"], "parse");
    let detail = out["meta"]["error"]["detail"].as_str().unwrap();
    assert!(detail.contains("'identity'"), "names the slot: {detail}");
    assert!(
        detail.contains("system_writable"),
        "names the rule: {detail}"
    );
    assert!(detail.contains("GH #118"), "names the issue: {detail}");
    assert!(
        !detail.contains("I am someone else now."),
        "the detail must never echo the leaf content: {detail}"
    );
    assert_eq!(
        slot_count(&mut db).await,
        0,
        "a refused write leaves the system tree untouched"
    );
}

/// The declared subtree keeps working — the summarizer's handover lane is the
/// production case this pin is built around.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_declared_subtree_still_accepts_its_writer() {
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = cell_with(
        &td,
        &mock.base_url,
        json!({"system_writable": ["handover"]}),
    );

    let out = send(
        &mut cell,
        &mut db,
        json!({"system": {"handover": {"text": "yesterday"}}}),
    )
    .await;
    assert!(out.is_none(), "accepted system-only stays silent: {out:?}");
    assert_eq!(
        slot(&mut db, "handover").await.as_deref(),
        Some(r#"{"text":"yesterday"}"#)
    );
}

/// All-or-nothing across the WHOLE transaction: a body that carries a refused
/// system leaf next to a `messages[]` array writes neither half and never
/// reaches the provider.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_write_rolls_back_the_messages_half_and_skips_the_provider() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("must not happen", "stop")]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = cell_with(
        &td,
        &mock.base_url,
        json!({"system_writable": ["handover"]}),
    );

    let out = send(
        &mut cell,
        &mut db,
        json!({
            "system": {"identity": {"text": "hijack"}},
            "messages": [{"origin": "user", "type": "text", "text": "Hi"}],
        }),
    )
    .await
    .expect("must answer");

    assert_eq!(out["header"]["error_code"], "invalid_input", "got: {out}");
    // Gate-1 pass-through: the input conversation travels on, so a failover
    // edge keyed on finish_reason="error" stays usable.
    assert_eq!(out["messages"][0]["text"], "Hi");
    assert_eq!(slot_count(&mut db).await, 0, "system half rolled back");
    let last_input: Option<String> = db
        .call(|conn| -> rusqlite::Result<Option<String>> {
            Ok(conn
                .query_row("SELECT message_json FROM last_input WHERE id=1", [], |r| {
                    r.get::<_, String>(0)
                })
                .ok())
        })
        .await
        .unwrap();
    assert!(
        last_input.is_none(),
        "the messages half must not survive a refused system write: {last_input:?}"
    );
    assert!(
        mock.recorded_requests().await.is_empty(),
        "a refused write never reaches the provider"
    );
}

// ───────────────────────── the bounds half ─────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_oversized_leaf_is_refused_whole_never_truncated() {
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = cell_with(&td, &mock.base_url, json!({"system_max_leaf_bytes": 128}));

    let out = send(
        &mut cell,
        &mut db,
        json!({"system": {"identity": {"text": "z".repeat(4096)}}}),
    )
    .await
    .expect("must answer");

    assert_eq!(out["header"]["error_code"], "invalid_input", "got: {out}");
    let detail = out["meta"]["error"]["detail"].as_str().unwrap();
    assert!(detail.contains("'identity'"), "names the slot: {detail}");
    assert!(
        detail.contains("system_max_leaf_bytes"),
        "names the rule: {detail}"
    );
    assert_eq!(
        slot_count(&mut db).await,
        0,
        "no truncated leaf may be left behind"
    );
}

/// The slot budget counts the tree, not the batch: refreshing a slot that is
/// already there is always in budget, opening a new subtree past the cap is not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_slot_budget_stops_growth_but_not_refresh() {
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    let (mut cell, mut db) = cell_with(&td, &mock.base_url, json!({"system_max_slots": 2}));

    send(
        &mut cell,
        &mut db,
        json!({"system": {"identity": {"text": "a"}, "handover": {"text": "b"}}}),
    )
    .await;
    assert_eq!(slot_count(&mut db).await, 2);

    // Refresh at the limit: fine.
    let out = send(
        &mut cell,
        &mut db,
        json!({"system": {"handover": {"text": "b2"}}}),
    )
    .await;
    assert!(out.is_none(), "a refresh at the limit must pass: {out:?}");
    assert_eq!(
        slot(&mut db, "handover").await.as_deref(),
        Some(r#"{"text":"b2"}"#)
    );

    // A new subtree past the cap: loud reject, nothing written.
    let out = send(
        &mut cell,
        &mut db,
        json!({"system": {"memory": {"text": "c"}}}),
    )
    .await
    .expect("must answer");
    assert_eq!(out["header"]["error_code"], "invalid_input", "got: {out}");
    let detail = out["meta"]["error"]["detail"].as_str().unwrap();
    assert!(
        detail.contains("system_max_slots"),
        "names the rule: {detail}"
    );
    assert_eq!(slot_count(&mut db).await, 2, "nothing was added");
    assert!(slot(&mut db, "memory").await.is_none());
}

// ───────────────────────── seed is configuration ───────────────────────────

/// The Egon-rollout shape, half one: the seed lands even though the cell pins
/// its message-writable surface elsewhere. A seed file is CONFIGURATION —
/// authored in the cell directory next to `config.json`, on the same trust tier
/// as the params that declare the pin — so it is not gated by the declaration
/// it sits next to. Gating it would be circular, and it would lock a pinned
/// cell out of its own identity at boot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pinned_cell_still_seeds_its_own_identity() {
    let mock = MockOpenAI::start(vec![canned_chat_completion("ok", "stop")]).await;
    let td = TempDir::new().unwrap();
    write_seed(&td);
    let (sender, _wiring, mut orx) = spawn_pinned(&td, &mock.base_url);

    // The SEEDED identity is what reaches the provider on the first call.
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
    recv_within(&mut orx).await;

    let snaps = mock.recorded_requests().await;
    assert_eq!(snaps.len(), 1, "exactly one provider call");
    let messages = snaps[0].messages().expect("request must carry messages[]");
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(
        messages[0]["content"], "I am the seeded assistant.",
        "the pin must protect the seeded identity, not erase it: {messages:?}"
    );
}

/// The Egon-rollout shape, half two: with the seed in place, a MESSAGE can no
/// longer overwrite that identity — it is refused, and the seeded row survives
/// untouched in `cell.db`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_message_cannot_overwrite_the_seeded_identity_of_a_pinned_cell() {
    let mock = MockOpenAI::start(vec![]).await;
    let td = TempDir::new().unwrap();
    write_seed(&td);
    let (sender, _wiring, mut orx) = spawn_pinned(&td, &mock.base_url);

    sender
        .send(
            MessageBuilder::new(Path::new("/llm"))
                .reply_to(Path::new("/observer"))
                .body(Body::Inline(
                    json!({"system": {"identity": {"text": "I am someone else."}}}),
                ))
                .build(),
        )
        .await
        .unwrap();
    let refused = recv_within(&mut orx).await;
    assert_eq!(
        refused["header"]["error_code"], "invalid_input",
        "got: {refused}"
    );

    let conn = rusqlite::Connection::open(td.path().join("cell.db")).unwrap();
    let identity: String = conn
        .query_row(
            "SELECT value FROM system WHERE slot_path='identity'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        identity, r#"{"text":"I am the seeded assistant."}"#,
        "the refused write must not have touched the seeded identity"
    );
    assert!(
        mock.recorded_requests().await.is_empty(),
        "a refused write never reaches the provider"
    );
}

/// The template seed a pinned cell ships with.
fn write_seed(td: &TempDir) {
    std::fs::create_dir_all(td.path().join("seed")).unwrap();
    std::fs::write(
        td.path().join("seed/system.jsonl"),
        "{\"schema\":{\"slot_path\":\"text\",\"value\":\"json\"}}\n\
         {\"slot_path\":\"identity\",\"value\":{\"text\":\"I am the seeded assistant.\"}}\n",
    )
    .unwrap();
}

/// Spawn a pinned (`system_writable: ["handover"]`) `llm` cell through the REAL
/// factory — the only path that runs the seed loader — and wake it.
///
/// The returned wiring must stay alive: dropping the stop-sender is a peace-stop
/// request. One message per woken cell: without a colony behind it nobody wakes
/// the cell again after its idle-sleep.
#[allow(clippy::type_complexity)]
fn spawn_pinned(
    td: &TempDir,
    base_url: &str,
) -> (
    mpsc::Sender<meclaw_core::Message>,
    (
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    ),
    mpsc::Receiver<CellEmission>,
) {
    use meclaw_colony::{CellFactory, SpawnedCellKind};
    use std::sync::Arc;

    let raw = json!({
        "provider": "openai", "model": "gpt-x", "api_key": "sk-test",
        "base_url": format!("{base_url}/v1"),
        "system_order": ["identity"],
        "system_writable": ["handover"],
    });
    let (otx, orx) = mpsc::channel::<CellEmission>(8);
    let (itx, irx) = mpsc::channel(8);
    // The colony inbox receiver must outlive the cell task (the watcher sends
    // `CellDied` into it); leaking it keeps the channel open for the test.
    std::mem::forget(irx);
    let kind = Arc::new(meclaw_cells::llm::LlmCellFactory)
        .spawn_cell(
            Path::new("/llm"),
            raw,
            otx,
            td.path().to_path_buf(),
            meclaw_colony::ContractView::default(),
            itx,
            None,
            32,
            None,
            None,
            16,
        )
        .expect("a pinned cell must still spawn and seed");
    let (sender, receiver, wake) = match kind {
        SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            ..
        } => (sender, receiver, wake),
        SpawnedCellKind::Active { .. } => unreachable!("llm spawns Dormant"),
    };
    let wiring = wake(receiver);
    (sender, wiring, orx)
}

/// Await one emission with the 30 s failure-marker convention: a gate that
/// silently accepts emits nothing, and the live cell task keeps the sender
/// alive — the wait must end in a verdict, not in a hang.
async fn recv_within(orx: &mut mpsc::Receiver<CellEmission>) -> Value {
    tokio::time::timeout(std::time::Duration::from_secs(30), orx.recv())
        .await
        .expect("the cell must answer, not swallow the message")
        .expect("the sink must stay open")
        .content
}
