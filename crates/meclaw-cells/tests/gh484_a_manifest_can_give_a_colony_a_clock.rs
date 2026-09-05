//! GH #484 -- nothing in the library was a clock, so no manifest could give a
//! hive a periodic tick.
//!
//! The substrate has had a `timer` cell type since phase 9, and nine shipped
//! templates carry one INSIDE them. None of those was instantiable on its own,
//! and `add_nodes` has no form for a bare cell -- `name` and `template` are both
//! required. `templates/firewall/README.md` names the consequence in its own
//! prose: `in_sweep` "needs a producer with a clock: a `timer` cell in the
//! parent, or an operator", and the library had nothing to wire.
//!
//! Four claims are pinned here, and the last one is the issue itself:
//!
//! 1. THE TEMPLATE IS ONE TIMER CELL AND NOTHING ELSE. A clock that carried a
//!    condition, a store or a body would be a second scheduler.
//! 2. THE SCHEDULE KEY IS A LITERAL. `${uuid7:...}` is an instantiation
//!    substitution with no filesystem-side producer, so a tree written straight
//!    to disk refuses to BOOT on it (`unsupported_substitution`) -- which is how
//!    a hand-built colony is written and how this template is read here.
//! 3. THE PROSE AND THE CONFIG NAME THE SAME CADENCE (development-rules § 2d).
//!    The number is derived from the config inside the test rather than typed
//!    into the assertion.
//! 4. A MANIFEST CAN GIVE A RUNNING COLONY A PERIODIC TICK. A colony boots
//!    without a clock, one declaration instantiates this template and draws one
//!    edge off it, and the tick lands on the lane the PARENT stamped -- the
//!    template names no lane of its own.

use meclaw_cells::TimerCellFactory;
use meclaw_colony::api_dto::ReadRegistryReply;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, Message, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const TEMPLATE: &str = "clock";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn template_dir() -> std::path::PathBuf {
    repo("templates").join(TEMPLATE)
}

fn json_at(rel: &str) -> Value {
    let raw = std::fs::read_to_string(template_dir().join(rel))
        .unwrap_or_else(|e| panic!("templates/{TEMPLATE}/{rel}: {e}"));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{rel} is not JSON: {e}"))
}

/// The `${VAR:-default}` default of a late-binding token, or the string itself
/// when it carries no token. This is what a boot without a `.env` sees.
fn env_default(raw: &str) -> String {
    let Some(rest) = raw.strip_prefix("${") else {
        return raw.to_string();
    };
    let inner = rest.strip_suffix('}').expect("unterminated ${...}");
    match inner.split_once(":-") {
        Some((_, d)) => d.to_string(),
        None => String::new(),
    }
}

