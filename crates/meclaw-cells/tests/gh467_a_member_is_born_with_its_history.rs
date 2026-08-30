//! GH #467 — a member is born with its history, through the one door.
//!
//! A member's memory leaves as a seed set (`in_export` →
//! `memory-hive/seed/<table>.jsonl`,
//! GH #447). Until now it had no declared way back: there is no `seed` field on
//! a diff and none on a `ref` marker, the only manifest key that carries files
//! is `add_templates[].files`, and `member/memory-hive` is a `ref` — nothing can
//! put a seed into a reference. So the way in was a file copy by hand, or
//! harvesting parts out of the dead-letter queue.
//!
//! `examples/memory-import/` is the recipe, and this file drives it end to end:
//!
//! 1. **A colony that remembers.** The shipped `memory-hive/store` and
//!    `memory-hive/porter`, wired with the hive's own transfer edges, feeding
//!    the shipped `member/export-sink`. It is walked with `in_export` and the
//!    seed set lands on disk.
//! 2. **The example's own tool** turns that directory into ONE manifest: the
//!    shipped `member` written out (the hive spliced in where the reference
//!    stood, the export under `memory-hive/store/seed/`), registered as a local
//!    template by `add_templates` and instantiated by `add_nodes` in the SAME
//!    diff.
//! 3. **A colony that never heard any of it** applies that manifest and answers
//!    a question out of the first colony's memory.
//!
//! # The order is the whole mechanism
//!
//! A seed is read once, when the `cell.db` is created. `add_templates` runs
//! FIRST inside a diff and its registrations are visible to the `add_nodes` of
//! the same diff (GH #443), which is what makes "declare the tree, then grow it"
//! one operation rather than two — and there is no second chance: a member that
//! is already running cannot be given a past.
//!
//! # What this file measures that a file-reading test cannot
//!
//! * The staging seeder builds each table from the header line alone, so the
//!   alias tables arrive **without their key**; the store's own
//!   `apply_canonical_ddl` repairs that at first wake (GH #255). This asserts
//!   the repair on a table whose rows came out of another colony — the case
//!   `memory-hive/README.md` describes as the half the birth path cannot reach.
//! * The FTS index is built at first wake, over rows the seeder had already
//!   written. Without the backfill every imported episode would be in the
//!   database and invisible to the lexical leg of a recall — a memory that is
//!   present and unfindable, which is the failure this step exists to rule out.
//! * The two colonies never share a file: what travels is a document.
//!
//! **One deliberate substitution, named rather than hidden** (the same one
//! `gh447_an_export_lands_as_a_seed_set` makes, for the same reason): the
//! shipped sink runs behind `params.sandbox` with `trust: "restricted"`, which
//! is fail-closed against the host, so a colony test that kept it would measure
//! the kernel it runs on. The profile is replaced and the shipped value is
//! asserted first; the script, the timeout, the concurrency cap and the contract
//! are the shipped bytes.
//!
//! Guarded like every template-reading test (GH #49): a tree that does not carry
//! the library or the example is skipped, never judged.

