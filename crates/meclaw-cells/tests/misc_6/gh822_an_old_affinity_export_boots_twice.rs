//! GH #822 — the owner's case (plan E § 1 E.4): an `affinity.proposals` export
//! written before `affinity@3.4.0`, which had no `audience` column, is born and
//! boots TWICE without a hand edit.
//!
//! Before #822 the `store` compared a seed header with its declaration by one
//! rule — every declared column must be named — so the old export failed the
//! spawn of the new store ("column audience missing in seed header"), on every
//! boot, and the only way to move such a member was to edit the export.
//!
//! Since #822 the shipped declaration says what such a row holds:
//! `proposals.audience` and `proposals.origin_round` are `{type: text,
//! default: ""}`. Both defaults are fail-closed — an empty audience releases
//! nothing to anybody (`decide_proposal` writes no `disclosure` without one),
//! an empty round is a row no round-bound task may use — so the old row lands,
//! and lands invisible.
//!
//! Two birth paths, both measured at the `cell.db` (the receiver):
//! 1. the boot path — a colony tree with `store/seed/proposals.jsonl`, booted:
//!    the store's own loader at `OpenStatus::Created`, then a second boot
//!    (`Resumed`) on the same directory;
//! 2. the mutation path — `add_nodes` from a template carrying the same seed:
//!    the staging seeder, then the store's wake.

use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{CellFactory, ColonyMsg, ContractView, MutationOutcome, SpawnedCellKind};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Path, Uuid};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Generous failure-marker timeout (CONTRIBUTING.md 30s convention).
const TIMEOUT: Duration = Duration::from_secs(30);

/// The shipped affinity store declaration, read from the repository.
fn shipped_store_config() -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../templates/affinity/store/config.json");
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).expect("affinity store config"))
        .expect("affinity store config parses")
}

/// One `proposals` part as an `affinity` before 3.4.0 wrote it: no `audience`,
/// no `origin_round`.
const OLD_EXPORT: &str = concat!(
    r#"{"schema":{"id":"text","source_ref":"text","entity_ref":"text","field_path":"text","value":"json","status":"text","decided_by":"text","decided_at":"text","recorded_at":"text","supersedes":"text"}}"#,
    "\n",
    r#"{"id":"prop:old-1","source_ref":"mem:ep-1","entity_ref":"entity:alex","field_path":"aieos.interests.favorites.music_genre","value":"jazz","status":"accepted","decided_by":"curator","decided_at":"2026-01-01T00:00:00Z","recorded_at":"2026-01-01T00:00:00Z","supersedes":""}"#,
    "\n"
);

/// The proposal rows as the `cell.db` holds them: `(id, status, audience,
/// origin_round)`, `NULL` read as `"<null>"` so a missing default is visible.
fn proposals(db: &std::path::Path) -> Vec<(String, String, String, String)> {
    let conn = rusqlite::Connection::open(db).expect("open cell.db");
    let mut st = conn
        .prepare(
            "SELECT id, status, COALESCE(audience, '<null>'), COALESCE(origin_round, '<null>') \
             FROM proposals ORDER BY id",
        )
        .expect("the proposals table carries both columns");
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows")
}

fn the_old_row_invisible() -> Vec<(String, String, String, String)> {
    vec![(
        "prop:old-1".to_string(),
        "accepted".to_string(),
        String::new(),
        String::new(),
    )]
}

/// One boot of the store cell in `cell_dir`, exactly as the colony runs it:
/// `spawn_cell` (which checks the seed against the declaration) and the wake
/// (DDL, then the seed only when the `cell.db` is new). Ends the cell again.
async fn boot(cell_dir: &std::path::Path) -> Result<(), String> {
    let raw = shipped_store_config()["params"].clone();
    let (otx, _orx) = mpsc::channel(8);
    let (itx, _irx) = mpsc::channel(8);
    let spawned = Arc::new(StoreCellFactory).spawn_cell(
        Path::new("/member/affinity/store"),
        raw,
        otx,
        cell_dir.to_path_buf(),
        ContractView::default(),
        itx,
        None,
        0,
        None,
        None,
        1000,
    )?;
    let SpawnedCellKind::Dormant {
        sender,
        receiver,
        wake,
        ..
    } = spawned
    else {
        panic!("the store is a lazy, stateful cell: Dormant");
    };
    // DDL and seed run synchronously inside the wake, before the task starts.
    let (stop_tx, done_rx) = wake(receiver);
    drop(sender);
    let _ = stop_tx.send(());
    let _ = tokio::time::timeout(TIMEOUT, done_rx).await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gh822_an_old_affinity_export_boots_twice() {
    let td = tempfile::TempDir::new().unwrap();
    let cell_dir = td.path().join("store");
    std::fs::create_dir_all(cell_dir.join("seed")).unwrap();
    std::fs::write(cell_dir.join("seed/proposals.jsonl"), OLD_EXPORT).unwrap();

    boot(&cell_dir)
        .await
        .expect("boot 1: the old export must be born without a hand edit");
    assert_eq!(
        proposals(&cell_dir.join("cell.db")),
        the_old_row_invisible(),
        "boot 1: the old row lands with the declared, fail-closed defaults"
    );

    boot(&cell_dir)
        .await
        .expect("boot 2: the same seed and the same declaration boot again");
    assert_eq!(
        proposals(&cell_dir.join("cell.db")),
        the_old_row_invisible(),
        "boot 2: nothing re-seeded, nothing lost, still invisible"
    );
}

/// Without a default the same case is still a refusal at birth, naming the
/// column — the declaration decides, never the reader.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_old_export_missing_a_column_without_default_is_refused_at_birth() {
    let td = tempfile::TempDir::new().unwrap();
    let cell_dir = td.path().join("store");
    std::fs::create_dir_all(cell_dir.join("seed")).unwrap();
    // `status` has no default: a writer without it is not an older writer of
    // this table, it is a different table.
    let header_without_status = OLD_EXPORT
        .lines()
        .next()
        .unwrap()
        .replace(r#""status":"text","#, "");
    std::fs::write(
        cell_dir.join("seed/proposals.jsonl"),
        format!("{header_without_status}\n"),
    )
    .unwrap();
    let err = boot(&cell_dir).await.expect_err("refused at spawn");
    assert!(
        err.contains("schema_mismatch") && err.contains("column status"),
        "{err}"
    );
    assert!(
        !cell_dir.join("cell.db").exists(),
        "a refusal at birth leaves no database behind"
    );
}

// ───────────────────────────────────────────── the mutation path

const NODE: &str = "keeper";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_old_affinity_export_is_born_through_a_mutation() {
    let td = tempfile::TempDir::new().unwrap();
    let factory: Arc<dyn CellFactory> = Arc::new(StoreCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("store".to_string(), factory)]);

    let templates_root = td.path().join("templates");
    let tpl = templates_root.join(NODE);
    std::fs::create_dir_all(tpl.join("seed")).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{NODE}"}}"#)).unwrap();
    std::fs::write(tpl.join("config.json"), shipped_store_config().to_string()).unwrap();
    std::fs::write(tpl.join("seed/proposals.jsonl"), OLD_EXPORT).unwrap();
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap().expect("rescan");

    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": "/", "diff": {"add_nodes": [{"name": NODE, "template": NODE}]}}),
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
        "the staging seeder must take the old export: {outcome:?}"
    );
    // The staging seeder wrote the database; the row is already final there.
    assert_eq!(
        proposals(&td.path().join(NODE).join("cell.db")),
        the_old_row_invisible()
    );
}
