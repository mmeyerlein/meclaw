//! GH #773 — a lift judges a child by what makes the instance, and the
//! receipt names every store it sets aside.
//!
//! The ruling of 2026-09-22: `replace_nodes` compares a standing child with
//! the new version by the three blocks the substrate reads (`cell`,
//! `params`, `contract`) — a rewritten `description` no longer condemns a
//! child, and with it the `cell.db` it owns. A child that IS replaced parks
//! with its whole directory as `<name>~<version>`; the receipt now says where
//! (`parked_path`) and whether a database went with it (`parked_store`), and
//! `/colony/graph` marks the parked row instead of passing it off as a cell
//! that merely lost its edges.
//!
//! The fixture is a trimmed copy of `gh682_a_standing_hive_is_lifted_in_place.rs`
//! (there is no shared support module under `crates/meclaw-colony/tests/`):
//! the same `echo_sub` factory with real stop wiring, the persona hive
//! `/alex`, and two small classes:
//!
//! ```text
//! screen@1.0.0   . ─in_view─▶ keep ─▶ bump ─▶ .
//! screen@1.0.1   keep: another description.purpose — nothing else
//! screen@1.1.0   keep and bump: other params
//!
//! panel@1.0.0    . ─in_view─▶ keep(ref leaf@1.0.0) ─▶ .
//! panel@1.0.2    keep: ref leaf@1.0.1 (leaf differs in description only)
//! panel@1.1.2    keep: ref leaf@1.1.0 (leaf with other params)
//! ```
//!
//! Every lock reads what the colony hands out or leaves behind — the
//! receipt, the door reply, the graph read, the bytes on disk.

use meclaw_colony::api_dto::GraphNodeDto;
use meclaw_colony::mutation::{NodeChange, NodeVerdict};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, ContractView, DiskBlobStore, MutationDoorOutcome,
    MutationOutcome, RespawnFn, SpawnedCellKind, bootstrap_from_filesystem, cell_task,
    mutation_door_reply, renotify_stop_wiring,
};
use meclaw_core::serde_json::json;
use meclaw_core::{CellEmission, JsonValue, Message, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

// ─────────────────────────────────────────────────────────────────────────────
// echo_sub — copied from gh682_a_standing_hive_is_lifted_in_place.rs: an echo
// cell with real stop wiring, so the lift's own recompute can stop and
// respawn children.
// ─────────────────────────────────────────────────────────────────────────────

struct SubtreeEchoFactory;

fn parse_echo_to(params: &JsonValue) -> Result<Path, String> {
    params
        .get("echo_to")
        .and_then(|v| v.as_str())
        .map(Path::new)
        .ok_or_else(|| "params.echo_to missing or not a string".to_string())
}

/// What one build of an echo cell hands back: the registry-facing mailbox
/// sender, the task, its peace and backstop receivers, and the stop pair the
/// colony uses to peace-stop it.
type BuiltEcho = (
    mpsc::Sender<Message>,
    JoinHandle<()>,
    oneshot::Receiver<()>,
    oneshot::Receiver<()>,
    oneshot::Sender<()>,
    oneshot::Receiver<()>,
);

/// Build one echo task at `path` with a pump in front of `cell_task` that
/// answers the colony's stop signal (fires peace, hands the mailbox back as
/// `ColonyMsg::Stopped`, drops the death-ack sender last).
fn build_echo(
    path: &Path,
    echo_to: &Path,
    outputs_tx: &mpsc::Sender<CellEmission>,
    colony_inbox: Option<&mpsc::Sender<ColonyMsg>>,
) -> BuiltEcho {
    let (tx, mut rx) = mpsc::channel::<Message>(1000);
    let (inner_tx, inner_rx) = mpsc::channel::<Message>(1000);
    let (peace_tx, peace_rx) = oneshot::channel();
    let (_backstop_tx, backstop_rx) = oneshot::channel();
    let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
    let (death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
    let cell = EchoMockCell::new(path.clone()).emitted_target(echo_to.clone());
    let inner = tokio::spawn(cell_task(
        path.clone(),
        inner_rx,
        outputs_tx.clone(),
        cell,
        None,
        None,
        None,
    ));
    let p = path.clone();
    let inbox = colony_inbox.cloned();
    let join = tokio::spawn(async move {
        let _death_ack = death_ack_tx;
        loop {
            tokio::select! {
                _ = &mut stop_rx => {
                    let _ = peace_tx.send(());
                    if let Some(inbox) = inbox {
                        let _ = inbox
                            .send(ColonyMsg::Stopped { path: p, receiver: rx })
                            .await;
                    }
                    break;
                }
                next = rx.recv() => match next {
                    Some(msg) => {
                        if inner_tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                },
            }
        }
        drop(inner_tx);
        let _ = inner.await;
    });
    (tx, join, peace_rx, backstop_rx, stop_tx, death_ack_rx)
}

/// The respawn closure the reconnect-eager arm invokes (GH #676 form).
fn make_echo_respawn(
    path: Path,
    echo_to: Path,
    outputs_tx: mpsc::Sender<CellEmission>,
    colony_inbox: mpsc::Sender<ColonyMsg>,
) -> RespawnFn {
    Box::new(move || {
        let (tx, join, peace_rx, backstop_rx, stop_tx, death_ack_rx) =
            build_echo(&path, &echo_to, &outputs_tx, Some(&colony_inbox));
        renotify_stop_wiring(&colony_inbox, path.clone(), stop_tx, death_ack_rx);
        (tx, join, peace_rx, backstop_rx)
    })
}

impl CellFactory for SubtreeEchoFactory {
    fn validate_params(&self, params: &JsonValue) -> Result<(), String> {
        parse_echo_to(params).map(|_| ())
    }

    fn spawn_cell(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let echo_to = parse_echo_to(&params)?;
        let (sender, join, peace_rx, backstop_rx, stop_tx, death_ack_rx) =
            build_echo(&path, &echo_to, &outputs_tx, Some(&colony_inbox_tx));
        let respawn = make_echo_respawn(path, echo_to, outputs_tx, colony_inbox_tx);
        Ok(SpawnedCellKind::Active {
            sender,
            join,
            peace_rx,
            stop_tx,
            death_ack_rx,
            backstop_rx,
            respawn,
        })
    }

    fn build_boot_inactive_respawn(
        self: Arc<Self>,
        path: Path,
        params: JsonValue,
        outputs_tx: mpsc::Sender<CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: ContractView,
        colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<DiskBlobStore>>,
        _mailbox_capacity: usize,
    ) -> Option<RespawnFn> {
        let echo_to = parse_echo_to(&params).ok()?;
        Some(make_echo_respawn(
            path,
            echo_to,
            outputs_tx,
            colony_inbox_tx,
        ))
    }
}

fn factory_list() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo_sub".to_string(),
        Arc::new(SubtreeEchoFactory) as Arc<dyn CellFactory>,
    )]
}

