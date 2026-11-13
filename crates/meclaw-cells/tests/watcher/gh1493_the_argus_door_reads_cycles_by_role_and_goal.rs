//! GH #1493 -- the argus door reads the cycle receipts by role and by goal.
//!
//! Every observation of the argus stays in its `cycles` table and is readable
//! page by page after a time (`read_since`, GH #928). What a reader could not
//! do was ask for the cycles of ONE role or ONE goal: the door selected by
//! `at > since` alone, so a reader who wanted one role had to page through
//! every row. `in_read` with `read_what` "cycles" now takes two optional hop
//! keys, `read_role` and `read_goal`, compared as strings against the columns
//! `cycles.role` and `cycles.goal`.
//!
//! The filter is applied IN THE STORE, in the `where` of the select beside
//! `at > since`, never by the door after the read: only then do `read_limit`
//! and `next_since` count the narrowed rows, and only then does paging with
//! `next_since` deliver every matching row exactly once.
//! `role_and_goal_together_narrow_and_next_since_pages_the_narrowed_rows`
//! seeds matching rows between foreign ones and pages with a limit below the
//! number of matches; a door that filtered after the read would hand out
//! short pages and a cursor that skips matches.
//!
//! Two layers, as in `gh928_the_argus_door_reads_and_takes_hints.rs`: the
//! validation of the two keys and their absence for the charter and the hints
//! as a table under python3 against the SHIPPED `script_inline`; the filter
//! and the paging on a BOOTED colony whose receipts store is seeded with
//! cycles of two roles and two goals, every assertion at the asker's sink.
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! template skips.

// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;

use meclaw_cells::TimerCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::llm::LlmCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use mock_openai::MockOpenAI;
use std::collections::BTreeSet;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use tokio::sync::mpsc;

