//! GH #928 -- the argus door: a read lane and a hint lane for another part of
//! the colony.
//!
//! The argus hive is sealed (`params.ports` is empty): nothing outside it could
//! read its charter or its cycle receipts, and nothing could hand it a
//! hypothesis to look at. `./door` is the one cell that opens both ways, and
//! the contract it keeps is narrow on purpose:
//!
//! ```text
//!   in_read -> door -> charter | receipts (select) -> door -> read
//!   in_hint -> door -> receipts (insert into `hints`) -> door -> hint_ack
//! ```
//!
//! **Read-only except one insert.** The door never writes `goals`, `rules`,
//! `cycles` or `waits`, and it never changes a `hints` row -- `consumed_by` is
//! set by the loop that shows the hint to its judge. **Rows as stored.** A read
//! returns every column of its table and nothing is projected away. The store
//! has no wildcard select (`columns` is mandatory), so the door names every
//! column, and a column a later version adds needs its entry in the door's
//! `COLUMNS`: `the_door_reads_every_column_its_tables_have` pins that list to
//! the schemas and fails on a column the door does not name.
//! **A hint decides nothing.** It is stored and acknowledged; no edge leads
//! from the door to the meter, the judge or the mutator, and the colony case
//! below checks that the judge was never asked and no cycle was written by a
//! hint.
//!
//! Two layers, the pure one first:
//! - the validation of both lanes and the dispatch of `step` as tables, run
//!   under python3 against the SHIPPED `script_inline` (`READ_TABLE`,
//!   `HINT_TABLE`, `STEP_TABLE`) -- every rule of the contract has a row, and
//!   every boundary has two;
//! - the lanes on a BOOTED colony: the shipped argus hive, a quiet clock, a
//!   judge on a mock provider, and an asker cell that enters the hive over an
//!   ordinary parent edge. Every assertion is made at the asker's sink.
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! template skips.

#[path = "mock_openai.rs"]
mod mock_openai;

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
use std::time::Duration;
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
/// string is capped at 128 KiB; same harness as `curator_history.rs`).
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

/// A top-level literal of the script, by name (same helper as
/// `curator_history.rs`).
fn literal(script: &str, name: &str) -> Value {
    let program = format!(
        concat!(
            "import ast, json, sys\n",
            "tree = ast.parse(sys.stdin.read())\n",
            "for node in tree.body:\n",
            "    if isinstance(node, ast.Assign) and any(getattr(t, 'id', '') == {name:?} for t in node.targets):\n",
            "        print(json.dumps(ast.literal_eval(node.value)))\n",
            "        break\n"
        ),
        name = name
    );
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(program)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(script.as_bytes())
        .expect("write the script");
    let out = child.wait_with_output().expect("wait");
    sj::from_slice(&out.stdout).unwrap_or_else(|e| panic!("`{name}` is no top-level literal: {e}"))
}

// ═══════════════════════════════════════════════════ the pure half, as a table

/// Loads the shipped script WITHOUT running its main (the cell runs it as
/// `__main__`; here it is a module), then `check(fn, cases)` compares each
/// case: a dict is the accepted request, a string is the key that is refused.
const PRELUDE: &str = r#"
import json, sys
ns = {"__name__": "gh928_table"}
exec(compile(SCRIPT, "door", "exec"), ns)
failures = []
count = 0

def check(fn, cases):
    global count
    for name, arg, want in cases:
        count += 1
        try:
            value, bad = ns[fn](arg)
        except Exception as e:
            failures.append("%s / %s: raised %r" % (fn, name, e))
            continue
        if isinstance(want, str):
            if bad != want or value is not None:
                failures.append("%s / %s: want refusal on %r, got value=%r bad=%r"
                                % (fn, name, want, value, bad))
        elif bad is not None or value != want:
            failures.append("%s / %s: want %r, got value=%r bad=%r"
                            % (fn, name, want, value, bad))
"#;

/// Every rule of `in_read`, whose contract is the lane's `because` in the hive:
/// `read_what` from a closed set, `read_since` RFC 3339 and normalised to the
/// stored form, `read_limit` 1..=100 defaulting to 20, `read_tag` 1..=64
/// characters. Checked in that order.
const READ_TABLE: &str = r#"
def req(what, since=None, limit=20, tag="t"):
    return {"what": what, "since": since, "limit": limit, "tag": tag}

