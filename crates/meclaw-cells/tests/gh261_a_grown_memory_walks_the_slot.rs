//! GH #261 — a memory that grew through this hive's own lanes leaves it as a
//! directory and comes back into an empty one, row for row.
//!
//! It replaces `gh243_a_memory_can_leave_a_hive_and_arrive_in_another.rs`, and
//! the replacement is the whole point of the issue. That file measured a
//! **mirror**: the porter carried a hand-kept copy of the store's schema, a
//! four-round-trip `scratch` probe for idempotence, a name list for provenance
//! and per-row inserts — four mechanisms beside the substrate's, each of them
//! doing the same job less well — and the test compared the copy against the
//! original. A drift lock on a mirror is the right test for a mirror and no test
//! at all for a transfer: it can be green while nothing moves.
//!
//! Since `memory-hive@3.3.0` there is no mirror. The porter names the sixteen
//! tables in the order they have to be applied, says what makes a row THE SAME
//! row in each of them, and hands both to the substrate's `transfer` slot. So
//! the test is the transfer: one hive is **grown** through the lanes this hive
//! ships — turns on `in_episode`, an extraction block on `in_remember` — and
//! then the same content is measured on the other side of a round trip.
//!
//! Four properties, one per way this could look finished and be wrong:
//!
//! * **The content is the same content.** Every one of the sixteen tables of the
//!   walk, compared row for row with every column, `audience_set` and `channel`
//!   among them — as DATA, not as presence. A transfer that quietly dropped a
//!   column would pass every count. And every one of the sixteen carries rows:
//!   `episodes` and `facts` grow through this hive's own write lanes,
//!   `embeddings` is queued by the `embed` cell behind them, and the eleven a
//!   nightly round would otherwise decide — the alias and rejected-pair
//!   families, `predicate_cardinality`, `entities`, `topics`, `entity_edges`,
//!   `beliefs`, `skills`, `consolidation_log` — arrive first through this same
//!   hive's transfer lane, out of a DIFFERENT directory than the one the round
//!   trip walks. The count is pinned per table, not as a total: a total is
//!   satisfied by one table that grew and fifteen that were empty on both
//!   sides, which is how `gh243` could be green while nothing moved.
//! * **One message each way.** The export leg has been one since #555; the
//!   import leg is one now. A walk that arrived as sixteen calls could not have
//!   the next property.
//! * **A broken document writes nothing at all** — not even the tables ahead of
//!   the broken one, and the refusal names the substrate's own code on the lane
//!   the caller already drains.
//! * **The same directory applied twice leaves the same state.**
//!
//! Guarded like every template-reading test (GH #49): a tree that does not carry
//! the library is skipped, never judged.

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

const RECV_TIMEOUT: Duration = Duration::from_secs(60);
/// The member whose memory grows, and the empty one it travels into.
const SRC: &str = "alex";
const DST: &str = "bea";
/// The directory of ONE run, named by the caller and passed through untouched.
const RUN: &str = "run-261";
/// The directory the tables no model can grow arrive in, before the run.
const SEED: &str = "seed-261";
/// The corpus this run grows through the hive's own lanes.
const TURNS: usize = 3;
const FACTS: usize = 2;
/// Entities the extraction block mints on its way through, measured on the
/// shipped ingress rather than assumed: it mints none — the two facts name
/// subjects, and an entity row is the nightly identity round's business.
const GROWN_ENTITIES: usize = 0;
const PROBE: &str = "gh261-probe";
const SESSION: &str = "0190a3f2-0000-7000-8000-000000000261";
const AUDIENCE: &str = r#"["member:alex","member:nora"]"#;
const CHANNEL: &str = "gh261-room";
const HAPPENED_AT: &str = "2026-09-05T09:00:00.000Z";

