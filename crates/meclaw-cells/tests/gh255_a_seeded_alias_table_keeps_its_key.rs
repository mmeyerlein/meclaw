//! GH #255 — a store-owned table that was seeded still owns its key.
//!
//! Two mechanisms that are each correct alone, and only wrong when they meet:
//!
//! 1. The mutation staging seeder builds a table from the header line of
//!    `seed/<table>.jsonl` alone — `CREATE TABLE IF NOT EXISTS "<table>"
//!    (<col> <type>, …)`, with **no key**. It runs at INSTANTIATION time, in
//!    the colony, before the cell has ever been awake.
//! 2. The store's own `apply_canonical_ddl` creates the alias table and the
//!    rejected-pair table with their PRIMARY KEY — also with `IF NOT EXISTS`,
//!    and it runs at the store's FIRST WAKE, which is necessarily later.
//!
//! So a template that ships a `seed/` file for one of the two store-owned
//! tables of a `params.canonical` binding hands the store a table that is
//! already standing, and `IF NOT EXISTS` leaves it exactly as it found it —
//! without the key. That the two cannot simply be reordered is structural:
//! the seeder lives in `meclaw-colony`, the canonical DDL in `meclaw-cells`,
//! and the dependency runs cells → colony, not back.
//!
//! What that costs is the upsert. `set_alias` and `reject_pair` both write
//! `INSERT … ON CONFLICT(<key>) DO UPDATE`, and SQLite refuses a conflict
//! target that matches no PRIMARY KEY or UNIQUE constraint — so the write does
//! not duplicate, it FAILS, every time, and the judgement it carried is simply
//! never recorded. The store answers with an `error_code`, on a lane (the
//! nightly GC) whose answers nobody reads; nothing at birth complains at all.
//!
//! This file drives the real chain: a real template through a real
//! `add_nodes` mutation (so the real staging seeder builds the table), a real
//! `StoreCellFactory` wake (so the real canonical DDL meets it), and the two
//! real ops over the wire. What it asserts is what a caller can see — the
//! store's own answer, and then the rows in the `cell.db` it wrote.

use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{CellFactory, ColonyMsg, MutationOutcome};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Generous failure-marker timeout (CONTRIBUTING.md 30s convention).
const RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// The template name, the node name and therefore the cell directory.
const NODE: &str = "keeper";

/// One binding on `facts`, with both store-owned tables declared — and both
/// of them shipped as a seed file, which is the whole point of the fixture.
fn store_config() -> Value {
    json!({
        "cell": {"type": "store"},
        "params": {
            "schema": {
                "facts": {"claim": "text", "canonical_claim": "text"}
            },
            "canonical": {
                "facts": [{
                    "source": "claim",
                    "target": "canonical_claim",
                    "aliases": "claim_aliases",
                    "rejected": "claim_rejected_pairs"
                }]
            }
        },
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    })
}

/// Both seeds carry one row, so the repair has something to lose: a seeded
/// row that disappears while the key is restored would be the cure being
/// worse than the disease.
const ALIAS_SEED: &str = concat!(
    r#"{"schema": {"alias": "text", "canonical": "text", "recorded_at": "text"}}"#,
    "\n",
    r#"{"alias": "jogging", "canonical": "running", "recorded_at": "2026-01-01T00:00:00Z"}"#,
    "\n"
);

const REJECTED_SEED: &str = concat!(
    r#"{"schema": {"left_value": "text", "right_value": "text", "recorded_at": "text"}}"#,
    "\n",
    r#"{"left_value": "cycling", "right_value": "spinning", "recorded_at": "2026-01-01T00:00:00Z"}"#,
    "\n"
);

/// Write the template and make the colony aware of it.
async fn install_template(td: &tempfile::TempDir, h: &ColonyHandle) {
    let templates_root = td.path().join("templates");
    let tpl = templates_root.join(NODE);
    std::fs::create_dir_all(tpl.join("seed")).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{NODE}"}}"#)).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        meclaw_core::serde_json::to_string(&store_config()).unwrap(),
    )
    .unwrap();
    std::fs::write(tpl.join("seed/claim_aliases.jsonl"), ALIAS_SEED).unwrap();
    std::fs::write(tpl.join("seed/claim_rejected_pairs.jsonl"), REJECTED_SEED).unwrap();

    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx
        .await
        .unwrap()
        .expect("GH #440: the rescan must not have aborted");
}

async fn colony_with_sink(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factory: Arc<dyn CellFactory> = Arc::new(StoreCellFactory);
    let h = ColonyHandle::new_with_factories_at(td, vec![("store".to_string(), factory)]);
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    (h, sink_rx)
}

/// `add_nodes` from the template — this is the step that runs the staging
/// seeder and therefore the step that creates the two tables.
async fn grow_the_store(h: &ColonyHandle) {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({
                "scope": "/",
                "diff": {"add_nodes": [{"name": NODE, "template": NODE}]}
            }),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    let outcome = ack_rx.await.unwrap();
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "add_nodes of /{NODE} must commit; got {outcome:?}"
    );
    // The store answers over its out-edge, so the answer needs somewhere to go.
    h.add_edge(
        Uuid::now_v7(),
        Path::new(&format!("/{NODE}")),
        Path::new("/sink"),
    )
    .await;
}

