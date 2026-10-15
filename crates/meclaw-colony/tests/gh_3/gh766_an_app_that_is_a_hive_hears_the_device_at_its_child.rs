//! GH #766 — an app that is a HIVE WITH CHILDREN hears a device cell at its
//! child, and it hears it only through the edge onto the hive.
//!
//! # The shape no test of this wave had
//!
//! Wave G proved the browser lane six times on a disposable colony whose app
//! was a FLAT node (`beweis/welle-g-grow.json`: `add_nodes {name: "voice2vision"}`
//! with no children of its own in the picture). On the candidate the same app
//! is a hive with `stage`, `show` and `board` inside it. Every proof therefore
//! ran on the one shape in which the question below cannot be asked.
//!
//! # The belief this file kills
//!
//! The wave's contract said the return edge is `./browser -> ./apps/voice2vision`
//! on `hop.route == 'page'` and that "`error`/`receipt` run upward" on their own.
//! Nothing runs upward. The colony's outputs arm matches a cell emission with
//! `apply_edges(&edges, &from, &merged_headers)` (`colony.rs`, the outputs arm)
//! — the SENDER and the HEADERS, and nothing else. `CellOutput::target` is not
//! read there at all; it is only copied into the dead-letter row as
//! `original_target`/`resolved_target`. That is Ruling A1, it is written down in
//! `meclaw-testing`'s echo factory (`factories/echo.rs`, GH #224, which renamed
//! `echo_to` for exactly this misreading), and it has one consequence the wave
//! read backwards:
//!
//! * an emission aimed at a child INSIDE a hive still arrives, as long as an
//!   edge of the emitting cell points at the HIVE — the aim is decoration;
//! * an emission on a route no out-edge carries arrives NOWHERE, however
//!   correctly it is aimed.
//!
//! # And the second half: why six runs could not see it
//!
//! The dead-lettered emission leaves NO row in `message_log`. Measured at the
//! twin on 2026-09-20 (wave G, g15): two turns, two `no_route` entries from the
//! browser cell, and `GET /colony/messages?from_path_prefix=<the cell>` empty
//! over the whole window. A marker that reads the message log cannot tell
//! "never emitted" from "emitted and dropped" — both are simply absent. So the
//! DLQ is asserted here beside the arrival, and `beweis/markers_g.py` grew the
//! same gate.
//!
//! No model, no network, no browser: two echo cells, one app hive, one capture
//! cell in it.

use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use meclaw_testing::topologies::phase_3a::CaptureCell;
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

/// The candidate's shape, small enough to read in one go.
///
/// ```text
/// /                  the member scope
/// ├── device         echo cell — stands in for the browser cell
/// ├── mute           echo cell — emits a route no out-edge carries
/// └── app            HIVE WITH CHILDREN — the form the disposable colony lacked
///     └── stage      capture cell — the child the emission is aimed at
/// ```
///
/// Both cells aim at `/app/stage`, a child INSIDE the hive, because that is
/// what the browser cell does: `emit_receipt` writes `msg.reply_to` into the
/// emission's target, and the message that asked came from the stage. An edge
/// drawn at that aim is refused (`hive_port_boundary`, GH #133) — which is the
/// right refusal and not the defect. The edges below point at the hive.
fn write_topology(root: &std::path::Path) {
    write(
        root,
        "main",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./device","to":"./app","condition":"has(hop.route) && hop.route == 'page'"},
            {"from":"./device","to":"./app","condition":"has(hop.route) && hop.route == 'receipt'"}
        ]}}}"#,
    );
    // The app's own door and its inward wiring, copied in shape from
    // `voice2vision@0.7.0`: it accepts `page` and `receipt` at the rim and
    // hands both to `./stage`. The app half of this lane was never the gap.
    write(
        root,
        "main/app",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":".","to":"./stage",
             "condition":"has(hop.route) && (hop.route == 'page' || hop.route == 'receipt')"}
        ]}}}"#,
    );
    write(
        root,
        "main/device",
        r#"{"cell":{"type":"echo"},
            "params":{"emitted_target":"/app/stage",
                      "emitted_header":{"key":"route","value":"receipt"}},
            "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
    write(
        root,
        "main/mute",
        r#"{"cell":{"type":"echo"},
            "params":{"emitted_target":"/app/stage",
                      "emitted_header":{"key":"route","value":"error"}},
            "contract":{"version":"0.1.0","settings":{},"consumes":{}}}"#,
    );
}

