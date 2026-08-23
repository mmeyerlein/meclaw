//! GH #295 — N ops in, ONE reply with N results out.
//!
//! A caller that needs three reads (the memory-hive's three tier-0 recall legs,
//! say) had to send three messages and wait three round trips. It may now send
//! one message carrying N `tool_call` turns; the store answers with one message
//! carrying N `tool_result` turns **in call order**.
//!
//! **Where the per-leg metadata lives.** Not in the turn. `$defs/TurnObject` in
//! `crates/meclaw-core/schemas/ubf-body.json` is `additionalProperties: false`,
//! and the colony validates every emission against it (`colony.rs`, debug
//! builds) — a turn carrying extra keys dead-letters the whole reply as
//! `InvalidUbfBody`. So each leg's `operation`, `rows_affected`, `duration_ms`
//! and (on failure) `error_code` travel in a store-specific TOP-LEVEL body slot
//! `results[]`, which the UBF schema explicitly permits ("Cell-specific
//! top-level slots are allowed", `additionalProperties: true`). Turn and result
//! are correlated by `tool_call_id`.
//!
//! **The envelope** says `operation: "bundle"`, the summed `rows_affected`, the
//! total `duration_ms` and **`bundle_errors`**: the count of legs carrying an
//! `error_code` (project ruling 2026-08-22, option C). That key is ALWAYS on a
//! bundle reply — `0` means checked-and-clean — and NEVER on a single-op reply,
//! which stays byte-identical to what it is today. The header's own
//! `error_code` keeps its hard meaning: the WHOLE reply is a refusal and
//! carries no payload. It never signals partial failure.
//!
//! **What is not promised:** the ops run sequentially over the one connection.
//! The bundle is not a transaction and not a dependent chain; beyond "results
//! in call order" there is nothing to lean on.

use meclaw_cells::store::{StoreCell, StoreCellFactory, StoreParams};
use meclaw_colony::stateful_cell::StatefulCell;
use meclaw_colony::{CellFactory, ColonyMsg, DbConn, MutationOutcome};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, CellEmission, Headers, Message, MessageBuilder, OutputSink, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Generous failure-marker timeout (CONTRIBUTING.md 30 s convention).
const RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// The store lives at `/aff/store`, so its owning write scope is `/aff` — the
/// same shape `gh132_store_internal_write_surface.rs` drives.
const STORE_PATH: &str = "/aff/store";

/// The `store` cell driven directly, as every other store suite drives it.
struct StoreRig {
    cell: StoreCell,
    db: DbConn,
    sink: OutputSink,
    rx: mpsc::Receiver<CellEmission>,
}

impl StoreRig {
    fn with(conn: rusqlite::Connection, raw_params: Value) -> Self {
        let timeout = raw_params
            .get("query_timeout_ms")
            .and_then(|v| v.as_u64())
            .map(Duration::from_millis);
        let db = DbConn::wrap(conn, timeout);
        let cell = StoreCell::new(StoreParams::parse(&raw_params).expect("params parse"));
        let (otx, rx) = mpsc::channel(16);
        let sink = OutputSink::new(
            otx,
            Path::new(STORE_PATH),
            Uuid::now_v7(),
            Uuid::now_v7(),
            64,
            Headers::new(),
            None,
        );
        Self { cell, db, sink, rx }
    }

    /// Send one message carrying the given `messages[]` array and read the one
    /// reply it produces.
    ///
    /// Every reply that leaves this rig is validated against the UBF schema
    /// exactly as the colony validates it — header split off, the rest checked.
    /// Driving `handle` directly is otherwise blind to the very rule that made
    /// the first version of this feature dead-letter (`InvalidUbfBody`); one
    /// colony-level test below proves the real path, this line keeps every
    /// other case in this file honest for free.
    async fn send(&mut self, turns: Value, sender: &str) -> Value {
        let msg = MessageBuilder::new(Path::new(STORE_PATH))
            .body(Body::Inline(json!({ "messages": turns })))
            .reply_to(Path::new(sender))
            .build();
        self.cell.handle(msg, &self.sink, &mut self.db).await;
        let content = tokio::time::timeout(RECV_TIMEOUT, self.rx.recv())
            .await
            .expect("no emission within 30s")
            .expect("channel open")
            .content;
        assert_ubf(&content);
        content
    }

