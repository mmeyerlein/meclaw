//! GH #132 — `store` with an internal-only write surface.
//!
//! A domain-owning hive wants its store writable ONLY from inside the hive.
//! Until now that was a convention: any cell wired to the store's port could
//! write. `params.write_surface: "internal"` makes it a contract — a write op
//! whose sender lies outside the store's own parent scope is refused at the
//! cell, loudly, before it reaches the database. Reads stay free.
//!
//! The store cell is driven directly here (the same shape the other `store`
//! cell tests use), because what is under test is the cell's decision, and the
//! two inputs it decides on — the cell's own path and the message's `reply_to`
//! — are both substrate-stamped: `OutputSink::sender_path()` comes from the
//! registry, `reply_to` from the colony's outputs arm. Neither can be forged
//! from a message body.
//!
//! Proven here:
//!
//! 1. **OPT-IN**: without the declaration an outside write still commits —
//!    byte-identical behaviour for every store shipped so far.
//! 2. **WRITE FROM OUTSIDE**: refused with `finish_reason: "error"` and
//!    `error_code: "write_denied"`, and the table stays empty.
//! 3. **WRITE FROM INSIDE**: the sibling in the same hive writes normally.
//! 4. **READS STAY FREE**: the same outside sender may `select`.
//! 5. **NO SENDER**: a source message (no `reply_to`) is outside — fail-closed.
//! 6. **THE SWITCH IS NOT REACHABLE BY MESSAGE**: `write_surface` is immutable,
//!    so an outside caller cannot turn the boundary off (and a params update
//!    from outside is itself a refused write).

use meclaw_cells::store::{StoreCell, StoreParams};
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use tokio::sync::mpsc;

/// The store lives at `/aff/store`, so its owning scope is `/aff`.
const STORE_PATH: &str = "/aff/store";

