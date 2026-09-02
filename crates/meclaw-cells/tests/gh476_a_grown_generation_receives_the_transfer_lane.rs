//! GH #476 — a generation grown from a WISH receives the transfer lane, not
//! only one instantiated by hand out of the README.
//!
//! GH #475 opened the lane: `assistant@2.4.0` accepts `in_export` and
//! `in_import` and emits `dump`, and `member@1.5.0` carries an export into
//! `./assistants` when the caller names a generation on `context.assistant`.
//! Reaching a generation therefore costs FOURTEEN edges in the member's
//! container, not eleven — and the builder's `grow_level` recipe still rendered
//! eleven. A generation grown the fast way came up complete in every other
//! respect and could not receive the one store its member cannot recompute: an
//! `in_export` that named it stopped as `no_route` at `<member>/assistants`, and
//! so would an `in_import` part carrying its session ledger.
//!
//! The failure is silent in exactly the way GH #475 exists to end. An export
//! that walked three holders looks like a complete one: three directories, three
//! `export_done`, a member-level marker naming three hives, and nothing that
//! says the fourth was never asked. So this file measures the POSITIVE signal —
//! the keeper's ledger on disk, carrying the row a real turn opened — rather
//! than the absence of a complaint.
//!
//! Two claims, and the second is the one that costs a colony:
//!
//! 1. **The renderer draws the three edges the shipped template declares.** The
//!    lanes are read off `templates/assistant/config.json`'s own contract rather
//!    than written down here, so a template that grows a fourth transfer lane
//!    makes this test red instead of leaving the recipe quietly behind. The
//!    two doors carry the same `context.assistant` guard a turn does; the drain
//!    is PLAIN, because every level between the container and the keeper pairs
//!    `in_export` with `dump` in `params.required_drains` and the probe that
//!    checks the pairing runs the described hop through the real edge evaluator
//!    — an edge that additionally tested `hop.dump_kind` reads as no drain at
//!    all and the mutation is refused.
//! 2. **A generation grown from the rendered manifest answers the export that
//!    names it.** One colony, the shipped library, the member grown through the
//!    mutation door and two generations grown from what `grow_level` renders —
//!    nothing in the wiring is typed out in this file. A real turn opens a
//!    session in the first, an `in_export` naming it puts that very row on disk
//!    as `session-keeper/seed/sessions.jsonl`, the member-level marker names
//!    four hives, and the second generation stays empty: the name is an address,
//!    not a fan-out.
//!
//! **Substitutions, named rather than hidden.** The same two
//! `gh475_a_member_reaches_the_keeper_it_holds.rs` makes, for the same reasons:
//! the `llm` type is an inert lazy double (a generation carries three brains and
//! a spawned one opens a connection to a provider; the keeper's stamp sits
//! BEFORE the brain in `talky`'s graph, so a session is opened by a turn
//! arriving), and the shipped `export-sink` is run `trusted` through
//! `override_params` because its shipped `restricted` sandbox is fail-closed
//! against the host and would measure the kernel this runs on. Every other cell
//! type is the shipped factory.
//!
//! Guarded like every template-reading test (GH #49): a tree that does not carry
//! the library is skipped, never judged.

use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_cells::vault::VaultCellFactory;
use meclaw_cells::{
    BashCellFactory, EditCellFactory, FileCellFactory, McpCellFactory, WebFetchCellFactory,
    WebSearchCellFactory,
};
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationDoorOutcome, RespawnFn, SpawnedCellKind,
    WakeFn, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, Value, from_str, json, to_string_pretty};
