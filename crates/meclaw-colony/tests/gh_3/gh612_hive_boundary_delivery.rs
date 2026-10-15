//! GH #612 — a message addressed BEHIND a hive boundary.
//!
//! `docs/meclaw-overview.md` § The hive boundary, requirement 1: "The address
//! is the hive. `<hive>/<cell>` is not an address … including where the
//! substrate still resolves it today for want of a declaration." A sealed hive
//! (`params.ports`) states the same thing in a form the substrate can read, and
//! `LaneSpec::at` is the one exception the hive pronounces itself: "for THIS
//! lane, and no other, an edge may end here".
//!
//! Until now that sentence governed `add_edges` only (GH #133). A message that
//! came in from OUTSIDE the colony named an interior path directly, the router
//! found it in the flat registry, and delivered — past every door the hive put
//! in front of it, and with no receipt anywhere saying a boundary had been
//! crossed. The topology in this file is the one from the issue: a container
//! hive with a cell `voice`, and next to it a sealed hive that holds a second
//! cell of the same name.
//!
//! No model, no network: capture cells and echo cells only.

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

/// `/channels` holds a cell `voice`; `/channels/phone` is a SEALED hive
/// (`ports: []`) holding a second cell of that name plus a `dial` cell that one
/// lane names as its connect point.
fn write_topology(root: &std::path::Path) {
    write(
        root,
        "main",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        root,
        "main/channels",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    // A LEVEL inside the sealed hive, with a cell of its own. A level is an
    // address the substrate has always answered, and it has a rim of its own.
    write(
        root,
        "main/channels/phone/dialer",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
              {"from":".","to":"./pad","condition":"has(hop.route) && hop.route == 'in_pad'"}
            ]}}}"#,
    );
    write(
        root,
        "main/channels/phone",
        r#"{"cell":{"type":"hive"},"params":{
            "ports": [],
            "contract": {
              "accepts": [
                {"route":"in_speak","because":"the finished turn, spoken into the call"},
                {"route":"tool","at":["./dial"],"because":"a model called `call`"}
              ],
              "emits": [{"route":"turn","because":"what was said"}]
            },
            "graph":{"edges":[
              {"from":".","to":"./voice","condition":"has(hop.route) && hop.route == 'in_speak'"}
            ]}}}"#,
    );
}

struct Fixture {
    _td: tempfile::TempDir,
    h: ColonyHandle,
    sibling_rx: mpsc::Receiver<meclaw_core::Message>,
    deep_rx: mpsc::Receiver<meclaw_core::Message>,
    dial_rx: mpsc::Receiver<meclaw_core::Message>,
    pad_rx: mpsc::Receiver<meclaw_core::Message>,
}

async fn boot() -> Fixture {
    let td = tempfile::TempDir::new().unwrap();
    write_topology(td.path());
    let h = ColonyHandle::new_with_factories_at(&td, echo_factories());

    let (sib_tx, sibling_rx) = mpsc::channel(8);
    h.spawn(Path::new("/channels/voice"), move || {
        CaptureCell::new(sib_tx.clone())
    })
    .await;
    let (deep_tx, deep_rx) = mpsc::channel(8);
    h.spawn(Path::new("/channels/phone/voice"), move || {
        CaptureCell::new(deep_tx.clone())
    })
    .await;
    let (dial_tx, dial_rx) = mpsc::channel(8);
    h.spawn(Path::new("/channels/phone/dial"), move || {
        CaptureCell::new(dial_tx.clone())
    })
    .await;
    let (pad_tx, pad_rx) = mpsc::channel(8);
    h.spawn(Path::new("/channels/phone/dialer/pad"), move || {
        CaptureCell::new(pad_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &echo_registry(), &h.runtime())
        .await
        .expect("the topology must boot");

    Fixture {
        _td: td,
        h,
        sibling_rx,
        deep_rx,
        dial_rx,
        pad_rx,
    }
}

fn source(target: &str, lane: Option<&str>) -> meclaw_core::Message {
    let mut b = MessageBuilder::new(Path::new(target))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":"ping"}]}),
        ))
        .ttl(16);
    if let Some(l) = lane {
        let mut m = meclaw_core::serde_json::Map::new();
        m.insert("route".into(), json!(l));
        b = b.hop(m);
    }
    b.build()
}