    /// A bundle from a sender inside the write scope.
    async fn bundle(&mut self, turns: Value) -> Value {
        self.send(turns, "/aff/caller").await
    }
}

/// The colony's own check, applied to an emission: strip the `header` block
/// (that becomes message headers, not body) and validate what remains.
fn assert_ubf(content: &Value) {
    let mut body = content.clone();
    if let Value::Object(map) = &mut body {
        map.remove("header");
    }
    if let Err(errors) = meclaw_core::validate_ubf_body(&body) {
        panic!(
            "the store emitted a body the colony would dead-letter as InvalidUbfBody: {errors}\nbody: {body}"
        );
    }
}

/// One `tool_call` turn: the args go on the wire as the turn's `text`, exactly
/// as an `llm` cell writes them.
fn call(id: &str, args: Value) -> Value {
    json!({"origin":"assistant","type":"tool_call","text": args.to_string(), "id": id})
}

/// A plain text turn — what the `llm` cell puts BESIDE its tool_calls when the
/// model wrote prose along with them.
fn text_turn(text: &str) -> Value {
    json!({"origin":"assistant","type":"text","text": text})
}

fn select(table: &str) -> Value {
    json!({"operation":"select","table":table,"columns":["x"]})
}

fn schema_only() -> Value {
    json!({"schema": {"items": {"id": "int", "name": "text"}}})
}

/// Three tables holding 1, 2 and 3 rows — so a reply that reorders the legs or
/// drops one is visible in `rows_affected` alone.
fn abc_conn() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE a (x INTEGER); INSERT INTO a(x) VALUES (1);
         CREATE TABLE b (x INTEGER); INSERT INTO b(x) VALUES (1),(2);
         CREATE TABLE c (x INTEGER); INSERT INTO c(x) VALUES (1),(2),(3);",
    )
    .unwrap();
    conn
}

/// The `tool_result` turns of a reply.
fn turns(out: &Value) -> &Vec<Value> {
    out["messages"]
        .as_array()
        .unwrap_or_else(|| panic!("reply carries no messages array: {out}"))
}

/// The per-leg metadata entries of a bundle reply.
fn results(out: &Value) -> &Vec<Value> {
    out["results"]
        .as_array()
        .unwrap_or_else(|| panic!("bundle reply carries no results array: {out}"))
}

// ---------------------------------------------------------------------------

/// Three ops, one message back, results in call order — each leg carrying its
/// own `tool_call_id`, `operation` and `rows_affected`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_ops_come_back_in_one_message_in_order() {
    let mut r = StoreRig::with(abc_conn(), schema_only());

    let out = r
        .bundle(json!([
            call("leg-a", select("a")),
            call("leg-b", select("b")),
            call("leg-c", select("c")),
        ]))
        .await;

    // One message, three turns, three results.
    let ts = turns(&out);
    let rs = results(&out);
    assert_eq!(ts.len(), 3, "three ops must answer in three turns: {out}");
    assert_eq!(rs.len(), 3, "…and in three result entries: {out}");

    // Order and identity: the ids come back as they were written, and the two
    // arrays are correlated by them.
    let ids: Vec<&str> = ts.iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["leg-a", "leg-b", "leg-c"], "call order: {out}");
    let rids: Vec<&str> = rs
        .iter()
        .map(|e| e["tool_call_id"].as_str().unwrap())
        .collect();
    assert_eq!(rids, ids, "results correlate to turns by id: {out}");

    for t in ts {
        assert_eq!(t["type"], "tool_result", "every turn is a tool_result: {t}");
    }
    for e in rs {
        assert_eq!(e["operation"], "select", "each leg names its own op: {e}");
        assert!(
            e["duration_ms"].as_i64().is_some(),
            "each leg carries its own duration_ms: {e}"
        );
        assert!(
            e.get("error_code").is_none(),
            "a green leg has no code: {e}"
        );
    }
    let rows: Vec<i64> = rs
        .iter()
        .map(|e| e["rows_affected"].as_i64().unwrap())
        .collect();
    assert_eq!(rows, [1, 2, 3], "each leg carries its own row count: {out}");

    // The envelope: the bundle names itself, sums the rows, totals the time.
    assert_eq!(out["header"]["operation"], "bundle", "{out}");
    assert_eq!(out["header"]["rows_affected"], 6, "summed rows: {out}");
    assert!(
        out["header"]["duration_ms"].as_i64().is_some(),
        "total duration: {out}"
    );
    assert!(
        out["header"].get("error_code").is_none(),
        "a bundle without refusals is not a refusal: {out}"
    );
}

