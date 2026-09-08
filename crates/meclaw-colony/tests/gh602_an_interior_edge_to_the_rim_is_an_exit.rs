//! GH #602 — an edge from inside a hive onto the hive's own path is an EXIT,
//! and an exit is measured against `emits`.
//!
//! # What paid for this file
//!
//! The catch-all the member ships (`./channels -> .`, stamping `hop.route =
//! 'error'`, GH #598) instantiates at boot with a warning at most, and was
//! refused as a live `add_edges` on two running colonies with the line
//!
//! ```text
//! sends hop.route='error' into hive <member>, which does not accept that lane
//! ```
//!
//! One edge, two verdicts: the validator read `to` alone, so an edge that
//! LEAVES the hive was judged as one that ENTERS it and was held against the
//! `accepts` list. `error` is a lane the member emits.
//!
//! Three things proven here:
//!
//! 1. **The shipped catch-all commits as a live mutation.** The member is grown
//!    from the shipped template, the edge is removed from its birth graph, and
//!    the very body of `templates/member/config.json` is applied as `add_edges`.
//! 2. **The same lane entering from OUTSIDE is still refused.** Same hive, same
//!    `hop.route`, same target — only the direction differs, and the direction
//!    is the whole verdict.
//! 3. **The boot judges the same way and WARNS.** An exit on a lane the hive
//!    does not emit produces a log line at boot, never a refusal: the birth
//!    topology is authorship.
//!
//! Guarded like every other template-reading test (GH #49): the public export
//! ships a subset of the library, and a template that did not travel is skipped
//! rather than judged.

use meclaw_colony::api_dto::ReadGraphReply;
use meclaw_colony::mutation::hive_contract::{
    BootEdge, HiveContract, Lane, warn_on_broken_contracts,
};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::factories::EchoCellFactory;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

// ───────────────────────────────────────────────────────────── the tree

/// A path inside this repository, from the crate's manifest directory.
fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped_member() -> Option<std::path::PathBuf> {
    let member = repo("templates/member");
    member.join("config.json").is_file().then_some(member)
}

/// Copy the template cell by cell: only `config.json` files travel, so the tree
/// under test IS the template and nothing else.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create the directory");
    for entry in std::fs::read_dir(src).expect("the template directory is readable") {
        let entry = entry.expect("directory entry");
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).expect("copy the config");
        }
    }
}

fn write_json(path: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(path.parent().expect("a parent directory")).expect("create");
    std::fs::write(
        path,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

fn read_json(path: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    meclaw_core::serde_json::from_str(&raw).expect("the config parses")
}

fn echo(path: &std::path::Path, emitted_target: &str) {
    write_json(
        path,
        &json!({
            "cell": {"type": "echo"},
            "params": {"emitted_target": emitted_target},
            "contract": {"version": "0.1.0", "settings": {}, "consumes": {}}
        }),
    );
}

/// The catch-all of `templates/member/config.json`, read out of the shipped file
/// rather than retyped: the body under test has to BE the shipped one, or the
/// test proves something about a sentence somebody wrote twice.
///
/// Returns the edge and the member's graph without it — the state the two
/// running colonies of GH #598 were in.
fn split_off_the_catch_all(cfg: &mut Value) -> Value {
    let edges = cfg["params"]["graph"]["edges"]
        .as_array_mut()
        .expect("the member ships a graph");
    let idx = edges
        .iter()
        .position(|e| {
            e["from"] == json!("./channels")
                && e["to"] == json!(".")
                && e["modifier"]["set_hop"]["route"] == json!("'error'")
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("hop.route == 'receipt'"))
        })
        .expect("the member ships the GH #598 catch-all `./channels -> .`");
    edges.remove(idx)
}

/// The colony around the member: a caller that can be wired to it, a sink, and
/// one edge onto the member path so its contract is not dormant.
fn main_config() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": "./caller", "to": "./person",
         "condition": "has(hop.route) && hop.route == 'in_turn'"},
        {"from": "./person", "to": "./sink",
         "condition": "has(hop.route) && hop.route == 'error'"}
    ]}}})
}

