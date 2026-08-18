//! GH #210 — a hive transit is a hop in its own right, and the trace says so.
//!
//! A hive has no mailbox and no cell task: it is a logical transit node that
//! reads its own out-edges and routes on. That makes it tempting to assume it
//! is invisible in a trace — the reasoning being that a trace names the cells
//! that handled a turn, and a hive handles nothing.
//!
//! It is the other way round. Every routable hop gets a `message_log` row, and
//! the `should_log` gate in `route_with_log` deliberately admits a hive target
//! next to a registry target, so the `parent_message_id` chain stays unbroken
//! across the transit. A hive therefore appears TWICE around one transit: once
//! as the `to_path` of the message that arrived at it, and once as the
//! `from_path` of the follow-up it forwarded. In a linear hop chain that reads
//! `… -> /box -> /box/relay -> /box -> /sink`.
//!
//! Two shipped READMEs quote such a chain (`examples/never-forgets`,
//! `examples/meclaw-os`) and a reader checks their own trace against it, so the
//! shape is documentation and needs a lock. What is pinned here is the shape,
//! not a row count: if a hive transit ever stopped being logged, the four-row
//! chain below would collapse to two and both READMEs would be fiction again.
//!
//! No model and no network: two hives, one echo cell, one capture cell.

use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::wait::wait_for_message_log_count;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn echo_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    r.insert(
        "echo".into(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    );
    r
}

fn write(root: &std::path::Path, rel: &str, body: &str) {
    let dir = root.join(rel);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.json"), body).unwrap();
}

/// A sealed hive `/box` with one cell in it, and a sink outside it.
///
/// The lane header is what keeps the two edges LEAVING `/box` apart: the hive's
/// own inbound edge and the root's onward edge both sit in the edge table under
/// `from_path = /box`, so without a condition a transit would fan out to both.
/// That is the shipped `talky` shape — the collector hive's `in_*` lanes and its
/// answer lanes share one from-path and are told apart by `hop.route`.
fn write_topology(root: &std::path::Path) {
    write(
        root,
        "main",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./box","to":"./sink","condition":"hop.lane == 'out'"}
        ]}}}"#,
    );
    write(
        root,
        "main/box",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./relay","condition":"hop.lane == 'in'"},
            {"from":"./relay","to":".","condition":"hop.lane == 'out'"}
        ]}}}"#,
    );
    // The relay answers TO THE HIVE, never past it — the boundary rule. Its
    // emitted header flips the lane, which is what lets the hive's next
    // out-edge evaluation pick the onward edge instead of routing back inside.
    write(
        root,
        "main/box/relay",
        r#"{"cell":{"type":"echo"},
            "params":{"emitted_target":"/box","emitted_header":{"key":"lane","value":"out"}},
            "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hive_is_named_twice_around_one_transit() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path());

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());

    // Anti-cascade: the terminal exists before anything is sent towards it.
    let (sink_tx, mut sink_rx) = mpsc::channel(8);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("the topology must boot");

    let probe = MessageBuilder::new(Path::new("/box"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"ping"}]}),
        ))
        .hop({
            let mut m = meclaw_core::serde_json::Map::new();
            m.insert("lane".into(), json!("in"));
            m
        })
        .ttl(16)
        .build();
    let trace_id = probe.trace_id.to_string();
    h.send(probe).await;

    // Positive receipt first: the run really did cross the hive twice. A log
    // assertion alone could not tell a completed chain from a stalled one.
    let got = tokio::time::timeout(Duration::from_secs(30), sink_rx.recv())
        .await
        .expect("the sink went quiet — the transit chain never completed")
        .expect("sink channel closed");
    assert_eq!(got.target, Path::new("/sink"));

    // The log is written by the writer task, so wait for the rows rather than
    // reading whatever happens to be flushed at this instant.
    let db = td.path().join("colony.db");
    wait_for_message_log_count(&db, &trace_id, 4, Duration::from_secs(30)).await;

    let conn = rusqlite::Connection::open(&db).expect("colony.db");
    let mut stmt = conn
        .prepare("SELECT from_path, to_path FROM message_log WHERE trace_id = ? ORDER BY rowid")
        .unwrap();
    let chain: Vec<(String, String)> = stmt
        .query_map([&trace_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    assert_eq!(
        chain,
        vec![
            ("@external".to_string(), "/box".to_string()),
            ("/box".to_string(), "/box/relay".to_string()),
            ("/box/relay".to_string(), "/box".to_string()),
            ("/box".to_string(), "/sink".to_string()),
        ],
        "a hive transit must be a hop of its own — arriving AT the hive and \
         leaving FROM it are two separate message-log rows, and the READMEs \
         that quote a hop chain depend on it"
    );

    h.shutdown().await;
}
