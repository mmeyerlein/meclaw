//! GH #682 — the five cell types a shipped display hive is made of are
//! replaced in place twice at a running colony.
//!
//! The `meclaw-colony` half of the wave proves the lift on an echo fixture
//! (`gh682_a_standing_hive_is_lifted_in_place.rs`). This file puts the REAL
//! factories under it: a `web` (with its mount), a `store`, a `code`
//! (python3, inline script), a `timer` and an `llm` (no key — it only has to
//! wake, never to answer). It lives in `meclaw-cells/tests/` like the #673 and
//! #676 tests, because the claim needs the shipped factories and
//! `meclaw-colony` cannot depend on the crate that holds them.
//!
//! The fixture is one hive class `screen` in two versions that differ only in
//! the version line of `template.json` and the children's `contract.version`
//! (1.0.0 / 1.1.0), so every child is a `replaced` verdict on every lift:
//!
//! ```text
//! screen@<v>            accepts in_view
//!   . ─in_view─▶ web    ─▶ .          web    mount "screen"
//!   . ─in_view─▶ store  ─▶ .          store  one table
//!   . ─in_view─▶ code   ─▶ .          code   python3, echoes nothing
//!   . ─in_view─▶ timer  ─▶ .          timer  no schedule
//!   . ─in_view─▶ llm    ─▶ .          llm    keyless, loopback endpoint
//! ```
//!
//! Around it: `/world` (a timer, wired `/world -> /alex` so the persona hive
//! is connected, GH #265), the screen grown as `/alex/display`, and the sink
//! `/alex/display -> /alex/capture` — the one edge external to the screen
//! that keeps it awake and takes whatever leaves it.
//!
//! Waking: `web`, `code` and `timer` are eager kinds. The grow and each lift
//! register them inactive and the recompute that follows starts them through
//! their `RespawnFn` (the reconnect-eager arm), so the colony learns their
//! stop pair ONLY through `renotify_stop_wiring` — the #673/#676 form — and
//! that is what the next lift's recompute needs to stop them. `store` and
//! `llm` are lazy (Dormant) and wake on their first message; their live stop
//! pair is what the `WakeFn` hands back. One `in_view` probe at the hive path
//! fans out to all five and makes the picture uniform: five `Awake` rows
//! before every lift. The probe's fan-out is not ordered against the
//! registry read that follows, so the test polls the registry (bounded by
//! the 30s failure marker) rather than reading it once.
//!
//! What is asserted at each lift: it commits (in particular NOT
//! `stop_wiring_unavailable` — the lift's own recompute must peace-stop five
//! awake real cells and wait for their death-acks), the receipt names all
//! five as `replaced` with the right versions (and, on the second lift, the
//! five parked `~1.0.0` rows as `left`, the documented rule for a later
//! lift), and the registry afterwards holds five active rows under the
//! birth names plus the parked `~<version>` rows inactive. The `web` mount
//! is checked to be held after every lift: the old life gives it back and
//! the new life takes it (ADR-0031, same-path replacement).

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::{LlmCellFactory, TimerCellFactory, WebCellFactory};
use meclaw_colony::api_dto::RegistryEntryDto;
use meclaw_colony::mutation::{NodeChange, NodeVerdict};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, SurfaceRegistry,
    bootstrap_from_filesystem, set_term_timeout_ms_for_test,
};
use meclaw_core::serde_json::json;
use meclaw_core::{Body, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{mpsc, oneshot};

/// The five children of the screen, in registry order (sorted by path).
const CHILDREN: [&str; 5] = ["code", "llm", "store", "timer", "web"];

/// The mount the screen's `web` child holds.
const MOUNT: &str = "screen";

/// A `code` script that reads its input and emits nothing: the cell only has
/// to be awake, and an empty `messages` list is a legal answer.
const SCRIPT: &str =
    "import sys, json; json.load(sys.stdin); sys.stdout.write(json.dumps({\"messages\": []}))";

/// The failure-marker window of this repo: generous, robust under cargo
/// parallel load, never a semantic discriminator.
const MARKER: Duration = Duration::from_secs(30);

// ─────────────────────────────────────────────────────────────────────────────
// Fixture on disk
// ─────────────────────────────────────────────────────────────────────────────

fn write(dir: &std::path::Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

/// The `cell` block of a child: the two long-running kinds carry the
/// shipped `timeout: -1`, the rest the default.
fn cell_block(cell_type: &str) -> JsonValue {
    match cell_type {
        "web" | "timer" => json!({"type": cell_type, "timeout": -1}),
        other => json!({"type": other}),
    }
}

/// The spawn params of one child. Hermetic: no key, no network, no port —
/// the `llm` points at a loopback port nothing listens on, and it is never
/// asked to answer anyway.
fn params_for(cell_type: &str) -> JsonValue {
    match cell_type {
        "web" => json!({"mount": MOUNT, "external_timeout_ms": 5000}),
        "store" => json!({
            "schema": {"views": {"owner": "text", "content": "json"}},
            "query_timeout_ms": 5000
        }),
        "code" => json!({
            "runner": "python3",
            "script_inline": SCRIPT,
            "external_timeout_ms": 10000
        }),
        "timer" => json!({"query_timeout_ms": 5000}),
        "llm" => json!({
            "provider": "openai",
            "model": "none",
            "api_key": "",
            "base_url": "http://127.0.0.1:9",
            "external_timeout_ms": 2000
        }),
        other => panic!("no params for cell type {other}"),
    }
}

/// One child's `config.json` at the given contract version — the version is
/// the only thing that differs between the two template versions.
fn child_config(cell_type: &str, version: &str) -> String {
    json!({
        "cell": cell_block(cell_type),
        "params": params_for(cell_type),
        "contract": {"version": version, "settings": {}, "consumes": {}}
    })
    .to_string()
}

/// The hive's own `config.json`: it accepts `in_view`, every child has a door
/// on that lane and a way back to the hive path.
fn hive_config() -> String {
    let mut edges = Vec::new();
    for child in CHILDREN {
        edges.push(json!({
            "from": ".",
            "to": format!("./{child}"),
            "condition": "has(hop.route) && hop.route == 'in_view'"
        }));
        edges.push(json!({"from": format!("./{child}"), "to": "."}));
    }
    json!({
        "cell": {"type": "hive"},
        "params": {
            "contract": {"accepts": [
                {"route": "in_view", "because": "put this view up on the screen"}
            ]},
            "graph": {"edges": edges}
        }
    })
    .to_string()
}

/// `screen@1.0.0` and `screen@1.1.0` side by side under
/// `{templates}/local/screen@<version>/` (the shape a versioned registration
/// builds, GH #664): the same hive, the same five children, the children's
/// `contract.version` being the diff.
fn write_screen_templates(root: &std::path::Path) {
    for version in ["1.0.0", "1.1.0"] {
        let dir = root.join(format!("templates/local/screen@{version}"));
        write(
            &dir,
            "template.json",
            &format!(r#"{{"name":"screen","version":"{version}"}}"#),
        );
        write(&dir, "config.json", &hive_config());
        for child in CHILDREN {
            write(
                &dir,
                &format!("{child}/config.json"),
                &child_config(child, version),
            );
        }
    }
}

/// The persona `/alex` (an empty hive until the screen is grown) and the
/// `/world` timer whose edge into it keeps the persona connected.
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
        &child_config("timer", "1.0.0"),
    );
    write(
        root,
        "main/alex/config.json",
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// A running colony
// ─────────────────────────────────────────────────────────────────────────────

/// The five shipped factories, the `web` one over `surfaces` so the test can
/// read the mount table.
fn factory_list(surfaces: &Arc<SurfaceRegistry>) -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "web".to_string(),
            Arc::new(WebCellFactory::new(Arc::clone(surfaces))) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("code".to_string(), Arc::new(CodeCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
    ]
}

fn factory_registry(surfaces: &Arc<SurfaceRegistry>) -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    for (name, f) in factory_list(surfaces) {
        r.insert(name, f);
    }
    r
}

struct Colony {
    h: ColonyHandle,
    surfaces: Arc<SurfaceRegistry>,
    capture_rx: mpsc::Receiver<Message>,
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

/// Boot a colony over `td`: spawn the sink, rescan the library, bootstrap
/// from disk.
async fn start_colony(td: &TempDir) -> Colony {
    let surfaces = Arc::new(SurfaceRegistry::new());
    let h = ColonyHandle::new_with_factories_at(td, factory_list(&surfaces));
    rescan_templates(&h, td.path().join("templates")).await;

    let (capture_tx, capture_rx) = mpsc::channel(64);
    h.spawn(Path::new("/alex/capture"), move || {
        CaptureCell::new(capture_tx.clone())
    })
    .await;

    bootstrap_from_filesystem(td.path(), &factory_registry(&surfaces), &h.runtime())
        .await
        .expect("the persona topology must boot");
    Colony {
        h,
        surfaces,
        capture_rx,
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

/// Grow the screen at 1.0.0 as `/alex/display` with its one outer edge into
/// the sink. That edge is what connects the screen (GH #265); the probes go
/// to the hive path directly, so no sender cell is needed.
async fn grow_screen(h: &ColonyHandle) -> MutationOutcome {
    submit_diff(
        h,
        "/alex",
        json!({
            "add_nodes": [{"name": "display", "template": "screen@1.0.0"}],
            "add_edges": [{"from": "./display", "to": "./capture"}]
        }),
    )
    .await
}

/// Every registry row under `/alex/display/`, sorted by path.
async fn child_rows(h: &ColonyHandle) -> Vec<RegistryEntryDto> {
    let (ack_tx, ack_rx) = oneshot::channel::<meclaw_colony::api_dto::ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: Some(Path::new("/alex/display/")),
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

/// A source message on the `in_view` lane at the screen.
fn probe(text: &str) -> Message {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("in_view"));
    MessageBuilder::new(Path::new("/alex/display"))
        .body(Body::Inline(
            json!({"messages":[{"origin":"user","type":"text","text":text}]}),
        ))
        .hop(hop)
        .ttl(16)
        .build()
}

/// Send one `in_view` probe at the hive path — it fans out to all five
/// children — and wait until every one of them is `Awake` (the lazy `store`
/// and `llm` wake on it; the eager three already run). Bounded by the
/// failure marker; on a miss the rows and the dead letters are in the panic.
async fn wake_all(colony: &mut Colony, why: &str) {
    colony.h.send(probe(why)).await;
    let deadline = tokio::time::Instant::now() + MARKER;
    loop {
        let rows = child_rows(&colony.h).await;
        let awake: Vec<&str> = rows
            .iter()
            .filter(|r| r.active && r.lifecycle_status == "Awake")
            .map(|r| r.path.as_str())
            .collect();
        let expected: Vec<String> = CHILDREN
            .iter()
            .map(|c| format!("/alex/display/{c}"))
            .collect();
        if awake == expected {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            let picture: Vec<(String, bool, String)> = rows
                .into_iter()
                .map(|r| (r.path, r.active, r.lifecycle_status))
                .collect();
            let dlq = colony.h.drain_dead_letters().await;
            panic!("not all five children awake within 30s ({why}); rows {picture:?}; DLQ {dlq:?}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Lift `/alex/display` to `screen@<version>` and hand back the receipt's
/// `changes`. A refusal fails here, and `stop_wiring_unavailable` — the one
/// refusal this file is about — is named on its own.
async fn lift_to(h: &ColonyHandle, version: &str) -> Vec<NodeChange> {
    let outcome = submit_diff(
        h,
        "/alex",
        json!({"replace_nodes": [{"match": {"name": "display"},
                                  "with": {"template": format!("screen@{version}")}}]}),
    )
    .await;
    match outcome {
        MutationOutcome::Committed { changes, .. } => changes,
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_ne!(
                error_code, "stop_wiring_unavailable",
                "the lift to screen@{version} could not stop a running child: {details}"
            );
            panic!(
                "lifting display to screen@{version} must commit; got Rejected({error_code}): {details}"
            );
        }
    }
}

/// The receipt every lift of this fixture must produce: all five children
/// `replaced`, from `from` to `to`, and — the documented rule for a later
/// lift (overview § Mutation operations, `replace_nodes`) — every parked
/// `~<version>` sibling an earlier lift left behind reported as `left`
/// again, because the template does not name it either. Sorted by path.
fn all_replaced(from: &str, to: &str, parked_versions: &[&str]) -> Vec<NodeChange> {
    let mut changes = Vec::new();
    for c in CHILDREN {
        changes.push(NodeChange {
            path: format!("/alex/display/{c}"),
            verdict: NodeVerdict::Replaced,
            from_version: Some(from.to_string()),
            to_version: Some(to.to_string()),
        });
        for v in parked_versions {
            changes.push(NodeChange {
                path: format!("/alex/display/{c}~{v}"),
                verdict: NodeVerdict::Left,
                from_version: Some((*v).to_string()),
                to_version: None,
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    changes
}

/// `(path, cell_type, active)` of every row under the screen — the picture
/// the registry shows after a lift.
async fn registry_picture(h: &ColonyHandle) -> Vec<(String, String, bool)> {
    child_rows(h)
        .await
        .into_iter()
        .map(|r| (r.path, r.cell_type, r.active))
        .collect()
}

/// The expected picture: the five live rows plus, for each parked version,
/// five inactive `<name>~<version>` rows.
fn expected_picture(parked_versions: &[&str]) -> Vec<(String, String, bool)> {
    let mut rows = Vec::new();
    for c in CHILDREN {
        rows.push((format!("/alex/display/{c}"), c.to_string(), true));
        for v in parked_versions {
            rows.push((format!("/alex/display/{c}~{v}"), c.to_string(), false));
        }
    }
    rows.sort();
    rows
}

/// The mount must be on the table and its handoff channel live: the life
/// that holds it now is the one the last lift spawned.
async fn assert_mount_held(colony: &Colony, why: &str) {
    let deadline = tokio::time::Instant::now() + MARKER;
    loop {
        let held = colony
            .surfaces
            .take_handoff(MOUNT)
            .await
            .is_some_and(|tx| !tx.is_closed());
        if held {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the mount {MOUNT:?} must be held by a live web cell ({why}); table {:?}",
            colony.surfaces.table().await
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The proof
// ─────────────────────────────────────────────────────────────────────────────

/// A screen of the five real types is grown at 1.0.0, all five children are
/// awake, it is lifted to 1.1.0 (five `replaced`), woken again, and lifted
/// back to 1.0.0 (five `replaced` again) — both lifts commit in one colony
/// lifetime, neither is refused `stop_wiring_unavailable`, and the registry
/// ends with five active children under their birth names beside ten
/// inactive parked rows, five per version left behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_screens_five_child_types_are_replaced_twice() {
    // Generous death-ack term-timeout: the refusal this test is about is the
    // stop-wiring guard, not a slow death-ack under cargo-parallel load.
    set_term_timeout_ms_for_test(30_000);

    let td = TempDir::new().unwrap();
    write_alex_topology(td.path());
    write_screen_templates(td.path());
    let mut colony = start_colony(&td).await;

    let grown = grow_screen(&colony.h).await;
    assert!(
        matches!(grown, MutationOutcome::Committed { .. }),
        "growing screen@1.0.0 must commit; got {grown:?}"
    );
    assert_eq!(
        registry_picture(&colony.h).await,
        expected_picture(&[]),
        "screen@1.0.0 has exactly the five children, all active"
    );
    wake_all(&mut colony, "wake_before_first_lift").await;
    assert_mount_held(&colony, "after the grow").await;

    // 1.0.0 → 1.1.0: every child is a new cell, every old one is stopped and
    // parked beside it as `<name>~1.0.0`.
    let changes = lift_to(&colony.h, "1.1.0").await;
    assert_eq!(
        changes,
        all_replaced("1.0.0", "1.1.0", &[]),
        "the first lift replaces all five children"
    );
    assert_eq!(
        registry_picture(&colony.h).await,
        expected_picture(&["1.0.0"]),
        "after the first lift: five active children, five parked 1.0.0 rows"
    );
    for c in CHILDREN {
        assert_eq!(
            read_contract_version(&td, c),
            "1.1.0",
            "{c} stands at 1.1.0 on disk"
        );
        assert_eq!(
            read_contract_version(&td, &format!("{c}~1.0.0")),
            "1.0.0",
            "the parked {c}~1.0.0 is the old 1.0.0 cell"
        );
    }
    wake_all(&mut colony, "wake_after_first_lift").await;
    assert_mount_held(&colony, "after the first lift").await;

    // 1.1.0 → 1.0.0: the same act back. The old 1.1.0 children park under
    // the OTHER version's suffix, so nothing collides with the 1.0.0 rows.
    let changes = lift_to(&colony.h, "1.0.0").await;
    assert_eq!(
        changes,
        all_replaced("1.1.0", "1.0.0", &["1.0.0"]),
        "the way back replaces all five children again and reports the parked 1.0.0 rows as left"
    );
    assert_eq!(
        registry_picture(&colony.h).await,
        expected_picture(&["1.0.0", "1.1.0"]),
        "after the second lift: five active children, ten parked rows"
    );
    for c in CHILDREN {
        assert_eq!(read_contract_version(&td, c), "1.0.0");
        assert_eq!(read_contract_version(&td, &format!("{c}~1.1.0")), "1.1.0");
    }
    // What the earlier probes left in the sink is drained first, so the
    // receive below can only be satisfied by THIS probe — a lift that had
    // dropped the outer edge would otherwise pass on the first probe's
    // leftovers.
    while colony.capture_rx.try_recv().is_ok() {}
    wake_all(&mut colony, "wake_after_second_lift").await;
    assert_mount_held(&colony, "after the second lift").await;

    // The last probe left the screen: the sink is reachable, so the outer
    // edge survived both lifts.
    let got = tokio::time::timeout(MARKER, colony.capture_rx.recv())
        .await
        .expect("/alex/capture receives within 30s")
        .expect("/alex/capture rx open");
    assert_eq!(got.target, Path::new("/alex/capture"));

    colony.h.shutdown().await;
}

/// `contract.version` of the child directory `name` under the standing
/// screen.
fn read_contract_version(td: &TempDir, name: &str) -> String {
    let path = td
        .path()
        .join("main/alex/display")
        .join(name)
        .join("config.json");
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let cfg: JsonValue = meclaw_core::serde_json::from_str(&raw).unwrap();
    cfg["contract"]["version"]
        .as_str()
        .expect("contract.version")
        .to_string()
}