check("parse_read", [
    ("the minimum, cycles", {"read_what": "cycles", "read_tag": "t"}, req("cycles")),
    ("the minimum, hints", {"read_what": "hints", "read_tag": "t"}, req("hints")),
    ("the minimum, charter", {"read_what": "charter", "read_tag": "t"}, req("charter")),
    ("no hop at all", None, "read_what"),
    ("no read_what", {"read_tag": "t"}, "read_what"),
    ("an unknown table", {"read_what": "goals", "read_tag": "t"}, "read_what"),
    ("the wrong case", {"read_what": "Cycles", "read_tag": "t"}, "read_what"),
    ("read_what not a string", {"read_what": 1, "read_tag": "t"}, "read_what"),
    ("the first refusal wins", {"read_what": "waits", "read_tag": ""}, "read_what"),

    ("limit 0", {"read_what": "hints", "read_limit": 0, "read_tag": "t"}, "read_limit"),
    ("limit 1", {"read_what": "hints", "read_limit": 1, "read_tag": "t"}, req("hints", limit=1)),
    ("limit 100", {"read_what": "hints", "read_limit": 100, "read_tag": "t"}, req("hints", limit=100)),
    ("limit 101", {"read_what": "hints", "read_limit": 101, "read_tag": "t"}, "read_limit"),
    ("limit -1", {"read_what": "hints", "read_limit": -1, "read_tag": "t"}, "read_limit"),
    ("limit as digits", {"read_what": "hints", "read_limit": "20", "read_tag": "t"}, req("hints", limit=20)),
    ("limit as digits 101", {"read_what": "hints", "read_limit": "101", "read_tag": "t"}, "read_limit"),
    ("limit as a word", {"read_what": "hints", "read_limit": "ten", "read_tag": "t"}, "read_limit"),
    ("limit as an exponent", {"read_what": "hints", "read_limit": "1e2", "read_tag": "t"}, "read_limit"),
    ("limit as a signed string", {"read_what": "hints", "read_limit": "+5", "read_tag": "t"}, "read_limit"),
    ("limit as a float", {"read_what": "hints", "read_limit": 20.0, "read_tag": "t"}, "read_limit"),
    ("limit as a fraction", {"read_what": "hints", "read_limit": 20.5, "read_tag": "t"}, "read_limit"),
    ("limit as a bool", {"read_what": "hints", "read_limit": True, "read_tag": "t"}, "read_limit"),
    ("limit empty is the default", {"read_what": "hints", "read_limit": "", "read_tag": "t"}, req("hints")),
    ("limit null is the default", {"read_what": "hints", "read_limit": None, "read_tag": "t"}, req("hints")),

    ("no tag", {"read_what": "cycles"}, "read_tag"),
    ("an empty tag", {"read_what": "cycles", "read_tag": ""}, "read_tag"),
    ("a tag of 64", {"read_what": "cycles", "read_tag": "x" * 64}, req("cycles", tag="x" * 64)),
    ("a tag of 65", {"read_what": "cycles", "read_tag": "x" * 65}, "read_tag"),
    ("a tag that is a number", {"read_what": "cycles", "read_tag": 7}, "read_tag"),

    ("since in Z", {"read_what": "cycles", "read_since": "2026-09-30T12:00:00Z", "read_tag": "t"},
     req("cycles", since="2026-09-30T12:00:00.000000Z")),
    ("since as stored", {"read_what": "hints", "read_since": "2026-09-30T12:00:00.123456Z", "read_tag": "t"},
     req("hints", since="2026-09-30T12:00:00.123456Z")),
    ("since with an offset", {"read_what": "cycles", "read_since": "2026-09-30T14:00:00.5+02:00", "read_tag": "t"},
     req("cycles", since="2026-09-30T12:00:00.500000Z")),
    ("since across midnight", {"read_what": "cycles", "read_since": "2026-09-30T23:30:00-01:00", "read_tag": "t"},
     req("cycles", since="2026-10-01T00:30:00.000000Z")),
    ("since in nanoseconds", {"read_what": "cycles", "read_since": "2026-09-30T12:00:00.123456789Z", "read_tag": "t"},
     req("cycles", since="2026-09-30T12:00:00.123456Z")),
    ("since in lower case", {"read_what": "cycles", "read_since": "2026-09-30t12:00:00z", "read_tag": "t"},
     req("cycles", since="2026-09-30T12:00:00.000000Z")),
    ("since empty is no since", {"read_what": "cycles", "read_since": "", "read_tag": "t"}, req("cycles")),
    ("since without an offset", {"read_what": "cycles", "read_since": "2026-09-30T12:00:00", "read_tag": "t"}, "read_since"),
    ("since a date only", {"read_what": "cycles", "read_since": "2026-09-30", "read_tag": "t"}, "read_since"),
    ("since with a space", {"read_what": "cycles", "read_since": "2026-09-30 12:00:00Z", "read_tag": "t"}, "read_since"),
    ("since month 13", {"read_what": "cycles", "read_since": "2026-13-01T00:00:00Z", "read_tag": "t"}, "read_since"),
    ("since offset 24h", {"read_what": "cycles", "read_since": "2026-09-30T12:00:00+24:00", "read_tag": "t"}, "read_since"),
    ("since with a newline", {"read_what": "cycles", "read_since": "2026-09-30T12:00:00Z\n", "read_tag": "t"}, "read_since"),
    ("since a word", {"read_what": "hints", "read_since": "yesterday", "read_tag": "t"}, "read_since"),
    ("since epoch seconds", {"read_what": "hints", "read_since": 1790000000, "read_tag": "t"}, "read_since"),
    # RFC 3339 digits are ASCII: a regex `\d` would take any Unicode decimal
    # digit and normalise it into a valid-looking cursor.
    ("since in arabic-indic digits", {"read_what": "cycles", "read_since": "٢٠٢٦-09-30T12:00:00Z", "read_tag": "t"}, "read_since"),
    ("since with a fraction in other digits", {"read_what": "cycles", "read_since": "2026-09-30T12:00:00.٥Z", "read_tag": "t"}, "read_since"),
    ("since with an offset in fullwidth digits", {"read_what": "cycles", "read_since": "2026-09-30T12:00:00+０２:00", "read_tag": "t"}, "read_since"),
    ("charter ignores since", {"read_what": "charter", "read_since": "yesterday", "read_tag": "t"}, req("charter")),
])
"#;

