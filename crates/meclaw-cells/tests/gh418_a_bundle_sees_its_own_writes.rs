//! GH #418 — a bundle sees its own writes, and of two parking bundles exactly
//! one reads a complete set.
//!
//! **WHAT IS BEING PINNED, AND WHY IT IS A PIN AND NOT A FIX.** Nothing in this
//! file changes behaviour. Every assertion below holds at the commit that
//! introduces it, and held since GH #295 built the bundle. It is written down
//! because the tier-1 recall chain of `templates/memory-hive` is about to stand
//! on it, and a property nobody asserts is a property somebody removes.
//!
//! The property: the `store` is a **stateful** cell (`docs/cell-types.md`,
//! concurrency note; `crates/meclaw-cells/src/store/cell.rs` implements
//! `StatefulCell`) — one task, one connection, one message at a time. A bundle
//! is ONE message, and `run_bundle` runs its ops in call order over that one
//! connection. Two consequences follow, and they are the two tests here:
//!
//! 1. A `select` at the END of a bundle sees the `insert`s in front of it.
//! 2. Two bundles of the form `[insert, select]` that park concurrently can
//!    never both read a complete set: whoever is served first reads only its
//!    own row, whoever is served second reads both. There is no interleaving in
//!    between, because there is no second connection to interleave on.
//!
//! That is exactly the election `update … set fired = 1 where fired = 0` plus
//! `rows_affected` used to buy, and it costs no message of its own. What the
//! bundle does NOT promise is unchanged: it is not a transaction (a failing leg
//! refuses only itself, GH #295) and not a dependent chain — only the ORDER is
//! guaranteed, and the order is all the election needs.

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
/// same shape every other store suite drives.
const STORE_PATH: &str = "/aff/store";

/// The node name of the colony-level fixture.
const NODE: &str = "keeper";

/// The parking table. Deliberately NOT `recall_scratch`: this file pins the
/// substrate, not the template. A test that spelled the hive's own table would
/// read as a statement about the hive and would have to move with it.
fn schema_only() -> Value {
    json!({"schema": {"park": {"leg": "text", "payload": "text"}}})
}

fn store_config() -> Value {
    json!({
        "cell": {"type": "store"},
        "params": {"schema": {"park": {"leg": "text", "payload": "text"}}},
        "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
    })
}

/// One `tool_call` turn: the args go on the wire as the turn's `text`, exactly
/// as an `llm` cell writes them.
fn call(id: &str, args: Value) -> Value {
    json!({"origin":"assistant","type":"tool_call","text": args.to_string(), "id": id})
}

fn insert(leg: &str) -> Value {
    json!({"operation": "insert", "table": "park",
           "row": {"leg": leg, "payload": "x"}})
}

fn read_back() -> Value {
    json!({"operation": "select", "table": "park", "columns": ["leg", "payload"]})
}

