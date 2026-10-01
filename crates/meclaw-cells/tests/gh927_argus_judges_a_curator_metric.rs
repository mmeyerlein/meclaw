//! GH #927 — the argus judges a goal measured at a curator, and a text change
//! reaches a cell only behind an evaluation batch.
//!
//! The shipped `argus` hive, booted, with everything outside it replaced by a
//! stub that answers the way the far side of its lanes is contracted to:
//!
//! ```text
//!   in_cycle -> meter -> charter -> receipts (overdue asks, applied cycle)
//!            -> stats  -> [stub curator]   -> in_stats -> meter -> receipts
//!            -> receipts (open hints) -> judge (a model, on a mock provider)
//!            -> mutator -> eval -> [stub evaluator] -> in_eval -> mutator
//!            -> slot_update -> [a capture at the target] -> probe -> receipts
//!            -> (the window passes) -> meter -> stats -> kept
//! ```
//!
//! The stub curator answers in exactly the form the curator's `in_stats` lane
//! is contracted to (wave plan § 2.2) — the real one is not needed to prove
//! the loop, and a real one would make this a test of two templates at once.
//! Every claim is read at a receiver: the requests the judge's provider saw,
//! the questions the stubs logged, the messages the capture at the target got,
//! and the receipts in the hive's own `receipts` store.
//!
//! Six cycles, one receipt each (the chain has no holes):
//!
//! 1. **Backed.** A text change of an allowed class, a passing batch of 40 that
//!    moves the metric the right way -> exactly one `slot_update` at the target,
//!    `applied` with its evidence and its way back, and after the window
//!    (better counts at the curator) `kept`. The one open hint was in the
//!    judge's request as a hypothesis and is consumed by this cycle. The
//!    window is anchored where the text LANDED, not where it was sent for
//!    evaluation: the applied row's `at` is later than the moment the
//!    evaluator saw the order (review finding I2 -- traffic before the change
//!    was applied is not its effect).
//! 2. **Too small a batch.** A batch of 10 -> `discarded` `no_evidence_10`,
//!    nothing applied.
//! 3. **Persona.** `proposed` `owner_approval_required`, no evaluation asked.
//! 4. **Identity.** `refused` `identity_out_of_radius`, and the mutator emits
//!    nothing for that cycle but its receipt.
//! 5. **Truncated counts.** `skipped` `stats_truncated`, and the judge is never
//!    asked.
//! 6. **The judge orders a revert.** Only the meter and the probe may order one,
//!    on the hive's `revert` lane; a judge answering `op: revert` with an
//!    identity text gets `refused` `judge_cannot_revert`, and nothing reaches
//!    the target (review finding C1). Ahead of it, a revert-shaped message on
//!    the hive's `in_eval` lane is dropped: that lane carries verdicts and
//!    nothing else (review finding C2).
//!
//! The fixtures that reach into the hive's `receipts` store (the hint, the
//! window moved back) write its own `cell.db` directly — the hive is sealed, a
//! test cell cannot reach `./receipts`, and no production path writes that way
//! (OR-BD-19, precedent `gh462_argus_runs_a_full_cycle.rs`).
//!
//! The table tests of the two scripts ride in the same binary (`gh927/`), so
//! the whole of #927 is one filter: `-E 'binary(~gh927)'`.

#[path = "mock_openai.rs"]
mod mock_openai;

#[path = "gh927/harness.rs"]
mod harness;
#[path = "gh927/meter.rs"]
mod meter;
#[path = "gh927/mutator.rs"]
mod mutator;

use meclaw_cells::TimerCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::llm::LlmCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::mock_http::MockResponse;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::MockOpenAI;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

// ═══════════════════════════════════════════════════════════════ the fixture

/// The one goal this test turns on: the gap rate of the `talky` role.
const GOAL: &str = "goal:talky-gap_rate";

/// The cell a text change is addressed to — a capture, so what arrives there
/// is read off the wire rather than trusted from a receipt.
const TARGET: &str = "/slotsink";
const SLOT: &str = "tool.search.description";
const TEXT_BEFORE: &str = "Search the web.";
const TEXT_BACKED: &str = "Search the web for facts you cannot answer from the conversation; \
                           name the question, not the topic.";
const TEXT_THIN: &str = "Search the web when unsure.";

/// The hint another part of the colony handed in, as a fixture row.
const HINT_ID: &str = "hint:0001";
const HINT_LINE: &str = "the search tool is called for questions the history already answers";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn shipped() -> Option<std::path::PathBuf> {
    let root = repo("templates/argus");
    root.join("config.json").is_file().then_some(root)
}

