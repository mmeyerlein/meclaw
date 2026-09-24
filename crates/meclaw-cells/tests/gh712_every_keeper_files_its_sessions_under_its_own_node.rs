//! GH #712 — every session keeper of a generation files its sessions under its own
//! node, and an import puts each ledger back into the keeper at the same node.
//!
//! Since `assistant@2.7.0` a generation holds two talkys, `./talky` and
//! `./talky-chat`, and each carries a session keeper. Until `session-keeper@2.2.2`
//! the porter filed its document under the constant hive name — `<run>/session-keeper`
//! — and the member imported on `hop.import_hive == 'session-keeper'`, so two keepers
//! would have claimed one directory on the way out and been indistinguishable on the
//! way in. The level therefore drew the four transfer lanes at `./talky` alone, and
//! the typed channel's sessions never travelled: the e26 → e27 transfer carried one
//! keeper directory with 29 rows and nothing of the chat keeper.
//!
//! **What is measured here, at the receiving end.** One colony, the shipped library,
//! the shipped member grown through the mutation door and three generations grown
//! from what `grow_level` renders. A typed turn opens a session in `scribe`'s chat
//! keeper and a spoken one in its other keeper — read back out of each keeper's own
//! `cell.db`. An export naming `scribe` then produces TWO keeper directories,
//! `<run>/talky/session-keeper` and `<run>/talky-chat/session-keeper`, each with its
//! own marker and its own row, and two `export_done` naming those two paths. The
//! shipped example turns the export into import messages for `coach`; each lands as
//! a `dump` with `rows_written == 1` at the member's rim, and each of `coach`'s keepers
//! holds exactly the row of the keeper at the same node — the channel is the proof.
//! Last, an export in the flat pre-2.2.2 form (`<run>/session-keeper`, which is what
//! every export written before this repair looks like) is read as the default talky's
//! ledger and lands in `tutor`'s `./talky` keeper.
//!
//! **Substitutions, named rather than hidden** — the two of
//! `gh476_a_grown_generation_receives_the_transfer_lane.rs`, for the same reasons: the
//! `llm` type is an inert lazy double (the keeper's stamp sits BEFORE the brain, so a
//! session is opened by a turn arriving), and the fence every store writes inside is
//! this run's tempdir.
//!
//! Guarded like every template-reading test (GH #49).

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
/// The generation the two sessions are opened in and the export is addressed to.
const SCRIBE: &str = "scribe";
/// The generation the export is imported into.
const COACH: &str = "coach";
/// The generation the pre-2.2.2 (flat) export is imported into.
const TUTOR: &str = "tutor";
/// The typed channel: its turn carries `context.channel_node == 'chat'` and is
/// served by `./talky-chat`.
const CHAT: &str = "tg:712a";
/// The spoken channel: no `channel_node` of its own, served by `./talky`.
const SPOKEN: &str = "tg:712b";
/// The run directory the caller names on `hop.export_to`.
const RUN: &str = "run712";
/// The round the turns are spoken in, as the JSON list the memory hive parses.
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
    // Only the provider lane is left to fill: since GH #138 the two crons this
    // helper used to append (`AFFINITY_PUSH_CRON`, `KEEPER_NIGHT_CRON`) are
    // params of the cells that read them, and are pushed out of the run's way
    // with an `override_params` entry instead of with a line here.
    names
        .into_iter()
        .map(|n| format!("{n}=dummy-{n}\n"))
        .collect()
}

/// The nightly consolidation, pushed to a date this run cannot reach.
///
/// It was a `MEMORY_DREAM_CRON=` line in the `.env` above until GH #138. The
/// hive's schedule is a LITERAL of `memory-hive/clock`'s own params now, so
/// such a line would be read by nothing at all: the night would fire into this
/// run and nobody would say so. `override_params` replaces the whole
/// `schedules` key -- the key that EXISTS under a timer's params, which is the
/// only kind GH #294 accepts -- and the timer plans on what it finds there
/// (`crates/meclaw-cells/tests/gh138_memory_hive_params.rs` is the proof).
fn quiet_night() -> Value {
    json!({"schedules": [{
        "schedule_id": "0190a3f2-0000-7000-8000-00000000dead",
        "schedule_name": "nightly-dream",
        "cron": "0 0 4 1 1 *",
        "emit_to": "../dream-glue",
        "emit_body": {"messages": [{"origin": "user", "type": "text", "text": "nightly-dream"}]},
        "emit_headers": {}
    }]})
}

