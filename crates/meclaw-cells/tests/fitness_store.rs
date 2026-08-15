//! Track T (#104) — fitness battery for the `store` cell's query-operator
//! surface, exactly as a coding agent drives it: `tool_call` turns in, one
//! `tool_result` per operation out.
//!
//! Deep coverage of FTS/canonical/traverse/similar lives in the store's own
//! suites (`store_query_layer.rs`, `p3_*`, `p4_*`); this battery pins the
//! BASIC operator contract an artifact log leans on (`docs/cell-types.md`
//! § store):
//!
//! - insert/select/update/delete round-trip with `rows_affected`;
//! - the `where` operator forms (`eq` shorthand, `neq`/`gt`/`in`/`is_null`/
//!   `or_null`) and `order_by`/`limit`/`distinct`;
//! - errors are NORMAL tool_results with `header.error_code` (E5) — an agent
//!   reads the code and repairs, the cell never crashes on bad SQL input.

use meclaw_cells::store::{StoreCell, StoreParams};
use meclaw_colony::DbConn;
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, MessageBuilder, OutputSink, Path, Uuid};
use tokio::sync::mpsc;

struct StoreRig {
    cell: StoreCell,
    db: DbConn,
    sink: OutputSink,
    rx: mpsc::Receiver<CellEmission>,
}

impl StoreRig {
    fn new() -> Self {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE artifacts (id INTEGER, kind TEXT, name TEXT, size INTEGER)",
            [],
        )
        .unwrap();
        let db = DbConn::wrap(conn, None);
        let cell = StoreCell::new(StoreParams {
            schema: Default::default(),
            query_timeout_ms: None,
            fts: Default::default(),
            canonical: Default::default(),
            write_surface: Default::default(),
        });
        let (otx, rx) = mpsc::channel(16);
        let sink = OutputSink::new(
            otx,
            Path::new("/store"),
            Uuid::now_v7(),
            Uuid::now_v7(),
            64,
            meclaw_core::Headers::new(),
            None,
        );
        Self { cell, db, sink, rx }
    }

    /// One store operation as a tool_call; returns the emission.
    async fn op(&mut self, args: Value) -> CellEmission {
        let msg = MessageBuilder::new(Path::new("/store"))
            .body(Body::Inline(json!({"messages":[{
                "origin": "assistant", "type": "tool_call",
                "text": args.to_string(), "id": "call-1"
            }]})))
            .reply_to(Path::new("/sink"))
            .build();
        self.cell.handle(msg, &self.sink, &mut self.db).await;
        tokio::time::timeout(std::time::Duration::from_secs(30), self.rx.recv())
            .await
            .expect("no emission within 30s")
            .expect("channel open")
    }

    async fn rows(&mut self, args: Value) -> Vec<Value> {
        let em = self.op(args).await;
        assert!(
            em.content["header"].get("error_code").is_none(),
            "expected rows, got error: {:?}",
            em.content
        );
        let text = em.content["messages"][0]["text"].as_str().unwrap();
        meclaw_core::serde_json::from_str::<Value>(text)
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }
}

