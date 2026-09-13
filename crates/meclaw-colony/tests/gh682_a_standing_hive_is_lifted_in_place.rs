//! GH #682 — a standing hive is lifted in place to a new version of its
//! template (`replace_nodes`).
//!
//! This file grows with the wave. The fixture it rests on is one hive class,
//! `screen`, shipped in two versions as siblings of the library
//! (`local/screen@1.0.0`, `local/screen@1.1.0` — GH #664 lets a class have
//! more than one version):
//!
//! ```text
//! screen@1.0.0                          screen@1.1.0
//!   accepts  in_view                      accepts  in_view, in_notice
//!   . ─in_view─▶ keep ─▶ bump ─▶ gone ─▶ .    . ─in_view───▶ keep ─▶ bump ─▶ .
//!                                            . ─in_notice─▶ fresh ─────────▶ .
//!   keep   1.0.0                           keep   1.0.0   (byte-identical)
//!   bump   1.0.0                           bump   1.1.0   (the version diff)
//!   gone   1.0.0                           fresh  1.0.0   (new)
//!                                          — no gone —
//! ```
//!
//! Around it stands a colony with one persona hive `/alex`: a `sender` cell,
//! a `capture` sink, and the `screen` grown as `/alex/display` with two outer
//! edges — `sender -> display` stating the `in_view` lane, and
//! `display -> capture` for whatever leaves the screen.
//!
//! Every child is an `echo_sub` cell (the eager-reconnect echo factory from
//! `paket_2_swap.rs`, here with real stop wiring — see `build_echo`), so a
//! probe walks the whole inner chain and the sink receives a positive
//! receipt. No model, no network.

use meclaw_colony::api_dto::{GraphEdgeDto, RegistryEntryDto};
use meclaw_colony::mutation::stage_replace::{StagedReplace, stage_replace_nodes};
use meclaw_colony::mutation::validate::DIFF_OPERATIONS;
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, ContractView, DiskBlobStore, MutationOutcome,
    RespawnFn, SpawnedCellKind, bootstrap_from_filesystem, cell_task, renotify_stop_wiring,
};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, CellEmission, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mocks::EchoMockCell;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

// ─────────────────────────────────────────────────────────────────────────────
// echo_sub — after `paket_2_swap.rs` (`SubtreeEchoFactory`, private there),
// with real stop wiring on top. An echo cell with a real
// `build_boot_inactive_respawn`, so a child that arrives inactive through
// `add_nodes` comes alive the moment an edge connects it — and can be stopped
// again, which the lift needs (a changed child's old task and a left child
// are stopped by the lift's own recompute).
// ─────────────────────────────────────────────────────────────────────────────

struct SubtreeEchoFactory;

fn parse_echo_to(params: &JsonValue) -> Result<Path, String> {
    params
        .get("echo_to")
        .and_then(|v| v.as_str())
        .map(Path::new)
        .ok_or_else(|| "params.echo_to missing or not a string".to_string())
}

/// GH #688: `params.panic_on_stop` (default `false`) makes the pump PANIC on
/// the colony's stop signal instead of exiting in peace — the non-peaceful
/// death inside a lift's stop window that the issue describes. The unwind
/// drops the mailbox receiver, the death-ack sender and the peace sender
/// unsent, exactly what a panicking production task drops.
fn parse_panic_on_stop(params: &JsonValue) -> bool {
    params
        .get("panic_on_stop")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
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

/// Build one echo task at `path` with REAL stop wiring (Task 5): the plain
/// `cell_task` has no stop arm, so a pump sits in front of it — it forwards
/// the registry-facing mailbox into the cell's own, and on `stop_tx` it fires
/// peace (silent exit), hands the mailbox back as `ColonyMsg::Stopped` the
/// way a stateful cell does, and drops the death-ack sender last. Without
/// this a child reactivated by an earlier recompute is `Awake` with no
/// `stop_tx`, and the lift's own recompute would be refused by the
/// `stop_wiring_unavailable` guard (the #673/#676 class).
fn build_echo(
    path: &Path,
    echo_to: &Path,
    panic_on_stop: bool,
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
                    if panic_on_stop {
                        panic!("{}: panicking on stop (GH #688 fixture)", p.as_str());
                    }
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

/// The respawn closure the reconnect-eager arm invokes: builds the task anew
/// and hands the fresh stop pair back to the colony through
/// `renotify_stop_wiring`, the way the shipped factories do (GH #676).
fn make_echo_respawn(
    path: Path,
    echo_to: Path,
    panic_on_stop: bool,
    outputs_tx: mpsc::Sender<CellEmission>,
    colony_inbox: mpsc::Sender<ColonyMsg>,
) -> RespawnFn {
    Box::new(move || {
        let (tx, join, peace_rx, backstop_rx, stop_tx, death_ack_rx) = build_echo(
            &path,
            &echo_to,
            panic_on_stop,
            &outputs_tx,
            Some(&colony_inbox),
        );
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
        let panic_on_stop = parse_panic_on_stop(&params);
        let (sender, join, peace_rx, backstop_rx, stop_tx, death_ack_rx) = build_echo(
            &path,
            &echo_to,
            panic_on_stop,
            &outputs_tx,
            Some(&colony_inbox_tx),
        );
        let respawn = make_echo_respawn(path, echo_to, panic_on_stop, outputs_tx, colony_inbox_tx);
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
            parse_panic_on_stop(&params),
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

/// Write one file below `dir`, creating the directories on the way (copied
/// from `paket_2_swap.rs`).
fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// The persona hive `/alex` and the cells that stand in it before the screen
/// is grown: `sender` (an echo whose emit target the outer edge overrides),
/// `notifier` (the same, edge-less until a test wires it) and the scope
/// itself. `capture` is a registry-only sink the colony handle
/// spawns before bootstrap (anti-cascade: sink before probe).
///
/// A hive is awake only through an edge EXTERNAL to its unit (GH #265) — its
/// own inside connects it to nothing. So the root wires a `world` cell at the
/// persona (`/world -> /alex`), the way a member hive hangs on its member
/// edge; without it every cell under `/alex` would be derived inactive and the
/// grow would try to disconnect the awake sink.
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
    // A second caller, wired to nothing until a test draws its edge (Task 5:
    // the `in_notice` lane the 1.1.0 contract adds).
    write(
        root,
        "main/alex/notifier/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/alex/display"},"contract":{"version":"1.0.0","settings":{},"consumes":{}}}"#,
    );
}

/// A screen child: an `echo_sub` that echoes to `/capture` (the inner edges
/// override the emit target anyway) at the given contract version.
fn child(version: &str) -> String {
    format!(
        r#"{{"cell":{{"type":"echo_sub"}},"params":{{"echo_to":"/capture"}},"contract":{{"version":"{version}","settings":{{}},"consumes":{{}}}}}}"#
    )
}

/// The two versions of the `screen` class, side by side under
/// `{templates}/local/screen@<version>/` (the shape a versioned registration
/// builds, GH #664).
///
/// 1.0.0: accepts `in_view`; chain `. -> keep -> bump -> gone -> .`; children
/// `keep`, `bump`, `gone`, all at contract 1.0.0.
///
/// 1.1.0: accepts `in_view` and `in_notice`; `keep` byte-identical; `bump` at
/// contract 1.1.0 (the bytes differ — that is the version diff); a new child
/// `fresh` (like `keep`) on the `in_notice` lane; no `gone`.
fn write_screen_templates(root: &std::path::Path) {
    let v1 = root.join("templates/local/screen@1.0.0");
    write(
        &v1,
        "template.json",
        r#"{"name":"screen","version":"1.0.0"}"#,
    );
    write(
        &v1,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{
            "contract":{"accepts":[
                {"route":"in_view","because":"put this view up on the screen"}
            ]},
            "graph":{"edges":[
                {"from":".","to":"./keep","condition":"has(hop.route) && hop.route == 'in_view'"},
                {"from":"./keep","to":"./bump"},
                {"from":"./bump","to":"./gone"},
                {"from":"./gone","to":"."}
            ]}}}"#,
    );
    write(&v1, "keep/config.json", &child("1.0.0"));
    write(&v1, "bump/config.json", &child("1.0.0"));
    write(&v1, "gone/config.json", &child("1.0.0"));

    let v2 = root.join("templates/local/screen@1.1.0");
    write(
        &v2,
        "template.json",
        r#"{"name":"screen","version":"1.1.0"}"#,
    );
    write(
        &v2,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{
            "contract":{"accepts":[
                {"route":"in_view","because":"put this view up on the screen"},
                {"route":"in_notice","because":"a short notice, shown once and gone"}
            ]},
            "graph":{"edges":[
                {"from":".","to":"./keep","condition":"has(hop.route) && hop.route == 'in_view'"},
                {"from":".","to":"./fresh","condition":"has(hop.route) && hop.route == 'in_notice'"},
                {"from":"./keep","to":"./bump"},
                {"from":"./bump","to":"."},
                {"from":"./fresh","to":"."}
            ]}}}"#,
    );
    write(&v2, "keep/config.json", &child("1.0.0"));
    write(&v2, "bump/config.json", &child("1.1.0"));
    write(&v2, "fresh/config.json", &child("1.0.0"));
}

// ─────────────────────────────────────────────────────────────────────────────
// A running colony
// ─────────────────────────────────────────────────────────────────────────────

/// One booted colony over a caller-owned root, with the `/alex/capture` sink's
/// receiver. The handle is public so a test can send, drain dead letters and
/// shut down; `reboot` hands back a fresh one over the same root.
struct Colony {
    h: ColonyHandle,
    capture_rx: mpsc::Receiver<Message>,
}

/// Boot a colony over `td` (which must already hold the topology and the
/// templates): spawn the sink, rescan the library, bootstrap from disk.
async fn start_colony(td: &TempDir) -> Colony {
    let h = ColonyHandle::new_with_factories_at(td, factory_list());
    rescan_templates(&h, td.path().join("templates")).await;

    let (capture_tx, capture_rx) = mpsc::channel(32);
    h.spawn(Path::new("/alex/capture"), move || {
        CaptureCell::new(capture_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &factory_registry(), &h.runtime())
        .await
        .expect("the persona topology must boot");
    Colony { h, capture_rx }
}

/// Shut the colony down and boot a fresh one over the same root — the
/// durability half of every later proof.
async fn reboot(colony: Colony, td: &TempDir) -> Colony {
    colony.h.shutdown().await;
    drop(colony.capture_rx);
    start_colony(td).await
}

async fn rescan_templates(h: &ColonyHandle, templates_root: std::path::PathBuf) {
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
}

/// Submit one diff in `scope` and return the colony's verdict.
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

/// Grow the screen at 1.0.0 as `/alex/display` and wire it to the persona:
/// `sender -> display` states the `in_view` lane, `display -> capture` takes
/// whatever leaves the screen. One composite diff, `add_nodes` before
/// `add_edges` by the apply order.
///
/// A hive transit evaluates EVERY out-edge of the hive path — the inner
/// `. -> ./keep` and the outer `display -> capture` alike — so the outer edge
/// is conditioned on the message having left a lane behind: the echo children
/// emit no `hop.route`, an inbound lane always carries one. Without the
/// condition the raw `in_view` probe would fan out to the sink directly,
/// beside the copy that walked the chain.
async fn grow_screen(h: &ColonyHandle) -> MutationOutcome {
    submit_diff(
        h,
        "/alex",
        json!({
            "add_nodes": [{"name": "display", "template": "screen@1.0.0"}],
            "add_edges": [
                {"from": "./sender", "to": "./display",
                 "modifier": {"set_hop": {"route": "'in_view'"}}},
                {"from": "./display", "to": "./capture",
                 "condition": "!has(hop.route)"}
            ]
        }),
    )
    .await
}

/// The versions the library holds for the class `name`, sorted.
async fn library_versions(h: &ColonyHandle, name: &str) -> Vec<String> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadTemplatesReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadTemplates {
            cell_type: None,
            name: Some(name.to_string()),
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let mut versions: Vec<String> = ack_rx
        .await
        .unwrap()
        .entries
        .into_iter()
        .filter_map(|t| t.version)
        .collect();
    versions.sort();
    versions
}

/// Every registry row whose path starts with `prefix`, sorted by path.
async fn registry_rows(h: &ColonyHandle, prefix: &str) -> Vec<RegistryEntryDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: Some(Path::new(prefix)),
            cell_type: None,
            active: None,
            limit: 200,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let mut rows = ack_rx.await.unwrap().entries;
    rows.sort_by(|a, b| a.path.cmp(&b.path));
    rows
}

/// The registry row at exactly `path`, if the colony has one.
async fn registry_row(h: &ColonyHandle, path: &str) -> Option<RegistryEntryDto> {
    registry_rows(h, path)
        .await
        .into_iter()
        .find(|e| e.path == path)
}

/// The persisted edges whose endpoints both lie inside `scope`, verbatim
/// (condition and modifier included).
async fn edges_of(h: &ColonyHandle, scope: &str) -> Vec<GraphEdgeDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new(scope),
            ack: ack_tx,
        })
        .await
        .unwrap();
    ack_rx.await.unwrap().edges
}