fn skip() -> bool {
    if shipped().is_none() {
        eprintln!("argus did not travel into this tree -- skipped (GH #49)");
        return true;
    }
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("no python3 -- skipped");
        return true;
    }
    false
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create the directory");
    for entry in std::fs::read_dir(src).expect("the source is readable") {
        let entry = entry.expect("directory entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy");
        }
    }
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("create the directory");
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

/// The judge's answer: one `argus_change` tool call, off the wire.
fn judge_says(args: Value) -> MockResponse {
    MockResponse::ok_json(
        json!({
            "id": "chatcmpl-judge",
            "model": "judge/thinker",
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": Value::Null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {"name": "argus_change", "arguments": args.to_string()}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 20, "completion_tokens": 10}
        })
        .to_string()
        .as_bytes(),
    )
}

/// A text change of one class, with the way back authored beforehand.
fn a_text_change(class: &str, key: &str, to: &str) -> Value {
    json!({
        "cycle_id": "",
        "action": "change",
        "reasoning": "one in five calls found a gap; the tool description invites calls the history answers",
        "simulated": {"gap_rate_expected": 15.0},
        "change": {"target": TARGET, "kind": "text_slot", "key": key, "class": class,
                   "from": TEXT_BEFORE, "to": to},
        "revert_plan": {"target": TARGET, "kind": "text_slot", "key": key, "to": TEXT_BEFORE}
    })
}

/// The stub curator: answers a `stats` ask in the contracted form, from a
/// scenario file the test rewrites between cycles, and logs every ask it saw.
fn curator_script(scenario: &std::path::Path, log: &std::path::Path) -> String {
    format!(
        concat!(
            "import sys, json\n",
            "doc = json.load(sys.stdin)\n",
            "hop = (doc['envelope'].get('header') or {{}}).get('hop') or {{}}\n",
            "with open({log}, 'a') as f:\n",
            "    f.write(json.dumps(hop, sort_keys=True) + '\\n')\n",
            "spec = json.load(open({scenario}))\n",
            "counts = spec.get('counts') or {{}}\n",
            "samples = [{{'at': '2026-01-01T00:00:00Z', 'kind': 'gap', 'session_id': 's-1',\n",
            "             'turn_id': 't-1', 'value': 'searched'}}]\n",
            "sys.stdout.write(json.dumps([{{\n",
            "    'header': {{'route': 'stats', 'stats_tag': str(hop.get('stats_tag') or ''),\n",
            "               'error_code': str(spec.get('error_code') or ''),\n",
            "               'detail': str(spec.get('detail') or '')}},\n",
            "    'messages': [],\n",
            "    'stats': {{'from': hop.get('stats_from'), 'to': hop.get('stats_to'),\n",
            "              'counts': counts, 'samples': samples,\n",
            "              'truncated': bool(spec.get('truncated'))}}}}]))\n"
        ),
        log = meclaw_core::serde_json::to_string(&log.display().to_string()).unwrap(),
        scenario = meclaw_core::serde_json::to_string(&scenario.display().to_string()).unwrap(),
    )
}

/// The stub evaluator: answers an `eval` order with the verdict the scenario
/// file names, and logs every order it saw.
fn evaluator_script(scenario: &std::path::Path, log: &std::path::Path) -> String {
    format!(
        concat!(
            "import sys, json, datetime\n",
            "doc = json.load(sys.stdin)\n",
            "hop = (doc['envelope'].get('header') or {{}}).get('hop') or {{}}\n",
            "order = doc['body'].get('eval') or {{}}\n",
            "seen_at = datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%dT%H:%M:%S.%fZ')\n",
            "with open({log}, 'a') as f:\n",
            "    f.write(json.dumps({{'hop': hop, 'eval': order, 'seen_at': seen_at}}, sort_keys=True) + '\\n')\n",
            "spec = json.load(open({scenario}))\n",
            "sys.stdout.write(json.dumps([{{\n",
            "    'header': {{'route': 'verdict', 'eval_tag': str(hop.get('eval_tag') or '')}},\n",
            "    'messages': [],\n",
            "    'eval': {{'verdict': spec['verdict'], 'batch_id': 'batch-' + str(order.get('cycle_id')),\n",
            "             'n': spec['n'], 'baseline': 20.0, 'candidate': 15.0,\n",
            "             'delta_pct': spec['delta_pct']}}}}]))\n"
        ),
        log = meclaw_core::serde_json::to_string(&log.display().to_string()).unwrap(),
        scenario = meclaw_core::serde_json::to_string(&scenario.display().to_string()).unwrap(),
    )
}