// ═══════════════════════════════════════════════════════════════ the template

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The shipped template, or `None` where it did not travel (GH #49 / R2b).
fn shipped() -> Option<std::path::PathBuf> {
    let root = repo("templates/argus");
    root.join("door/config.json").is_file().then_some(root)
}

fn skip() -> bool {
    if shipped().is_none() {
        eprintln!("argus did not travel into this tree -- skipped (GH #49)");
        return true;
    }
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("no python3 -- skipped");
        return true;
    }
    false
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    sj::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn door_script() -> String {
    read_json(&repo("templates/argus/door/config.json"))["params"]["script_inline"]
        .as_str()
        .expect("the door is a code cell with an inline script")
        .to_string()
}

/// Run a program under python3, the program itself on stdin (a single argv
/// string is capped at 128 KiB).
fn run_python(program: &str) -> std::process::Output {
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(program.as_bytes())
        .expect("write the program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

// ═══════════════════════════════════════════════════ the pure half, as a table

/// Loads the shipped script WITHOUT running its main and runs `start_read` on
/// a hop: `asked(hop)` is what the door does with the question -- either the
/// refusal it answers (`{"refused": <detail>}`) or the store call it sends
/// (`{"table", "where"}`).
const PRELUDE: &str = r#"
import json, sys
ns = {"__name__": "gh1493_table"}
exec(compile(SCRIPT, "door", "exec"), ns)
failures = []
count = 0

def asked(hop):
    out = ns["start_read"](hop)
    if len(out) != 1:
        return {"messages": len(out)}
    m = out[0]
    head = m["header"]
    if head.get("route") == "read":
        return {"refused": head.get("detail"), "error_code": head.get("error_code"),
                "read_what": head.get("read_what"), "read_tag": head.get("read_tag")}
    args = json.loads(m["messages"][0]["text"])
    return {"table": args.get("table"), "where": args.get("where")}

def expect(name, hop, want):
    global count
    count += 1
    try:
        got = asked(hop)
    except Exception as e:
        failures.append("%s: raised %r" % (name, e))
        return
    if got != want:
        failures.append("%s: want %r, got %r" % (name, want, got))

def refused(key, what="cycles", tag="t"):
    return {"refused": key, "error_code": "invalid_input", "read_what": what, "read_tag": tag}

def cycles(**where):
    return {"table": "cycles", "where": where or None}

SINCE = "2026-09-30T12:00:00Z"
STORED = "2026-09-30T12:00:00.000000Z"
"#;

/// `read_role` and `read_goal` are checked after the keys before them, in the
/// order `read_what`, `read_since`, `read_limit`, `read_tag`, `read_role`,
/// `read_goal`: blank or missing is no filter, otherwise a string of 1..=64
/// characters, otherwise `invalid_input` with the key as `detail`.
const REFUSAL_TABLE: &str = r#"
def hop(**kw):
    base = {"read_what": "cycles", "read_tag": "t"}
    base.update(kw)
    return base

expect("a role of 1", hop(read_role="r"), cycles(role={"eq": "r"}))
expect("a role of 64", hop(read_role="r" * 64), cycles(role={"eq": "r" * 64}))
expect("a role of 65", hop(read_role="r" * 65), refused("read_role"))
expect("a role that is a number", hop(read_role=7), refused("read_role"))
expect("a role that is a bool", hop(read_role=True), refused("read_role"))
expect("a role that is a list", hop(read_role=["r"]), refused("read_role"))
expect("a role that is an object", hop(read_role={"eq": "r"}), refused("read_role"))
expect("an empty role is no filter", hop(read_role=""), cycles())
expect("a null role is no filter", hop(read_role=None), cycles())

expect("a goal of 1", hop(read_goal="g"), cycles(goal={"eq": "g"}))
expect("a goal of 64", hop(read_goal="g" * 64), cycles(goal={"eq": "g" * 64}))
expect("a goal of 65", hop(read_goal="g" * 65), refused("read_goal"))
expect("a goal that is a number", hop(read_goal=3), refused("read_goal"))
expect("a goal that is a bool", hop(read_goal=False), refused("read_goal"))
expect("a goal that is a list", hop(read_goal=["g"]), refused("read_goal"))
expect("an empty goal is no filter", hop(read_goal=""), cycles())
expect("a null goal is no filter", hop(read_goal=None), cycles())

expect("both, with the cursor", hop(read_role="r", read_goal="g", read_since=SINCE),
       cycles(at={"gt": STORED}, role={"eq": "r"}, goal={"eq": "g"}))
expect("the role is named before the goal", hop(read_role=1, read_goal=2), refused("read_role"))
expect("a good role, a bad goal", hop(read_role="r", read_goal="g" * 65), refused("read_goal"))
expect("the tag is named before the role", hop(read_tag="", read_role=1), refused("read_tag", tag=""))
expect("the limit is named before the role", hop(read_limit=101, read_role=1), refused("read_limit"))
expect("the cursor is named before the goal", hop(read_since="yesterday", read_goal=2), refused("read_since"))
"#;

/// The charter and the hints have no role and no goal: there both keys are
/// not looked at -- a value that would be refused for the cycles is no
/// refusal there, and no store call of theirs carries a role or a goal.
const OTHERS_TABLE: &str = r#"
for what, table in (("hints", "hints"), ("charter", "goals")):
    for role, goal in (("r", "g"), ("r" * 65, 7), (["r"], {"eq": "g"})):
        want = {"table": table, "where": None}
        expect("%s with role %r and goal %r" % (what, role, goal),
               {"read_what": what, "read_tag": "t", "read_role": role, "read_goal": goal}, want)
expect("hints with a cursor keep only the cursor",
       {"read_what": "hints", "read_tag": "t", "read_since": SINCE, "read_role": "r", "read_goal": "g"},
       {"table": "hints", "where": {"at": {"gt": STORED}}})
"#;

const TABLE_END: &str = r#"
print(json.dumps({"cases": count, "failures": failures}))
sys.exit(1 if failures else 0)
"#;

fn run_table(table: &str) -> Value {
    let program = format!(
        "SCRIPT = {}\n{PRELUDE}\n{table}\n{TABLE_END}",
        sj::to_string(&door_script()).expect("the script serialises")
    );
    let out = run_python(&program);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let verdict: Value = sj::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("the table did not run ({e}):\nstdout: {stdout}\nstderr: {stderr}")
    });
    assert!(
        out.status.success(),
        "{} of {} rows disagree:\n{:#}",
        verdict["failures"].as_array().map_or(0, Vec::len),
        verdict["cases"],
        verdict["failures"]
    );
    verdict
}

#[test]
fn a_role_or_goal_that_is_not_a_short_string_is_refused_with_its_key() {
    if skip() {
        return;
    }
    let verdict = run_table(REFUSAL_TABLE);
    assert!(verdict["cases"].as_u64().unwrap_or(0) >= 23, "{verdict:#}");
}

#[test]
fn role_and_goal_are_not_looked_at_for_the_charter_and_the_hints() {
    if skip() {
        return;
    }
    let verdict = run_table(OTHERS_TABLE);
    assert!(verdict["cases"].as_u64().unwrap_or(0) >= 7, "{verdict:#}");
}

// ═══════════════════════════════════════════════════════ the booted colony

const MODEL_JUDGE: &str = "judge/thinker";

/// The asker: puts one message on the lane its hop names and carries the
/// `read_*` keys and the whole body along, so the lane is entered over an
/// edge onto the HIVE, the way a colony enters it.
const ASKER: &str = "import sys, json\n\
                     doc = json.load(sys.stdin)\n\
                     hop = (doc[\"envelope\"].get(\"header\") or {}).get(\"hop\") or {}\n\
                     head = {k: v for k, v in hop.items() if k == \"route\" or k.startswith(\"read_\")}\n\
                     out = {\"header\": head}\n\
                     out.update(doc[\"body\"])\n\
                     sys.stdout.write(json.dumps([out]))\n";

/// The seeded cycles: 24 rows, oldest first, of two roles and two goals.
/// Row `i` has role `role-a` when `i` is even (else `role-b`) and goal
/// `goal:one` when `i` is a multiple of three (else `goal:two`), so the rows
/// of one role, of one goal and of both are interleaved with foreign rows.
const SEEDED: usize = 24;

fn role_of(i: usize) -> &'static str {
    if i.is_multiple_of(2) {
        "role-a"
    } else {
        "role-b"
    }
}