/// Grows the member from the shipped template and hands back the catch-all its
/// birth graph no longer carries.
fn build_tree(root: &std::path::Path, member: &std::path::Path) -> Value {
    write_json(&root.join("main/config.json"), &main_config());
    echo(&root.join("main/caller/config.json"), "/caller");
    echo(&root.join("main/sink/config.json"), "/sink");

    copy_cells(member, &root.join("main/person"));
    // The member's four holders arrive as `ref` markers; a marker is a template
    // reference and this tree ships no library. Each becomes an inert double,
    // exactly as `gh598_a_screen_receipt_is_not_a_turn.rs` does it — what is
    // measured here is the member's own rim, not what lives behind it.
    for holder in ["access", "affinity", "firewall", "memory-hive"] {
        echo(
            &root.join(format!("main/person/{holder}/config.json")),
            &format!("/person/{holder}"),
        );
    }

    let cfg_path = root.join("main/person/config.json");
    let mut cfg = read_json(&cfg_path);
    let catch_all = split_off_the_catch_all(&mut cfg);
    write_json(&cfg_path, &cfg);
    catch_all
}

// ───────────────────────────────────────────────────────────── the colony

fn echo_factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "echo".to_string(),
        Arc::new(EchoCellFactory) as Arc<dyn CellFactory>,
    )]
}

async fn boot(td: &tempfile::TempDir) -> ColonyHandle {
    let h = ColonyHandle::new_with_factories_at(td, echo_factories());
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in echo_factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped member must boot");
    h
}

async fn send_mutation(h: &ColonyHandle, payload: Value) -> MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("the colony takes the mutation");
    ack_rx.await.expect("the colony answers")
}

async fn read_graph(h: &ColonyHandle) -> ReadGraphReply {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadGraphReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadGraph {
            scope: Path::new("/"),
            ack: ack_tx,
        })
        .await
        .expect("the colony takes the read");
    ack_rx.await.expect("the colony answers")
}

fn skip() -> bool {
    if shipped_member().is_none() {
        eprintln!("templates/member did not travel into this tree -- skipped (GH #49)");
        return true;
    }
    false
}

// ═══════════════════════════════ 1. the shipped catch-all commits live

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_shipped_member_catch_all_commits_as_a_live_add_edges() {
    if skip() {
        return;
    }
    let Some(member) = shipped_member() else {
        return;
    };
    let td = tempfile::tempdir().expect("a temporary directory");
    let catch_all = build_tree(td.path(), &member);
    let h = boot(&td).await;

    let outcome = send_mutation(
        &h,
        json!({"scope": "/person", "diff": {"add_edges": [catch_all]}}),
    )
    .await;
    match &outcome {
        MutationOutcome::Committed { .. } => {}
        other => panic!(
            "an edge from inside the member onto its own path LEAVES the member on a lane it \
             emits -- it must commit, got {other:?}"
        ),
    }

    let edges: Vec<(String, String)> = read_graph(&h)
        .await
        .edges
        .iter()
        .map(|e| (e.from.clone(), e.to.clone()))
        .collect();
    assert!(
        edges.contains(&("/person/channels".to_string(), "/person".to_string())),
        "the exit is in the table: {edges:?}"
    );
    h.shutdown().await;
}

// ═══════════════════════════ 2. the same lane entering is still refused

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_lane_entering_the_member_from_outside_is_still_refused() {
    if skip() {
        return;
    }
    let Some(member) = shipped_member() else {
        return;
    };
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(td.path(), &member);
    let h = boot(&td).await;

    // Same hive, same lane, same target path. Only `from` moved outside — and
    // that is the entire difference between an exit and a dead letter.
    let outcome = send_mutation(
        &h,
        json!({"scope": "/", "diff": {"add_edges": [
            {"from": "./caller", "to": "./person",
             "modifier": {"set_hop": {"route": "'error'"}}}
        ]}}),
    )
    .await;
    match &outcome {
        MutationOutcome::Rejected {
            error_code,
            details,
            ..
        } => {
            assert_eq!(error_code, "hive_contract", "{outcome:?}");
            assert!(
                details.contains("error") && details.contains("in_turn"),
                "the refusal names the lane asked for and the lanes on offer: {details}"
            );
        }
        other => panic!("a lane the member does not accept must stay refused, got {other:?}"),
    }
    h.shutdown().await;
}

