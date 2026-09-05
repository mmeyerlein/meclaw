//! GH #462 — one full cycle of the shipped `argus` hive, on a BOOTED colony.
//!
//! Every hop of this loop was tested before this file existed, and none of the
//! joins between them were: the scripts were fed on stdin, the ledger answers
//! were fixtures, and the context that travels over the hive's own edges was
//! asserted nowhere. That is the shape of gap this file closes. The chain it
//! drives is the whole one —
//!
//! ```text
//!   in_cycle -> meter -> charter -> /colony/ledger -> meter
//!            -> judge (a model, on a mock provider)
//!            -> mutator -> the target cell's OWN cell.db
//!            -> probe -> /colony/ledger -> receipts
//!            -> (the window passes) -> meter -> kept | reverted
//! ```
//!
//! — with real cells, real edges, a real `colony.db` behind `/colony/ledger`,
//! and a real `llm` cell at the far end whose `params` table is read off disk
//! at the finish. Nothing in the middle is doubled except the provider.
//!
//! # What the joins hid
//!
//! Four defects were only reachable from here, and all four are fixed in the
//! same change as this test:
//!
//! 1. **`ar_measured` / `ar_require_plan` were read and never set.** The
//!    mutator reads both out of `context`; no edge in the hive put either
//!    there. Every `applied` receipt this loop has ever written recorded
//!    `measured: {}`, and the charter's own revert-plan rule was enforced off a
//!    compiled-in default instead of off the charter row. A stdin test cannot
//!    see that, because a stdin test supplies the context itself.
//! 2. **The receipts store had no way back to the mutator.** `./mutator ->
//!    ./receipts` promotes `argus_origin: 'mutator'` and the return edges only
//!    matched `'meter'` and `'probe'`, so the store's answer to every write the
//!    mutator made DEAD-LETTERED — on the happy path, on every applied cycle.
//!    And a dead letter is exactly what the probe two hops later reads as an
//!    unhealthy colony, so the loop's own bookkeeping reverted its own changes.
//! 3. **`./probe -> .` on `error` had no emitter.** Declared from the first
//!    version of this hive, wired as an edge, raised by nothing.
//! 4. **`./mutator -> .` on `error` had no emitter either**, and unlike (3) it
//!    had no case to raise it: everything that can go wrong in the mutator is a
//!    recorded OUTCOME. The edge and the declaration are gone rather than
//!    filled.
//!
//! # Why the window is moved rather than waited out
//!
//! A goal's `window_minutes` is both the span the ledger is asked about and the
//! age an applied cycle must reach before its effect is judged. The test needs
//! the first to be wide (so the traffic it made is inside it) and the second to
//! be past. Those are the same number, so the only honest lever is the clock:
//! the applied row's `at` is moved back in the receipts store's own `cell.db`,
//! which is a test fixture saying "the window has passed" and nothing else. No
//! production path writes that column twice.
//!
//! Guarded like every other template-reading test (GH #49): a checkout without
//! the template skips rather than failing on a dead reference.

#[path = "mock_openai.rs"]
mod mock_openai;

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

/// The model the target cell is born with, the one the first cycle moves it to,
/// and the one the second cycle tries and has to take back.
const MODEL_BIRTH: &str = "target/model-a";
const MODEL_KEPT: &str = "target/model-b";
const MODEL_REVERTED: &str = "target/model-c";

/// The judge's model. Deliberately absent from the charter's price row, so the
/// judge's own token spend prices at zero and the measured cost is the TARGET's
/// cost alone. Otherwise asking the judge would move the number the judge is
/// being asked about.
const MODEL_JUDGE: &str = "judge/thinker";

