//! GH #512 — the derived menu keeps the two tools the collector serves ITSELF.
//!
//! Since GH #464 a collector's tool menu is asked for rather than typed:
//! `params.tools` names what the agent uses, the tools hive answers with the
//! declarations, and the collector writes them into the brain's own
//! `system.tools` — with `$replace`, so the subtree becomes exactly the answer.
//!
//! Two names on that menu are not the parent's. `memory_recall` is answered by
//! the collector out of its own recall port and `thread_recall` out of its own
//! slate (GH #55, GH #451); the composite routes both here by name, and no tools
//! hive has — or could have — a declaration for either. They shipped as SEED
//! rows in the brain's `cell.db`, which is written once at birth, so the first
//! tick deleted both of them. Measured on three grown colonies: the brain held
//! `web_search` and `web_fetch` and nothing else, the whole recall chain below
//! it was wired and idle, and the agent answered questions about its own past
//! with "my memory is not reachable" — true, and never asked.
//!
//! What this file pins:
//!
//! 1. **The declarations are the collector's.** A menu written on `in_menu`
//!    carries the hive's answer AND the names this cell answers itself, named on
//!    the message in `hop.menu_self`.
//! 2. **The switches decide, and there is no new one.** `memory_call_tier` and
//!    `thread_recall` already decide whether the lane is answered at all instead
//!    of refused with a typed error; empty means the tool is off, and off means
//!    undeclared.
//! 3. **The guard still comes first.** An answer with nothing usable writes
//!    nothing at all — the self-served names are not evidence that the hive
//!    answered, and a menu carrying only them would still be the revocation
//!    `$replace` makes of an empty write.
//! 4. **The hive wins a collision.** A parent that wired a real cell behind one
//!    of the two names has overridden this collector, and no menu declares one
//!    tool twice.
//! 5. **The shipped composites match their own graphs.** `talky` and, since
//!    `cogny@4.4.0` (GH #528), `cogny` route both names into their collector and
//!    leave both switches on. The rule is the agreement, not the answer: until
//!    4.4.0 the core routed neither `memory_recall` edge nor declared the tool,
//!    and that was the same rule read the other way round.
//! 6. **End to end on a booted colony.** The shipped `talky` beside the shipped
//!    `tools`, with no seed anywhere: the tick fires by itself and all four
//!    declarations land in the brain's own `cell.db`.

#[path = "mock_openai.rs"]
mod mock_openai;

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Message, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use meclaw_testing::{code_stdin, emit_all, run_shipped_script, shipped_script};
use mock_openai::MockOpenAI;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ───────────────────────────────────────────────────────────── the shipped tree

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// R2b / GH #49: a tree without the template SKIPS instead of failing.
fn shipped(name: &str) -> Option<std::path::PathBuf> {
    let root = templates_root().join(name);
    root.join("config.json").exists().then_some(root)
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

const ASSEMBLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/collector/assemble/config.json"
);
const SCHEMAS_CELL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/tools/schemas/config.json"
);

const MEM: &str = "memory_recall";
const THREAD: &str = "thread_recall";

/// GH #277: a `cell.type: "ref"` directory is a REFERENCE — the referenced
/// template's tree belongs in its place.
fn resolve_template_ref(dir: &std::path::Path) -> std::path::PathBuf {
    let mut dir = dir.to_path_buf();
    for _ in 0..8 {
        let Ok(raw) = std::fs::read_to_string(dir.join("config.json")) else {
            return dir;
        };
        let Ok(v) = meclaw_core::serde_json::from_str::<Value>(&raw) else {
            return dir;
        };
        if v["cell"]["type"] != "ref" {
            return dir;
        }
        let reference = v["cell"]["template"]
            .as_str()
            .expect("a ref cell names a template");
        dir = templates_root().join(reference.split('@').next().unwrap_or_default());
    }
    panic!("template ref chain does not terminate at {}", dir.display());
}

/// The shipped template, copied cell by cell: only `config.json` files travel,
/// so the tree under test IS the template — and, for this file, so that NO seed
/// travels with it. The measurement is what the collector writes, never what a
/// brain was born holding.
fn copy_cells(src: &std::path::Path, dst: &std::path::Path) {
    let src = &resolve_template_ref(src);
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        if from.is_dir() {
            copy_cells(&from, &dst.join(entry.file_name()));
        } else if entry.file_name() == "config.json" {
            std::fs::copy(&from, dst.join("config.json")).unwrap();
        }
    }
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