fn goal_of(i: usize) -> &'static str {
    if i.is_multiple_of(3) {
        "goal:one"
    } else {
        "goal:two"
    }
}

fn cycle_id(i: usize) -> String {
    format!("cycle:seed-{i:02}")
}

/// A fixed stored `at` per row: the rows are data of the past, compared with
/// each other and never with a clock.
fn cycle_at(i: usize) -> String {
    format!("2026-01-01T00:{:02}:00.000000Z", i)
}

fn seed_cycles(receipts: &Value) -> String {
    let schema = receipts["params"]["schema"]["cycles"].clone();
    let mut lines = vec![json!({ "schema": schema }).to_string()];
    for i in 0..SEEDED {
        lines.push(
            json!({
                "id": cycle_id(i),
                "goal": goal_of(i),
                "at": cycle_at(i),
                "status": "observed",
                "outcome": "observed",
                "reason_code": "",
                "role": role_of(i),
                "hints": [],
            })
            .to_string(),
        );
    }
    lines.join("\n") + "\n"
}

fn main_config() -> Value {
    json!({
        "cell": {"type": "hive"},
        "params": {"graph": {"edges": [
            {"from": "./asker", "to": "./argus",
             "condition": "has(hop.route) && hop.route == 'in_read'"},
            {"from": "./argus", "to": "/sink",
             "condition": "has(hop.route) && (hop.route == 'read' || hop.route == 'error')"}
        ]}}
    })
}

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("create the directory");
    std::fs::write(p, sj::to_string_pretty(v).expect("serialise")).expect("write");
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

