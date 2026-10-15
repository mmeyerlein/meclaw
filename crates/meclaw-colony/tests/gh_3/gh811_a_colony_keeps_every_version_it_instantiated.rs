//! GH #811 — a colony keeps every template version it has ever instantiated.
//!
//! The fixture is the `screen` class of GH #682 (`gh682_a_standing_hive_is_
//! lifted_in_place.rs`, helpers copied), but shipped the way an instance build
//! lays a library down: ONE directory `templates/screen/` holding the current
//! version, not the `local/screen@<v>/` siblings a registration builds. That is
//! the shape in which a library swap used to take the old version with it — the
//! directory is replaced, the next rescan deletes the row, and the way back of
//! a lift needed an `add_templates` of bytes nobody kept.
//!
//! What this file pins:
//! 1. after a committed instantiation the colony holds its own copy under
//!    `templates/local/<name>@<version>/`, and the row points there;
//! 2. the way back after the library moved on needs no `add_templates`;
//! 3. a changed tree under a stored version is `template_version_immutable`,
//!    the same tree again stays `template_name_taken`;
//! 4. a rescan over a changed shipped tree keeps the kept version and skips the
//!    shipped one by name;
//! 5. an existing colony (row on the shipped directory) gets its copy at boot,
//!    idempotently;
//! 6. `/colony/templates` lists every version with `scanned_at`.

use meclaw_colony::api_dto::RegistryEntryDto;
use meclaw_colony::templates::scan_templates_dir_with_skips;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, ContractView, DiskBlobStore, MutationOutcome,
    RespawnFn, SpawnedCellKind, bootstrap_from_filesystem, cell_task, renotify_stop_wiring,
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

/// One `replace_nodes` diff with the given entry, submitted in `/alex`.
async fn submit_replace(h: &ColonyHandle, entry: JsonValue) -> MutationOutcome {
    submit_diff(h, "/alex", json!({"replace_nodes": [entry]})).await
}

/// Lift `/alex/display` to the given version and expect it to commit.
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

// ─────────────────────────────────────────────────────────────────────────────
// GH #811 fixture: the library in shipped form
// ─────────────────────────────────────────────────────────────────────────────

/// The 1.0.0 files of `screen`, relative to the template directory.
fn screen_v1_files() -> Vec<(&'static str, String)> {
    vec![
        (
            "template.json",
            r#"{"name":"screen","version":"1.0.0"}"#.to_string(),
        ),
        (
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
            ]}}}"#
                .to_string(),
        ),
        ("keep/config.json", child("1.0.0")),
        ("bump/config.json", child("1.0.0")),
        ("gone/config.json", child("1.0.0")),
    ]
}

/// The 1.1.0 files of `screen` (see `gh682`: `bump` changes, `fresh` arrives,
/// `gone` leaves).
fn screen_v2_files() -> Vec<(&'static str, String)> {
    vec![
        (
            "template.json",
            r#"{"name":"screen","version":"1.1.0"}"#.to_string(),
        ),
        (
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
            ]}}}"#
                .to_string(),
        ),
        ("keep/config.json", child("1.0.0")),
        ("bump/config.json", child("1.1.0")),
        ("fresh/config.json", child("1.0.0")),
    ]
}