use meclaw_core::{Body, JsonValue, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::{ColonyHandle, emit_one, shipped_script};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const RECV_TIMEOUT: Duration = Duration::from_secs(60);
const MEMBER: &str = "alex";
/// The generation the session is opened in and the export is addressed to.
const SCRIBE: &str = "scribe";
/// The second generation. It holds a keeper of its own and is never named — so
/// an export that reached "the assistant" instead of the one that was named is a
/// failure with a name rather than a smaller number.
const COACH: &str = "coach";
const CHANNEL: &str = "tg:476";
/// The round the turn is spoken in. A JSON LIST, because that is what the
/// memory hive's `audience_of()` parses and refuses anything else as
/// `missing_audience` (#244). It read as a comma-separated string here until
/// GH #527, and nothing was red: no shipped topology consumed `turn_write`, so
/// the malformed value never reached a writer. It does now.
const AUDIENCE: &str = r#"["member:alex","agent:scribe"]"#;

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/builder/recipes/config.json",
        "templates/member/config.json",
        "templates/member/export-sink/config.json",
        "templates/assistant/config.json",
        "templates/talky/config.json",
        "templates/session-keeper/config.json",
        "templates/session-keeper/sessions/config.json",
        "examples/memory-import/build_import.py",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

// ═══════════════════════════════════════════════════════════════ the recipe

/// One rendered `grow_level` declaration, produced by the SHIPPED recipe script.
fn grow_level(params: Value) -> Value {
    let out = emit_one(
        &shipped_script(
            repo("templates/builder/recipes/config.json")
                .to_str()
                .expect("a utf-8 path"),
        ),
        &json!({
            "target": "/os/builder/recipes",
            "header": {"hop": {"route": "recipe"}, "context": {}},
            "ttl": 64,
            "messages": [{"origin": "tool", "type": "tool_result", "id": "",
                          "text": json!({"recipe": "grow_level", "request": "grow a generation",
                                         "params": params}).to_string()}],
        }),
    );
    let decls = out["manifest"]
        .as_array()
        .unwrap_or_else(|| panic!("the recipe rendered no manifest: {out}"));
    assert_eq!(
        decls.len(),
        1,
        "a level is ONE declaration -- the node and its edges are one decision"
    );
    decls[0].clone()
}

/// The transfer edges the recipe renders for one generation, in the order it
/// writes them.
fn rendered_transfer_edges(name: &str) -> Vec<Value> {
    let decl = grow_level(json!({
        "scope": format!("/members/{MEMBER}"), "level": "assistant", "name": name,
        "template": "assistant@2.4.1"}));
    decl["diff"]["add_edges"]
        .as_array()
        .expect("add_edges")
        .iter()
        .filter(|e| {
            let c = e["condition"].as_str().unwrap_or_default();
            ["in_export", "in_import", "dump"]
                .iter()
                .any(|lane| c.contains(&format!("hop.route == '{lane}'")))
        })
        .cloned()
        .collect()
}

// ═════════════════════════════════════════════════════════ the inert brain

/// A lazy factory that accepts every params block and never runs anything. It
/// stands in for `llm`: three of them travel with a generation, and a spawned
/// one talks to a provider over the network.
///
/// `is_lazy() == true` registers the cell as `Dormant`. The `WakeFn` and
/// `RespawnFn` are reachable through a delivery and a restart, so they are
/// written correctly rather than left as `unimplemented!()` — a panic on either
/// path would take the whole colony task with it.
struct InertCellFactory;

impl CellFactory for InertCellFactory {
    fn validate_params(&self, _params: &JsonValue) -> Result<(), String> {
        Ok(())
    }

    fn is_lazy(&self) -> bool {
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_cell(
        self: Arc<Self>,
        _path: Path,
        _params: JsonValue,
        _outputs_tx: mpsc::Sender<meclaw_core::CellEmission>,
        _cell_dir: std::path::PathBuf,
        _contract: meclaw_colony::ContractView,
        _colony_inbox_tx: mpsc::Sender<ColonyMsg>,
        _idle_timeout: Option<Duration>,
        _cell_timeout: i64,
        _message_timeout: Option<Duration>,
        _blob_store: Option<Arc<meclaw_colony::DiskBlobStore>>,
        mailbox_capacity: usize,
    ) -> Result<SpawnedCellKind, String> {
        let capacity = mailbox_capacity.max(1);
        let (sender, receiver) = mpsc::channel::<Message>(capacity);

        let wake: WakeFn = Box::new(|mut rx: mpsc::Receiver<Message>| {
            tokio::spawn(async move { while rx.recv().await.is_some() {} });
            let (stop_tx, _stop_rx) = oneshot::channel::<()>();
            let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
            (stop_tx, death_ack_rx)
        });

        let respawn: RespawnFn = Box::new(move || {
            let (tx, mut rx) = mpsc::channel::<Message>(capacity);
            let (peace_tx, peace_rx) = oneshot::channel::<()>();
            let (_backstop_tx, backstop_rx) = oneshot::channel::<()>();
            let join = tokio::spawn(async move {
                let _peace_keep = peace_tx;
                while rx.recv().await.is_some() {}
            });
            (tx, join, peace_rx, backstop_rx)
        });

        let (stop_tx, _stop_rx) = oneshot::channel::<()>();
        let (_death_ack_tx, death_ack_rx) = oneshot::channel::<()>();
        Ok(SpawnedCellKind::Dormant {
            sender,
            receiver,
            wake,
            stop_tx,
            death_ack_rx,
            respawn,
        })
    }
}

fn factories() -> Vec<(String, Arc<dyn CellFactory>)> {
    vec![
        (
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        ),
        ("store".to_string(), Arc::new(StoreCellFactory)),
        ("timer".to_string(), Arc::new(TimerCellFactory)),
        // The tool surface a generation carries. None of it is exercised here;
        // it is registered because a cell type with no factory is a cell that
        // never enters the registry, and a generation missing its tools is not
        // the generation this file claims to have grown.
        ("bash".to_string(), Arc::new(BashCellFactory)),
        ("edit".to_string(), Arc::new(EditCellFactory)),
        ("file".to_string(), Arc::new(FileCellFactory)),
        ("mcp".to_string(), Arc::new(McpCellFactory)),
        ("vault".to_string(), Arc::new(VaultCellFactory)),
        ("web_fetch".to_string(), Arc::new(WebFetchCellFactory)),
        ("web_search".to_string(), Arc::new(WebSearchCellFactory)),
        ("llm".to_string(), Arc::new(InertCellFactory)),
    ]
}

fn registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    for (name, f) in factories() {
        r.insert(name, f);
    }
    r
}

// ══════════════════════════════════════════════════════════════ the plumbing

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

fn write_json(path: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, to_string_pretty(v).unwrap()).unwrap();
}

