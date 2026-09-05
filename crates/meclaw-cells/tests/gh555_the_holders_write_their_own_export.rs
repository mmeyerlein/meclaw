//! GH #555 — the holders of a member write their own export, and no cell of
//! the member level touches a file any more.
//!
//! Until `member@1.5.1` an export travelled as MESSAGES. Every holder walked
//! its tables, handed one `dump` part per table up to the member level, and one
//! `code` cell there — `export-sink` — turned the stack back into the files it
//! had been all along. The ruling that removed it is one sentence (R-0904-3,
//! 2026-09-04): *"cells manage their own files, nobody else does."*
//!
//! So this is the whole path, end to end and on the shipped tree:
//!
//! 1. the request enters at the shipped `operator/export` occupant, which owns
//!    no format and holds no dump — it turns `{target, export_to}` into an
//!    ADDRESS on the `export` lane;
//! 2. the OS's own restamp (`set_hop {route: 'in_export'}`, spelled here
//!    exactly as `templates/meclaw-os/config.json` spells it) carries it down;
//! 3. the member fans it out to all three holders at once;
//! 4. each holder's porter names a directory and its STORE writes the seed set
//!    itself, through the substrate's `transfer` slot, inside the fence that
//!    store declares for itself (`params.transfer.base_path`);
//! 5. each porter says `export_done` off the slot's own receipt, with
//!    `hop.seed_dir` naming where — three of them, one per holder.
//!
//! Four properties, one per way this could look finished and be wrong:
//!
//! * **Three directories, one per holder.** `memory-hive` and `affinity` both
//!   have a table called `entities`; one shared directory would write one over
//!   the other without a word (GH #471). The name comes from the holder, the
//!   run directory from `hop.export_to` — which is exactly what the operator
//!   passes through and nothing here invents.
//! * **Every directory is a complete document.** Schema line first, one row per
//!   line after it, and `export_final.json` beside them, written last: a reader
//!   that watches the marker never meets a directory that is still filling.
//! * **The completion word comes from the holder itself**, carrying
//!   `hop.seed_dir` relative to the fence. Before this the LEVEL said it, for a
//!   walk it had reassembled.
//! * **The member level owns no cell at all any more**, and the example that
//!   reads an export back reads what this run wrote — as a subprocess, so the
//!   claim is the shipped script's and not a re-implementation of it.
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
use meclaw_core::{Body, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

const RECV_TIMEOUT: Duration = Duration::from_secs(30);
const MEMBER: &str = "alex";
/// The directory of ONE run, named by the caller and passed through untouched.
const RUN: &str = "run-555";
/// The three holders that write themselves out, and the store cell inside each
/// one that owns the fence.
const HOLDERS: [(&str, &str); 3] = [
    ("memory-hive", "store"),
    ("affinity", "store"),
    ("firewall", "rules"),
];

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/member/config.json",
        "templates/operator/export/config.json",
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

fn shipped_config(rel: &str) -> Value {
    from_str(&std::fs::read_to_string(repo(rel)).expect(rel)).expect("shipped config is json")
}

/// Every `${VAR}` the library references WITHOUT a default, bound to a dummy,
/// plus the three crons this file pushes out of the way: a tick firing mid-run
/// would emit into edges no test topology drew.
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

/// The shell: the shipped operator occupant as the trigger, a members
/// container, and one flag cell that takes everything either of them raises.
///
/// The two edges around the trigger are the OS's own, spelled the way
/// `templates/meclaw-os/config.json` spells them — `export` restamped to
/// `in_export` by a `set_hop` and relayed unchanged. That is the point of
/// driving the request from here instead of sending `in_export` by hand: what
/// carries `hop.export_to` down is a restamp, and a restamp that dropped it
/// would be invisible to a test that stamped the lane itself.
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
    let mut edges = vec![
        json!({"from": ".", "to": "./trigger",
               "condition": "has(hop.route) && hop.route == 'in_dump'"}),
        json!({"from": "./trigger", "to": "./members",
               "condition": "has(hop.route) && hop.route == 'export'",
               "modifier": {"set_hop": {"route": "'in_export'"}}}),
        json!({"from": "./trigger", "to": "./flag",
               "condition": "has(hop.route) && hop.route == 'receipt'"}),
    ];
    for lane in [
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
    ] {
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
        &root.join("main/trigger/config.json"),
        &shipped_config("templates/operator/export/config.json"),
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

/// The member, with one `override_params` entry per holder: the fence, and
/// nothing else. It is the ONLY thing an instance has to say about files now —
/// the sink used to need a directory, a sandbox write root and a cell of its
/// own.
fn member_manifest(fence: &std::path::Path) -> Value {
    let mut over = Map::new();
    over.insert("memory-hive/clock".into(), quiet_night());
    over.insert("affinity/clock".into(), quiet_push());
    for (hive, cell) in HOLDERS {
        over.insert(
            format!("{hive}/{cell}"),
            json!({"transfer": {"base_path": fence.to_str().unwrap()}}),
        );
    }
    json!({"manifest": [{
        "scope": "/members",
        "diff": {
            "add_nodes": [{"name": MEMBER, "template": "member@1.6.0",
                           "override_params": Value::Object(over)}],
            "add_edges": [
                {"from": ".", "to": format!("./{MEMBER}"),
                 "condition": "has(hop.route) && hop.route == 'in_export'"},
                {"from": format!("./{MEMBER}"), "to": ".",
                 "condition": "has(hop.route) && hop.route == 'export_done'"},
                {"from": format!("./{MEMBER}"), "to": ".",
                 "condition": "has(hop.route) && hop.route == 'reject'"},
                {"from": format!("./{MEMBER}"), "to": ".",
                 "condition": "has(hop.route) && hop.route == 'error'"},
            ],
        }
    }]})
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

/// Poll the `export_done` flag file until `want` of them have arrived.
async fn wait_done(flags: &std::path::Path, want: usize, h: &ColonyHandle) -> Vec<Value> {
    wait_lane(flags, "export_done", want, h).await
}

/// Poll one lane's flag file until `want` messages have arrived on it.
async fn wait_lane(
    flags: &std::path::Path,
    lane: &str,
    want: usize,
    h: &ColonyHandle,
) -> Vec<Value> {
    let p = flags.join(format!("{lane}.json"));
    let deadline = std::time::Instant::now() + RECV_TIMEOUT;
    loop {
        let seen = std::fs::read_to_string(&p)
            .ok()
            .and_then(|raw| from_str::<Value>(&raw).ok())
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        if seen.len() >= want {
            return seen;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "only {} of {want} holders reached `{lane}` (last: {seen:?}) -- \
                 dead letters: {:?}",
                seen.len(),
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
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn read_lines(p: &std::path::Path) -> Vec<Value> {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| from_str(l).unwrap_or_else(|e| panic!("{} line is not json: {e}", p.display())))
        .collect()
}

// ─────────────────────────────────────────────────────────────────── the run

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_three_holders_write_their_own_seed_sets_and_say_so_themselves() {
    if !shipped() {
        return;
    }
    assert!(
        !repo("templates/member/export-sink").exists(),
        "the member level still ships a cell that writes somebody else's files. \
         The whole of GH #555 is that it does not: a store writes its own seed \
         set through the `transfer` slot, inside the fence it declares itself"
    );

    let td = tempfile::TempDir::new().unwrap();
    let flags = td.path().join("flags");
    let fence = td.path().join("exports");
    std::fs::create_dir_all(&fence).unwrap();
    let h = boot(&td, &flags).await;
    let outcome = apply(&h, member_manifest(&fence)).await;
    assert!(
        outcome.is_committed(),
        "growing the shipped member must commit; got {outcome:?}"
    );

    // The request, in the form the shell hands the operator: a tool call
    // naming the member and the directory of this run.
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!("in_dump"));
    h.send(
        MessageBuilder::new(Path::new("/"))
            .hop(hop)
            .body(Body::Inline(json!({"messages": [{
                "origin": "assistant", "type": "tool_call", "id": "call-555",
                "text": to_string_pretty(&json!({
                    "target": format!("/members/{MEMBER}"), "export_to": RUN})).unwrap()}]})))
            .build(),
    )
    .await;

    for (hive, _) in HOLDERS {
        wait_for(
            &fence.join(RUN).join(hive).join("seed/export_final.json"),
            &format!("{hive}'s completeness marker"),
            &h,
        )
        .await;
    }
    let done = wait_done(&flags, 3, &h).await;

    // (1) the completion word comes from the holder, and says where
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
                "affinity".to_string(),
                format!("{RUN}/affinity/seed").to_string()
            ),
            ("firewall".to_string(), format!("{RUN}/firewall/seed")),
            ("memory-hive".to_string(), format!("{RUN}/memory-hive/seed")),
        ],
        "each holder says `export_done` for ITSELF, naming the directory \
         RELATIVE to its own fence -- a receipt travels further than the fence \
         does, so the host prefix stays behind"
    );

    // (2) one directory per holder, and the two `entities` tables are two files
    assert!(
        fence
            .join(RUN)
            .join("memory-hive/seed/entities.jsonl")
            .is_file(),
        "the memory hive's own entity table is missing"
    );
    assert!(
        fence
            .join(RUN)
            .join("affinity/seed/entities.jsonl")
            .is_file(),
        "one directory per holder is a requirement rather than tidiness: two \
         hives with a table of the same name would write one over the other"
    );
    assert!(fence.join(RUN).join("firewall/seed/rules.jsonl").is_file());
    assert!(
        !fence
            .join(RUN)
            .join("memory-hive/seed/emb_models.jsonl")
            .exists(),
        "which embedding generation is live is the RECEIVING hive's own \
         configuration and never travels -- the walk the porter names is what \
         bounds the export, not the whole database"
    );
    assert!(
        !fence
            .join(RUN)
            .join("firewall/seed/arrivals.jsonl")
            .exists(),
        "the rate window travelled. It is the budget THIS colony spent"
    );

    // (3) the seed format is the birth format: schema line first, one row per
    //     line after it
    let rules = read_lines(&fence.join(RUN).join("firewall/seed/rules.jsonl"));
    assert!(
        rules[0]["schema"].is_object(),
        "line 1 is the store's own declaration -- that is what makes this file \
         a seed: {:?}",
        rules[0]
    );
    assert!(
        rules.len() > 1,
        "the shipped firewall seeds example rules, so its table is not empty"
    );
    let marker: Value = from_str(
        &std::fs::read_to_string(fence.join(RUN).join("firewall/seed/export_final.json")).unwrap(),
    )
    .expect("the marker must be parseable JSON");
    assert_eq!(marker["format"], "meclaw-cell-export/1");
    assert_eq!(
        marker["cell"],
        format!("/members/{MEMBER}/firewall/rules"),
        "the marker names the CELL that wrote it -- the substrate knows no hives"
    );
    assert!(marker["rows"]["rules"].is_number());

    // (4) nothing died on the way
    let dl = h.drain_dead_letters().await;
    assert!(
        dl.is_empty(),
        "an export that dead-letters is the state this lane was built to end; \
         got {:?}",
        dl.iter()
            .map(|d| (
                d.sender_path.as_str().to_string(),
                d.resolved_target.as_str().to_string(),
                d.reason.as_code()
            ))
            .collect::<Vec<_>>()
    );
    h.shutdown().await;

    // (5) and the example reads back what this run wrote -- as a subprocess,
    //     so the claim is the shipped script's own
    let out = std::process::Command::new("python3")
        .arg(repo("examples/memory-import/build_import.py"))
        .arg("--export")
        .arg(fence.join(RUN))
        .arg("--templates")
        .arg(td.path().join("templates"))
        .arg("--scope")
        .arg("/")
        .arg("--name")
        .arg(MEMBER)
        .output()
        .expect("python3");
    assert!(
        out.status.success(),
        "build_import.py could not read what the slot wrote: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let manifest: Value = from_str(&String::from_utf8_lossy(&out.stdout)).expect("manifest");
    let files = &manifest["manifest"][0]["diff"]["add_templates"][0]["files"];
    for (hive, cell) in HOLDERS {
        assert!(
            files
                .as_object()
                .unwrap()
                .keys()
                .any(|k| k.starts_with(&format!("{hive}/{cell}/seed/"))),
            "the derived template carries no seed for {hive}: {:?}",
            files.as_object().unwrap().keys().collect::<Vec<_>>()
        );
    }
}

/// The other half of the completion word: what a holder says when its store
/// would NOT write.
///
/// The slot refuses by name — `transfer_io_error` when the fence is not there,
/// `transfer_path_out_of_bounds` when the path climbs out of it — and a porter
/// that swallowed that would leave a caller waiting for an `export_done` that
/// is never coming. It says `reject` with `hop.reject_reason ==
/// "export_write_failed"` instead, and the point of the name is what it does
/// NOT claim: no marker was written, so the directory is not a document, and a
/// prefix looks complete to whoever imports it.
///
/// Measured POSITIVELY on the refusal (a `reject` that HAD to arrive), and only
/// then on the silence: no `export_done`, and nothing on disk. A test that
/// asserted the silence alone would pass on a colony that never started.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_store_that_cannot_write_says_so_and_says_no_completion_word() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    let flags = td.path().join("flags");
    // The fence names a directory that does not exist and is never created.
    // That boots and validates cleanly — the fence is parsed, never
    // canonicalised — and costs a `transfer_io_error` at the first `to`.
    let fence = td.path().join("nowhere").join("deeper");
    assert!(!fence.exists());
    let h = boot(&td, &flags).await;
    let outcome = apply(&h, member_manifest(&fence)).await;
    assert!(
        outcome.is_committed(),
        "a fence that is not there yet must still commit and still boot — that \
         is the whole reason the parse is pure; got {outcome:?}"
    );

    let mut hop = Map::new();
    hop.insert("route".to_string(), json!("in_dump"));
    h.send(
        MessageBuilder::new(Path::new("/"))
            .hop(hop)
            .body(Body::Inline(json!({"messages": [{
                "origin": "assistant", "type": "tool_call", "id": "call-555-red",
                "text": to_string_pretty(&json!({
                    "target": format!("/members/{MEMBER}"), "export_to": RUN})).unwrap()}]})))
            .build(),
    )
    .await;

    // (1) the positive signal: three refusals, each naming the case.
    let rejects = wait_lane(&flags, "reject", 3, &h).await;
    let mut said: Vec<String> = rejects
        .iter()
        .map(|e| e["hop"]["reject_reason"].as_str().unwrap_or("").to_string())
        .collect();
    said.sort();
    assert_eq!(
        said,
        vec![
            "export_write_failed".to_string(),
            "export_write_failed".to_string(),
            "export_write_failed".to_string()
        ],
        "a store that would not write is a refusal with a NAME. Any other \
         reason here means the porter read the slot's answer as something it \
         is not: {rejects:?}"
    );

    // (2) and only then the silence: no completion word, and no directory.
    assert!(
        !flags.join("export_done.json").exists(),
        "a holder said `export_done` although nothing was written. That is the \
         one lie this lane must never tell: a reader trusts a directory whose \
         marker stands, and `export_done` is what tells it to look"
    );
    assert!(
        !fence.exists(),
        "the fence was created by the export. It is a boundary, not a \
         destination the substrate may bring into being"
    );

    let dl = h.drain_dead_letters().await;
    assert!(
        dl.is_empty(),
        "a refused export dead-lettered instead of leaving on `reject`; got {:?}",
        dl.iter()
            .map(|d| (
                d.sender_path.as_str().to_string(),
                d.resolved_target.as_str().to_string(),
                d.reason.as_code()
            ))
            .collect::<Vec<_>>()
    );
    h.shutdown().await;
}
