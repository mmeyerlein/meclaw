//! GH #475 — a member's export reaches the session keeper of a NAMED
//! generation, and the ledger of that generation comes back the same way.
//!
//! GH #471 taught the member level to hand out everything a member IS: the
//! memory of what was said to this person, the curated record that decides who
//! may be told what, and the screen every inbound turn passes. Three holders,
//! three walks, three directories on disk. What it could not reach was the one
//! table a rebuild cannot recompute: `sessions`. A session keeper stands four
//! levels down, inside a generation — `<member>/assistants/<agent>/talky/
//! session-keeper` — and the container that holds generations ships EMPTY, so
//! there was no path from the member's door to it and no path from a derived
//! member template to its seed. A member exported and reborn kept every episode
//! it had ever been told and greeted the person it had been talking to for a
//! year as a stranger, because the generation that held the conversation opened
//! a fresh one on the first turn.
//!
//! The lane could not simply be fanned to, and the reason is measurable rather
//! than aesthetic: a member with two generations holds TWO session ledgers, they
//! are not one document, and the export sink files a part under the hive it came
//! out of — so two keepers walking at once would write one directory and the
//! sink would keep whichever walk finished last and say nothing about the other.
//! So the generation is NAMED, in `context.assistant`, the same key a turn is
//! addressed with, and the member's door carries the demand into `./assistants`
//! only when it finds one.
//!
//! What runs here is one colony, booted from the shipped library, with the
//! member grown through the mutation door and TWO generations of
//! `assistant@2.4.1` grown into its container the way
//! `templates/member/README.md` § *Addressing an assistant through a channel*
//! and `templates/assistant/README.md` § *Instantiating* prescribe — including
//! the two transfer edges #475 added to that recipe.
//!
//! Six claims, in the order the run makes them:
//!
//! 1. **A real session is opened.** An `in_turn` addressed at `scribe` crosses
//!    the assistant, crosses `talky`, and the SHIPPED keeper stamps it: the
//!    `sessions` row exists before anything is exported, and its `session_id`
//!    was minted by the keeper rather than by this file.
//! 2. **An export that names no generation reaches three holders and no
//!    fourth.** Three walks finish, the member-level marker names three hives,
//!    `session-keeper/` is not on disk — and there is no dead letter, which is
//!    the entire justification of the guard on the door: an unguarded edge into
//!    an open container with no addressing edge would be `no_route` at the
//!    container's own path on every export any member ever ran.
//! 3. **An export that names `scribe` reaches FOUR.** The keeper's ledger lands
//!    as `session-keeper/seed/sessions.jsonl`, carrying the very row from (1),
//!    and the member-level `export_final.json` names all four hives.
//! 4. **The other generation is not touched.** `coach` holds a keeper too and it
//!    stays empty: the name is an address, not a fan-out.
//! 5. **`build_import.py --after-boot --assistant coach` writes the way back
//!    in.** The one hive an export carries that a BIRTH cannot seed is written
//!    out as `in_import` messages instead of being silently dropped; posted at
//!    the member's own path they reach `coach`'s keeper, and the receipt on the
//!    `dump` lane says `rows_written == 1`.
//! 6. **The transfer is addressed, not broadcast.** After the import `coach` has
//!    exactly one row and `scribe` still has exactly one — the same generation,
//!    delivered once, to the keeper that was named.
//!
//! **Substitutions, named rather than hidden.** Two, and both are the same kind
//! of thing: a cell that would leave this machine.
//!
//! * The `llm` cell type is replaced by an inert lazy double. A generation
//!   carries three of them (`talky/brain`, `cogny/brain`, `cogny/brain_fast`),
//!   and a spawned one opens an HTTP connection to a provider. Nothing in this
//!   file is about what a model answers: the keeper's stamp sits BEFORE the
//!   brain in `talky`'s graph, so a session is opened by a turn arriving, not by
//!   a turn being answered. The brain double swallows what the collector hands
//!   it, and the round ends there.
//! * The shipped `export-sink` runs behind `params.sandbox` with
//!   `trust: "restricted"`, which is fail-closed against the host, so a colony
//!   test that kept it would measure the kernel it runs on. It is replaced
//!   through `override_params`, and the shipped value is asserted first — the
//!   same substitution `gh447`, `gh467` and `gh471` make.
//!
//! `llm` is the ONLY cell type this file substitutes. Every other type a
//! generation and a member carry is the shipped factory — `code`, `store`,
//! `timer`, and the seven of the tool surface (`bash`, `edit`, `file`, `mcp`,
//! `vault`, `web_fetch`, `web_search`) — because a type with no factory is a
//! cell that never enters the registry at all, and a generation missing its
//! tools is not the generation this file claims to have grown. So the whole
//! session path — keeper, stamp, sessions store, porter, collector, splitter,
//! errors, the export sink, the three holders of the member — is the shipped
//! tree, running. The four crons the library carries
//! (`KEEPER_NIGHT_CRON`, `MENU_CRON`, `MEMORY_DREAM_CRON`, `AFFINITY_PUSH_CRON`)
//! are pushed to a date this run cannot reach, because a nightly close firing
//! mid-run would close the very generation claim (1) opens.
//!
//! Guarded like every template-reading test (GH #49): a tree that does not carry
//! the library or the example is skipped, never judged.

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
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