fn shipped_config(rel: &str) -> Value {
    from_str(&std::fs::read_to_string(repo(rel)).expect(rel)).expect("shipped config is json")
}

fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    if !db.is_file() {
        return Vec::new();
    }
    let conn = rusqlite::Connection::open(db).expect("open cell.db");
    let mut st = match conn.prepare(sql) {
        Ok(st) => st,
        // The store creates its tables when it first wakes; a keeper nobody has
        // spoken to yet has a file and no schema, and that is an empty ledger
        // rather than a defect.
        Err(_) => return Vec::new(),
    };
    let n = st.column_count();
    st.query_map([], |r| {
        Ok((0..n)
            .map(|i| {
                r.get::<_, Option<String>>(i)
                    .unwrap_or_default()
                    .unwrap_or_default()
            })
            .collect::<Vec<String>>())
    })
    .expect("query")
    .collect::<Result<Vec<_>, _>>()
    .expect("rows")
}

/// Every `${VAR}` the library references WITHOUT a default, bound to a dummy,
/// plus the four crons this file pushes out of the way. A nightly close, a menu
/// refresh, a dream or an identity push firing mid-run would emit into edges no
/// test topology drew — and the nightly close would end the very session the
/// export is supposed to carry.
fn dummy_env(source: &std::path::Path) -> String {
    let mut names = std::collections::BTreeSet::new();
    let mut stack = vec![source.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&p) else {
                continue;
            };
            let mut rest = raw.as_str();
            while let Some(start) = rest.find("${") {
                rest = &rest[start + 2..];
                let Some(end) = rest.find('}') else { break };
                let name = &rest[..end];
                if !name.contains(":-")
                    && !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                {
                    names.insert(name.to_string());
                }
                rest = &rest[end + 1..];
            }
        }
    }
    let mut out: String = names
        .into_iter()
        .map(|n| format!("{n}=dummy-{n}\n"))
        .collect();
    for cron in [
        "AFFINITY_PUSH_CRON",
        "MEMORY_DREAM_CRON",
        "KEEPER_NIGHT_CRON",
        "MENU_CRON",
    ] {
        out.push_str(&format!("{cron}=0 0 4 1 1 *\n"));
    }
    out
}

