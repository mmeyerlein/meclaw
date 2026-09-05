//! GH #553 — the menu and the screen follow the mutation receipt, and the two
//! poll timers are gone.
//!
//! Two cells in the shipped library asked a question that already had an
//! answer. `collector/menu-clock` woke every five minutes to ask the tools hive
//! for declarations that only change when somebody mutates the graph;
//! `colony-view/refresh` woke every minute to ask `/colony/graph` for a
//! topology that only changes for the same reason. In an event-driven substrate
//! that is a poll, and a poll costs availability: on a real colony the two of
//! them were roughly two thousand ticks a day, every one of them a full pass
//! through the colony loop, and the answer was identical to the last one in all
//! but a handful of cases.
//!
//! Since the substrate half of this issue the door itself says it: a committed
//! mutation leaves ONE terminal receipt at the hive `colony.json` names
//! (`mutation_receipts.to`), on the lane `mutation_committed` — and **the boot
//! is the first receipt** (ruling O-0904-2), so a colony that has just started
//! has already said "the graph moved" once before anybody touches it.
//!
//! What this file measures is the template half, end to end on a booted colony,
//! with no `timer` cell anywhere in the tree:
//!
//! 1. **The boot receipt reaches the screen.** The picture is drawn and
//!    published without a tick, so the display serves its route with `200`
//!    instead of the `404` an empty page map answers with.
//! 2. **A committed mutation refreshes both.** One `add_nodes` grows the tools
//!    hive beside the agent; the receipt that follows it drives the collector's
//!    menu ask (the brain's own `system.tools` fills up, having been EMPTY
//!    before) and the colony view's snapshot (the new cells appear on the
//!    screen). One mutation, both consumers, no timer.
//! 3. **No timer ships in either template.** `templates/collector/menu-clock`
//!    and `templates/colony-view/refresh` are gone, and the graphs that drew
//!    edges from them are gone with them (rule R3).
//! 4. **A receipt does not beget a receipt.** The colony is counted at rest:
//!    the lane fires once per commit and never feeds itself.
//!
//! Free of a real provider by construction: the agent's `llm` cell talks to the
//! mock OpenAI wire and the tool occupants are `code` doubles, so the file
//! spends nothing. Guarded like every template-reading test (GH #49): a tree
//! without the library is skipped, never judged.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::web::WebCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, ColonyMsg, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Message, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::free_port;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::MockOpenAI;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the library

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// Every template this file reads, next to the path it really reads — spelled
/// out rather than formatted, so the export's R2b check can see the names
/// (GH #9).
const NEEDED: [&str; 6] = [
    "templates/talky",
    "templates/collector",
    "templates/session-keeper",
    "templates/dispatcher",
    "templates/colony-view",
    "templates/display",
];

/// The tools hive arrives by MUTATION, so it travels as a template rather than
/// as a copied tree, together with the `web` template the display refs.
const GROWN: [&str; 2] = ["templates/tools", "templates/web"];