/// `(from, to)` pairs of [`edges_of`], for the assertions that do not care
/// about conditions.
async fn edge_pairs(h: &ColonyHandle, scope: &str) -> Vec<(String, String)> {
    edges_of(h, scope)
        .await
        .into_iter()
        .map(|e| (e.from, e.to))
        .collect()
}

/// A source message on the lane `lane`, addressed at `target`.
fn probe(lane: &str, target: &str, text: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!(lane));
    MessageBuilder::new(Path::new(target))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":text}]}),
        ))
        .hop(hop)
        .ttl(16)
        .build()
}

/// Wait for the sink to receive, or fail with the dead letters in hand
/// (failure-marker timeout, generous by convention).
async fn expect_capture(colony: &mut Colony, why: &str) -> Message {
    match tokio::time::timeout(Duration::from_secs(30), colony.capture_rx.recv()).await {
        Ok(Some(m)) => m,
        Ok(None) => panic!("/alex/capture rx closed ({why})"),
        Err(_) => {
            let dlq = colony.h.drain_dead_letters().await;
            panic!("/alex/capture must receive within 30s ({why}); DLQ: {dlq:?}");
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Task 1 — the fixture itself
// ─────────────────────────────────────────────────────────────────────────────

/// The screen grows at 1.0.0: its three children stand active in the
/// registry, the outer edges are in place, and an `in_view` message at the
/// hive path walks `keep -> bump -> gone` and leaves the screen into
/// `/alex/capture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_fixture_grows_a_screen_at_one_zero_zero() {
    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_screen_templates(td.path());
    let mut colony = start_colony(&td).await;

    // Both versions stand in the library as one class (GH #664) — the 1.1.0
    // half of the fixture is scanned here even though only 1.0.0 is grown.
    assert_eq!(
        library_versions(&colony.h, "screen").await,
        vec!["1.0.0", "1.1.0"],
        "the library must hold screen in exactly the two fixture versions"
    );

    let outcome = grow_screen(&colony.h).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "growing screen@1.0.0 must commit; got {outcome:?}"
    );

    // The children stand, and they are awake — the hive carries both outer
    // edges, so its inside is connected.
    let rows = registry_rows(&colony.h, "/alex/display/").await;
    let paths: Vec<&str> = rows.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "/alex/display/bump",
            "/alex/display/gone",
            "/alex/display/keep"
        ],
        "screen@1.0.0 has exactly the children keep, bump, gone"
    );
    for row in &rows {
        assert!(row.active, "{} must be active after the grow", row.path);
        assert_eq!(row.cell_type, "echo_sub");
    }

    // The outer edges and the inner chain, as persisted.
    let pairs = edge_pairs(&colony.h, "/alex").await;
    for (from, to) in [
        ("/alex/sender", "/alex/display"),
        ("/alex/display", "/alex/capture"),
        ("/alex/display", "/alex/display/keep"),
        ("/alex/display/keep", "/alex/display/bump"),
        ("/alex/display/bump", "/alex/display/gone"),
        ("/alex/display/gone", "/alex/display"),
    ] {
        assert!(
            pairs.contains(&(from.to_string(), to.to_string())),
            "edge {from} -> {to} must be persisted; got {pairs:?}"
        );
    }
    let sender_edge = edges_of(&colony.h, "/alex")
        .await
        .into_iter()
        .find(|e| e.from == "/alex/sender" && e.to == "/alex/display")
        .expect("the sender edge is persisted");
    assert_eq!(
        sender_edge
            .modifier
            .as_ref()
            .and_then(|m| m.pointer("/set_hop/route"))
            .and_then(|v| v.as_str()),
        Some("'in_view'"),
        "the sender edge states the in_view lane verbatim"
    );

    // Positive receipt: an in_view message at the hive path walks the chain
    // and leaves into the sink.
    colony
        .h
        .send(probe("in_view", "/alex/display", "fixture_probe"))
        .await;
    let got = expect_capture(&mut colony, "in_view through keep -> bump -> gone").await;
    assert_eq!(got.target, Path::new("/alex/capture"));
    let texts: Vec<String> = match &got.body {
        Body::Inline(v) => v["messages"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m["text"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    assert_eq!(
        texts,
        vec![
            "fixture_probe",
            "echo from /alex/display/keep",
            "echo from /alex/display/bump",
            "echo from /alex/display/gone",
        ],
        "the probe must have walked keep -> bump -> gone in that order"
    );

    // Nothing else may have been refused on the way.
    let dlq = colony.h.drain_dead_letters().await;
    assert!(dlq.is_empty(), "no dead letters expected; got {dlq:?}");

    colony.h.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Task 2 — the door
// ─────────────────────────────────────────────────────────────────────────────

/// One `replace_nodes` diff with the given entry, submitted in `/alex`.
async fn submit_replace(h: &ColonyHandle, entry: JsonValue) -> MutationOutcome {
    submit_diff(h, "/alex", json!({"replace_nodes": [entry]})).await
}

/// The refusal's `error_code` and rendered details, or a panic naming the
/// outcome that was not a refusal.
fn refused(outcome: MutationOutcome, why: &str) -> (String, String) {
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => (error_code, details),
        other => panic!("{why}: expected a refusal, got {other:?}"),
    }
}

/// `replace_nodes` is the ninth key the door reads, and its form is
/// `{match: {name}, with: {template, params?}}` — no `with.name`, because
/// the node keeps its path. This test measures the DOOR only: a well-formed
/// entry against the standing fixture is let through, and each malformed
/// form is refused under the code the spec names. What the door then does
/// with the entry (the lift itself) is not pinned here — the staging and
/// apply arms come in later steps of the same wave, and a `committed` here
/// says only that validation found nothing to refuse.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replace_nodes_is_a_door_and_names_its_form() {
    // The vocabulary: nine keys, `replace_nodes` right after `swap_nodes`.
    assert_eq!(DIFF_OPERATIONS.len(), 9, "the door reads nine operations");
    let swap = DIFF_OPERATIONS
        .iter()
        .position(|k| *k == "swap_nodes")
        .expect("swap_nodes is a door");
    assert_eq!(
        DIFF_OPERATIONS.get(swap + 1).copied(),
        Some("replace_nodes"),
        "replace_nodes stands right after swap_nodes in the door's list"
    );

    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_screen_templates(td.path());
    let colony = start_colony(&td).await;
    let outcome = grow_screen(&colony.h).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "growing screen@1.0.0 must commit; got {outcome:?}"
    );

    // 1 — the well-formed entry: `match.name` is the standing HIVE `display`
    //     (no registry row of its own — it resolves through the hive scopes),
    //     `with.template` is the other version the library holds.
    let outcome = submit_replace(
        &colony.h,
        json!({"match": {"name": "display"}, "with": {"template": "screen@1.1.0"}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "a well-formed replace_nodes entry passes the door; got {outcome:?}"
    );

    // 2 — `with.name` is not a field: the node keeps its path.
    let (code, details) = refused(
        submit_replace(
            &colony.h,
            json!({"match": {"name": "display"},
                   "with": {"template": "screen@1.1.0", "name": "display2"}}),
        )
        .await,
        "with.name",
    );
    assert_eq!(code, "schema", "with.name is a schema refusal: {details}");
    assert!(
        details.contains("replace_nodes[].with.name is not a field: the node keeps its path"),
        "the refusal says why the field does not exist: {details}"
    );

    // 3 — `with.template` is required.
    let (code, details) = refused(
        submit_replace(
            &colony.h,
            json!({"match": {"name": "display"}, "with": {"params": {}}}),
        )
        .await,
        "with.template missing",
    );
    assert_eq!(
        code, "schema",
        "a missing template is a schema refusal: {details}"
    );
    assert!(
        details.contains("replace_nodes[].with.template missing"),
        "the refusal names the missing key: {details}"
    );

    // 4 — a template the library does not hold.
    let (code, details) = refused(
        submit_replace(
            &colony.h,
            json!({"match": {"name": "display"}, "with": {"template": "screen@9.9.9"}}),
        )
        .await,
        "unknown template",
    );
    assert_eq!(
        code, "template_missing",
        "an unknown template is template_missing: {details}"
    );

    // 4b — `with.params` is the override_params form of the template it
    //      lifts to. On a subtree template that form is keyed by the cells'
    //      paths, so a key naming no cell of `screen@1.1.0` is refused and the
    //      refusal lists the cells that exist. This is the one path of the door
    //      `swap_nodes` can never take: its instantiate form refuses a subtree
    //      template before any param is read.
    let (code, details) = refused(
        submit_replace(
            &colony.h,
            json!({"match": {"name": "display"},
                   "with": {"template": "screen@1.1.0", "params": {"nope": {}}}}),
        )
        .await,
        "with.params addressing no cell",
    );
    assert_eq!(
        code, "schema",
        "an unaddressable params key is a schema refusal: {details}"
    );
    assert!(
        details.contains(
            "override_params['nope'] names no cell of the subtree template 'screen@1.1.0'"
        ),
        "the refusal names the key that reached nothing: {details}"
    );
    for cell in ["keep", "bump", "fresh"] {
        assert!(
            details.contains(cell),
            "the refusal lists the cell '{cell}' the template does have: {details}"
        );
    }

    // 4c — and any other key inside `with` is not the form either.
    let (code, details) = refused(
        submit_replace(
            &colony.h,
            json!({"match": {"name": "display"},
                   "with": {"template": "screen@1.1.0", "foo": 1}}),
        )
        .await,
        "with.foo",
    );
    assert_eq!(
        code, "schema",
        "an unknown with key is a schema refusal: {details}"
    );
    assert!(
        details.contains("replace_nodes[].with unknown key 'foo'"),
        "the refusal names the key: {details}"
    );

    // 5 — `match.name` must hit something that stands in the scope.
    let (code, details) = refused(
        submit_replace(
            &colony.h,
            json!({"match": {"name": "nothing"}, "with": {"template": "screen@1.1.0"}}),
        )
        .await,
        "match.name hits nothing",
    );
    assert_eq!(
        code, "match_no_hit",
        "an absent node is match_no_hit: {details}"
    );

    // 6 — and the name is bounded by the scope like every other top-level
    //     name in a diff: a `..` cannot reach a node outside `/alex`.
    let (code, details) = refused(
        submit_replace(
            &colony.h,
            json!({"match": {"name": "../world"}, "with": {"template": "screen@1.1.0"}}),
        )
        .await,
        "match.name escapes the scope",
    );
    assert_eq!(
        code, "scope_out_of_bounds",
        "a name escaping the scope is refused before anything else: {details}"
    );

    colony.h.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Task 4 — staging: the lift is prepared beside the live tree, nothing moves
// ─────────────────────────────────────────────────────────────────────────────

/// The library as staging sees it: the given `(class, version)` pairs, by
/// the paths the fixture writers put them at (`templates/local/<class>@<v>`).
fn library(root: &std::path::Path, classes: &[(&str, &str)]) -> TemplatesRegistry {
    TemplatesRegistry::from_entries(
        classes
            .iter()
            .map(|(name, v)| TemplateEntry {
                template_id: format!("{name}-{v}"),
                name: (*name).to_string(),
                version: Some((*v).to_string()),
                filesystem_path: root.join(format!("templates/local/{name}@{v}")),
            })
            .collect(),
    )
}

/// The library holding both fixture versions of `screen`.
fn screen_registry(root: &std::path::Path) -> TemplatesRegistry {
    library(root, &[("screen", "1.0.0"), ("screen", "1.1.0")])
}

/// A LEAF class in two versions, `note@1.0.0` and `note@1.1.0` — one
/// `echo_sub` cell each, the contract version being the diff.
fn write_note_templates(root: &std::path::Path) {
    for v in ["1.0.0", "1.1.0"] {
        let dir = root.join(format!("templates/local/note@{v}"));
        write(
            &dir,
            "template.json",
            &format!(r#"{{"name":"note","version":"{v}"}}"#),
        );
        write(&dir, "config.json", &child(v));
    }
}

/// A `config.json` read back as JSON.
fn read_config(path: &std::path::Path) -> JsonValue {
    let raw =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap()
}

/// `cell.id` of a config, as a string.
fn cell_id_of(cfg: &JsonValue) -> String {
    cfg["cell"]["id"]
        .as_str()
        .expect("config carries cell.id")
        .to_string()
}

/// `cell.provenance.template_version` of a config.
fn stamped_version(cfg: &JsonValue) -> String {
    cfg["cell"]["provenance"]["template_version"]
        .as_str()
        .expect("config carries a provenance stamp")
        .to_string()
}

/// The entries of `<root>/.staging`, sorted — empty when there is none.
fn staging_entries(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(root.join(".staging"))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    entries.sort();
    entries
}

/// Stage one `replace_nodes` lift of `display` to `screen@1.1.0` over a root
/// a colony grew and then left (the staging-inspection form: the plan and the
/// `.staging` tree are read directly, the way the `build_staging_tree_*`
/// tests do). `params` is the entry's `with.params`, if any.
fn stage_lift(
    root: &std::path::Path,
    mutation_id: &str,
    params: Option<JsonValue>,
) -> Result<Vec<StagedReplace>, meclaw_colony::mutation::MutationError> {
    let mut with = json!({"template": "screen@1.1.0"});
    if let Some(p) = params {
        with["params"] = p;
    }
    stage_replace_nodes(
        root,
        mutation_id,
        "/alex",
        &json!({"replace_nodes": [{"match": {"name": "display"}, "with": with}]}),
        &screen_registry(root),
        &HashMap::new(),
        &HashMap::new(),
        &factory_registry(),
        &meclaw_colony::WorkPulse::silent(),
    )
}

/// Staging a lift of the standing `screen@1.0.0` to `1.1.0` prepares exactly
/// what the apply will need and touches nothing that stands: `fresh` (added)
/// and `bump` (changed) are instantiated into `.staging` with fresh `cell.id`s
/// and a 1.1.0 stamp; the plan carries one rename `bump -> bump~1.0.0` for the
/// old directory (its whole subtree, identity kept); `keep` has no staging
/// entry at all; `gone` is listed as left; and the hive's own renewed
/// declaration lies beside them as `config.json.replace` — the 1.1.0 contract
/// (`accepts` gains `in_notice`) under the OLD hive `cell.id`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn staging_prepares_added_and_changed_and_renames_the_old() {
    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_screen_templates(td.path());
    let colony = start_colony(&td).await;
    assert!(
        matches!(
            grow_screen(&colony.h).await,
            MutationOutcome::Committed { .. }
        ),
        "growing screen@1.0.0 must commit"
    );
    colony.h.shutdown().await;

    let display = td.path().join("main/alex/display");
    let old_hive = read_config(&display.join("config.json"));
    let old_hive_id = cell_id_of(&old_hive);
    let old_bump_id = cell_id_of(&read_config(&display.join("bump/config.json")));
    let keep_before = std::fs::read(display.join("keep/config.json")).unwrap();

    let plan = stage_lift(td.path(), "m-lift", None).expect("staging the lift");
    assert_eq!(plan.len(), 1, "one entry, one staged lift");
    let lift = &plan[0];
    let staging = td.path().join(".staging/m-lift/display");

    // The lifted node and where its staging lives.
    assert_eq!(lift.absolute_path.as_str(), "/alex/display");
    assert_eq!(lift.final_path, display);
    assert_eq!(lift.root_staging_path, staging);
    assert_eq!(lift.provenance.template, "screen");
    assert_eq!(lift.provenance.template_version.as_deref(), Some("1.1.0"));

    // ADDED: `fresh`, staged whole, fresh id, stamped 1.1.0, aimed at a
    // directory that does not exist yet.
    assert_eq!(lift.added.len(), 1, "one added child: {:?}", lift.added);
    let fresh = &lift.added[0];
    assert_eq!(fresh.root_staging_path, staging.join("fresh"));
    assert_eq!(fresh.root_final_path, display.join("fresh"));
    assert!(
        !fresh.root_final_path.exists(),
        "nothing moved into the live tree"
    );
    let fresh_cfg = read_config(&fresh.root_staging_path.join("config.json"));
    assert_eq!(stamped_version(&fresh_cfg), "1.1.0");
    assert_eq!(fresh_cfg["cell"]["provenance"]["template"], "screen");
    assert_eq!(fresh.cells.len(), 1);
    assert_eq!(fresh.cells[0].absolute_path.as_str(), "/alex/display/fresh");
    assert_eq!(fresh.cells[0].cell_type, "echo_sub");

    // CHANGED: `bump`, the new one staged under its own name, the old one
    // planned aside as `bump~1.0.0` — logical path and directory alike.
    assert_eq!(
        lift.changed.len(),
        1,
        "one changed child: {:?}",
        lift.changed
    );
    let bump = &lift.changed[0];
    assert_eq!(bump.from_version, "1.0.0");
    assert_eq!(bump.to_version, "1.1.0");
    assert_eq!(bump.aside.from.as_str(), "/alex/display/bump");
    assert_eq!(bump.aside.to.as_str(), "/alex/display/bump~1.0.0");
    assert_eq!(bump.aside.from_dir, display.join("bump"));
    assert_eq!(bump.aside.to_dir, display.join("bump~1.0.0"));
    assert_eq!(bump.fresh.root_staging_path, staging.join("bump"));
    assert_eq!(bump.fresh.root_final_path, display.join("bump"));
    let bump_cfg = read_config(&bump.fresh.root_staging_path.join("config.json"));
    assert_ne!(
        cell_id_of(&bump_cfg),
        old_bump_id,
        "the new bump is a new cell"
    );
    assert_eq!(stamped_version(&bump_cfg), "1.1.0");
    assert_eq!(bump_cfg["contract"]["version"], "1.1.0");
    assert_eq!(bump.fresh.cells.len(), 1);
    assert_eq!(
        bump.fresh.cells[0].absolute_path.as_str(),
        "/alex/display/bump"
    );

    // KEPT: `keep` is untouched — no staging entry, same bytes.
    assert_eq!(lift.kept.len(), 1, "one kept child: {:?}", lift.kept);
    assert_eq!(lift.kept[0].absolute_path.as_str(), "/alex/display/keep");
    assert!(
        !staging.join("keep").exists(),
        "a kept child is not staged (F1)"
    );
    assert_eq!(
        std::fs::read(display.join("keep/config.json")).unwrap(),
        keep_before
    );

    // LEFT: `gone` — receipt material, nothing staged.
    let left: Vec<&str> = lift.left.iter().map(|l| l.rel_path.as_str()).collect();
    assert_eq!(left, vec!["gone"]);
    assert_eq!(lift.left[0].abs_path, "/alex/display/gone");
    assert_eq!(lift.left[0].version, "1.0.0");

    // The hive's own renewed declaration: the 1.1.0 contract under the OLD id,
    // stamped with the new version, staged as `config.json.replace` — and not
    // as a `config.json` that a rename could take for a fresh hive.
    let decl = lift
        .declaration
        .as_ref()
        .expect("a lifted hive renews its declaration");
    assert_eq!(decl.staging_path, staging.join("config.json.replace"));
    assert_eq!(decl.final_path, display.join("config.json"));
    assert!(!staging.join("config.json").exists());
    let renewed = read_config(&decl.staging_path);
    assert_eq!(
        cell_id_of(&renewed),
        old_hive_id,
        "the hive keeps its identity"
    );
    assert_eq!(stamped_version(&renewed), "1.1.0");
    let routes: Vec<&str> = renewed["params"]["contract"]["accepts"]
        .as_array()
        .expect("accepts")
        .iter()
        .filter_map(|a| a["route"].as_str())
        .collect();
    assert_eq!(routes, vec!["in_view", "in_notice"]);
    assert_eq!(renewed["cell"]["type"], "hive");

    // The new inner graph, resolved — for the apply to lay in place of the old.
    let mut inner: Vec<(String, String)> = lift
        .internal_edges
        .iter()
        .map(|e| (e.from.as_str().to_string(), e.to.as_str().to_string()))
        .collect();
    inner.sort();
    assert_eq!(
        inner,
        vec![
            (
                "/alex/display".to_string(),
                "/alex/display/fresh".to_string()
            ),
            (
                "/alex/display".to_string(),
                "/alex/display/keep".to_string()
            ),
            (
                "/alex/display/bump".to_string(),
                "/alex/display".to_string()
            ),
            (
                "/alex/display/fresh".to_string(),
                "/alex/display".to_string()
            ),
            (
                "/alex/display/keep".to_string(),
                "/alex/display/bump".to_string()
            ),
        ]
    );

    // And the live tree is exactly what it was.
    assert_eq!(
        cell_id_of(&read_config(&display.join("bump/config.json"))),
        old_bump_id
    );
    assert_eq!(read_config(&display.join("config.json")), old_hive);
    assert!(!display.join("bump~1.0.0").exists());
    assert!(!display.join("fresh").exists());
    assert!(display.join("gone").exists());
}

/// A second lift from the same version finds the suffix path `bump~1.0.0`
/// taken — and takes the next free name, `bump~1.0.0~2` (Spec OR-P2: the
/// suffix scheme is a constant; the round trip must not be blocked by its
/// own history). Through the colony: the lift COMMITS, the old bump stands
/// at `bump~1.0.0~2` on disk and in the registry (inactive, its identity
/// kept), the pre-existing `bump~1.0.0` is untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_lift_from_the_same_version_takes_the_next_free_name() {
    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_screen_templates(td.path());
    let colony = start_colony(&td).await;
    assert!(
        matches!(
            grow_screen(&colony.h).await,
            MutationOutcome::Committed { .. }
        ),
        "growing screen@1.0.0 must commit"
    );
    let display = td.path().join("main/alex/display");
    // What an earlier lift from 1.0.0 leaves behind.
    write(&display, "bump~1.0.0/config.json", &child("1.0.0"));
    let taken_before = std::fs::read(display.join("bump~1.0.0/config.json")).unwrap();
    let bump_config_id = cell_id_of(&read_config(&display.join("bump/config.json")));
    let bump_row_id = registry_row(&colony.h, "/alex/display/bump")
        .await
        .unwrap()
        .cell_id;

    let outcome = submit_replace(
        &colony.h,
        json!({"match": {"name": "display"}, "with": {"template": "screen@1.1.0"}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "the suffix path being taken does not block the lift; got {outcome:?}"
    );

    assert_eq!(
        std::fs::read(display.join("bump~1.0.0/config.json")).unwrap(),
        taken_before,
        "the pre-existing bump~1.0.0 is untouched"
    );
    assert_eq!(
        cell_id_of(&read_config(&display.join("bump~1.0.0~2/config.json"))),
        bump_config_id,
        "the old bump went aside under the next free name"
    );
    let aside = registry_row(&colony.h, "/alex/display/bump~1.0.0~2")
        .await
        .expect("the moved row stands at the next free name");
    assert_eq!(aside.cell_id, bump_row_id);
    assert!(!aside.active);
    assert!(
        registry_row(&colony.h, "/alex/display/bump~1.0.0")
            .await
            .is_none(),
        "a directory without a row gets no row from the lift"
    );

    colony.h.shutdown().await;
}

/// A leaf is lifted the same way, with itself as the one changed node: the
/// standing `note` (a `note@1.0.0` instance) becomes the rename root of its
/// own lift — new instantiation staged under its name, the old one planned
/// aside as `note~1.0.0`, no declaration to renew (the node IS the template's
/// single cell), no inner edges, nothing added, kept or left. `with.params`
/// takes the flat form of a single-cell template and reaches the new cell.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_leaf_is_lifted_as_its_own_changed_node() {
    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_screen_templates(td.path());
    write_note_templates(td.path());
    let colony = start_colony(&td).await;
    let grown = submit_diff(
        &colony.h,
        "/alex",
        json!({"add_nodes": [{"name": "note", "template": "note@1.0.0"}]}),
    )
    .await;
    assert!(
        matches!(grown, MutationOutcome::Committed { .. }),
        "growing note@1.0.0 must commit; got {grown:?}"
    );
    colony.h.shutdown().await;

    let note = td.path().join("main/alex/note");
    let old_id = cell_id_of(&read_config(&note.join("config.json")));

    let plan = stage_replace_nodes(
        td.path(),
        "m-leaf",
        "/alex",
        &json!({"replace_nodes": [{"match": {"name": "note"},
                                   "with": {"template": "note@1.1.0",
                                            "params": {"echo_to": "/alex/capture"}}}]}),
        &library(td.path(), &[("note", "1.0.0"), ("note", "1.1.0")]),
        &HashMap::new(),
        &HashMap::new(),
        &factory_registry(),
        &meclaw_colony::WorkPulse::silent(),
    )
    .expect("staging the leaf lift");
    assert_eq!(plan.len(), 1);
    let lift = &plan[0];
    assert_eq!(lift.absolute_path.as_str(), "/alex/note");
    assert!(
        lift.declaration.is_none(),
        "a leaf has no declaration apart from itself"
    );
    assert!(lift.internal_edges.is_empty());
    assert!(lift.added.is_empty() && lift.kept.is_empty() && lift.left.is_empty());

    assert_eq!(
        lift.changed.len(),
        1,
        "the leaf itself is the changed node: {:?}",
        lift.changed
    );
    let c = &lift.changed[0];
    assert_eq!(
        (c.from_version.as_str(), c.to_version.as_str()),
        ("1.0.0", "1.1.0")
    );
    assert_eq!(c.aside.from.as_str(), "/alex/note");
    assert_eq!(c.aside.to.as_str(), "/alex/note~1.0.0");
    assert_eq!(c.aside.from_dir, note);
    assert_eq!(c.aside.to_dir, td.path().join("main/alex/note~1.0.0"));
    assert_eq!(
        c.fresh.root_staging_path,
        td.path().join(".staging/m-leaf/note")
    );
    assert_eq!(c.fresh.root_final_path, note);
    let staged = read_config(&c.fresh.root_staging_path.join("config.json"));
    assert_ne!(cell_id_of(&staged), old_id);
    assert_eq!(stamped_version(&staged), "1.1.0");
    assert_eq!(staged["contract"]["version"], "1.1.0");
    assert_eq!(
        staged["params"]["echo_to"], "/alex/capture",
        "the flat with.params of a leaf reach the new cell"
    );
    assert_eq!(c.fresh.cells.len(), 1);
    assert_eq!(c.fresh.cells[0].params["echo_to"], "/alex/capture");

    // The live leaf is untouched, its suffix path still free.
    assert_eq!(cell_id_of(&read_config(&note.join("config.json"))), old_id);
    assert!(!td.path().join("main/alex/note~1.0.0").exists());

    // And a name nothing stands at on disk is not lifted into existence: the
    // door resolves names against the registry, staging against the tree, and
    // a node that is not there is a grow, not a lift.
    let err = stage_replace_nodes(
        td.path(),
        "m-ghost",
        "/alex",
        &json!({"replace_nodes": [{"match": {"name": "ghost"},
                                   "with": {"template": "note@1.1.0"}}]}),
        &library(td.path(), &[("note", "1.0.0"), ("note", "1.1.0")]),
        &HashMap::new(),
        &HashMap::new(),
        &factory_registry(),
        &meclaw_colony::WorkPulse::silent(),
    )
    .expect_err("nothing stands at ghost");
    assert_eq!(err.error_code(), "schema", "{err:?}");
    assert!(
        err.message().contains("nothing stands at"),
        "{}",
        err.message()
    );
    assert!(!td.path().join(".staging/m-ghost").exists());
}

/// A child at the given contract version that ALSO declares `owner` as an
/// `operator_set` param (ADR-0032) with an empty shipped default.
fn owned_child(version: &str) -> String {
    format!(
        r#"{{"cell":{{"type":"echo_sub"}},"params":{{"echo_to":"/capture","owner":""}},
            "contract":{{"version":"{version}","consumes":{{}},"settings":{{
                "owner":{{"type":"string","operator_set":true,"default":"",
                          "description":"whose memo this is"}}}}}}}}"#
    )
}

/// Two classes whose 1.1.0 introduces an `operator_set` param: `memo`, a hive
/// with one child `pad` (the addressed form), and `jot`, a leaf (the flat
/// form). Their 1.0.0 are the plain fixture cells.
fn write_operator_set_templates(root: &std::path::Path) {
    let hive = |edges: &str| {
        format!(r#"{{"cell":{{"type":"hive"}},"params":{{"graph":{{"edges":[{edges}]}}}}}}"#)
    };
    let m1 = root.join("templates/local/memo@1.0.0");
    write(&m1, "template.json", r#"{"name":"memo","version":"1.0.0"}"#);
    write(
        &m1,
        "config.json",
        &hive(r#"{"from":".","to":"./pad"},{"from":"./pad","to":"."}"#),
    );
    write(&m1, "pad/config.json", &child("1.0.0"));
    let m2 = root.join("templates/local/memo@1.1.0");
    write(&m2, "template.json", r#"{"name":"memo","version":"1.1.0"}"#);
    write(
        &m2,
        "config.json",
        &hive(r#"{"from":".","to":"./pad"},{"from":"./pad","to":"."}"#),
    );
    write(&m2, "pad/config.json", &owned_child("1.1.0"));

    let j1 = root.join("templates/local/jot@1.0.0");
    write(&j1, "template.json", r#"{"name":"jot","version":"1.0.0"}"#);
    write(&j1, "config.json", &child("1.0.0"));
    let j2 = root.join("templates/local/jot@1.1.0");
    write(&j2, "template.json", r#"{"name":"jot","version":"1.1.0"}"#);
    write(&j2, "config.json", &owned_child("1.1.0"));
}

/// Stage one lift of `name` to `template` with the given `with.params`, over
/// the library of the operator-set fixture.
fn stage_owned_lift(
    root: &std::path::Path,
    mutation_id: &str,
    name: &str,
    template: &str,
    params: Option<JsonValue>,
) -> Result<Vec<StagedReplace>, meclaw_colony::mutation::MutationError> {
    let mut with = json!({"template": template});
    if let Some(p) = params {
        with["params"] = p;
    }
    stage_replace_nodes(
        root,
        mutation_id,
        "/alex",
        &json!({"replace_nodes": [{"match": {"name": name}, "with": with}]}),
        &library(
            root,
            &[
                ("memo", "1.0.0"),
                ("memo", "1.1.0"),
                ("jot", "1.0.0"),
                ("jot", "1.1.0"),
            ],
        ),
        &HashMap::new(),
        &HashMap::new(),
        &factory_registry(),
        &meclaw_colony::WorkPulse::silent(),
    )
}

/// A target version that declares a param `operator_set` which the standing
/// instance never had: the lift must bring a value for it, in the addressed
/// form on a hive (`override_params['pad']['owner']`) and in the flat form on
/// a leaf (`override_params['owner']`). Refused as `operator_param_unset`
/// before anything is staged — the standing instance's params are not read,
/// so silence would spawn the shipped default (ADR-0032). With the value
/// given, the new child carries it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_operator_set_param_of_the_target_version_must_be_given() {
    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_screen_templates(td.path());
    write_operator_set_templates(td.path());
    let colony = start_colony(&td).await;
    let grown = submit_diff(
        &colony.h,
        "/alex",
        json!({"add_nodes": [
            {"name": "memo", "template": "memo@1.0.0"},
            {"name": "jot", "template": "jot@1.0.0"}
        ]}),
    )
    .await;
    assert!(
        matches!(grown, MutationOutcome::Committed { .. }),
        "growing memo@1.0.0 and jot@1.0.0 must commit; got {grown:?}"
    );
    colony.h.shutdown().await;

    // The hive, addressed form.
    let err = stage_owned_lift(td.path(), "m-memo-unset", "memo", "memo@1.1.0", None)
        .expect_err("a silent operator_set param is refused");
    assert_eq!(err.error_code(), "operator_param_unset", "{err:?}");
    assert!(
        err.message().contains("override_params['pad']['owner']"),
        "the refusal names the addressed param: {}",
        err.message()
    );
    assert!(
        !td.path().join(".staging/m-memo-unset").exists(),
        "refused before anything is staged"
    );

    // The leaf, flat form.
    let err = stage_owned_lift(td.path(), "m-jot-unset", "jot", "jot@1.1.0", None)
        .expect_err("a silent operator_set param is refused on a leaf too");
    assert_eq!(err.error_code(), "operator_param_unset", "{err:?}");
    assert!(
        err.message().contains("override_params['owner']"),
        "the refusal names the flat param: {}",
        err.message()
    );
    assert!(!td.path().join(".staging/m-jot-unset").exists());

    // With the value given, both lifts stage and the new child carries it.
    let plan = stage_owned_lift(
        td.path(),
        "m-memo",
        "memo",
        "memo@1.1.0",
        Some(json!({"pad": {"owner": "alex"}})),
    )
    .expect("the addressed value satisfies the declaration");
    let pad = &plan[0].changed[0];
    assert_eq!(pad.aside.to.as_str(), "/alex/memo/pad~1.0.0");
    assert_eq!(pad.fresh.cells[0].params["owner"], "alex");
    let plan = stage_owned_lift(
        td.path(),
        "m-jot",
        "jot",
        "jot@1.1.0",
        Some(json!({"owner": "alex"})),
    )
    .expect("the flat value satisfies the declaration");
    let jot = &plan[0].changed[0];
    assert_eq!(jot.aside.to.as_str(), "/alex/jot~1.0.0");
    assert_eq!(jot.fresh.cells[0].params["owner"], "alex");
}

// ─────────────────────────────────────────────────────────────────────────────
// Task 5 — the apply: registry, inner edges, one recompute, and the way back
// ─────────────────────────────────────────────────────────────────────────────

/// The texts a captured message carries, in order.
fn texts_of(got: &Message) -> Vec<String> {
    match &got.body {
        Body::Inline(v) => v["messages"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|m| m["text"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// The edge rows of `scope` with BOTH ends inside `hive` (the hive path
/// included), as `(from, to)` pairs sorted — the inner graph of a hive.
async fn inner_pairs(h: &ColonyHandle, scope: &str, hive: &str) -> Vec<(String, String)> {
    let under = |p: &str| p == hive || p.starts_with(&format!("{hive}/"));
    let mut pairs: Vec<(String, String)> = edges_of(h, scope)
        .await
        .into_iter()
        .filter(|e| under(&e.from) && under(&e.to))
        .map(|e| (e.from, e.to))
        .collect();
    pairs.sort();
    pairs
}

/// One persisted edge row, whole: id, endpoints, condition, modifier, phase,
/// lane. Two readings that compare equal on this are the same rows — a
/// remove-and-reinsert would change the id.
type EdgeRow = (
    String,
    String,
    String,
    Option<String>,
    Option<JsonValue>,
    bool,
    Option<String>,
);

fn edge_row(e: GraphEdgeDto) -> EdgeRow {
    (
        e.id,
        e.from,
        e.to,
        e.condition,
        e.modifier,
        e.is_default,
        e.lane,
    )
}

/// The edge rows of `scope` with exactly ONE end inside `hive` — the outer
/// edges — as whole rows sorted by id, so two readings compare row for row.
async fn outer_rows(h: &ColonyHandle, scope: &str, hive: &str) -> Vec<EdgeRow> {
    let under = |p: &str| p == hive || p.starts_with(&format!("{hive}/"));
    let mut rows: Vec<EdgeRow> = edges_of(h, scope)
        .await
        .into_iter()
        .filter(|e| under(&e.from) != under(&e.to))
        .map(edge_row)
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}

/// The five inner edges of `screen@1.1.0` under `/alex/display`, sorted.
fn inner_of_one_one_zero() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = [
        ("/alex/display", "/alex/display/keep"),
        ("/alex/display", "/alex/display/fresh"),
        ("/alex/display/keep", "/alex/display/bump"),
        ("/alex/display/bump", "/alex/display"),
        ("/alex/display/fresh", "/alex/display"),
    ]
    .iter()
    .map(|(f, t)| ((*f).to_string(), (*t).to_string()))
    .collect();
    v.sort();
    v
}

/// The four inner edges of `screen@1.0.0` under `/alex/display`, sorted.
fn inner_of_one_zero_zero() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = [
        ("/alex/display", "/alex/display/keep"),
        ("/alex/display/keep", "/alex/display/bump"),
        ("/alex/display/bump", "/alex/display/gone"),
        ("/alex/display/gone", "/alex/display"),
    ]
    .iter()
    .map(|(f, t)| ((*f).to_string(), (*t).to_string()))
    .collect();
    v.sort();
    v
}

/// A colony with the screen grown at 1.0.0 and one `in_view` probe through
/// it — so every child is awake and has handled a message before the lift.
async fn grown_and_probed(td: &TempDir) -> Colony {
    write_alex_topology(td.path());
    write_screen_templates(td.path());
    let mut colony = start_colony(td).await;
    assert!(
        matches!(
            grow_screen(&colony.h).await,
            MutationOutcome::Committed { .. }
        ),
        "growing screen@1.0.0 must commit"
    );
    colony
        .h
        .send(probe("in_view", "/alex/display", "before_lift"))
        .await;
    let got = expect_capture(&mut colony, "the 1.0.0 chain before the lift").await;
    assert_eq!(texts_of(&got).len(), 4, "probe + keep + bump + gone");
    colony
}

/// Lift `/alex/display` to the given version through the colony and expect
/// it to commit.
async fn lift_to(h: &ColonyHandle, version: &str) {
    let outcome = submit_replace(
        h,
        json!({"match": {"name": "display"},
               "with": {"template": format!("screen@{version}")}}),
    )
    .await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "lifting display to screen@{version} must commit; got {outcome:?}"
    );
}

/// The registry rows under `/alex/display/` as `(path, active)` pairs.
async fn child_activity(h: &ColonyHandle) -> Vec<(String, bool)> {
    registry_rows(h, "/alex/display/")
        .await
        .into_iter()
        .map(|r| (r.path, r.active))
        .collect()
}

/// The main proof of the wave: a standing `screen@1.0.0` is lifted to
/// `1.1.0` at its path. `keep` keeps its `cell.id` (kept), `bump` is a new
/// cell under its own name with the old one inactive beside it as
/// `bump~1.0.0` (changed), `fresh` stands active (added), `gone` stands
/// inactive (left); the two outer edges are the same rows as before; the
/// inner graph is exactly the five edges of 1.1.0; the hive's own
/// declaration on disk is the 1.1.0 one under the old hive id; and an
/// `in_view` message still walks the screen into `/alex/capture` — through
/// the NEW bump.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_standing_hive_is_lifted_in_place() {
    let td = TempDir::new().unwrap();
    let mut colony = grown_and_probed(&td).await;
    let display = td.path().join("main/alex/display");
    let hive_id_before = cell_id_of(&read_config(&display.join("config.json")));
    let keep_id_before = registry_row(&colony.h, "/alex/display/keep")
        .await
        .expect("keep stands")
        .cell_id;
    let bump_id_before = registry_row(&colony.h, "/alex/display/bump")
        .await
        .expect("bump stands")
        .cell_id;
    let bump_config_id_before = cell_id_of(&read_config(&display.join("bump/config.json")));
    let outer_before = outer_rows(&colony.h, "/alex", "/alex/display").await;
    assert_eq!(
        outer_before.len(),
        2,
        "sender -> display, display -> capture"
    );

    lift_to(&colony.h, "1.1.0").await;

    // The registry, child by child.
    assert_eq!(
        child_activity(&colony.h).await,
        vec![
            ("/alex/display/bump".to_string(), true),
            ("/alex/display/bump~1.0.0".to_string(), false),
            ("/alex/display/fresh".to_string(), true),
            ("/alex/display/gone".to_string(), false),
            ("/alex/display/keep".to_string(), true),
        ],
        "kept and new children active, the old bump and the left gone inactive"
    );
    let keep = registry_row(&colony.h, "/alex/display/keep").await.unwrap();
    assert_eq!(keep.cell_id, keep_id_before, "keep is the same cell (kept)");
    let bump = registry_row(&colony.h, "/alex/display/bump").await.unwrap();
    assert_ne!(bump.cell_id, bump_id_before, "bump is a new cell (changed)");
    let old_bump = registry_row(&colony.h, "/alex/display/bump~1.0.0")
        .await
        .unwrap();
    assert_eq!(
        old_bump.cell_id, bump_id_before,
        "the old bump keeps its identity beside the new one"
    );

    // The disk agrees: the new bump's config is a fresh instantiation, the
    // old one moved whole with its identity, the hive's declaration is the
    // 1.1.0 one under the old hive id. (The registry mints its own row id for
    // every instantiated cell — that is how every door works — so the two
    // ids are compared each against its own before.)
    assert_ne!(
        cell_id_of(&read_config(&display.join("bump/config.json"))),
        bump_config_id_before,
        "the new bump's config is a new instantiation"
    );
    assert_eq!(
        cell_id_of(&read_config(&display.join("bump~1.0.0/config.json"))),
        bump_config_id_before,
        "the old bump's config moved with it"
    );
    assert_eq!(
        stamped_version(&read_config(&display.join("bump/config.json"))),
        "1.1.0"
    );
    let hive = read_config(&display.join("config.json"));
    assert_eq!(cell_id_of(&hive), hive_id_before, "the hive keeps its id");
    assert_eq!(stamped_version(&hive), "1.1.0");
    assert!(
        !display.join("config.json.replace").exists()
            && !td
                .path()
                .join(".staging")
                .join("config.json.replace")
                .exists(),
        "the staged declaration was moved, not copied"
    );

    // Outer edges: the same rows, condition and modifier included.
    assert_eq!(
        outer_rows(&colony.h, "/alex", "/alex/display").await,
        outer_before,
        "the outer edges are untouched — the same rows, id and lane included"
    );
    // Inner edges: exactly the five of 1.1.0.
    assert_eq!(
        inner_pairs(&colony.h, "/alex", "/alex/display").await,
        inner_of_one_one_zero(),
        "the inner graph is the new template's"
    );

    // And the screen still works — through keep and the NEW bump.
    colony
        .h
        .send(probe("in_view", "/alex/display", "after_lift"))
        .await;
    let got = expect_capture(&mut colony, "in_view through keep -> bump after the lift").await;
    assert_eq!(
        texts_of(&got),
        vec![
            "after_lift",
            "echo from /alex/display/keep",
            "echo from /alex/display/bump",
        ],
        "the probe walked keep -> bump and left the screen"
    );
    let dlq = colony.h.drain_dead_letters().await;
    assert!(dlq.is_empty(), "no dead letters expected; got {dlq:?}");

    colony.h.shutdown().await;
}

/// The hive's contract is the new template's from the next mutation on: an
/// `add_edges` stating the `in_notice` lane into `/alex/display` is refused
/// with `hive_contract` while the screen is 1.0.0 (it accepts `in_view`
/// only), and commits after the lift to 1.1.0 — the same diff, the same
/// door, a different answer, because `collect_hive_contracts` reads the
/// declaration the lift swapped in. And the lane works: an `in_notice`
/// message reaches the sink through `fresh`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_new_contract_holds_from_the_next_mutation() {
    let td = TempDir::new().unwrap();
    let mut colony = grown_and_probed(&td).await;
    let notice_edge = json!({
        "add_edges": [
            {"from": "./notifier", "to": "./display",
             "modifier": {"set_hop": {"route": "'in_notice'"}}}
        ]
    });

    let (code, details) = refused(
        submit_diff(&colony.h, "/alex", notice_edge.clone()).await,
        "in_notice into a 1.0.0 screen",
    );
    assert_eq!(
        code, "hive_contract",
        "screen@1.0.0 does not accept in_notice: {details}"
    );

    lift_to(&colony.h, "1.1.0").await;

    let outcome = submit_diff(&colony.h, "/alex", notice_edge).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "screen@1.1.0 accepts in_notice from the next mutation on; got {outcome:?}"
    );
    colony
        .h
        .send(probe("in_notice", "/alex/display", "a_notice"))
        .await;
    let got = expect_capture(&mut colony, "in_notice through fresh after the lift").await;
    assert_eq!(
        texts_of(&got),
        vec!["a_notice", "echo from /alex/display/fresh"],
        "the notice walked the new lane through fresh"
    );

    colony.h.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// GH #688 — a parked child's non-peaceful death is the parked entry's
// ─────────────────────────────────────────────────────────────────────────────

/// Like [`grown_and_probed`], but the 1.0.0 `bump` panics on the stop
/// signal — so the lift's own recompute, which stops the old bump after the
/// new one stands at `/alex/display/bump`, gets a PANIC death reported under
/// the birth path instead of a `Stopped` (the GH #688 window).
async fn grown_with_a_bump_that_panics_on_stop(td: &TempDir) -> Colony {
    write_alex_topology(td.path());
    write_screen_templates(td.path());
    write(
        &td.path().join("templates/local/screen@1.0.0"),
        "bump/config.json",
        r#"{"cell":{"type":"echo_sub"},"params":{"echo_to":"/capture","panic_on_stop":true},"contract":{"version":"1.0.0","settings":{},"consumes":{}}}"#,
    );
    let mut colony = start_colony(td).await;
    assert!(
        matches!(
            grow_screen(&colony.h).await,
            MutationOutcome::Committed { .. }
        ),
        "growing screen@1.0.0 must commit"
    );
    colony
        .h
        .send(probe("in_view", "/alex/display", "before_lift"))
        .await;
    let got = expect_capture(&mut colony, "the 1.0.0 chain before the lift").await;
    assert_eq!(texts_of(&got).len(), 4, "probe + keep + bump + gone");
    colony
}

/// The `(lifecycle_status, active, failed)` of the row at `path`, polled
/// until `until` accepts it or the failure-marker timeout runs out (generous
/// by convention: the death is reported by a watcher task after the lift's
/// ack, so its arrival is not ordered against this read).
async fn row_state_when(
    h: &ColonyHandle,
    path: &str,
    until: impl Fn(&(String, bool, bool)) -> bool,
) -> Option<(String, bool, bool)> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let state = registry_row(h, path)
            .await
            .map(|r| (r.lifecycle_status, r.active, r.failed));
        if state.as_ref().is_some_and(&until) || tokio::time::Instant::now() > deadline {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// GH #688: the old `bump` dies by panic inside the lift's stop window. Its
/// watcher reports the death under `/alex/display/bump` — the path the NEW
/// bump now holds. The death belongs to the parked entry: `bump~1.0.0` is
/// parked (`NotYetSpawned`, inactive, not failed), the new bump's row stays
/// as registered (`Awake`, active — not restarted, not removed), and an
/// `in_view` probe still walks `keep -> bump` into the sink afterwards.
///
/// Without the ownership test at the `CellDied` arm the corridor restarts
/// the parked closure against the new bump's row: the parked row stays
/// `Awake` for good (this test's first wait runs out), and the new row is
/// removed in RAM once the displaced task exits on mailbox close.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_child_that_panics_on_stop_is_parked_not_restarted() {
    let td = TempDir::new().unwrap();
    let mut colony = grown_with_a_bump_that_panics_on_stop(&td).await;
    let bump_id_before = registry_row(&colony.h, "/alex/display/bump")
        .await
        .expect("bump stands")
        .cell_id;

    lift_to(&colony.h, "1.1.0").await;

    // The parked entry takes the death: parked like a peaceful stop would
    // have parked it.
    let parked = row_state_when(&colony.h, "/alex/display/bump~1.0.0", |s| {
        s.0 == "NotYetSpawned"
    })
    .await;
    assert_eq!(
        parked,
        Some(("NotYetSpawned".to_string(), false, false)),
        "the old bump's entry is parked by its own death (inactive, not failed)"
    );
    // The new bump is untouched by it — still the cell the lift registered,
    // awake and active.
    let bump = registry_row(&colony.h, "/alex/display/bump")
        .await
        .expect("the new bump's row is still registered");
    assert_ne!(bump.cell_id, bump_id_before, "it is the new bump");
    assert_eq!(bump.lifecycle_status, "Awake");
    assert!(bump.active && !bump.failed);

    // And it works: the probe walks keep -> the new bump -> out.
    colony
        .h
        .send(probe("in_view", "/alex/display", "after_lift"))
        .await;
    let got = expect_capture(&mut colony, "in_view through keep -> bump after the lift").await;
    assert_eq!(
        texts_of(&got),
        vec![
            "after_lift",
            "echo from /alex/display/keep",
            "echo from /alex/display/bump",
        ],
        "the probe walked keep -> bump and left the screen"
    );
    // The row survived the round trip (a restarted-then-removed row would be
    // gone by now: the displaced task exits the moment its sender is
    // dropped, and its Normal death removes the row).
    assert!(
        registry_row(&colony.h, "/alex/display/bump")
            .await
            .is_some(),
        "the new bump's row is not removed behind its back"
    );
    let dlq = colony.h.drain_dead_letters().await;
    assert!(dlq.is_empty(), "no dead letters expected; got {dlq:?}");

    colony.h.shutdown().await;
}

/// A third version of the class that DROPS `in_view`: accepts `in_notice`
/// only, one child `fresh` on that lane. Written by the test that needs it.
fn write_screen_one_two_zero(root: &std::path::Path) {
    let v3 = root.join("templates/local/screen@1.2.0");
    write(
        &v3,
        "template.json",
        r#"{"name":"screen","version":"1.2.0"}"#,
    );
    write(
        &v3,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{
            "contract":{"accepts":[
                {"route":"in_notice","because":"a short notice, shown once and gone"}
            ]},
            "graph":{"edges":[
                {"from":".","to":"./fresh","condition":"has(hop.route) && hop.route == 'in_notice'"},
                {"from":"./fresh","to":"."}
            ]}}}"#,
    );
    write(&v3, "fresh/config.json", &child("1.0.0"));
}

/// A hive loses no lane somebody hangs on: lifting to `screen@1.2.0`, whose
/// contract drops `in_view`, is refused with `hive_contract` while the outer
/// edge `sender -> display` states that lane — and nothing changes: the
/// registry rows, the edges, the hive's declaration on disk, the `.staging`
/// listing are what they were.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lane_an_outer_edge_uses_cannot_be_dropped() {
    let td = TempDir::new().unwrap();
    write_screen_one_two_zero(td.path());
    let colony = grown_and_probed(&td).await;
    let display = td.path().join("main/alex/display");
    let hive_before = std::fs::read(display.join("config.json")).unwrap();
    let rows_before = registry_rows(&colony.h, "/alex/").await;
    let edges_before = edges_of(&colony.h, "/alex").await;
    let staging_before = staging_entries(td.path());

    let (code, details) = refused(
        submit_replace(
            &colony.h,
            json!({"match": {"name": "display"}, "with": {"template": "screen@1.2.0"}}),
        )
        .await,
        "dropping in_view under a standing in_view edge",
    );
    assert_eq!(code, "hive_contract", "{details}");
    assert!(
        details.contains("in_view") && details.contains("/alex/sender"),
        "the refusal names the lane and the edge that uses it: {details}"
    );

    assert_eq!(
        std::fs::read(display.join("config.json")).unwrap(),
        hive_before
    );
    assert!(!display.join("fresh").exists() && !display.join("bump~1.0.0").exists());
    let rows_after = registry_rows(&colony.h, "/alex/").await;
    assert_eq!(
        rows_after
            .iter()
            .map(|r| (&r.path, r.active))
            .collect::<Vec<_>>(),
        rows_before
            .iter()
            .map(|r| (&r.path, r.active))
            .collect::<Vec<_>>()
    );
    let edges_after = edges_of(&colony.h, "/alex").await;
    assert_eq!(
        edges_after.iter().map(|e| &e.id).collect::<Vec<_>>(),
        edges_before.iter().map(|e| &e.id).collect::<Vec<_>>(),
        "the edge table is untouched, id for id"
    );
    assert_eq!(staging_entries(td.path()), staging_before);

    colony.h.shutdown().await;
}

/// The way back is the same act: after the lift to 1.1.0, `replace_nodes`
/// to `screen@1.0.0` again. `bump` is a new 1.0.0 cell with `bump~1.1.0`
/// inactive beside it (and `bump~1.0.0` still there, left); `gone` is
/// active again with its old identity — the template names it, it stands
/// on disk at the same version and bytes, so it is kept and the 1.0.0
/// edges reconnect it; `fresh` is left, inactive; `keep` is untouched. The
/// inner graph is the four edges of 1.0.0, the outer rows are unchanged,
/// and an `in_view` message walks keep -> bump -> gone again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_way_back_is_the_same_act() {
    let td = TempDir::new().unwrap();
    let mut colony = grown_and_probed(&td).await;
    let display = td.path().join("main/alex/display");
    let outer_before = outer_rows(&colony.h, "/alex", "/alex/display").await;
    let ids_before: HashMap<String, String> = registry_rows(&colony.h, "/alex/display/")
        .await
        .into_iter()
        .map(|r| (r.path, r.cell_id))
        .collect();

    lift_to(&colony.h, "1.1.0").await;
    let bump_at_one_one = registry_row(&colony.h, "/alex/display/bump")
        .await
        .unwrap()
        .cell_id;

    lift_to(&colony.h, "1.0.0").await;

    assert_eq!(
        child_activity(&colony.h).await,
        vec![
            ("/alex/display/bump".to_string(), true),
            ("/alex/display/bump~1.0.0".to_string(), false),
            ("/alex/display/bump~1.1.0".to_string(), false),
            ("/alex/display/fresh".to_string(), false),
            ("/alex/display/gone".to_string(), true),
            ("/alex/display/keep".to_string(), true),
        ],
        "bump fresh at 1.0.0, both old bumps aside, gone back, fresh left"
    );
    let ids_after: HashMap<String, String> = registry_rows(&colony.h, "/alex/display/")
        .await
        .into_iter()
        .map(|r| (r.path, r.cell_id))
        .collect();
    assert_eq!(
        ids_after["/alex/display/keep"],
        ids_before["/alex/display/keep"]
    );
    assert_eq!(
        ids_after["/alex/display/gone"], ids_before["/alex/display/gone"],
        "gone is the same cell, reconnected"
    );
    assert_eq!(
        ids_after["/alex/display/bump~1.0.0"], ids_before["/alex/display/bump"],
        "the first bump still stands aside"
    );
    assert_eq!(
        ids_after["/alex/display/bump~1.1.0"], bump_at_one_one,
        "the 1.1.0 bump went aside under its version"
    );
    assert_ne!(ids_after["/alex/display/bump"], bump_at_one_one);
    assert_ne!(
        ids_after["/alex/display/bump"],
        ids_before["/alex/display/bump"]
    );
    let bump_cfg = read_config(&display.join("bump/config.json"));
    assert_eq!(stamped_version(&bump_cfg), "1.0.0");
    assert_eq!(
        stamped_version(&read_config(&display.join("bump~1.1.0/config.json"))),
        "1.1.0"
    );
    let hive = read_config(&display.join("config.json"));
    assert_eq!(stamped_version(&hive), "1.0.0");
    let routes: Vec<&str> = hive["params"]["contract"]["accepts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["route"].as_str())
        .collect();
    assert_eq!(routes, vec!["in_view"]);

    assert_eq!(
        outer_rows(&colony.h, "/alex", "/alex/display").await,
        outer_before
    );
    assert_eq!(
        inner_pairs(&colony.h, "/alex", "/alex/display").await,
        inner_of_one_zero_zero()
    );

    colony
        .h
        .send(probe("in_view", "/alex/display", "way_back"))
        .await;
    let got = expect_capture(&mut colony, "in_view through keep -> bump -> gone again").await;
    assert_eq!(
        texts_of(&got),
        vec![
            "way_back",
            "echo from /alex/display/keep",
            "echo from /alex/display/bump",
            "echo from /alex/display/gone",
        ]
    );
    let dlq = colony.h.drain_dead_letters().await;
    assert!(dlq.is_empty(), "no dead letters expected; got {dlq:?}");

    colony.h.shutdown().await;
}

/// The lift is durable (the form of Demo f in `paket_2_swap.rs`): after the
/// lift to 1.1.0 the colony is shut down and booted again over the same
/// root and `colony.db`. The same picture comes back — the five children
/// with the same activity and the same ids, the moved row at `bump~1.0.0`
/// among them, the outer rows and the five inner edges — and an `in_view`
/// message still reaches `/alex/capture` through keep and the new bump.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lifted_hive_survives_a_reboot() {
    let td = TempDir::new().unwrap();
    let colony = grown_and_probed(&td).await;
    lift_to(&colony.h, "1.1.0").await;
    let activity_before = child_activity(&colony.h).await;
    let ids_before: Vec<(String, String)> = registry_rows(&colony.h, "/alex/display/")
        .await
        .into_iter()
        .map(|r| (r.path, r.cell_id))
        .collect();
    let outer_before = outer_rows(&colony.h, "/alex", "/alex/display").await;

    let mut colony = reboot(colony, &td).await;

    assert_eq!(
        child_activity(&colony.h).await,
        activity_before,
        "the same children, the same activity, after the reboot"
    );
    let ids_after: Vec<(String, String)> = registry_rows(&colony.h, "/alex/display/")
        .await
        .into_iter()
        .map(|r| (r.path, r.cell_id))
        .collect();
    assert_eq!(ids_after, ids_before, "every row keeps its identity");
    assert_eq!(
        outer_rows(&colony.h, "/alex", "/alex/display").await,
        outer_before
    );
    assert_eq!(
        inner_pairs(&colony.h, "/alex", "/alex/display").await,
        inner_of_one_one_zero()
    );

    colony
        .h
        .send(probe("in_view", "/alex/display", "after_reboot"))
        .await;
    let got = expect_capture(
        &mut colony,
        "in_view through the lifted screen after a reboot",
    )
    .await;
    assert_eq!(
        texts_of(&got),
        vec![
            "after_reboot",
            "echo from /alex/display/keep",
            "echo from /alex/display/bump",
        ]
    );
    let dlq = colony.h.drain_dead_letters().await;
    assert!(dlq.is_empty(), "no dead letters expected; got {dlq:?}");

    colony.h.shutdown().await;
}

/// Ruling T4-C4: a hive is not lifted to a leaf template (nor a leaf to a
/// hive template) — the classes have to match, or the "lift" would plan the
/// whole hive tree aside and put a single cell in its place. Refused as
/// `schema` in staging, where both classes are first known, before a byte
/// is staged: no `.staging/<id>` entry is added, the hive stands as it was.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hive_is_not_lifted_to_a_leaf_template() {
    let td = TempDir::new().unwrap();
    write_note_templates(td.path());
    let colony = grown_and_probed(&td).await;
    let display = td.path().join("main/alex/display");
    let hive_before = std::fs::read(display.join("config.json")).unwrap();
    let staging_before = staging_entries(td.path());

    let (code, details) = refused(
        submit_replace(
            &colony.h,
            json!({"match": {"name": "display"}, "with": {"template": "note@1.1.0"}}),
        )
        .await,
        "a hive lifted to a leaf template",
    );
    assert_eq!(code, "schema", "{details}");
    assert!(
        details.contains("hive") && details.contains("note@1.1.0"),
        "the refusal names both classes: {details}"
    );
    assert_eq!(
        std::fs::read(display.join("config.json")).unwrap(),
        hive_before
    );
    assert!(!td.path().join("main/alex/display~1.0.0").exists());
    assert_eq!(staging_entries(td.path()), staging_before);

    colony.h.shutdown().await;
}

/// A version whose contract PROMISES `in_notice` but whose graph has no door
/// for it — the post-state lane-doors check refuses it. Written by the test
/// that needs it.
fn write_screen_one_three_zero(root: &std::path::Path) {
    let v = root.join("templates/local/screen@1.3.0");
    write(
        &v,
        "template.json",
        r#"{"name":"screen","version":"1.3.0"}"#,
    );
    write(
        &v,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{
            "contract":{"accepts":[
                {"route":"in_view","because":"put this view up on the screen"},
                {"route":"in_notice","because":"a short notice, shown once and gone"}
            ]},
            "graph":{"edges":[
                {"from":".","to":"./keep","condition":"has(hop.route) && hop.route == 'in_view'"},
                {"from":"./keep","to":"./bump"},
                {"from":"./bump","to":"."}
            ]}}}"#,
    );
    write(&v, "keep/config.json", &child("1.0.0"));
    write(&v, "bump/config.json", &child("1.3.0"));
}

/// The rollback: a lift refused AFTER its moves (here by the post-state
/// lane-doors check — `screen@1.3.0` promises `in_notice` and has no door
/// for it; the stop-wiring guard and the death-ack term-timeout take the
/// same way out) leaves the hive exactly as it was: the old bump back under
/// its name with its identity, no `bump~1.0.0` on disk or in the registry,
/// the 1.0.0 declaration byte for byte, the edge table id for id, every
/// child still active — and the screen still works.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lift_refused_after_its_moves_is_undone() {
    let td = TempDir::new().unwrap();
    write_screen_one_three_zero(td.path());
    let mut colony = grown_and_probed(&td).await;
    let display = td.path().join("main/alex/display");
    let hive_before = std::fs::read(display.join("config.json")).unwrap();
    let bump_before = std::fs::read(display.join("bump/config.json")).unwrap();
    let rows_before: Vec<(String, String, bool)> = registry_rows(&colony.h, "/alex/")
        .await
        .into_iter()
        .map(|r| (r.path, r.cell_id, r.active))
        .collect();
    let staging_before = staging_entries(td.path());
    let edge_ids_before: Vec<String> = {
        let mut v: Vec<String> = edges_of(&colony.h, "/alex")
            .await
            .into_iter()
            .map(|e| e.id)
            .collect();
        v.sort();
        v
    };

    let (code, details) = refused(
        submit_replace(
            &colony.h,
            json!({"match": {"name": "display"}, "with": {"template": "screen@1.3.0"}}),
        )
        .await,
        "a promised lane without a door",
    );
    assert_eq!(code, "hive_contract", "{details}");
    assert!(details.contains("in_notice"), "{details}");

    // Disk: as it was.
    assert_eq!(
        std::fs::read(display.join("config.json")).unwrap(),
        hive_before
    );
    assert_eq!(
        std::fs::read(display.join("bump/config.json")).unwrap(),
        bump_before
    );
    assert!(
        !display.join("bump~1.0.0").exists(),
        "the old bump is back under its name"
    );
    assert!(!display.join("fresh").exists());
    assert_eq!(
        staging_entries(td.path()),
        staging_before,
        "a refused lift sweeps its staging"
    );
    // Registry: as it was.
    let rows_after: Vec<(String, String, bool)> = registry_rows(&colony.h, "/alex/")
        .await
        .into_iter()
        .map(|r| (r.path, r.cell_id, r.active))
        .collect();
    assert_eq!(
        rows_after, rows_before,
        "every row, identity and activity back"
    );
    // Edges: as they were.
    let mut edge_ids_after: Vec<String> = edges_of(&colony.h, "/alex")
        .await
        .into_iter()
        .map(|e| e.id)
        .collect();
    edge_ids_after.sort();
    assert_eq!(edge_ids_after, edge_ids_before, "the edge table, id for id");

    // And the screen still works, through the OLD bump.
    colony
        .h
        .send(probe("in_view", "/alex/display", "still_here"))
        .await;
    let got = expect_capture(&mut colony, "in_view after the refused lift").await;
    assert_eq!(
        texts_of(&got),
        vec![
            "still_here",
            "echo from /alex/display/keep",
            "echo from /alex/display/bump",
            "echo from /alex/display/gone",
        ]
    );
    let dlq = colony.h.drain_dead_letters().await;
    assert!(dlq.is_empty(), "no dead letters expected; got {dlq:?}");

    colony.h.shutdown().await;
}