const RECV_TIMEOUT: Duration = Duration::from_secs(60);
const MEMBER: &str = "alex";
/// The generation the session is opened in and the export is addressed to.
const SCRIBE: &str = "scribe";
/// The second generation. It holds a keeper of its own, it is never exported,
/// and it is the one the import is addressed to — so a transfer that went to
/// "the assistant" rather than to the one that was named is a failure with a
/// name rather than a smaller number.
const COACH: &str = "coach";
const CHANNEL: &str = "tg:475";
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

// ═════════════════════════════════════════════════════════ the inert brain

/// A lazy factory that accepts every params block and never runs anything —
/// the same device `gh302_the_stack_grows_from_templates.rs` uses, and for the
/// same reason. It stands in for `llm` here: three of them travel with a
/// generation, and a spawned one talks to a provider over the network.
///
/// `is_lazy() == true` registers the cell as `Dormant`: a mailbox pair and no
/// task until something is delivered. The `WakeFn` and `RespawnFn` are reachable
/// through a delivery and a restart, so they are written correctly rather than
/// left as `unimplemented!()` — a panic on either path would take the whole
/// colony task with it.
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
/// test topology drew — and the nightly close would end the very generation
/// claim (1) opens.
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
/// from the shipped script rather than repeated here.
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

/// One generation, wired the way `templates/assistant/README.md` §
/// *Instantiating* and `templates/member/README.md` § *Addressing an assistant
/// through a channel* prescribe: the addressing pair, the guarded inbound lanes,
/// the eight outward ones that are not `answer` — and the two transfer edges
/// GH #475 added to that recipe.
fn assistant_manifest(name: &str) -> Value {
    let guarded = |routes: &str| {
        json!({"from": "./assistants", "to": format!("./assistants/{name}"),
               "condition": format!(
                   "has(hop.route) && ({routes}) && has(context.assistant) && context.assistant == '{name}'")})
    };
    // GH #562 — the memory road is a v-lane and does not end on the
    // generation's path any more: `recall` leaves the asker, `in_bundle` goes
    // back down to it, and the generation's contract names both askers under
    // `at`. This manifest draws the same four edges the shipped recipe renders,
    // for the plainest reason there is: without them a `recall` raised inside
    // this generation has no exit at all and dead-letters at the surface's own
    // path, which is what a member-level export walk then trips over.
    let v_lane = |asker: &str, down: bool| {
        let deep = format!("./assistants/{name}/{asker}");
        if down {
            let mut e = json!({"from": "./assistants", "to": deep,
                               "lane": "in_bundle",
                               "condition": format!(
                                   "has(hop.route) && hop.route == 'in_bundle' && has(context.assistant) && context.assistant == '{name}'")});
            if asker == "cogny" {
                e["condition"] = json!(format!(
                    "{} && has(hop.recall_caller) && hop.recall_caller == 'cogny'",
                    e["condition"].as_str().expect("just built")
                ));
            } else {
                e["default"] = json!(true);
            }
            e
        } else {
            json!({"from": deep, "to": "./assistants", "lane": "recall",
                   "condition": "has(hop.route) && hop.route == 'recall'",
                   "modifier": {"set_context": {"recall_caller": format!("'{asker}'")}}})
        }
    };
    let mut add_edges = vec![
        guarded("hop.route == 'in_turn'"),
        v_lane("cogny", true),
        v_lane("talky", true),
        v_lane("talky", false),
        v_lane("cogny", false),
        guarded("hop.route == 'in_build_result'"),
        // #475: the transfer lanes are addressed with the same key and the same
        // shape as a turn.
        guarded("hop.route == 'in_export' || hop.route == 'in_import'"),
    ];
    for lane in [
        "answer",
        "write",
        "turn_write",
        "extraction",
        "prune",
        "error",
        "build",
        // ... and the plain `dump` edge back. It has to be plain: every level
        // between here and the keeper pairs `in_export` with `dump` in
        // `params.required_drains`, and the probe that checks the pairing runs
        // the described hop through the real edge evaluator, so an edge that
        // additionally tested `hop.dump_kind` would read as no drain at all.
        "dump",
    ] {
        add_edges.push(
            json!({"from": format!("./assistants/{name}"), "to": "./assistants",
                              "condition": format!("has(hop.route) && hop.route == '{lane}'")}),
        );
    }
    json!({"manifest": [{
        "scope": format!("/members/{MEMBER}"),
        // The three brains of a generation are the doubles named in the header,
        // and a `ctx` key is still required: the model is a RESOLVED literal in
        // the template's `requires`, and the mutation refuses a generation whose
        // brain has no name for what it infers with, double or not.
        "ctx": {"model": "double/no-network", "model_fast": "double/no-network",
                "model_surface": "double/no-network"},
        "diff": {
            "add_nodes": [{"name": format!("assistants/{name}"), "template": "assistant@2.4.1"}],
            "add_edges": add_edges,
        }
    }]})
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

/// Poll the MEMBER-level marker until it names `want` holders.
///
/// A hive's own `seed/export_final.json` is not the signal that the member-level
/// one is on disk: the sink writes the hive marker FIRST and rebuilds the
/// member-level marker after it, in the same run. Waiting for the hive markers
/// and then reading the member-level file reads it in the gap — as the empty
/// file the rebuild has just truncated, or as the shorter list of the rebuild
/// before it. The completeness of the LEVEL is what this file asserts, so the
/// level's own document is what it waits for. `gh476` waits the same way.
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

// ═════════════════════════════════════════════════════════════════ the run

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_export_that_names_a_generation_reaches_that_generations_session_keeper() {
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
        let outcome = apply(&h, assistant_manifest(name)).await;
        assert!(
            outcome.is_committed(),
            "growing {name} must commit; got {outcome:?}"
        );
    }

    let base = format!("/members/{MEMBER}");
    let keeper_db = |agent: &str| {
        td.path().join(format!(
            "main/members/{MEMBER}/assistants/{agent}/talky/session-keeper/sessions/cell.db"
        ))
    };

    // ── 1. a real session, opened by a real turn ────────────────────────────
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
        "the keeper of the generation the turn was addressed to never opened a \
         session -- the stamp sits before the brain, so a turn that reaches the \
         generation at all reaches it",
        &h,
    )
    .await;
    let session = opened[0][0].clone();
    assert_eq!(opened[0][1], CHANNEL);
    assert!(
        session.starts_with(CHANNEL),
        "the keeper mints `<channel>-<opened_at>`; this file never names a \
         session and reads the one the keeper wrote: {session}"
    );
    assert!(
        rows(&keeper_db(COACH), "SELECT session_id FROM sessions").is_empty(),
        "the turn was addressed to one generation and opened a session in the \
         other one as well"
    );

    // ── 2. an export that names nobody reaches three holders, and no fourth ──
    h.send(send_at(&base, "in_export", &[], &[])).await;
    for hive in ["memory-hive", "affinity", "firewall"] {
        wait_for(
            &export_dir.join(hive).join("seed/export_final.json"),
            &format!("{hive}'s completeness marker"),
            &h,
        )
        .await;
    }
    let marker = wait_marker(&export_dir.join("export_final.json"), 3, &h).await;
    assert_eq!(
        marker["hives"],
        json!(["affinity", "firewall", "memory-hive"]),
        "an export that names no generation is the export member@1.4.0 always \
         did: three holders, and the guard on the fourth edge is what keeps it \
         that way"
    );
    assert!(
        !export_dir.join("session-keeper").exists(),
        "a keeper walked without being named. Two generations of one person hold \
         two session ledgers under one hive name, and a sink that filed both \
         would keep whichever walk finished last and say nothing about the other"
    );
    let dl = dead_letters(&h).await;
    assert!(
        dl.is_empty(),
        "the export that names no generation dead-lettered. That is the whole \
         reason the door edge is guarded on `context.assistant`: an unguarded \
         edge into an open container with no addressing edge is `no_route` at \
         the container's own path on every export any member ever runs; got {dl:?}"
    );

    // ── 3. an export that names `scribe` reaches FOUR ───────────────────────
    h.send(send_at(
        &base,
        "in_export",
        &[],
        &[("assistant", json!(SCRIBE))],
    ))
    .await;
    wait_for(
        &export_dir.join("session-keeper/seed/export_final.json"),
        "the session keeper's completeness marker",
        &h,
    )
    .await;
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

    let marker = wait_marker(&export_dir.join("export_final.json"), 4, &h).await;
    assert_eq!(
        marker["hives"],
        json!(["affinity", "firewall", "memory-hive", "session-keeper"]),
        "the member-level marker names every holder whose walk finished. Before \
         GH #475 the fourth could not be among them at any price: the keeper \
         stands four levels down inside a generation, and no lane reached it"
    );
    assert!(
        !flags.join("reject.json").exists(),
        "a walk refused something: {:?}",
        std::fs::read_to_string(flags.join("reject.json"))
    );

    // ── 4. the other generation was not touched ─────────────────────────────
    assert!(
        rows(&keeper_db(COACH), "SELECT session_id FROM sessions").is_empty(),
        "the export named one generation and walked the other one too -- the \
         name is an address, not a fan-out"
    );

    // ── 5. the way back in, written by the shipped example ──────────────────
    // A probe on `coach`'s own dump lane, beside the member's edge into the
    // export sink: the sink IGNORES an import receipt on purpose, so the only
    // positive signal an import has needs a reader of its own.
    let (tx, mut rx) = mpsc::channel::<Message>(64);
    let probe = format!("{base}/assistants/probe475");
    h.spawn(Path::new(&probe), move || CaptureCell::new(tx.clone()))
        .await;
    h.add_edge(
        Uuid::now_v7(),
        Path::new(&format!("{base}/assistants/{COACH}")),
        Path::new(&probe),
    )
    .await;

    let after_boot = td.path().join("after-boot.json");
    let out = std::process::Command::new("python3")
        .arg(repo("examples/memory-import/build_import.py"))
        .arg("--export")
        .arg(&export_dir)
        .arg("--templates")
        .arg(td.path().join("templates"))
        .arg("--scope")
        .arg("/")
        .arg("--name")
        .arg(MEMBER)
        .arg("--after-boot")
        .arg(&after_boot)
        .arg("--assistant")
        .arg(COACH)
        .output()
        .expect("python3");
    assert!(
        out.status.success(),
        "build_import.py --after-boot failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let msgs: Vec<Value> =
        from_str(&std::fs::read_to_string(&after_boot).unwrap()).expect("after-boot messages");
    assert_eq!(
        msgs.len(),
        1,
        "the keeper holds ONE content table, so its document is one part -- and \
         `session-keeper` is the one hive in the catalogue a BIRTH cannot seed, \
         so it is the one this file writes out: {msgs:?}"
    );
    assert_eq!(msgs[0]["target"], format!("/members/{MEMBER}"));
    assert_eq!(msgs[0]["header"]["hop"]["import_hive"], "session-keeper");
    assert_eq!(msgs[0]["header"]["context"]["assistant"], COACH);

    for m in &msgs {
        let hop: Map<String, Value> = m["header"]["hop"]
            .as_object()
            .expect("a hop object")
            .clone();
        let context: Map<String, Value> = m["header"]["context"]
            .as_object()
            .expect("a context object")
            .clone();
        h.send(
            MessageBuilder::new(Path::new(m["target"].as_str().unwrap()))
                .hop(hop)
                .context(context)
                .body(Body::Inline(m["body"].clone()))
                .build(),
        )
        .await;
    }

    let deadline = std::time::Instant::now() + RECV_TIMEOUT;
    let mut receipt = None;
    while receipt.is_none() && std::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Some(m)) => {
                if m.headers.hop.get("dump_kind").and_then(|v| v.as_str()) == Some("import_receipt")
                {
                    receipt = Some(m);
                }
            }
            Ok(None) => break,
            Err(_) => {}
        }
    }
    let receipt = receipt.unwrap_or_else(|| {
        panic!("no import receipt left the generation the part was addressed to")
    });
    assert_eq!(
        receipt.headers.hop.get("rows_written"),
        Some(&json!(1)),
        "the part reached a keeper and wrote nothing. The receipt on the `dump` \
         lane is the only positive signal an import has -- the member's sink \
         ignores it on purpose, so undrained it would be the whole evidence of \
         a transfer, dropped: {:?}",
        receipt.headers.hop
    );

    // ── 6. addressed, not broadcast ─────────────────────────────────────────
    let arrived = wait_rows(
        &keeper_db(COACH),
        "SELECT session_id, channel FROM sessions",
        1,
        "the transferred generation never reached the keeper it was addressed to",
        &h,
    )
    .await;
    assert_eq!(
        arrived,
        vec![vec![session.clone(), CHANNEL.to_string()]],
        "the second generation is now carrying the conversation the first one \
         held -- one row, the same generation, and no fork"
    );
    assert_eq!(
        rows(&keeper_db(SCRIBE), "SELECT session_id FROM sessions"),
        vec![vec![session.clone()]],
        "the import was addressed to one generation and landed in both. A member \
         with two generations has two session ledgers, and they are not one \
         document"
    );

    let dl = dead_letters(&h).await;
    assert!(
        dl.is_empty(),
        "the transfer that GH #475 opened dead-lettered; got {dl:?}"
    );
    h.shutdown().await;
}