/// Every `${VAR:-default}` in a blob of JSON, replaced by its default -- what a
/// boot without a `.env` writes into the instance.
fn resolve_tokens(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail.find('}').expect("unterminated ${...}");
        out.push_str(&env_default(&format!("${{{}}}", &tail[..end])));
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// The shipped schedule key, as a boot without an env file resolves it.
fn schedule_key() -> String {
    env_default(
        the_one_schedule()["schedule_id"]
            .as_str()
            .expect("schedule_id"),
    )
}

fn the_one_schedule() -> Value {
    let cfg = json_at("config.json");
    let arr = cfg["params"]["schedules"]
        .as_array()
        .expect("params.schedules")
        .clone();
    assert_eq!(
        arr.len(),
        1,
        "one cell, one schedule: a template that shipped two would be deciding \
         which of them a parent edge meant"
    );
    arr[0].clone()
}

// ===================================================================== THE FORM

#[test]
fn the_library_holds_a_clock_and_it_is_one_timer_cell() {
    let tpl = json_at("template.json");
    assert_eq!(tpl["name"], json!(TEMPLATE));
    assert_eq!(tpl["version"], json!("1.0.1"));
    for slot in ["purpose", "use_when", "not_in_scope", "examples"] {
        assert!(
            !tpl["description"][slot].is_null(),
            "template.json needs the {slot} slot the catalogue serves"
        );
    }

    let cfg = json_at("config.json");
    assert_eq!(
        cfg["cell"]["type"],
        json!("timer"),
        "the whole content of this template is a timer cell"
    );

    // "One cell and nothing else" as a property of the DIRECTORY: a sub-cell
    // would be a second actor with an opinion, which is exactly what the issue
    // asked not to be built.
    let mut files: Vec<String> = std::fs::read_dir(template_dir())
        .expect("the template directory")
        .map(|e| {
            let e = e.expect("dir entry");
            assert!(
                !e.file_type().expect("file type").is_dir(),
                "a clock has no sub-cells: {:?}",
                e.path()
            );
            e.file_name().to_string_lossy().to_string()
        })
        .collect();
    files.sort();
    assert_eq!(
        files,
        vec![
            "README.md".to_string(),
            "config.json".to_string(),
            "template.json".to_string()
        ],
        "three files: the declaration, the cell, and the prose that explains it"
    );

    // It emits and it does not decide: no condition, no filter, no store.
    let params = &cfg["params"];
    for forbidden in ["graph", "ports", "script_inline", "tables"] {
        assert!(
            params[forbidden].is_null(),
            "a clock carries no {forbidden}: a clock that knew what it was \
             ticking for would be a second scheduler"
        );
    }

    // The name is its own word, not one taken from the substrate glossary
    // (development-rules § 3, ruled as Q19): `timer` is a cell TYPE.
    assert_ne!(
        TEMPLATE, "timer",
        "a template named after a built-in cell type is a review defect"
    );
}

#[test]
fn the_schedule_key_is_a_literal_a_filesystem_boot_can_read() {
    // The WHOLE file, not just the params block: the substitution engine reads
    // every string in the config, so a description that merely quoted the token
    // would be refused at instantiation exactly like a schedule seeded with one.
    let raw = std::fs::read_to_string(template_dir().join("config.json")).expect("config.json");
    assert!(
        !raw.contains("${uuid7"),
        "an instantiation substitution has no filesystem-side producer, so a \
         tree written straight to disk refuses to boot on it \
         (unsupported_substitution) -- and that is how this template is read here"
    );

    let sched = the_one_schedule();
    let id = env_default(sched["schedule_id"].as_str().expect("schedule_id"));
    meclaw_core::Uuid::parse_str(&id)
        .unwrap_or_else(|e| panic!("the schedule key must be a UUID a boot can parse: {e}"));

    // The lane a parent edge matches on is a constant, not a knob: an edge
    // written against it must not change under a colony-wide env value.
    assert_eq!(
        sched["schedule_name"],
        json!("tick"),
        "one schedule, one name, and the README wires an edge against it"
    );
    assert_eq!(
        sched["emit_to"],
        json!("."),
        "a tick has no destination of its own -- the edge gives it one, and \
         without an edge the dead-letter names the clock itself"
    );
    assert_eq!(
        sched["emit_body"],
        json!({"messages": []}),
        "a clock has nothing to say"
    );
}

/// Drift lock (development-rules § 2d): the cadence in the prose is DERIVED
/// from the config here, and the mechanism is asserted beside the sentence.
#[test]
fn the_prose_and_the_config_name_the_same_cadence() {
    let sched = the_one_schedule();
    let cron = env_default(sched["cron"].as_str().expect("cron"));

    // The mechanism: the seed really is what a spawn accepts -- a 6-field
    // Quartz expression the substrate parses, not a string somebody typed.
    let raw_params = meclaw_core::serde_json::to_string(&json_at("config.json")["params"])
        .expect("serialise params");
    let resolved: Value = meclaw_core::serde_json::from_str(&resolve_tokens(&raw_params))
        .expect("the resolved params are still JSON");
    let params = meclaw_cells::timer::params::TimerParams::parse(&resolved)
        .expect("the shipped seed must parse the way a spawn parses it");
    assert_eq!(params.schedules.len(), 1);

    let cfg = json_at("config.json");
    assert_eq!(
        cfg["contract"]["settings"]["cron"]["default"],
        json!(cron),
        "the declared default and the seeded cron are one number"
    );

    let readme = std::fs::read_to_string(template_dir().join("README.md")).expect("README.md");
    assert!(
        readme.contains(&format!("`{cron}`")),
        "the knob table must name the cadence the config actually ships"
    );
    assert!(
        readme.contains("one cell, one schedule, and no decision"),
        "the README states what this template is; the assertions above are what \
         keep the sentence true"
    );
    assert!(
        readme.contains("A missed firing is dropped rather than replayed"),
        "no catch-up is a promise about behaviour, so it is pinned"
    );

    // The per-instance form the README publishes is the FLAT one -- a single-cell
    // template has no inner cell to address, and the path-keyed form is refused
    // with `schema`. The mechanism half of this half-sentence is the headline
    // test below, which grows the clock through exactly this shape.
    assert!(
        readme.contains("On a single-cell template it is a **flat** params object"),
        "the README must publish the override form the validator accepts"
    );

    let tpl = json_at("template.json");
    let contract: &str = tpl["description"]["not_in_scope"].as_str().expect("slot");
    assert!(
        contract.contains("No catch-up"),
        "the catalogue row says the same thing the README does"
    );
}

// =================================================================== THE COLONY

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![(
        "timer".to_string(),
        Arc::new(TimerCellFactory) as Arc<dyn CellFactory>,
    )]
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create destination");
    for entry in std::fs::read_dir(src).expect("read source") {
        let entry = entry.expect("dir entry");
        let target = dst.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copy");
        }
    }
}

