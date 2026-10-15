//! GH #1083 — a lift renews the hives nested in the composite it lifts.
//!
//! `replace_nodes` on a composite re-stages the leaves below it from the new
//! version, and renews the lifted hive's own declaration. A hive NESTED in
//! the composite used to stand as a kept marker with the contract of its
//! birth: a new version whose leaf is reached on a new v-lane (the nested
//! hive now declares the connect point) was refused
//! `v_lane_no_connect_point` on the leaf, because the post-state judged the
//! nested hive by its old `config.json`. Found by the 0.62.0 migration proof
//! of GH #801 (`…/curator/summarizer`).
//!
//! Fixture (echo cells with real stop wiring, copied from
//! `gh773_a_rewritten_description_keeps_the_store.rs`; there is no shared
//! support module under `crates/meclaw-colony/tests/`):
//!
//! ```text
//! board@1.0.0  hive, accepts in_view           . ─▶ summarizer ─▶ .
//! board@1.1.0  hive, accepts in_view + digest at ./summarizer; summarizer
//!              with other params
//! desk@1.0.0   hive  . ─▶ curator(ref board@1.0.0) ─▶ .
//! desk@1.1.0   hive  . ─▶ curator(ref board@1.1.0) ─▶ . ; feeder ─digest─▶ curator/summarizer
//! ```
//!
//! Both `desk` versions carry the same ref `override_params` (the enclosing
//! template's word on the nested hive and on its leaf).

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

// ─────────────────────────────────────────────────────────────────────────────
// The two classes
// ─────────────────────────────────────────────────────────────────────────────

/// An `echo_sub` leaf; `flavour` is what a version changes, `tone` is there
/// to be overridden by the enclosing template's ref, `mood` by the entry.
fn leaf(flavour: &str) -> String {
    format!(
        r#"{{"cell":{{"type":"echo_sub"}},"params":{{"echo_to":"/capture","flavour":"{flavour}","tone":"plain","mood":"plain"}},"contract":{{"version":"1.0.0","settings":{{}},"consumes":{{}}}}}}"#
    )
}

/// `board`: a hive around `summarizer`. 1.1.0 docks the `digest` v-lane at
/// `./summarizer` and gives the leaf other params.
fn write_board_templates(root: &std::path::Path) {
    let edges = r#""graph":{"edges":[
        {"from":".","to":"./summarizer"},
        {"from":"./summarizer","to":"."}]}"#;
    class(
        root,
        "board",
        "1.0.0",
        &format!(
            r#"{{"cell":{{"type":"hive"}},"params":{{
            "contract":{{"accepts":[{{"route":"in_view","because":"a view to sum up"}}]}},
            {edges}}}}}"#
        ),
        &[("summarizer", &leaf("a"))],
    );
    class(
        root,
        "board",
        "1.1.0",
        &format!(
            r#"{{"cell":{{"type":"hive"}},"params":{{
            "contract":{{"accepts":[
                {{"route":"in_view","because":"a view to sum up"}},
                {{"route":"digest","at":["./summarizer"],"because":"what the feeder hands in"}}]}},
            {edges}}}}}"#
        ),
        &[("summarizer", &leaf("b"))],
    );
}

/// `desk`: a composite holding `curator` through a ref to a `board`
/// version. Both versions carry the same ref `override_params` on the
/// nested hive's leaf. 1.1.0 adds `feeder` and the
/// `digest` v-lane from it onto `./curator/summarizer`.
fn write_desk_templates(root: &std::path::Path) {
    let marker = |board: &str| {
        format!(
            r#"{{"cell":{{"type":"ref","template":"board@{board}"}},"override_params":{{
                "summarizer":{{"tone":"desk"}}}}}}"#
        )
    };
    class(
        root,
        "desk",
        "1.0.0",
        r#"{"cell":{"type":"hive"},"params":{
            "contract":{"accepts":[{"route":"in_view","because":"put this view up"}]},
            "graph":{"edges":[
                {"from":".","to":"./curator","condition":"has(hop.route) && hop.route == 'in_view'"},
                {"from":"./curator","to":"."}]}}}"#,
        &[("curator", &marker("1.0.0"))],
    );
    class(
        root,
        "desk",
        "1.1.0",
        r#"{"cell":{"type":"hive"},"params":{
            "contract":{"accepts":[{"route":"in_view","because":"put this view up"}]},
            "graph":{"edges":[
                {"from":".","to":"./curator","condition":"has(hop.route) && hop.route == 'in_view'"},
                {"from":"./curator","to":"."},
                {"from":"./feeder","to":"./curator/summarizer","lane":"digest"}]}}}"#,
        &[("curator", &marker("1.1.0")), ("feeder", &leaf("feed"))],
    );
}

fn read_json(p: &std::path::Path) -> JsonValue {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap()
}