fn factory_registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    for (name, f) in factory_list() {
        r.insert(name, f);
    }
    r
}

// ─────────────────────────────────────────────────────────────────────────────
// Fixture on disk
// ─────────────────────────────────────────────────────────────────────────────

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// The persona hive `/alex` (awake through `/world -> /alex`, GH #265) with
/// one caller, `sender`; the `capture` sink is spawned by the handle.
fn write_alex_topology(root: &std::path::Path) {
    write(
        root,
        "main/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[
            {"from":"./world","to":"./alex"}
        ]}}}"#,
    );
    write(
        root,
        "main/world/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/alex"},"contract":{"version":"1.0.0","settings":{},"consumes":{}}}"#,
    );
    write(
        root,
        "main/alex/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
    write(
        root,
        "main/alex/sender/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/alex/display"},"contract":{"version":"1.0.0","settings":{},"consumes":{}}}"#,
    );
}

/// An `echo_sub` child. `flavour` is an extra param (the behaviour a version
/// may change), `purpose` the prose of its `description` (which it may
/// rewrite without consequence since GH #773).
fn child(flavour: &str, purpose: &str) -> String {
    format!(
        r#"{{"cell":{{"type":"echo_sub"}},"params":{{"echo_to":"/capture","flavour":"{flavour}"}},"contract":{{"version":"1.0.0","settings":{{}},"consumes":{{}}}},"description":{{"purpose":"{purpose}"}}}}"#
    )
}