/// One store op as a `tool_call` turn, answered back to `/sink`.
fn op(args: Value) -> Message {
    MessageBuilder::new(Path::new(&format!("/{NODE}")))
        .reply_to(Path::new("/sink"))
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(json!({"messages": [{
            "origin": "assistant",
            "type": "tool_call",
            "text": meclaw_core::serde_json::to_string(&args).unwrap(),
            "id": "call_1"
        }]})))
        .build()
}

async fn recv(sink_rx: &mut mpsc::Receiver<Message>, what: &str) -> Message {
    tokio::time::timeout(RECV_TIMEOUT, sink_rx.recv())
        .await
        .unwrap_or_else(|_| panic!("sink recv timeout: {what}"))
        .unwrap_or_else(|| panic!("sink channel closed: {what}"))
}

/// Send one op and return the store's own answer: the `error_code` header (if
/// any) and the text of the `tool_result` turn.
async fn answer(
    h: &ColonyHandle,
    sink_rx: &mut mpsc::Receiver<Message>,
    args: Value,
) -> (Option<String>, String) {
    h.send(op(args.clone())).await;
    let m = recv(sink_rx, &args.to_string()).await;
    let code = m
        .headers
        .hop
        .get("error_code")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let text = match &m.body {
        Body::Inline(v) => v["messages"][0]["text"].as_str().unwrap_or("").to_string(),
        Body::Blob(_) => panic!("inline expected"),
    };
    (code, text)
}

/// Read the store's `cell.db` from outside — an observation of the result, not
/// a re-implementation of the mechanism.
fn rows(td: &tempfile::TempDir, sql: &str) -> Vec<Vec<String>> {
    let db = td.path().join(NODE).join("cell.db");
    assert!(db.is_file(), "the store never wrote a cell.db at {db:?}");
    let conn = rusqlite::Connection::open(&db).expect("open cell.db");
    let mut st = conn.prepare(sql).expect("prepare");
    let n = st.column_count();
    st.query_map([], |r| {
        Ok((0..n)
            .map(|i| {
                r.get::<_, Option<String>>(i)
                    .unwrap_or_default()
                    .unwrap_or_default()
            })
            .collect::<Vec<String>>())
    })
    .expect("query")
    .collect::<Result<Vec<_>, _>>()
    .expect("rows")
}

/// The primary-key columns SQLite reports for a table, in key order.
fn primary_key(td: &tempfile::TempDir, table: &str) -> Vec<String> {
    rows(
        td,
        &format!("SELECT name FROM pragma_table_info('{table}') WHERE pk > 0 ORDER BY pk"),
    )
    .into_iter()
    .map(|r| r[0].clone())
    .collect()
}

// ─────────────────────────────────────────────────────────────── the claims

/// A `set_alias` written twice for the same alias is a correction, not a
/// second row — even when the alias table was built by the seeder first.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seeded_alias_table_still_upserts_on_its_key() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut sink_rx) = colony_with_sink(&td).await;
    install_template(&td, &h).await;
    grow_the_store(&h).await;

    for canonical in ["exercise", "physical exercise"] {
        let (code, text) = answer(
            &h,
            &mut sink_rx,
            json!({
                "operation": "set_alias",
                "table": "facts",
                "column": "claim",
                "alias": "yoga",
                "canonical": canonical
            }),
        )
        .await;
        assert_eq!(
            code, None,
            "set_alias -> {canonical:?} was refused by the database: {text}"
        );
    }

    assert_eq!(
        primary_key(&td, "claim_aliases"),
        vec!["alias".to_string()],
        "the seeded alias table stands without the key set_alias upserts on"
    );
    assert_eq!(
        rows(
            &td,
            "SELECT alias, canonical FROM claim_aliases ORDER BY alias"
        ),
        vec![
            vec!["jogging".to_string(), "running".to_string()],
            vec!["yoga".to_string(), "physical exercise".to_string()],
        ],
        "the second judgement must correct the first, and the seeded row must survive"
    );
}

/// The same claim for the other store-owned table, whose key is a PAIR: a
/// re-judged pair is a correction, not a second refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seeded_rejected_pair_table_still_upserts_on_its_pair() {
    let td = tempfile::TempDir::new().unwrap();
    let (h, mut sink_rx) = colony_with_sink(&td).await;
    install_template(&td, &h).await;
    grow_the_store(&h).await;

    for at in ["2026-02-01T00:00:00Z", "2026-03-01T00:00:00Z"] {
        let (code, text) = answer(
            &h,
            &mut sink_rx,
            json!({
                "operation": "reject_pair",
                "table": "facts",
                "column": "claim",
                "left": "swimming",
                "right": "sauna",
                "recorded_at": at
            }),
        )
        .await;
        assert_eq!(
            code, None,
            "reject_pair at {at:?} was refused by the database: {text}"
        );
    }

    assert_eq!(
        primary_key(&td, "claim_rejected_pairs"),
        vec!["left_value".to_string(), "right_value".to_string()],
        "the seeded rejected-pair table stands without the pair key reject_pair upserts on"
    );
    assert_eq!(
        rows(
            &td,
            "SELECT left_value, right_value, recorded_at FROM claim_rejected_pairs \
             ORDER BY left_value"
        ),
        vec![
            vec![
                "cycling".to_string(),
                "spinning".to_string(),
                "2026-01-01T00:00:00Z".to_string()
            ],
            vec![
                "sauna".to_string(),
                "swimming".to_string(),
                "2026-03-01T00:00:00Z".to_string()
            ],
        ],
        "the second refusal must correct the first, and the seeded pair must survive"
    );
}
