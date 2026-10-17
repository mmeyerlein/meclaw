//! #1233 (T6-08) — a store takes seed rows on its own lane and answers with a
//! digest receipt.
//!
//! Until now `seed_rows` reached a RUNNING store by the door opening a second
//! connection into the store's `cell.db` (`mutation::seed_rows::apply_entries`).
//! Cell boundaries are hard (no exceptions, the way back is the message path), so
//! the rows now travel as a message: a top-level `seed_rows` slot, sent by the
//! mutation door, written by the store on its own connection with the same
//! counting and idempotence the door used, and answered with a receipt that
//! names what landed — inserted, already present, and a digest over the rows.
//!
//! The assertions read the store's own database and its receipt: a lane that
//! answered but wrote nothing would pass an outcome-only check.

use meclaw_cells::store::{StoreCell, StoreParams};
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use tokio::sync::mpsc;

const STORE_PATH: &str = "/keeper";

/// The sender the colony stamps on a message its mutation door sends.
const DOOR: &str = "/colony/mutations";

fn store() -> StoreCell {
    let raw = json!({"schema": {"policy": {
        "rule_id": "text", "subject": "text", "verdict": "text", "priority": "int"
    }}});
    StoreCell::new(StoreParams::parse(&raw).expect("params parse"))
}

/// A sink stamped with the store's own path — exactly what `cell_task` builds.
fn sink_pair() -> (OutputSink, mpsc::Receiver<CellEmission>) {
    let (otx, orx) = mpsc::channel(8);
    let sink = OutputSink::new(
        otx,
        Path::new(STORE_PATH),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        Headers::new(),
        None,
    );
    (sink, orx)
}

fn seed_msg(rows: Value) -> Message {
    MessageBuilder::new(Path::new(STORE_PATH))
        .reply_to(Path::new(DOOR))
        .body(Body::Inline(json!({"seed_rows": {
            "id": "m-1", "table": "policy", "rows": rows
        }})))
        .build()
}

async fn run(cell: &mut StoreCell, db: &mut DbConn, msg: Message) -> Value {
    let (sink, mut orx) = sink_pair();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    orx.recv()
        .await
        .map(|em| em.content)
        .expect("the seed lane answers with a receipt")
}

async fn fresh_db() -> (tempfile::TempDir, DbConn) {
    let td = tempfile::TempDir::new().expect("tempdir");
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db"))
        .expect("cell.db");
    (td, DbConn::wrap(conn, None))
}

fn row(rule_id: &str) -> Value {
    json!({"rule_id": rule_id, "subject": "*", "verdict": "allow", "priority": 100})
}

/// The store writes the rows on its own connection and answers a receipt; the
/// same rows a second time are counted, not written, and carry the same digest.
#[tokio::test]
async fn the_store_takes_seed_rows_on_its_lane_and_answers_a_digest_receipt() {
    let mut cell = store();
    let (_td, mut db) = fresh_db().await;

    let first = run(&mut cell, &mut db, seed_msg(json!([row("r1")]))).await;
    let receipt = &first["seed_receipt"];
    assert_eq!(
        receipt["id"],
        json!("m-1"),
        "the receipt names the order: {first}"
    );
    assert_eq!(receipt["table"], json!("policy"), "{first}");
    assert_eq!(receipt["inserted"], json!(1), "{first}");
    assert_eq!(receipt["already_present"], json!(0), "{first}");
    let digest = receipt["digest"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        digest.len(),
        64,
        "a sha256 hex digest over the rows: {first}"
    );

    let second = run(&mut cell, &mut db, seed_msg(json!([row("r1")]))).await;
    assert_eq!(second["seed_receipt"]["inserted"], json!(0), "{second}");
    assert_eq!(
        second["seed_receipt"]["already_present"],
        json!(1),
        "{second}"
    );
    assert_eq!(
        second["seed_receipt"]["digest"],
        json!(digest),
        "the same rows carry the same digest"
    );

    let count: i64 = db
        .call(|c| c.query_row("SELECT COUNT(*) FROM policy", [], |r| r.get(0)))
        .await
        .expect("the store stood its declared table");
    assert_eq!(count, 1, "re-seeding the same row is a no-op");
}