/// A hive declaration: accepts `in_view`, routes it through `chain`.
fn hive(chain: &[&str]) -> String {
    let mut edges = vec![
        r#"{"from":".","to":"./keep","condition":"has(hop.route) && hop.route == 'in_view'"}"#
            .to_string(),
    ];
    let mut prev = "./keep".to_string();
    for next in chain {
        edges.push(format!(r#"{{"from":"{prev}","to":"./{next}"}}"#));
        prev = format!("./{next}");
    }
    edges.push(format!(r#"{{"from":"{prev}","to":"."}}"#));
    format!(
        r#"{{"cell":{{"type":"hive"}},"params":{{
            "contract":{{"accepts":[{{"route":"in_view","because":"put this view up"}}]}},
            "graph":{{"edges":[{}]}}}}}}"#,
        edges.join(",")
    )
}

/// One template directory `templates/local/<name>@<version>` with its
/// `template.json`, its own `config.json` and the given children.
fn class(root: &std::path::Path, name: &str, version: &str, own: &str, children: &[(&str, &str)]) {
    let dir = root.join(format!("templates/local/{name}@{version}"));
    write(
        &dir,
        "template.json",
        &format!(r#"{{"name":"{name}","version":"{version}"}}"#),
    );
    write(&dir, "config.json", own);
    for (rel, cfg) in children {
        write(&dir, &format!("{rel}/config.json"), cfg);
    }
}

/// `screen` in three versions (`keep` → `bump` → `.`):
/// 1.0.1 differs from 1.0.0 only in `keep`'s `description.purpose`;
/// 1.1.0 gives `keep` and `bump` other `params`.
fn write_screen_templates(root: &std::path::Path) {
    let own = hive(&["bump"]);
    class(
        root,
        "screen",
        "1.0.0",
        &own,
        &[
            ("keep", &child("a", "keeps the view")),
            ("bump", &child("a", "bumps it on")),
        ],
    );
    class(
        root,
        "screen",
        "1.0.1",
        &own,
        &[
            (
                "keep",
                &child("a", "holds the view it was handed, word for word"),
            ),
            ("bump", &child("a", "bumps it on")),
        ],
    );
    class(
        root,
        "screen",
        "1.1.0",
        &own,
        &[
            ("keep", &child("b", "keeps the view")),
            ("bump", &child("b", "bumps it on")),
        ],
    );
}

/// `leaf` in three versions — 1.0.1 differs from 1.0.0 in `description`
/// only, 1.1.0 in `params` — and `panel`, a hive whose one child `keep`
/// comes in through a `ref` to a `leaf` version.
fn write_panel_templates(root: &std::path::Path) {
    for (v, flavour, purpose) in [
        ("1.0.0", "a", "a leaf"),
        ("1.0.1", "a", "a leaf, described anew"),
        ("1.1.0", "b", "a leaf"),
    ] {
        class(root, "leaf", v, &child(flavour, purpose), &[]);
    }
    let own = hive(&[]);
    for (v, leaf) in [("1.0.0", "1.0.0"), ("1.0.2", "1.0.1"), ("1.1.2", "1.1.0")] {
        let marker = format!(r#"{{"cell":{{"type":"ref","template":"leaf@{leaf}"}}}}"#);
        class(root, "panel", v, &own, &[("keep", &marker)]);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// A running colony
// ─────────────────────────────────────────────────────────────────────────────

struct Colony {
    h: ColonyHandle,
    _capture_rx: mpsc::Receiver<Message>,
}

async fn start_colony(td: &TempDir) -> Colony {
    let h = ColonyHandle::new_with_factories_at(td, factory_list());
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap().expect("the rescan must not abort");

    let (capture_tx, capture_rx) = mpsc::channel(32);
    h.spawn(Path::new("/alex/capture"), move || {
        CaptureCell::new(capture_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &factory_registry(), &h.runtime())
        .await
        .expect("the persona topology must boot");
    Colony {
        h,
        _capture_rx: capture_rx,
    }
}

async fn submit_diff(h: &ColonyHandle, scope: &str, diff: JsonValue) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload: json!({"scope": scope, "diff": diff}),
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap()
}

/// Grow `display` from `template` and wire it: `sender -> display` on the
/// `in_view` lane, `display -> capture` for what leaves it.
async fn grow(h: &ColonyHandle, template: &str) {
    let outcome = submit_diff(
        h,
        "/alex",
        json!({
            "add_nodes": [{"name": "display", "template": template}],
            "add_edges": [
                {"from": "./sender", "to": "./display",
                 "modifier": {"set_hop": {"route": "'in_view'"}}},
                {"from": "./display", "to": "./capture",
                 "condition": "!has(hop.route)"}
            ]
        }),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "growing {template} must commit; got {outcome:?}"
    );
}

/// Lift `/alex/display` to `template`; the committed outcome is returned.
async fn lift_to(h: &ColonyHandle, template: &str) -> MutationOutcome {
    let outcome = submit_diff(
        h,
        "/alex",
        json!({"replace_nodes": [
            {"match": {"name": "display"}, "with": {"template": template}}
        ]}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "lifting display to {template} must commit; got {outcome:?}"
    );
    outcome
}

/// The receipt entry for `path` of a committed outcome.
fn change_at(outcome: &MutationOutcome, path: &str) -> NodeChange {
    let MutationOutcome::Committed { changes, .. } = outcome else {
        panic!("not committed: {outcome:?}");
    };
    changes
        .iter()
        .find(|c| c.path == path)
        .cloned()
        .unwrap_or_else(|| panic!("no receipt entry for {path}: {changes:?}"))
}

/// The graph nodes `/colony/graph` hands out for `scope`.
async fn graph_nodes(h: &ColonyHandle, scope: &str) -> Vec<GraphNodeDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new(scope),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap().nodes
}

/// A child's directory under the colony root.
fn cell_dir(td: &TempDir, rel: &str) -> std::path::PathBuf {
    td.path().join("main/alex/display").join(rel)
}

/// Put a sentinel database into a standing child — the bytes a store cell
/// would have written (the `mutation/recovery.rs` form).
fn plant_store(td: &TempDir, rel: &str) -> Vec<u8> {
    let bytes = format!("sentinel-db-of-{rel}").into_bytes();
    std::fs::write(cell_dir(td, rel).join("cell.db"), &bytes).unwrap();
    bytes
}

fn read(p: &std::path::Path) -> Vec<u8> {
    std::fs::read(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

// ─────────────────────────────────────────────────────────────────────────────
// The locks
// ─────────────────────────────────────────────────────────────────────────────

/// A version that only rewrites a child's `description` keeps the child: its
/// `config.json` and its `cell.db` stand byte for byte, nothing is parked,
/// and the receipt says `kept` with no park path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rewritten_description_keeps_the_child() {
    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_screen_templates(td.path());
    let colony = start_colony(&td).await;
    grow(&colony.h, "screen@1.0.0").await;
    let db = plant_store(&td, "keep");
    let cfg_before = read(&cell_dir(&td, "keep").join("config.json"));

    let outcome = lift_to(&colony.h, "screen@1.0.1").await;

    let keep = change_at(&outcome, "/alex/display/keep");
    assert_eq!(keep.verdict, NodeVerdict::Kept, "{keep:?}");
    assert_eq!(keep.parked_path, None);
    assert_eq!(keep.parked_store, None);
    assert_eq!(
        read(&cell_dir(&td, "keep").join("cell.db")),
        db,
        "the store stays"
    );
    assert_eq!(
        read(&cell_dir(&td, "keep").join("config.json")),
        cfg_before,
        "the config stands byte for byte"
    );
    assert!(
        !cell_dir(&td, "keep~1.0.0").exists(),
        "nothing is parked for a rewritten description"
    );
    colony.h.shutdown().await;
}

/// A version with other `params` replaces the child — and the receipt names
/// where the old one went and that its `cell.db` went with it; a replaced
/// child without a database says `parked_store: false`. The door reply
/// carries both fields on the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_receipt_names_the_parked_store() {
    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_screen_templates(td.path());
    let colony = start_colony(&td).await;
    grow(&colony.h, "screen@1.0.0").await;
    let db = plant_store(&td, "keep");

    let outcome = lift_to(&colony.h, "screen@1.1.0").await;

    let keep = change_at(&outcome, "/alex/display/keep");
    assert_eq!(keep.verdict, NodeVerdict::Replaced, "{keep:?}");
    assert_eq!(
        keep.parked_path.as_deref(),
        Some("/alex/display/keep~1.0.0")
    );
    assert_eq!(keep.parked_store, Some(true));
    assert_eq!(
        read(&cell_dir(&td, "keep~1.0.0").join("cell.db")),
        db,
        "the database went aside with the directory"
    );
    assert!(
        !cell_dir(&td, "keep").join("cell.db").exists(),
        "the successor starts without it"
    );
    let bump = change_at(&outcome, "/alex/display/bump");
    assert_eq!(bump.verdict, NodeVerdict::Replaced, "{bump:?}");
    assert_eq!(
        bump.parked_path.as_deref(),
        Some("/alex/display/bump~1.0.0")
    );
    assert_eq!(bump.parked_store, Some(false), "bump owned no database");

    let reply = mutation_door_reply(&MutationDoorOutcome::Single(outcome.clone()));
    let mut wire = reply["mutation"]["changes"]
        .as_array()
        .expect("changes on the wire")
        .iter()
        .find(|c| c["path"] == "/alex/display/keep")
        .cloned()
        .expect("keep on the wire");
    // GH #811: the template ids are pinned in gh811; this test is about the park.
    if let Some(obj) = wire.as_object_mut() {
        obj.remove("from_template_id");
        obj.remove("to_template_id");
    }
    assert_eq!(
        wire,
        json!({"path": "/alex/display/keep", "verdict": "replaced",
               "from_version": "1.0.0", "to_version": "1.1.0",
               "parked_path": "/alex/display/keep~1.0.0", "parked_store": true})
    );
    colony.h.shutdown().await;
}

/// `/colony/graph` marks what a lift parked instead of leaving it out or
/// passing it off as a live cell: the parked predecessor is `parked: true,
/// active: false`, its successor `parked: false, active: true`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_graph_marks_a_parked_node() {
    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_screen_templates(td.path());
    let colony = start_colony(&td).await;
    grow(&colony.h, "screen@1.0.0").await;
    lift_to(&colony.h, "screen@1.1.0").await;

    let nodes = graph_nodes(&colony.h, "/alex/display").await;
    let node = |path: &str| {
        nodes
            .iter()
            .find(|n| n.path == path)
            .cloned()
            .unwrap_or_else(|| panic!("no graph node {path}: {nodes:?}"))
    };
    let parked = node("/alex/display/keep~1.0.0");
    assert!(parked.parked, "{parked:?}");
    assert!(!parked.active, "{parked:?}");
    let live = node("/alex/display/keep");
    assert!(!live.parked, "{live:?}");
    assert!(live.active, "{live:?}");
    colony.h.shutdown().await;
}

/// A child that comes in through a `ref` is judged by the same three blocks:
/// a lift whose ref moved to a leaf version with the same blocks keeps the
/// child — bytes, database and the old provenance stamp; a ref to a leaf
/// with other `params` replaces it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_moved_ref_with_the_same_blocks_keeps_its_child() {
    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_panel_templates(td.path());
    let colony = start_colony(&td).await;
    grow(&colony.h, "panel@1.0.0").await;
    let db = plant_store(&td, "keep");
    let cfg_before = read(&cell_dir(&td, "keep").join("config.json"));

    let outcome = lift_to(&colony.h, "panel@1.0.2").await;
    let keep = change_at(&outcome, "/alex/display/keep");
    assert_eq!(keep.verdict, NodeVerdict::Kept, "{keep:?}");
    assert_eq!(keep.from_version.as_deref(), Some("1.0.0"));
    assert_eq!(
        keep.to_version.as_deref(),
        Some("1.0.0"),
        "the old stamp stays"
    );
    assert_eq!(read(&cell_dir(&td, "keep").join("cell.db")), db);
    assert_eq!(read(&cell_dir(&td, "keep").join("config.json")), cfg_before);
    let cfg: JsonValue = meclaw_core::serde_json::from_slice(&cfg_before).expect("config parses");
    assert_eq!(cfg["cell"]["provenance"]["template"], "leaf");
    assert_eq!(cfg["cell"]["provenance"]["template_version"], "1.0.0");

    let outcome = lift_to(&colony.h, "panel@1.1.2").await;
    let keep = change_at(&outcome, "/alex/display/keep");
    assert_eq!(keep.verdict, NodeVerdict::Replaced, "{keep:?}");
    assert_eq!(keep.from_version.as_deref(), Some("1.0.0"));
    assert_eq!(keep.to_version.as_deref(), Some("1.1.0"));
    assert_eq!(
        keep.parked_path.as_deref(),
        Some("/alex/display/keep~1.0.0")
    );
    assert_eq!(keep.parked_store, Some(true));
    colony.h.shutdown().await;
}