/// Lay `files` down as the ONE shipped directory `templates/screen/`,
/// replacing whatever stood there — the way an instance build swaps a library.
fn ship_screen(root: &std::path::Path, files: &[(&'static str, String)]) {
    let dir = root.join("templates/screen");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    for (rel, body) in files {
        write(&dir, rel, body);
    }
}

fn kept(root: &std::path::Path, version: &str) -> std::path::PathBuf {
    root.join(format!("templates/local/screen@{version}"))
}

/// `(template_id, version, filesystem_path)` of every `screen` row in
/// `colony.db`, sorted by version — the catalogue itself, not the reply.
fn screen_rows(root: &std::path::Path) -> Vec<(String, Option<String>, String)> {
    let conn = rusqlite::Connection::open_with_flags(
        root.join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open colony.db read-only");
    let mut stmt = conn
        .prepare(
            "SELECT template_id, version, filesystem_path FROM templates \
             WHERE name = 'screen' ORDER BY version",
        )
        .expect("prepare");
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .expect("query");
    rows.map(|r| r.expect("row")).collect()
}

/// A colony over a fresh root with `screen@1.0.0` shipped and grown as
/// `/alex/display`.
async fn grown_from_the_shipped_library(td: &TempDir) -> Colony {
    write_alex_topology(td.path());
    ship_screen(td.path(), &screen_v1_files());
    let colony = start_colony(td).await;
    let outcome = grow_screen(&colony.h).await;
    assert!(
        matches!(outcome, MutationOutcome::Committed { .. }),
        "growing screen@1.0.0 from the shipped library must commit; got {outcome:?}"
    );
    colony
}

/// One `add_templates` diff that declares `screen` with `files`.
fn register_screen(files: &[(&'static str, String)]) -> JsonValue {
    let map: meclaw_core::serde_json::Map<String, JsonValue> = files
        .iter()
        .map(|(rel, body)| (rel.to_string(), json!(body)))
        .collect();
    json!({"add_templates": [{"name": "screen", "files": map}]})
}

fn verdict(outcome: MutationOutcome) -> (String, String) {
    match outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => (error_code, details),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// The entries under `templates/local/`, sorted.
fn local_entries(root: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root.join("templates/local"))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

// ─────────────────────────────────────────────────────────────────────────────
// The proofs
// ─────────────────────────────────────────────────────────────────────────────

/// The main proof of GH #811: grow 1.0.0 from the shipped library, swap the
/// library to 1.1.0 (the 1.0.0 directory is GONE), lift to 1.1.0 and back to
/// 1.0.0 — without a single `add_templates`. The reboot keeps both rows under
/// their ids and leaves the copies untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_way_back_needs_no_add_templates_after_the_library_moved_on() {
    let td = TempDir::new().unwrap();
    let colony = grown_from_the_shipped_library(&td).await;
    assert!(
        kept(td.path(), "1.0.0").join("template.json").is_file(),
        "after the committed grow the colony must hold its own copy of screen@1.0.0"
    );
    let rows = screen_rows(td.path());
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        std::path::Path::new(&rows[0].2),
        kept(td.path(), "1.0.0"),
        "the row must point at the colony's own copy"
    );
    let id_one = rows[0].0.clone();

    // The library moves on: the shipped 1.0.0 directory is replaced by 1.1.0.
    ship_screen(td.path(), &screen_v2_files());
    assert!(
        !td.path().join("templates/screen/gone/config.json").exists(),
        "the shipped 1.0.0 must really be gone"
    );
    rescan_templates(&colony.h, td.path().join("templates")).await;
    assert_eq!(
        library_versions(&colony.h, "screen").await,
        vec!["1.0.0", "1.1.0"],
        "the rescan after the swap must keep 1.0.0 beside the new 1.1.0"
    );

    lift_to(&colony.h, "1.1.0").await;
    assert!(
        kept(td.path(), "1.1.0").join("template.json").is_file(),
        "the lift instantiated 1.1.0, so the colony keeps it too"
    );
    lift_to(&colony.h, "1.0.0").await;
    assert_eq!(
        registry_row(&colony.h, "/alex/display/gone")
            .await
            .map(|r| r.active),
        Some(true),
        "back at 1.0.0, gone stands active again"
    );

    let before = screen_rows(td.path());
    let colony = reboot(colony, &td).await;
    let after = screen_rows(td.path());
    assert_eq!(after.len(), 2, "{after:?}");
    assert_eq!(before, after, "the reboot kept both rows, ids and paths");
    assert_eq!(after[0].0, id_one, "1.0.0 kept its id through it all");
    for (_, v, p) in &after {
        let v = v.as_deref().unwrap();
        assert_eq!(std::path::Path::new(p), kept(td.path(), v));
        assert!(kept(td.path(), v).join("config.json").is_file());
    }
    assert!(
        kept(td.path(), "1.0.0").join("gone/config.json").is_file(),
        "the kept 1.0.0 is the 1.0.0 tree"
    );
    colony.h.shutdown().await;
}

/// `add_templates` with a changed tree under a version the colony already
/// holds is refused as `template_version_immutable` — and leaves nothing
/// under `local/` beside the kept copy.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_changed_tree_under_a_stored_version_is_refused_as_immutable() {
    let td = TempDir::new().unwrap();
    let colony = grown_from_the_shipped_library(&td).await;
    let mut files = screen_v1_files();
    for (rel, body) in files.iter_mut() {
        if *rel == "keep/config.json" {
            *body = child("1.0.9");
        }
    }
    let (code, details) = verdict(submit_diff(&colony.h, "/", register_screen(&files)).await);
    assert_eq!(code, "template_version_immutable", "{details}");
    assert!(details.contains("screen@1.0.0"), "{details}");
    assert_eq!(local_entries(td.path()), vec!["screen@1.0.0"]);
    assert!(
        !td.path().join(".staging-templates").exists()
            || std::fs::read_dir(td.path().join(".staging-templates"))
                .unwrap()
                .next()
                .is_none(),
        "the refused declaration left staging residue"
    );
    colony.h.shutdown().await;
}

/// The identical tree again is what it always was: `template_name_taken`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_tree_again_is_still_taken() {
    let td = TempDir::new().unwrap();
    let colony = grown_from_the_shipped_library(&td).await;
    let (code, details) =
        verdict(submit_diff(&colony.h, "/", register_screen(&screen_v1_files())).await);
    assert_eq!(code, "template_name_taken", "{details}");
    assert_eq!(local_entries(td.path()), vec!["screen@1.0.0"]);
    colony.h.shutdown().await;
}

/// A shipped tree changed WITHOUT a version bump does not replace what the
/// colony keeps: the rescan goes through (no aborted boot, GH #668's rule), the
/// row stays on the kept copy, and the shipped directory is skipped by name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rescan_over_a_changed_shipped_tree_keeps_the_kept_version() {
    let td = TempDir::new().unwrap();
    let colony = grown_from_the_shipped_library(&td).await;
    write(
        &td.path().join("templates/screen"),
        "keep/config.json",
        &child("1.0.9"),
    );
    rescan_templates(&colony.h, td.path().join("templates")).await;
    let rows = screen_rows(td.path());
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(std::path::Path::new(&rows[0].2), kept(td.path(), "1.0.0"));

    let (found, skipped) = scan_templates_dir_with_skips(&td.path().join("templates")).unwrap();
    assert_eq!(
        found
            .iter()
            .filter(|t| t.name == "screen")
            .map(|t| t.filesystem_path.clone())
            .collect::<Vec<_>>(),
        vec![kept(td.path(), "1.0.0")]
    );
    let skip = skipped
        .iter()
        .find(|s| s.name == "screen")
        .expect("the changed shipped tree must be skipped by name");
    assert!(
        skip.reason.contains("template_version_immutable"),
        "{}",
        skip.reason
    );
    assert!(
        skip.reason
            .contains(&kept(td.path(), "1.0.0").display().to_string()),
        "the reason names the kept copy: {}",
        skip.reason
    );
    assert_eq!(
        skip.filesystem_path,
        td.path().join("templates/screen"),
        "the skip names the shipped directory"
    );
    colony.h.shutdown().await;
}

/// Migration: a colony that instantiated before GH #811 has its row on the
/// SHIPPED directory and no copy. Its next boot keeps the version, under the
/// same `template_id`, and a second boot changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_existing_colony_gets_its_versions_kept_at_boot() {
    let td = TempDir::new().unwrap();
    let colony = grown_from_the_shipped_library(&td).await;
    colony.h.shutdown().await;
    drop(colony.capture_rx);

    // Roll the colony back to the pre-#811 state: no copy, the row on the
    // shipped directory.
    std::fs::remove_dir_all(td.path().join("templates/local")).unwrap();
    let shipped = td.path().join("templates/screen");
    {
        let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
        conn.execute(
            "UPDATE templates SET filesystem_path = ?1 WHERE name = 'screen'",
            [shipped.to_string_lossy().into_owned()],
        )
        .unwrap();
    }
    let id_before = screen_rows(td.path())[0].0.clone();
    assert_eq!(
        std::path::Path::new(&screen_rows(td.path())[0].2),
        shipped,
        "precondition: the row sits on the shipped directory"
    );

    let colony = start_colony(&td).await;
    let rows = screen_rows(td.path());
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, id_before, "the same row, the same id");
    assert_eq!(std::path::Path::new(&rows[0].2), kept(td.path(), "1.0.0"));
    assert!(kept(td.path(), "1.0.0").join("gone/config.json").is_file());

    let colony = reboot(colony, &td).await;
    assert_eq!(
        screen_rows(td.path()),
        rows,
        "the second boot is idempotent"
    );
    assert_eq!(local_entries(td.path()), vec!["screen@1.0.0"]);
    colony.h.shutdown().await;
}