fn library_ships() -> bool {
    NEEDED
        .iter()
        .chain(GROWN.iter())
        .all(|rel| repo(rel).join("template.json").is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(p: &std::path::Path, f: impl FnOnce(&mut Value)) {
    let mut v = read_json(p);
    f(&mut v);
    write_json(p, &v);
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// Every `config.json` under `dir`, relative, sorted.
fn config_paths(dir: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(d: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(d) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, base, out);
            } else if p.file_name().is_some_and(|n| n == "config.json") {
                out.push(
                    p.strip_prefix(base)
                        .expect("under base")
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    walk(dir, dir, &mut out);
    out.sort();
    out
}

// ───────────────────────────────────────────────────────────── the fixture

/// A `code` stand-in for a tool occupant this file never calls: the tools hive
/// is grown for its DECLARATIONS, and a real `bash` or `web_search` cell would
/// only add a factory to the registry.
fn tool_double() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {
            "runner": "python3",
            "script_inline": "import sys, json\nsys.stdin.read()\nsys.stdout.write(json.dumps([]))\n",
            "external_timeout_ms": 10000
        },
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {"body": {"messages": {"type": "array", "required": true}},
                      "hop": {"operation": {"type": "string", "required": false}}},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in for a tool occupant this file never calls.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

fn double_the_tool_cells(tools_root: &std::path::Path) {
    for entry in std::fs::read_dir(tools_root).expect("the tools template copied") {
        let dir = entry.expect("a directory entry").path();
        let cfg = dir.join("config.json");
        if !dir.is_dir() || !cfg.exists() {
            continue;
        }
        if read_json(&cfg)["cell"]["type"] == "code" {
            continue;
        }
        write_json(&cfg, &tool_double());
    }
}

/// The colony this file boots: an agent, a colony view, a screen — and the ONE
/// place the receipt enters, which is the root hive itself.
///
/// The two edges out of `.` are the whole template half in miniature: the
/// receipt arrives on `mutation_committed` at the hive `colony.json` names, and
/// the level it lands in draws it onwards, one level at a time -- to the agent
/// and to the app, each of them a plain door at its own rim. The lane keeps ONE
/// name the whole way down; only the collector inside the agent turns it into
/// its internal `in_menu_tick`.
fn root_hive() -> Value {
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": ".", "to": "./agent",
         "condition": "has(hop.route) && hop.route == 'mutation_committed'"},
        {"from": ".", "to": "./view",
         "condition": "has(hop.route) && hop.route == 'mutation_committed'"},
        {"from": "./view", "to": "./screen",
         "condition": "has(hop.route) && hop.route == 'view'",
         "modifier": {"set_hop": {"route": "'in_view'"}}},
        {"from": "./view", "to": "/park",
         "condition": "has(hop.route) && hop.route == 'error'"},
        {"from": "./screen", "to": "/park"},
        {"from": "./agent", "to": "/park"}
    ]}}})
}

/// The mutation of claim 2: one `add_nodes` for the tools hive, plus the two
/// edges that bind it to the agent. Nothing about the receipt is in here — the
/// point is that an ORDINARY mutation is what moves the menu and the picture.
fn grow_the_tools_hive() -> Value {
    json!({"scope": "/", "diff": {
    "add_nodes": [{"name": "tools", "template": "tools"}],
    "add_edges": [
        {"from": "./agent", "to": "./tools",
         "condition": "has(hop.route) && hop.route == 'schemas'",
         "modifier": {"set_hop": {"route": "'in_schemas'"},
                      "set_context": {"tool_caller": "'surface'"}}},
        {"from": "./tools", "to": "./agent",
         "condition": "has(hop.route) && hop.route == 'tool_schemas'",
         "modifier": {"set_hop": {"route": "'in_menu'"}}}
    ]}})
}