/// The lift can go back and forth: 1.0.0 → 1.1.0 → 1.0.0 → 1.1.0, every
/// step committed. Each changed bump goes aside under a free name — the
/// third lift finds `bump~1.0.0` taken from the first and uses
/// `bump~1.0.0~2` — and the registry shows all three inactive beside the
/// active bump at 1.1.0.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_lift_can_go_back_and_forth() {
    let td = TempDir::new().unwrap();
    let mut colony = grown_and_probed(&td).await;
    lift_to(&colony.h, "1.1.0").await;
    lift_to(&colony.h, "1.0.0").await;
    lift_to(&colony.h, "1.1.0").await;

    assert_eq!(
        child_activity(&colony.h).await,
        vec![
            ("/alex/display/bump".to_string(), true),
            ("/alex/display/bump~1.0.0".to_string(), false),
            ("/alex/display/bump~1.0.0~2".to_string(), false),
            ("/alex/display/bump~1.1.0".to_string(), false),
            ("/alex/display/fresh".to_string(), true),
            ("/alex/display/gone".to_string(), false),
            ("/alex/display/keep".to_string(), true),
        ]
    );
    let display = td.path().join("main/alex/display");
    assert_eq!(
        stamped_version(&read_config(&display.join("bump/config.json"))),
        "1.1.0"
    );
    assert_eq!(
        stamped_version(&read_config(&display.join("bump~1.0.0~2/config.json"))),
        "1.0.0"
    );
    assert_eq!(
        inner_pairs(&colony.h, "/alex", "/alex/display").await,
        inner_of_one_one_zero()
    );
    colony
        .h
        .send(probe("in_view", "/alex/display", "round_trip"))
        .await;
    let got = expect_capture(&mut colony, "in_view after the round trip").await;
    assert_eq!(
        texts_of(&got),
        vec![
            "round_trip",
            "echo from /alex/display/keep",
            "echo from /alex/display/bump",
        ]
    );
    colony.h.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Task 7 — the children are awake, and the lift still goes