use meclaw_cells::LlmCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_cells::timer::TimerCellFactory;
use meclaw_colony::{
    CellFactory, CellFactoryRegistry, ColonyMsg, MutationDoorOutcome, bootstrap_from_filesystem,
};
use meclaw_core::serde_json::{Map, Value, from_str, json, to_string_pretty};
use meclaw_core::{Body, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// Generous failure-marker timeout (CONTRIBUTING.md 30 s convention).
const RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// The member this example carries a past for. The name is the template name's
/// tail and the node name, exactly as `build_import.py` composes it.
const MEMBER: &str = "alex";

/// The word only the FIRST colony ever heard. Everything the second colony
/// answers with it, it answers out of a document.
const ONLY_IN_THE_PAST: &str = "ecotec";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The library, the two templates this walks and the example's own tool.
fn shipped() -> bool {
    [
        "templates/member/config.json",
        "templates/memory-hive/store/config.json",
        "templates/memory-hive/porter/config.json",
        "templates/member/export-sink/config.json",
        "templates/affinity/config.json",
        "templates/firewall/config.json",
        "examples/memory-import/build_import.py",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
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
    ]
}

fn registry() -> CellFactoryRegistry {
    let mut r = CellFactoryRegistry::new();
    for (name, f) in factories() {
        r.insert(name, f);
    }
    r
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

fn write_json(path: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, to_string_pretty(v).unwrap()).unwrap();
}

fn shipped_config(rel: &str) -> Value {
    from_str(&std::fs::read_to_string(repo(rel)).expect(rel)).expect("shipped config is json")
}

/// Read a cell's `cell.db` from outside — an observation of the result, never a
/// re-implementation of the mechanism (the device `gh255` uses).
fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    assert!(db.is_file(), "no cell.db at {}", db.display());
    let conn = rusqlite::Connection::open(db).expect("open cell.db");
    let mut st = conn.prepare(sql).expect("prepare");
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

fn primary_key(db: &std::path::Path, table: &str) -> Vec<String> {
    rows(
        db,
        &format!("SELECT name FROM pragma_table_info('{table}') WHERE pk > 0 ORDER BY pk"),
    )
    .into_iter()
    .map(|r| r[0].clone())
    .collect()
}

// ───────────────────────────────────────────────── colony A: it remembers

/// The two content tables this past is made of. Their headers are the store's
/// own declaration for those tables — which is what makes them seed files, and
/// what the porter's document repeats on the way out.
fn boot_seed(store_params: &Value) -> Vec<(String, String)> {
    let header = |table: &str| {
        let cols = store_params["schema"][table]
            .as_object()
            .unwrap_or_else(|| panic!("the shipped store declares no {table}"));
        json!({ "schema": Value::Object(cols.clone()) })
    };
    let row = |v: Value| meclaw_core::serde_json::to_string(&v).unwrap();
    let episodes = format!(
        "{}\n{}\n{}\n",
        header("episodes"),
        row(json!({
            "id": "e-1", "session_id": "s-1", "turn_id": "s-1#1",
            "sender": "user", "speaker": "member:alex", "channel": "kitchen",
            "audience_set": "[\"member:alex\",\"agent:scribe\"]",
            "content": "the boiler in the flat is a Vaillant ecoTEC and the plumber serviced it",
            "happened_at": "2026-01-14T09:00:00Z", "recorded_at": "2026-01-14T09:00:01Z"
        })),
        row(json!({
            "id": "e-2", "session_id": "s-1", "turn_id": "s-1#2",
            "sender": "assistant", "speaker": "agent:scribe", "channel": "kitchen",
            "audience_set": "[\"member:alex\",\"agent:scribe\"]",
            "content": "noted, the service is due again in a year",
            "happened_at": "2026-01-14T09:00:10Z", "recorded_at": "2026-01-14T09:00:11Z"
        })),
    );
    let facts = format!(
        "{}\n{}\n",
        header("facts"),
        row(json!({
            "id": "f-1", "episode_id": "e-1", "session_id": "s-1", "channel": "kitchen",
            "audience_set": "[\"member:alex\",\"agent:scribe\"]",
            "subject": "boiler", "canonical_subject": "boiler",
            "predicate": "is model", "canonical_predicate": "is model",
            "claim": "Vaillant ecoTEC", "canonical_claim": "Vaillant ecoTEC",
            "claim_hash": "h-1", "fact_kind": "attribute",
            "valid_from": "2026-01-14T09:00:00Z", "valid_until": "",
            "recorded_at": "2026-01-14T09:00:01Z", "expired_at": "",
            "superseded_by": "", "closure_source": "", "confidence": 90
        })),
    );
    vec![
        ("episodes.jsonl".to_string(), episodes),
        ("facts.jsonl".to_string(), facts),
    ]
}

/// A code cell that lives INSIDE the hive and writes one `set_alias`.
///
/// It has to be inside: the shipped store declares `write_surface: "internal"`,
/// so only a sender within the hive may write to it. That is also why the alias
/// row cannot be a boot seed — the store loads `seed/<table>.jsonl` only for the
/// tables `params.schema` declares, and the alias families are created out of
/// `params.canonical` instead. A test fixture, never a template.
fn alias_writer() -> Value {
    let script = r#"
import sys, json
doc = json.load(sys.stdin)
hop = (doc["envelope"].get("header") or {}).get("hop") or {}
if hop.get("operation"):
    sys.stdout.write(json.dumps([{"header": {"route": "alias_done"},
                                  "messages": [{"origin": "assistant", "type": "text",
                                                "text": "written"}]}]))
else:
    args = {"operation": "set_alias", "table": "facts", "column": "claim",
            "alias": "combi boiler", "canonical": "Vaillant ecoTEC"}
    sys.stdout.write(json.dumps([{"header": {"route": "astore"},
                                  "messages": [{"origin": "assistant", "type": "tool_call",
                                                "id": "a-1", "text": json.dumps(args)}]}]))
"#;
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script,
                   "sandbox": {"trust": "trusted"}},
        "contract": {"version": "1.0.0", "settings": {}, "multi_send_capable": true,
                     "emits": {"body": {"messages": {"type": "array", "required": true}},
                               "hop": {"route": {"type": "string", "required": true}}},
                     "consumes": {"body": {"messages": {"type": "array", "required": true}}},
                     "capabilities": ["shell:exec"]}
    })
}

