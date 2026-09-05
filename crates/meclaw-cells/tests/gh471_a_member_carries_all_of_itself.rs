//! GH #471 — a member's export carries everything a member IS, and a member
//! born from it arrives with all of it.
//!
//! Measured before this landed: growing a fresh colony from an older one's
//! export reproduced the memory hive completely — every episode, every fact,
//! every embedding — and reproduced nothing else. `affinity` (six entities, ten
//! relations, seventeen trust rows, forty-three disclosure decisions) arrived
//! empty, and so did `firewall/rules`. A member reborn like that remembers
//! everything it was told and knows nothing about who may be told what, and it
//! screens its first inbound turn against an empty rule table.
//!
//! `memory` produces and `affinity` decides. That is the whole reason the
//! second half is not a nice-to-have: the curated record IS the disclosure
//! machinery, and `firewall/rules` IS the screen. An export that carries one
//! and not the other is not a smaller backup, it is a different security
//! posture wearing the same name.
//!
//! Two colonies, both real, sharing nothing but a directory of files:
//!
//! * **A** grows a shipped `member@1.5.0`, gets one distinctive row written
//!   into each of its three holders, and is told `in_export` ONCE. Three walks
//!   run, each holder's own store writes its document, and each of the three
//!   directories carries its own completeness marker (GH #555).
//! * **B** never heard any of it. `examples/memory-import/build_import.py`
//!   turns the directory into one manifest; the manifest grows a member; and
//!   the member arrives with the memory, the record AND the screen.
//!
//! The third claim is the one a row count cannot reach: B's firewall REFUSES a
//! turn on a rule that only ever existed in A. A rule table that arrives and is
//! not read is a table, not a screen.
//!
//! **No substitution any more.** Until GH #555 this file relaxed the shipped
//! sink's `restricted` sandbox through `override_params`, because a profile
//! that is fail-closed against the host makes a colony test measure the kernel
//! it runs on. There is no sink and no sandbox: the writer is the substrate,
//! and the only thing an instance says about files is the fence each holder's
//! store declares (`params.transfer.base_path`), which this file sets through
//! the same declared override.
//!
//! Guarded like every template-reading test (GH #49): a tree that does not
//! carry the library or the example is skipped, never judged.

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

const RECV_TIMEOUT: Duration = Duration::from_secs(30);
const MEMBER: &str = "alex";
/// The three rows that exist in colony A and nowhere else. Each one is in a
/// different holder, so a transfer that carried two of three is a failure with
/// a name rather than a smaller number.
const ENTITY: &str = "entity:zora";
const RULE: &str = "block-zora-471";
const EPISODE: &str = "e-zora-471";
const FORBIDDEN: &str = "zora protocol";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/member/config.json",
        "templates/affinity/porter/config.json",
        "templates/firewall/porter/config.json",
        "templates/memory-hive/porter/config.json",
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

fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    if !db.is_file() {
        return Vec::new();
    }
    let conn = rusqlite::Connection::open(db).expect("open cell.db");
    let mut st = match conn.prepare(sql) {
        Ok(st) => st,
        // The store creates its tables when it first wakes; a keeper nobody has
        // spoken to yet has a file and no schema, and that is an empty ledger
        // rather than a defect (GH #565).
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
/// plus the two crons this file pushes out of the way. A nightly dream or a
/// five-minute identity push firing mid-run would emit into edges no test
/// topology drew, and the dead-letter assertion at the end is worth more than
/// the coincidence.
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

/// The shell both colonies boot: a members container, and a flag cell that
/// takes everything the member level raises.
async fn boot(td: &tempfile::TempDir, flag_dir: &std::path::Path) -> ColonyHandle {
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

/// The container wiring `examples/memory-import/build_import.py` writes, which
/// `gh470_a_grown_container_level_carries_its_export_lanes` pins against the
/// builder's own table. Read from the shipped script rather than repeated here.
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

fn op(target: &str, reply_to: &str, args: Value) -> Message {
    MessageBuilder::new(Path::new(target))
        .reply_to(Path::new(reply_to))
        .trace_id(Uuid::now_v7())
        .body(Body::Inline(json!({"messages": [{
            "origin": "assistant", "type": "tool_call",
            "text": meclaw_core::serde_json::to_string(&args).unwrap(), "id": "call_1"}]})))
        .build()
}

async fn ask(
    h: &ColonyHandle,
    sink_rx: &mut mpsc::Receiver<Message>,
    target: &str,
    reply_to: &str,
    args: Value,
) -> Option<String> {
    h.send(op(target, reply_to, args.clone())).await;
    let m = match tokio::time::timeout(RECV_TIMEOUT, sink_rx.recv()).await {
        Ok(Some(m)) => m,
        other => panic!(
            "no answer to {args} from {target} ({other:?}) -- dead letters: {:?}",
            h.drain_dead_letters()
                .await
                .iter()
                .map(|d| (d.sender_path.as_str().to_string(), d.reason.as_code()))
                .collect::<Vec<_>>()
        ),
    };
    m.headers
        .hop
        .get("error_code")
        .and_then(|v| v.as_str())
        .map(str::to_string)
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
        h.drain_dead_letters()
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

fn member_manifest(export_dir: Option<&std::path::Path>) -> Value {
    // Since GH #555 an instance says ONE thing about files: the fence each
    // holder's own store writes inside. There is no cell to relax and no
    // sandbox to substitute — the writer is the substrate.
    let mut over = json!({"memory-hive/clock": quiet_night(),
                          "affinity/clock": quiet_push()});
    if let Some(dir) = export_dir {
        for (hive, cell) in [
            ("memory-hive", "store"),
            ("affinity", "store"),
            ("firewall", "rules"),
        ] {
            over[format!("{hive}/{cell}")] =
                json!({"transfer": {"base_path": dir.to_str().unwrap()}});
        }
    }
    json!({"manifest": [{
        // The declaration stands AT the container it grows into (GH #503),
        // which is the form `build_import.py` writes and the form
        // `container_edges()` above is spelled in: `.` is `/members`, the
        // member is named bare, and the path it lands at is unchanged.
        "scope": "/members",
        "diff": {
            "add_nodes": [{"name": MEMBER, "template": "member@1.6.0",
                           "override_params": over}],
            "add_edges": container_edges(),
        }
    }]})
}

// ─────────────────────────────────────────────────────────────────── the run

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_export_carries_memory_record_and_screen_and_a_member_is_born_with_all_three() {
    if !shipped() {
        return;
    }

    // ── colony A: a member with something of its own in every holder ────────
    let a_td = tempfile::TempDir::new().unwrap();
    let a_flags = a_td.path().join("flags");
    let export_dir = a_td.path().join("exports");
    std::fs::create_dir_all(&export_dir).unwrap();
    let a = boot(&a_td, &a_flags).await;
    let outcome = apply(&a, member_manifest(Some(&export_dir))).await;
    assert!(
        outcome.is_committed(),
        "growing the shipped member must commit; got {outcome:?}"
    );

    let (tx, mut rx) = mpsc::channel::<Message>(64);
    let base = format!("/members/{MEMBER}");
    // One probe INSIDE each holder. `affinity/store` declares
    // `write_surface: "internal"`, so a writer outside the hive is refused
    // `write_denied` -- which is the guarantee, not an obstacle: this file
    // writes the way the hive's own cells do.
    let mut probes = std::collections::BTreeMap::new();
    for (hive, cell) in [
        ("affinity", "store"),
        ("firewall", "rules"),
        ("memory-hive", "store"),
    ] {
        let probe = format!("{base}/{hive}/probe471");
        let tx = tx.clone();
        a.spawn(Path::new(&probe), move || CaptureCell::new(tx.clone()))
            .await;
        a.add_edge(
            Uuid::now_v7(),
            Path::new(&format!("{base}/{hive}/{cell}")),
            Path::new(&probe),
        )
        .await;
        probes.insert(hive, probe);
    }
    let probe_affinity = probes["affinity"].clone();
    let probe_firewall = probes["firewall"].clone();
    let probe_memory = probes["memory-hive"].clone();

    // One row per holder, written the way an operator writes one: a store op
    // over an edge, never a file edited under a running colony.
    assert_eq!(
        ask(
            &a,
            &mut rx,
            &format!("{base}/affinity/store"),
            &probe_affinity,
            json!({"operation": "insert", "table": "entities",
                   "row": {"entity_id": ENTITY, "kind": "person",
                           "display_name": "Zora Vale", "owner_member": "member:alex",
                           "aieos": {}, "aieos_version": "1.1.0", "mx": {},
                           "status": "active", "supersedes": "", "source": "curated",
                           "confidence": 90, "recorded_at": "2026-08-28T10:00:00Z"}})
        )
        .await,
        None
    );
    assert_eq!(
        ask(
            &a,
            &mut rx,
            &format!("{base}/firewall/rules"),
            &probe_firewall,
            json!({"operation": "insert", "table": "rules",
                   "row": {"rule_id": RULE, "kind": "substring", "field": "text",
                           "value": FORBIDDEN, "action": "reject", "enabled": 1,
                           "note": "the row that only colony A has"}})
        )
        .await,
        None
    );
    assert_eq!(
        ask(
            &a,
            &mut rx,
            &format!("{base}/memory-hive/store"),
            &probe_memory,
            json!({"operation": "insert", "table": "episodes",
                   "row": {"id": EPISODE, "session_id": "s-471", "turn_id": "t-1",
                           "sender": "user", "speaker": "member:alex",
                           "channel": "tg:471", "audience_set": "member:alex",
                           "content": "Zora runs the protocol",
                           "happened_at": "2026-08-28T10:00:00Z",
                           "recorded_at": "2026-08-28T10:00:00Z"}})
        )
        .await,
        None
    );

    // ── one word, three walks ───────────────────────────────────────────────
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!("in_export"));
    a.send(
        MessageBuilder::new(Path::new(&base))
            .hop(hop)
            .body(Body::Inline(json!({"messages": []})))
            .build(),
    )
    .await;

    for hive in ["memory-hive", "affinity", "firewall"] {
        wait_for(
            &export_dir.join(hive).join("seed/export_final.json"),
            &format!("{hive}'s completeness marker"),
            &a,
        )
        .await;
    }
    // Each directory carries its own marker and NOTHING carries a member-level
    // one any more: it was a composition statement about four hives, and it
    // went with the cell that composed it (GH #555). What replaces it is that
    // every directory says for itself whether it is whole.
    assert!(
        !export_dir.join("export_final.json").exists(),
        "something wrote a member-level marker. The substrate knows no hives, and \
         a composition statement written by nobody is a claim with no author"
    );
    for hive in ["memory-hive", "affinity", "firewall"] {
        let marker: Value = from_str(
            &std::fs::read_to_string(export_dir.join(hive).join("seed/export_final.json")).unwrap(),
        )
        .expect("a marker is parseable JSON");
        assert_eq!(marker["format"], "meclaw-cell-export/1");
        assert!(marker["tables"].as_array().map(Vec::len).unwrap_or(0) > 0);
    }
    assert!(
        !a_flags.join("reject.json").exists(),
        "a walk refused something: {:?}",
        std::fs::read_to_string(a_flags.join("reject.json"))
    );
    // and every part is filed under its own hive, so the two `entities` tables
    // in this export -- the memory hive's and affinity's -- are two files
    assert!(export_dir.join("memory-hive/seed/entities.jsonl").is_file());
    assert!(export_dir.join("affinity/seed/entities.jsonl").is_file());
    assert!(export_dir.join("firewall/seed/rules.jsonl").is_file());
    assert!(
        !export_dir.join("firewall/seed/arrivals.jsonl").exists(),
        "the rate window travelled. It is the budget THIS colony spent, and a \
         member born with a full one refuses turns for traffic it never saw"
    );
    a.shutdown().await;

    // ── colony B: it never heard any of this ────────────────────────────────
    let b_td = tempfile::TempDir::new().unwrap();
    let b_flags = b_td.path().join("flags");
    let b = boot(&b_td, &b_flags).await;
    let out = std::process::Command::new("python3")
        .arg(repo("examples/memory-import/build_import.py"))
        .arg("--export")
        .arg(&export_dir)
        .arg("--templates")
        .arg(b_td.path().join("templates"))
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
    let manifest: Value = from_str(&String::from_utf8_lossy(&out.stdout)).expect("manifest");
    let outcome = apply(&b, manifest).await;
    assert!(
        outcome.is_committed(),
        "the import manifest must commit; got {outcome:?}"
    );

    // ── all three holders, born full ────────────────────────────────────────
    let root = b_td.path().join(format!("main/members/{MEMBER}"));
    assert_eq!(
        rows(
            &root.join("affinity/store/cell.db"),
            &format!("SELECT display_name FROM entities WHERE entity_id = '{ENTITY}'")
        ),
        vec![vec!["Zora Vale".to_string()]],
        "the curated record did not travel. This is the half GH #471 measured \
         as empty: a member that remembers everything and knows nothing about \
         who may be told it"
    );
    assert_eq!(
        rows(
            &root.join("firewall/rules/cell.db"),
            &format!("SELECT value FROM rules WHERE rule_id = '{RULE}'")
        ),
        vec![vec![FORBIDDEN.to_string()]],
        "the screen did not travel"
    );
    assert_eq!(
        rows(
            &root.join("memory-hive/store/cell.db"),
            &format!("SELECT session_id FROM episodes WHERE id = '{EPISODE}'")
        ),
        vec![vec!["s-471".to_string()]],
        "the memory did not travel -- the half that always worked"
    );
    assert!(
        rows(
            &root.join("affinity/store/cell.db"),
            &format!("SELECT entity_id FROM entities WHERE entity_id = '{ENTITY}'")
        )
        .len()
            == 1,
        "the imported record is doubled -- the shipped placeholder seed was \
         left standing beside the export instead of being replaced by it"
    );

    // ── and the screen SCREENS ──────────────────────────────────────────────
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!("in_turn"));
    let mut ctx = Map::new();
    ctx.insert("channel".to_string(), json!("tg:471"));
    b.send(
        MessageBuilder::new(Path::new(&base))
            .hop(hop)
            .context(ctx)
            .body(Body::Inline(json!({"messages": [{
                "origin": "user", "type": "text",
                "text": format!("please run the {FORBIDDEN} now")}]})))
            .build(),
    )
    .await;
    wait_for(&b_flags.join("reject.json"), "the screened turn", &b).await;
    let refusals: Value =
        from_str(&std::fs::read_to_string(b_flags.join("reject.json")).unwrap()).unwrap();
    assert_eq!(
        refusals[0]["hop"]["rule_id"], RULE,
        "the turn was refused by some other rule, or not by the imported one. A \
         rule table that arrives and is not read is a table, not a screen: \
         {refusals:?}"
    );

    let dl = b.drain_dead_letters().await;
    assert!(
        dl.is_empty(),
        "a member born from an export dead-lettered; got {:?}",
        dl.iter()
            .map(|d| (
                d.sender_path.as_str().to_string(),
                d.resolved_target.as_str().to_string(),
                d.reason.as_code()
            ))
            .collect::<Vec<_>>()
    );
    b.shutdown().await;
}