// ─────────────────────────────────────────────────────────────────────────────

/// The lifecycle status of every row under `/alex/display/`, as
/// `(path, status)` pairs sorted by path — `"Awake"` means a task is running
/// for that row.
async fn child_lifecycle(h: &ColonyHandle) -> Vec<(String, String)> {
    registry_rows(h, "/alex/display/")
        .await
        .into_iter()
        .map(|r| (r.path, r.lifecycle_status))
        .collect()
}

/// Lift `/alex/display` to `version` while its children run, and fail with
/// the guard's name if the lift's own recompute could not stop a child
/// (`stop_wiring_unavailable`, the #673/#676 class) — that is the one refusal
/// this test is about; any other refusal fails with the outcome in hand.
async fn lift_awake_to(h: &ColonyHandle, version: &str) {
    let outcome = submit_replace(
        h,
        json!({"match": {"name": "display"},
               "with": {"template": format!("screen@{version}")}}),
    )
    .await;
    if let MutationOutcome::Rejected {
        error_code,
        details,
        ..
    } = &outcome
    {
        assert_ne!(
            error_code, "stop_wiring_unavailable",
            "the lift to screen@{version} could not stop a running child: {details}"
        );
    }
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "lifting display to screen@{version} while its children are awake must commit; got {outcome:?}"
    );
}