/// A code cell that writes one file named after the route that reached it.
///
/// Every lane this colony has to wait on ends here, so waiting is one rule and
/// a refusal is a FILE rather than the absence of one.
fn flag_cell(dir: &str) -> Value {
    let script = r#"
import sys, json, os
doc = json.load(sys.stdin)
hop = (doc["envelope"].get("header") or {}).get("hop") or {}
route = str(hop.get("route") or "unknown")
with open(os.path.join(doc["params"]["flag_dir"], route + ".json"), "w") as fh:
    fh.write(json.dumps(hop, sort_keys=True))
sys.stdout.write(json.dumps([]))
"#;
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "flag_dir": dir,
                   "sandbox": {"trust": "trusted"}},
        "contract": {"version": "1.0.0", "settings": {}, "multi_send_capable": true,
                     "emits": {}, "consumes": {},
                     "capabilities": ["shell:exec", "fs:write"]}
    })
}

/// The shipped sink, pointed at this test's directory (see the module header for
/// the sandbox substitution).
fn sink_config(export_dir: &str) -> Value {
    let mut c = shipped_config("templates/member/export-sink/config.json");
    assert_eq!(
        c["params"]["sandbox"]["trust"], "restricted",
        "the shipped sink is the one behind a boundary; if this ever reads \
         `trusted`, the substitution here is hiding a real regression"
    );
    c["params"]["export_dir"] = json!(export_dir);
    c["params"]["sandbox"] = json!({"trust": "trusted"});
    c
}