fn patch(root: &std::path::Path, rel: &str, f: impl FnOnce(&mut Value)) {
    let p = root.join(rel);
    let mut v: Value = read_json(&p);
    f(&mut v);
    std::fs::write(&p, meclaw_core::serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

const SCHEDULE_ID: &str = "0190a3f2-0000-7000-8000-000000000512";
const NEVER: &str = "0 0 0 1 1 *";
const EVERY_SECOND: &str = "* * * * * *";

// ═══════════════════════════════ 1.-4. the menu, over the shipped assembler

/// One question at the shipped `tools/schemas` cell, the way its own hive asks it.
fn ask_the_hive(names: &Value) -> Value {
    let out = emit_all(
        &shipped_script(SCHEMAS_CELL),
        &json!({
            "target": "/main/tools/schemas",
            "header": {"hop": {"route": "in_schemas"}, "context": {}},
            "ttl": 64,
            "tools": names,
            "messages": [],
        }),
    );
    assert_eq!(out.len(), 1, "the hive answers once");
    out.into_iter().next().expect("one answer")
}

fn run_assembler(doc: &Value) -> Vec<Value> {
    let out = run_shipped_script(&shipped_script(ASSEMBLE), &doc.to_string());
    assert!(out.status.success(), "the assembler exited non-zero");
    match meclaw_core::serde_json::from_slice(&out.stdout).expect("json out") {
        Value::Array(a) => a,
        other => vec![other],
    }
}

/// Phase 1 of the answer lane (GH #529): what the collector RECORDS for the
/// answerer that just spoke. Since the menu became a union over answerers it is
/// no longer written straight out — the submenu is stored, and the write is
/// derived from every stored row.
fn record_of(declared: &Value, knobs: &[(&str, Value)]) -> Vec<Value> {
    let answer = ask_the_hive(declared);
    let mut params = json!({"tools": declared.clone()});
    for (k, v) in knobs {
        params[*k] = v.clone();
    }
    run_assembler(&code_stdin(&json!({
        "target": "/main/collector",
        "header": {"hop": {"route": "in_menu"}, "context": {}},
        "ttl": 64,
        "messages": [],
        "schemas": answer["schemas"].clone(),
        "unknown": answer["unknown"].clone(),
        "params": params,
    })))
}

/// The row one recorded answer would put in the store, read off the `insert`
/// leg of the bundle the cell emitted. Reading it back rather than writing one
/// here is what keeps this helper honest: a change to the row shape moves both
/// halves of the round trip at once.
fn recorded_row(recorded: &[Value]) -> Value {
    let msg = recorded.first().expect("phase 1 emits one bundle");
    let leg = msg["messages"]
        .as_array()
        .expect("a bundle of tool_calls")
        .iter()
        .find(|m| m["id"] == json!("c-menu-put"))
        .expect("the bundle inserts this answerer's submenu");
    let args: Value =
        meclaw_core::serde_json::from_str(leg["text"].as_str().expect("a tool_call text"))
            .expect("the op is json");
    args["row"].clone()
}

/// Phase 2: the merge, driven with the rows a store would hand back.
fn merged(rows: Vec<Value>, knobs: &[(&str, Value)], declared: &Value) -> Vec<Value> {
    let mut params = json!({"tools": declared.clone()});
    for (k, v) in knobs {
        params[*k] = v.clone();
    }
    run_assembler(&code_stdin(&json!({
        "target": "/main/collector",
        "header": {"hop": {"route": "cstore", "operation": "bundle"},
                   "context": {"col_phase": "menu-merge"}},
        "ttl": 64,
        "messages": [{"id": "c-menu-all", "type": "tool_result",
                      "text": meclaw_core::serde_json::to_string(&rows).unwrap()}],
        "results": [{"tool_call_id": "c-menu-all", "operation": "select"}],
        "params": params,
    })))
}

/// The answer lane end to end over the shipped assembler with ONE answerer:
/// record what the shipped hive answered, hand that row back as the only row in
/// the store, and read the menu derived from it.
fn menu_of(declared: &Value, knobs: &[(&str, Value)]) -> Vec<Value> {
    let recorded = record_of(declared, knobs);
    if recorded.is_empty() {
        return recorded;
    }
    merged(vec![recorded_row(&recorded)], knobs, declared)
}

fn leaves(msg: &Value) -> BTreeSet<String> {
    msg["system"]["tools"]
        .as_object()
        .expect("the subtree is an object")
        .keys()
        .filter(|k| *k != "$replace")
        .cloned()
        .collect()
}

fn want(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|s| (*s).to_string()).collect()
}

/// Claim 1. The two names the collector answers itself are on the menu it
/// writes, and the message says which ones they were.
#[test]
fn the_menu_carries_the_two_tools_this_cell_answers_itself() {
    let out = menu_of(&json!(["web_search", "web_fetch"]), &[]);
    assert_eq!(out.len(), 1, "one menu message: {out:#?}");
    let msg = &out[0];
    assert_eq!(
        leaves(msg),
        want(&["web_fetch", "web_search", MEM, THREAD]),
        "a tool the composite IMPLEMENTS is topology, and its declaration belongs to \
         the cell that answers the call — not to a seed row written once at birth and \
         replaced by the first tick: {msg:#?}"
    );
    assert_eq!(
        msg["header"]["menu_count"], "4",
        "the receipt counts what was WRITTEN"
    );
    assert_eq!(
        msg["header"]["menu_self"],
        format!("{MEM},{THREAD}"),
        "and names the ones no hive answered for: {msg:#?}"
    );
    for name in [MEM, THREAD] {
        let raw = msg["system"]["tools"][name]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("`{name}` must be a `text` leaf: {msg:#?}"));
        let decl: Value = meclaw_core::serde_json::from_str(raw).expect("a leaf holds json");
        assert_eq!(decl["type"], "function");
        assert_eq!(decl["function"]["name"], name);
        assert!(
            decl["function"]["parameters"]["properties"].is_object(),
            "a declaration without arguments is one the model cannot use: {decl:#?}"
        );
    }
}