/// The record hive's push tick, pushed to a date this run cannot reach.
///
/// It was an `AFFINITY_PUSH_CRON=` line in the `.env` above until GH #138. The
/// hive's cadence is a LITERAL of `affinity/clock`'s own params now, so such a
/// line would be read by nothing at all: the lane would tick into this run
/// every five minutes and nobody would say so. `override_params` replaces the
/// whole `schedules` key -- the key that EXISTS under a timer's params, which
/// is the only kind GH #294 accepts -- and the timer plans on what it finds
/// there (`crates/meclaw-cells/tests/gh138_affinity_firewall_params.rs` is the
/// proof).
fn quiet_push() -> Value {
    json!({"schedules": [{
        "schedule_id": "0190a3f2-0000-7000-8000-00000000beef",
        "schedule_name": "affinity-push",
        "cron": "0 0 4 1 1 *",
        "emit_to": "../push",
        "emit_body": {"messages": [{"origin": "user", "type": "text", "text": "affinity-push"}]},
        "emit_headers": {}
    }]})
}

/// A code cell that appends every message it is handed to one file per lane, so
/// a wait can be a wait for something that HAD to arrive.
///
/// **One line per message, appended — never a rewritten document.** A `code`
/// cell is a stateless dispatcher whose default `max_concurrency` is 4
/// (`CodeParams::effective_max_concurrency`), so two arrivals close enough
/// together run as two overlapping subprocesses. The earlier body read the
/// whole lane file, appended to what it had read and wrote it back — the last
/// writer then erased every receipt that had arrived while it was working, and
/// a wait on that lane spent its whole window on a lane that had in fact
/// delivered (GH #587, measured there; GH #588, the same form here).
///
/// A single `O_APPEND` write places each line whole and at the end instead, so
/// no execution can observe — let alone overwrite — another one's, and the
/// return value is checked so a short write falls loudly rather than leaving
/// half a line lying there. A reader that meets a line mid-flight fails to
/// parse it and retries, which is why an arrival can never be miscounted.
fn flag_cell(dir: &str) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "flag_dir": dir, "sandbox": {"trust": "trusted"},
                   "script_inline": r#"
import sys, json, os
doc = json.load(sys.stdin)
hop = (doc["envelope"].get("header") or {}).get("hop") or {}
path = os.path.join(doc["params"]["flag_dir"], str(hop.get("route") or "unknown") + ".jsonl")
blob = (json.dumps({"hop": hop}) + "\n").encode("utf-8")
fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o644)
try:
    written = os.write(fd, blob)
finally:
    os.close(fd)
if written != len(blob):
    sys.exit("short write: %d of %d bytes" % (written, len(blob)))
sys.stdout.write(json.dumps([]))
"#},
        "contract": {"version": "1.0.0", "settings": {}, "multi_send_capable": true,
                     "emits": {}, "consumes": {}}
    })
}