/// The hive's own transfer edges, verbatim in meaning from
/// `templates/memory-hive/config.json`, plus the two the fixture writer needs.
fn memory_hive_island(root: &std::path::Path) {
    let edges = json!([
        {"from": ".", "to": "./porter",
         "condition": "has(hop.route) && hop.route == 'in_export'",
         "modifier": {"set_context": {"store_origin": "'porter'", "mem_phase": "'export'"}}},
        {"from": "./porter", "to": "./store",
         "condition": "has(hop.route) && hop.route == 'pstore'",
         "modifier": {"set_context": {"store_origin": "'porter'", "mem_phase": "hop.phase",
                                      "port_run": "hop.port_run", "port_table": "hop.port_table"}}},
        {"from": "./store", "to": "./porter",
         "condition": "has(context.store_origin) && context.store_origin == 'porter'",
         "modifier": {"set_context": {"mem_phase": "context.mem_phase"}}},
        {"from": "./porter", "to": ".", "condition": "has(hop.route) && hop.route == 'dump'"},
        {"from": "./porter", "to": ".", "condition": "has(hop.route) && hop.route == 'reject'"},
        // the fixture that writes the one alias row, from inside the hive
        {"from": ".", "to": "./alias-writer",
         "condition": "has(hop.route) && hop.route == 'in_alias'"},
        {"from": "./alias-writer", "to": "./store",
         "condition": "has(hop.route) && hop.route == 'astore'",
         "modifier": {"set_context": {"store_origin": "'alias'"}}},
        {"from": "./store", "to": "./alias-writer",
         "condition": "has(context.store_origin) && context.store_origin == 'alias'"},
        {"from": "./alias-writer", "to": ".",
         "condition": "has(hop.route) && hop.route == 'alias_done'"}
    ]);
    write_json(
        &root.join("main/memory/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": edges}}}),
    );
    let store = shipped_config("templates/memory-hive/store/config.json");
    for (name, body) in boot_seed(&store["params"]) {
        std::fs::create_dir_all(root.join("main/memory/store/seed")).unwrap();
        std::fs::write(root.join("main/memory/store/seed").join(name), body).unwrap();
    }
    write_json(&root.join("main/memory/store/config.json"), &store);
    // The shipped seed of the hive itself travels with it: which embedding
    // generation is live is the store's own configuration.
    std::fs::copy(
        repo("templates/memory-hive/store/seed/emb_models.jsonl"),
        root.join("main/memory/store/seed/emb_models.jsonl"),
    )
    .unwrap();
    write_json(
        &root.join("main/memory/porter/config.json"),
        &shipped_config("templates/memory-hive/porter/config.json"),
    );
    write_json(
        &root.join("main/memory/alias-writer/config.json"),
        &alias_writer(),
    );
}

/// The colony that remembers: the hive island, the shipped sink, one flag cell.
fn build_source_colony(root: &std::path::Path, export_dir: &str, flag_dir: &str) {
    write_json(
        &root.join("main/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": ".", "to": "./memory",
             "condition": "has(hop.route) && hop.route == 'in_export'"},
            {"from": ".", "to": "./memory",
             "condition": "has(hop.route) && hop.route == 'in_alias'"},
            {"from": "./memory", "to": "./sink",
             "condition": "has(hop.route) && hop.route == 'dump'"},
            {"from": "./memory", "to": "./flag",
             "condition": "has(hop.route) && hop.route == 'alias_done'"},
            {"from": "./memory", "to": "./flag",
             "condition": "has(hop.route) && hop.route == 'reject'"},
            {"from": "./sink", "to": "./flag",
             "condition": "has(hop.route) && hop.route == 'export_done'"},
            {"from": "./sink", "to": "./flag",
             "condition": "has(hop.route) && hop.route == 'error'"}
        ]}}}),
    );
    memory_hive_island(root);
    write_json(
        &root.join("main/sink/config.json"),
        &sink_config(export_dir),
    );
    write_json(&root.join("main/flag/config.json"), &flag_cell(flag_dir));
}

async fn wait_for(path: &std::path::Path, what: &str, h: &ColonyHandle) {
    let deadline = std::time::Instant::now() + RECV_TIMEOUT;
    while std::time::Instant::now() < deadline && !path.exists() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        path.exists(),
        "{what} never arrived — dead letters: {:?}",
        h.drain_dead_letters()
            .await
            .iter()
            .map(|d| (d.sender_path.as_str().to_string(), d.reason.as_code()))
            .collect::<Vec<_>>()
    );
}

async fn nudge(h: &ColonyHandle, route: &str) {
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!(route));
    h.send(
        MessageBuilder::new(Path::new("/memory"))
            .hop(hop)
            .body(Body::Inline(json!({"messages": []})))
            .build(),
    )
    .await;
}

// ───────────────────────────────────────── colony B: it never heard any of it

/// Every `${VAR}` the library references WITHOUT a default, bound to a dummy —
/// collected from the tree rather than listed, so a key added to a template does
/// not read as a contract failure here (the device `gh291` uses).
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
    names
        .into_iter()
        .map(|n| format!("{n}=dummy-{n}\n"))
        .collect()
}