/// A failing op refuses only itself: its siblings still return their rows, its
/// own result carries the `error_code`, and the envelope says how many did.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_op_does_not_stop_its_siblings() {
    let mut r = StoreRig::with(abc_conn(), schema_only());

    let out = r
        .bundle(json!([
            call("leg-a", select("a")),
            call("leg-gone", select("does_not_exist")),
            call("leg-c", select("c")),
        ]))
        .await;

    let ts = turns(&out);
    let rs = results(&out);
    assert_eq!(
        ts.len(),
        3,
        "the failing leg does not swallow the rest: {out}"
    );
    assert_eq!(rs[1]["error_code"], "unknown_table", "leg 2 failed: {out}");
    assert_eq!(
        rs[1]["operation"], "select",
        "and still names its op: {out}"
    );
    assert_eq!(rs[1]["rows_affected"], 0, "{out}");
    assert_eq!(rs[1]["tool_call_id"], "leg-gone", "{out}");

    // The siblings ran and returned rows.
    assert!(rs[0].get("error_code").is_none(), "{out}");
    assert_eq!(rs[0]["rows_affected"], 1, "{out}");
    assert!(rs[2].get("error_code").is_none(), "{out}");
    assert_eq!(rs[2]["rows_affected"], 3, "{out}");

    // The envelope signal (project ruling 2026-08-22, option C): one header read tells a
    // consumer that this bundle is poisoned.
    assert_eq!(
        out["header"]["bundle_errors"], 1,
        "the header counts the refused legs: {out}"
    );
    // …and the header's own error_code stays reserved for a whole-message
    // refusal, which this is not.
    assert!(
        out["header"].get("error_code").is_none(),
        "a partial failure is not a whole-message refusal: {out}"
    );
    assert_eq!(out["header"]["operation"], "bundle", "{out}");
    assert_eq!(out["header"]["rows_affected"], 4, "1 + 0 + 3: {out}");
}

/// A clean bundle says so explicitly: `bundle_errors: 0` is PRESENT, so a
/// consumer can tell "checked and clean" from "nobody stamped it".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clean_bundle_says_bundle_errors_zero() {
    let mut r = StoreRig::with(abc_conn(), schema_only());

    let out = r
        .bundle(json!([
            call("leg-a", select("a")),
            call("leg-b", select("b")),
            call("leg-c", select("c")),
        ]))
        .await;

    let h = &out["header"];
    assert!(
        h.get("bundle_errors").is_some(),
        "a clean bundle stamps the key rather than omitting it: {out}"
    );
    assert_eq!(h["bundle_errors"], 0, "{out}");
}

/// N == 1 is byte-identical to what the store emits today — headers included,
/// and WITHOUT `bundle_errors` and WITHOUT `results`. The expectation below was
/// recorded from the implementation as it stood before this change.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_single_tool_call_is_byte_identical() {
    let mut r = StoreRig::with(abc_conn(), schema_only());

    let out = r.bundle(json!([call("only", select("b"))])).await;

    assert_eq!(recorded(&out), single_op_expectation("only", 2, "b"));
    assert!(
        out["header"].get("bundle_errors").is_none(),
        "bundle_errors never appears on a single-op reply: {out}"
    );
    assert!(
        out.get("results").is_none(),
        "results[] never appears on a single-op reply: {out}"
    );
}