// ═════════════════════════════════════ 3. the boot warns and never refuses

#[derive(Clone, Default)]
struct Recorder {
    lines: Arc<Mutex<Vec<(tracing::Level, String)>>>,
}

impl Recorder {
    fn warnings(&self) -> Vec<String> {
        self.lines
            .lock()
            .expect("log mutex")
            .iter()
            .filter(|(l, _)| *l == tracing::Level::WARN)
            .map(|(_, m)| m.clone())
            .collect()
    }
}

/// Every field of an event, flattened — the lane a contract warning names rides
/// in `reason`, not in `message`.
struct FieldVisitor(String);

impl tracing::field::Visit for FieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!("{}={value:?} ", field.name()));
    }
}

impl tracing::Subscriber for Recorder {
    fn enabled(&self, _meta: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut v = FieldVisitor(String::new());
        event.record(&mut v);
        self.lines
            .lock()
            .expect("log mutex")
            .push((*event.metadata().level(), v.0));
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

fn lane(route: &str) -> Lane {
    Lane {
        route: route.into(),
        context: Vec::new(),
        at: Vec::new(),
        required: false,
        because: format!("the {route} lane"),
    }
}

fn stamping_boot_edge(from: &str, to: &str, cond: &str, route_expr: &str) -> BootEdge {
    (
        from.into(),
        to.into(),
        Some(cond.into()),
        Some(json!({"set_hop": {"route": route_expr}})),
        false,
    )
}

/// The boot half of the same rule: an exit on a lane the hive does not emit is
/// a line in the log and nothing else. The birth topology is authorship — a
/// colony that cannot boot is worse than one that boots loudly.
#[test]
fn the_boot_warns_about_an_exit_the_hive_does_not_emit() {
    let contracts = vec![HiveContract {
        hive_path: "/mem".into(),
        accepts: vec![lane("in_batch")],
        emits: vec![lane("episode")],
    }];
    let edges: Vec<BootEdge> = vec![
        (
            "/mem".into(),
            "/mem/glue".into(),
            Some("has(hop.route) && hop.route == 'in_batch'".into()),
            None,
            false,
        ),
        (
            "/mem/glue".into(),
            "/mem".into(),
            Some("has(hop.route) && hop.route == 'episode'".into()),
            None,
            false,
        ),
        // The offending one: it leaves the hive on a lane the contract never
        // promised a caller.
        stamping_boot_edge(
            "/mem/glue",
            "/mem",
            "has(hop.route) && hop.route == 'stray'",
            "'in_bath'",
        ),
    ];

    let rec = Recorder::default();
    tracing::subscriber::with_default(rec.clone(), || warn_on_broken_contracts(&contracts, &edges));

    let warnings = rec.warnings();
    assert!(
        warnings.iter().any(|w| w.contains("leaves hive")),
        "the boot has to say the exit out loud: {warnings:?}"
    );
}

/// And the green case stays silent: every door and every exit is declared, so
/// the boot has nothing to say.
#[test]
fn the_boot_is_silent_when_every_exit_is_declared() {
    let contracts = vec![HiveContract {
        hive_path: "/mem".into(),
        accepts: vec![lane("in_batch")],
        emits: vec![lane("episode")],
    }];
    let edges: Vec<BootEdge> = vec![
        (
            "/mem".into(),
            "/mem/glue".into(),
            Some("has(hop.route) && hop.route == 'in_batch'".into()),
            None,
            false,
        ),
        stamping_boot_edge(
            "/mem/glue",
            "/mem",
            "has(hop.finish_reason) && hop.finish_reason == 'stop'",
            "'episode'",
        ),
    ];

    let rec = Recorder::default();
    tracing::subscriber::with_default(rec.clone(), || warn_on_broken_contracts(&contracts, &edges));

    assert!(
        rec.warnings().is_empty(),
        "a sound hive produces no line: {:?}",
        rec.warnings()
    );
}