/// The legs a `select` turn came back with, read out of the turn's `text`.
fn legs_of(turn: &Value) -> Vec<String> {
    let rows: Value = meclaw_core::serde_json::from_str(turn["text"].as_str().unwrap_or("[]"))
        .unwrap_or_else(|e| panic!("select turn carries no row array: {turn} ({e})"));
    rows.as_array()
        .unwrap_or_else(|| panic!("select turn is not an array: {turn}"))
        .iter()
        .map(|r| r["leg"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The `store` cell driven directly, as every other store suite drives it.
struct StoreRig {
    cell: StoreCell,
    db: DbConn,
    sink: OutputSink,
    rx: mpsc::Receiver<CellEmission>,
}

impl StoreRig {
    fn with(conn: rusqlite::Connection, raw_params: Value) -> Self {
        let db = DbConn::wrap(conn, None);
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

    async fn bundle(&mut self, turns: Value) -> Value {
        let msg = MessageBuilder::new(Path::new(STORE_PATH))
            .body(Body::Inline(json!({ "messages": turns })))
            .reply_to(Path::new("/aff/caller"))
            .build();
        self.cell.handle(msg, &self.sink, &mut self.db).await;
        tokio::time::timeout(RECV_TIMEOUT, self.rx.recv())
            .await
            .expect("no emission within 30s")
            .expect("channel open")
            .content
    }
}

fn park_conn() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE park (leg TEXT, payload TEXT);")
        .unwrap();
    conn
}

// ---------------------------------------------------------------------------

/// A bundle is ONE message to a stateful cell, so its ops run in order over the
/// one connection: the trailing select sees the insert in front of it. This is
/// the whole election mechanism of GH #418 — without it the tier-1 fan-in needs
/// its guarded update back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_trailing_select_sees_the_insert_in_front_of_it() {
    let mut r = StoreRig::with(park_conn(), schema_only());

    let out = r
        .bundle(json!([
            call("c-ins", insert("a")),
            call("c-sel", read_back())
        ]))
        .await;

    let ts = out["messages"].as_array().expect("messages array");
    let rs = out["results"].as_array().expect("results array");
    assert_eq!(ts.len(), 2, "two ops, two turns: {out}");
    assert_eq!(rs[0]["rows_affected"], 1, "the insert wrote its row: {out}");
    // The point of the file, in one line: the select that ran AFTER the insert
    // returned the inserted row, in the same reply.
    assert_eq!(
        rs[1]["rows_affected"], 1,
        "the trailing select must see the insert in front of it: {out}"
    );
    assert_eq!(
        legs_of(&ts[1]),
        vec!["a".to_string()],
        "and it must carry the row itself, not only a count: {out}"
    );
}

/// Two concurrent parking bundles: exactly ONE of them reads a complete set.
/// That is the property the three guarded updates of the tier-1 path bought
/// with a round trip each.
///
/// Driven through a real colony rather than through the rig: the rig calls
/// `handle` itself and therefore proves ordering it imposed. Here the two
/// messages go into the store's mailbox back to back, and the serialisation is
/// the cell task's, not the test's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn of_two_parking_bundles_exactly_one_reads_the_complete_set() {
    let td = tempfile::TempDir::new().unwrap();
    let factory: Arc<dyn CellFactory> = Arc::new(StoreCellFactory);
    let h = ColonyHandle::new_with_factories_at(&td, vec![("store".to_string(), factory)]);
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    let templates_root = td.path().join("templates");
    let tpl = templates_root.join(NODE);
    std::fs::create_dir_all(&tpl).unwrap();
    std::fs::write(tpl.join("template.json"), format!(r#"{{"name":"{NODE}"}}"#)).unwrap();
    std::fs::write(
        tpl.join("config.json"),
        meclaw_core::serde_json::to_string(&store_config()).unwrap(),
    )
    .unwrap();
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
    assert!(
        matches!(ack_rx.await.unwrap(), MutationOutcome::Committed { .. }),
        "add_nodes of /{NODE} must commit"
    );
    h.add_edge(
        Uuid::now_v7(),
        Path::new(&format!("/{NODE}")),
        Path::new("/sink"),
    )
    .await;

    // Two hops park at the same time. Neither waits for the other; both ask the
    // same question in the same message they answer it with.
    for leg in ["a", "b"] {
        let msg = MessageBuilder::new(Path::new(&format!("/{NODE}")))
            .reply_to(Path::new("/sink"))
            .trace_id(Uuid::now_v7())
            .body(Body::Inline(json!({"messages": [
                call("c-ins", insert(leg)),
                call("c-sel", read_back()),
            ]})))
            .build();
        h.send(msg).await;
    }

    let mut complete = 0;
    let mut seen = 0;
    for _ in 0..2 {
        let reply = tokio::time::timeout(RECV_TIMEOUT, sink_rx.recv())
            .await
            .expect("no bundle reply within 30s")
            .expect("sink channel closed");
        let body = match &reply.body {
            Body::Inline(v) => v.clone(),
            Body::Blob(_) => panic!("inline expected"),
        };
        let ts = body["messages"].as_array().expect("messages array");
        let mut legs = legs_of(&ts[1]);
        legs.sort();
        seen += 1;
        if legs == vec!["a".to_string(), "b".to_string()] {
            complete += 1;
        }
    }
    assert_eq!(seen, 2, "both bundles must answer");
    assert_eq!(
        complete, 1,
        "exactly one of two parking bundles reads a complete set — \
         the election of GH #418 is that one"
    );

    let dls = h.drain_dead_letters().await;
    assert!(dls.is_empty(), "no dead letters: {dls:?}");
    h.shutdown().await;
}