/// The driver: puts one message on the lane its hop names (as in gh462).
const DRIVER: &str = "import sys, json\n\
                      doc = json.load(sys.stdin)\n\
                      hop = (doc[\"envelope\"].get(\"header\") or {}).get(\"hop\") or {}\n\
                      route = str(hop.get(\"route\") or \"\")\n\
                      body = doc[\"body\"]\n\
                      sys.stdout.write(json.dumps([{\n\
                      \"header\": {\"route\": route},\n\
                      \"messages\": body.get(\"messages\") or []}]))\n";

fn code_cell(script: &str, purpose: &str, emits_body: Value) -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": script, "external_timeout_ms": 10000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "emits": {
                "body": emits_body,
                "hop": {"route": {"type": "string", "required": true}}
            }
        },
        "description": {"purpose": purpose, "use_when": "Once, in this test.", "not_in_scope": "Anything else."}
    })
}

/// The colony around the hive. Every lane that leaves the argus is drained:
/// an undrained lane is a dead letter, and a dead letter is what the probe
/// reads as an unhealthy colony.
fn main_config() -> Value {
    json!({
        "cell": {"type": "hive"},
        "params": {"graph": {"edges": [
            {"from": "./driver", "to": "./argus",
             "condition": "has(hop.route) && hop.route == 'in_cycle'"},
            {"from": "./driver", "to": "./argus",
             "condition": "has(hop.route) && hop.route == 'in_eval'"},
            {"from": "./argus", "to": "./curator",
             "condition": "has(hop.route) && hop.route == 'stats'"},
            {"from": "./curator", "to": "./argus",
             "condition": "has(hop.route) && hop.route == 'stats'",
             "modifier": {"set_hop": {"route": "'in_stats'"}}},
            {"from": "./argus", "to": "./evaluator",
             "condition": "has(hop.route) && hop.route == 'eval'"},
            {"from": "./evaluator", "to": "./argus",
             "condition": "has(hop.route) && hop.route == 'verdict'",
             "modifier": {"set_hop": {"route": "'in_eval'"}}},
            {"from": "./argus", "to": TARGET,
             "condition": "has(hop.route) && hop.route == 'slot_update'"},
            {"from": "./argus", "to": "/sink",
             "condition": "has(hop.route) && hop.route == 'mutate'"},
            {"from": "./argus", "to": "/sink",
             "condition": "has(hop.route) && hop.route == 'alert'"},
            {"from": "./argus", "to": "/sink",
             "condition": "has(hop.route) && hop.route == 'error'"}
        ]}}
    })
}

struct Files {
    curator_spec: std::path::PathBuf,
    curator_log: std::path::PathBuf,
    eval_spec: std::path::PathBuf,
    eval_log: std::path::PathBuf,
}

fn build_tree(td: &tempfile::TempDir, judge_url: &str) -> Files {
    let root = td.path();
    let stub = root.join("stub");
    std::fs::create_dir_all(&stub).expect("stub dir");
    let files = Files {
        curator_spec: stub.join("curator.json"),
        curator_log: stub.join("curator.log"),
        eval_spec: stub.join("evaluator.json"),
        eval_log: stub.join("evaluator.log"),
    };

    write_json(&root.join("main/config.json"), &main_config());
    write_json(
        &root.join("main/driver/config.json"),
        &code_cell(
            DRIVER,
            "Puts one message on the lane its hop names.",
            json!({"messages": {"type": "array", "required": false}}),
        ),
    );
    write_json(
        &root.join("main/curator/config.json"),
        &code_cell(
            &curator_script(&files.curator_spec, &files.curator_log),
            "Answers a stats ask in the contracted form, from a scenario file.",
            json!({"messages": {"type": "array", "required": false},
                   "stats": {"type": "object", "required": false}}),
        ),
    );
    write_json(
        &root.join("main/evaluator/config.json"),
        &code_cell(
            &evaluator_script(&files.eval_spec, &files.eval_log),
            "Answers an eval order with the verdict a scenario file names.",
            json!({"messages": {"type": "array", "required": false},
                   "eval": {"type": "object", "required": false}}),
        ),
    );
    copy_tree(&shipped().expect("guarded"), &root.join("main/argus"));

    // The clock is written out of the way (a boot-time literal, GH #138): the
    // cycles here are driven one at a time by `in_cycle`.
    let clock_path = root.join("main/argus/clock/config.json");
    let mut clock: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(&clock_path).expect("the shipped clock"),
    )
    .expect("json");
    clock["params"]["schedules"][0]["schedule_id"] = json!("01930000-0000-7000-8000-000000000927");
    clock["params"]["schedules"][0]["cron"] = json!("0 0 0 1 1 *");
    write_json(&clock_path, &clock);

    // One goal on, and it is a role goal. Everything else ships disabled.
    let goals_path = root.join("main/argus/charter/seed/goals.jsonl");
    let raw = std::fs::read_to_string(&goals_path).expect("the shipped goals");
    let mut lines = Vec::new();
    let mut found = false;
    for line in raw.lines() {
        let mut v: Value = meclaw_core::serde_json::from_str(line).expect("a seed row");
        if v.get("id").and_then(Value::as_str) == Some(GOAL) {
            v["enabled"] = json!(1);
            found = true;
        }
        lines.push(v.to_string());
    }
    assert!(found, "the charter ships the role goal {GOAL}");
    std::fs::write(&goals_path, lines.join("\n") + "\n").expect("write the goals seed");

    std::fs::write(
        root.join(".env"),
        format!(
            "OPENROUTER_API_KEY=test-key\n\
             ARGUS_JUDGE_BASE_URL={judge_url}\n\
             ARGUS_JUDGE_MODEL=judge/thinker\n\
             ARGUS_JUDGE_PROVIDER=openai\n"
        ),
    )
    .expect("write .env");
    files
}