/// One target answer costs exactly 1.0 with the price row below: a million
/// prompt tokens at 1.0 per million, no completion tokens at 0.0.
const PROMPT_TOKENS_PER_ANSWER: u64 = 1_000_000;
const PRICE_ROW: &str = "target/model-a=1.0/0.0,target/model-b=1.0/0.0,target/model-c=1.0/0.0";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The shipped template, or `None` where it did not travel (GH #49 / R2b).
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

fn write_json(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("create the directory");
    write_json_at(&p, v);
}

/// The same write, to a path the caller already holds.
fn write_json_at(p: &std::path::Path, v: &Value) {
    std::fs::write(
        p,
        meclaw_core::serde_json::to_string_pretty(v).expect("serialise"),
    )
    .expect("write");
}

/// A canned provider answer that names its own model, because the ledger reads
/// the model out of the ANSWER (`translated.model`) and the whole measurement
/// hangs on telling the judge's calls from the target's.
fn answer(model: &str, prompt_tokens: u64, content: &str) -> MockResponse {
    MockResponse::ok_json(
        json!({
            "id": "chatcmpl-target",
            "model": model,
            "choices": [{
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": prompt_tokens, "completion_tokens": 0}
        })
        .to_string()
        .as_bytes(),
    )
}

/// The judge's answer: one `argus_change` tool call and nothing else, which is
/// what its shipped charter demands of it.
fn judge_says(args: Value) -> MockResponse {
    MockResponse::ok_json(
        json!({
            "id": "chatcmpl-judge",
            "model": MODEL_JUDGE,
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": Value::Null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "argus_change",
                            "arguments": args.to_string()
                        }
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

/// A model swap the mutator will accept: inside the radius, absolute target,
/// and a revert plan that really restores the original.
fn a_model_swap(from: &str, to: &str) -> Value {
    json!({
        "cycle_id": "",
        "action": "change",
        "reasoning": "the numbers say the cheaper model carries this traffic",
        "simulated": {"counterfactual_cost_eur": 0.0},
        "change": {"target": "/target", "kind": "model", "from": from, "to": to},
        "revert_plan": {"target": "/target", "kind": "model", "to": from}
    })
}

/// How the charter is set up for one test. The shipped seed rows are edited in
/// the COPY under the test's own templates directory, never in the tree.
struct Charter {
    cost_goal_enabled: bool,
    dlq_goal_enabled: bool,
    /// Drop a column the meter selects, so the charter store refuses the read.
    break_the_schema: bool,
}

impl Charter {
    fn inert() -> Self {
        Self {
            cost_goal_enabled: false,
            dlq_goal_enabled: false,
            break_the_schema: false,
        }
    }
    fn cost() -> Self {
        Self {
            cost_goal_enabled: true,
            ..Self::inert()
        }
    }
    fn dlq() -> Self {
        Self {
            dlq_goal_enabled: true,
            ..Self::inert()
        }
    }
    fn broken() -> Self {
        Self {
            break_the_schema: true,
            ..Self::inert()
        }
    }
}

/// The driver: one `code` cell that puts a message on the lane its hop names.
/// It exists so the `in_cycle` lane is entered the way a colony enters it — over
/// an edge from a sibling — rather than by an injection that skips the edge.
const DRIVER: &str = "import sys, json\n\
                      doc = json.load(sys.stdin)\n\
                      hop = (doc[\"envelope\"].get(\"header\") or {}).get(\"hop\") or {}\n\
                      route = str(hop.get(\"route\") or \"\")\n\
                      body = doc[\"body\"]\n\
                      sys.stdout.write(json.dumps([{\n\
                      \"header\": {\"route\": route},\n\
                      \"messages\": body.get(\"messages\") or []}]))\n";

/// The colony around the hive: a driver, the argus hive, one real `llm` cell as
/// the target of its changes, and a capture for every lane that leaves.
///
/// Draining all three outbound lanes matters: an undrained lane is a dead
/// letter, a dead letter is what the probe reads as an unhealthy colony, and
/// this test would then be measuring its own topology instead of the loop.
fn main_config() -> Value {
    json!({
        "cell": {"type": "hive"},
        "params": {"graph": {"edges": [
            {"from": "./driver", "to": "./argus",
             "condition": "has(hop.route) && hop.route == 'in_cycle'"},
            {"from": "./driver", "to": "./target",
             "condition": "has(hop.route) && hop.route == 'in_turn'"},
            {"from": "./argus", "to": "./target",
             "condition": "has(hop.route) && hop.route == 'mutate'"},
            {"from": "./argus", "to": "/sink",
             "condition": "has(hop.route) && hop.route == 'alert'"},
            {"from": "./argus", "to": "/sink",
             "condition": "has(hop.route) && hop.route == 'error'"},
            {"from": "./target", "to": "/sink", "condition": "true"}
        ]}}
    })
}

fn target_config(base_url: &str) -> Value {
    json!({
        "cell": {"type": "llm"},
        "params": {
            "provider": "openai",
            "base_url": base_url,
            "model": MODEL_BIRTH,
            "api_key": "${OPENROUTER_API_KEY}",
            "external_timeout_ms": 20000,
            "max_tokens": 64
        },
        "contract": {
            "version": "1.0.0",
            "settings": {},
            // `messages` is NOT required, and that is a fact about any cell a
            // control loop may reach: a params update is a body with `params`
            // and an empty `system` and no turns at all, so a target that
            // demands turns answers every change with `consumes_violation`.
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {
                    "model": {"type": "string", "required": false},
                    "finish_reason": {"type": "string", "required": false}
                }
            }
        },
        "description": {
            "purpose": "The cell the loop is allowed to change. A real llm cell, because the promise under test is that a params update lands in a RUNNING cell's own cell.db.",
            "use_when": "Once, in this test.",
            "not_in_scope": "Nothing else."
        }
    })
}

/// Build the tree: the shipped template copied in, its charter seed edited for
/// the case under test, and a `.env` naming the two mock providers.
fn build_tree(td: &tempfile::TempDir, judge_url: &str, target_url: &str, charter: Charter) {
    let root = td.path();
    let template = shipped().expect("guarded by the caller");

    write_json(root, "main/config.json", &main_config());
    write_json(
        root,
        "main/driver/config.json",
        &json!({
            "cell": {"type": "code"},
            "params": {"runner": "python3", "script_inline": DRIVER,
                       "external_timeout_ms": 10000},
            "contract": {
                "version": "1.0.0",
                "settings": {},
                "consumes": {"body": {"messages": {"type": "array", "required": false}}},
                "emits": {
                    "body": {"messages": {"type": "array", "required": false}},
                    "hop": {"route": {"type": "string", "required": true}}
                }
            },
            "description": {
                "purpose": "Puts one message on the lane its hop names.",
                "use_when": "Once, in this test.",
                "not_in_scope": "It decides nothing."
            }
        }),
    );
    write_json(root, "main/target/config.json", &target_config(target_url));
    copy_tree(&template, &root.join("main/argus"));

    // `${uuid7:…}` is an INSTANTIATION-time substitution, resolved by the
    // mutation that grows a template. This test boots the template directly, so
    // the id is written out here — the same value a growth would have minted,
    // just minted by the fixture instead.
    //
    // The cron is written out for a different reason: since GH #138 the
    // schedule is a LITERAL inside `params.schedules[0].cron`, so a case that
    // wants the cycle driven by hand rather than by the six-hourly tick says so
    // where a mutation's `override_params` would have merged it. It used to be
    // an `ARGUS_CYCLE_CRON` line in the `.env` below, and after the migration
    // that line would have reached nothing at all — this case would have kept
    // the shipped six-hour tick with nothing saying so.
    let clock_path = root.join("main/argus/clock/config.json");
    let raw = std::fs::read_to_string(&clock_path).expect("the shipped clock is on disk");
    let mut clock: Value =
        meclaw_core::serde_json::from_str(&raw).expect("the shipped clock is json");
    clock["params"]["schedules"][0]["schedule_id"] = json!("01930000-0000-7000-8000-000000000462");
    clock["params"]["schedules"][0]["cron"] = json!("0 0 0 1 1 *");
    write_json_at(&clock_path, &clock);

    // The charter, as this case wants it. Every edit is to the COPY.
    let goals_path = root.join("main/argus/charter/seed/goals.jsonl");
    let raw = std::fs::read_to_string(&goals_path).expect("the shipped charter seeds its goals");
    let mut lines: Vec<String> = Vec::new();
    for line in raw.lines() {
        let mut v: Value = meclaw_core::serde_json::from_str(line).expect("a seed row is json");
        match v.get("id").and_then(|x| x.as_str()) {
            Some("goal:llm-cost") => {
                v["enabled"] = json!(if charter.cost_goal_enabled { 1 } else { 0 });
                // Wide enough that everything this test made is inside it, and
                // it is the same number the effect gate uses -- which is why
                // that gate is stepped over by moving the row's `at`.
                v["window_minutes"] = json!(1440);
                v["min_samples"] = json!(2);
                v["min_delta_pct"] = json!(0);
            }
            Some("goal:dlq-watch") => {
                v["enabled"] = json!(if charter.dlq_goal_enabled { 1 } else { 0 });
            }
            _ => {}
        }
        lines.push(v.to_string());
    }
    std::fs::write(&goals_path, lines.join("\n") + "\n").expect("write the goals seed");

    let rules_path = root.join("main/argus/charter/seed/rules.jsonl");
    let raw = std::fs::read_to_string(&rules_path).expect("the shipped charter seeds its rules");
    let mut lines: Vec<String> = Vec::new();
    for line in raw.lines() {
        let mut v: Value = meclaw_core::serde_json::from_str(line).expect("a seed row is json");
        if v.get("kind").and_then(|x| x.as_str()) == Some("price_per_mtok") {
            v["value"] = json!(PRICE_ROW);
        }
        lines.push(v.to_string());
    }
    std::fs::write(&rules_path, lines.join("\n") + "\n").expect("write the rules seed");

    if charter.break_the_schema {
        // One column the meter's own select names, taken away. The store then
        // refuses the read -- which is the store-reject path, arrived at
        // honestly rather than by injecting an error the cell never made.
        let cfg_path = root.join("main/argus/charter/config.json");
        let mut cfg: Value =
            meclaw_core::serde_json::from_str(&std::fs::read_to_string(&cfg_path).expect("read"))
                .expect("the charter config parses");
        cfg["params"]["schema"]["goals"]
            .as_object_mut()
            .expect("the goals schema is an object")
            .remove("quality_gate")
            .expect("the shipped schema has that column");
        let seed_path = root.join("main/argus/charter/seed/goals.jsonl");
        let raw = std::fs::read_to_string(&seed_path).expect("read");
        let mut lines: Vec<String> = Vec::new();
        for line in raw.lines() {
            let mut v: Value = meclaw_core::serde_json::from_str(line).expect("json");
            if let Some(o) = v.as_object_mut() {
                o.remove("quality_gate");
                if let Some(s) = o.get_mut("schema").and_then(|s| s.as_object_mut()) {
                    s.remove("quality_gate");
                }
            }
            lines.push(v.to_string());
        }
        std::fs::write(&seed_path, lines.join("\n") + "\n").expect("write");
        std::fs::write(
            &cfg_path,
            meclaw_core::serde_json::to_string_pretty(&cfg).expect("serialise"),
        )
        .expect("write the charter config");
    }

    std::fs::write(
        root.join(".env"),
        format!(
            "OPENROUTER_API_KEY=test-key\n\
             ARGUS_JUDGE_BASE_URL={judge_url}\n\
             ARGUS_JUDGE_MODEL={MODEL_JUDGE}\n\
             ARGUS_JUDGE_PROVIDER=openai\n"
        ),
    )
    .expect("write .env");
    let _ = target_url;
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

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<meclaw_core::Message>) {
    let h = ColonyHandle::new_with_factories_at(td, factories());
    let (sink_tx, sink_rx) = mpsc::channel::<meclaw_core::Message>(256);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    for (name, f) in factories() {
        registry.insert(name, f);
    }
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("the shipped argus must boot");
    (h, sink_rx)
}

// ═════════════════════════════════════════════════════════════ driving it

async fn on_lane(h: &ColonyHandle, route: &str, text: &str) {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!(route));
    h.send(
        MessageBuilder::new(Path::new("/driver"))
            .body(Body::Inline(
                json!({"messages": [{"origin": "user", "type": "text", "text": text}]}),
            ))
            .hop(hop)
            .ttl(200)
            .build(),
    )
    .await;
}

/// One turn through the target cell, awaited on the capture.
async fn a_turn(h: &ColonyHandle, rx: &mut mpsc::Receiver<meclaw_core::Message>, text: &str) {
    on_lane(h, "in_turn", text).await;
    tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("the target has to answer within the failure marker")
        .expect("the capture channel stays open");
}

/// How many answers of the target cell the LEDGER can see, which is a different
/// number from how many it produced: `colony.db` is written by a queue behind
/// the routing loop, so an answer that has already reached its reader can still
/// be missing from the log for a moment.
fn logged_answers(td: &tempfile::TempDir) -> i64 {
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        td.path().join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return 0;
    };
    conn.query_row(
        "SELECT COUNT(*) FROM message_log
          WHERE json_extract(headers, '$.hop.model') LIKE 'target/%'",
        [],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

fn logged_dead_letters(td: &tempfile::TempDir) -> i64 {
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        td.path().join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return 0;
    };
    conn.query_row("SELECT COUNT(*) FROM dead_letters", [], |r| r.get(0))
        .unwrap_or(0)
}

/// Wait until the ledger can actually see the traffic this test made.
///
/// Not a courtesy and not a sleep: the whole measurement under test is a
/// question put to `colony.db`, and asking it before the writer queue has
/// drained would measure the queue rather than the loop. The loop's OWN answer
/// to that race is the probe's bounded re-ask; the test's answer is to not
/// start the cycle until the premise is on disk.
async fn ledger_sees(td: &tempfile::TempDir, answers: i64, dead_letters: i64) {
    let start = std::time::Instant::now();
    loop {
        let (a, d) = (logged_answers(td), logged_dead_letters(td));
        if a >= answers && d >= dead_letters {
            return;
        }
        if start.elapsed() > Duration::from_secs(30) {
            panic!(
                "the colony log never caught up: answers {a}/{answers}, \
                 dead letters {d}/{dead_letters}"
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ═══════════════════════════════════════════════════════ reading it back

fn receipts_db(td: &tempfile::TempDir) -> std::path::PathBuf {
    td.path().join("main/argus/receipts/cell.db")
}

/// Every `cycles` row, newest last. Read-only, out of the store's own cell.db —
/// which is the loop's public record and the thing the whole issue is about.
fn cycles(td: &tempfile::TempDir) -> Vec<Value> {
    let path = receipts_db(td);
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, goal, at, status, measured, judged, simulated, change,
                revert_plan, verified, effect, outcome, reason_code
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
            "simulated": as_json(field(6)), "change": as_json(field(7)),
            "revert_plan": as_json(field(8)), "verified": as_json(field(9)),
            "effect": as_json(field(10)), "outcome": field(11),
            "reason_code": field(12)
        }))
    });
    match rows {
        Ok(it) => it.filter_map(Result::ok).collect(),
        Err(_) => Vec::new(),
    }
}

/// Everything the colony logged, for a timeout that has to say what it saw.
fn traffic(td: &tempfile::TempDir) -> String {
    let mut out = String::new();
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        td.path().join("colony.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return "no colony.db".into();
    };
    if let Ok(row) = conn.query_row(
        "SELECT COUNT(*), MIN(created_at), MAX(created_at) FROM message_log",
        [],
        |r| {
            Ok(format!(
                "message_log rows={} min={} max={}",
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?
            ))
        },
    ) {
        out.push_str(&row);
        out.push('\n');
    }
    out.push_str(&format!(
        "now={}\n",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    ));
    if let Ok(mut stmt) =
        conn.prepare("SELECT from_path, to_path, headers FROM message_log ORDER BY created_at")
        && let Ok(rows) = stmt.query_map([], |r| {
            Ok(format!(
                "  {} -> {}  {}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?
            ))
        })
    {
        out.push_str("message_log:\n");
        for row in rows.flatten() {
            out.push_str(&row);
            out.push('\n');
        }
    }
    if let Ok(mut stmt) = conn
        .prepare("SELECT sender_path, original_target, error_code, message_json FROM dead_letters")
        && let Ok(rows) = stmt.query_map([], |r| {
            Ok(format!(
                "  {} -> {}  {}\n    {}",
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?
            ))
        })
    {
        out.push_str("dead_letters:\n");
        for row in rows.flatten() {
            out.push_str(&row);
            out.push('\n');
        }
    }
    out
}

/// Poll the receipts until one row satisfies `pred`, or fail loudly with every
/// row that WAS written — a timeout that does not say what it saw costs a run.
async fn wait_for_row(td: &tempfile::TempDir, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let start = std::time::Instant::now();
    loop {
        let rows = cycles(td);
        if let Some(row) = rows.iter().find(|r| pred(r)) {
            return row.clone();
        }
        if start.elapsed() > Duration::from_secs(60) {
            eprintln!("{}", traffic(td));
            panic!(
                "no receipt matching `{what}` after {:?}. The rows that were written:\n{}",
                start.elapsed(),
                meclaw_core::serde_json::to_string_pretty(&rows).unwrap_or_default()
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The target cell's live model, read out of its OWN `cell.db` params overlay.
/// This is the assertion the whole chain exists for: not that a message was
/// sent, but that a running cell's parameters moved.
fn target_model(td: &tempfile::TempDir) -> Option<String> {
    let path = td.path().join("main/target/cell.db");
    let conn =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    let raw: String = conn
        .query_row("SELECT value FROM params WHERE key = 'model'", [], |r| {
            r.get(0)
        })
        .ok()?;
    meclaw_core::serde_json::from_str::<Value>(&raw)
        .ok()?
        .as_str()
        .map(str::to_string)
}

/// Move an applied cycle's `at` into the past. The measurement window and the
/// effect gate are the same number by design, so this is the only lever a test
/// has that does not either wait a day or lie about the window.
fn the_window_has_passed(td: &tempfile::TempDir, cycle_id: &str) {
    let conn = rusqlite::Connection::open(receipts_db(td)).expect("the receipts db is on disk");
    let long_ago = "2020-01-01T00:00:00.000000Z";
    let n = conn
        .execute(
            "UPDATE cycles SET at = ?1 WHERE id = ?2",
            rusqlite::params![long_ago, cycle_id],
        )
        .expect("move the applied row back");
    assert_eq!(n, 1, "exactly one row is moved: {cycle_id}");
}

// ═══════════════════════════════════════════════════════════ the measurements

/// **The proof.** One `llm-cost` goal, driven twice through the whole chain on a
/// booted colony: once to `kept`, once to `reverted`, with the target cell's own
/// `cell.db` read at every turning point.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_cost_goal_runs_a_full_cycle_to_kept_and_then_to_reverted() {
    if skip() {
        return;
    }
    let judge = MockOpenAI::start(vec![
        judge_says(a_model_swap(MODEL_BIRTH, MODEL_KEPT)),
        judge_says(a_model_swap(MODEL_KEPT, MODEL_REVERTED)),
    ])
    .await;
    let provider = MockOpenAI::start(vec![answer(
        MODEL_BIRTH,
        PROMPT_TOKENS_PER_ANSWER,
        "an ordinary answer",
    )])
    .await;

    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &judge.base_url, &provider.base_url, Charter::cost());
    let (h, mut rx) = boot(&td).await;

    // ── traffic, so there is something to measure. Three answers at 1.0 each.
    for i in 0..3 {
        a_turn(&h, &mut rx, &format!("turn {i}")).await;
    }
    ledger_sees(&td, 3, 0).await;

    // ── CYCLE A ────────────────────────────────────────────────────────────
    on_lane(&h, "in_cycle", "run a cycle").await;
    let applied = wait_for_row(&td, "an applied cycle with a probe verdict", |r| {
        r["status"] == "applied" && r["verified"]["verdict"].is_string()
    })
    .await;

    if applied["verified"]["verdict"] != "healthy" {
        // An unhealthy verdict here is almost always something ELSE in the
        // colony erroring inside the probe's window, and the receipt alone
        // cannot say what. Print the traffic before failing.
        eprintln!("{}", traffic(&td));
    }
    assert_eq!(
        applied["verified"]["verdict"], "healthy",
        "the probe has to find the colony healthy right after the change: {applied:#}"
    );
    assert_eq!(
        applied["verified"]["params_update_seen"], true,
        "the probe's whole job is seeing the update arrive: {applied:#}"
    );
    // Defect (1): the mutator reads the measurement out of `context.ar_measured`
    // and nothing set it, so this object was `{}` on every cycle ever run.
    assert_eq!(
        applied["measured"]["cost_eur"], 3.0,
        "the receipt carries the measurement the judge was shown -- three answers \
         at 1.0 each: {applied:#}"
    );
    assert_eq!(
        applied["measured"]["samples"], 3,
        "and the sample count it was significant on: {applied:#}"
    );
    assert_eq!(
        applied["change"]["to"], MODEL_KEPT,
        "the decided change is on the record: {applied:#}"
    );
    assert_eq!(
        applied["revert_plan"]["to"], MODEL_BIRTH,
        "the way back was authored BEFORE the change, which is the charter rule: {applied:#}"
    );
    assert_eq!(applied["judged"]["action"], "change", "{applied:#}");
    assert!(
        applied["simulated"].is_object() && !applied["simulated"].as_object().unwrap().is_empty(),
        "the counterfactual the judge computed is kept: {applied:#}"
    );

    // The point of the whole chain: a RUNNING cell's parameters moved.
    let mut seen = None;
    for _ in 0..200 {
        seen = target_model(&td);
        if seen.as_deref() == Some(MODEL_KEPT) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        seen.as_deref(),
        Some(MODEL_KEPT),
        "the params update has to land in the target's own cell.db"
    );

    // ── the window passes, and the effect is judged mechanically ───────────
    let cycle_a = applied["id"]
        .as_str()
        .expect("a cycle has an id")
        .to_string();
    the_window_has_passed(&td, &cycle_a);
    on_lane(&h, "in_cycle", "the window has passed").await;

    let closed = wait_for_row(&td, "cycle A closed", |r| {
        r["id"] == cycle_a.as_str() && r["status"] == "closed"
    })
    .await;
    assert_eq!(
        closed["outcome"], "kept",
        "no new spend inside the window, so the change proved itself: {closed:#}"
    );
    assert_eq!(
        closed["effect"]["before"], 3.0,
        "the effect names what it compared: {closed:#}"
    );
    assert_eq!(closed["effect"]["after"], 3.0, "{closed:#}");
    assert_eq!(
        target_model(&td).as_deref(),
        Some(MODEL_KEPT),
        "a kept change is not taken back"
    );

    // ── CYCLE B ────────────────────────────────────────────────────────────
    on_lane(&h, "in_cycle", "run another cycle").await;
    let applied_b = wait_for_row(&td, "a second applied cycle", |r| {
        r["id"] != cycle_a.as_str()
            && r["status"] == "applied"
            && r["verified"]["verdict"].is_string()
    })
    .await;
    let cycle_b = applied_b["id"].as_str().expect("an id").to_string();
    assert_eq!(applied_b["change"]["to"], MODEL_REVERTED, "{applied_b:#}");
    assert_eq!(applied_b["revert_plan"]["to"], MODEL_KEPT, "{applied_b:#}");

    let mut seen = None;
    for _ in 0..200 {
        seen = target_model(&td);
        if seen.as_deref() == Some(MODEL_REVERTED) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        seen.as_deref(),
        Some(MODEL_REVERTED),
        "the second change lands too"
    );

    // Two more answers -- the colony now costs MORE than it did at the baseline,
    // which is a change that has not proved itself.
    for i in 0..2 {
        a_turn(&h, &mut rx, &format!("expensive turn {i}")).await;
    }
    ledger_sees(&td, 5, 0).await;
    the_window_has_passed(&td, &cycle_b);
    on_lane(&h, "in_cycle", "the second window has passed").await;

    let closed_b = wait_for_row(&td, "cycle B closed", |r| {
        r["id"] == cycle_b.as_str() && r["status"] == "closed"
    })
    .await;
    assert_eq!(
        closed_b["outcome"], "reverted",
        "the metric moved the wrong way, so the change goes back: {closed_b:#}"
    );
    assert!(
        closed_b["reason_code"]
            .as_str()
            .is_some_and(|s| s.starts_with("not_proven_")),
        "the reason code carries the delta it was ruled on: {closed_b:#}"
    );

    // And the way back travels the same wire the change travelled.
    let mut seen = None;
    for _ in 0..200 {
        seen = target_model(&td);
        if seen.as_deref() == Some(MODEL_KEPT) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        seen.as_deref(),
        Some(MODEL_KEPT),
        "the revert plan restores the value the cycle moved away from"
    );

    // ── the chain has no holes ─────────────────────────────────────────────
    let rows = cycles(&td);
    assert_eq!(
        rows.len(),
        2,
        "two cycles, two rows, no orphans: {}",
        meclaw_core::serde_json::to_string_pretty(&rows).unwrap_or_default()
    );
    assert!(
        rows.iter().all(|r| r["status"] == "closed"),
        "nothing is left open: {rows:#?}"
    );

    // Defect (2): the receipts store's answer to a mutator write had no edge
    // home and dead-lettered on every applied cycle -- which the probe two hops
    // later read as an unhealthy colony.
    let dl = h.drain_dead_letters().await;
    assert!(dl.is_empty(), "a full cycle loses no message: {dl:#?}");

    // The judge was asked exactly twice: once per cycle, never for the effect
    // ruling, which is mechanical by design.
    assert_eq!(
        judge.recorded_requests().await.len(),
        2,
        "one model call per decided cycle and not one more"
    );

    h.shutdown().await;
}

/// GH #462 — **the empty tick is in the chain.** A charter with nothing enabled
/// still leaves a row, so a watcher whose clock stopped and a watcher with
/// nothing to do stop writing the same nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_tick_leaves_a_receipt_on_a_booted_colony() {
    if skip() {
        return;
    }
    let judge = MockOpenAI::start(vec![judge_says(a_model_swap(MODEL_BIRTH, MODEL_KEPT))]).await;
    let provider =
        MockOpenAI::start(vec![answer(MODEL_BIRTH, PROMPT_TOKENS_PER_ANSWER, "ok")]).await;
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &judge.base_url, &provider.base_url, Charter::inert());
    let (h, _rx) = boot(&td).await;

    on_lane(&h, "in_cycle", "tick on an inert charter").await;
    let row = wait_for_row(&td, "an idle receipt", |r| r["outcome"] == "idle").await;
    assert_eq!(row["reason_code"], "no_enabled_goal", "{row:#}");
    assert_eq!(row["status"], "closed", "{row:#}");
    assert_eq!(
        judge.recorded_requests().await.len(),
        0,
        "an inert charter reaches no model"
    );
    let dl = h.drain_dead_letters().await;
    assert!(dl.is_empty(), "and loses nothing: {dl:#?}");
    h.shutdown().await;
}

/// GH #462 — **a cycle that dies at a store reject is in the chain too.** The
/// charter's schema is one column short of what the meter selects, so the store
/// refuses the read for real rather than by injection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_store_rejection_leaves_a_receipt_on_a_booted_colony() {
    if skip() {
        return;
    }
    let judge = MockOpenAI::start(vec![judge_says(a_model_swap(MODEL_BIRTH, MODEL_KEPT))]).await;
    let provider =
        MockOpenAI::start(vec![answer(MODEL_BIRTH, PROMPT_TOKENS_PER_ANSWER, "ok")]).await;
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &judge.base_url, &provider.base_url, Charter::broken());
    let (h, mut rx) = boot(&td).await;

    on_lane(&h, "in_cycle", "tick against a charter that cannot answer").await;
    let row = wait_for_row(&td, "a store_error receipt", |r| {
        r["outcome"] == "store_error"
    })
    .await;
    assert!(
        row["reason_code"]
            .as_str()
            .is_some_and(|s| s.starts_with("store_rejected_select_")),
        "the row names the operation that was refused: {row:#}"
    );
    assert_eq!(row["status"], "closed", "{row:#}");

    // And the failure leaves the hive, so the colony around it is not the last
    // to know.
    let out = tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("the error lane has to reach the rim")
        .expect("the capture channel stays open");
    assert_eq!(
        out.headers.hop.get("route").and_then(|v| v.as_str()),
        Some("error"),
        "{out:#?}"
    );
    h.shutdown().await;
}

/// GH #462 — **the deterministic reaction path.** A `dlq-watch` observation
/// counts what it sees, records it, alerts on it, and never asks a model. The
/// mock's call count is the whole assertion: the judge is for optimisation goals
/// and for nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_dlq_watch_goal_reacts_without_a_model() {
    if skip() {
        return;
    }
    let judge = MockOpenAI::start(vec![judge_says(a_model_swap(MODEL_BIRTH, MODEL_KEPT))]).await;
    let provider =
        MockOpenAI::start(vec![answer(MODEL_BIRTH, PROMPT_TOKENS_PER_ANSWER, "ok")]).await;
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &judge.base_url, &provider.base_url, Charter::dlq());
    let (h, mut rx) = boot(&td).await;

    // One turn, so the goal's sample floor is met, and one letter that dies.
    a_turn(&h, &mut rx, "a turn").await;
    h.send(
        MessageBuilder::new(Path::new("/nobody"))
            .body(Body::Inline(
                json!({"messages": [{"origin": "user", "type": "text", "text": "into the void"}]}),
            ))
            .ttl(8)
            .build(),
    )
    .await;

    ledger_sees(&td, 1, 1).await;
    on_lane(&h, "in_cycle", "watch the dead letters").await;
    let row = wait_for_row(&td, "an observation", |r| r["outcome"] == "observed").await;
    assert_eq!(
        row["reason_code"], "observed_dlq_rate_1",
        "the count is in the reason code, so a clean watch and a spike differ: {row:#}"
    );
    assert!(
        row["change"].as_object().is_some_and(|o| o.is_empty()),
        "an observation proposes nothing: {row:#}"
    );

    // The alert leaves the hive.
    let alert = loop {
        let m = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .expect("the alert lane has to reach the rim")
            .expect("the capture channel stays open");
        if m.headers.hop.get("route").and_then(|v| v.as_str()) == Some("alert") {
            break m;
        }
    };
    let body = match &alert.body {
        Body::Inline(v) => v.clone(),
        other => panic!("the alert carries an inline body: {other:?}"),
    };
    let text = body["messages"][0]["text"]
        .as_str()
        .expect("the alert says what it saw");
    let payload: Value = meclaw_core::serde_json::from_str(text).expect("the alert body is json");
    assert_eq!(payload["metric"], "dlq_rate", "{payload:#}");
    assert_eq!(payload["value"], 1.0, "{payload:#}");
    assert_eq!(payload["goal"], "goal:dlq-watch", "{payload:#}");

    assert_eq!(
        judge.recorded_requests().await.len(),
        0,
        "a symptom is never handed to a model -- the whole point of `observe`"
    );
    h.shutdown().await;
}