/// `/colony/templates` lists every version with the time it was last scanned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_version_is_listed_with_scanned_at() {
    let td = TempDir::new().unwrap();
    let colony = grown_from_the_shipped_library(&td).await;
    ship_screen(td.path(), &screen_v2_files());
    rescan_templates(&colony.h, td.path().join("templates")).await;

    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadTemplatesReply>();
    colony
        .h
        .inbox_tx
        .send(ColonyMsg::ReadTemplates {
            cell_type: None,
            name: Some("screen".to_string()),
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let entries = ack_rx.await.unwrap().entries;
    assert_eq!(entries.len(), 2, "{entries:?}");
    for e in &entries {
        assert!(e.scanned_at > 0, "{e:?} carries no scanned_at");
    }
    colony.h.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// T2b: provenance as a join, and the receipt names the rows
// ─────────────────────────────────────────────────────────────────────────────

/// A colony grown at 1.0.0 with the library moved on to 1.1.0 (rescanned),
/// plus the two row ids.
async fn grown_with_both_versions(td: &TempDir) -> (Colony, String, String) {
    let colony = grown_from_the_shipped_library(td).await;
    ship_screen(td.path(), &screen_v2_files());
    rescan_templates(&colony.h, td.path().join("templates")).await;
    let rows = screen_rows(td.path());
    assert_eq!(rows.len(), 2, "{rows:?}");
    (colony, rows[0].0.clone(), rows[1].0.clone())
}

/// Every hop of every chain resolves to a `templates` row, and
/// `/colony/templates?name=` answers which nodes stand on which version.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chain_resolves_to_template_rows() {
    let td = TempDir::new().unwrap();
    let (colony, id_one, id_two) = grown_with_both_versions(&td).await;

    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadTemplatesReply>();
    colony
        .h
        .inbox_tx
        .send(ColonyMsg::ReadTemplates {
            cell_type: None,
            name: Some("screen".to_string()),
            limit: 100,
            ack: ack_tx,
        })
        .await
        .unwrap();
    let entries = ack_rx.await.unwrap().entries;
    let used_by = |id: &str| {
        entries
            .iter()
            .find(|e| e.template_id == id)
            .and_then(|e| e.used_by.clone())
            .unwrap_or_else(|| panic!("no used_by for {id}: {entries:?}"))
    };
    assert_eq!(
        used_by(&id_one),
        vec![
            "/alex/display/bump".to_string(),
            "/alex/display/gone".to_string(),
            "/alex/display/keep".to_string(),
        ],
        "the three children of the 1.0.0 screen stand on 1.0.0"
    );
    assert!(used_by(&id_two).is_empty(), "nothing stands on 1.1.0 yet");
    colony.h.shutdown().await;
    drop(colony.capture_rx);

    let db = meclaw_colony::ColonyDb::open(&td.path().join("colony.db")).unwrap();
    let uses = db.read_template_uses().unwrap();
    assert!(!uses.is_empty());
    for u in &uses {
        assert!(
            u.template_id.is_some(),
            "a stamp without a row: {u:?} (all: {uses:?})"
        );
    }
    db.shutdown_async().await;
}

/// The lift receipt names, per child, the `templates` row it stood on and the
/// one it stands on now.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_lift_receipt_names_both_template_ids() {
    let td = TempDir::new().unwrap();
    let (colony, id_one, id_two) = grown_with_both_versions(&td).await;
    let outcome = submit_replace(
        &colony.h,
        json!({"match": {"name": "display"}, "with": {"template": "screen@1.1.0"}}),
    )
    .await;
    let MutationOutcome::Committed { changes, .. } = &outcome else {
        panic!("the lift must commit; got {outcome:?}");
    };
    let ids: Vec<(&str, Option<&str>, Option<&str>)> = changes
        .iter()
        .map(|c| {
            (
                c.path.as_str(),
                c.from_template_id.as_deref(),
                c.to_template_id.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        ids,
        vec![
            (
                "/alex/display/bump",
                Some(id_one.as_str()),
                Some(id_two.as_str())
            ),
            ("/alex/display/fresh", None, Some(id_two.as_str())),
            ("/alex/display/gone", Some(id_one.as_str()), None),
            (
                "/alex/display/keep",
                Some(id_one.as_str()),
                Some(id_one.as_str())
            ),
        ]
    );
    let reply = meclaw_colony::mutation_door_reply(&meclaw_colony::MutationDoorOutcome::Single(
        outcome.clone(),
    ));
    assert_eq!(
        reply["mutation"]["changes"][0]["to_template_id"],
        json!(id_two),
        "the ids travel on the wire"
    );
    colony.h.shutdown().await;
}

/// T2 review M4: a stored tree the door cannot read cannot be compared, and
/// the conservative answer stays `template_name_taken` — but the refusal says
/// WHY it could not tell a changed tree from the same one.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreadable_stored_tree_is_taken_and_says_why() {
    use std::os::unix::fs::PermissionsExt;
    let td = TempDir::new().unwrap();
    let colony = grown_from_the_shipped_library(&td).await;
    let locked = kept(td.path(), "1.0.0").join("keep");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&locked).is_ok() {
        // A user the permission bits do not stop (root) cannot measure this.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        colony.h.shutdown().await;
        return;
    }
    let outcome = submit_diff(&colony.h, "/", register_screen(&screen_v1_files())).await;
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (code, details) = verdict(outcome);
    assert_eq!(code, "template_name_taken", "{details}");
    assert!(
        details.contains("could not be compared"),
        "the refusal names the unreadable stored tree: {details}"
    );
    colony.h.shutdown().await;
}