/// VARIANT 1 — the issue's own shape: an external message names the interior
/// path directly. Neither cell may be reached by it, and the refusal has to be
/// a receipt that names the boundary.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_external_message_naming_an_interior_path_is_refused() {
    let mut f = boot().await;
    let probe = source("/channels/phone/voice", None);
    let (trace_id, message_id) = (probe.trace_id, probe.id);
    f.h.send(probe).await;

    // Nothing may arrive at either cell — not at the addressed one, and above
    // all not at the same-named sibling one level up.
    assert!(
        tokio::time::timeout(Duration::from_millis(600), f.deep_rx.recv())
            .await
            .is_err(),
        "the interior cell must not be reached by an external message naming its path"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(200), f.sibling_rx.recv())
            .await
            .is_err(),
        "the same-named sibling must never see a message addressed past the boundary"
    );

    let dlq = f.h.drain_dead_letters().await;
    let codes: Vec<&str> = dlq.iter().map(|d| d.reason.as_code()).collect();
    assert!(
        codes.contains(&"hive_boundary"),
        "the refusal must name the boundary, got {codes:?}"
    );
    let e = dlq
        .iter()
        .find(|d| d.reason.as_code() == "hive_boundary")
        .unwrap();
    assert_eq!(e.resolved_target, Path::new("/channels/phone/voice"));

    // The receipt NAMES THE BOUNDARY. `resolved_target` is the address that was
    // refused and `sender_path` the caller; the hive that refused is neither, so
    // without `detail` a reader of `/colony/dead_letters` could not tell which
    // boundary said no — only that some boundary did. The drain reconstructs the
    // entry from `colony.db`, so this also proves the column round-trips.
    assert_eq!(
        e.detail(),
        Some("/channels/phone"),
        "the entry must name the hive that refused, not only the address"
    );

    // And it is findable. A boundary refusal happens BEFORE the corridor, so it
    // writes no `message_log` row and `/colony/trace` stays empty for it — the
    // dead letter is the only record, and it carries both ids a caller can join
    // on (`DeadLetterDto` exposes them as `trace_id` and `message_id`).
    assert_eq!(e.message.trace_id, trace_id, "the entry carries the trace");
    assert_eq!(
        e.message.id, message_id,
        "the entry carries the message id of the very message that was posted"
    );
}

/// VARIANT 2 — the address the hive DOES have: the hive path plus the lane.
/// This is the shape the boundary rule asks for, and it must keep working.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_hive_path_plus_its_lane_still_reaches_the_interior() {
    let mut f = boot().await;
    f.h.send(source("/channels/phone", Some("in_speak"))).await;

    let got = tokio::time::timeout(Duration::from_secs(30), f.deep_rx.recv())
        .await
        .expect("the transit never completed")
        .expect("channel closed");
    assert_eq!(got.target, Path::new("/channels/phone/voice"));
}

/// VARIANT 3 — the exception a sealed hive pronounces itself: a lane with a
/// connect point (`accepts[].at`). On that lane, and no other, the interior
/// address is an address.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_declared_connect_point_is_reachable_on_its_own_lane() {
    let mut f = boot().await;
    f.h.send(source("/channels/phone/dial", Some("tool"))).await;

    let got = tokio::time::timeout(Duration::from_secs(30), f.dial_rx.recv())
        .await
        .expect("the declared connect point went quiet")
        .expect("channel closed");
    assert_eq!(got.target, Path::new("/channels/phone/dial"));
}

/// The same connect point on a DIFFERENT lane is not an address — the
/// declaration is per lane, and `ports: []` stays literally true for the rest.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_connect_point_on_a_foreign_lane_is_refused() {
    let mut f = boot().await;
    f.h.send(source("/channels/phone/dial", Some("in_speak")))
        .await;

    assert!(
        tokio::time::timeout(Duration::from_millis(600), f.dial_rx.recv())
            .await
            .is_err(),
        "a connect point is declared per lane, not per address"
    );
    let dlq = f.h.drain_dead_letters().await;
    assert!(
        dlq.iter().any(|d| d.reason.as_code() == "hive_boundary"),
        "got {:?}",
        dlq.iter().map(|d| d.reason.as_code()).collect::<Vec<_>>()
    );
}

/// An UNSEALED hive declares no boundary the substrate can read, and nothing
/// about it moves: GH #133's opt-in shape, one layer down.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unsealed_container_is_untouched() {
    let mut f = boot().await;
    f.h.send(source("/channels/voice", None)).await;

    let got = tokio::time::timeout(Duration::from_secs(30), f.sibling_rx.recv())
        .await
        .expect("the unsealed container's own cell went quiet")
        .expect("channel closed");
    assert_eq!(got.target, Path::new("/channels/voice"));
}

/// A LEVEL inside a sealed hive is refused exactly like a cell (ruling 9).
///
/// The looser reading — a nested rim is an address of its own, so naming it
/// walks past no declaration — was taken first and then measured away: five
/// shipped sealed templates hold a nested hive through a `ref` marker
/// (`operator/submit`, `builder/builder-librarian`, `cogny/collector`,
/// `talky/collector`, `talky/session-keeper`). Under the looser reading an
/// outside caller could name one of those and reach into the composite past its
/// rim, which is the bypass requirement 1 exists to refuse: "`<hive>/<cell>` is
/// not an address, and `<hive>/<subhive>/<cell>` is less of one". Nothing shipped
/// depends on the looser reading, so the literal one is what is enforced.
///
/// The way in stays what it always was: the hive declares the address, as a port
/// or as a connect point of a lane.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_level_inside_a_sealed_hive_is_refused_like_a_cell() {
    let mut f = boot().await;
    f.h.send(source("/channels/phone/dialer", Some("in_pad")))
        .await;

    assert!(
        tokio::time::timeout(Duration::from_millis(600), f.pad_rx.recv())
            .await
            .is_err(),
        "a nested rim is not a way around the boundary its parent drew"
    );
    let dlq = f.h.drain_dead_letters().await;
    let e = dlq
        .iter()
        .find(|d| d.reason.as_code() == "hive_boundary")
        .expect("the refusal names the boundary");
    assert_eq!(e.resolved_target, Path::new("/channels/phone/dialer"));
    assert_eq!(e.detail(), Some("/channels/phone"));
}