/// Claim 2. No new switch: the two that already decide whether the lane is
/// ANSWERED decide whether it is DECLARED. A collector that declares a tool it
/// would refuse and one that refuses a tool it declared are the same defect.
#[test]
fn a_switched_off_lane_is_an_undeclared_tool() {
    let both_off = menu_of(
        &json!(["web_search"]),
        &[
            ("memory_call_tier", json!("")),
            ("thread_recall", json!("")),
        ],
    );
    assert_eq!(
        leaves(&both_off[0]),
        want(&["web_search"]),
        "with both lanes off the menu is the hive's answer and nothing else: {both_off:#?}"
    );
    assert_eq!(both_off[0]["header"]["menu_self"], "");

    let no_memory = menu_of(&json!(["web_search"]), &[("memory_call_tier", json!(""))]);
    assert_eq!(
        leaves(&no_memory[0]),
        want(&["web_search", THREAD]),
        "`memory_call_tier` empty answers every memory call with a typed error, so the \
         tool must not be on the menu: {no_memory:#?}"
    );

    let no_thread = menu_of(&json!(["web_search"]), &[("thread_recall", json!(""))]);
    assert_eq!(
        leaves(&no_thread[0]),
        want(&["web_search", MEM]),
        "and the same holds one lane over: {no_thread:#?}"
    );
}

/// Claim 3. The guard that refuses an empty write comes first and is untouched.
#[test]
fn an_answer_with_nothing_usable_still_writes_nothing_at_all() {
    let out = menu_of(&json!(["telepathy"]), &[]);
    assert!(
        out.is_empty(),
        "the self-served names are not evidence that the hive answered; a menu carrying \
         only them would still revoke the model's whole tool set, because `tools_slot` \
         is a `$replace`: {out:#?}"
    );
}