fn build_root(td: &tempfile::TempDir, base_url: &str, port: u16) {
    let root = td.path();
    write_json(
        &root.join("colony.json"),
        &json!({
            "schema_version": 1,
            "mutation_receipts": {"to": "/"}
        }),
    );
    write_json(&root.join("main/config.json"), &root_hive());

    copy_tree(&repo("templates/talky"), &root.join("main/agent"));
    // `talky` REFERENCES its sub-units; a directly written tree carries them.
    for (name, rel) in [
        ("collector", "templates/collector"),
        ("session-keeper", "templates/session-keeper"),
        ("dispatcher", "templates/dispatcher"),
    ] {
        copy_tree(&repo(rel), &root.join("main/agent").join(name));
    }
    copy_tree(&repo("templates/colony-view"), &root.join("main/view"));
    copy_tree(&repo("templates/display"), &root.join("main/screen"));
    for rel in GROWN {
        let name = rel.rsplit('/').next().expect("a template name");
        copy_tree(&repo(rel), &root.join("templates").join(name));
    }
    double_the_tool_cells(&root.join("templates/tools"));

    patch(&root.join("main/screen/web/config.json"), |v| {
        v["override_params"][""]["port"] = json!(port)
    });
    patch(&root.join("main/agent/brain/config.json"), |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!("gpt-4o-mock");
    });
    patch(
        &root.join("main/agent/collector/assemble/config.json"),
        |v| v["params"]["tools"] = json!(["web_search", "web_fetch"]),
    );
    // The one timer this tree still carries is the keeper's nightly close, and
    // it is pushed past the run: what is measured here is the ABSENCE of a tick.
    let night = root.join("main/agent/session-keeper/night/config.json");
    if night.exists() {
        patch(&night, |v| {
            v["params"]["schedules"][0]["schedule_id"] =
                json!("0190a3f2-0000-7000-8000-000000000553");
            v["params"]["schedules"][0]["cron"] = json!("0 0 0 1 1 *");
        });
    }
    // Every open generation is a candidate the moment the sweep runs. It was a
    // `KEEPER_IDLE_MS=0` line in the `.env` below until GH #138; the knob is a
    // param of `./close` now, so such a line would be read by NOTHING.
    let close = root.join("main/agent/session-keeper/close/config.json");
    if close.exists() {
        patch(&close, |v| v["params"]["idle_ms"] = json!(0));
    }
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\nSEARCH_API_KEY=\n",
    )
    .unwrap();
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        ("llm".to_string(), Arc::new(LlmCellFactory)),
        ("web".to_string(), Arc::new(WebCellFactory)),
    ]
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (park_tx, park_rx) = mpsc::channel::<Message>(1024);
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    // The scan comes BEFORE the boot: the display refs `web@1.1.0`, and a growth
    // at boot time resolves against the templates table in `colony.db`, which is
    // empty until somebody fills it (GH #424).
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: td.path().join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx.await.expect("rescan ack").expect("rescan outcome");
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the tree must boot");
    (h, park_rx)
}