/// Send one probe on `lane` at the screen and expect it in the sink, with
/// the texts it collected on the way.
async fn walk(colony: &mut Colony, lane: &str, text: &str) -> Vec<String> {
    colony.h.send(probe(lane, "/alex/display", text)).await;
    let got = expect_capture(colony, &format!("{lane} probe '{text}'")).await;
    texts_of(&got)
}

/// A hive is lifted while its children are AWAKE — a task running for each
/// of them — twice over: 1.0.0 → 1.1.0 → 1.0.0 → 1.1.0, with a probe between
/// the lifts so the children the previous lift spawned are awake before the
/// next one stops them. The lift's own recompute stops the changed child's
/// old task and the left child, and can only do so with the stop wiring the
/// child re-notified on its (re)spawn (GH #676 form) — the
/// `stop_wiring_unavailable` guard does not fire at any step. The resume
/// guard `subtree_resume_awake_check` is not on this path: a lift keeps a
/// kept child's task as it is, and a changed child goes through the stop
/// mechanics, not through a resume. After the last lift an `in_view` probe
/// still leaves the screen into `/alex/capture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hive_is_lifted_while_its_children_are_awake() {
    let td = TempDir::new().unwrap();
    // `grown_and_probed` already walked one `in_view` through the 1.0.0 chain.
    let mut colony = grown_and_probed(&td).await;
    let awake = |paths: &[&str]| -> Vec<(String, String)> {
        paths
            .iter()
            .map(|p| (format!("/alex/display/{p}"), "Awake".to_string()))
            .collect()
    };
    assert_eq!(
        child_lifecycle(&colony.h).await,
        awake(&["bump", "gone", "keep"]),
        "every 1.0.0 child runs before the first lift"
    );

    // 1.0.0 → 1.1.0: stops the running `bump` (moved aside) and `gone`.
    lift_awake_to(&colony.h, "1.1.0").await;
    assert_eq!(
        walk(&mut colony, "in_view", "first_lift").await,
        vec![
            "first_lift",
            "echo from /alex/display/keep",
            "echo from /alex/display/bump",
        ]
    );
    assert_eq!(
        walk(&mut colony, "in_notice", "first_notice").await,
        vec!["first_notice", "echo from /alex/display/fresh"]
    );
    let after_first = child_lifecycle(&colony.h).await;
    let running = |rows: &[(String, String)]| -> Vec<String> {
        rows.iter()
            .filter(|(_, s)| s == "Awake")
            .map(|(p, _)| p.clone())
            .collect()
    };
    assert_eq!(
        running(&after_first),
        vec![
            "/alex/display/bump",
            "/alex/display/fresh",
            "/alex/display/keep"
        ],
        "the new bump, fresh and keep run; the parked bump~1.0.0 and gone do not: {after_first:?}"
    );

    // 1.1.0 → 1.0.0: stops the running NEW `bump` (moved aside) and `fresh`,
    // reactivates `gone`.
    lift_awake_to(&colony.h, "1.0.0").await;
    assert_eq!(
        walk(&mut colony, "in_view", "second_lift").await,
        vec![
            "second_lift",
            "echo from /alex/display/keep",
            "echo from /alex/display/bump",
            "echo from /alex/display/gone",
        ]
    );
    let after_second = child_lifecycle(&colony.h).await;
    assert_eq!(
        running(&after_second),
        vec![
            "/alex/display/bump",
            "/alex/display/gone",
            "/alex/display/keep"
        ],
        "bump (1.0.0 again), the reactivated gone and keep run: {after_second:?}"
    );

    // 1.0.0 → 1.1.0 once more: stops the running `bump` and `gone` again.
    lift_awake_to(&colony.h, "1.1.0").await;
    assert_eq!(
        walk(&mut colony, "in_view", "third_lift").await,
        vec![
            "third_lift",
            "echo from /alex/display/keep",
            "echo from /alex/display/bump",
        ],
        "after the last lift an in_view probe still reaches the sink"
    );
    let after_third = child_lifecycle(&colony.h).await;
    assert_eq!(
        running(&after_third),
        vec![
            "/alex/display/bump",
            "/alex/display/fresh",
            "/alex/display/keep"
        ],
        "{after_third:?}"
    );
    assert_eq!(
        child_activity(&colony.h).await,
        vec![
            ("/alex/display/bump".to_string(), true),
            ("/alex/display/bump~1.0.0".to_string(), false),
            ("/alex/display/bump~1.0.0~2".to_string(), false),
            ("/alex/display/bump~1.1.0".to_string(), false),
            ("/alex/display/fresh".to_string(), true),
            ("/alex/display/gone".to_string(), false),
            ("/alex/display/keep".to_string(), true),
        ]
    );
    let dlq = colony.h.drain_dead_letters().await;
    assert!(dlq.is_empty(), "no dead letters expected; got {dlq:?}");
    colony.h.shutdown().await;
}