/// I1 — the dispatch counts `tool_call` TURNS, not `messages.len()`.
///
/// The `llm` cell emits a mixed `[tool_call, text]` body today
/// (`llm/translate.rs` + `llm/output.rs`), and today's store reads `messages[0]`
/// and answers the one call it finds. That body has TWO entries in `messages[]`
/// but ONE call, so a dispatch branching on `messages.len()` would turn it into
/// a bundle and change a shape that ships. This test goes red the moment
/// `tool_call_turn_count` is simplified back to a length check.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mixed_single_call_body_takes_the_single_path() {
    let mut r = StoreRig::with(abc_conn(), schema_only());

    let out = r
        .bundle(json!([
            call("only", select("b")),
            text_turn("I looked that up for you."),
        ]))
        .await;

    assert_eq!(
        recorded(&out),
        single_op_expectation("only", 2, "b"),
        "one call beside prose is ONE call, answered in today's shape: {out}"
    );
    assert!(out.get("results").is_none(), "not a bundle: {out}");
    assert!(
        out["header"].get("bundle_errors").is_none(),
        "not a bundle: {out}"
    );
}

/// I2 — two calls beside prose are a bundle of TWO, and the prose is ignored.
///
/// `llm/translate.rs` really produces `[tool_call, tool_call, text]`, and the
/// store used to drop every call after the first — the defect this issue fixes.
/// Refusing such a body instead would have replaced a silent drop with a hard
/// refusal on a shape that ships, so the parser skips what is not a `tool_call`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_text_turn_beside_two_calls_is_ignored() {
    let mut r = StoreRig::with(abc_conn(), schema_only());

    let out = r
        .bundle(json!([
            call("leg-a", select("a")),
            call("leg-c", select("c")),
            text_turn("Both of those are in front of you now."),
        ]))
        .await;

    let ts = turns(&out);
    let rs = results(&out);
    assert_eq!(
        ts.len(),
        2,
        "two calls, two turns — the text is not one: {out}"
    );
    assert_eq!(rs.len(), 2, "{out}");
    let ids: Vec<&str> = rs
        .iter()
        .map(|e| e["tool_call_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["leg-a", "leg-c"], "{out}");
    assert_eq!(out["header"]["rows_affected"], 4, "1 + 3: {out}");
    assert_eq!(out["header"]["bundle_errors"], 0, "{out}");
}

/// The write surface (GH #132) is checked per op: the out-of-scope write is
/// refused in its own leg, the read beside it still returns its rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_denied_op_refuses_only_itself() {
    let conn = abc_conn();
    conn.execute("CREATE TABLE items (id INTEGER, name TEXT)", [])
        .unwrap();
    let mut params = schema_only();
    params["write_surface"] = json!("internal");
    let mut r = StoreRig::with(conn, params);

    // `/outside` is not inside `/aff`: the write is refused before the DB sees
    // it, the read is unaffected.
    let out = r
        .send(
            json!([
                call("leg-read", select("a")),
                call(
                    "leg-write",
                    json!({"operation":"insert","table":"items",
                           "row":{"id":1,"name":"x"}})
                ),
            ]),
            "/outside",
        )
        .await;

    let rs = results(&out);
    assert_eq!(rs.len(), 2, "{out}");
    assert!(rs[0].get("error_code").is_none(), "the read passed: {out}");
    assert_eq!(rs[0]["rows_affected"], 1, "{out}");
    assert_eq!(rs[1]["error_code"], "write_denied", "{out}");
    assert_eq!(rs[1]["operation"], "insert", "{out}");
    assert_eq!(out["header"]["bundle_errors"], 1, "{out}");
    assert!(out["header"].get("error_code").is_none(), "{out}");

    // The refused write touched nothing.
    let left: i64 =
        r.db.call(|c| {
            c.query_row("SELECT count(*) FROM items", [], |r| r.get(0))
                .unwrap()
        })
        .await;
    assert_eq!(left, 0, "a refused write must not land");

    // N == 1 keeps today's whole-message refusal shape.
    let single = r
        .send(
            json!([call(
                "solo",
                json!({"operation":"insert","table":"items",
                       "row":{"id":2,"name":"y"}})
            )]),
            "/outside",
        )
        .await;
    assert_eq!(single["header"]["finish_reason"], "error", "{single}");
    assert_eq!(single["header"]["error_code"], "write_denied", "{single}");
    assert_eq!(single["header"]["operation"], "insert", "{single}");
    assert!(single["header"].get("bundle_errors").is_none(), "{single}");
    assert!(single.get("results").is_none(), "{single}");
}