// ─────────────────────────────────────────────────────────────────────────────
// The lock
// ─────────────────────────────────────────────────────────────────────────────

/// Lifting `desk` 1.0.0 → 1.1.0 renews the nested `curator` like a fresh
/// instantiation would: a v-lane drawn onto `curator/summarizer` afterwards
/// commits (no `v_lane_no_connect_point`), `curator`'s `config.json` carries the 1.1.0
/// contract with the `digest` connect point, its `cell.id` is the one it was
/// born with, and below it the enclosing template's ref override and the
/// entry's params (the birth's, repeated by the lift) stand in the leaf.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lift_renews_the_contract_of_a_nested_hive() {
    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_board_templates(td.path());
    write_desk_templates(td.path());
    let colony = start_colony(&td).await;

    let grown = submit_diff(
        &colony.h,
        "/alex",
        json!({
            "add_nodes": [{"name": "display", "template": "desk@1.0.0",
                           "override_params": {"curator/summarizer": {"mood": "born"}}}],
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
        matches!(grown, MutationOutcome::Committed { .. }),
        "growing desk@1.0.0 must commit; got {grown:?}"
    );
    let curator = td.path().join("main/alex/display/curator");
    let born = read_json(&curator.join("config.json"));
    let born_id = born["cell"]["id"].clone();

    let lifted = submit_diff(
        &colony.h,
        "/alex",
        json!({"replace_nodes": [
            {"match": {"name": "display"},
             "with": {"template": "desk@1.1.0",
                      "params": {"curator/summarizer": {"mood": "born"}}}}
        ]}),
    )
    .await;
    assert!(
        matches!(lifted, MutationOutcome::Committed { .. }),
        "lifting desk to 1.1.0 must commit — the nested curator carries the new \
         connect point; got {lifted:?}"
    );

    // The migration proof's next entry: a v-lane from outside onto the leaf,
    // judged by the contract the nested hive now stands with.
    let drawn = submit_diff(
        &colony.h,
        "/alex",
        json!({"add_edges": [
            {"from": "./sender", "to": "./display/curator/summarizer", "lane": "digest"}
        ]}),
    )
    .await;
    assert!(
        matches!(drawn, MutationOutcome::Committed { .. }),
        "the v-lane onto curator/summarizer must find the 1.1.0 connect point; got {drawn:?}"
    );

    let renewed = read_json(&curator.join("config.json"));
    let accepts = renewed["params"]["contract"]["accepts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        accepts
            .iter()
            .any(|l| l["route"] == "digest" && l["at"] == json!(["./summarizer"])),
        "curator carries the 1.1.0 contract: {renewed}"
    );
    assert_eq!(renewed["cell"]["id"], born_id, "curator keeps its identity");
    assert_eq!(
        renewed["cell"]["provenance"]["template_version"], "1.1.0",
        "{renewed}"
    );
    let summarizer = read_json(&curator.join("summarizer/config.json"));
    assert_eq!(summarizer["params"]["flavour"], "b", "{summarizer}");
    assert_eq!(summarizer["params"]["tone"], "desk", "{summarizer}");
    assert_eq!(summarizer["params"]["mood"], "born", "{summarizer}");
}

// ─────────────────────────────────────────────────────────────────────────────
// Fix round 1: two levels deep, the rollback, the dropped lane, the class
// rule and the double lift
// ─────────────────────────────────────────────────────────────────────────────

/// The refusal's `error_code` and rendered details, or a panic naming `why`.
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

/// `case`: a hive around `inner`, a ref to a `board` version; `tower`: a
/// composite around `curator`, a ref to a `case` version. Lifting `tower`
/// renews two nested hives at once — `curator` and `curator/inner`.
fn write_tower_templates(root: &std::path::Path) {
    let inner_edges = r#""graph":{"edges":[
        {"from":".","to":"./inner"},
        {"from":"./inner","to":"."}]}"#;
    for (v, board) in [("1.0.0", "1.0.0"), ("1.1.0", "1.1.0")] {
        class(
            root,
            "case",
            v,
            &format!(
                r#"{{"cell":{{"type":"hive"}},"params":{{
                "contract":{{"accepts":[{{"route":"in_view","because":"a view to file"}}]}},
                {inner_edges}}}}}"#
            ),
            &[(
                "inner",
                &format!(r#"{{"cell":{{"type":"ref","template":"board@{board}"}}}}"#),
            )],
        );
        class(
            root,
            "tower",
            v,
            r#"{"cell":{"type":"hive"},"params":{
                "contract":{"accepts":[{"route":"in_view","because":"put this view up"}]},
                "graph":{"edges":[
                    {"from":".","to":"./curator","condition":"has(hop.route) && hop.route == 'in_view'"},
                    {"from":"./curator","to":"."}]}}}"#,
            &[(
                "curator",
                &format!(r#"{{"cell":{{"type":"ref","template":"case@{v}"}}}}"#),
            )],
        );
    }
}