async fn seed(r: &mut StoreRig) {
    for (id, kind, name, size) in [
        (1, "module", "calc.py", 120),
        (2, "test", "test_calc.py", 240),
        (3, "doc", "README.md", 800),
        (4, "module", "util.py", 60),
    ] {
        let em = r
            .op(json!({"operation": "insert", "table": "artifacts",
                       "row": {"id": id, "kind": kind, "name": name, "size": size}}))
            .await;
        assert_eq!(em.content["header"]["rows_affected"], 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn insert_select_update_delete_round_trip_with_rows_affected() {
    let mut r = StoreRig::new();
    seed(&mut r).await;

    // select is projection-mandatory; eq shorthand in where.
    let rows = r
        .rows(json!({"operation": "select", "table": "artifacts",
                     "columns": ["name"], "where": {"kind": "module"},
                     "order_by": [{"col": "name", "dir": "asc"}]}))
        .await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "calc.py");
    assert_eq!(rows[1]["name"], "util.py");

    // update with where; rows_affected counts changes.
    let em = r
        .op(json!({"operation": "update", "table": "artifacts",
                   "set": {"size": 130}, "where": {"name": "calc.py"}}))
        .await;
    assert_eq!(em.content["header"]["operation"], "update");
    assert_eq!(em.content["header"]["rows_affected"], 1);

    // delete with where.
    let em = r
        .op(json!({"operation": "delete", "table": "artifacts",
                   "where": {"kind": "doc"}}))
        .await;
    assert_eq!(em.content["header"]["rows_affected"], 1);

    let rows = r
        .rows(json!({"operation": "select", "table": "artifacts", "columns": ["id"]}))
        .await;
    assert_eq!(rows.len(), 3, "the doc row is gone, the rest remain");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_where_operator_forms_cover_the_comparison_surface() {
    let mut r = StoreRig::new();
    seed(&mut r).await;

    // neq
    let rows = r
        .rows(
            json!({"operation": "select", "table": "artifacts", "columns": ["name"],
                     "where": {"kind": {"neq": "module"}},
                     "order_by": [{"col": "name", "dir": "asc"}]}),
        )
        .await;
    assert_eq!(rows.len(), 2);

    // gt / lte band
    let rows = r
        .rows(
            json!({"operation": "select", "table": "artifacts", "columns": ["name"],
                     "where": {"size": {"gt": 100}},
                     "order_by": [{"col": "size", "dir": "desc"}]}),
        )
        .await;
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], "README.md", "descending order holds");

    // in
    let rows = r
        .rows(
            json!({"operation": "select", "table": "artifacts", "columns": ["id"],
                     "where": {"name": {"in": ["calc.py", "util.py"]}},
                     "order_by": [{"col": "id", "dir": "asc"}]}),
        )
        .await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["id"], 1);

    // is_null / or_null against a NULL cell
    let em = r
        .op(json!({"operation": "insert", "table": "artifacts",
                   "row": {"id": 5, "kind": "orphan", "name": "x"}}))
        .await;
    assert_eq!(em.content["header"]["rows_affected"], 1);
    let rows = r
        .rows(
            json!({"operation": "select", "table": "artifacts", "columns": ["id"],
                     "where": {"size": {"is_null": true}}}),
        )
        .await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], 5);
    let rows = r
        .rows(
            json!({"operation": "select", "table": "artifacts", "columns": ["id"],
                     "where": {"size": {"or_null": {"gt": 500}}},
                     "order_by": [{"col": "id", "dir": "asc"}]}),
        )
        .await;
    assert_eq!(rows.len(), 2, "the big row and the NULL row");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn limit_and_distinct_shape_the_answer_set() {
    let mut r = StoreRig::new();
    seed(&mut r).await;

    let rows = r
        .rows(
            json!({"operation": "select", "table": "artifacts", "columns": ["name"],
                     "order_by": [{"col": "size", "dir": "desc"}], "limit": 2}),
        )
        .await;
    assert_eq!(rows.len(), 2, "limit caps the rows");
    assert_eq!(rows[0]["name"], "README.md");

    // distinct dedupes the PROJECTION (GH #68): four rows, three kinds.
    let rows = r
        .rows(
            json!({"operation": "select", "table": "artifacts", "columns": ["kind"],
                     "distinct": true, "order_by": [{"col": "kind", "dir": "asc"}]}),
        )
        .await;
    assert_eq!(rows.len(), 3);
    let kinds: Vec<&str> = rows.iter().map(|x| x["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, vec!["doc", "module", "test"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn errors_are_normal_tool_results_with_typed_codes_never_crashes() {
    let mut r = StoreRig::new();
    seed(&mut r).await;

    // E5 family: SQL-level failures are NORMAL tool_results with
    // header.error_code and NO finish_reason error.
    let sql_cases = [
        (
            json!({"operation": "select", "table": "no_such_table", "columns": ["id"]}),
            "unknown_table",
        ),
        (
            json!({"operation": "select", "table": "artifacts", "columns": ["no_such_col"]}),
            "unknown_column",
        ),
    ];
    for (args, want) in sql_cases {
        let em = r.op(args.clone()).await;
        assert_eq!(
            em.content["header"]["error_code"].as_str(),
            Some(want),
            "args {args} must yield {want}: {:?}",
            em.content
        );
        assert_ne!(
            em.content["header"]["finish_reason"].as_str(),
            Some("error"),
            "SQL-level store errors are NORMAL tool_results (E5)"
        );
        assert_eq!(em.content["messages"][0]["type"], "tool_result");
    }

    // ARGS-level failures (malformed args, unknown operation, missing
    // projection, unknown operator) are `invalid_input` WITH
    // `finish_reason: "error"` — pinned as BUILT (store/cell.rs doc:
    // "Only parse failures (invalid_input) and query timeouts produce
    // Error-Messages"). GH #109 resolved the doc/code divergence in favour of
    // the CODE: cell-types.md § store (de + en) now describes these two
    // families explicitly. This pin is unchanged by that ruling — it was the
    // evidence for it.
    let args_cases = [
        json!({"operation": "select", "table": "artifacts"}), // projection mandatory
        json!({"operation": "select", "table": "artifacts", "columns": ["id"],
               "where": {"id": {"frobnicate": 1}}}), // unknown operator key
        json!({"operation": "warp", "table": "artifacts"}),   // unknown operation
    ];
    for args in args_cases {
        let em = r.op(args.clone()).await;
        assert_eq!(
            em.content["header"]["error_code"].as_str(),
            Some("invalid_input"),
            "args {args}: {:?}",
            em.content
        );
        assert_eq!(
            em.content["header"]["finish_reason"].as_str(),
            Some("error"),
            "args-level rejects are loud error messages (pinned behaviour)"
        );
    }

    // And the cell still answers after every rejection.
    let rows = r
        .rows(json!({"operation": "select", "table": "artifacts", "columns": ["id"], "limit": 1}))
        .await;
    assert_eq!(rows.len(), 1, "the cell survived the whole error battery");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_table_then_use_it_dynamically() {
    let mut r = StoreRig::new();

    let em = r
        .op(json!({"operation": "create_table", "table": "log",
                   "columns": {"step": "text", "detail": "json"}}))
        .await;
    assert_eq!(em.content["header"]["operation"], "create_table");
    assert!(em.content["header"].get("error_code").is_none());

    let em = r
        .op(json!({"operation": "insert", "table": "log",
                   "row": {"step": "red", "detail": {"exit": 1}}}))
        .await;
    assert_eq!(em.content["header"]["rows_affected"], 1);

    let rows = r
        .rows(json!({"operation": "select", "table": "log", "columns": ["step", "detail"]}))
        .await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["step"], "red");
}