/// Claim 4. A parent that wired a real cell behind one of the two names has
/// overridden this collector, and the menu never declares a tool twice.
#[test]
fn a_declaration_the_hive_answered_wins_over_this_cells_own() {
    let answer = ask_the_hive(&json!(["web_search"]));
    let mut schemas = answer["schemas"].as_array().cloned().expect("an array");
    schemas.push(json!({
        "name": MEM,
        "description": "a memory cell the parent wired itself",
        "parameters": {"type": "object", "properties": {"q": {"type": "string"}}}
    }));
    let declared = json!(["web_search", MEM]);
    let recorded = run_assembler(&code_stdin(&json!({
        "target": "/main/collector",
        "header": {"hop": {"route": "in_menu"}, "context": {}},
        "ttl": 64,
        "messages": [],
        "schemas": schemas,
        "unknown": [],
        "params": {"tools": declared.clone()},
    })));
    let out = merged(vec![recorded_row(&recorded)], &[], &declared);
    let msg = &out[0];
    assert_eq!(
        leaves(msg),
        want(&["web_search", MEM, THREAD]),
        "one leaf per name, and the parent's is the one that survives: {msg:#?}"
    );
    assert_eq!(
        msg["header"]["menu_self"], THREAD,
        "only the name nobody else answered is this cell's own: {msg:#?}"
    );
    let decl: Value = meclaw_core::serde_json::from_str(
        msg["system"]["tools"][MEM]["text"]
            .as_str()
            .expect("a leaf"),
    )
    .expect("json");
    assert_eq!(
        decl["function"]["description"], "a memory cell the parent wired itself",
        "the hive's declaration, verbatim: {decl:#?}"
    );
}

// ══════════════════════ 5. the shipped composites match their own graphs

fn routes_into_the_collector(composite: &str, tool: &str) -> bool {
    let cfg = read_json(&templates_root().join(composite).join("config.json"));
    cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("a composite has edges")
        .iter()
        .any(|e| {
            e["to"] == json!("./collector")
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains(&format!("hop.tool_name == '{tool}'")))
        })
}

fn collector_override(composite: &str, key: &str) -> Option<Value> {
    let cfg = read_json(
        &templates_root()
            .join(composite)
            .join("collector/config.json"),
    );
    cfg["override_params"]["assemble"]
        .get(key)
        .filter(|v| !v.is_null())
        .cloned()
}

/// The declaration and the edge are one statement. A composite that routes a
/// name into its collector must leave that lane switched on, and one that does
/// not route it must switch it off — otherwise the model is handed a tool whose
/// call leaves the composite through the guarded default and dies at a hive that
/// never heard of it.
#[test]
fn the_shipped_composites_declare_exactly_the_lanes_they_route() {
    if shipped("talky").is_some() {
        assert!(
            routes_into_the_collector("talky", MEM) && routes_into_the_collector("talky", THREAD),
            "talky routes both names into its collector (GH #55)"
        );
        assert_ne!(
            collector_override("talky", "memory_call_tier"),
            Some(json!("")),
            "so it must not switch the memory lane off"
        );
        assert_ne!(
            collector_override("talky", "thread_recall"),
            Some(json!("")),
            "nor the thread lane"
        );
    }
    if shipped("cogny").is_some() {
        // Since `cogny@4.4.0` (GH #528) the core routes BOTH names. `4.3.1`
        // routed neither `memory_recall` edge nor declared the tool, and the two
        // halves agreed then exactly as they agree now: the rule is that the
        // declaration and the edge are one statement, not that the core has no
        // memory tool.
        assert!(
            routes_into_the_collector("cogny", MEM),
            "the core asks its memory by TOOL since GH #528 -- one ordinary edge \
             on `{MEM}` keeps the call inside the composite"
        );
        assert_ne!(
            collector_override("cogny", "memory_call_tier"),
            Some(json!("")),
            "so it must not switch the memory lane off"
        );
        assert!(
            routes_into_the_collector("cogny", THREAD),
            "the thread lane it does route too (GH #451)"
        );
        assert_ne!(
            collector_override("cogny", "thread_recall"),
            Some(json!("")),
            "nor the thread lane"
        );
    }
}

// ══════════════════════════════════════════════ 6. the booted colony

fn main_config(composite: &str) -> Value {
    let hive = format!("./{composite}");
    json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
        {"from": hive.clone(), "to": "./tools",
         "condition": "has(hop.route) && hop.route == 'schemas'",
         "modifier": {"set_hop": {"route": "'in_schemas'"},
                      "set_context": {"tool_caller": "'surface'"}}},
        {"from": "./tools", "to": hive.clone(),
         "condition": "has(hop.route) && hop.route == 'tool_schemas'",
         "modifier": {"set_hop": {"route": "'in_menu'"}}},
        {"from": hive, "to": "/park"},
        {"from": "./tools", "to": "/park",
         "condition": "has(hop.route) && hop.route != 'tool_schemas'"}
    ]}}})
}