/// Every rule of `in_hint`: the body slot `hint` is an object; `origin`
/// `[a-z0-9-]{1,64}`; `confidence` a JSON integer 0..=100 (no string, no
/// float, no bool); `line` 1..=500 characters after trimming, with no control
/// character and no line break anywhere in it. Checked in that order.
const HINT_TABLE: &str = r#"
def body(**hint):
    base = {"origin": "probe-origin", "confidence": 50, "line": "the answers got longer"}
    base.update(hint)
    return {"messages": [], "hint": {k: v for k, v in base.items() if v is not ...}}

def ok(**hint):
    base = {"origin": "probe-origin", "confidence": 50, "line": "the answers got longer"}
    base.update(hint)
    return base

check("parse_hint", [
    ("a valid hint", body(), ok()),
    ("the line is trimmed", body(line="  padded  "), ok(line="padded")),
    ("letters beyond ascii", body(line="caf\u00e9 na\u00efve r\u00e9sum\u00e9"), ok(line="caf\u00e9 na\u00efve r\u00e9sum\u00e9")),
    ("no body", None, "hint"),
    ("no hint slot", {"messages": []}, "hint"),
    ("a hint that is a string", {"messages": [], "hint": "look here"}, "hint"),
    ("a hint that is a list", {"messages": [], "hint": []}, "hint"),

    ("an origin with a capital", body(origin="Probe-origin"), "origin"),
    ("an origin with a slash", body(origin="probe/origin"), "origin"),
    ("an origin with an underscore", body(origin="probe_origin"), "origin"),
    ("an origin of 64", body(origin="a" * 64), ok(origin="a" * 64)),
    ("an origin of 65", body(origin="a" * 65), "origin"),
    ("an empty origin", body(origin=""), "origin"),
    ("an origin with a newline", body(origin="probe-origin\n"), "origin"),
    ("an origin that is a number", body(origin=7), "origin"),
    ("no origin", body(origin=...), "origin"),

    ("confidence 0", body(confidence=0), ok(confidence=0)),
    ("confidence 100", body(confidence=100), ok(confidence=100)),
    ("confidence -1", body(confidence=-1), "confidence"),
    ("confidence 101", body(confidence=101), "confidence"),
    ("confidence as a string", body(confidence="50"), "confidence"),
    ("confidence as a fraction", body(confidence=50.5), "confidence"),
    ("confidence as a float", body(confidence=50.0), "confidence"),
    ("confidence as a bool", body(confidence=True), "confidence"),
    ("confidence null", body(confidence=None), "confidence"),
    ("no confidence", body(confidence=...), "confidence"),

    ("an empty line", body(line=""), "line"),
    ("a line of spaces", body(line="    "), "line"),
    ("a line of 500", body(line="x" * 500), ok(line="x" * 500)),
    ("a line of 501", body(line="x" * 501), "line"),
    ("500 after trimming", body(line="  " + "x" * 500 + "  "), ok(line="x" * 500)),
    ("a line break inside", body(line="two\nlines"), "line"),
    ("a line break at the end", body(line="one line\n"), "line"),
    ("a carriage return", body(line="one\rline"), "line"),
    ("a tab", body(line="tab\there"), "line"),
    ("a unicode line separator", body(line="one\u2028line"), "line"),
    ("a delete character", body(line="one\x7fline"), "line"),
    ("a line that is a number", body(line=5), "line"),
    ("no line", body(line=...), "line"),

    # The three columns the door sets itself are not the caller's: an `id`,
    # an `at` or a `consumed_by` sent along in the slot is dropped here, and
    # STEP_TABLE checks the row that is actually inserted.
    ("the caller's id, at and consumed_by are dropped",
     body(id="hint:mine", at="2020-01-01T00:00:00.000000Z", consumed_by="cycle:x"), ok()),
])
"#;

/// The dispatch of `step`: a message on one of the two lanes is told apart
/// by its ROUTE, and only a message on neither is read as a store answer --
/// a question whose hop happens to carry `operation` is still a question
/// (before, it was read as a store answer with no phase and ended without an
/// answer and without a dead letter). Every phase of a store answer that
/// failed ends in `store_error`, never in an empty or partial page; and the
/// inserted hint row carries the door's `id`, `at` and `consumed_by`, never
/// the caller's.
const STEP_TABLE: &str = r#"
import re

STORED_AT = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}\.[0-9]{6}Z")
ENTRY = {"argus_origin": "door", "ar_phase": "", "ar_carry": ""}
HINT = {"origin": "probe-origin", "confidence": 50, "line": "the answers got longer"}

def expect(name, thunk, want):
    global count
    count += 1
    try:
        got = thunk()
    except Exception as e:
        failures.append("step / %s: raised %r" % (name, e))
        return
    if got != want:
        failures.append("step / %s: want %r, got %r" % (name, want, got))

