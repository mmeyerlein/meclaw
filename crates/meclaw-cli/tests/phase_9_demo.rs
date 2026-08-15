//! Phase-9 End-to-End-Demo.
//!
//! Two static chains, harness-sequenced (cell-types.md § store +
//! § code via multi-send):
//!
//!   /sink              (CaptureCell -- collects all tool_result receipts)
//!   /store             (StoreCell, params.schema.items = {id, name})
//!   /code/transform    (CodeCell, transform.py, multi_send_capable=true)
//!   /code/query        (CodeCell, query.py)
//!
//! Static edges (all conditionless):
//!   /code/transform -> /store     (every multi-send emission lands here)
//!   /code/query     -> /store
//!   /store          -> /sink
//!
//! Anti-cascade discipline: register /sink BEFORE bootstrap_from_filesystem
//! (phase-6.5 lesson).
//! Harness sequencing: wait for 2 insert receipts BEFORE probe 2 is sent.
//! Asserts positive and deep: plus a fresh-connection DB probe.

use meclaw_cli::factories::built_in_factories;
use meclaw_colony::bootstrap_from_filesystem;
use meclaw_core::{Body, MessageBuilder, Path, serde_json::Value};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phase_9_demo_code_to_store_round_trip() {
    let td = tempfile::TempDir::new().unwrap();

    // Skript-Pfade: CARGO_MANIFEST_DIR-relativ -> CWD-unabhaengig.
    let transform_py = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/phase_9/transform.py"
    );
    let query_py = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/phase_9/query.py"
    );

    // FS-Tree
    write(
        td.path(),
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"/code/transform","to":"/store"},
            {"from":"/code/query","to":"/store"},
            {"from":"/store","to":"/sink"}
        ]}}}"#,
    );
    write(
        td.path(),
        "main/store/config.json",
        r#"{"cell":{"type":"store"},"params":{
            "schema":{"items":{"id":"int","name":"text"}}
        },"contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        td.path(),
        "main/code/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    let transform_config = format!(
        r#"{{"cell":{{"type":"code"}},"params":{{
            "runner":"python3",
            "script_path":"{transform_py}",
            "external_timeout_ms":10000
        }},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}},"multi_send_capable":true}}}}"#
    );
    write(
        td.path(),
        "main/code/transform/config.json",
        &transform_config,
    );
    let query_config = format!(
        r#"{{"cell":{{"type":"code"}},"params":{{
            "runner":"python3",
            "script_path":"{query_py}",
            "external_timeout_ms":10000
        }},"contract":{{"version":"0.1.0","settings":{{}},"consumes":{{}}}}}}"#
    );
    write(td.path(), "main/code/query/config.json", &query_config);

    // Boot the colony with built_in_factories (contains store + code).
    let factories = built_in_factories();
    let factory_vec: Vec<(String, Arc<dyn meclaw_colony::CellFactory>)> = factories
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let h = ColonyHandle::new_with_factories_at(&td, factory_vec);

    // /sink VOR bootstrap registrieren (Anti-Cascade).
    let (sink_tx, mut sink_rx) = mpsc::channel::<meclaw_core::Message>(16);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &factories, &h.runtime())
        .await
        .expect("bootstrap must succeed");

    // === Kette 1 (Insert) ===
    // Probe an /code/transform: 2 Items -> Multi-Send -> 2 insert-tool_calls
    // -> 2 /store-Emissions an /sink.
    let probe1_body = meclaw_core::serde_json::json!({
        "items": [{"id": 1, "name": "a"}, {"id": 2, "name": "b"}]
    });
    let probe1 = MessageBuilder::new(Path::new("/code/transform"))
        .body(Body::Inline(probe1_body))
        .build();
    h.send(probe1).await;

    // Wait for 2 insert receipts BEFORE probe 2 is sent
    // (otherwise select may see fewer rows).
    let mut inserts: Vec<meclaw_core::Message> = Vec::new();
    for i in 0..2 {
        let m = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
            .await
            .unwrap_or_else(|_| panic!("insert receipt {i} timeout"))
            .expect("sink closed before 2 receipts");
        inserts.push(m);
    }
    // Verify both insert receipts.
    for (i, m) in inserts.iter().enumerate() {
        assert_eq!(m.headers.hop["operation"], "insert", "insert receipt #{i}");
        assert_eq!(m.headers.hop["rows_affected"], 1, "insert receipt #{i}");
        assert!(
            m.headers.hop.get("error_code").is_none(),
            "insert receipt #{i} must not have error_code (happy path)"
        );
    }

    // === Kette 2 (Select) ===
    // Probe to /code/query: ignores the input, builds 1 select tool_call
    // -> /store answers with all rows.
    let probe2 = MessageBuilder::new(Path::new("/code/query"))
        .body(Body::Inline(meclaw_core::serde_json::json!({})))
        .build();
    h.send(probe2).await;

    let select_receipt = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("select receipt timeout")
        .expect("sink closed before select");
    assert_eq!(select_receipt.headers.hop["operation"], "select");
    assert_eq!(select_receipt.headers.hop["rows_affected"], 2);

    // Select payload: the text slot = a JSON array with both items.
    let body_v = match &select_receipt.body {
        Body::Inline(v) => v,
        Body::Blob(_) => panic!("body must be Inline"),
    };
    let turn = body_v["messages"].as_array().expect("messages array");
    let text = turn[0]["text"].as_str().expect("text in turn");
    let payload: Value =
        meclaw_core::serde_json::from_str(text).expect("payload must be valid JSON");
    let rows = payload.as_array().expect("payload is array");
    assert_eq!(rows.len(), 2, "select payload must have 2 rows");

    // Sort for a deterministic comparison.
    let mut row_ids: Vec<i64> = rows.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    row_ids.sort();
    assert_eq!(row_ids, vec![1, 2]);
    let mut row_names: Vec<&str> = rows.iter().map(|r| r["name"].as_str().unwrap()).collect();
    row_names.sort();
    assert_eq!(row_names, vec!["a", "b"]);

    // === DB truth probe (positive and deep) ===
    // A fresh connection on store/cell.db -- independent of the receipts.
    let store_db = td.path().join("main").join("store").join("cell.db");
    assert!(store_db.exists(), "store cell.db at {}", store_db.display());
    let conn = rusqlite::Connection::open(&store_db).unwrap();
    let mut stmt = conn
        .prepare("SELECT id, name FROM items ORDER BY id")
        .unwrap();
    let db_rows: Vec<(i64, String)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        db_rows,
        vec![(1, "a".to_string()), (2, "b".to_string())],
        "DB wahrheit confirms exactly the 2 reported rows"
    );

    // Total: 3 sink receipts seen, nothing beyond that.
    let mut extra = 0;
    while sink_rx.try_recv().is_ok() {
        extra += 1;
    }
    assert_eq!(extra, 0, "exactly 3 receipts (2 insert + 1 select)");

    h.shutdown().await;
}
