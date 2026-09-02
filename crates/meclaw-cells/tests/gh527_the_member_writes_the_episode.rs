//! GH #527 — the member consumes the one lane that fills its own `episodes`
//! table, so a conversation reaches the memory it is held in.
//!
//! `turn_write` is the only path in this substrate from a conversation into an
//! `episodes` table: `templates/collector/README.md` says so in the knob row,
//! `collector/assemble` says so beside `TURN_WRITE`, and GH #298 made it true by
//! removing everything else. **No shipped topology routed it anywhere.** It left
//! the collector, climbed nine hops unchanged and dead-lettered at the OS root
//! as `hive_no_route`, once per stored turn, for ever — while the collector
//! stamped every one of those turns `episode_written = 1`, so the sender
//! believed each was delivered and none was. Measured on two grown colonies of
//! the shipped shape, one of them live: the dead-letter count matched the
//! emitted count exactly, and `episodes` had not grown in nine days while the
//! colony answered questions daily.
//!
//! `templates/member/config.json` named the lane and declined it in one
//! sentence — *"the member's own episode path is `extraction` → the memory
//! hive's `in_remember`, so this lane crosses the boundary untouched"* — and
//! that sentence was wrong in both halves:
//!
//! * `in_remember` is the FACTS lane and it **presupposes** episodes. A block
//!   names no turn, so the ingress binds it to the newest `user` episode of the
//!   session; with no episode there is nothing to bind to and the hive refuses
//!   every block by design, which is the failure `templates/talky/README.md`
//!   states in one line.
//! * `write` → `in_close_pass` is no second writer either: nothing travels in
//!   its body, it names a session, and the pass reads the hive's OWN `episodes`
//!   — the table that was never filled.
//!
//! So the level that holds the memory (GH #122) declined the only lane that
//! fills it. The repair lands where the same container already hands `recall`,
//! `extraction` and `write` to the same hive: `./assistants -> ./memory-hive`,
//! re-stamped to `in_episode`. It is a **fan-out** and not a redirection — the
//! copy still leaves the level on `turn_write`, because a parent that wants an
//! archive of its own still gets one.
//!
//! What this file pins:
//!
//! 1. **The shipped shape**, off the file: the edge exists, it stamps
//!    `in_episode`, and it promotes `turn_id` **off the hop**. `context.turn_id`
//!    is a round uuid; `hop.turn_id` is the deterministic `<session_id>#<index>`
//!    the collector mints, and it is what the inline bind and the queue row are
//!    keyed on. An edge that promoted the context key would write episodes
//!    nothing can ever bind to — a defect that looks exactly like this one from
//!    the outside.
//! 2. **The lane arrives**, on a booted colony carrying the shipped
//!    `member@1.5.0`: one turn on `turn_write` out of the member's own
//!    `./assistants` becomes one `episodes` row in the member's own
//!    `memory-hive/store`, with the collector's `turn_id` on it, the caller's
//!    `happened_at` as the event time and TODAY as `recorded_at` — the
//!    bi-temporal split the writer performs, and the half that says the row was
//!    written now rather than imported.
//! 3. **The copy still leaves**, on the same run: the parent's drain sees the
//!    turn as well. One lane, two readers, both served.
//! 4. **The red probe**: the same colony with that one edge removed from the
//!    template lets the very same turn out of the level and writes NO episode —
//!    the state the whole shipped library was in until this issue, and the
//!    reason it was invisible: from above, a delivered lane and a lost memory
//!    look identical.
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
use meclaw_core::{Body, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

const MEMBER: &str = "alex";
const PROBE: &str = "probe527";
const SESSION: &str = "s-527";
const CHANNEL: &str = "tg:527";
const AUDIENCE: &str = r#"["member:alex","agent:egon"]"#;
const SAID: &str = "the roof beam is spruce, not oak";
const HAPPENED_AT: &str = "2026-08-29T09:15:00Z";
const DEADLINE: Duration = Duration::from_secs(30);
/// What an UNWIRED run waits before "no episode" is the answer. The wired run
/// needs about a second on this path; five is a wide margin under a full nextest.
const SETTLE: Duration = Duration::from_secs(5);

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> bool {
    [
        "templates/member/config.json",
        "templates/memory-hive/writer/config.json",
        "templates/collector/assemble/config.json",
        "examples/memory-import/build_import.py",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

fn read_json(p: &std::path::Path) -> Value {
    from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
        .expect("json")
}

fn arr(v: &Value) -> Vec<Value> {
    v.as_array().cloned().unwrap_or_default()
}

// ═══════════════════════════════════════════════════════ (1) the shipped shape

/// The edge off the file, and the one key whose SOURCE is the whole trap.
///
/// This is the drift lock GH #527 asked for in its own words — *a shipped
/// member's graph consumes the lane its own collector emits* — and it does both
/// halves § 2d demands: it greps the promise (the collector's knob row still
/// calls `turn_write` the only path into an episodes table) and it asserts the
/// mechanism (the member's graph carries the edge that receives it).
#[test]
fn the_shipped_member_consumes_the_only_lane_that_fills_an_episodes_table() {
    if !shipped() {
        return;
    }

    // The promise, on the public surface of the cell that emits the lane.
    let knob = std::fs::read_to_string(repo("templates/collector/README.md")).expect("readme");
    assert!(
        knob.contains("it is the only path from a conversation into an episodes table"),
        "`templates/collector/README.md` no longer claims `turn_write` is the only path \
         into an episodes table. If a second path was built, this lock is describing a \
         mechanism that moved; if the sentence was merely reworded, the reword has to come \
         here in the same commit (`docs/development-rules.md` § 2d)"
    );

    // The mechanism, in the graph of the level that holds the memory (GH #122).
    let member = read_json(&repo("templates/member/config.json"));
    let edges = arr(&member["params"]["graph"]["edges"]);
    let episode: Vec<&Value> = edges
        .iter()
        .filter(|e| {
            e["from"] == json!("./assistants")
                && e["to"] == json!("./memory-hive")
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("'turn_write'"))
        })
        .collect();
    assert_eq!(
        episode.len(),
        1,
        "the shipped member does not hand `turn_write` to its own memory hive. That was \
         the state of the whole library until GH #527: the lane left the collector, climbed \
         nine hops and dead-lettered at the OS root as `hive_no_route`, once per stored \
         turn, while `episodes` stood still"
    );
    let e = episode[0];
    assert_eq!(
        e["modifier"]["set_hop"]["route"],
        json!("'in_episode'"),
        "the edge does not re-stamp the lane to the hive's own ingress; the hive would \
         refuse it as an unknown route"
    );

    let ctx = &e["modifier"]["set_context"];
    let turn_id = ctx["turn_id"].as_str().unwrap_or_default();
    assert!(
        turn_id.contains("hop.turn_id") && !turn_id.contains("context.turn_id"),
        "`turn_id` must be promoted off the HOP: `context.turn_id` is a round uuid, \
         `hop.turn_id` is the deterministic `<session_id>#<index>` the collector mints, \
         and it is what the inline bind and the queue row are keyed on. An edge that \
         promotes the context key writes episodes nothing can ever bind to — a defect \
         that looks exactly like the missing edge from the outside. Got: {turn_id:?}"
    );
    for key in ["session_id", "audience_set", "channel", "happened_at"] {
        assert!(
            ctx[key].is_string(),
            "the edge does not promote `{key}`. The writer reads all four off the \
             context: the audience and the channel are the fail-closed gate of #244, \
             and `happened_at` is the event half of the bi-temporal split — absent, \
             every replayed turn is stamped with the writer's clock"
        );
    }

    // The copy still leaves the level: this is a fan-out, not a redirection.
    assert!(
        edges.iter().any(|e| e["from"] == json!("./assistants")
            && e["to"] == json!(".")
            && e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("'turn_write'"))),
        "the pass-through exit is gone. #527 added a reader, it did not take one away — \
         a parent that wires an archive of its own still gets the turn"
    );

    // The hive pairs the ingress with a refusal, and the level owes it a drain.
    let hive = read_json(&repo("templates/memory-hive/config.json"));
    assert!(
        arr(&hive["params"]["contract"]["accepts"])
            .iter()
            .any(|a| a["route"] == json!("in_episode")),
        "`memory-hive` no longer accepts `in_episode`"
    );
    assert!(
        arr(&hive["params"]["required_drains"])
            .iter()
            .any(|d| d["accepts"] == json!("in_episode") && d["emits"] == json!("reject")),
        "`memory-hive` no longer pairs `in_episode` with `reject`"
    );
    assert!(
        edges.iter().any(|e| e["from"] == json!("./memory-hive")
            && e["to"] == json!(".")
            && e["condition"]
                .as_str()
                .is_some_and(|c| c.contains("'reject'"))),
        "the member sends `in_episode` and does not carry the hive's refusal out — the \
         required_drains probe refuses the mutation, and a turn refused for a missing \
         audience would be silent"
    );
}

// ═══════════════════════════════════════════════════════════ the booted colony

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

/// Every `${VAR}` the library references without a default, bound to a dummy,
/// plus the crons that would otherwise fire into edges this topology never drew.
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
    out.push_str("AFFINITY_PUSH_CRON=0 0 4 1 1 *\n");
    out.push_str("MEMORY_DREAM_CRON=0 0 4 1 1 *\n");
    out.push_str("KEEPER_NIGHT_CRON=0 0 4 1 1 *\n");
    out
}