def run(hop, context=ENTRY, body=None):
    return ns["step"](hop, dict(context), body if body is not None else {"messages": []})

def call(out):
    # A store call the door emitted: route, phase, carry and the arguments.
    return {"route": out["header"]["route"], "phase": out["header"]["phase"],
            "carry": json.loads(out["header"]["carry"]),
            "args": json.loads(out["messages"][0]["text"])}

def answered(phase, carry, hop, rows=()):
    # The door's emissions for a store's answer in `phase`.
    body = {"messages": [{"origin": "tool", "type": "tool_result", "id": "x",
                          "text": json.dumps(list(rows))}]}
    context = {"argus_origin": "door", "ar_phase": phase, "ar_carry": json.dumps(carry)}
    return run(hop, context, body)

def question(what, **more):
    carry = {"what": what, "since": None, "limit": 20, "tag": "t"}
    carry.update(more)
    return carry

def refused(what, detail):
    return [{"header": {"route": "read", "read_tag": "t", "read_what": what,
                        "error_code": "store_error", "detail": detail},
             "messages": [], "rows": [], "next_since": ""}]

FAILED = {"operation": "select", "error_code": "unknown_column"}

# ── the lanes are told apart by route, whatever else the hop carries
expect("a read that carries `operation` is still a read",
       lambda: [(c["route"], c["args"]["operation"], c["args"]["table"])
                for c in map(call, run({"route": "in_read", "read_what": "cycles",
                                        "read_tag": "t", "operation": "select"}))],
       [("rstore", "select", "cycles")])
expect("a hint that carries `operation` is still a hint",
       lambda: [(c["route"], c["args"]["operation"], c["args"]["table"])
                for c in map(call, run({"route": "in_hint", "operation": "insert"},
                                       body={"messages": [], "hint": dict(HINT)}))],
       [("rstore", "insert", "hints")])
expect("a store answer with no phase starts nothing",
       lambda: run({"operation": "select"}), [])
expect("a message on no lane and with no operation is not answered",
       lambda: run({"route": "in_cycle"}), [])

# ── a failed store call is `store_error` in every phase, never a page
expect("the goals select failed",
       lambda: answered("goals", question("charter"), FAILED),
       refused("charter", "unknown_column"))
expect("the rules select failed after the goals came back",
       lambda: answered("rules", question("charter", goals=[{"id": "g1", "table": "goals"}]),
                        FAILED),
       refused("charter", "unknown_column"))
expect("the cycles select failed",
       lambda: answered("rows", question("cycles"), FAILED),
       refused("cycles", "unknown_column"))
expect("the hints select failed",
       lambda: answered("rows", question("hints"), FAILED),
       refused("hints", "unknown_column"))
expect("the hint insert failed",
       lambda: answered("hint", {"hint_id": "hint:abc"},
                        {"operation": "insert", "error_code": "unknown_column"}),
       [{"header": {"route": "hint_ack", "hint_id": "", "error_code": "store_error",
                    "detail": "unknown_column"}, "messages": []}])
expect("the hint insert succeeded",
       lambda: answered("hint", {"hint_id": "hint:abc"}, {"operation": "insert"}),
       [{"header": {"route": "hint_ack", "hint_id": "hint:abc", "error_code": "",
                    "detail": ""}, "messages": []}])

# ── the inserted row is the door's: its own id, its own at, no consumer
SENT = "2020-01-01T00:00:00.000000Z"
def inserted():
    out = run({"route": "in_hint"}, body={"messages": [], "hint": dict(
        HINT, id="hint:mine", at=SENT, consumed_by="cycle:x")})
    c = call(out[0])
    row = c["args"]["row"]
    return {"calls": len(out), "table": c["args"]["table"], "columns": sorted(row),
            "own id": row["id"].startswith("hint:") and row["id"] != "hint:mine",
            "own at": bool(STORED_AT.fullmatch(row["at"])) and row["at"] != SENT,
            "consumed_by": row["consumed_by"], "acked id": c["carry"]["hint_id"] == row["id"]}
expect("the caller's id, at and consumed_by never reach the row", inserted,
       {"calls": 1, "table": "hints",
        "columns": ["at", "confidence", "consumed_by", "id", "line", "origin"],
        "own id": True, "own at": True, "consumed_by": "", "acked id": True})
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
fn every_rule_of_the_read_lane_has_a_row() {
    if skip() {
        return;
    }
    let verdict = run_table(READ_TABLE);
    assert!(verdict["cases"].as_u64().unwrap_or(0) >= 40, "{verdict:#}");
}

#[test]
fn every_rule_of_the_hint_lane_has_a_row() {
    if skip() {
        return;
    }
    let verdict = run_table(HINT_TABLE);
    assert!(verdict["cases"].as_u64().unwrap_or(0) >= 35, "{verdict:#}");
}

#[test]
fn a_question_is_told_apart_by_its_route_and_a_failed_store_call_is_never_a_page() {
    if skip() {
        return;
    }
    let verdict = run_table(STEP_TABLE);
    assert!(verdict["cases"].as_u64().unwrap_or(0) >= 11, "{verdict:#}");
}