/// The walk the shipped porter names — read off the shipped config, so this
/// test cannot measure a walk of its own invention.
fn walk() -> Vec<String> {
    let cfg = shipped_config("templates/memory-hive/porter/config.json");
    let script = cfg["params"]["script_inline"]
        .as_str()
        .expect("the porter is a script cell");
    let start = script.find("WALK = [").expect("the porter names a WALK");
    let end = script[start..].find(']').expect("WALK is a list") + start;
    script[start + 7..=end]
        .replace('\n', " ")
        .trim_matches(|c| c == '[' || c == ']' || c == ' ')
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/member/config.json",
        "templates/operator/export/config.json",
        "templates/memory-hive/porter/config.json",
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

/// Every `${VAR}` the library references WITHOUT a default, bound to a dummy.
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

/// The nightly consolidation and the record's push tick, pushed to a date this
/// run cannot reach: a tick firing mid-run would emit into edges no test
/// topology drew.
fn quiet(name: &str, to: &str, id: &str) -> Value {
    json!({"schedules": [{
        "schedule_id": id,
        "schedule_name": name,
        "cron": "0 0 4 1 1 *",
        "emit_to": to,
        "emit_body": {"messages": [{"origin": "user", "type": "text", "text": name}]},
        "emit_headers": {}
    }]})
}

/// A code cell that appends every message it is handed to one file per lane, so
/// a wait can be a wait for something that HAD to arrive (GH #587: one
/// `O_APPEND` write, never a read-modify-write).
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

/// The generation's stand-in: three turns on `turn_write`, and — on a second
/// poke, once those turns are episodes — one extraction block on `sidecar`.
///
/// A real `talky` in its place would cost a provider and prove less: what is
/// under test is the transfer, and the two lanes are pinned where they are
/// produced (`gh527_the_member_writes_the_episode.rs`, `f8_inline_coverage.rs`).
fn probe_template() -> (Value, Value) {
    let script = format!(
        r#"
import sys, json
doc = json.load(sys.stdin)
grow = ((doc["envelope"].get("header") or {{}}).get("hop") or {{}}).get("grow")
out = []
if grow == "turns":
    for i in range(3):
        out.append({{
            "header": {{"route": "turn_write", "phase": "", "iter": "1",
                        "session_id": "{SESSION}", "turn_id": "{SESSION}#%d" % i,
                        "turn_index": str(i), "happened_at": "{HAPPENED_AT}"}},
            "messages": [{{"origin": "user", "type": "text",
                           "text": "turn number %d of a memory that grew" % i}}]}})
else:
    block = {{"facts": [
        {{"subject": "alex", "predicate": "favourite_colour", "claim": "blue",
          "fact_kind": "world", "confidence": 90}},
        {{"subject": "nora", "predicate": "works_at", "claim": "the observatory",
          "fact_kind": "world", "confidence": 80}}]}}
    out.append({{"header": {{"route": "sidecar", "section": "memory"}},
                 "messages": [{{"origin": "user", "type": "text",
                                "text": json.dumps(block)}}]}})
sys.stdout.write(json.dumps(out))
"#
    );
    (
        json!({
            "cell": {"type": "code"},
            "params": {"runner": "python3", "script_inline": script,
                       "external_timeout_ms": 10000,
                       "sandbox": {"trust": "trusted"}},
            "contract": {"version": "1.0.0", "settings": {}, "multi_send_capable": true,
                         "emits": {"body": {"messages": {"type": "array", "required": false}}},
                         "consumes": {"body": {"messages": {"type": "array", "required": false}}},
                         "capabilities": ["shell:exec"]},
            "description": {"purpose": "Test fixture: turns and one extraction block.",
                            "use_when": "Never outside this test.",
                            "not_in_scope": "Not a library template."}
        }),
        json!({"name": PROBE, "version": "1.0.0",
               "description": {"purpose": "Test fixture for GH #261.",
                               "use_when": "Never outside this test.",
                               "not_in_scope": "Not a library template.",
                               "contract_in": "A poke.",
                               "contract_out": "turn_write, or one sidecar section."}}),
    )
}

/// The shell: the shipped operator occupant as the export trigger, a members
/// container, and one flag cell that takes everything either of them raises.
async fn boot(td: &tempfile::TempDir, flag_dir: &std::path::Path) -> ColonyHandle {
    let root = td.path();
    copy_tree(&repo("templates"), &root.join("templates"));
    meclaw_testing::quiet_keeper_night(&root.join("templates/session-keeper"));
    let (probe_cfg, probe_tpl) = probe_template();
    write_json(
        &root.join(format!("templates/{PROBE}/config.json")),
        &probe_cfg,
    );
    write_json(
        &root.join(format!("templates/{PROBE}/template.json")),
        &probe_tpl,
    );
    std::fs::create_dir_all(flag_dir).unwrap();

    let mut edges = vec![
        json!({"from": ".", "to": "./trigger",
               "condition": "has(hop.route) && hop.route == 'in_dump'"}),
        json!({"from": "./trigger", "to": "./members",
               "condition": "has(hop.route) && hop.route == 'export'",
               "modifier": {"set_hop": {"route": "'in_export'"}}}),
        json!({"from": "./trigger", "to": "./flag",
               "condition": "has(hop.route) && hop.route == 'receipt'"}),
        json!({"from": ".", "to": "./members",
               "condition": "has(hop.route) && hop.route == 'in_import'"}),
        json!({"from": ".", "to": "./members",
               "condition": "has(hop.route) && hop.route == 'in_build_result'"}),
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

/// Two members from the shipped template, each with the one thing an instance
/// still has to say about files: the fence its stores write inside.
fn members_manifest(fence: &std::path::Path) -> Value {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for name in [SRC, DST] {
        let mut over = Map::new();
        over.insert(
            "memory-hive/clock".into(),
            quiet(
                "nightly-dream",
                "../dream-glue",
                "0190a3f2-0000-7000-8000-00000000dead",
            ),
        );
        over.insert(
            "affinity/clock".into(),
            quiet(
                "affinity-push",
                "../push",
                "0190a3f2-0000-7000-8000-00000000beef",
            ),
        );
        for (hive, cell) in [
            ("memory-hive", "store"),
            ("affinity", "store"),
            ("firewall", "rules"),
        ] {
            over.insert(
                format!("{hive}/{cell}"),
                json!({"transfer": {"base_path": fence.to_str().unwrap()}}),
            );
        }
        nodes.push(json!({"name": name, "template": "member@1.7.0",
                          "override_params": Value::Object(over)}));
        // The two members get the lane each of them needs and no more: one run
        // directory is named per export, and two members exporting into it at
        // once would write one holder's tables over another's.
        let lanes: &[&str] = if name == SRC {
            // The source takes `in_import` as well: the tables no model can
            // grow arrive through the hive's own transfer lane before the run
            // (see `seed_what_no_model_grows`).
            &["in_export", "in_import", "in_build_result"]
        } else {
            &["in_import"]
        };
        for lane in lanes {
            edges.push(json!({"from": ".", "to": format!("./{name}"),
                              "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
        }
        for lane in ["export_done", "dump", "reject", "error", "turn_write"] {
            edges.push(json!({"from": format!("./{name}"), "to": ".",
                              "condition": format!("has(hop.route) && hop.route == '{lane}'")}));
        }
    }
    json!({"manifest": [{"scope": "/members",
                         "diff": {"add_nodes": nodes, "add_edges": edges}}]})
}

/// The generation's stand-in inside the source member, and the two edges the
/// growing lanes cost. The round keys are promoted where a real channel
/// promotes them — on the way OUT of the generation — because a turn may not
/// assert its own audience (#244).
fn probe_manifest() -> Value {
    json!({"manifest": [{
        "scope": format!("/members/{SRC}/assistants"),
        "diff": {
            "add_nodes": [{"name": PROBE, "template": format!("{PROBE}@1.0.0")}],
            "add_edges": [
                {"from": ".", "to": format!("./{PROBE}"),
                 "condition": "has(hop.route) && hop.route == 'in_build_result'"},
                {"from": format!("./{PROBE}"), "to": ".",
                 "condition": "has(hop.route) && hop.route == 'turn_write'",
                 "modifier": {"set_context": {
                     "session_id": format!("'{SESSION}'"),
                     "audience_set": format!("'{AUDIENCE}'"),
                     "channel": format!("'{CHANNEL}'")}}},
                {"from": format!("./{PROBE}"), "to": ".",
                 "condition": "has(hop.route) && hop.route == 'sidecar'",
                 "modifier": {"set_context": {
                     "session_id": format!("'{SESSION}'"),
                     "audience_set": format!("'{AUDIENCE}'"),
                     "channel": format!("'{CHANNEL}'")}}},
            ],
        }
    }]})
}

async fn poke(h: &ColonyHandle, member: &str, route: &str, hop_extra: Vec<(&str, Value)>) {
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!(route));
    for (k, v) in hop_extra {
        hop.insert(k.to_string(), v);
    }
    h.send(
        MessageBuilder::new(Path::new(&format!("/members/{member}")))
            .hop(hop)
            .body(Body::Inline(json!({"messages": []})))
            .build(),
    )
    .await;
}

/// Every lane the recorder has seen, with what arrived on it — the diagnostic a
/// wait that ran out owes the reader.
fn lanes(flags: &std::path::Path) -> Vec<(String, Vec<Value>)> {
    let Ok(rd) = std::fs::read_dir(flags) else {
        return Vec::new();
    };
    rd.flatten()
        .map(|e| {
            (
                e.file_name().to_string_lossy().into_owned(),
                lane_entries(&e.path()),
            )
        })
        .collect()
}

fn lane_file(flags: &std::path::Path, lane: &str) -> std::path::PathBuf {
    flags.join(format!("{lane}.jsonl"))
}

fn lane_entries(p: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| from_str::<Value>(l).ok())
        .collect()
}

async fn wait_lane(
    flags: &std::path::Path,
    lane: &str,
    want: usize,
    h: &ColonyHandle,
) -> Vec<Value> {
    let p = lane_file(flags, lane);
    let deadline = std::time::Instant::now() + RECV_TIMEOUT;
    loop {
        let seen = lane_entries(&p);
        if seen.len() >= want {
            return seen;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "only {} of {want} on `{lane}` (last: {seen:?}) -- dead letters: {:?}",
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

fn store_db(root: &std::path::Path, member: &str) -> std::path::PathBuf {
    root.join("main/members")
        .join(member)
        .join("memory-hive/store/cell.db")
}

/// Every row of one table, every column, sorted — the whole content of the
/// table as a comparable value.
fn rows_of(db: &std::path::Path, table: &str) -> Vec<Vec<String>> {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap_or_else(|e| panic!("{}: {e}", db.display()));
    let cols: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info(?1) ORDER BY name")
        .unwrap()
        .query_map([table], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(!cols.is_empty(), "{table} does not exist in {db:?}");
    let sql = format!(
        "SELECT {} FROM \"{table}\"",
        cols.iter()
            .map(|c| format!("CAST(COALESCE(\"{c}\", '') AS TEXT)"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut st = conn.prepare(&sql).unwrap();
    let mut out: Vec<Vec<String>> = st
        .query_map([], |r| {
            (0..cols.len())
                .map(|i| r.get::<_, String>(i))
                .collect::<rusqlite::Result<Vec<String>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    out.sort();
    out
}

/// What each table of the walk has to hold on BOTH sides when the round trip is
/// done: three turns and two facts grown through this hive's own lanes, plus
/// the rows `what_no_model_grows` seeded, plus what the extraction block minted
/// on its way through (`entities`).
///
/// Pinned per table rather than as a total, because a total is satisfied by one
/// table that grew and fifteen that were empty on both sides — which is exactly
/// the shape of proof the `gh243` drift test gave and the reason it could be
/// green while nothing moved.
fn expected_counts() -> Vec<(String, usize)> {
    let seeded: std::collections::BTreeMap<&str, usize> = what_no_model_grows()
        .into_iter()
        .map(|(t, rows)| (t, rows.len()))
        .collect();
    walk()
        .into_iter()
        .map(|t| {
            let n = seeded.get(t.as_str()).copied().unwrap_or(0)
                + match t.as_str() {
                    "episodes" => TURNS,
                    "facts" => FACTS,
                    "entities" => GROWN_ENTITIES,
                    // One queued vector per episode and per fact. The `embed`
                    // cell writes the row when the content is written and fills
                    // it in when an embedder answers; without one it stays
                    // `pending` — which is a row, and a row that has to travel.
                    "embeddings" => TURNS + FACTS,
                    _ => 0,
                };
            (t, n)
        })
        .collect()
}

/// The column declaration of one table, read off the running store — the same
/// source the substrate's own export reads it from, so nothing here is a mirror
/// that could drift away from the schema it describes.
fn schema_of(db: &std::path::Path, table: &str) -> Value {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap_or_else(|e| panic!("{}: {e}", db.display()));
    let cols: Vec<(String, String)> = conn
        .prepare("SELECT name, COALESCE(type, '') FROM pragma_table_info(?1) ORDER BY cid")
        .unwrap()
        .query_map([table], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(!cols.is_empty(), "the running hive has no table {table}");
    let mut out = Map::new();
    for (name, decl) in cols {
        let kind = if decl.to_ascii_uppercase().starts_with("INT") {
            "int"
        } else {
            "text"
        };
        out.insert(name, json!(kind));
    }
    Value::Object(out)
}

/// The rows this run puts into each table of the walk that **no model can
/// grow**, keyed by table. Everything here is a decision a night or a judge
/// would have made; none of it needs one to exist, and a transfer that dropped
/// any of it would be a memory that lost what it had decided.
///
/// `embeddings` is deliberately absent HERE and still not empty in the store:
/// the `embed` cell queues one row per episode and per fact when the content is
/// written, and without an embedder to answer it the row stays `pending`. A
/// pending vector is a row like any other and has to travel, so the round trip
/// covers that table too — what a free test cannot measure is a FILLED vector,
/// and the private round-trip receipt of this issue does that on a real, grown
/// hive.
fn what_no_model_grows() -> Vec<(&'static str, Vec<Value>)> {
    let at = "2026-09-05T08:00:00.000000Z";
    vec![
        (
            "predicate_aliases",
            vec![
                json!({"alias": "works_for", "canonical": "employed_by", "recorded_at": at}),
                json!({"alias": "is_employed_at", "canonical": "employed_by",
                       "recorded_at": at}),
            ],
        ),
        (
            "subject_aliases",
            vec![json!({"alias": "nora m.", "canonical": "nora", "recorded_at": at})],
        ),
        (
            "claim_aliases",
            vec![
                json!({"alias": "the observatory", "canonical": "observatory",
                        "recorded_at": at}),
            ],
        ),
        (
            "predicate_rejected_pairs",
            vec![
                json!({"left_value": "employed_by", "right_value": "volunteers_at",
                        "recorded_at": at}),
            ],
        ),
        (
            "subject_rejected_pairs",
            vec![json!({"left_value": "nora", "right_value": "norah",
                        "recorded_at": at})],
        ),
        (
            "claim_rejected_pairs",
            vec![
                json!({"left_value": "observatory", "right_value": "laboratory",
                       "recorded_at": at}),
                json!({"left_value": "blue", "right_value": "blue-green",
                       "recorded_at": at}),
            ],
        ),
        (
            "predicate_cardinality",
            vec![
                json!({"canonical_predicate": "employed_by", "verdict": "functional",
                       "source": "judged", "decided_at": at}),
                json!({"canonical_predicate": "reads", "verdict": "enumerating",
                       "source": "judged", "decided_at": at}),
            ],
        ),
        (
            "entities",
            vec![
                json!({"id": "ent-nora", "canonical_name": "nora", "kind": "person",
                       "aliases": "[\"nora m.\"]"}),
                json!({"id": "ent-obs", "canonical_name": "observatory", "kind": "place",
                       "aliases": "[]"}),
                json!({"id": "ent-alex", "canonical_name": "alex", "kind": "person",
                       "aliases": "[]"}),
            ],
        ),
        (
            "topics",
            vec![
                json!({"id": "top-1", "session_id": SESSION, "channel": CHANNEL,
                        "audience_set": AUDIENCE, "name": "the observatory move",
                        "opened_episode_id": "", "closed_episode_id": "",
                        "opened_at": at, "closed_at": "", "closure_source": ""}),
            ],
        ),
        (
            "entity_edges",
            vec![
                json!({"id": "edge-1", "src_entity": "ent-nora", "dst_entity": "ent-obs",
                       "edge_kind": "works_at", "weight": 3, "episode_id": "",
                       "channel": CHANNEL, "audience_set": AUDIENCE,
                       "valid_from": at, "valid_until": ""}),
                json!({"id": "edge-2", "src_entity": "ent-alex", "dst_entity": "ent-nora",
                       "edge_kind": "knows", "weight": 1, "episode_id": "",
                       "channel": CHANNEL, "audience_set": AUDIENCE,
                       "valid_from": at, "valid_until": ""}),
            ],
        ),
        (
            "beliefs",
            vec![
                json!({"id": "bel-1", "holder": "alex", "observer": "alex",
                       "observed": "nora", "statement": "nora keeps early hours",
                       "confidence": 70, "active": 1, "source_fact_ids": "[]",
                       "audience_set": AUDIENCE, "created_at": at, "updated_at": at}),
                json!({"id": "bel-2", "holder": "alex", "observer": "alex",
                       "observed": "alex", "statement": "alex prefers the quiet room",
                       "confidence": 60, "active": 1, "source_fact_ids": "[]",
                       "audience_set": AUDIENCE, "created_at": at, "updated_at": at}),
            ],
        ),
        (
            "skills",
            vec![
                json!({"id": "skill-1", "trigger_pattern": "when the dome is open",
                        "procedure": "[]", "source_episode_ids": "[]",
                        "audience_set": AUDIENCE, "success_count": 2,
                        "last_used_at": at}),
            ],
        ),
        (
            "consolidation_log",
            vec![
                json!({"run_id": "night-1", "started_at": at, "finished_at": at,
                        "delta_from": at, "delta_to": at, "llm_calls": 0,
                        "tokens_prompt": 0, "tokens_completion": 0,
                        "verdicts": "{}", "status": "done"}),
            ],
        ),
    ]
}

/// Write the document that carries them, as the directory the slot reads: one
/// `<table>.jsonl` per table of the walk, schema header first, every table of
/// the walk present — an empty one included, because the call names the walk
/// and a missing file is a broken document.
fn write_seed_dir(dir: &std::path::Path, src_db: &std::path::Path) {
    std::fs::create_dir_all(dir).unwrap();
    let rows: std::collections::BTreeMap<&str, Vec<Value>> =
        what_no_model_grows().into_iter().collect();
    for table in walk() {
        let mut out =
            meclaw_core::serde_json::to_string(&json!({"schema": schema_of(src_db, &table)}))
                .unwrap();
        out.push('\n');
        for row in rows.get(table.as_str()).unwrap_or(&Vec::new()) {
            out.push_str(&meclaw_core::serde_json::to_string(row).unwrap());
            out.push('\n');
        }
        std::fs::write(dir.join(format!("{table}.jsonl")), out).unwrap();
    }
}

/// How many rows a table holds, or `None` while the lazy store has not woken and
/// created its tables yet. Only a wait may ask that question — the comparison
/// below asks `rows_of`, which insists the table is there.
fn maybe_count(db: &std::path::Path, table: &str) -> Option<usize> {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    conn.query_row(&format!("SELECT count(*) FROM \"{table}\""), [], |r| {
        r.get::<_, i64>(0)
    })
    .ok()
    .map(|n| n as usize)
}

/// Grow one member's memory through this hive's own lanes, then export it.
async fn grown(
    td: &tempfile::TempDir,
    flags: &std::path::Path,
    fence: &std::path::Path,
) -> ColonyHandle {
    let h = boot(td, flags).await;
    let outcome = apply(&h, members_manifest(fence)).await;
    assert!(
        outcome.is_committed(),
        "growing two shipped members must commit; got {outcome:?}"
    );
    let outcome = apply(&h, probe_manifest()).await;
    assert!(
        outcome.is_committed(),
        "wiring the generation stand-in must commit; got {outcome:?}"
    );

    poke(&h, SRC, "in_build_result", vec![("grow", json!("turns"))]).await;
    wait_lane(flags, "turn_write", 3, &h).await;
    // The store write is one hop behind the copy that left the level.
    let src_db = store_db(td.path(), SRC);
    let deadline = std::time::Instant::now() + RECV_TIMEOUT;
    while std::time::Instant::now() < deadline && maybe_count(&src_db, "episodes").unwrap_or(0) < 3
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        maybe_count(&src_db, "episodes").unwrap_or(0),
        3,
        "the three turns did not become episodes. lanes: {:?} dead letters: {:?}",
        lanes(flags),
        h.drain_dead_letters().await.len()
    );

    // What no model can grow arrives through the hive's own transfer lane,
    // before the run. It is the standard way a corpus is put in front of a
    // measurement in this repository (the scenario suite's `seed`), and it is a
    // DIFFERENT directory from the one the round trip below walks -- so nothing
    // here is the measurement reading back its own document.
    write_seed_dir(&fence.join(SEED).join("memory-hive/seed"), &src_db);
    poke(&h, SRC, "in_import", vec![("import_from", json!(SEED))]).await;
    let seeded = wait_lane(flags, "dump", 1, &h).await;
    assert_eq!(
        seeded[0]["hop"]["export_of"], 16,
        "the seed document has to cover the whole walk: {:?}",
        seeded[0]
    );

    poke(&h, SRC, "in_build_result", vec![("grow", json!("facts"))]).await;
    let deadline = std::time::Instant::now() + RECV_TIMEOUT;
    while std::time::Instant::now() < deadline && maybe_count(&src_db, "facts").unwrap_or(0) < 2 {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        maybe_count(&src_db, "facts").unwrap_or(0),
        2,
        "the extraction block did not become facts. lanes: {:?} dead letters: {:?}",
        lanes(flags),
        h.drain_dead_letters().await.len()
    );

    // The queued vectors are one hop behind the content that queued them: the
    // `embed` cell writes its row after the writer's, so an export that raced
    // it would carry a corpus this test cannot pin.
    let deadline = std::time::Instant::now() + RECV_TIMEOUT;
    while std::time::Instant::now() < deadline
        && maybe_count(&src_db, "embeddings").unwrap_or(0) < TURNS + FACTS
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // The request, in the form the shell hands the operator.
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!("in_dump"));
    h.send(
        MessageBuilder::new(Path::new("/"))
            .hop(hop)
            .body(Body::Inline(json!({"messages": [{
                "origin": "assistant", "type": "tool_call", "id": "call-261",
                "text": to_string_pretty(&json!({
                    "target": format!("/members/{SRC}"), "export_to": RUN})).unwrap()}]})))
            .build(),
    )
    .await;
    wait_lane(flags, "export_done", 3, &h).await;
    h
}

// ───────────────────────────────────────────────────────────────────── the run

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_grown_memory_arrives_in_an_empty_hive_row_for_row() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    let flags = td.path().join("flags");
    let fence = td.path().join("exports");
    std::fs::create_dir_all(&fence).unwrap();
    let h = grown(&td, &flags, &fence).await;

    // ONE message in. The receipt is the whole directory's, not one part's.
    poke(
        &h,
        DST,
        "in_import",
        vec![
            ("import_hive", json!("memory-hive")),
            ("import_from", json!(RUN)),
        ],
    )
    .await;
    let receipts = wait_lane(&flags, "dump", 1, &h).await;
    assert_eq!(
        receipts.len(),
        1,
        "the import leg is ONE message now: sixteen receipts would mean sixteen calls, \
         and sixteen calls cannot leave a broken document unapplied: {receipts:?}"
    );
    let hop = &receipts[0]["hop"];
    assert_eq!(hop["export_final"], "1");
    assert_eq!(
        hop["export_of"], 16,
        "the receipt counts the tables of the walk: {hop}"
    );
    assert!(
        hop["rows_written"].as_i64().unwrap_or(0) >= 5,
        "three episodes and two facts at the very least: {hop}"
    );

    // The same directory again: nothing new, and no refusal either.
    poke(
        &h,
        DST,
        "in_import",
        vec![
            ("import_hive", json!("memory-hive")),
            ("import_from", json!(RUN)),
        ],
    )
    .await;
    let again = wait_lane(&flags, "dump", 3, &h).await;
    assert_eq!(
        again[2]["hop"]["rows_written"], 0,
        "the same document applied twice must write nothing the second time: {:?}",
        again[2]
    );
    assert!(
        !lane_file(&flags, "reject").exists(),
        "nothing about this run may be refused: {:?}",
        lane_entries(&lane_file(&flags, "reject"))
    );

    let dl = h.drain_dead_letters().await;
    assert!(
        dl.is_empty(),
        "a transfer that dead-letters is the state this lane was built to end; got {:?}",
        dl.iter()
            .map(|d| (
                d.sender_path.as_str().to_string(),
                d.resolved_target.as_str().to_string(),
                d.reason.as_code()
            ))
            .collect::<Vec<_>>()
    );
    h.shutdown().await;

    // ── the content is the same content, row for row, every column
    let src_db = store_db(td.path(), SRC);
    let dst_db = store_db(td.path(), DST);
    let mut counted: Vec<(String, usize)> = Vec::new();
    for table in walk() {
        let left = rows_of(&src_db, &table);
        let right = rows_of(&dst_db, &table);
        assert_eq!(
            left, right,
            "table {table} did not survive the round trip row for row"
        );
        counted.push((table.clone(), left.len()));
    }
    // Per TABLE, not as a sum: a total is satisfied by one table that grew and
    // fifteen that were empty on both sides, which is the shape of proof this
    // test replaced. No table is left at zero: `embeddings` carries the
    // `pending` rows the embed cell queues per episode and per fact, which is
    // what a colony without an embedder can prove (see the module docs).
    assert_eq!(
        counted,
        expected_counts(),
        "the round trip moved a different corpus than this test grew and seeded"
    );

    // ── and provenance travelled as DATA, not as presence
    let conn =
        rusqlite::Connection::open_with_flags(&dst_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let audiences: Vec<String> = conn
        .prepare("SELECT audience_set FROM episodes")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(audiences.len(), 3);
    assert!(
        audiences.iter().all(|a| a.contains("nora")),
        "the participant set an episode was learned in front of did not arrive: {audiences:?}"
    );
    let channels: Vec<String> = conn
        .prepare("SELECT DISTINCT channel FROM episodes")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(channels, vec![CHANNEL.to_string()]);

    // ── the machine tables stayed behind: they are lane state, not memory
    for table in ["pending_extraction", "recall_scratch", "scratch"] {
        assert!(
            rows_of(&dst_db, table).is_empty(),
            "{table} travelled. It is the receiving hive's own work queue"
        );
    }
    assert!(
        !fence
            .join(RUN)
            .join("memory-hive/seed/emb_models.jsonl")
            .exists(),
        "which embedding generation is live is the RECEIVING hive's configuration"
    );
}

/// A document with one broken file writes nothing at all — not even the tables
/// ahead of the broken one — and says so on the lane the caller already drains,
/// carrying the substrate's own code.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_broken_document_is_refused_whole_and_names_the_substrates_code() {
    if !shipped() {
        return;
    }
    let td = tempfile::TempDir::new().unwrap();
    let flags = td.path().join("flags");
    let fence = td.path().join("exports");
    std::fs::create_dir_all(&fence).unwrap();
    let h = grown(&td, &flags, &fence).await;

    // `episodes` is the ninth table of the walk and it is whole; `facts`, the
    // eleventh, is not. A walk of sixteen calls would have applied the first
    // ten before it found out.
    let seed = fence.join(RUN).join("memory-hive/seed");
    assert!(seed.join("episodes.jsonl").is_file());
    std::fs::write(seed.join("facts.jsonl"), "{\"schema\": {}}\nnot json\n").unwrap();

    poke(
        &h,
        DST,
        "in_import",
        vec![
            ("import_hive", json!("memory-hive")),
            ("import_from", json!(RUN)),
        ],
    )
    .await;
    let rejects = wait_lane(&flags, "reject", 1, &h).await;
    let hop = &rejects[0]["hop"];
    assert_eq!(hop["reject_reason"], "import_failed");
    assert_eq!(
        hop["store_error"], "transfer_seed_malformed",
        "the substrate's own code rides beside the reason, because that list is OPEN \
         and a reason enum that grew with it would turn the next code into a failed \
         emit (GH #343): {hop}"
    );
    assert_eq!(
        lane_entries(&lane_file(&flags, "dump")).len(),
        1,
        "a refused import said it applied something -- the one receipt on this lane \
         is the source's own seed, from before the run"
    );
    h.shutdown().await;

    let dst_db = store_db(td.path(), DST);
    assert!(
        rows_of(&dst_db, "episodes").is_empty(),
        "the table AHEAD of the broken one landed. That is the whole difference \
         between one call and one call per table: the slot parses every file of the \
         directory before it writes the first row"
    );
}