/// A stand-in for the generation: it takes one poke and hands out ONE message
/// on `turn_write`, in the shape `collector/assemble.turn_episode()` writes —
/// the deterministic `<session_id>#<index>` on `hop.turn_id`, the row's own
/// `recorded_at` on `hop.happened_at`, and one `user` text turn in the body,
/// because the hive's writer takes the first such turn and ignores the rest.
///
/// A real `talky` in its place would cost a provider and prove less: what is
/// under test is the member's edge, and the collector's own emission is pinned
/// where it is produced (`gh298_the_turn_writes_its_own_episode.rs`).
fn probe_template() -> (Value, Value) {
    let script = format!(
        r#"
import sys, json
json.load(sys.stdin)
sys.stdout.write(json.dumps({{
    "header": {{"route": "turn_write", "phase": "", "iter": "1",
                "session_id": "{SESSION}", "turn_id": "{SESSION}#0",
                "turn_index": "0", "happened_at": "{HAPPENED_AT}"}},
    "messages": [{{"origin": "user", "type": "text", "text": "{SAID}"}}]}}))
"#
    );
    (
        json!({
            "cell": {"type": "code"},
            "params": {"runner": "python3", "script_inline": script,
                       "external_timeout_ms": 10000,
                       "sandbox": {"trust": "trusted"}},
            "contract": {"version": "1.0.0", "settings": {},
                         "emits": {"body": {"messages": {"type": "array", "required": false}}},
                         "consumes": {"body": {"messages": {"type": "array", "required": false}}},
                         "capabilities": ["shell:exec"]},
            "description": {"purpose": "Test fixture: one poke, one turn_write.",
                            "use_when": "Never outside this test.",
                            "not_in_scope": "Not a library template."}
        }),
        json!({"name": PROBE, "version": "1.0.0",
               "description": {"purpose": "Test fixture for GH #527.",
                               "use_when": "Never outside this test.",
                               "not_in_scope": "Not a library template.",
                               "contract_in": "Any message.",
                               "contract_out": "One message on route turn_write."}}),
    )
}