/// A `code` stand-in for a tool occupant this file never calls.
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
    for entry in std::fs::read_dir(tools_root).expect("the tools hive copied") {
        let dir = entry.expect("a directory entry").path();
        if !dir.is_dir() {
            continue;
        }
        let cfg = dir.join("config.json");
        if !cfg.exists() {
            continue;
        }
        if read_json(&cfg)["cell"]["type"] == "code" {
            continue;
        }
        std::fs::write(
            &cfg,
            meclaw_core::serde_json::to_string_pretty(&tool_double()).unwrap(),
        )
        .unwrap();
    }
}

fn build_tree(td: &tempfile::TempDir, composite: &str, declared: &Value, base_url: &str) {
    let root = td.path();
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\nKEEPER_IDLE_MS=0\nSEARCH_API_KEY=\n",
    )
    .unwrap();
    write(root, "main/config.json", &main_config(composite));
    copy_cells(
        &templates_root().join(composite),
        &root.join(format!("main/{composite}")),
    );
    copy_cells(&templates_root().join("tools"), &root.join("main/tools"));
    double_the_tool_cells(&root.join("main/tools"));

    let keeper = root.join(format!("main/{composite}/session-keeper/night/config.json"));
    if keeper.exists() {
        patch(
            root,
            &format!("main/{composite}/session-keeper/night/config.json"),
            |v| {
                v["params"]["schedules"][0]["schedule_id"] = json!(SCHEDULE_ID);
                v["params"]["schedules"][0]["cron"] = json!(NEVER);
            },
        );
    }
    patch(
        root,
        &format!("main/{composite}/collector/menu-clock/config.json"),
        |v| v["params"]["schedules"][0]["cron"] = json!(EVERY_SECOND),
    );
    patch(
        root,
        &format!("main/{composite}/collector/assemble/config.json"),
        |v| v["params"]["tools"] = declared.clone(),
    );
    patch(root, &format!("main/{composite}/brain/config.json"), |v| {
        v["params"]["base_url"] = json!(base_url);
        v["params"]["model"] = json!("gpt-4o-mock");
    });
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let factories = || -> Vec<(String, Arc<dyn CellFactory>)> {
        vec![
            (
                "code".to_string(),
                Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
            ),
            ("store".to_string(), Arc::new(StoreCellFactory)),
            ("timer".to_string(), Arc::new(TimerCellFactory)),
            ("llm".to_string(), Arc::new(LlmCellFactory)),
        ]
    };
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (park_tx, park_rx) = mpsc::channel::<Message>(256);
    h.spawn(Path::new("/park"), move || {
        CaptureCell::new(park_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, park_rx)
}

fn brain_slots(td: &tempfile::TempDir, composite: &str) -> Vec<String> {
    let p = td.path().join(format!("main/{composite}/brain/cell.db"));
    if !p.exists() {
        return Vec::new();
    }
    let Ok(conn) = rusqlite::Connection::open(&p) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare("SELECT slot_path FROM system ORDER BY slot_path") else {
        return Vec::new();
    };
    match stmt.query_map([], |r| r.get::<_, String>(0)) {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Claim 6. The shipped talky, beside the shipped tools hive, with no seed row
/// anywhere in the tree: the tick fires by itself, and the agent's own `cell.db`
/// ends up holding the two it declared AND the two it can answer for itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grown_talky_ends_up_with_a_memory_tool_it_never_seeded() {
    let (Some(_), Some(_)) = (shipped("talky"), shipped("tools")) else {
        return;
    };
    let mock = MockOpenAI::start(vec![]).await;
    let td = tempfile::tempdir().unwrap();
    build_tree(
        &td,
        "talky",
        &json!(["web_search", "web_fetch"]),
        &mock.base_url,
    );
    let (_h, _park) = boot(&td).await;

    for _ in 0..1500 {
        let names: BTreeSet<String> = brain_slots(&td, "talky")
            .into_iter()
            .filter_map(|p| p.strip_prefix("tools.").map(str::to_string))
            .collect();
        if names.len() >= 4 {
            assert_eq!(
                names,
                want(&["web_fetch", "web_search", MEM, THREAD]),
                "the two it declared and the two it answers itself"
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "the menu never reached talky/brain's own cell.db; it holds {:?}",
        brain_slots(&td, "talky")
    );
}