/// Run the example's own tool. It is what a reader would run, so it is what is
/// under test — a second implementation here would prove nothing about it.
fn build_manifest(export_dir: &std::path::Path, templates: &std::path::Path) -> Value {
    let out = std::process::Command::new("python3")
        .arg(repo("examples/memory-import/build_import.py"))
        .arg("--export")
        .arg(export_dir)
        .arg("--templates")
        .arg(templates)
        .arg("--scope")
        .arg("/")
        .arg("--name")
        .arg(MEMBER)
        .output()
        .expect("python3");
    assert!(
        out.status.success(),
        "build_import.py failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    from_str(&String::from_utf8_lossy(&out.stdout)).expect("the tool prints one manifest")
}

async fn boot_target(td: &tempfile::TempDir) -> ColonyHandle {
    let root = td.path();
    copy_tree(&repo("templates"), &root.join("templates"));
    // The container the org level ships for its people, and the one pair of
    // edges that makes it more than a picture. Activity is derived from the
    // edge table and it is RECURSIVE: a container nothing crosses into is
    // inactive, and a member grown into it inherits that -- every message to
    // its store would dead-letter as `cell_inactive`. Two edges are the whole
    // shell here; what this test is about is what arrives inside it.
    write_json(
        &root.join("main/config.json"),
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            {"from": ".", "to": "./members",
             "condition": "has(hop.route) && hop.route == 'in_turn'"},
            {"from": "./members", "to": ".",
             "condition": "has(hop.route) && hop.route == 'answer'"}
        ]}}}),
    );
    write_json(
        &root.join("main/members/config.json"),
        &json!({"cell": {"type": "hive"}}),
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

/// One store op as a `tool_call` turn, answered back to the probe.
fn op(target: &str, reply_to: &str, args: Value) -> Message {
    MessageBuilder::new(Path::new(target))
        .reply_to(Path::new(reply_to))
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(json!({"messages": [{
            "origin": "assistant",
            "type": "tool_call",
            "text": meclaw_core::serde_json::to_string(&args).unwrap(),
            "id": "call_1"
        }]})))
        .build()
}

async fn ask(
    h: &ColonyHandle,
    sink_rx: &mut mpsc::Receiver<Message>,
    target: &str,
    reply_to: &str,
    args: Value,
) -> (Option<String>, String) {
    h.send(op(target, reply_to, args.clone())).await;
    let m = match tokio::time::timeout(RECV_TIMEOUT, sink_rx.recv()).await {
        Ok(Some(m)) => m,
        other => panic!(
            "no answer to {args} from {target} ({other:?}) -- dead letters: {:?}",
            h.drain_dead_letters()
                .await
                .iter()
                .map(|d| (
                    d.sender_path.as_str().to_string(),
                    d.resolved_target.as_str().to_string(),
                    d.reason.as_code()
                ))
                .collect::<Vec<_>>()
        ),
    };
    let code = m
        .headers
        .hop
        .get("error_code")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let text = match &m.body {
        Body::Inline(v) => v["messages"][0]["text"].as_str().unwrap_or("").to_string(),
        Body::Blob(_) => panic!("inline expected"),
    };
    (code, text)
}

/// Step 1, shared by both claims: a colony that remembers, walked out onto disk.
///
/// It returns the directory the shipped sink wrote, which is the only thing that
/// travels — the colony itself is shut down before anything reads it.
async fn a_past_on_disk(source_td: &tempfile::TempDir) -> std::path::PathBuf {
    let export_dir = source_td.path().join("exports");
    let flag_dir = source_td.path().join("flags");
    std::fs::create_dir_all(&export_dir).unwrap();
    std::fs::create_dir_all(&flag_dir).unwrap();
    build_source_colony(
        source_td.path(),
        export_dir.to_str().unwrap(),
        flag_dir.to_str().unwrap(),
    );

    let source = ColonyHandle::new_with_factories_at(source_td, factories());
    bootstrap_from_filesystem(source_td.path(), &registry(), &source.runtime())
        .await
        .expect("the remembering colony must boot");

    nudge(&source, "in_alias").await;
    wait_for(&flag_dir.join("alias_done.json"), "the alias row", &source).await;

    nudge(&source, "in_export").await;
    wait_for(&flag_dir.join("export_done.json"), "the walk", &source).await;
    assert!(
        !flag_dir.join("reject.json").exists(),
        "the walk refused: {}",
        std::fs::read_to_string(flag_dir.join("reject.json")).unwrap_or_default()
    );
    source.shutdown().await;
    export_dir
}