/// Rows as stored: the store has no wildcard select, so the door names its
/// columns -- and this is what keeps "every column" true. A column added to a
/// schema without the door is a projection by omission, and it fails here
/// rather than in a reader who never sees the new field.
#[test]
fn the_door_reads_every_column_its_tables_have() {
    if skip() {
        return;
    }
    let root = shipped().expect("guarded");
    let named = literal(&door_script(), "COLUMNS");
    let charter = read_json(&root.join("charter/config.json"));
    let receipts = read_json(&root.join("receipts/config.json"));
    for (table, schema) in [
        ("goals", &charter["params"]["schema"]["goals"]),
        ("rules", &charter["params"]["schema"]["rules"]),
        ("cycles", &receipts["params"]["schema"]["cycles"]),
        ("hints", &receipts["params"]["schema"]["hints"]),
    ] {
        let stored: BTreeSet<String> = schema
            .as_object()
            .unwrap_or_else(|| panic!("the store declares a `{table}` table"))
            .keys()
            .cloned()
            .collect();
        let read: BTreeSet<String> = named[table]
            .as_array()
            .unwrap_or_else(|| panic!("the door names the columns of `{table}`"))
            .iter()
            .map(|v| v.as_str().expect("a column name").to_string())
            .collect();
        assert_eq!(
            read, stored,
            "`{table}`: the door reads exactly the columns the store has"
        );
    }
    assert_eq!(
        named.as_object().map(|o| o.len()),
        Some(4),
        "four tables and no fifth: `waits` is the loop's working memory, not a record"
    );
}