fn curator_says(files: &Files, calls: i64, gaps: i64, tool_errors: i64, truncated: bool) {
    write_json(
        &files.curator_spec,
        &json!({"counts": {"calls": calls, "gap": gaps, "tool_error": tool_errors},
                "truncated": truncated}),
    );
}

fn evaluator_says(files: &Files, verdict: &str, n: i64, delta_pct: f64) {
    write_json(
        &files.eval_spec,
        &json!({"verdict": verdict, "n": n, "delta_pct": delta_pct}),
    );
}

fn logged(p: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| meclaw_core::serde_json::from_str(l).expect("a logged line is json"))
        .collect()
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

struct Booted {
    h: ColonyHandle,
    slots: mpsc::Receiver<meclaw_core::Message>,
}

async fn boot(td: &tempfile::TempDir) -> Booted {
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, mut sink_rx) = mpsc::channel::<meclaw_core::Message>(256);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    // Nothing is expected on `/sink`; it exists so a stray `error` or `alert`
    // is drained instead of dead-lettered. Kept open for the whole test.
    tokio::spawn(async move { while sink_rx.recv().await.is_some() {} });
    let (slot_tx, slot_rx) = mpsc::channel::<meclaw_core::Message>(256);
    h.spawn(Path::new(TARGET), move || CaptureCell::new(slot_tx.clone()))
        .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped argus must boot");
    Booted { h, slots: slot_rx }
}

async fn run_a_cycle(h: &ColonyHandle, why: &str) {
    drive(
        h,
        "in_cycle",
        json!([{"origin": "user", "type": "text", "text": why}]),
    )
    .await;
}

/// Puts `messages` on the hive lane `route`, through the driver.
async fn drive(h: &ColonyHandle, route: &str, messages: Value) {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!(route));
    h.send(
        MessageBuilder::new(Path::new("/driver"))
            .body(Body::Inline(json!({ "messages": messages })))
            .hop(hop)
            .ttl(200)
            .build(),
    )
    .await;
}

/// A revert order for `cycle_id` that puts an identity text back -- the form
/// the meter and the probe send on the hive's `revert` lane, here arriving
/// where it has no business.
fn a_revert_of_identity(cycle_id: &str) -> Value {
    json!({
        "op": "revert",
        "cycle_id": cycle_id,
        "plan": {"target": TARGET, "kind": "text_slot", "key": "identity.name",
                 "to": "Somebody else"}
    })
}

// ═══════════════════════════════════════════════════════ reading it back

fn receipts_db(td: &tempfile::TempDir) -> std::path::PathBuf {
    td.path().join("main/argus/receipts/cell.db")
}