/// The tree: the shipped argus copied in, its receipts store seeded with the
/// cycles above (a seed applies once, at the birth of the store), the judge's
/// endpoint on a mock that is never asked.
fn build_tree(td: &tempfile::TempDir, judge_url: &str) {
    let root = td.path();
    write_json(&root.join("main/config.json"), &main_config());
    write_json(
        &root.join("main/asker/config.json"),
        &json!({
            "cell": {"type": "code"},
            "params": {"runner": "python3", "script_inline": ASKER,
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
    copy_tree(&shipped().expect("guarded"), &root.join("main/argus"));
    let receipts = read_json(&root.join("main/argus/receipts/config.json"));
    let seed = root.join("main/argus/receipts/seed/cycles.jsonl");
    std::fs::create_dir_all(seed.parent().expect("a parent")).expect("create the seed directory");
    std::fs::write(&seed, seed_cycles(&receipts)).expect("write the seed");
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

/// A booted colony with the seeded argus, its sink, and the judge mock that
/// must stay unasked.
async fn boot() -> (
    tempfile::TempDir,
    MockOpenAI,
    ColonyHandle,
    mpsc::Receiver<meclaw_core::Message>,
) {
    let judge = MockOpenAI::start(vec![]).await;
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &judge.base_url);
    let h = ColonyHandle::new_with_factories_at(&td, factories());
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
    (td, judge, h, sink_rx)
}

/// What came back at the sink: the hop and the body.
struct Answer {
    hop: Map<String, Value>,
    body: Value,
}

impl Answer {
    fn hop_str(&self, key: &str) -> &str {
        self.hop.get(key).and_then(Value::as_str).unwrap_or("")
    }
    fn rows(&self) -> Vec<Value> {
        self.body["rows"].as_array().cloned().unwrap_or_default()
    }
}

/// One `in_read` for the cycles, answered at the sink without an error.
async fn read_cycles(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<meclaw_core::Message>,
    tag: &str,
    extra: Value,
) -> Answer {
    let mut hop = json!({"route": "in_read", "read_what": "cycles", "read_tag": tag});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        hop[k] = v;
    }
    h.send(
        MessageBuilder::new(Path::new("/asker"))
            .body(Body::Inline(json!({"messages": []})))
            .hop(hop.as_object().cloned().expect("a hop is an object"))
            .ttl(64)
            .build(),
    )
    .await;
    let hang = meclaw_testing::deadline::scaled(meclaw_testing::deadline::HANG);
    let m = tokio::time::timeout(hang, rx.recv())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "HANG: the door's answer to the read {tag} (waited {} s)",
                hang.as_secs()
            )
        })
        .expect("the capture channel stays open");
    let body = match &m.body {
        Body::Inline(v) => v.clone(),
        other => panic!("the answer carries an inline body: {other:?}"),
    };
    let a = Answer {
        hop: m.headers.hop.clone(),
        body,
    };
    assert_eq!(a.hop_str("route"), "read", "{:#?} {:#}", a.hop, a.body);
    assert_eq!(
        a.hop_str("read_tag"),
        tag,
        "the tag is echoed: {:#?}",
        a.hop
    );
    assert_eq!(a.hop_str("read_what"), "cycles", "{:#?}", a.hop);
    assert_eq!(a.hop_str("error_code"), "", "{:#?} {:#}", a.hop, a.body);
    a
}

fn ids(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|r| r["id"].as_str().unwrap_or("").to_string())
        .collect()
}

fn expected(keep: impl Fn(usize) -> bool) -> Vec<String> {
    (0..SEEDED).filter(|i| keep(*i)).map(cycle_id).collect()
}

/// Every column of the cycles table, as the receipts store declares it.
fn cycle_columns() -> BTreeSet<String> {
    read_json(&repo("templates/argus/receipts/config.json"))["params"]["schema"]["cycles"]
        .as_object()
        .expect("the receipts keep a cycles table")
        .keys()
        .cloned()
        .collect()
}

async fn finish(h: ColonyHandle, judge: MockOpenAI) {
    assert_eq!(
        judge.recorded_requests().await.len(),
        0,
        "a read reaches no model"
    );
    let dl = h.drain_dead_letters().await;
    assert!(dl.is_empty(), "nothing was lost on the way: {dl:#?}");
    h.shutdown().await;
}