/// No edge leads from the door into the loop: a hint is stored, not acted on.
/// The door reaches the two stores and the hive path, nothing else -- and
/// nothing but the hive path reaches the door.
#[test]
fn the_door_has_no_way_into_the_loop() {
    if skip() {
        return;
    }
    let hive = read_json(&shipped().expect("guarded").join("config.json"));
    let edges = hive["params"]["graph"]["edges"]
        .as_array()
        .expect("the hive has edges");
    let mut out: BTreeSet<String> = BTreeSet::new();
    let mut into: BTreeSet<String> = BTreeSet::new();
    for e in edges {
        let (from, to) = (
            e["from"].as_str().unwrap_or(""),
            e["to"].as_str().unwrap_or(""),
        );
        if from == "./door" {
            out.insert(to.to_string());
        }
        if to == "./door" {
            into.insert(from.to_string());
        }
    }
    let want_out: BTreeSet<String> = [".", "./charter", "./receipts"]
        .into_iter()
        .map(String::from)
        .collect();
    let want_in: BTreeSet<String> = [".", "./charter", "./receipts"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(
        out, want_out,
        "the door leads to its stores and out, never into the loop"
    );
    assert_eq!(
        into, want_in,
        "only the hive path and the stores' answers reach the door"
    );

    let accepts: Vec<&str> = hive["params"]["contract"]["accepts"]
        .as_array()
        .expect("accepts")
        .iter()
        .filter_map(|a| a["route"].as_str())
        .collect();
    let emits: Vec<&str> = hive["params"]["contract"]["emits"]
        .as_array()
        .expect("emits")
        .iter()
        .filter_map(|a| a["route"].as_str())
        .collect();
    for lane in ["in_read", "in_hint"] {
        assert!(
            accepts.contains(&lane),
            "the hive accepts `{lane}`: {accepts:?}"
        );
    }
    for lane in ["read", "hint_ack"] {
        assert!(emits.contains(&lane), "the hive emits `{lane}`: {emits:?}");
    }
}

// ═══════════════════════════════════════════════════════ the booted colony

const MODEL_JUDGE: &str = "judge/thinker";

/// The asker: puts one message on the lane its hop names and carries the
/// `read_*` keys and the whole body along. It exists so the lanes are entered
/// the way a colony enters them -- over an edge onto the HIVE -- rather than by
/// an injection that skips the edge.
const ASKER: &str = "import sys, json\n\
                     doc = json.load(sys.stdin)\n\
                     hop = (doc[\"envelope\"].get(\"header\") or {}).get(\"hop\") or {}\n\
                     head = {k: v for k, v in hop.items() if k == \"route\" or k.startswith(\"read_\")}\n\
                     out = {\"header\": head}\n\
                     out.update(doc[\"body\"])\n\
                     sys.stdout.write(json.dumps([out]))\n";

fn main_config() -> Value {
    json!({
        "cell": {"type": "hive"},
        "params": {"graph": {"edges": [
            {"from": "./asker", "to": "./argus",
             "condition": "has(hop.route) && (hop.route == 'in_read' || hop.route == 'in_hint' || hop.route == 'in_cycle')"},
            {"from": "./argus", "to": "/sink",
             "condition": "has(hop.route) && (hop.route == 'read' || hop.route == 'hint_ack' || hop.route == 'error')"}
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

/// The rows a seed file ships (its first line is the schema, not a row).
fn seed_rows(root: &std::path::Path, table: &str) -> Vec<Value> {
    let raw = std::fs::read_to_string(root.join(format!("charter/seed/{table}.jsonl")))
        .expect("the charter seeds its tables");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| sj::from_str::<Value>(l).expect("a seed line is json"))
        .filter(|v| v.get("id").is_some())
        .collect()
}

/// The tree: the shipped argus copied in, its clock written quiet, the
/// judge's endpoint on a mock. `break_stores` takes the column `note` out of
/// the charter's goals table and the column `line` out of the receipts'
/// hints table, so both stores refuse the door's select -- and the receipts
/// store its insert -- for real.
fn build_tree(td: &tempfile::TempDir, judge_url: &str, break_stores: bool) {
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

    // The id a growth would mint, and a cron that does not fire during a test
    // (same fixture as gh462): every cycle here is one the test asked for.
    let clock_path = root.join("main/argus/clock/config.json");
    let mut clock = read_json(&clock_path);
    clock["params"]["schedules"][0]["schedule_id"] = json!("01930000-0000-7000-8000-000000000928");
    clock["params"]["schedules"][0]["cron"] = json!("0 0 0 1 1 *");
    write_json(&clock_path, &clock);

    if break_stores {
        let receipts_path = root.join("main/argus/receipts/config.json");
        let mut receipts = read_json(&receipts_path);
        receipts["params"]["schema"]["hints"]
            .as_object_mut()
            .expect("the receipts keep a hints table")
            .remove("line")
            .expect("a hint has a line");
        write_json(&receipts_path, &receipts);

        let cfg_path = root.join("main/argus/charter/config.json");
        let mut cfg = read_json(&cfg_path);
        cfg["params"]["schema"]["goals"]
            .as_object_mut()
            .expect("the goals schema is an object")
            .remove("note")
            .expect("the shipped goals carry a note");
        write_json(&cfg_path, &cfg);
        let seed_path = root.join("main/argus/charter/seed/goals.jsonl");
        let raw = std::fs::read_to_string(&seed_path).expect("read");
        let mut lines: Vec<String> = Vec::new();
        for line in raw.lines().filter(|l| !l.trim().is_empty()) {
            let mut v: Value = sj::from_str(line).expect("json");
            if let Some(o) = v.as_object_mut() {
                o.remove("note");
                if let Some(s) = o.get_mut("schema").and_then(|s| s.as_object_mut()) {
                    s.remove("note");
                }
            }
            lines.push(v.to_string());
        }
        std::fs::write(&seed_path, lines.join("\n") + "\n").expect("write");
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

/// Put one message on the asker's lane, without waiting.
async fn send(h: &ColonyHandle, hop: Value, body: Value) {
    let hop = hop.as_object().cloned().expect("a hop is an object");
    h.send(
        MessageBuilder::new(Path::new("/asker"))
            .body(Body::Inline(body))
            .hop(hop)
            .ttl(64)
            .build(),
    )
    .await;
}

/// One question at the door, answered at the sink.
async fn ask(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<meclaw_core::Message>,
    hop: Value,
    body: Value,
) -> Answer {
    send(h, hop, body).await;
    let m = tokio::time::timeout(Duration::from_secs(30), rx.recv())
        .await
        .expect("the door answers within the failure marker")
        .expect("the capture channel stays open");
    let body = match &m.body {
        Body::Inline(v) => v.clone(),
        other => panic!("the answer carries an inline body: {other:?}"),
    };
    let answer = Answer {
        hop: m.headers.hop.clone(),
        body,
    };
    assert_ne!(
        answer.hop_str("route"),
        "error",
        "the loop reported an error instead: {:#?} {:#}",
        answer.hop,
        answer.body
    );
    answer
}

async fn read(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<meclaw_core::Message>,
    what: &str,
    tag: &str,
    extra: Value,
) -> Answer {
    let mut hop = json!({"route": "in_read", "read_what": what, "read_tag": tag});
    for (k, v) in extra.as_object().cloned().unwrap_or_default() {
        hop[k] = v;
    }
    let a = ask(h, rx, hop, json!({"messages": []})).await;
    assert_eq!(a.hop_str("route"), "read", "{:#?}", a.hop);
    assert_eq!(
        a.hop_str("read_tag"),
        tag,
        "the tag is echoed: {:#?}",
        a.hop
    );
    assert_eq!(a.hop_str("read_what"), what, "{:#?}", a.hop);
    a
}

fn ok(a: &Answer) {
    assert_eq!(a.hop_str("error_code"), "", "{:#?} {:#}", a.hop, a.body);
}

async fn hint(
    h: &ColonyHandle,
    rx: &mut mpsc::Receiver<meclaw_core::Message>,
    hint: Value,
) -> Answer {
    let a = ask(
        h,
        rx,
        json!({"route": "in_hint"}),
        json!({"messages": [], "hint": hint}),
    )
    .await;
    assert_eq!(a.hop_str("route"), "hint_ack", "{:#?}", a.hop);
    a
}

fn ids(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|r| r["id"].as_str().unwrap_or("").to_string())
        .collect()
}

/// **The proof.** Charter, cycles and hints read through the door, page by
/// page; a valid hint stored and an invalid one refused; and at the end the
/// charter and the cycles exactly as they were but for the one cycle the test
/// itself asked for -- read through the door as well.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_door_reads_the_charter_cycles_and_hints_and_takes_a_hint() {
    if skip() {
        return;
    }
    let judge = MockOpenAI::start(vec![]).await;
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &judge.base_url, false);
    let (h, mut rx) = boot(&td).await;
    let root = shipped().expect("guarded");
    let (goals, rules) = (seed_rows(&root, "goals"), seed_rows(&root, "rules"));
    let charter = read_json(&root.join("charter/config.json"));
    let receipts = read_json(&root.join("receipts/config.json"));
    let columns = |store: &Value, table: &str| -> BTreeSet<String> {
        store["params"]["schema"][table]
            .as_object()
            .expect("a schema")
            .keys()
            .cloned()
            .collect()
    };

    // ── 1. the charter: every seed row of both tables, each naming its table
    // and carrying every column. 100 because the seeds may already be as many
    // as the default page.
    let a = read(
        &h,
        &mut rx,
        "charter",
        "t-charter",
        json!({"read_limit": 100}),
    )
    .await;
    ok(&a);
    let rows = a.rows();
    assert_eq!(rows.len(), goals.len() + rules.len(), "{rows:#?}");
    for (table, seeded, store) in [("goals", &goals, &charter), ("rules", &rules, &charter)] {
        let got: Vec<&Value> = rows.iter().filter(|r| r["table"] == table).collect();
        assert_eq!(
            got.len(),
            seeded.len(),
            "every `{table}` seed row: {rows:#?}"
        );
        for r in got {
            let keys: BTreeSet<String> = r
                .as_object()
                .expect("a row is an object")
                .keys()
                .filter(|k| *k != "table")
                .cloned()
                .collect();
            assert_eq!(
                keys,
                columns(store, table),
                "a `{table}` row as stored: {r:#}"
            );
        }
    }
    let mut seeded_ids = ids(&goals);
    seeded_ids.extend(ids(&rules));
    let mut read_ids = ids(&rows);
    seeded_ids.sort();
    read_ids.sort();
    assert_eq!(read_ids, seeded_ids);
    assert_eq!(a.body["next_since"], "", "a charter has no time axis");
    let charter_rows = rows;

    // An invalid question is answered, not dropped.
    let a = read(&h, &mut rx, "cycles", "t-bad", json!({"read_limit": 101})).await;
    assert_eq!(a.hop_str("error_code"), "invalid_input", "{:#?}", a.hop);
    assert_eq!(a.hop_str("detail"), "read_limit", "{:#?}", a.hop);
    assert!(a.rows().is_empty(), "{:#}", a.body);

    // ── 2. one cycle, asked for over `in_cycle`: the seeded goals are disabled,
    // so it is an `idle` receipt. The door reads it with every column.
    let a = read(&h, &mut rx, "cycles", "t-none", json!({})).await;
    ok(&a);
    assert!(
        a.rows().is_empty(),
        "no cycle before the test asks for one: {:#}",
        a.body
    );
    assert_eq!(a.body["next_since"], "");
    send(
        &h,
        json!({"route": "in_cycle"}),
        json!({"messages": [
        {"origin": "user", "type": "text", "text": "one tick"}]}),
    )
    .await;
    let start = std::time::Instant::now();
    let cycle = loop {
        let a = read(&h, &mut rx, "cycles", "t-cycles", json!({})).await;
        ok(&a);
        if let Some(row) = a.rows().first() {
            assert_eq!(a.rows().len(), 1, "{:#}", a.body);
            assert_eq!(
                a.body["next_since"], row["at"],
                "next_since is the last row's `at`"
            );
            break row.clone();
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "the idle receipt never became readable"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(cycle["outcome"], "idle", "{cycle:#}");
    let keys: BTreeSet<String> = cycle.as_object().expect("a row").keys().cloned().collect();
    assert_eq!(
        keys,
        columns(&receipts, "cycles"),
        "a cycle as stored: {cycle:#}"
    );

    // ── 3. a valid hint: acknowledged with its id, readable with an empty
    // `consumed_by`, its line trimmed.
    let a = read(&h, &mut rx, "hints", "t-no-hints", json!({})).await;
    ok(&a);
    assert!(a.rows().is_empty(), "{:#}", a.body);
    let a = hint(
        &h,
        &mut rx,
        json!({"origin": "probe-origin", "confidence": 70, "line": "  the answers get longer  "}),
    )
    .await;
    ok(&a);
    let first = a.hop_str("hint_id").to_string();
    assert!(!first.is_empty(), "{:#?}", a.hop);
    let a = read(&h, &mut rx, "hints", "t-hints", json!({})).await;
    ok(&a);
    let rows = a.rows();
    assert_eq!(ids(&rows), vec![first.clone()], "{rows:#?}");
    let row = &rows[0];
    assert_eq!(row["consumed_by"], "", "{row:#}");
    assert_eq!(row["origin"], "probe-origin", "{row:#}");
    assert_eq!(row["confidence"], 70, "{row:#}");
    assert_eq!(row["line"], "the answers get longer", "{row:#}");
    let keys: BTreeSet<String> = row.as_object().expect("a row").keys().cloned().collect();
    assert_eq!(
        keys,
        columns(&receipts, "hints"),
        "a hint as stored: {row:#}"
    );

    // ── 4. an invalid hint: refused by name, nothing stored.
    let a = hint(
        &h,
        &mut rx,
        json!({"origin": "probe-origin", "confidence": 101, "line": "too sure"}),
    )
    .await;
    assert_eq!(a.hop_str("error_code"), "invalid_hint", "{:#?}", a.hop);
    assert_eq!(a.hop_str("detail"), "confidence", "{:#?}", a.hop);
    assert_eq!(a.hop_str("hint_id"), "", "{:#?}", a.hop);
    let a = read(&h, &mut rx, "hints", "t-hints-2", json!({})).await;
    ok(&a);
    assert_eq!(ids(&a.rows()), vec![first.clone()], "{:#}", a.body);

    // ── 5. paging: three hints, two per page, none twice.
    let mut all = vec![first.clone()];
    for line in ["a second hypothesis", "a third hypothesis"] {
        let a = hint(
            &h,
            &mut rx,
            json!({"origin": "probe-origin", "confidence": 40, "line": line}),
        )
        .await;
        ok(&a);
        all.push(a.hop_str("hint_id").to_string());
    }
    let a = read(&h, &mut rx, "hints", "t-page-1", json!({"read_limit": 2})).await;
    ok(&a);
    let page = a.rows();
    assert_eq!(
        ids(&page),
        all[..2].to_vec(),
        "the two oldest first: {page:#?}"
    );
    assert_eq!(a.body["next_since"], page[1]["at"], "{:#}", a.body);
    let mut hint_rows = page;
    let since = a.body["next_since"].clone();
    let a = read(
        &h,
        &mut rx,
        "hints",
        "t-page-2",
        json!({"read_limit": 2, "read_since": since}),
    )
    .await;
    ok(&a);
    let page = a.rows();
    assert_eq!(
        ids(&page),
        all[2..].to_vec(),
        "the rest, none twice: {page:#?}"
    );
    assert_eq!(a.body["next_since"], page[0]["at"], "{:#}", a.body);
    hint_rows.extend(page);

    // ── 6. nothing the door did wrote the charter, a cycle or a hint: every
    // row EQUAL to what was read before (a count would miss an update), the
    // one cycle the test asked for, and no model asked anything.
    let a = read(
        &h,
        &mut rx,
        "charter",
        "t-charter-after",
        json!({"read_limit": 100}),
    )
    .await;
    ok(&a);
    assert_eq!(a.rows(), charter_rows, "the charter as first read");
    let a = read(
        &h,
        &mut rx,
        "cycles",
        "t-cycles-after",
        json!({"read_limit": 100}),
    )
    .await;
    ok(&a);
    assert_eq!(a.rows(), vec![cycle], "the one cycle, unchanged");
    let a = read(
        &h,
        &mut rx,
        "hints",
        "t-hints-after",
        json!({"read_limit": 100}),
    )
    .await;
    ok(&a);
    assert_eq!(a.rows(), hint_rows, "the three hints as first read");
    assert_eq!(
        judge.recorded_requests().await.len(),
        0,
        "a hint and a read reach no model"
    );
    let dl = h.drain_dead_letters().await;
    assert!(dl.is_empty(), "and nothing was lost on the way: {dl:#?}");
    h.shutdown().await;
}

/// A store that refuses the call is a `store_error` answer: a refused select
/// is never an empty page ("there is nothing" and "the read failed" must not
/// look alike), and a refused insert is never an acknowledged id. Both stores
/// are broken for real; the cycles table, untouched, still reads.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_store_call_is_a_store_error_never_a_page_or_an_id() {
    if skip() {
        return;
    }
    let judge = MockOpenAI::start(vec![]).await;
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td, &judge.base_url, true);
    let (h, mut rx) = boot(&td).await;

    for what in ["charter", "hints"] {
        let a = read(&h, &mut rx, what, "t-refused", json!({})).await;
        assert_eq!(
            a.hop_str("error_code"),
            "store_error",
            "`{what}`: {:#?} {:#}",
            a.hop,
            a.body
        );
        assert!(
            !a.hop_str("detail").is_empty(),
            "`{what}`: the store's reason travels: {:#?}",
            a.hop
        );
        assert!(a.rows().is_empty(), "`{what}`: {:#}", a.body);
        assert_eq!(a.body["next_since"], "", "`{what}`");
    }

    let a = hint(
        &h,
        &mut rx,
        json!({"origin": "probe-origin", "confidence": 50, "line": "kept nowhere"}),
    )
    .await;
    assert_eq!(a.hop_str("error_code"), "store_error", "{:#?}", a.hop);
    assert!(
        !a.hop_str("detail").is_empty(),
        "the store's reason travels: {:#?}",
        a.hop
    );
    assert_eq!(
        a.hop_str("hint_id"),
        "",
        "no id for a hint that was not kept"
    );

    let a = read(&h, &mut rx, "cycles", "t-intact", json!({})).await;
    ok(&a);
    assert!(a.rows().is_empty(), "{:#}", a.body);

    let dl = h.drain_dead_letters().await;
    assert!(dl.is_empty(), "{dl:#?}");
    h.shutdown().await;
}