fn cycles(td: &tempfile::TempDir) -> Vec<Value> {
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        receipts_db(td),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, goal, at, status, measured, judged, change, revert_plan, verified,
                effect, outcome, reason_code, role, evidence, hints
           FROM cycles ORDER BY at ASC",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map([], |r| {
        let field = |i: usize| -> String { r.get::<_, String>(i).unwrap_or_default() };
        let as_json =
            |s: String| -> Value { meclaw_core::serde_json::from_str(&s).unwrap_or(Value::Null) };
        Ok(json!({
            "id": field(0), "goal": field(1), "at": field(2), "status": field(3),
            "measured": as_json(field(4)), "judged": as_json(field(5)),
            "change": as_json(field(6)), "revert_plan": as_json(field(7)),
            "verified": as_json(field(8)), "effect": as_json(field(9)),
            "outcome": field(10), "reason_code": field(11), "role": field(12),
            "evidence": as_json(field(13)), "hints": as_json(field(14))
        }))
    });
    match rows {
        Ok(it) => it.filter_map(Result::ok).collect(),
        Err(_) => Vec::new(),
    }
}

fn scalar(db: &std::path::Path, sql: &str, id: &str) -> Option<String> {
    let conn =
        rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    conn.query_row(sql, [id], |r| r.get::<_, Option<String>>(0))
        .ok()
        .flatten()
}

fn open_waits(td: &tempfile::TempDir) -> i64 {
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        receipts_db(td),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return -1;
    };
    conn.query_row(
        "SELECT COUNT(*) FROM waits WHERE status = 'open'",
        [],
        |r| r.get(0),
    )
    .unwrap_or(-1)
}

/// What the mutator emitted for one cycle anywhere but its own receipts
/// store, as the colony logged it. Every order it can send beyond a receipt
/// (`eval`, `slot_update`, `mutate`, `probe`) carries `hop.cycle_id`.
fn mutator_emissions_for(td: &tempfile::TempDir, cycle_id: &str) -> i64 {
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        td.path().join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return -1;
    };
    conn.query_row(
        "SELECT COUNT(*) FROM message_log
          WHERE from_path LIKE '%/mutator'
            AND to_path NOT LIKE '%/receipts'
            AND json_extract(headers, '$.hop.cycle_id') = ?1",
        [cycle_id],
        |r| r.get(0),
    )
    .unwrap_or(-1)
}