fn store(write_surface: Option<&str>) -> StoreCell {
    let mut raw = json!({"schema": {"items": {"id": "int", "name": "text"}}});
    if let Some(ws) = write_surface {
        raw["write_surface"] = json!(ws);
    }
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

fn op_msg(sender: Option<&str>, args: Value) -> Message {
    let body = json!({"messages":[{
        "origin":"assistant","type":"tool_call",
        "text": args.to_string(),
        "id":"call_1"
    }]});
    let b = MessageBuilder::new(Path::new(STORE_PATH)).body(Body::Inline(body));
    match sender {
        Some(s) => b.reply_to(Path::new(s)).build(),
        None => b.build(),
    }
}

async fn run(cell: &mut StoreCell, db: &mut DbConn, msg: Message) -> Option<Value> {
    let (sink, mut orx) = sink_pair();
    cell.handle(msg, &sink, db).await;
    drop(sink);
    orx.recv().await.map(|em| em.content)
}

async fn fresh_db() -> (tempfile::TempDir, DbConn) {
    let td = tempfile::TempDir::new().unwrap();
    let conn = meclaw_colony::persist::open_or_create_cell_db(&td.path().join("cell.db")).unwrap();
    conn.execute("CREATE TABLE items (id INTEGER, name TEXT)", [])
        .unwrap();
    (td, DbConn::wrap(conn, None))
}

fn insert_args() -> Value {
    json!({"operation":"insert","table":"items","row":{"id":1,"name":"x"}})
}

async fn row_count(db: &mut DbConn) -> i64 {
    db.call(|c| {
        c.query_row("SELECT COUNT(*) FROM items", [], |r| r.get(0))
            .unwrap()
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_the_declaration_an_outside_write_still_commits() {
    // The OPT-IN half: no `write_surface` key at all.
    let (_td, mut db) = fresh_db().await;
    let mut cell = store(None);
    let content = run(
        &mut cell,
        &mut db,
        op_msg(Some("/elsewhere"), insert_args()),
    )
    .await
    .expect("an open store answers");
    assert_eq!(content["header"]["operation"], "insert");
    assert_eq!(content["header"]["rows_affected"], 1);
    assert_eq!(row_count(&mut db).await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_internal_surface_refuses_a_write_from_outside_the_owning_scope() {
    let (_td, mut db) = fresh_db().await;
    let mut cell = store(Some("internal"));
    let content = run(
        &mut cell,
        &mut db,
        op_msg(Some("/elsewhere"), insert_args()),
    )
    .await
    .expect("a refusal is an answer, never a drop");
    assert_eq!(content["header"]["finish_reason"], "error");
    assert_eq!(content["header"]["error_code"], "write_denied");
    let text = content["messages"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("/aff") && text.contains("/elsewhere"),
        "the refusal names the owning scope AND the sender: {text}"
    );
    assert_eq!(
        content["messages"][0]["id"], "call_1",
        "the refusal is correlatable inside a tool loop"
    );
    assert_eq!(row_count(&mut db).await, 0, "nothing reached the database");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_internal_surface_lets_the_owning_hive_write() {
    let (_td, mut db) = fresh_db().await;
    let mut cell = store(Some("internal"));
    // The gate cell of the same hive.
    let content = run(&mut cell, &mut db, op_msg(Some("/aff/gate"), insert_args()))
        .await
        .expect("the owner writes");
    assert_eq!(content["header"]["operation"], "insert");
    assert_eq!(content["header"]["rows_affected"], 1);
    assert_eq!(row_count(&mut db).await, 1);

    // The hive marker itself counts as inside, and so does a cell deeper in.
    for sender in ["/aff", "/aff/sub/deep"] {
        let content = run(&mut cell, &mut db, op_msg(Some(sender), insert_args()))
            .await
            .expect("inside the scope");
        assert_eq!(
            content["header"]["operation"], "insert",
            "sender {sender} is inside /aff"
        );
    }
    assert_eq!(row_count(&mut db).await, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_internal_surface_leaves_every_read_free() {
    let (_td, mut db) = fresh_db().await;
    let mut cell = store(Some("internal"));
    // Seed from inside so there is something to read.
    run(&mut cell, &mut db, op_msg(Some("/aff/gate"), insert_args())).await;

    for op in [
        json!({"operation":"select","table":"items","columns":["id","name"]}),
        json!({"operation":"select","table":"items","columns":["id"],"where":{"id":1}}),
    ] {
        let content = run(&mut cell, &mut db, op_msg(Some("/elsewhere"), op))
            .await
            .expect("a read from outside answers");
        assert_eq!(
            content["header"]["operation"], "select",
            "reads are never bounded by the write surface: {content}"
        );
        assert!(content["header"].get("error_code").is_none());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_without_a_sender_is_outside_the_scope() {
    // Fail-closed: a source message (HTTP ingress, event) was produced by no
    // edge inside the hive, so it is outside by definition.
    let (_td, mut db) = fresh_db().await;
    let mut cell = store(Some("internal"));
    let content = run(&mut cell, &mut db, op_msg(None, insert_args()))
        .await
        .expect("a refusal is an answer");
    assert_eq!(content["header"]["error_code"], "write_denied");
    assert_eq!(row_count(&mut db).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_boundary_cannot_be_switched_off_by_a_message() {
    let (_td, mut db) = fresh_db().await;
    let mut cell = store(Some("internal"));

    // From OUTSIDE: a params update is itself a write and is refused before the
    // merge, so not even a persisted overlay is left behind.
    let msg = MessageBuilder::new(Path::new(STORE_PATH))
        .body(Body::Inline(json!({"params": {"write_surface": "open"}})))
        .reply_to(Path::new("/elsewhere"))
        .build();
    let content = run(&mut cell, &mut db, msg)
        .await
        .expect("a refusal is an answer");
    assert_eq!(content["header"]["error_code"], "write_denied");

    // From INSIDE: the key is immutable, so even the owner cannot open the
    // surface at runtime — it is a config.json declaration, not a setting.
    let msg = MessageBuilder::new(Path::new(STORE_PATH))
        .body(Body::Inline(json!({"params": {"write_surface": "open"}})))
        .reply_to(Path::new("/aff/gate"))
        .build();
    let content = run(&mut cell, &mut db, msg)
        .await
        .expect("a rejected params update answers");
    assert_eq!(content["header"]["error_code"], "invalid_input");

    // And the boundary still holds afterwards.
    let content = run(
        &mut cell,
        &mut db,
        op_msg(Some("/elsewhere"), insert_args()),
    )
    .await
    .expect("still refusing");
    assert_eq!(content["header"]["error_code"], "write_denied");
    assert_eq!(row_count(&mut db).await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_write_op_is_bounded_and_every_read_op_is_not() {
    // The closed lists, walked once: whoever adds an op has to place it.
    let (_td, mut db) = fresh_db().await;
    let mut cell = store(Some("internal"));
    run(&mut cell, &mut db, op_msg(Some("/aff/gate"), insert_args())).await;

    let writes = [
        insert_args(),
        json!({"operation":"update","table":"items","set":{"name":"y"}}),
        json!({"operation":"delete","table":"items"}),
        json!({"operation":"create_table","table":"t2","columns":{"a":"int"}}),
    ];
    for args in writes {
        let op = args["operation"].as_str().unwrap().to_string();
        let content = run(&mut cell, &mut db, op_msg(Some("/elsewhere"), args))
            .await
            .expect("answer");
        assert_eq!(
            content["header"]["error_code"], "write_denied",
            "op '{op}' must be bounded by the write surface"
        );
    }
    assert_eq!(row_count(&mut db).await, 1, "only the inside write stands");

    // A read that the store answers with a plain result, from outside.
    let content = run(
        &mut cell,
        &mut db,
        op_msg(
            Some("/elsewhere"),
            json!({"operation":"select","table":"items","columns":["id"]}),
        ),
    )
    .await
    .expect("answer");
    assert_eq!(content["header"]["operation"], "select");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_declaration_is_a_closed_value_set() {
    assert!(StoreParams::parse(&json!({"schema":{"t":{"a":"int"}}})).is_ok());
    assert!(
        StoreParams::parse(&json!({"schema":{"t":{"a":"int"}},"write_surface":"open"})).is_ok()
    );
    assert!(
        StoreParams::parse(&json!({"schema":{"t":{"a":"int"}},"write_surface":"internal"})).is_ok()
    );
    for bad in [
        json!("private"),
        json!(true),
        json!(null),
        json!(["internal"]),
    ] {
        assert!(
            StoreParams::parse(&json!({"schema":{"t":{"a":"int"}},"write_surface":bad})).is_err(),
            "an unknown surface must never reach spawn as a silently-open store: {bad}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_open_store_serialises_exactly_as_before() {
    // The β overlay round-trips params through `to_value`. The default must not
    // appear in that form, or every pre-existing store would change shape.
    let p = StoreParams::parse(&json!({"schema":{"t":{"a":"int"}}})).unwrap();
    let v = meclaw_core::serde_json::to_value(&p).unwrap();
    assert!(
        v.get("write_surface").is_none(),
        "the default stays absent: {v}"
    );

    let p = StoreParams::parse(&json!({"schema":{"t":{"a":"int"}},"write_surface":"internal"}))
        .unwrap();
    let v = meclaw_core::serde_json::to_value(&p).unwrap();
    assert_eq!(
        v["write_surface"], "internal",
        "and the declaration survives"
    );
    // …and comes back through the same parse path (the overlay's merge base).
    assert_eq!(
        StoreParams::parse(&v).unwrap().write_surface,
        meclaw_cells::store::WriteSurface::Internal
    );
}