// ───────────────────────────────────────────────────────────────── the claims

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fresh_colony_answers_out_of_a_memory_it_never_saw_written() {
    if !shipped() {
        return;
    }

    // ── 1. the colony that remembers, and the walk out of it ────────────────
    let source_td = tempfile::TempDir::new().unwrap();
    let export_dir = a_past_on_disk(&source_td).await;
    // Since GH #471 the sink files a part under the hive it came out of, so
    // this colony's one sender writes `memory-hive/seed/`.
    let seed = export_dir.join("memory-hive").join("seed");
    assert!(
        seed.join("export_final.json").is_file(),
        "an export without its completeness marker is a prefix, not a document"
    );
    for table in ["episodes", "facts", "claim_aliases"] {
        assert!(
            seed.join(format!("{table}.jsonl")).is_file(),
            "the walk wrote no {table} part"
        );
    }
    assert!(
        !seed.join("emb_models.jsonl").exists(),
        "the embedding generation is the RECEIVING hive's configuration and \
         must not travel"
    );

    // ── 2. the recipe: one manifest, built from that directory ──────────────
    let target_td = tempfile::TempDir::new().unwrap();
    let target = boot_target(&target_td).await;
    let manifest = build_manifest(&export_dir, &target_td.path().join("templates"));
    let files = &manifest["manifest"][0]["diff"]["add_templates"][0]["files"];
    assert!(
        files["memory-hive/store/config.json"].is_string(),
        "the reference was not written out — a ref carries no files, and a seed \
         is a file"
    );
    assert!(
        files["memory-hive/store/seed/episodes.jsonl"].is_string(),
        "the export did not land under the hive's own seed directory"
    );
    // The derived level inherits the `in_import` door from the shipped one
    // (member@1.4.0 accepts the lane and carries the edge onto `./memory-hive`);
    // the tool copies it rather than patching it in, and the second claim below
    // drives it. The lock on the shipped surface itself is
    // `gh467_the_shipped_member_carries_the_import_lane.rs`.
    assert!(
        files["config.json"]
            .as_str()
            .unwrap_or_default()
            .contains("in_import"),
        "the derived level carries no `in_import` door — the shipped member \
         declares one and this tool copies the level verbatim"
    );

    // ── 3. the colony that never heard any of it ────────────────────────────
    let outcome = apply(&target, manifest).await;
    assert!(
        outcome.is_committed(),
        "the import manifest must commit; got {outcome:?}"
    );

    // The probe sits INSIDE the hive and reads its answers over one internal
    // edge. Inside is not a convenience: the hive is sealed (`params.ports` is
    // empty), so the drain a caller may draw is at the hive path, and a probe
    // wired from the outside straight onto `./store` would be the breach
    // `hive_port_boundary` exists to refuse. What is asked here is a READ, and
    // a read is what the recall lane asks of this same cell.
    let store_path = format!("/members/{MEMBER}/memory-hive/store");
    let probe_path = format!("/members/{MEMBER}/memory-hive/probe");
    let (sink_tx, mut sink_rx) = mpsc::channel::<Message>(64);
    target
        .spawn(Path::new(&probe_path), move || {
            CaptureCell::new(sink_tx.clone())
        })
        .await;
    target
        .add_edge(
            Uuid::now_v7(),
            Path::new(&store_path),
            Path::new(&probe_path),
        )
        .await;

    // The lexical leg of a recall, asked directly of the store that answers it.
    // A `search` is a READ, so the store's internal write surface does not bound
    // it — and it is the one question that can only be answered if the FTS index
    // was backfilled over rows the seeder had already written.
    let (code, text) = ask(
        &target,
        &mut sink_rx,
        &store_path,
        &probe_path,
        json!({"operation": "search", "table": "episodes",
               "columns": ["id", "content", "audience_set"],
               "match": ONLY_IN_THE_PAST}),
    )
    .await;
    assert_eq!(code, None, "the search was refused: {text}");
    let hits: Value = from_str(&text).expect("search returns a row array");
    assert_eq!(
        hits.as_array().map(Vec::len),
        Some(1),
        "the imported episode is in the database and invisible to a recall: {text}"
    );
    assert_eq!(hits[0]["id"], "e-1");
    assert_eq!(
        hits[0]["audience_set"], "[\"member:alex\",\"agent:scribe\"]",
        "provenance is never reconstructed: an imported row whose participant \
         set did not survive is a row that may be told to anyone"
    );

    // ── 4. what the seeder cannot express, and the store repairs ────────────
    let db = target_td
        .path()
        .join(format!("main/members/{MEMBER}/memory-hive/store/cell.db"));
    assert_eq!(
        primary_key(&db, "claim_aliases"),
        vec!["alias".to_string()],
        "the seeded alias table stands without the key `set_alias` upserts on — \
         every identity judgement against it would fail from now on (GH #255)"
    );
    assert_eq!(
        rows(
            &db,
            "SELECT alias, canonical FROM claim_aliases ORDER BY alias"
        ),
        vec![vec![
            "combi boiler".to_string(),
            "Vaillant ecoTEC".to_string()
        ]],
        "the alias the first colony judged did not survive the transfer"
    );
    // Half two of the drift lock: the two public template surfaces that state
    // this mechanism in prose. A sentence no test reads is how the tree ended up
    // asserting the opposite of what the store does (GH #255 landed and both
    // READMEs kept saying a seeded alias table has no key).
    for (surface, sentence) in [
        (
            "templates/memory-hive/README.md",
            "the exception in 4b is retired",
        ),
        (
            "templates/memory-hive/README.md",
            "the key at its first wake instead of assuming it",
        ),
    ] {
        let text = std::fs::read_to_string(repo(surface)).expect(surface);
        assert!(
            text.contains(sentence),
            "{surface} no longer says {sentence:?} — the prose and the mechanism \
             this test measures have come apart"
        );
    }
    assert_eq!(
        rows(&db, "SELECT model_id FROM emb_models").len(),
        1,
        "the hive's own seed must survive beside the imported one"
    );
    assert_eq!(
        rows(&db, "SELECT id FROM facts ORDER BY id"),
        vec![vec!["f-1".to_string()]],
        "the fact the first colony learned did not arrive"
    );

    let dl = target.drain_dead_letters().await;
    assert!(
        dl.is_empty(),
        "an import that dead-letters is the state this lane was built to end; got {:?}",
        dl.iter()
            .map(|d| (
                d.sender_path.as_str().to_string(),
                d.resolved_target.as_str().to_string(),
                d.reason.as_code()
            ))
            .collect::<Vec<_>>()
    );
    target.shutdown().await;
}