/// The cycles of one role: every row of that role, oldest first, every
/// column, none of the other role; the cursor is the last row's `at`. An
/// unfiltered read first shows that both roles are stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_of_cycles_by_role_returns_only_that_roles_rows() {
    if skip() {
        return;
    }
    let (_td, judge, h, mut rx) = boot().await;

    let a = read_cycles(&h, &mut rx, "t-all", json!({"read_limit": 100})).await;
    assert_eq!(ids(&a.rows()), expected(|_| true), "every seeded cycle");

    let a = read_cycles(
        &h,
        &mut rx,
        "t-role",
        json!({"read_limit": 100, "read_role": "role-b"}),
    )
    .await;
    let rows = a.rows();
    assert_eq!(
        ids(&rows),
        expected(|i| role_of(i) == "role-b"),
        "only role-b, oldest first: {rows:#?}"
    );
    for r in &rows {
        assert_eq!(r["role"], "role-b", "{r:#}");
        let keys: BTreeSet<String> = r.as_object().expect("a row").keys().cloned().collect();
        assert_eq!(keys, cycle_columns(), "a cycle as stored: {r:#}");
    }
    assert_eq!(
        a.body["next_since"],
        rows.last().expect("rows")["at"],
        "{:#}",
        a.body
    );

    let a = read_cycles(
        &h,
        &mut rx,
        "t-nobody",
        json!({"read_limit": 100, "read_role": "role-z"}),
    )
    .await;
    assert!(a.rows().is_empty(), "a role nobody has: {:#}", a.body);
    assert_eq!(a.body["next_since"], "", "{:#}", a.body);
    finish(h, judge).await;
}

/// The cycles of one goal: every row of that goal, oldest first, none of the
/// other goal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_read_of_cycles_by_goal_returns_only_that_goals_rows() {
    if skip() {
        return;
    }
    let (_td, judge, h, mut rx) = boot().await;

    let a = read_cycles(
        &h,
        &mut rx,
        "t-goal",
        json!({"read_limit": 100, "read_goal": "goal:one"}),
    )
    .await;
    let rows = a.rows();
    assert_eq!(
        ids(&rows),
        expected(|i| goal_of(i) == "goal:one"),
        "only goal:one, oldest first: {rows:#?}"
    );
    for r in &rows {
        assert_eq!(r["goal"], "goal:one", "{r:#}");
    }
    assert_eq!(
        a.body["next_since"],
        rows.last().expect("rows")["at"],
        "{:#}",
        a.body
    );

    let a = read_cycles(
        &h,
        &mut rx,
        "t-goal-2",
        json!({"read_limit": 100, "read_goal": "goal:two", "read_role": ""}),
    )
    .await;
    assert_eq!(
        ids(&a.rows()),
        expected(|i| goal_of(i) == "goal:two"),
        "an empty role is no filter: {:#}",
        a.body
    );
    finish(h, judge).await;
}

/// Role and goal together narrow to the rows of both (rows 0, 6, 12, 18 of
/// 24), and with `read_limit` 3 below that count the pages are the narrowed
/// rows: three, then one, then none -- every match exactly once, no foreign
/// row, and each `next_since` the `at` of the last row of its page. A door
/// that filtered after reading would page through all 24 rows three at a time
/// and hand out short pages.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn role_and_goal_together_narrow_and_next_since_pages_the_narrowed_rows() {
    if skip() {
        return;
    }
    let (_td, judge, h, mut rx) = boot().await;
    let want = expected(|i| role_of(i) == "role-a" && goal_of(i) == "goal:one");
    assert_eq!(want.len(), 4, "the seed holds four rows of both");

    let filter = json!({"read_limit": 3, "read_role": "role-a", "read_goal": "goal:one"});
    let mut pages: Vec<Vec<String>> = Vec::new();
    let mut since = String::new();
    for n in 0..3 {
        let mut ask = filter.clone();
        if !since.is_empty() {
            ask["read_since"] = json!(since);
        }
        let a = read_cycles(&h, &mut rx, &format!("t-page-{n}"), ask).await;
        let rows = a.rows();
        for r in &rows {
            assert_eq!(r["role"], "role-a", "no foreign role: {r:#}");
            assert_eq!(r["goal"], "goal:one", "no foreign goal: {r:#}");
        }
        let cursor = a.body["next_since"].as_str().unwrap_or("").to_string();
        match rows.last() {
            Some(last) => assert_eq!(last["at"], json!(cursor), "{:#}", a.body),
            None => assert_eq!(cursor, "", "{:#}", a.body),
        }
        pages.push(ids(&rows));
        since = cursor;
        if since.is_empty() {
            break;
        }
    }
    assert_eq!(
        pages,
        vec![want[..3].to_vec(), want[3..].to_vec(), Vec::<String>::new()],
        "three, one, none"
    );
    finish(h, judge).await;
}
