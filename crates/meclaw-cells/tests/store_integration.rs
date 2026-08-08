//! Phase-9 store integration: StoreCell via cell.handle() directly,
//! insert + select tool_call messages, verify tool_result emissions
//! with operation/rows_affected headers. SQL-Error path verifies
//! Brainstorm-E5: SQL-errors are NORMAL tool_results with `error_code`-
//! Header, NOT `finish_reason: "error"`.

use meclaw_cells::store::{StoreCell, StoreParams};
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, OutputSink, Path, Uuid};
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn insert_then_select_round_trip() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute("CREATE TABLE items (id INTEGER, name TEXT)", [])
        .unwrap();
    let mut db = DbConn::wrap(conn, None);
    let mut cell = StoreCell::new(StoreParams {
        schema: Default::default(),
        query_timeout_ms: None,
        fts: Default::default(),
    });

    let (otx, mut orx) = mpsc::channel(8);
    let sink = OutputSink::new(
        otx,
        Path::new("/store"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    );

    // 1) INSERT
    let ins_body = json!({"messages":[{
        "origin":"assistant","type":"tool_call",
        "text": r#"{"operation":"insert","table":"items","row":{"id":1,"name":"a"}}"#,
        "id":"call_ins"
    }]});
    let ins_msg = MessageBuilder::new(Path::new("/store"))
        .body(Body::Inline(ins_body))
        .reply_to(Path::new("/sink"))
        .build();
    cell.handle(ins_msg, &sink, &mut db).await;
    let em_ins = orx.recv().await.unwrap();
    assert_eq!(em_ins.content["header"]["operation"], "insert");
    assert_eq!(em_ins.content["header"]["rows_affected"], 1);
    assert!(
        em_ins.content["header"].get("error_code").is_none(),
        "happy-path insert must not set error_code"
    );

    // 2) SELECT
    let sel_body = json!({"messages":[{
        "origin":"assistant","type":"tool_call",
        "text": r#"{"operation":"select","table":"items","columns":["id","name"]}"#,
        "id":"call_sel"
    }]});
    let sel_msg = MessageBuilder::new(Path::new("/store"))
        .body(Body::Inline(sel_body))
        .reply_to(Path::new("/sink"))
        .build();
    cell.handle(sel_msg, &sink, &mut db).await;
    let em_sel = orx.recv().await.unwrap();
    assert_eq!(em_sel.content["header"]["operation"], "select");
    assert_eq!(em_sel.content["header"]["rows_affected"], 1);
    let turn = em_sel.content["messages"].as_array().unwrap();
    let text = turn[0]["text"].as_str().unwrap();
    let payload: Value = meclaw_core::serde_json::from_str(text).unwrap();
    assert_eq!(payload[0]["id"], 1);
    assert_eq!(payload[0]["name"], "a");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sql_error_emits_tool_result_with_error_code_header_not_finish_reason() {
    // Brainstorm E5 proof: SQL errors are a NORMAL tool_result with an error_code
    // header, NOT finish_reason:"error".
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    // Deliberately create NO table → the insert is rejected with sql_error, but as
    // a tool_result turn (not as an error message).
    let mut db = DbConn::wrap(conn, None);
    let mut cell = StoreCell::new(StoreParams {
        schema: Default::default(),
        query_timeout_ms: None,
        fts: Default::default(),
    });

    let (otx, mut orx) = mpsc::channel(8);
    let sink = OutputSink::new(
        otx,
        Path::new("/store"),
        Uuid::now_v7(),
        Uuid::now_v7(),
        64,
        meclaw_core::Headers::new(),
        None,
    );

    let body = json!({"messages":[{
        "origin":"assistant","type":"tool_call",
        "text": r#"{"operation":"insert","table":"missing","row":{"id":1}}"#,
        "id":"call_err"
    }]});
    let msg = MessageBuilder::new(Path::new("/store"))
        .body(Body::Inline(body))
        .reply_to(Path::new("/sink"))
        .build();
    cell.handle(msg, &sink, &mut db).await;
    let em = orx.recv().await.unwrap();

    // KEY ASSERT: error_code set in the header.
    // paket-7 D1: missing table now classifies as unknown_table (Zero-Drift-with-intent:
    // spec cell-types.md Z.71 always declared this code; now wired up).
    assert_eq!(em.content["header"]["error_code"], "unknown_table");
    // KEY ASSERT: no finish_reason:"error" — that would violate brainstorm E5.
    assert!(
        em.content["header"].get("finish_reason").is_none(),
        "SQL-Error must NOT set finish_reason:error per Brainstorm E5 — \
         only Connection-Open-Fail (Spawn-time) and query_timeout do."
    );
    // operation + rows_affected still set (a normal tool_result).
    assert_eq!(em.content["header"]["operation"], "insert");
    assert_eq!(em.content["header"]["rows_affected"], 0);
    // The turn is a tool_result.
    let turn = em.content["messages"].as_array().unwrap();
    assert_eq!(turn[0]["type"], "tool_result");
}