async fn mutate(h: &ColonyHandle, payload: Value) -> meclaw_colony::mutation::MutationOutcome {
    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::Mutation {
            payload,
            reply_to: None,
            trace_id: meclaw_core::Uuid::now_v7(),
            parent_message_id: meclaw_core::Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send mutation");
    ack_rx.await.expect("mutation ack")
}

/// The names the agent's own brain holds in `system.tools` — the durable end of
/// the menu road, one row per declaration.
fn menu_in_the_brain(td: &tempfile::TempDir) -> BTreeSet<String> {
    let p = td.path().join("main/agent/brain/cell.db");
    if !p.exists() {
        return BTreeSet::new();
    }
    let Ok(conn) = rusqlite::Connection::open(&p) else {
        return BTreeSet::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT slot_path FROM system ORDER BY slot_path") else {
        return BTreeSet::new();
    };
    match stmt.query_map([], |r| r.get::<_, String>(0)) {
        Ok(rows) => rows
            .filter_map(|r| r.ok())
            .filter_map(|p| p.strip_prefix("tools.").map(str::to_string))
            .collect(),
        Err(_) => BTreeSet::new(),
    }
}

/// GET the screen's own route until the listener answers at all.
async fn get_page(port: u16) -> (u16, String) {
    let url = format!("http://127.0.0.1:{port}/");
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match reqwest::get(&url).await {
            Ok(r) => {
                let status = r.status().as_u16();
                let body = r.text().await.unwrap_or_default();
                return (status, body);
            }
            Err(e) if Instant::now() < deadline => {
                let _ = e;
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(e) => panic!("the screen never came up on {url}: {e}"),
        }
    }
}

async fn wait_for_page(port: u16, wanted: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (status, body) = get_page(port).await;
        if status == 200 && body.contains(wanted) {
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "the screen never served a page containing {wanted:?}: status \
             {status}, {} bytes",
            body.len()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ════════════════════════════════════════════════════════════ claim 3

/// The two poll timers are gone from the library, and so is every edge that
/// named them — a cell removed without its edges is exactly what rule R3 of the
/// tree gate refuses.
#[test]
fn neither_template_ships_a_poll_timer_any_more() {
    if !library_ships() {
        return;
    }
    assert!(
        !repo("templates/collector/menu-clock").exists(),
        "templates/collector/menu-clock is back: the menu is asked for on the \
         mutation receipt now, not on a five-minute tick"
    );
    assert!(
        !repo("templates/colony-view/refresh").exists(),
        "templates/colony-view/refresh is back: the picture follows the \
         mutation receipt now, not a one-minute tick"
    );
    for (template, gone) in [
        ("templates/collector", "menu-clock"),
        ("templates/colony-view", "refresh"),
    ] {
        let cfg = read_json(&repo(template).join("config.json"));
        let edges = cfg["params"]["graph"]["edges"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !edges.iter().any(|e| {
                [e["from"].as_str(), e["to"].as_str()]
                    .iter()
                    .flatten()
                    .any(|p| *p == format!("./{gone}"))
            }),
            "{template} still draws an edge to or from ./{gone}, which no longer exists"
        );
    }
    // Positive half: the templates are still there and still ship their cells,
    // so the assertions above are about a deletion and not about a missing tree.
    assert!(config_paths(&repo("templates/collector")).len() >= 3);
    assert!(config_paths(&repo("templates/colony-view")).len() >= 3);
}

// ════════════════════════════════════════════════════════════ claims 1, 2, 4

/// The whole claim of the issue in one run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_boot_receipt_fills_the_screen_and_a_mutation_refreshes_both() {
    if !library_ships() {
        return;
    }
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::tempdir().unwrap();
    let port = free_port();
    build_root(&td, &mock.base_url, port);

    // No `timer` cell anywhere on the live path but the keeper's night close.
    let timers: Vec<String> = config_paths(td.path().join("main").as_path())
        .into_iter()
        .filter(|rel| read_json(&td.path().join("main").join(rel))["cell"]["type"] == "timer")
        .collect();
    assert_eq!(
        timers,
        vec!["agent/session-keeper/night/config.json".to_string()],
        "the grown tree still carries a poll timer"
    );

    let (h, park) = boot(&td).await;

    // --- claim 1: the boot receipt alone draws the picture and publishes it.
    // Nothing has ticked, nothing has been asked, and the screen answers.
    let first = wait_for_page(port, "agent/brain").await;
    assert!(
        !first.contains("tools/"),
        "precondition: the tools hive does not exist yet"
    );
    let before = menu_in_the_brain(&td);
    assert!(
        !before.contains("web_search") && !before.contains("web_fetch"),
        "precondition: no tools hive exists yet, so nothing has ANSWERED a menu \
         question — what the brain holds is what the collector serves out of its \
         own slate: {before:?}"
    );

    // --- claim 2: ONE ordinary mutation, and both consumers follow it.
    let outcome = mutate(&h, grow_the_tools_hive()).await;
    assert!(
        matches!(
            outcome,
            meclaw_colony::mutation::MutationOutcome::Committed { .. }
        ),
        "precondition: the tools hive grows; got {outcome:?}"
    );

    let after = wait_for_page(port, "tools/schemas").await;
    assert!(
        after.contains("agent/brain"),
        "the refreshed picture lost the agent it already had"
    );

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let names = menu_in_the_brain(&td);
        if names.contains("web_search") && names.contains("web_fetch") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the receipt never refreshed the menu; the brain holds {names:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // --- claim 4: the lane fires once per commit and never feeds itself. If a
    // receipt begat a receipt the colony would still be moving here.
    let quiet = park.len();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        park.len(),
        quiet,
        "the colony is still emitting after the mutation settled — a receipt \
         that triggers a mutation is the feedback loop GH #161 wedged on"
    );
}