/// Grow `display` from `tpl` under `/alex`, wired like the lock above.
async fn grow_display(h: &ColonyHandle, tpl: &str) {
    let grown = submit_diff(
        h,
        "/alex",
        json!({
            "add_nodes": [{"name": "display", "template": tpl}],
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
        matches!(grown, MutationOutcome::Committed { .. }),
        "growing {tpl} must commit; got {grown:?}"
    );
}

async fn lift_display(h: &ColonyHandle, tpl: &str) -> MutationOutcome {
    submit_diff(
        h,
        "/alex",
        json!({"replace_nodes": [{"match": {"name": "display"}, "with": {"template": tpl}}]}),
    )
    .await
}

/// Two levels deep in ONE lift: `tower` 1.0.0 → 1.1.0 renews `curator` and
/// `curator/inner` alike, and a v-lane onto `curator/inner/summarizer`
/// finds the connect point the 1.1.0 `board` declares.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_lift_renews_hives_two_levels_deep() {
    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_board_templates(td.path());
    write_tower_templates(td.path());
    let colony = start_colony(&td).await;
    grow_display(&colony.h, "tower@1.0.0").await;
    let inner = td.path().join("main/alex/display/curator/inner");
    let born_id = read_json(&inner.join("config.json"))["cell"]["id"].clone();

    let lifted = lift_display(&colony.h, "tower@1.1.0").await;
    assert!(
        matches!(lifted, MutationOutcome::Committed { .. }),
        "lifting tower to 1.1.0 must commit; got {lifted:?}"
    );
    let drawn = submit_diff(
        &colony.h,
        "/alex",
        json!({"add_edges": [
            {"from": "./sender", "to": "./display/curator/inner/summarizer", "lane": "digest"}
        ]}),
    )
    .await;
    assert!(
        matches!(drawn, MutationOutcome::Committed { .. }),
        "the v-lane two levels down must find the 1.1.0 connect point; got {drawn:?}"
    );
    let renewed = read_json(&inner.join("config.json"));
    assert!(
        renewed["params"]["contract"]["accepts"]
            .as_array()
            .is_some_and(|a| a.iter().any(|l| l["route"] == "digest")),
        "inner carries the 1.1.0 contract: {renewed}"
    );
    assert_eq!(renewed["cell"]["id"], born_id, "inner keeps its identity");
    let curator = read_json(&td.path().join("main/alex/display/curator/config.json"));
    assert_eq!(
        curator["cell"]["provenance"]["template_version"], "1.1.0",
        "{curator}"
    );
}

/// `board@1.2.0` promises a lane `extra` it opens no door for — refused by
/// the post-state lane-door check, after the lift laid its files. The
/// rollback puts the nested hive's declaration back byte for byte.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_lift_restores_the_nested_declaration() {
    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_board_templates(td.path());
    write_desk_templates(td.path());
    class(
        td.path(),
        "board",
        "1.2.0",
        r#"{"cell":{"type":"hive"},"params":{
            "contract":{"accepts":[
                {"route":"in_view","because":"a view to sum up"},
                {"route":"extra","because":"a lane with no door"}]},
            "graph":{"edges":[
                {"from":".","to":"./summarizer","condition":"has(hop.route) && hop.route == 'in_view'"},
                {"from":"./summarizer","to":"."}]}}}"#,
        &[("summarizer", &leaf("c"))],
    );
    class(
        td.path(),
        "desk",
        "1.2.0",
        r#"{"cell":{"type":"hive"},"params":{
            "contract":{"accepts":[{"route":"in_view","because":"put this view up"}]},
            "graph":{"edges":[
                {"from":".","to":"./curator","condition":"has(hop.route) && hop.route == 'in_view'"},
                {"from":"./curator","to":"."}]}}}"#,
        &[(
            "curator",
            r#"{"cell":{"type":"ref","template":"board@1.2.0"}}"#,
        )],
    );
    let colony = start_colony(&td).await;
    grow_display(&colony.h, "desk@1.0.0").await;
    let curator = td.path().join("main/alex/display/curator/config.json");
    let born = std::fs::read(&curator).unwrap();

    let (code, details) = refused(
        lift_display(&colony.h, "desk@1.2.0").await,
        "a nested lane without a door",
    );
    assert_eq!(code, "hive_contract", "{details}");
    assert!(details.contains("extra"), "{details}");
    assert_eq!(
        std::fs::read(&curator).unwrap(),
        born,
        "the nested declaration is the one of the birth again"
    );
}