/// One node is lifted once per diff (T5-R4 c): two `replace_nodes` entries
/// naming the same node would stage the same aside name twice and the
/// second rename would strict-fail the colony task. So `match.name` is a
/// path claim like every entry that puts a node at a path, and the diff is
/// refused against itself as `naming_collision` before anything is staged:
/// the registry, the edges and the staging area are as they were.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_node_is_lifted_once_per_diff() {
    let td = TempDir::new().unwrap();
    let mut colony = grown_and_probed(&td).await;
    let activity_before = child_activity(&colony.h).await;
    let edges_before: Vec<EdgeRow> = edges_of(&colony.h, "/alex")
        .await
        .into_iter()
        .map(edge_row)
        .collect();
    let staging_before = staging_entries(td.path());

    let (code, details) = refused(
        submit_diff(
            &colony.h,
            "/alex",
            json!({"replace_nodes": [
                {"match": {"name": "display"}, "with": {"template": "screen@1.1.0"}},
                {"match": {"name": "./display"}, "with": {"template": "screen@1.0.0"}}
            ]}),
        )
        .await,
        "two lifts of one node in one diff",
    );
    assert_eq!(code, "naming_collision", "{details}");
    assert!(
        details.contains("replace_nodes[1].match.name")
            && details.contains("replace_nodes[0].match.name")
            && details.contains("/alex/display"),
        "the refusal names both entries and the path: {details}"
    );

    assert_eq!(child_activity(&colony.h).await, activity_before);
    let edges_after: Vec<EdgeRow> = edges_of(&colony.h, "/alex")
        .await
        .into_iter()
        .map(edge_row)
        .collect();
    assert_eq!(edges_after, edges_before);
    assert_eq!(staging_entries(td.path()), staging_before);
    assert_eq!(
        stamped_version(&read_config(
            &td.path().join("main/alex/display/config.json")
        )),
        "1.0.0",
        "the hive still stands at 1.0.0"
    );
    assert_eq!(
        walk(&mut colony, "in_view", "still_one_zero_zero").await,
        vec![
            "still_one_zero_zero",
            "echo from /alex/display/keep",
            "echo from /alex/display/bump",
            "echo from /alex/display/gone",
        ]
    );
    colony.h.shutdown().await;
}