/// A colony with a root hive, no clock, and a library that holds this template.
/// No env file: the cadence the declaration below carries is the documented
/// per-instance form, and it is turned down to one second so the test measures
/// the wiring rather than the wall clock.
fn tree() -> tempfile::TempDir {
    let td = tempfile::TempDir::new().expect("tempdir");
    let root = td.path();
    std::fs::create_dir_all(root.join("main")).expect("root hive dir");
    std::fs::write(
        root.join("main/config.json"),
        r#"{"cell":{"type":"hive"},"params":{"graph":{"edges":[]}}}"#,
    )
    .expect("write the root hive");
    copy_tree(&template_dir(), &root.join("templates").join(TEMPLATE));
    td
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (tx, rx) = mpsc::channel::<Message>(16);
    h.spawn(Path::new("/sink"), move || CaptureCell::new(tx.clone()))
        .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("a colony without a clock boots");
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan sent");
    ack_rx
        .await
        .expect("rescan acked")
        .expect("the library must register");
    (h, rx)
}

async fn mutate(h: &ColonyHandle, payload: Value) -> meclaw_colony::MutationOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("mutation sent");
    ack_rx.await.expect("mutation acked")
}

async fn registry_paths(h: &ColonyHandle) -> Vec<String> {
    let (ack_tx, ack_rx) = oneshot::channel::<ReadRegistryReply>();
    h.inbox_tx
        .send(ColonyMsg::ReadRegistry {
            path: None,
            path_prefix: None,
            cell_type: None,
            active: None,
            limit: 1000,
            ack: ack_tx,
        })
        .await
        .expect("registry read sent");
    ack_rx
        .await
        .expect("registry read acked")
        .entries
        .into_iter()
        .map(|e| e.path)
        .collect()
}

/// **The load-bearing test.** The declaration in the issue, run against the
/// library: one `add_nodes` entry naming a template, one `add_edges` entry
/// stamping the lane, and a tick that arrives without a restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_manifest_can_give_a_running_colony_a_periodic_tick() {
    let td = tree();
    let (h, mut rx) = boot(&td).await;

    let before = registry_paths(&h).await;
    assert!(
        !before.iter().any(|p| p == "/sweeper"),
        "the point of the exercise is that the colony had no clock: {before:?}"
    );

    let outcome = mutate(
        &h,
        json!({"scope": "/", "diff": {
            "add_nodes": [{"name": "sweeper", "template": "clock@1.0.1",
                "override_params": {"schedules": [{
                    "schedule_id": schedule_key(), "schedule_name": "tick",
                    "cron": "* * * * * *", "emit_to": ".",
                    "emit_body": {"messages": []}, "emit_headers": {}}]}}],
            "add_edges": [{
                "from": "./sweeper", "to": "./sink",
                "condition": "has(hop.schedule_name) && hop.schedule_name == 'tick'",
                "modifier": {"set_hop": {"route": "'in_sweep'"}}
            }]
        }}),
    )
    .await;
    assert!(
        matches!(outcome, meclaw_colony::MutationOutcome::Committed { .. }),
        "a clock must be nameable by an ordinary declaration: {outcome:?}"
    );

    let after = registry_paths(&h).await;
    assert!(
        after.iter().any(|p| p == "/sweeper"),
        "the clock did not grow: {after:?}"
    );

    // 30 s is the repo's failure-marker convention; the seeded cron fires every
    // second, so a clock that never ticks still fails in bounded time.
    let tick = tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("a clock that never fired")
        .expect("the sink channel closed");

    assert_eq!(
        tick.headers.hop.get("route"),
        Some(&json!("in_sweep")),
        "the lane is the PARENT's word: the template names none"
    );
    assert_eq!(tick.headers.hop.get("schedule_name"), Some(&json!("tick")));
    assert_eq!(
        tick.headers.hop.get("schedule_id"),
        Some(&json!(schedule_key())),
        "the key the runtime ops address rides on every firing"
    );
    for key in ["event_id", "scheduled_at", "fired_at", "iteration_n"] {
        assert!(
            tick.headers.hop.contains_key(key),
            "the firing must carry {key}: {:?}",
            tick.headers.hop
        );
    }
    match &tick.body {
        Body::Inline(v) => assert_eq!(
            v["messages"],
            json!([]),
            "a clock has nothing to say -- the tick is a trigger, not content"
        ),
        Body::Blob(_) => panic!("an empty trigger is never a blob"),
    }

    h.shutdown().await;
}