/// `shelf@1.1.0` drops the lane `note` that an outer edge states into the
/// nested `curator` — refused before staging, like a lane the lifted hive
/// itself drops (Spec § 2.6), and nothing on disk moves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_nested_hive_loses_no_lane_somebody_hangs_on() {
    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    let edges = r#""graph":{"edges":[
        {"from":".","to":"./summarizer"},
        {"from":"./summarizer","to":"."}]}"#;
    for (v, accepts) in [
        (
            "1.0.0",
            r#"[{"route":"in_view","because":"a view"},{"route":"note","because":"a note"}]"#,
        ),
        ("1.1.0", r#"[{"route":"in_view","because":"a view"}]"#),
    ] {
        class(
            td.path(),
            "shelf",
            v,
            &format!(
                r#"{{"cell":{{"type":"hive"}},"params":{{"contract":{{"accepts":{accepts}}},{edges}}}}}"#
            ),
            &[("summarizer", &leaf("a"))],
        );
        class(
            td.path(),
            "bureau",
            v,
            r#"{"cell":{"type":"hive"},"params":{
                "contract":{"accepts":[
                    {"route":"in_view","because":"put this view up"},
                    {"route":"note","at":["./curator"],"because":"a note for the curator"}]},
                "graph":{"edges":[
                    {"from":".","to":"./curator","condition":"has(hop.route) && hop.route == 'in_view'"},
                    {"from":"./curator","to":"."}]}}}"#,
            &[(
                "curator",
                &format!(r#"{{"cell":{{"type":"ref","template":"shelf@{v}"}}}}"#),
            )],
        );
    }
    let colony = start_colony(&td).await;
    grow_display(&colony.h, "bureau@1.0.0").await;
    let drawn = submit_diff(
        &colony.h,
        "/alex",
        json!({"add_edges": [
            {"from": "./sender", "to": "./display/curator", "lane": "note",
             "modifier": {"set_hop": {"route": "'note'"}}}
        ]}),
    )
    .await;
    assert!(
        matches!(drawn, MutationOutcome::Committed { .. }),
        "the note lane onto the nested curator must commit; got {drawn:?}"
    );
    let curator = td.path().join("main/alex/display/curator/config.json");
    let before = std::fs::read(&curator).unwrap();

    let (code, details) = refused(
        lift_display(&colony.h, "bureau@1.1.0").await,
        "dropping note under a standing note edge",
    );
    assert_eq!(code, "hive_contract", "{details}");
    assert!(
        details.contains("note")
            && details.contains("/alex/sender")
            && details.contains("/alex/display/curator"),
        "the refusal names the lane, the edge and the nested hive: {details}"
    );
    assert_eq!(std::fs::read(&curator).unwrap(), before);
}

/// A version that turns the nested hive `curator` into a single cell is no
/// lift of it: refused with the class rule, nothing written.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_nested_hive_does_not_become_a_leaf() {
    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_board_templates(td.path());
    write_desk_templates(td.path());
    class(
        td.path(),
        "desk",
        "1.3.0",
        r#"{"cell":{"type":"hive"},"params":{
            "contract":{"accepts":[{"route":"in_view","because":"put this view up"}]},
            "graph":{"edges":[
                {"from":".","to":"./curator","condition":"has(hop.route) && hop.route == 'in_view'"},
                {"from":"./curator","to":"."}]}}}"#,
        &[("curator", &leaf("flat"))],
    );
    let colony = start_colony(&td).await;
    grow_display(&colony.h, "desk@1.0.0").await;
    let curator = td.path().join("main/alex/display/curator/config.json");
    let before = std::fs::read(&curator).unwrap();

    let (code, details) = refused(
        lift_display(&colony.h, "desk@1.3.0").await,
        "a nested hive lifted to a leaf",
    );
    assert_eq!(code, "schema", "{details}");
    assert!(
        details.contains("display/curator") && details.contains("keeps the node's class"),
        "{details}"
    );
    assert_eq!(std::fs::read(&curator).unwrap(), before);
}

/// Two entries of one diff on one tree — `display` and the nested
/// `display/curator` — would stage the curator twice: refused, nothing written.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_diff_does_not_lift_a_tree_twice() {
    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_board_templates(td.path());
    write_desk_templates(td.path());
    let colony = start_colony(&td).await;
    grow_display(&colony.h, "desk@1.0.0").await;
    let curator = td.path().join("main/alex/display/curator/config.json");
    let before = std::fs::read(&curator).unwrap();

    let (code, details) = refused(
        submit_diff(
            &colony.h,
            "/alex",
            json!({"replace_nodes": [
                {"match": {"name": "display"}, "with": {"template": "desk@1.1.0"}},
                {"match": {"name": "display/curator"}, "with": {"template": "board@1.1.0"}}
            ]}),
        )
        .await,
        "a lift inside a lift",
    );
    assert_eq!(code, "schema", "{details}");
    assert!(details.contains("staged twice"), "{details}");
    assert_eq!(std::fs::read(&curator).unwrap(), before);
}