fn probe(to: &str) -> meclaw_core::Message {
    MessageBuilder::new(Path::new(to))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"ping"}]}),
        ))
        .hop({
            let mut m = meclaw_core::serde_json::Map::new();
            m.insert("route".into(), json!("in_open"));
            m
        })
        .ttl(16)
        .build()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_child_hears_what_the_edge_onto_the_hive_carries() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path());

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());

    // Anti-cascade: the child exists before anything is sent towards it.
    let (stage_tx, mut stage_rx) = mpsc::channel(8);
    h.spawn(Path::new("/app/stage"), move || {
        CaptureCell::new(stage_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("the topology must boot");

    h.send(probe("/device")).await;

    // Half one: the aim is decoration, the edge is the delivery. The emission
    // names `/app/stage` and the only edge out of `/device` names `/app` — and
    // the child gets it, because the hive's own door hands it inward.
    let got = tokio::time::timeout(Duration::from_secs(30), stage_rx.recv())
        .await
        .expect(
            "the child of the app hive went quiet: a `receipt` the device emitted never \
             arrived. The edge out of the device is what delivers it — a route without one \
             dead-letters as `no_route`, and nothing runs upward on its own (Ruling A1)",
        )
        .expect("stage channel closed");
    assert_eq!(
        got.target,
        Path::new("/app/stage"),
        "the hive's inward edge places the emission at its child"
    );

    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_route_without_an_out_edge_is_a_dead_letter_and_not_a_log_row() {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path());

    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());

    let (stage_tx, mut stage_rx) = mpsc::channel(8);
    h.spawn(Path::new("/app/stage"), move || {
        CaptureCell::new(stage_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("the topology must boot");

    let p = probe("/mute");
    let trace_id = p.trace_id.to_string();
    h.send(p).await;

    // The dead letter is the ONLY trace of it. Waiting on the DLQ rather than
    // sleeping: `drain_dead_letters` is what a test can watch, and the entry is
    // written on the same arm that would otherwise have routed the emission.
    let mut entry = None;
    for _ in 0..300 {
        for dl in h.drain_dead_letters().await {
            if dl.sender_path == Path::new("/mute") {
                entry = Some(dl);
            }
        }
        if entry.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let entry = entry.expect(
        "an emission on a route no out-edge carries must dead-letter LOUDLY — silence here \
         would be the one failure mode no marker of this wave could see",
    );
    assert_eq!(
        entry.reason.as_code(),
        "no_route",
        "the reason names the missing edge, not the aim"
    );
    assert_eq!(
        entry.original_target,
        Path::new("/app/stage"),
        "the DLQ row keeps the AIM — which is why a reader of it can mistake the aim for the \
         cause. It is not the cause: `apply_edges` never reads the target"
    );

    // And the half that blinded six proof runs: nothing about this emission is
    // in the message log. A marker that reads `GET /colony/messages` sees the
    // same thing for "dropped" as for "never sent".
    let db = td.path().join("colony.db");
    let conn = rusqlite::Connection::open(&db).expect("colony.db");
    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_log WHERE trace_id = ? AND from_path = '/mute'",
            [&trace_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        rows, 0,
        "a dead-lettered emission writes no message-log row: the log proves DEPARTURE of \
         what was routed, never ARRIVAL, and absence in it is not evidence of silence"
    );

    assert!(
        stage_rx.try_recv().is_err(),
        "nothing reached the child on a route no edge carries"
    );

    h.shutdown().await;
}
