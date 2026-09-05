//! GH #555 — a real `store` writes its own seed set, and the two things a bare
//! `cell.db` cannot show come along: the FTS5 index does not travel, and the
//! provenance columns do.
//!
//! The fence and the file shapes are pinned where they live
//! (`crates/meclaw-colony/tests/gh555_the_slot_writes_its_own_files.rs`). What
//! can only be shown here is the same claim `gh253_store_transfer.rs` makes for
//! the message form, now on a directory: `{"operation": "export", "to": …}`
//! without a `table` writes ONE file per content table of a live store, the
//! derived index is not one of them, and `audience_set`/`channel`/`speaker` are
//! in the file rather than being projected away on the way out.
//!
//! No template is involved. The fixture is a bare store cell — the schema the
//! memory hive's `facts` reduces to — spawned through the substrate's own
//! `build_stateful_task_with_peace`, which is the shape
//! `gh314_a_vault_does_not_travel.rs` uses for the same reason.

use meclaw_cells::store::{StoreCell, StoreParams};
use meclaw_colony::{DbConn, build_stateful_task_with_peace};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Message, MessageBuilder, Path, TransferBounds};
use std::sync::Arc;
use tokio::sync::mpsc;

const STORE: &str = "/main/memory/store";
const CALLER: &str = "/main/memory/porter";

/// The memory hive's `facts` shape reduced to what a transfer has to carry: an
/// identity, an indexed claim, and the three provenance columns.
fn facts_params() -> StoreParams {
    StoreParams::parse(&json!({
        "schema": {"facts": {
            "id": "text", "claim": "text",
            "audience_set": "json", "channel": "text", "speaker": "text"
        }},
        "fts": {"facts": ["claim"]}
    }))
    .unwrap()
}

/// A live store on a real `cell.db`, fenced to `base`, holding two rows.
async fn store_holding_two_facts(
    base: &std::path::Path,
) -> (
    tempfile::TempDir,
    mpsc::Sender<Message>,
    mpsc::Receiver<CellEmission>,
    tokio::task::JoinHandle<()>,
) {
    let dir = tempfile::TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&dir.path().join("cell.db")).unwrap();
    let params = facts_params();
    meclaw_cells::store::query::install_connection_extensions(&conn).unwrap();
    meclaw_cells::store::ddl::apply_schema_ddl(&conn, &params.schema).unwrap();
    meclaw_cells::store::ddl::apply_fts_ddl(&conn, &params.fts, &params.canonical).unwrap();
    for (id, claim) in [("f1", "alex prefers helix editors"), ("f2", "beta")] {
        let out = meclaw_cells::store::ops::dispatch(
            &conn,
            &json!({"operation": "insert", "table": "facts",
                    "row": {"id": id, "claim": claim, "audience_set": "[\"member:alex\"]",
                            "channel": "room:one", "speaker": "member:alex"}}),
        )
        .unwrap();
        assert_eq!(out.error_code, None, "{:?}", out.error_text);
    }

    let (mailbox, mb_rx) = mpsc::channel::<Message>(8);
    let (otx, out) = mpsc::channel::<CellEmission>(8);
    let (inbox_tx, _inbox_rx) = mpsc::channel::<meclaw_colony::ColonyMsg>(8);
    let bounds = TransferBounds {
        base_path: Some(Arc::from(base)),
        ..TransferBounds::default()
    };
    let (join, _peace, _stop, _ack, _backstop) = build_stateful_task_with_peace(
        Path::new(STORE),
        mb_rx,
        otx,
        inbox_tx,
        None,
        None,
        0,
        StoreCell::new(params),
        DbConn::wrap(conn, None),
        None,
        None,
        bounds,
    );
    (dir, mailbox, out, join)
}

/// A store's whole content set lands as one seed directory: one file per
/// content table, the derived index nowhere, and every column of every row —
/// provenance included — in the file a seed loader would read back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_store_exports_every_content_table_into_one_directory() {
    let fence = tempfile::TempDir::new().unwrap();
    let (dir, mailbox, mut out, join) = store_holding_two_facts(fence.path()).await;

    let msg = MessageBuilder::new(Path::new(STORE))
        .body(Body::Inline(
            json!({"transfer": {"operation": "export", "to": "gen-1"}}),
        ))
        .reply_to(Path::new(CALLER))
        .build();
    mailbox.send(msg).await.expect("mailbox open");
    let reply = tokio::time::timeout(std::time::Duration::from_secs(30), out.recv())
        .await
        .expect("the substrate must answer within 30s")
        .expect("the substrate must answer")
        .content;
    drop(mailbox);
    drop(out);
    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), join).await;
    drop(dir);

    assert!(
        reply["header"].get("error_code").is_none(),
        "an export into a declared fence is not refused: {reply}"
    );

    let seed = fence.path().join("gen-1/seed");
    let mut names: Vec<String> = std::fs::read_dir(&seed)
        .expect("the store must have written its own seed directory")
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();

    // The FTS index and its four shadow tables are derived data a receiving
    // cell rebuilds from its own rows — they are not content and get no file.
    assert!(
        names.iter().all(|n| !n.starts_with("facts_fts")),
        "an index must never be written as a seed file: {names:?}"
    );
    assert!(names.contains(&"facts.jsonl".to_string()), "{names:?}");
    assert!(
        names.contains(&"export_final.json".to_string()),
        "the completion marker is written last: {names:?}"
    );

    // Provenance is IN the file. An export projects every column, so a row that
    // travels through a directory carries the participant set it was learned in
    // front of — the same structural guarantee the message form gives.
    let text = std::fs::read_to_string(seed.join("facts.jsonl")).unwrap();
    let lines: Vec<&str> = text.trim_end_matches('\n').split('\n').collect();
    assert_eq!(lines.len(), 3, "one header plus two rows: {text}");
    let header: Value = meclaw_core::serde_json::from_str(lines[0]).unwrap();
    for col in ["id", "claim", "audience_set", "channel", "speaker"] {
        assert!(
            header["schema"].get(col).is_some(),
            "the seed header must declare {col}: {header}"
        );
    }
    let row: Value = meclaw_core::serde_json::from_str(lines[1]).unwrap();
    assert_eq!(row["audience_set"], "[\"member:alex\"]");
    assert_eq!(row["channel"], "room:one");
    assert_eq!(row["speaker"], "member:alex");
}