/// A parked node takes no edges: a row named `<name>~<version>` is what an
/// earlier replace left beside its successor, and its task, watcher and
/// respawn closure are still addressed at the birth path — wiring it would
/// spawn a task under the successor's name. Refused as `schema` in the
/// validate stage, for an `add_edges` endpoint on either side; nothing
/// changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_node_takes_no_edges() {
    let td = TempDir::new().unwrap();
    let colony = grown_and_probed(&td).await;
    lift_to(&colony.h, "1.1.0").await;
    let edges_before: Vec<EdgeRow> = edges_of(&colony.h, "/alex")
        .await
        .into_iter()
        .map(edge_row)
        .collect();

    for (from, to) in [("./keep", "./bump~1.0.0"), ("./bump~1.0.0", ".")] {
        let (code, details) = refused(
            submit_diff(
                &colony.h,
                "/alex/display",
                json!({"add_edges": [{"from": from, "to": to}]}),
            )
            .await,
            "an edge onto a parked node",
        );
        assert_eq!(code, "schema", "{details}");
        assert!(
            details.contains("parked node") && details.contains("bump~1.0.0"),
            "the refusal names the parked node: {details}"
        );
    }
    let edges_after: Vec<EdgeRow> = edges_of(&colony.h, "/alex")
        .await
        .into_iter()
        .map(edge_row)
        .collect();
    assert_eq!(edges_after, edges_before);
    colony.h.shutdown().await;
}