/// A code cell that appends every message it is handed to one file per lane, so
/// a wait can be a wait for something that HAD to arrive.
fn flag_cell(dir: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "flag_dir": dir, "sandbox": {"trust": "trusted"},
                   "script_inline": r#"
import sys, json, os
doc = json.load(sys.stdin)
hop = (doc["envelope"].get("header") or {}).get("hop") or {}
path = os.path.join(doc["params"]["flag_dir"], str(hop.get("route") or "unknown") + ".json")
seen = []
if os.path.exists(path):
    with open(path) as fh:
        seen = json.load(fh)
seen.append({"hop": hop})
with open(path, "w") as fh:
    fh.write(json.dumps(seen))
sys.stdout.write(json.dumps([]))
"#},
        "contract": {"version": "1.0.0", "settings": {}, "multi_send_capable": true,
                     "emits": {}, "consumes": {}}
    })
}

/// The shell the member is grown into: a members container, and a flag cell that
/// takes every lane the member level raises.
async fn boot(td: &tempfile::TempDir, flag_dir: &std::path::Path) -> ColonyHandle {
    let root = td.path();
    copy_tree(&repo("templates"), &root.join("templates"));
    std::fs::create_dir_all(flag_dir).unwrap();
    let lanes = [
        "answer",
        "ack",
        "reject",
        "error",
        "write",
        "turn_write",
        "prune",
        "build",
        "close_report",
        "export_done",
        "pack_ack",
    ];
    let mut edges = vec![
        json!({"from": ".", "to": "./members",
               "condition": "has(hop.route) && hop.route == 'in_turn'"}),
        json!({"from": ".", "to": "./members",
               "condition": "has(hop.route) && hop.route == 'in_export'"}),
        json!({"from": ".", "to": "./members",
               "condition": "has(hop.route) && hop.route == 'in_import'"}),
    ];
    for lane in lanes {
        edges.push(json!({"from": "./members", "to": "./flag",
                          "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
    }
    write_json(
        &root.join("main/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    write_json(
        &root.join("main/members/config.json"),
        &json!({"cell": {"type": "hive"}}),
    );
    write_json(
        &root.join("main/flag/config.json"),
        &flag_cell(flag_dir.to_str().unwrap()),
    );
    std::fs::write(root.join(".env"), dummy_env(&root.join("templates"))).unwrap();

    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::RescanTemplates {
            templates_root: root.join("templates"),
            ack: ack_tx,
        })
        .await
        .expect("rescan");
    ack_rx
        .await
        .expect("rescan ack")
        .expect("GH #440: the rescan must not have aborted");
    bootstrap_from_filesystem(root, &registry(), &h.runtime())
        .await
        .expect("the shell must boot");
    h
}

async fn apply(h: &ColonyHandle, payload: Value) -> MutationDoorOutcome {
    let (ack_tx, ack_rx) = oneshot::channel();
    h.inbox_tx
        .send(ColonyMsg::MutationDoor {
            payload,
            reply_to: None,
            trace_id: Uuid::now_v7(),
            parent_message_id: Uuid::now_v7(),
            ack: ack_tx,
        })
        .await
        .expect("send manifest");
    ack_rx.await.expect("manifest ack")
}

/// The container wiring `examples/memory-import/build_import.py` writes, read
/// from the shipped script rather than repeated here. The member's own level is
/// not what this file is measuring — the generation inside it is.
fn container_edges() -> Vec<Value> {
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(format!(
            "import json, runpy, sys\n\
             m = runpy.run_path({})\n\
             sys.stdout.write(json.dumps(m['edges']({})))\n",
            meclaw_core::serde_json::to_string(
                repo("examples/memory-import/build_import.py")
                    .to_str()
                    .unwrap()
            )
            .unwrap(),
            meclaw_core::serde_json::to_string(MEMBER).unwrap(),
        ))
        .output()
        .expect("python3");
    assert!(
        out.status.success(),
        "build_import.edges() did not run: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    from_str(&String::from_utf8_lossy(&out.stdout)).expect("edges are json")
}

fn member_manifest(export_dir: &std::path::Path) -> Value {
    let sink = shipped_config("templates/member/export-sink/config.json");
    assert_eq!(
        sink["params"]["sandbox"]["trust"], "restricted",
        "the shipped sink is the one behind a boundary; if this ever reads \
         `trusted`, the substitution below is hiding a real regression"
    );
    let over = json!({"export-sink": {"sandbox": {"trust": "trusted"},
                                      "export_dir": export_dir.to_str().unwrap()}});
    json!({"manifest": [{
        // The declaration stands AT the container it grows into (GH #503),
        // which is the form `build_import.py` writes and the form
        // `container_edges()` above is spelled in: `.` is `/members`, the
        // member is named bare, and the path it lands at is unchanged.
        "scope": "/members",
        "diff": {
            "add_nodes": [{"name": MEMBER, "template": "member@1.5.1",
                           "override_params": over}],
            "add_edges": container_edges(),
        }
    }]})
}

/// One generation, as a WISH. Nothing about the wiring is typed out here: the
/// node and all fourteen edges come out of `grow_level`, which is the whole
/// point of the file.
fn grown_generation(name: &str) -> Value {
    let decl = grow_level(json!({
        "scope": format!("/members/{MEMBER}"), "level": "assistant", "name": name,
        "template": "assistant@2.4.1",
        // The three brains of a generation are the doubles named in the header,
        // and a `ctx` key is still required: the model is a RESOLVED literal in
        // the template's `requires`, and the mutation refuses a generation whose
        // brain has no name for what it infers with, double or not.
        "ctx": {"model": "double/no-network", "model_fast": "double/no-network",
                "model_surface": "double/no-network"}}));
    json!({"manifest": [decl]})
}

// ══════════════════════════════════════════════════════════════════ waiting

async fn dead_letters(h: &ColonyHandle) -> Vec<(String, String, String)> {
    h.drain_dead_letters()
        .await
        .iter()
        .map(|d| {
            (
                d.sender_path.as_str().to_string(),
                d.resolved_target.as_str().to_string(),
                d.reason.as_code().to_string(),
            )
        })
        .collect()
}

async fn wait_for(p: &std::path::Path, what: &str, h: &ColonyHandle) {
    let deadline = std::time::Instant::now() + RECV_TIMEOUT;
    while std::time::Instant::now() < deadline && !p.exists() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        p.exists(),
        "{what} never arrived at {} -- dead letters: {:?}",
        p.display(),
        dead_letters(h).await
    );
}

/// Poll the member-level completeness marker until it names `want` holders. The
/// sink rewrites it once per finishing walk, so reading it at the first sight of
/// the file measures whichever walk happened to be first.
async fn wait_marker(p: &std::path::Path, want: usize, h: &ColonyHandle) -> Value {
    let deadline = std::time::Instant::now() + RECV_TIMEOUT;
    loop {
        let named = std::fs::read_to_string(p)
            .ok()
            .and_then(|raw| from_str::<Value>(&raw).ok());
        if let Some(v) = &named
            && v["hives"].as_array().map(Vec::len).unwrap_or_default() >= want
        {
            return v.clone();
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "the member-level marker never named {want} holders (last: {named:?}) -- \
                 dead letters: {:?}",
                dead_letters(h).await
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Poll one store's own `cell.db` until it holds what the run is waiting for.
async fn wait_rows(
    db: &std::path::Path,
    sql: &str,
    want: usize,
    what: &str,
    h: &ColonyHandle,
) -> Vec<Vec<String>> {
    let deadline = std::time::Instant::now() + RECV_TIMEOUT;
    loop {
        let got = rows(db, sql);
        if got.len() >= want {
            return got;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "{what}: {} holds {} row(s), expected {want} -- dead letters: {:?}",
                db.display(),
                got.len(),
                dead_letters(h).await
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn send_at(
    target: &str,
    route: &str,
    hop_extra: &[(&str, Value)],
    ctx: &[(&str, Value)],
) -> Message {
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!(route));
    for (k, v) in hop_extra {
        hop.insert((*k).to_string(), v.clone());
    }
    let mut context = Map::new();
    for (k, v) in ctx {
        context.insert((*k).to_string(), v.clone());
    }
    MessageBuilder::new(Path::new(target))
        .hop(hop)
        .context(context)
        .body(Body::Inline(json!({"messages": []})))
        .build()
}

// ══════════════════════════════════════════════════════════ 1. the renderer

#[test]
fn the_rendered_level_carries_every_transfer_lane_the_generation_declares() {
    if !shipped() {
        return;
    }
    // The lanes are read off the shipped contract, never written down here: a
    // template that grows a fourth transfer lane must make this red rather than
    // leave the recipe quietly behind.
    let contract = shipped_config("templates/assistant/config.json")["params"]["contract"].clone();
    let doors: Vec<String> = contract["accepts"]
        .as_array()
        .expect("accepts")
        .iter()
        .filter_map(|a| a["route"].as_str())
        .filter(|r| *r == "in_export" || *r == "in_import")
        .map(str::to_string)
        .collect();
    assert_eq!(
        doors,
        vec!["in_export".to_string(), "in_import".to_string()],
        "GH #475 put both transfer doors on the generation; if one of them is \
         gone the recipe is not the thing to fix first"
    );
    assert!(
        contract["emits"]
            .as_array()
            .expect("emits")
            .iter()
            .any(|e| e["route"] == "dump"),
        "a generation that accepts an export and emits no `dump` has nowhere to \
         put the document it walked"
    );

    let got = rendered_transfer_edges(SCRIBE);
    // The container is `.` and the generation is named bare since GH #503: a
    // level declares itself AT the container it grows into, so both endpoints
    // are relative to that scope. The absolute edges are unchanged.
    let want = json!([
        {"from": ".", "to": format!("./{SCRIBE}"),
         "condition": format!("has(hop.route) && hop.route == 'in_export' && has(context.assistant) && context.assistant == '{SCRIBE}'")},
        {"from": ".", "to": format!("./{SCRIBE}"),
         "condition": format!("has(hop.route) && hop.route == 'in_import' && has(context.assistant) && context.assistant == '{SCRIBE}'")},
        {"from": format!("./{SCRIBE}"), "to": ".",
         "condition": "has(hop.route) && hop.route == 'dump'"},
    ]);
    assert_eq!(
        Value::Array(got.clone()),
        want,
        "the recipe does not draw the transfer lanes the shipped generation \
         declares. Both doors carry the SAME `context.assistant` guard a turn \
         does -- a member with two generations holds two session ledgers and \
         they are not one document -- and the drain is plain"
    );
    assert!(
        !got[2]["condition"]
            .as_str()
            .unwrap_or_default()
            .contains("dump_kind"),
        "the `dump` drain has to be PLAIN. Every level between this container \
         and the keeper pairs `in_export` with `dump` in `params.required_drains`, \
         and the probe that checks the pairing runs the described hop through the \
         real edge evaluator -- an edge that also tested `hop.dump_kind` \
         evaluates false under it, reads as no drain at all, and the mutation is \
         refused"
    );
}

// ═══════════════════════════════════════════════════════════════ 2. the run

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_generation_grown_from_a_wish_receives_the_export_that_names_it() {
    if !shipped() {
        return;
    }

    let td = tempfile::TempDir::new().unwrap();
    let flags = td.path().join("flags");
    let export_dir = td.path().join("exports");
    std::fs::create_dir_all(&export_dir).unwrap();
    let h = boot(&td, &flags).await;

    let outcome = apply(&h, member_manifest(&export_dir)).await;
    assert!(
        outcome.is_committed(),
        "growing the shipped member must commit; got {outcome:?}"
    );
    for name in [SCRIBE, COACH] {
        let outcome = apply(&h, grown_generation(name)).await;
        assert!(
            outcome.is_committed(),
            "the manifest `grow_level` renders for a generation must pass the \
             real mutation door -- `in_export` and `in_import` are paired with \
             `dump` in the generation's own `required_drains`, and a door edge \
             without its drain is refused rather than delivered; got {outcome:?}"
        );
    }

    let base = format!("/members/{MEMBER}");
    let keeper_db = |agent: &str| {
        td.path().join(format!(
            "main/members/{MEMBER}/assistants/{agent}/talky/session-keeper/sessions/cell.db"
        ))
    };

    // ── a real session, opened by a real turn ───────────────────────────────
    // The turn is addressed at the generation's own path: the keeper's stamp
    // sits BEFORE the brain in talky's graph, so a session exists because a turn
    // ARRIVED, not because one was answered. The double behind `./brain` ends
    // the round there.
    h.send(
        MessageBuilder::new(Path::new(&format!("{base}/assistants/{SCRIBE}")))
            .hop({
                let mut hop = Map::new();
                hop.insert("route".to_string(), json!("in_turn"));
                hop
            })
            .context({
                let mut ctx = Map::new();
                ctx.insert("channel".to_string(), json!(CHANNEL));
                ctx.insert("audience_set".to_string(), json!(AUDIENCE));
                ctx.insert("assistant".to_string(), json!(SCRIBE));
                ctx
            })
            .body(Body::Inline(json!({"messages": [
                {"origin": "user", "type": "text", "text": "open a generation for me"}]})))
            .build(),
    )
    .await;
    let opened = wait_rows(
        &keeper_db(SCRIBE),
        "SELECT session_id, channel FROM sessions",
        1,
        "the keeper of the grown generation never opened a session -- the stamp \
         sits before the brain, so a turn that reaches the generation at all \
         reaches it",
        &h,
    )
    .await;
    let session = opened[0][0].clone();
    assert_eq!(opened[0][1], CHANNEL);

    // ── the export that names it reaches FOUR holders ───────────────────────
    h.send(send_at(
        &base,
        "in_export",
        &[],
        &[("assistant", json!(SCRIBE))],
    ))
    .await;
    for hive in ["memory-hive", "affinity", "firewall", "session-keeper"] {
        wait_for(
            &export_dir.join(hive).join("seed/export_final.json"),
            &format!(
                "{hive}'s completeness marker -- and before GH #476 the demand \
                 stopped as `no_route` at the generations container for the \
                 fourth of them, because the level the recipe rendered had no \
                 door for it"
            ),
            &h,
        )
        .await;
    }
    let ledger = std::fs::read_to_string(export_dir.join("session-keeper/seed/sessions.jsonl"))
        .expect("the keeper's ledger is on disk beside the other three documents");
    let lines: Vec<&str> = ledger.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        lines.len(),
        2,
        "a seed file is one schema header plus one row per session: {ledger}"
    );
    let header: Value = from_str(lines[0]).unwrap();
    assert_eq!(
        header["schema"],
        shipped_config("templates/session-keeper/sessions/config.json")["params"]["schema"]["sessions"],
        "line 1 of a seed file is the store's own declaration, verbatim -- that \
         is what makes the part a birth format rather than a row dump"
    );
    let row: Value = from_str(lines[1]).unwrap();
    assert_eq!(
        row["session_id"], session,
        "the ledger that travelled is not the one the turn opened"
    );
    assert_eq!(row["channel"], CHANNEL);

    // The member-level marker is rewritten once per finishing walk, so it is
    // read when it has stopped growing rather than at the first sight of it —
    // four holders finish in whatever order their walks do.
    let marker = wait_marker(&export_dir.join("export_final.json"), 4, &h).await;
    assert_eq!(
        marker["hives"],
        json!(["affinity", "firewall", "memory-hive", "session-keeper"]),
        "the member-level marker names every holder whose walk finished. Three \
         of them is what a grown generation produced before GH #476, and three \
         is exactly what a complete export looks like -- which is why this file \
         measures the fourth rather than the absence of a complaint"
    );

    // ── and the generation nobody named stayed out of it ────────────────────
    assert!(
        rows(&keeper_db(COACH), "SELECT session_id FROM sessions").is_empty(),
        "the export named one generation and walked the other one too -- the \
         guard the recipe renders makes the name an address, not a fan-out"
    );
    assert!(
        !flags.join("reject.json").exists(),
        "a walk refused something: {:?}",
        std::fs::read_to_string(flags.join("reject.json"))
    );
    let dl = dead_letters(&h).await;
    assert!(
        dl.is_empty(),
        "the transfer lane of a GROWN generation dead-lettered; got {dl:?}"
    );
    h.shutdown().await;
}