/// A timed-out op reports in its own result: the interrupt is that leg's
/// outcome, not the bundle's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_timed_out_op_reports_in_its_own_result() {
    let conn = abc_conn();
    // 400k rows: a full projection of this table outlives the budget below by
    // orders of magnitude, while the one-row selects beside it finish in
    // microseconds. The discriminator is deliberately wide (25 ms, not 1 ms)
    // so that scheduling jitter on a loaded test machine cannot make the GREEN
    // legs trip — the timing claim is "one query is enormous", not "one query
    // is slightly slower".
    conn.execute_batch(
        "CREATE TABLE big (x INTEGER);
         INSERT INTO big(x)
           WITH RECURSIVE r(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM r WHERE x < 400000)
           SELECT x FROM r;",
    )
    .unwrap();
    let mut params = schema_only();
    params["query_timeout_ms"] = json!(25);
    let mut r = StoreRig::with(conn, params);

    let out = r
        .bundle(json!([
            call("leg-a", select("a")),
            call("leg-big", select("big")),
            call("leg-c", select("c")),
        ]))
        .await;

    let rs = results(&out);
    assert_eq!(rs.len(), 3, "{out}");
    assert_eq!(rs[1]["error_code"], "query_timeout", "{out}");
    assert_eq!(rs[1]["operation"], "select", "{out}");
    assert!(rs[0].get("error_code").is_none(), "sibling ran: {out}");
    assert_eq!(rs[0]["rows_affected"], 1, "{out}");
    assert!(rs[2].get("error_code").is_none(), "sibling ran: {out}");
    assert_eq!(rs[2]["rows_affected"], 3, "{out}");
    assert_eq!(out["header"]["bundle_errors"], 1, "{out}");
    assert!(out["header"].get("error_code").is_none(), "{out}");

    // N == 1 keeps today's `emit_query_timeout` message.
    let single = r.bundle(json!([call("solo", select("big"))])).await;
    assert_eq!(single["header"]["finish_reason"], "error", "{single}");
    assert_eq!(single["header"]["error_code"], "query_timeout", "{single}");
    assert_eq!(single["header"]["operation"], "select", "{single}");
    assert!(single["header"].get("bundle_errors").is_none(), "{single}");
}

// ---- the single-op expectation, recorded before the change ----------------

/// A reply with the wall-clock `duration_ms` pinned to a fixed value: its
/// presence and type are asserted here, so the rest of the document can be
/// compared byte for byte.
fn recorded(out: &Value) -> Value {
    let d = out["header"]["duration_ms"]
        .as_i64()
        .unwrap_or_else(|| panic!("duration_ms must be an integer: {out}"));
    assert!(d >= 0, "{out}");
    let mut recorded = out.clone();
    recorded["header"]["duration_ms"] = json!(0);
    recorded
}

/// What a single-op `select` reply looked like BEFORE GH #295, recorded from
/// that implementation. Nothing about it may move.
fn single_op_expectation(id: &str, rows: i64, table: &str) -> Value {
    let payload: Vec<Value> = (1..=rows).map(|x| json!({"x": x})).collect();
    assert_eq!(table.len(), 1, "the abc fixture tables are single letters");
    json!({
        "header": {
            "operation": "select",
            "rows_affected": rows,
            "duration_ms": 0
        },
        "messages": [{
            "origin": "tool",
            "type": "tool_result",
            "text": meclaw_core::serde_json::to_string(&payload).unwrap(),
            "id": id
        }]
    })
}

// ---- C1: the same bundle through a REAL colony ----------------------------

/// The node name, and therefore the template name and the cell directory.
const NODE: &str = "keeper";

/// A store holding one seeded table. No write surface, no canonical bindings —
/// the point of this fixture is the WIRE, not the ops.
fn store_config() -> Value {
    json!({
        "cell": {"type": "store"},
        "params": {"schema": {"notes": {"x": "int"}}},
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    })
}