/// The named second step: a document that arrives AFTER the birth.
///
/// A seed is read once, when the `cell.db` is created, and is inert for ever
/// after — so everything the source learned since the export was walked has no
/// way in through the manifest. `memory-hive` has accepted `in_import` since
/// 2.2.0, and since `member@1.4.0` the level carries the door through: one
/// accepted lane and one plain edge onto `./memory-hive`, shipped rather than
/// patched into a derived template. This drives that lane end to end, because a
/// lane a template declares and nothing drives is the dead-lane class of
/// `docs/development-rules.md` § 2c.
///
/// Two claims, and the second is what makes the first usable: the delta lands,
/// and the same part applied twice leaves the same state. Idempotency is not a
/// nicety here — it is the whole repair procedure for a partial transfer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_second_step_takes_a_later_document_into_the_running_member() {
    if !shipped() {
        return;
    }
    let source_td = tempfile::TempDir::new().unwrap();
    let export_dir = a_past_on_disk(&source_td).await;

    let target_td = tempfile::TempDir::new().unwrap();
    let target = boot_target(&target_td).await;
    let mut manifest = build_manifest(&export_dir, &target_td.path().join("templates"));
    // An import receipt rides the same `dump` lane the export parts ride, so
    // the level's sink is going to be spawned by this test. Its shipped
    // `restricted` profile is fail-closed against the host (see the module
    // header), and it is relaxed here through the DECLARED override rather than
    // by editing the tree — which is also the one place a reader would relax it.
    manifest["manifest"][0]["diff"]["add_nodes"][0]["override_params"] =
        json!({"export-sink": {"sandbox": {"trust": "trusted"}}});
    let outcome = apply(&target, manifest).await;
    assert!(
        outcome.is_committed(),
        "the import manifest must commit; got {outcome:?}"
    );

    let db = target_td
        .path()
        .join(format!("main/members/{MEMBER}/memory-hive/store/cell.db"));
    let schema =
        shipped_config("templates/memory-hive/store/config.json")["params"]["schema"]["episodes"]
            .clone();
    let part = json!({
        "format": "meclaw-memory-export/1", "hive_template": "memory-hive",
        "export_id": "delta-467", "exported_at": "2026-03-02T00:00:00Z",
        "table": "episodes", "part": 1, "of": 1, "final": false, "absent": false,
        "key": ["id"], "schema": schema,
        "rows": [{
            "id": "e-3", "session_id": "s-2", "turn_id": "s-2#1",
            "sender": "user", "speaker": "member:alex", "channel": "kitchen",
            "audience_set": "[\"member:alex\",\"agent:scribe\"]",
            "content": "the plumber is coming back in March for the annual service",
            "happened_at": "2026-03-01T08:00:00Z", "recorded_at": "2026-03-01T08:00:01Z"
        }]
    });

    for round in 1..=2 {
        let mut hop = Map::new();
        hop.insert("route".to_string(), json!("in_import"));
        target
            .send(
                MessageBuilder::new(Path::new(&format!("/members/{MEMBER}")))
                    .hop(hop)
                    .body(Body::Inline(json!({"messages": [{
                        "origin": "assistant", "type": "text",
                        "text": meclaw_core::serde_json::to_string(&part).unwrap()
                    }]})))
                    .build(),
            )
            .await;

        let deadline = std::time::Instant::now() + RECV_TIMEOUT;
        while std::time::Instant::now() < deadline
            && rows(&db, "SELECT id FROM episodes WHERE id = 'e-3'").is_empty()
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(
            rows(&db, "SELECT id FROM episodes WHERE id = 'e-3'").len(),
            1,
            "round {round}: the delta did not land, or landed twice — dead \
             letters: {:?}",
            target
                .drain_dead_letters()
                .await
                .iter()
                .map(|d| (
                    d.sender_path.as_str().to_string(),
                    d.resolved_target.as_str().to_string(),
                    d.reason.as_code()
                ))
                .collect::<Vec<_>>()
        );
    }

    // The document is additive and never replacing: what the birth seed brought
    // is still there beside what the second step added.
    assert_eq!(
        rows(&db, "SELECT id FROM episodes ORDER BY id"),
        vec![
            vec!["e-1".to_string()],
            vec!["e-2".to_string()],
            vec!["e-3".to_string()]
        ],
        "an import never deletes and never replaces"
    );
    target.shutdown().await;
}

/// A directory without the completeness marker is a PREFIX of a document, and
/// the tool refuses it rather than growing a member with half a past.
///
/// The failure it prevents is silent by construction: a prefix looks exactly
/// like a whole document from the outside, and a member born from one has no way
/// to discover what it is missing.
#[test]
fn an_unfinished_export_is_refused_before_it_can_become_a_member() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    let seed = td.path().join("memory-hive").join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    std::fs::write(
        seed.join("episodes.jsonl"),
        "{\"schema\": {\"id\": \"text\"}}\n{\"id\": \"e-1\"}\n",
    )
    .unwrap();

    let out = std::process::Command::new("python3")
        .arg(repo("examples/memory-import/build_import.py"))
        .arg("--export")
        .arg(td.path())
        .arg("--templates")
        .arg(repo("templates"))
        .arg("--scope")
        .arg("/")
        .arg("--name")
        .arg(MEMBER)
        .output()
        .expect("python3");
    assert!(
        !out.status.success(),
        "a prefix of a document was accepted as a whole one"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("export_final.json"),
        "the refusal must name what is missing; got: {err}"
    );
}