/// The shell the member is grown into: a members container, and a flag cell that
/// takes every lane the member level raises.
async fn boot(
    td: &tempfile::TempDir,
    flag_dir: &std::path::Path,
    fence: &std::path::Path,
) -> ColonyHandle {
    let root = td.path();
    copy_tree(&repo("templates"), &root.join("templates"));
    // The keeper's nightly close sweep, pushed to a date this run cannot reach.
    // It was a `KEEPER_NIGHT_CRON` line in the `.env` below until GH #138: the
    // schedule is a LITERAL of `session-keeper/night`'s own params now, so such
    // a line is read by nothing at all -- the sweep would fire into this run and
    // nobody would say so. The library copy is this tree's own, so writing the
    // key into it is what an `override_params` entry does to a staged config
    // (`crates/meclaw-cells/tests/gh138_keeper_summarizer_dispatcher_params.rs`
    // is the proof that the timer plans on what it finds there).
    meclaw_testing::quiet_keeper_night(&root.join("templates/session-keeper"));
    // GH #555 — the keeper four levels down writes its OWN ledger, inside the
    // fence its store declares. A generation is grown from the library rather
    // than from a manifest this file writes (that is the whole of GH #476), so
    // the fence is set on the library copy this colony instantiates from: the
    // shipped default is an absolute path outside this tempdir, and a test that
    // wrote there would measure the machine it runs on.
    {
        let p = root.join("templates/session-keeper/sessions/config.json");
        let mut cfg: Value = from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        cfg["params"]["transfer"]["base_path"] = json!(fence.to_str().unwrap());
        write_json(&p, &cfg);
    }
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
        "dump",
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
    // Since GH #555 an instance says ONE thing about files: the fence each
    // holder's own store writes inside. There is no cell to relax and no
    // sandbox to substitute — the writer is the substrate.
    let mut over = json!({"memory-hive/clock": quiet_night(),
                          "affinity/clock": quiet_push()});
    for (hive, cell) in [
        ("memory-hive", "store"),
        ("affinity", "store"),
        ("firewall", "rules"),
    ] {
        over[format!("{hive}/{cell}")] =
            json!({"transfer": {"base_path": export_dir.to_str().unwrap()}});
    }
    json!({"manifest": [{
        // The declaration stands AT the container it grows into (GH #503),
        // which is the form `build_import.py` writes and the form
        // `container_edges()` above is spelled in: `.` is `/members`, the
        // member is named bare, and the path it lands at is unchanged.
        "scope": "/members",
        "diff": {
            "add_nodes": [{"name": MEMBER, "template": "member@1.10.0",
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
        "template": "assistant@2.9.0",
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

/// The recorder's append log for one lane.
fn lane_file(flags: &std::path::Path, lane: &str) -> std::path::PathBuf {
    flags.join(format!("{lane}.jsonl"))
}

/// Every message the recorder has finished placing on that lane. A line that is
/// still being placed does not parse and is simply not there yet.
fn lane_entries(p: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| from_str::<Value>(l).ok())
        .collect()
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

// ══════════════════════════════════════════════════════════════════ the run

/// The keeper `cell.db` of one talky of one generation.
fn keeper_db(td: &tempfile::TempDir, agent: &str, talky: &str) -> std::path::PathBuf {
    td.path().join(format!(
        "main/members/{MEMBER}/assistants/{agent}/{talky}/session-keeper/sessions/cell.db"
    ))
}

/// The channels a keeper holds sessions for, sorted.
fn channels(td: &tempfile::TempDir, agent: &str, talky: &str) -> Vec<String> {
    let mut v: Vec<String> = rows(&keeper_db(td, agent, talky), "SELECT channel FROM sessions")
        .into_iter()
        .map(|r| r[0].clone())
        .collect();
    v.sort();
    v
}

/// One turn at a generation's own path. `chat` stamps the typed channel's node.
fn turn(agent: &str, channel: &str, chat: bool) -> Message {
    let mut ctx = Map::new();
    ctx.insert("channel".to_string(), json!(channel));
    ctx.insert("audience_set".to_string(), json!(AUDIENCE));
    ctx.insert("assistant".to_string(), json!(agent));
    if chat {
        ctx.insert("channel_node".to_string(), json!("chat"));
    }
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!("in_turn"));
    MessageBuilder::new(Path::new(&format!("/members/{MEMBER}/assistants/{agent}")))
        .hop(hop)
        .context(ctx)
        .body(Body::Inline(json!({"messages": [
            {"origin": "user", "type": "text", "text": "open a generation for me"}]})))
        .build()
}

/// Wait until the recorder holds at least `want` entries on one lane that match.
async fn wait_lane(
    flags: &std::path::Path,
    lane: &str,
    want: usize,
    keep: impl Fn(&Value) -> bool,
    h: &ColonyHandle,
) -> Vec<Value> {
    let deadline = std::time::Instant::now() + RECV_TIMEOUT;
    loop {
        let got: Vec<Value> = lane_entries(&lane_file(flags, lane))
            .into_iter()
            .filter(|e| keep(e))
            .collect();
        if got.len() >= want {
            return got;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "`{lane}`: {} matching entr(y/ies), expected {want} -- dead letters: {:?}",
                got.len(),
                dead_letters(h).await
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn is_keeper(e: &Value, key: &str) -> bool {
    e["hop"][key]
        .as_str()
        .is_some_and(|s| s.ends_with("/session-keeper"))
}

/// Run the shipped example over one export directory for one generation, and
/// return the after-boot messages plus its stderr.
fn build_import(
    td: &tempfile::TempDir,
    export: &std::path::Path,
    agent: &str,
) -> (Vec<Value>, String) {
    let after_boot = td.path().join(format!("after-boot-{agent}.json"));
    let out = std::process::Command::new("python3")
        .arg(repo("examples/memory-import/build_import.py"))
        .arg("--export")
        .arg(export)
        .arg("--templates")
        .arg(td.path().join("templates"))
        .arg("--scope")
        .arg("/")
        .arg("--name")
        .arg(MEMBER)
        .arg("--after-boot")
        .arg(&after_boot)
        .arg("--assistant")
        .arg(agent)
        .output()
        .expect("python3");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "build_import.py failed: {stderr}");
    let msgs: Vec<Value> =
        from_str(&std::fs::read_to_string(&after_boot).unwrap()).expect("after-boot messages");
    (msgs, stderr)
}

async fn post(h: &ColonyHandle, msgs: &[Value]) {
    for m in msgs {
        let hop: Map<String, Value> = m["header"]["hop"].as_object().expect("hop").clone();
        let context: Map<String, Value> =
            m["header"]["context"].as_object().expect("context").clone();
        h.send(
            MessageBuilder::new(Path::new(m["target"].as_str().unwrap()))
                .hop(hop)
                .context(context)
                .body(Body::Inline(m["body"].clone()))
                .build(),
        )
        .await;
    }
}

fn listing(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_keeper_files_its_sessions_under_its_own_node() {
    if !shipped() {
        return;
    }

    let td = tempfile::TempDir::new().unwrap();
    let flags = td.path().join("flags");
    let export_dir = td.path().join("exports");
    std::fs::create_dir_all(&export_dir).unwrap();
    let h = boot(&td, &flags, &export_dir).await;

    let outcome = apply(&h, member_manifest(&export_dir)).await;
    assert!(
        outcome.is_committed(),
        "growing the shipped member must commit; got {outcome:?}"
    );
    for name in [SCRIBE, COACH, TUTOR] {
        let outcome = apply(&h, grown_generation(name)).await;
        assert!(
            outcome.is_committed(),
            "the manifest `grow_level` renders for {name} must pass the real mutation \
             door -- with an `in_import` door at each talky the level still has a door \
             for a bare route, which is what the door probe sends; got {outcome:?}"
        );
    }

    // ── two sessions, one per keeper ────────────────────────────────────────
    h.send(turn(SCRIBE, CHAT, true)).await;
    h.send(turn(SCRIBE, SPOKEN, false)).await;
    wait_rows(
        &keeper_db(&td, SCRIBE, "talky-chat"),
        "SELECT session_id FROM sessions",
        1,
        "the typed turn never opened a session in the chat keeper",
        &h,
    )
    .await;
    wait_rows(
        &keeper_db(&td, SCRIBE, "talky"),
        "SELECT session_id FROM sessions",
        1,
        "the spoken turn never opened a session in the default keeper",
        &h,
    )
    .await;
    assert_eq!(channels(&td, SCRIBE, "talky-chat"), vec![CHAT.to_string()]);
    assert_eq!(channels(&td, SCRIBE, "talky"), vec![SPOKEN.to_string()]);

    // ── the export that names scribe: two keepers, two directories ─────────
    h.send(send_at(
        &format!("/members/{MEMBER}"),
        "in_export",
        &[("export_to", json!(RUN))],
        &[("assistant", json!(SCRIBE))],
    ))
    .await;
    let run = export_dir.join(RUN);
    for talky in ["talky", "talky-chat"] {
        wait_for(
            &run.join(talky)
                .join("session-keeper/seed/export_final.json"),
            &format!(
                "the completeness marker of {talky}'s keeper -- before GH #712 both keepers \
                 would have written `{RUN}/session-keeper`, and the chat keeper was never \
                 even asked"
            ),
            &h,
        )
        .await;
    }
    for hive in ["memory-hive", "affinity", "firewall"] {
        wait_for(
            &run.join(hive).join("seed/export_final.json"),
            &format!("{hive}'s completeness marker"),
            &h,
        )
        .await;
    }
    assert_eq!(
        listing(&run),
        vec!["affinity", "firewall", "memory-hive", "talky", "talky-chat"],
        "one directory per holder, and the keepers are filed under their talky"
    );
    for (talky, channel) in [("talky", SPOKEN), ("talky-chat", CHAT)] {
        let ledger =
            std::fs::read_to_string(run.join(talky).join("session-keeper/seed/sessions.jsonl"))
                .unwrap_or_else(|e| panic!("{talky}'s ledger: {e}"));
        let lines: Vec<&str> = ledger.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(
            lines.len(),
            2,
            "{talky}: one schema header plus the ONE session its keeper opened -- two rows \
             would be the other keeper's walk in the same directory: {ledger}"
        );
        let header: Value = from_str(lines[0]).unwrap();
        assert_eq!(
            header["schema"],
            shipped_config("templates/session-keeper/sessions/config.json")["params"]["schema"]["sessions"],
            "{talky}: line 1 of a seed file is the store's own declaration, verbatim"
        );
        let row: Value = from_str(lines[1]).unwrap();
        assert_eq!(
            row["channel"], channel,
            "{talky} exported the other keeper's session"
        );
    }
    let done = wait_lane(
        &flags,
        "export_done",
        2,
        |e| is_keeper(e, "export_hive"),
        &h,
    )
    .await;
    let mut said: Vec<(String, String)> = done
        .iter()
        .map(|e| {
            (
                e["hop"]["export_hive"].as_str().unwrap_or("").to_string(),
                e["hop"]["seed_dir"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    said.sort();
    assert_eq!(
        said,
        vec![
            (
                "talky-chat/session-keeper".to_string(),
                format!("{RUN}/talky-chat/session-keeper/seed")
            ),
            (
                "talky/session-keeper".to_string(),
                format!("{RUN}/talky/session-keeper/seed")
            ),
        ],
        "each keeper says `export_done` for itself, naming its own path -- one per \
         keeper of the generation, and distinct"
    );

    // ── the way back in: coach receives both ledgers ────────────────────────
    let (msgs, _) = build_import(&td, &run, COACH);
    let mut addressed: Vec<String> = msgs
        .iter()
        .map(|m| {
            m["header"]["hop"]["import_hive"]
                .as_str()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    addressed.sort();
    assert_eq!(
        addressed,
        vec!["talky-chat/session-keeper", "talky/session-keeper"],
        "one part per keeper directory, each addressed with its path: {msgs:?}"
    );
    for m in &msgs {
        assert_eq!(m["header"]["context"]["assistant"], COACH);
        assert_eq!(m["target"], format!("/members/{MEMBER}"));
    }
    post(&h, &msgs).await;
    let dumps = wait_lane(&flags, "dump", 2, |e| is_keeper(e, "port_hive"), &h).await;
    let mut landed: Vec<(String, Value)> = dumps
        .iter()
        .map(|e| {
            (
                e["hop"]["port_hive"].as_str().unwrap_or("").to_string(),
                e["hop"]["rows_written"].clone(),
            )
        })
        .collect();
    landed.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        landed,
        vec![
            ("talky-chat/session-keeper".to_string(), json!(1)),
            ("talky/session-keeper".to_string(), json!(1)),
        ],
        "each part reached ONE keeper and wrote its one row; the receipt on `dump` is \
         the only positive signal an import has"
    );
    let arrived_chat = wait_rows(
        &keeper_db(&td, COACH, "talky-chat"),
        "SELECT channel FROM sessions",
        1,
        "the chat ledger never reached coach's chat keeper",
        &h,
    )
    .await;
    let arrived_spoken = wait_rows(
        &keeper_db(&td, COACH, "talky"),
        "SELECT channel FROM sessions",
        1,
        "the default ledger never reached coach's default keeper",
        &h,
    )
    .await;
    assert_eq!(
        arrived_chat,
        vec![vec![CHAT.to_string()]],
        "coach's chat keeper holds a session that is not the chat keeper's"
    );
    assert_eq!(
        arrived_spoken,
        vec![vec![SPOKEN.to_string()]],
        "coach's default keeper holds a session that is not the default keeper's"
    );
    assert_eq!(channels(&td, SCRIBE, "talky-chat"), vec![CHAT.to_string()]);
    assert_eq!(
        channels(&td, SCRIBE, "talky"),
        vec![SPOKEN.to_string()],
        "the import was addressed to coach and touched scribe"
    );

    // ── an export in the pre-2.2.2 form still reads, as the default keeper ──
    let flat = export_dir.join("run712-flat");
    for hive in ["memory-hive", "affinity", "firewall"] {
        copy_tree(&run.join(hive), &flat.join(hive));
    }
    copy_tree(
        &run.join("talky/session-keeper"),
        &flat.join("session-keeper"),
    );
    let (old, stderr) = build_import(&td, &flat, TUTOR);
    assert_eq!(old.len(), 1, "{old:?}");
    assert_eq!(
        old[0]["header"]["hop"]["import_hive"], "talky/session-keeper",
        "a flat `session-keeper/` directory is the one keeper an export had before \
         2.2.2 -- the default talky's"
    );
    assert!(
        stderr.contains("pre-2.2.2"),
        "the example reads the old form without saying so: {stderr}"
    );
    post(&h, &old).await;
    wait_lane(&flags, "dump", 3, |e| is_keeper(e, "port_hive"), &h).await;
    let arrived = wait_rows(
        &keeper_db(&td, TUTOR, "talky"),
        "SELECT channel FROM sessions",
        1,
        "the flat export never reached tutor's default keeper",
        &h,
    )
    .await;
    assert_eq!(arrived, vec![vec![SPOKEN.to_string()]]);
    assert!(
        channels(&td, TUTOR, "talky-chat").is_empty(),
        "the flat export landed in the chat keeper as well"
    );

    // Closing count (T3 review M-4): the waits above stop at "at least", so a
    // fan-out double arriving after them would pass. By now every walk of the
    // run has ended, and each lane carries exactly what was sent.
    for (lane, key, want) in [("export_done", "export_hive", 2), ("dump", "port_hive", 3)] {
        let got = lane_entries(&lane_file(&flags, lane))
            .into_iter()
            .filter(|e| is_keeper(e, key))
            .count();
        assert_eq!(
            got, want,
            "`{lane}` carries {got} keeper entries, not exactly {want} -- a fan-out doubled"
        );
    }
    assert!(
        !lane_file(&flags, "reject").exists(),
        "a walk refused something: {:?}",
        lane_entries(&lane_file(&flags, "reject"))
    );
    // One dead letter is this file's topology, not the transfer, and it is named
    // rather than tolerated: the typed turn's collector asks the member's memory for
    // its ambient bundle (`memory_tier` "1"), and that `recall` v-lane of the
    // `./talky-chat` rim is drawn by whoever wires a chat channel
    // (`templates/assistant/README.md`, "What the SENDER draws"). This file wires
    // none -- it needs the typed keeper's session, which is stamped BEFORE the
    // collector -- so the ask stops at the talky's rim. Anything else is a failure.
    let chat_rim = format!("/members/{MEMBER}/assistants/{SCRIBE}/talky-chat");
    let chat_assemble = format!("{chat_rim}/collector/assemble");
    let dl: Vec<_> = dead_letters(&h)
        .await
        .into_iter()
        .filter(|(from, to, why)| {
            !(*from == chat_assemble && *to == chat_rim && why.as_str() == "hive_no_route")
        })
        .collect();
    assert!(dl.is_empty(), "the transfer dead-lettered; got {dl:?}");
    h.shutdown().await;
}