/// A parked node is not lifted either: `bump~1.0.0` is what an earlier
/// replace left beside its successor, and lifting it would create a cell
/// under a park name that no recompute ever activates. Refused as `schema`
/// in the validate stage, naming the parked node; the registry rows and the
/// disk stay what they were. The live `bump` beside it is the node to lift.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_parked_node_is_not_lifted() {
    let td = TempDir::new().unwrap();
    write_note_templates(td.path());
    let colony = grown_and_probed(&td).await;
    lift_to(&colony.h, "1.1.0").await;
    let rows_before = child_activity(&colony.h).await;
    let display = td.path().join("main/alex/display");
    let old_bump_before = std::fs::read(display.join("bump~1.0.0/config.json")).unwrap();

    let (code, details) = refused(
        submit_diff(
            &colony.h,
            "/alex/display",
            json!({"replace_nodes": [{"match": {"name": "bump~1.0.0"},
                                      "with": {"template": "note@1.1.0"}}]}),
        )
        .await,
        "lifting a parked node",
    );
    assert_eq!(code, "schema", "{details}");
    assert!(
        details.contains("/alex/display/bump~1.0.0, which is a parked node from an earlier replace; lift the live node instead"),
        "the refusal names the parked node and points at the live one: {details}"
    );

    assert_eq!(child_activity(&colony.h).await, rows_before);
    assert_eq!(
        std::fs::read(display.join("bump~1.0.0/config.json")).unwrap(),
        old_bump_before
    );
    assert!(!display.join("bump~1.0.0~1.0.0").exists());
    colony.h.shutdown().await;
}

/// GH #682 (OR-P4) — a committed replace names what happened to every child:
/// one entry per child, sorted by path, with the version it came from and the
/// one it went to. The same list rides the door reply the HTTP and the EDA
/// door render, so a caller of either reads it without the Rust type. Any
/// other operation commits with an empty list — the field is additive.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_receipt_names_every_child() {
    use meclaw_colony::mutation::{NodeChange, NodeVerdict};
    use meclaw_colony::{MutationDoorOutcome, mutation_door_reply};

    let td = TempDir::new().unwrap();
    let colony = grown_and_probed(&td).await;
    let outcome = submit_replace(
        &colony.h,
        json!({"match": {"name": "display"}, "with": {"template": "screen@1.1.0"}}),
    )
    .await;
    let MutationOutcome::Committed { id, changes } = &outcome else {
        panic!("the lift must commit; got {outcome:?}");
    };
    let change =
        |path: &str, verdict: NodeVerdict, from: Option<&str>, to: Option<&str>| NodeChange {
            path: path.to_string(),
            verdict,
            from_version: from.map(str::to_string),
            to_version: to.map(str::to_string),
        };
    assert_eq!(
        changes,
        &vec![
            change(
                "/alex/display/bump",
                NodeVerdict::Replaced,
                Some("1.0.0"),
                Some("1.1.0")
            ),
            change(
                "/alex/display/fresh",
                NodeVerdict::Added,
                None,
                Some("1.1.0")
            ),
            change("/alex/display/gone", NodeVerdict::Left, Some("1.0.0"), None),
            change(
                "/alex/display/keep",
                NodeVerdict::Kept,
                Some("1.0.0"),
                Some("1.0.0")
            ),
        ],
        "exactly the four children, sorted by path"
    );

    // The door reply — the document `POST /colony/mutations` and the EDA
    // reply carry — says the same in JSON, verdicts lowercase.
    let reply = mutation_door_reply(&MutationDoorOutcome::Single(outcome.clone()));
    assert_eq!(reply["mutation"]["outcome"], "committed");
    assert_eq!(reply["mutation"]["id"], json!(id));
    assert_eq!(
        reply["mutation"]["changes"],
        json!([
            {"path": "/alex/display/bump", "verdict": "replaced",
             "from_version": "1.0.0", "to_version": "1.1.0"},
            {"path": "/alex/display/fresh", "verdict": "added",
             "from_version": null, "to_version": "1.1.0"},
            {"path": "/alex/display/gone", "verdict": "left",
             "from_version": "1.0.0", "to_version": null},
            {"path": "/alex/display/keep", "verdict": "kept",
             "from_version": "1.0.0", "to_version": "1.0.0"},
        ])
    );
    let round_trip: Vec<NodeChange> =
        meclaw_core::serde_json::from_value(reply["mutation"]["changes"].clone())
            .expect("the wire form reads back");
    assert_eq!(&round_trip, changes);

    // Any other operation: an empty list, present.
    let grown = submit_diff(
        &colony.h,
        "/alex",
        json!({"add_nodes": [{"name": "second", "template": "screen@1.0.0"}]}),
    )
    .await;
    let MutationOutcome::Committed { changes, .. } = &grown else {
        panic!("growing a second screen must commit; got {grown:?}");
    };
    assert!(changes.is_empty(), "add_nodes names no child: {changes:?}");
    let reply = mutation_door_reply(&MutationDoorOutcome::Single(grown.clone()));
    assert_eq!(reply["mutation"]["changes"], json!([]));

    colony.h.shutdown().await;
}

/// `screen@1.1.1`: the 1.1.0 graph with one difference — the `. -> ./keep`
/// edge names its lane (`"lane": "in_view"`). Written by the test that
/// needs it.
fn write_screen_one_one_one(root: &std::path::Path) {
    let v = root.join("templates/local/screen@1.1.1");
    write(
        &v,
        "template.json",
        r#"{"name":"screen","version":"1.1.1"}"#,
    );
    write(
        &v,
        "config.json",
        r#"{"cell":{"type":"hive"},"params":{
            "contract":{"accepts":[
                {"route":"in_view","because":"put this view up on the screen"},
                {"route":"in_notice","because":"a short notice, shown once and gone"}
            ]},
            "graph":{"edges":[
                {"from":".","to":"./keep","condition":"has(hop.route) && hop.route == 'in_view'","lane":"in_view"},
                {"from":".","to":"./fresh","condition":"has(hop.route) && hop.route == 'in_notice'"},
                {"from":"./keep","to":"./bump"},
                {"from":"./bump","to":"."},
                {"from":"./fresh","to":"."}
            ]}}}"#,
    );
    write(&v, "keep/config.json", &child("1.0.0"));
    write(&v, "bump/config.json", &child("1.1.0"));
    write(&v, "fresh/config.json", &child("1.0.0"));
}

/// GH #682 (final review): an edge's identity has five terms and `lane` is
/// not one of them. A version that names a standing inner edge again under
/// a lane (here `. -> ./keep` with `"lane": "in_view"`, unlaned at 1.0.0)
/// must land that declaration: after the lift the edge table holds the
/// laned edge under `/alex/display -> /alex/display/keep`, once, and not the
/// old unlaned one — the dedup must not swallow the new edge behind a
/// retained old one. The screen still works through it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lane_named_on_a_standing_inner_edge_lands() {
    let td = TempDir::new().unwrap();
    write_screen_one_one_one(td.path());
    let mut colony = grown_and_probed(&td).await;
    let keep_edges_before: Vec<GraphEdgeDto> = edges_of(&colony.h, "/alex")
        .await
        .into_iter()
        .filter(|e| e.from == "/alex/display" && e.to == "/alex/display/keep")
        .collect();
    assert_eq!(keep_edges_before.len(), 1);
    assert_eq!(keep_edges_before[0].lane, None, "1.0.0 names no lane");
    let outer_before = outer_rows(&colony.h, "/alex", "/alex/display").await;

    lift_to(&colony.h, "1.1.1").await;

    let keep_edges: Vec<GraphEdgeDto> = edges_of(&colony.h, "/alex")
        .await
        .into_iter()
        .filter(|e| e.from == "/alex/display" && e.to == "/alex/display/keep")
        .collect();
    assert_eq!(
        keep_edges.len(),
        1,
        "one edge display -> keep after the lift; got {keep_edges:?}"
    );
    assert_eq!(
        keep_edges[0].lane.as_deref(),
        Some("in_view"),
        "the lift landed the lane the new version names on the edge"
    );
    assert_ne!(
        keep_edges[0].id, keep_edges_before[0].id,
        "the laned edge is a new row, the unlaned old one is gone"
    );
    assert_eq!(
        inner_pairs(&colony.h, "/alex", "/alex/display").await,
        inner_of_one_one_zero(),
        "the inner graph is the five edges of the 1.1.x shape"
    );
    assert_eq!(
        outer_rows(&colony.h, "/alex", "/alex/display").await,
        outer_before,
        "the outer edges are untouched"
    );

    colony
        .h
        .send(probe("in_view", "/alex/display", "over_the_laned_edge"))
        .await;
    let got = expect_capture(&mut colony, "in_view through the laned edge").await;
    assert_eq!(
        texts_of(&got),
        vec![
            "over_the_laned_edge",
            "echo from /alex/display/keep",
            "echo from /alex/display/bump",
        ]
    );
    let dlq = colony.h.drain_dead_letters().await;
    assert!(dlq.is_empty(), "no dead letters expected; got {dlq:?}");

    colony.h.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Drift lock: the documentation names the lift (Task 8 of the wave).
// ─────────────────────────────────────────────────────────────────────────────

/// A path under the repository root, from this crate's manifest.
fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The overview names the lift, in both editions, at the table row and in the
/// receipt sentence.
///
/// Two tree shapes (the gh325/gh677 form): the private tree carries
/// `docs/meclaw-overview.md` (German) beside `docs/meclaw-overview.en.md`
/// (English); the export copies the `.en.md` bytes ONTO the plain name and
/// ships nothing else, so a public clone holds the English text under
/// `meclaw-overview.md` and no `.en.md`. The English phrases are therefore
/// read from `.en.md` where it exists and from `.md` where it does not; the
/// table row is looked for in every edition on disk.
#[test]
fn the_docs_name_the_lift() {
    let german = repo("docs/meclaw-overview.md");
    let english = repo("docs/meclaw-overview.en.md");
    let english = if english.is_file() {
        english
    } else {
        german.clone()
    };
    let read = |p: &std::path::Path| {
        std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    };

    let en = read(&english);
    assert!(
        en.contains("lifted to a new version of its template in place"),
        "the English overview says what replace_nodes is ({})",
        english.display()
    );
    assert!(
        en.contains("`changes` names every child of a replaced node"),
        "the English overview says what the receipt's `changes` carries ({})",
        english.display()
    );

    // The diff-operation table has a `replace_nodes` row in every edition
    // this tree carries, right after `swap_nodes`.
    for path in [&german, &english] {
        let doc = read(path);
        let rows: Vec<&str> = doc
            .lines()
            .filter(|l| l.starts_with("| `swap_nodes` |") || l.starts_with("| `replace_nodes` |"))
            .map(|l| l.split('|').nth(1).unwrap_or("").trim())
            .collect();
        assert_eq!(
            rows,
            ["`swap_nodes`", "`replace_nodes`"],
            "{}: the table has a replace_nodes row right after swap_nodes",
            path.display()
        );
    }
}