/// T2 review M5: a node stamped before `template_chain` existed has its stamp
/// and no chain. It is kept (the keep reads stamp and chain) and must be named
/// in `used_by` as well — as hop 0, the one hop its stamp is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stamp_without_a_chain_is_a_use_too() {
    let td = TempDir::new().unwrap();
    let (colony, _id_one, _id_two) = grown_with_both_versions(&td).await;
    colony.h.shutdown().await;
    drop(colony.capture_rx);
    {
        let conn = rusqlite::Connection::open(td.path().join("colony.db")).unwrap();
        let n = conn
            .execute(
                "UPDATE registry SET template_chain = NULL WHERE path = '/alex/display/keep'",
                [],
            )
            .unwrap();
        assert_eq!(n, 1, "precondition: the row exists");
        let stamp: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT template, template_version FROM registry WHERE path = '/alex/display/keep'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(
            stamp.0.is_some(),
            "precondition: the row carries a stamp: {stamp:?}"
        );
    }
    let db = meclaw_colony::ColonyDb::open(&td.path().join("colony.db")).unwrap();
    let uses = db.read_template_uses().unwrap();
    let keep: Vec<_> = uses
        .iter()
        .filter(|u| u.path == "/alex/display/keep")
        .collect();
    assert_eq!(keep.len(), 1, "{uses:?}");
    assert_eq!(keep[0].hop, 0);
    assert!(keep[0].template_id.is_some(), "{keep:?}");
    db.shutdown_async().await;
}