/// A code cell that appends every message it is handed to one file per lane, so
/// "the copy still left the level" can be a wait for something that HAD to
/// arrive rather than a sleep.
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

/// The container wiring `examples/memory-import/build_import.py` writes — the
/// second generator of the builder's own table, read from the shipped script
/// rather than repeated here (GH #470).
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

/// Strips the one edge under test out of the copied template — the red probe,
/// which is the library as it shipped before GH #527.
fn remove_episode_edge(member_config: &std::path::Path) {
    let mut cfg = read_json(member_config);
    let kept: Vec<Value> = arr(&cfg["params"]["graph"]["edges"])
        .into_iter()
        .filter(|e| {
            !(e["from"] == json!("./assistants")
                && e["to"] == json!("./memory-hive")
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains("'turn_write'")))
        })
        .collect();
    cfg["params"]["graph"]["edges"] = json!(kept);
    write_json(member_config, &cfg);
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

fn rows(db: &std::path::Path, sql: &str) -> Vec<Vec<String>> {
    if !db.is_file() {
        return Vec::new();
    }
    let conn = rusqlite::Connection::open(db).expect("open cell.db");
    // The store mints its tables when it first wakes, so "no such table" is the
    // honest answer "nothing was written yet" and not a broken query.
    let Ok(mut st) = conn.prepare(sql) else {
        return Vec::new();
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

struct Run {
    episodes: Vec<Vec<String>>,
    left_the_level: bool,
    dead_letters: Vec<(String, String)>,
}

/// One colony, one member from the shipped template, one turn on `turn_write`.
///
/// `wired` decides whether the member's copy of the template still carries the
/// edge under test — everything else about the two runs is identical, which is
/// what makes the red probe a probe of that edge and of nothing else.
async fn one_turn(wired: bool) -> Run {
    let td = tempfile::TempDir::new().unwrap();
    let root = td.path();
    let flags = root.join("flags");
    std::fs::create_dir_all(&flags).unwrap();

    copy_tree(&repo("templates"), &root.join("templates"));
    if !wired {
        remove_episode_edge(&root.join("templates/member/config.json"));
    }
    let (probe_cfg, probe_tpl) = probe_template();
    write_json(
        &root.join(format!("templates/{PROBE}/config.json")),
        &probe_cfg,
    );
    write_json(
        &root.join(format!("templates/{PROBE}/template.json")),
        &probe_tpl,
    );

    // The shell: a members container, and one drain per lane the member emits.
    // Draining all eleven is the point — an undrained lane is a dead letter, and
    // the dead-letter assertion below would then be reading a silence.
    let mut edges = vec![json!({"from": ".", "to": "./members",
                                "condition": "has(hop.route) && hop.route == 'in_build_result'"})];
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
        &root.join("main/flag/config.json"),
        &flag_cell(flags.to_str().unwrap()),
    );
    std::fs::write(root.join(".env"), dummy_env(&root.join("templates"))).unwrap();

    let h = ColonyHandle::new_with_factories_at(&td, factories());
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

    let outcome = apply(
        &h,
        json!({"manifest": [{
            "scope": "/members",
            "diff": {
                "add_nodes": [{"name": MEMBER, "template": "member@1.5.0"}],
                "add_edges": container_edges(),
            }
        }]}),
    )
    .await;
    assert!(
        outcome.is_committed(),
        "growing the shipped member must commit; got {outcome:?}"
    );

    // The generation's stand-in, and the two edges an assistant costs on this
    // lane: one poke down, one turn up. The round keys are promoted where a
    // real channel promotes them — on the way OUT of the generation — because
    // a turn may not assert its own audience (#244).
    let outcome = apply(
        &h,
        json!({"manifest": [{
            "scope": format!("/members/{MEMBER}/assistants"),
            "diff": {
                "add_nodes": [{"name": PROBE, "template": format!("{PROBE}@1.0.0")}],
                "add_edges": [
                    {"from": ".", "to": format!("./{PROBE}"),
                     "condition": "has(hop.route) && hop.route == 'in_build_result'"},
                    {"from": format!("./{PROBE}"), "to": ".",
                     "condition": "has(hop.route) && hop.route == 'turn_write'",
                     "modifier": {"set_context": {
                         "audience_set": format!("'{AUDIENCE}'"),
                         "channel": format!("'{CHANNEL}'")}}}
                ],
            }
        }]}),
    )
    .await;
    assert!(
        outcome.is_committed(),
        "wiring the generation stand-in must commit; got {outcome:?}"
    );

    // One poke, through the member's own door and its own container edge.
    let mut hop = Map::new();
    hop.insert("route".to_string(), json!("in_build_result"));
    h.send(
        MessageBuilder::new(Path::new(&format!("/members/{MEMBER}")))
            .hop(hop)
            .body(Body::Inline(json!({"messages": []})))
            .build(),
    )
    .await;

    // The copy that leaves the level is the fast half of the fan-out and the
    // honest thing to wait on: it arrives whether or not the edge under test
    // exists, so both runs wait the same way and neither waits on its own
    // conclusion.
    let turn_flag = flags.join("turn_write.json");
    let deadline = std::time::Instant::now() + DEADLINE;
    while std::time::Instant::now() < deadline && !turn_flag.exists() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // The store write is one hop behind it. A wired run stops as soon as the row
    // is there; an unwired one has nothing to wait for, so it gets a settle
    // window instead of the full budget — long enough that "no row" is a
    // finding rather than an unfinished run.
    let db = root.join(format!("main/members/{MEMBER}/memory-hive/store/cell.db"));
    let deadline = std::time::Instant::now() + if wired { DEADLINE } else { SETTLE };
    while std::time::Instant::now() < deadline
        && rows(&db, "SELECT id FROM episodes").is_empty()
        && !flags.join("reject.json").exists()
    {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let run = Run {
        episodes: rows(
            &db,
            "SELECT turn_id, session_id, sender, channel, content, happened_at, recorded_at \
             FROM episodes ORDER BY id",
        ),
        left_the_level: turn_flag.exists(),
        dead_letters: h
            .drain_dead_letters()
            .await
            .iter()
            .map(|d| {
                (
                    d.sender_path.as_str().to_string(),
                    d.reason.as_code().to_string(),
                )
            })
            .collect(),
    };
    h.shutdown().await;
    run
}

// ══════════════════════════════════════ (2)+(3) the lane arrives, and it fans

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_turn_becomes_an_episode_in_the_memory_of_the_member_that_produced_it() {
    if !shipped() {
        return;
    }
    let run = one_turn(true).await;

    assert_eq!(
        run.episodes.len(),
        1,
        "one turn on `turn_write` did not become exactly one episode in the member's own \
         hive. Dead letters: {:?}",
        run.dead_letters
    );
    let row = &run.episodes[0];
    assert_eq!(
        row[0],
        format!("{SESSION}#0"),
        "the episode does not carry the collector's own turn id. `<session_id>#<index>` is \
         what the inline bind and the extraction queue are keyed on; a row minted from \
         `context.turn_id` carries a round uuid and nothing can bind to it later"
    );
    assert_eq!(row[1], SESSION, "the session did not travel");
    assert_eq!(row[2], "user", "the role of the speaker did not travel");
    assert_eq!(
        row[3], CHANNEL,
        "the channel did not travel — without it the writer refuses the turn (#244)"
    );
    assert_eq!(row[4], SAID, "the turn's own text did not travel");
    assert_eq!(
        row[5], HAPPENED_AT,
        "the event time was not taken from the caller. `happened_at` is promoted off the \
         hop by the member's edge and read off the context by the writer; without it every \
         turn is stamped with the writer's clock and a replay is indistinguishable from a \
         live turn"
    );
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    assert!(
        row[6].starts_with(&today),
        "`recorded_at` is the system half of the bi-temporal split and must be NOW: \
         expected a row recorded today ({today}), got {:?}",
        row[6]
    );

    assert!(
        run.left_the_level,
        "the copy did not leave the member. #527 is a FAN-OUT: the same turn is this \
         member's episode below and the parent's archive above, and both edges are regular \
         edges so both fire"
    );
    assert!(
        !run.dead_letters
            .iter()
            .any(|(_, code)| code.contains("no_route")),
        "a message dead-lettered as unroutable on the write path: {:?}",
        run.dead_letters
    );
}

/// The red probe: the library exactly as it shipped until GH #527.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_the_edge_the_turn_leaves_the_member_and_no_episode_is_written() {
    if !shipped() {
        return;
    }
    let run = one_turn(false).await;

    assert!(
        run.episodes.is_empty(),
        "the red probe wrote an episode with the edge removed — then something else in \
         this colony fills `episodes`, and the claim that `turn_write` is the only path \
         into that table is false: {:?}",
        run.episodes
    );
    assert!(
        run.left_the_level,
        "the probe measured nothing: the turn never even reached the member's rim, so the \
         green run's episode cannot be attributed to the edge"
    );
}