async fn wait_for_row(td: &tempfile::TempDir, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let start = std::time::Instant::now();
    loop {
        let rows = cycles(td);
        if let Some(row) = rows.iter().find(|r| pred(r)) {
            return row.clone();
        }
        if start.elapsed() > Duration::from_secs(60) {
            panic!(
                "no receipt matching `{what}` after {:?}. The rows that were written:\n{}",
                start.elapsed(),
                meclaw_core::serde_json::to_string_pretty(&rows).unwrap_or_default()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Moves an applied row's `at` back past its window. The window is not waited
/// out (an hour at the least); that `at` is the moment of application, not of
/// the evaluation order, is asserted in cycle 1 before this runs.
fn the_window_has_passed(td: &tempfile::TempDir, cycle_id: &str) {
    let conn = rusqlite::Connection::open(receipts_db(td)).expect("the receipts db");
    let n = conn
        .execute(
            "UPDATE cycles SET at = ?1 WHERE id = ?2",
            rusqlite::params!["2020-01-01T00:00:00.000000Z", cycle_id],
        )
        .expect("move the applied row back");
    assert_eq!(n, 1, "exactly one row is moved: {cycle_id}");
}

/// One open hint, written the way the hive's door would have stored it.
///
/// A store is a lazy cell: it stays dormant, with no `cell.db` at all, until
/// its first message -- here the first tick's `waits` select, which is already
/// cycle 1. Waiting for the store to create `hints` therefore waited forever
/// (measured: 30 s, no table). So the file is created the way the store itself
/// creates it on wake -- `open_or_create_cell_db`, then the declared schema
/// through the store's own DDL, both idempotent (`CREATE TABLE IF NOT
/// EXISTS`) -- and the store finds it on wake as a resumed database, the case
/// its DDL already covers for a hand-written `cell.db` (precedent
/// `curator_cells.rs`).
fn a_hint_is_open(td: &tempfile::TempDir) {
    let config: Value = meclaw_core::serde_json::from_str(
        &std::fs::read_to_string(td.path().join("main/argus/receipts/config.json"))
            .expect("the receipts config"),
    )
    .expect("json");
    let schema: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>> =
        meclaw_core::serde_json::from_value(config["params"]["schema"].clone())
            .expect("the receipts schema");
    let conn =
        meclaw_colony::persist::open_or_create_cell_db(&receipts_db(td)).expect("the receipts db");
    meclaw_cells::store::ddl::apply_schema_ddl(&conn, &schema).expect("the receipts tables");
    let n = conn
        .execute(
            "INSERT INTO hints (id, at, origin, confidence, line, consumed_by)
             VALUES (?1, ?2, 'probe-origin', 70, ?3, '')",
            rusqlite::params![HINT_ID, "2026-01-01T00:00:00.000000Z", HINT_LINE],
        )
        .expect("the hint row");
    assert_eq!(n, 1, "exactly one hint is open");
}

fn judge_text(req: &mock_openai::OpenAiRequestSnapshot) -> String {
    req.body["messages"].to_string()
}

// ═══════════════════════════════════════════════════════════ the lock

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh927_argus_judges_a_curator_metric() {
    if skip() {
        return;
    }
    let judge = MockOpenAI::start(vec![
        judge_says(a_text_change("tool_description", SLOT, TEXT_BACKED)),
        judge_says(a_text_change("tool_description", SLOT, TEXT_THIN)),
        judge_says(a_text_change("persona", "persona.tone", "Be brisk.")),
        judge_says(a_text_change("identity", "identity.name", "Somebody else")),
        // Cycle 6: the judge answers with a revert order of its own. The mock
        // is loaded up front, so the id it names is one of its own making.
        judge_says(a_revert_of_identity("cycle:the-judge-made-this-up")),
    ])
    .await;
    let td = tempfile::tempdir().expect("a temporary directory");
    let files = build_tree(&td, &judge.base_url);
    let Booted { h, mut slots } = boot(&td).await;
    a_hint_is_open(&td);

    // ── CYCLE 1: backed ────────────────────────────────────────────────────
    // One call in five found a gap; the batch of 40 says the new text cuts
    // the rate by a quarter.
    curator_says(&files, 100, 20, 2, false);
    evaluator_says(&files, "pass", 40, -25.0);
    run_a_cycle(&h, "cycle 1").await;
    let applied = wait_for_row(&td, "an applied text change with a probe verdict", |r| {
        r["status"] == "applied" && r["verified"]["verdict"].is_string()
    })
    .await;
    let c1 = applied["id"].as_str().expect("an id").to_string();

    // The question reached the curator in the contracted form.
    let asks = logged(&files.curator_log);
    assert_eq!(asks.len(), 1, "one stats ask for one goal: {asks:?}");
    let ask = &asks[0];
    assert_eq!(ask["stats_role"], "talky", "{ask}");
    assert_eq!(
        meclaw_core::serde_json::from_str::<Value>(ask["stats_kinds"].as_str().unwrap_or("null"))
            .unwrap_or(Value::Null),
        json!(["calls", "gap", "tool_error"]),
        "calls, the metric's mark and the quality gate's mark: {ask}"
    );
    assert_eq!(ask["stats_samples"], 10, "{ask}");
    for key in ["stats_from", "stats_to"] {
        let t = ask[key].as_str().unwrap_or("");
        assert!(
            t.len() == 20 && t.ends_with('Z') && t.as_bytes()[10] == b'T',
            "{key} is RFC 3339 in UTC: {ask}"
        );
    }
    assert!(
        ask["stats_tag"]
            .as_str()
            .is_some_and(|t| t.starts_with("wait:")),
        "the tag is the id of the ask's wait row: {ask}"
    );

    // The judge saw the rate and the hint -- the hint as a hypothesis.
    let reqs = judge.recorded_requests().await;
    assert_eq!(reqs.len(), 1, "one judge request so far");
    let seen = judge_text(&reqs[0]);
    assert!(
        seen.contains(HINT_LINE),
        "the open hint is in the judge's input: {seen}"
    );
    assert!(
        seen.contains("hypotheses"),
        "and it is marked as a hypothesis, not a measurement: {seen}"
    );
    assert!(
        seen.contains("gap_rate"),
        "the rate the goal is about: {seen}"
    );
    assert_eq!(
        scalar(
            &receipts_db(&td),
            "SELECT consumed_by FROM hints WHERE id = ?1",
            HINT_ID
        )
        .as_deref(),
        Some(c1.as_str()),
        "the hint is consumed by the cycle whose judge was shown it"
    );

    // Evaluated before it moved: one order, with the batch floor.
    let orders = logged(&files.eval_log);
    assert_eq!(orders.len(), 1, "one evaluation order: {orders:?}");
    let order = &orders[0]["eval"];
    assert_eq!(order["cycle_id"], c1.as_str(), "{order}");
    assert_eq!(order["target"], TARGET, "{order}");
    assert_eq!(order["key"], SLOT, "{order}");
    assert_eq!(order["from"], TEXT_BEFORE, "{order}");
    assert_eq!(order["to"], TEXT_BACKED, "{order}");
    assert_eq!(
        order["batch_min"], 30,
        "max(eval_min_batch 30, min_samples 30): {order}"
    );
    // Review finding I2: the meter measures the effect window from the row's
    // `at`, so `at` has to be the moment the text LANDED. Written as the time
    // the order left, it would count everything between the order and the
    // verdict as the change's effect. The evaluator saw the order strictly
    // after it left and strictly before the verdict came back, so the applied
    // row being later than that moment is the whole claim.
    let seen_at = orders[0]["seen_at"]
        .as_str()
        .expect("the evaluator's clock");
    assert!(
        applied["at"].as_str().is_some_and(|at| at > seen_at),
        "the window starts when the text is applied, after the evaluator saw the order \
         ({seen_at}): {applied:#}"
    );

    // Exactly one slot_update, at the target, with the new text.
    let slot = tokio::time::timeout(Duration::from_secs(30), slots.recv())
        .await
        .expect("the text change reaches its target")
        .expect("the capture stays open");
    let hop = slot.headers.hop.clone();
    assert_eq!(hop.get("target"), Some(&json!(TARGET)), "{hop:?}");
    assert_eq!(hop.get("cycle_id"), Some(&json!(c1.as_str())), "{hop:?}");
    let body = match &slot.body {
        Body::Inline(v) => v.clone(),
        other => panic!("an inline body: {other:?}"),
    };
    assert_eq!(
        body["slot"],
        json!({"name": SLOT, "text": TEXT_BACKED}),
        "{body}"
    );

    // The receipt: evidence, the way back, the role, the hint it was shown.
    assert_eq!(applied["verified"]["verdict"], "healthy", "{applied:#}");
    assert_eq!(
        applied["verified"]["params_update_seen"], true,
        "{applied:#}"
    );
    assert_eq!(applied["goal"], GOAL, "{applied:#}");
    assert_eq!(applied["role"], "talky", "{applied:#}");
    assert_eq!(applied["change"]["kind"], "text_slot", "{applied:#}");
    assert_eq!(applied["revert_plan"]["to"], TEXT_BEFORE, "{applied:#}");
    assert_eq!(applied["evidence"]["verdict"], "pass", "{applied:#}");
    assert_eq!(applied["evidence"]["n"], 40, "{applied:#}");
    assert_eq!(applied["hints"], json!([HINT_ID]), "{applied:#}");
    assert_eq!(applied["measured"]["calls"], 100, "{applied:#}");
    assert_eq!(
        applied["measured"]["rates"]["gap_rate"], 20.0,
        "{applied:#}"
    );
    assert_eq!(cycles(&td).len(), 1, "one cycle, one receipt");

    // ── the window passes; the curator now counts half the gaps ────────────
    curator_says(&files, 100, 10, 2, false);
    the_window_has_passed(&td, &c1);
    run_a_cycle(&h, "the window has passed").await;
    let kept = wait_for_row(&td, "cycle 1 closed", |r| {
        r["id"] == c1.as_str() && r["status"] == "closed"
    })
    .await;
    assert_eq!(kept["outcome"], "kept", "{kept:#}");
    assert_eq!(kept["effect"]["before"], 20.0, "{kept:#}");
    assert_eq!(kept["effect"]["after"], 10.0, "{kept:#}");
    assert_eq!(
        logged(&files.curator_log).len(),
        2,
        "the effect was asked, too"
    );
    assert_eq!(
        cycles(&td).len(),
        1,
        "the effect completes the row, it adds none"
    );

    // ── CYCLE 2: a batch too small to count ────────────────────────────────
    curator_says(&files, 100, 20, 2, false);
    evaluator_says(&files, "pass", 10, -25.0);
    run_a_cycle(&h, "cycle 2").await;
    let thin = wait_for_row(&td, "a discarded text change", |r| {
        r["id"] != c1.as_str() && r["outcome"] == "discarded"
    })
    .await;
    assert_eq!(thin["status"], "closed", "{thin:#}");
    assert_eq!(thin["reason_code"], "no_evidence_10", "{thin:#}");
    assert_eq!(
        thin["evidence"]["n"], 10,
        "the batch it was judged on: {thin:#}"
    );
    assert_eq!(logged(&files.eval_log).len(), 2, "it was evaluated");
    assert_eq!(judge.recorded_requests().await.len(), 2);
    let second = judge_text(&judge.recorded_requests().await[1]);
    assert!(
        !second.contains(HINT_LINE),
        "a consumed hint is not shown twice: {second}"
    );
    assert_eq!(cycles(&td).len(), 2);

    // ── CYCLE 3: persona is the owner's ────────────────────────────────────
    run_a_cycle(&h, "cycle 3").await;
    let persona = wait_for_row(&td, "a proposed persona change", |r| {
        r["outcome"] == "proposed"
    })
    .await;
    assert_eq!(
        persona["reason_code"], "owner_approval_required",
        "{persona:#}"
    );
    assert_eq!(persona["status"], "closed", "{persona:#}");
    assert_eq!(cycles(&td).len(), 3);

    // ── CYCLE 4: identity is out of reach ──────────────────────────────────
    run_a_cycle(&h, "cycle 4").await;
    let identity = wait_for_row(&td, "a refused identity change", |r| {
        r["outcome"] == "refused"
    })
    .await;
    assert_eq!(
        identity["reason_code"], "identity_out_of_radius",
        "{identity:#}"
    );
    assert_eq!(cycles(&td).len(), 4);
    assert_eq!(
        logged(&files.eval_log).len(),
        2,
        "neither the persona nor the identity change was sent for evaluation"
    );

    // ── CYCLE 5: the curator cut its count short ───────────────────────────
    curator_says(&files, 100, 20, 2, true);
    run_a_cycle(&h, "cycle 5").await;
    let cut = wait_for_row(&td, "a skipped cycle on a truncated count", |r| {
        r["outcome"] == "skipped"
    })
    .await;
    assert_eq!(cut["reason_code"], "stats_truncated", "{cut:#}");
    assert_eq!(cut["role"], "talky", "{cut:#}");
    // Baseline and effect of cycle 1, then one baseline each for cycles 2, 3,
    // 4 and this one: the persona and identity cycles are measured before
    // their judge is asked, like every other.
    assert_eq!(logged(&files.curator_log).len(), 6, "it was asked");
    assert_eq!(
        judge.recorded_requests().await.len(),
        4,
        "a truncated count is never put before the judge"
    );
    assert_eq!(cycles(&td).len(), 5, "five cycles, five receipts");

    // ── CYCLE 6: a revert only the meter or the probe may order ────────────
    // First a revert-shaped message on the hive's verdict lane. Nothing
    // answers it: `in_eval` takes an evaluation verdict and nothing else.
    // It is routed into the mutator's mailbox before anything of cycle 6 can
    // reach it (the colony routes the driver's two emissions in order, and
    // this one is one step from the mutator), so once cycle 6's receipt is
    // written, the stray message has been handled.
    drive(
        &h,
        "in_eval",
        json!([{"origin": "assistant", "type": "tool_call", "id": "stray",
                "text": a_revert_of_identity(&c1).to_string()}]),
    )
    .await;
    curator_says(&files, 100, 20, 2, false);
    run_a_cycle(&h, "cycle 6").await;
    let judged_revert = wait_for_row(&td, "a refused revert from the judge", |r| {
        r["reason_code"] == "judge_cannot_revert"
    })
    .await;
    assert_eq!(judged_revert["outcome"], "refused", "{judged_revert:#}");
    assert_eq!(judged_revert["status"], "closed", "{judged_revert:#}");
    assert_ne!(
        judged_revert["id"],
        c1.as_str(),
        "the refusal is a row of its own: {judged_revert:#}"
    );
    assert_eq!(judge.recorded_requests().await.len(), 5);
    let rows = cycles(&td);
    assert_eq!(rows.len(), 6, "six cycles, six receipts: {rows:#?}");
    let first = rows
        .iter()
        .find(|r| r["id"] == c1.as_str())
        .expect("cycle 1 is still there");
    assert_eq!(
        first["outcome"], "kept",
        "neither the judge nor a stray verdict moved cycle 1: {first:#}"
    );
    let refused_id = judged_revert["id"].as_str().expect("an id");
    assert_eq!(
        mutator_emissions_for(&td, refused_id),
        0,
        "the judge's revert left the mutator as its receipt and nothing else"
    );

    // ── what did NOT happen ────────────────────────────────────────────────
    assert!(
        slots.try_recv().is_err(),
        "one text change was backed, and one slot_update is all the target got -- \
         neither the judge's revert nor the stray one on `in_eval` reached it"
    );
    for (what, row) in [("persona", &persona), ("identity", &identity)] {
        let id = row["id"].as_str().expect("an id");
        assert_eq!(
            mutator_emissions_for(&td, id),
            0,
            "the {what} cycle left the mutator as its receipt and nothing else"
        );
    }
    assert_eq!(open_waits(&td), 0, "every ask was answered and closed");
    assert_eq!(logged(&files.eval_log).len(), 2);
}