/// Two rows, so a bundle leg has something to bring back.
const NOTES_SEED: &str = concat!(
    r#"{"schema": {"x": "int"}}"#,
    "\n",
    r#"{"x": 1}"#,
    "\n",
    r#"{"x": 2}"#,
    "\n"
);

/// A bundle reply is not a shape a rig can vouch for: `handle` hands its
/// emission to the colony, and the colony validates every emission against the
/// UBF schema before it routes it (debug builds) — an invalid body never
/// becomes a message at all, it becomes an `InvalidUbfBody` dead-letter that no
/// caller ever sees. So the reply has to be caught on the far side of that
/// check, in a real colony, with the real store factory, over a real edge.
///
/// This is the test that would have caught the first version of this feature,
/// which put the per-leg metadata into the turns and therefore dead-lettered
/// every single bundle it ever produced while eleven rig-driven tests stayed
/// green.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_bundle_survives_a_real_colony() {
    let td = tempfile::TempDir::new().unwrap();
    let factory: Arc<dyn CellFactory> = Arc::new(StoreCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("store".to_string(), factory)]);
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    // The template, and a colony that knows about it.
    let templates_root = td.path().join("templates");
    let tpl = templates_root.join(NODE);
    std::fs::create_dir_all(tpl.join("seed")).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{NODE}"}}"#)).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        meclaw_core::serde_json::to_string(&store_config()).unwrap(),
    )
    .unwrap();
    std::fs::write(tpl.join("seed/notes.jsonl"), NOTES_SEED).unwrap();
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root,
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap();

    // Grow the store and give its answers somewhere to go.
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
    h.add_edge(
        Uuid::now_v7(),
        Path::new(&format!("/{NODE}")),
        Path::new("/sink"),
    )
    .await;

    // Three legs, one of them against a table that does not exist.
    let msg = MessageBuilder::new(Path::new(&format!("/{NODE}")))
        .reply_to(Path::new("/sink"))
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(json!({"messages": [
            call("leg-1", select("notes")),
            call("leg-gone", select("nowhere")),
            call("leg-2", select("notes")),
        ]})))
        .build();
    h.send(msg).await;

    // POSITIVE receipt: the reply ARRIVED. An emission the colony refuses never
    // reaches a mailbox, so this line alone proves it passed validation.
    let reply = tokio::time::timeout(RECV_TIMEOUT, sink_rx.recv())
        .await
        .expect("no bundle reply within 30s — the emission was dead-lettered")
        .expect("sink channel closed");

    let body = match &reply.body {
        Body::Inline(v) => v.clone(),
        Body::Blob(_) => panic!("inline expected"),
    };
    let ts = body["messages"].as_array().expect("messages array");
    let rs = body["results"].as_array().expect("results array");
    assert_eq!(ts.len(), 3, "three legs answered: {body}");
    assert_eq!(rs.len(), 3, "{body}");
    assert_eq!(
        rs[0]["rows_affected"], 2,
        "the seeded rows came back: {body}"
    );
    assert_eq!(rs[1]["error_code"], "unknown_table", "{body}");
    assert_eq!(rs[2]["rows_affected"], 2, "{body}");

    // The envelope survived the header split.
    let hop = &reply.headers.hop;
    assert_eq!(
        hop.get("operation").and_then(|v| v.as_str()),
        Some("bundle"),
        "hop: {hop:?}"
    );
    assert_eq!(
        hop.get("bundle_errors").and_then(|v| v.as_i64()),
        Some(1),
        "hop: {hop:?}"
    );
    assert_eq!(
        hop.get("rows_affected").and_then(|v| v.as_i64()),
        Some(4),
        "hop: {hop:?}"
    );

    // And nothing was dead-lettered on the way.
    let dls = h.drain_dead_letters().await;
    assert!(
        dls.is_empty(),
        "a bundle must not dead-letter: {:?}",
        dls.iter().map(|d| d.reason.as_code()).collect::<Vec<_>>()
    );
    h.shutdown().await;
}
