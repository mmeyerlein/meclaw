//! GH #1376 -- the argus takes a cell's `error` output as a hint.
//!
//! Before #1376 an `error` message could not reach the argus as a hint: the
//! only report lane, `in_hint`, wants the body slot `hint: {origin, confidence,
//! line}`, and an `error` carries `{messages: [], meta: {detail}}`, so an edge
//! from an error path was refused `invalid_hint`, and an `error` nobody drew
//! ended as `hive_no_route` in the dead-letter queue. The new lane `in_error`
//! takes the error as it is and stores it the way `in_hint` stores a hint:
//!
//! ```text
//!   in_error -> door -> receipts (insert into `hints`, `if_absent` on a slot id) -> door
//! ```
//!
//! The hint is `{origin: "error:<code>", confidence, line: "<sender>: <detail>"}`.
//! `<code>` is an identifier (`meta.provider_code`, else `hop.error_code`,
//! `[A-Za-z0-9_.-]{1,64}`, anything else `invalid_code`). A burst is throttled
//! per that code and the sender: at most `error_hint_max_per_window` hints per
//! `error_hint_window_ms` of the door's clock, and the throttle never swallows
//! the first one of a key (a store that refuses the insert loses it, answered
//! by nothing -- the lane is fire-and-forget). The throttle holds
//! no state of its own -- the door is a code cell and keeps nothing between
//! messages -- so the window's slots are rows: each error tries slot 0, 1, ...
//! with an `if_absent` insert, and the store, which runs one statement at a
//! time, decides which error gets which slot. That holds while the door runs
//! several messages at once (`max_concurrency`), which is exactly a burst.
//!
//! The pure layer runs the SHIPPED `script_inline` under python3 with an
//! injected clock and a store double that keeps `hints` and honours
//! `if_absent` as the store does (one row matching the guard = nothing
//! written, `rows_affected` 0). A burst is delivered the way it arrives: every
//! error reaches the door before the first store answer comes back.
//!
//! Above the pure layer, one test boots the shipped argus in a colony and
//! sends it a real `error` over an edge the test draws, so the cell contract,
//! the hive's edges and the real store are in the proof as well.
//!
//! Guarded like every template-reading test (GH #49): a tree without the
//! template skips.

use meclaw_cells::TimerCellFactory;
use meclaw_cells::code::CodeCellFactory;
use meclaw_cells::llm::LlmCellFactory;
use meclaw_cells::store::StoreCellFactory;
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{self as sj, Value, json};
use meclaw_core::{Body, MessageBuilder, Path};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::collections::BTreeSet;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use tokio::sync::mpsc;

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

fn door() -> Value {
    read_json(&repo("templates/argus/door/config.json"))
}

fn door_script() -> String {
    door()["params"]["script_inline"]
        .as_str()
        .expect("the door is a code cell with an inline script")
        .to_string()
}

/// Run a program under python3, the program itself on stdin (a single argv
/// string is capped at 128 KiB; same harness as the door's GH #928 tables).
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

/// The shipped script as a module, a fixed `uuid` and `now` (so outputs are
/// bytes, not samples), a store double for `hints`, and the delivery of a
/// burst in arrival order.
const DRIVER: &str = r#"
import json, sys, datetime
ns = {"__name__": "gh1376"}
exec(compile(SCRIPT, "door", "exec"), ns)

class _Uuid:
    n = 0
    @classmethod
    def uuid4(cls):
        cls.n += 1
        class _V:
            pass
        v = _V()
        v.hex = "%032x" % cls.n
        return v

ns["uuid"] = _Uuid
ns["now"] = lambda: "2026-11-04T12:00:00.000000Z"

HINTS = []

def _matches(row, where):
    return all(row.get(k) == v for k, v in where.items())

def store_answer(args):
    # The store's answer to one operation: `hop` and the rows of its turn.
    op = args.get("operation")
    if op == "insert" and args.get("table") == "hints":
        guard = args.get("if_absent")
        if isinstance(guard, dict) and guard and any(_matches(r, guard) for r in HINTS):
            return {"operation": "insert", "rows_affected": 0}, []
        HINTS.append(dict(args["row"]))
        return {"operation": "insert", "rows_affected": 1}, []
    if op == "select":
        rows = [dict(r) for r in HINTS] if args.get("table") == "hints" else []
        return {"operation": "select", "rows_affected": len(rows)}, rows
    return {"operation": op or "error", "error_code": "invalid_input"}, []

def deliver(pending, params, clock):
    # The hive's edges ./door -> ./receipts -> ./door, in arrival order: the
    # answer travels back with `ar_phase`/`ar_carry` in the context.
    out = []
    while pending:
        m = pending.pop(0)
        head = m.get("header") or {}
        if head.get("route") in ("rstore", "cstore"):
            hop, rows = store_answer(json.loads(m["messages"][0]["text"]))
            ctx = {"argus_origin": "door", "ar_phase": head.get("phase", ""),
                   "ar_carry": head.get("carry", "")}
            body = {"messages": [{"origin": "tool", "type": "tool_result",
                                  "text": json.dumps(rows)}]}
            pending.extend(ns["step"](hop, ctx, body, params, clock))
        else:
            out.append(m)
    return out

def burst(messages, params, clock):
    # Every message reaches the door before the first store answer does.
    pending = []
    for hop, ctx, body in messages:
        pending.extend(ns["step"](hop, ctx, body, params, clock))
    return deliver(pending, params, clock)

def error(code, sender, detail, where="hop"):
    hop = {"route": "in_error", "error_code": code, "finish_reason": "error"}
    ctx = {"argus_origin": "door", "ar_phase": "", "ar_carry": ""}
    (hop if where == "hop" else ctx)["error_sender"] = sender
    return hop, ctx, {"messages": [], "meta": {"detail": detail}}

BASE = 1800000000000  # a whole minute: 30 000 000 windows of 60 s
T = [BASE]
clock = lambda: T[0]

def at_of(ms):
    return ns["stamp"](datetime.datetime.fromtimestamp(ms / 1000, datetime.timezone.utc))
"#;

fn run(program: &str) {
    let full = format!(
        "SCRIPT = {}\n{DRIVER}\n{program}\nprint('ok')\n",
        sj::to_string(&door_script()).expect("the script serialises")
    );
    let out = run_python(&full);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success() && stdout.trim_end().ends_with("ok"),
        "the program failed:\nstdout: {stdout}\nstderr: {stderr}"
    );
}

/// Pin 1: one error, one stored hint in the form of an `in_hint` hint, and
/// nothing on the way out -- the lane is fire-and-forget, an answer would
/// dead-letter at every parent that drew only the error edge.
#[test]
fn an_error_message_becomes_one_stored_hint() {
    if skip() {
        return;
    }
    run(r#"
out = burst([error("duplex_failed", "voice", "duplex protocol error: no credits")], {}, clock)
assert len(HINTS) == 1, HINTS
h = HINTS[0]
assert sorted(h) == ["at", "confidence", "consumed_by", "id", "line", "origin"], h
assert h["origin"] == "error:duplex_failed", h
assert h["line"] == "voice: duplex protocol error: no credits", h
assert h["consumed_by"] == "", h
assert h["at"] == at_of(BASE), (h, at_of(BASE))
assert h["id"].startswith("hint:"), h
c = h["confidence"]
assert isinstance(c, int) and not isinstance(c, bool) and 0 <= c <= 100, h
# the stored hint leaves once, on `error_hint`, as stored (OR-D53-85 b)
assert out == [{"header": {"route": "error_hint"}, "messages": [],
                "hint": {"origin": h["origin"], "confidence": c, "line": h["line"]}}], out

# the line is one an `in_hint` would take: one line, trimmed, not empty
line_ok = lambda line: ns["parse_hint"]({"hint": {"origin": "x", "confidence": c, "line": line}})[1] is None
assert line_ok(h["line"]), h

# the sender may come from the context as well (an edge's `set_context`)
HINTS.clear()
burst([error("store_degraded", "memory", "cannot open", where="context")], {}, clock)
assert [x["line"] for x in HINTS] == ["memory: cannot open"], HINTS
assert HINTS[0]["origin"] == "error:store_degraded", HINTS

# a detail with line breaks is folded to ONE line and kept whole: no fixed
# character cap (OR-D53-85 c, R-IG-1 -- a cap on content is the curator's)
HINTS.clear()
burst([error("script_failed", "affinity", "Traceback:\n  line 1\r\n " + "x" * 2000)], {}, clock)
assert len(HINTS) == 1, HINTS
assert HINTS[0]["line"] == "affinity: Traceback: line 1 " + "x" * 2000, HINTS[0]["line"][:80]
"#);
}

/// Pin 2: a burst is throttled per key to the window's maximum, the first
/// error of a key is never swallowed, a new window lets hints through again,
/// and the shipped defaults are 60 s and 3.
#[test]
fn a_burst_of_errors_is_throttled_to_the_window_max() {
    if skip() {
        return;
    }
    let params = &door()["params"];
    assert_eq!(params["error_hint_window_ms"], 60000, "{params:#}");
    assert_eq!(params["error_hint_max_per_window"], 3, "{params:#}");
    run(r#"
P = {"error_hint_window_ms": 60000, "error_hint_max_per_window": 3}
lines = lambda: [x["line"] for x in HINTS]

# ten at once: the first three, in arrival order, and one `error_hint` out
# for each stored one -- the throttled seven send nothing
out = burst([error("duplex_failed", "voice", "no credits %d" % i) for i in range(10)], P, clock)
assert lines() == ["voice: no credits 0", "voice: no credits 1", "voice: no credits 2"], lines()
assert [m["header"]["route"] for m in out] == ["error_hint"] * 3, out
assert [m["hint"]["line"] for m in out] == lines(), out

# the key is code AND sender: another sender, another code -- each its own first
burst([error("duplex_failed", "talky", "a"), error("timeout", "voice", "b")], P, clock)
assert lines()[3:] == ["talky: a", "voice: b"], lines()

# later in the same window: nothing more for the full key
T[0] = BASE + 59999
out = burst([error("duplex_failed", "voice", "late")], P, clock)
assert len(HINTS) == 5, lines()
assert out == [], out

# the next window lets the key through again, up to the max
T[0] = BASE + 60000
burst([error("duplex_failed", "voice", "next %d" % i) for i in range(5)], P, clock)
assert lines()[5:] == ["voice: next 0", "voice: next 1", "voice: next 2"], lines()

# without params: the defaults
HINTS.clear(); T[0] = BASE
burst([error("duplex_failed", "voice", "d%d" % i) for i in range(6)], {}, clock)
assert len(HINTS) == 3, lines()

# a max of 0 (or junk) never swallows the first; a junk window is the default
for p, want in (({"error_hint_max_per_window": 0}, 1), ({"error_hint_max_per_window": -2}, 1),
                ({"error_hint_max_per_window": "x"}, 3),
                ({"error_hint_window_ms": "", "error_hint_max_per_window": None}, 3)):
    HINTS.clear(); T[0] = BASE
    burst([error("duplex_failed", "voice", "e%d" % i) for i in range(4)], p, clock)
    assert len(HINTS) == want, (p, lines())
    assert lines()[0] == "voice: e0", (p, lines())

# a smaller window and max: 2 per 1000 ms
HINTS.clear(); T[0] = BASE
Q = {"error_hint_window_ms": 1000, "error_hint_max_per_window": 2}
burst([error("x", "voice", "q%d" % i) for i in range(4)], Q, clock)
T[0] = BASE + 1000
burst([error("x", "voice", "r%d" % i) for i in range(4)], Q, clock)
assert lines() == ["voice: q0", "voice: q1", "voice: r0", "voice: r1"], lines()
"#);
}

/// The inputs of every lane the door had before #1376, and the store answers
/// it resumes from. Their outputs are the golden: without an edge onto
/// `in_error` the door answers each of them byte for byte as before.
const GOLDEN_CASES: &str = r#"
G = json.dumps
CASES = [
    ("read charter", {"route": "in_read", "read_what": "charter", "read_tag": "t"}, {}, {"messages": []}),
    ("read cycles since", {"route": "in_read", "read_what": "cycles", "read_tag": "t",
                           "read_since": "2026-11-01T00:00:00Z", "read_limit": 5}, {}, {"messages": []}),
    ("read hints", {"route": "in_read", "read_what": "hints", "read_tag": "h"}, {}, {"messages": []}),
    ("read refused", {"route": "in_read", "read_what": "goals", "read_tag": "t"}, {}, {"messages": []}),
    ("hint taken", {"route": "in_hint"}, {}, {"messages": [], "hint": {"origin": "probe", "confidence": 70, "line": " a line "}}),
    ("hint refused", {"route": "in_hint"}, {}, {"messages": [], "hint": {"origin": "Bad Origin", "confidence": 70, "line": "x"}}),
    ("an error no edge restamped", {"route": "error", "error_code": "duplex_failed", "finish_reason": "error"}, {},
     {"messages": [], "meta": {"detail": "no credits"}}),
    ("no lane, no store answer", {}, {}, {"messages": []}),
    ("goals page", {"operation": "select"}, {"ar_phase": "goals", "ar_carry": G({"what": "charter", "tag": "t", "limit": 2})},
     {"messages": [{"origin": "tool", "type": "tool_result", "text": G([{"id": "g1"}, {"id": "g2"}, {"id": "g3"}])}]}),
    ("rules page", {"operation": "select"}, {"ar_phase": "rules", "ar_carry": G({"what": "charter", "tag": "t", "limit": 5, "goals": [{"id": "g1", "table": "goals"}]})},
     {"messages": [{"origin": "tool", "type": "tool_result", "text": G([{"id": "r1"}])}]}),
    ("rows page", {"operation": "select"}, {"ar_phase": "rows", "ar_carry": G({"what": "hints", "tag": "h", "limit": 20})},
     {"messages": [{"origin": "tool", "type": "tool_result", "text": G([{"id": "hint:1", "at": "2026-11-04T12:00:00.000000Z"}])}]}),
    ("read failed", {"operation": "select", "error_code": "unknown_column"}, {"ar_phase": "rows", "ar_carry": G({"what": "cycles", "tag": "t"})},
     {"messages": []}),
    ("hint stored", {"operation": "insert"}, {"ar_phase": "hint", "ar_carry": G({"hint_id": "hint:abc"})}, {"messages": []}),
    ("hint store failed", {"operation": "insert", "error_code": "unknown_column"}, {"ar_phase": "hint", "ar_carry": G({"hint_id": "hint:abc"})}, {"messages": []}),
    ("unknown phase", {"operation": "insert"}, {"ar_phase": "other", "ar_carry": "{}"}, {"messages": []}),
]
GOT = {}
for name, hop, ctx, body in CASES:
    _Uuid.n = 0
    GOT[name] = ns["step"](hop, ctx, body)
"#;

fn golden_path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/gh1376_door_without_in_error.json")
}

/// Whether `dir` is an argus hive: the template itself or a `ref` onto it.
fn is_argus_hive(dir: &std::path::Path) -> bool {
    let Ok(dir) = dir.canonicalize() else {
        return false;
    };
    if repo("templates/argus")
        .canonicalize()
        .is_ok_and(|argus| argus == dir)
    {
        return true;
    }
    let cfg = dir.join("config.json");
    cfg.is_file()
        && read_json(&cfg)["cell"]["template"]
            .as_str()
            .is_some_and(|t| t == "argus" || t.starts_with("argus@"))
}

/// A level's rim door onto its argus: `from: "."`, the lane's own condition
/// and nothing else. It is no producer: it forwards only what reaches that
/// level's own rim, as it is, and the substrate's door check demands it for
/// every lane the level declares (`meclaw-os` carries `in_error` like
/// `in_hint`, gh302). An edge that also rewrites (a `modifier` turning an
/// arriving `error` into `in_error`, say) or routes on anything else IS a
/// producer and counts (mini review n1, GH #1376).
fn is_rim_door(e: &Value) -> bool {
    let keys: Vec<&str> = e
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    e["from"] == "."
        && e["condition"] == "has(hop.route) && hop.route == 'in_error'"
        && keys
            .iter()
            .all(|k| matches!(*k, "from" | "to" | "condition"))
}

/// The rim door exception of pin 3 is exactly the door: the same edge with a
/// rewriting modifier is a producer (mini review n1, GH #1376).
#[test]
fn a_rim_edge_that_rewrites_is_a_producer() {
    let door = json!({
        "from": ".", "to": "./argus",
        "condition": "has(hop.route) && hop.route == 'in_error'"
    });
    assert!(is_rim_door(&door), "{door}");
    let mut rewriting = door.clone();
    rewriting["modifier"] = json!({"set_hop": {"route": "in_error"}});
    assert!(!is_rim_door(&rewriting), "{rewriting}");
    let mut other = door.clone();
    other["condition"] = json!("has(hop.error_code)");
    assert!(!is_rim_door(&other), "{other}");
}

/// Pin 3: without the edge nothing changes. No shipped template draws an
/// edge onto an argus's `in_error` but the argus's own entry edge to its
/// door (another hive's lane of the same name is not the argus's), and the
/// door answers every input of its older lanes exactly as before #1376 (the
/// golden was written by the door of master before the lane existed).
#[test]
fn without_the_edge_nothing_changes() {
    if skip() {
        return;
    }
    // (1) nothing is wired by default
    let mut drawn = Vec::new();
    let mut stack = vec![repo("templates")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("the templates are readable") {
            let p = entry.expect("directory entry").path();
            if p.is_dir() {
                stack.push(p);
            } else if p.file_name().is_some_and(|n| n == "config.json") {
                let cfg = read_json(&p);
                let dir = p.parent().expect("a config has a directory");
                for e in cfg["params"]["graph"]["edges"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                {
                    // Only the argus's lane counts: an edge inside an argus or
                    // onto one. Another hive may run a lane of its own under
                    // the same name (the summarizer's internal `in_error`).
                    let to = e["to"].as_str().unwrap_or_default();
                    let at_argus = is_argus_hive(dir)
                        || to.rsplit('/').next() == Some("argus")
                        || (!to.starts_with('/') && is_argus_hive(&dir.join(to)));
                    let rim_door = !is_argus_hive(dir) && is_rim_door(&e);
                    if at_argus && !rim_door && e.to_string().contains("in_error") {
                        drawn.push((p.clone(), e));
                    }
                }
            }
        }
    }
    let argus = repo("templates/argus/config.json");
    assert_eq!(drawn.len(), 1, "one edge names `in_error`: {drawn:#?}");
    assert_eq!(drawn[0].0, argus, "{drawn:#?}");
    assert_eq!(drawn[0].1["from"], ".", "{drawn:#?}");
    assert_eq!(drawn[0].1["to"], "./door", "{drawn:#?}");

    // (2) the door's older lanes, byte for byte
    let program = format!(
        "SCRIPT = {}\n{DRIVER}\n{GOLDEN_CASES}\nprint(json.dumps(GOT, sort_keys=True))\n",
        sj::to_string(&door_script()).expect("the script serialises")
    );
    let out = run_python(&program);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "the cases did not run:\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got: Value = sj::from_str(stdout.trim()).expect("the outputs are json");
    // The repo's one golden switch, not a GH1376 one: only the switches in
    // `scripts/pipeline/lane.py` GOLDEN_SWITCHES travel to a lane run.
    if std::env::var("GH277_WRITE_GOLDEN").as_deref() == Ok("1") {
        std::fs::write(
            golden_path(),
            sj::to_string_pretty(&got).expect("serialise") + "\n",
        )
        .expect("write the golden");
        return;
    }
    let want = read_json(&golden_path());
    assert_eq!(
        got, want,
        "the door answers its older lanes as before #1376"
    );
}

/// Pin 4: the line runs through the redaction of URLs the cell outputs
/// (`web_fetch`, GH #1375): a credential in `detail` never reaches the hint.
#[test]
fn a_secret_in_the_detail_reaches_the_hint_redacted() {
    if skip() {
        return;
    }
    run(r#"
detail = ("duplex start failed at "
          "wss://svc:hunter2-pass@api.example.com/v1/realtime?model=m1&api_key=sk-live-777#access_token=tok-999 "
          "after 3 tries")
burst([error("duplex_failed", "voice", detail)], {}, clock)
assert len(HINTS) == 1, HINTS
line = HINTS[0]["line"]
for secret in ("hunter2-pass", "sk-live-777", "tok-999", "svc:"):
    assert secret not in line, (secret, line)
for kept in ("voice: duplex start failed at", "api.example.com/v1/realtime", "model=m1", "after 3 tries"):
    assert kept in line, (kept, line)
assert line == ("voice: duplex start failed at "
                "wss://REDACTED@api.example.com/v1/realtime?model=m1&api_key=REDACTED#access_token=REDACTED "
                "after 3 tries"), line

# what the port cannot read the way the URL parser does goes whole
HINTS.clear()
burst([error("x", "voice", "at https://u-mock:pw-mock@h.example:99999/ and http://h.example/\u00fc?token=t-mock")], {}, clock)
assert HINTS[0]["line"] == "voice: at <url redacted> and <url redacted>", HINTS[0]["line"]
"#);
}

fn parity_table() -> String {
    std::fs::read_to_string(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/gh1376_redact_url_parity.json"),
    )
    .expect("the parity table is readable")
}

/// Pin 4, the lock: the door's port answers the shared table exactly as
/// `web_fetch::redact_url` does (the Rust half is
/// `web_fetch::tests::gh1376_redact_url_matches_the_shared_parity_table`),
/// and its word lists are the Rust constants word for word.
#[test]
fn the_door_redacts_a_url_as_web_fetch_does() {
    if skip() {
        return;
    }
    run(&format!(
        "TABLE = json.loads({})\n{}",
        sj::to_string(&parity_table()).expect("the table serialises"),
        r#"
assert list(ns["STRONG_SECRET_WORDS"]) == TABLE["strong_secret_words"], ns["STRONG_SECRET_WORDS"]
assert list(ns["WEAK_SECRET_WORDS"]) == TABLE["weak_secret_words"], ns["WEAK_SECRET_WORDS"]
assert ns["REDACTED"] == TABLE["redacted"]
assert ns["REDACTED_WHOLE"] == TABLE["redacted_whole"]
assert len(TABLE["cases"]) >= 20
for case in TABLE["cases"]:
    got = ns["redact_url"](case["in"])
    assert got == case["out"], (case["why"], case["in"], got)
"#
    ));
}

/// A provider's own code names the origin when the error carries one
/// (OR-D53-85 a): `meta.provider_code` before `hop.error_code`, and the
/// throttle keys on the code the origin names.
#[test]
fn a_provider_code_names_the_origin() {
    if skip() {
        return;
    }
    run(r#"
def perr(code, provider, sender, detail):
    hop, ctx, body = error(code, sender, detail)
    body["meta"]["provider_code"] = provider
    return hop, ctx, body

out = burst([perr("duplex_failed", "insufficient_credits", "voice", "no credits")], {}, clock)
assert [h["origin"] for h in HINTS] == ["error:insufficient_credits"], HINTS
assert out[0]["hint"]["origin"] == "error:insufficient_credits", out

# without one (absent, blank, null) the error code
HINTS.clear()
burst([perr("duplex_failed", "", "voice", "a"), perr("timeout", None, "voice", "b"),
       error("store_degraded", "memory", "c")], {}, clock)
assert [h["origin"] for h in HINTS] == ["error:duplex_failed", "error:timeout", "error:store_degraded"], HINTS

# the throttle keys on the origin's code: a full window of one provider code
# does not swallow another under the same error code
HINTS.clear()
P = {"error_hint_max_per_window": 2}
burst([perr("duplex_failed", "no_credits", "voice", "n%d" % i) for i in range(4)]
      + [perr("duplex_failed", "rate_limited", "voice", "r")], P, clock)
assert sorted((h["origin"], h["line"]) for h in HINTS) == [
    ("error:no_credits", "voice: n0"), ("error:no_credits", "voice: n1"),
    ("error:rate_limited", "voice: r")], HINTS
"#);
}

/// The output lane `error_hint` (OR-D53-85 b): every STORED error hint leaves
/// the hive once, body `{messages: [], hint: {origin, confidence, line}}` as
/// stored; a throttled error sends nothing. The hive lets it out and names it
/// in its contract, and nothing draws it by default.
#[test]
fn a_stored_error_hint_goes_out_once_a_throttled_one_not_at_all() {
    if skip() {
        return;
    }
    let hive = read_json(&repo("templates/argus/config.json"));
    let emits = hive["params"]["contract"]["emits"]
        .as_array()
        .expect("the hive declares what it emits");
    assert!(
        emits.iter().any(|e| e["route"] == "error_hint"),
        "the hive names `error_hint` in contract.emits"
    );
    let out_edges: Vec<&Value> = hive["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .filter(|e| e["from"] == "./door" && e["to"] == ".")
        .collect();
    assert!(
        out_edges.iter().any(|e| e["condition"]
            .as_str()
            .unwrap_or_default()
            .contains("'error_hint'")),
        "an edge lets `error_hint` out of the hive: {out_edges:#?}"
    );
    run(r#"
P = {"error_hint_max_per_window": 3}
out = burst([error("duplex_failed", "voice", "e%d" % i) for i in range(5)], P, clock)
assert len(HINTS) == 3, HINTS
want = [{"header": {"route": "error_hint"}, "messages": [],
         "hint": {"origin": h["origin"], "confidence": h["confidence"], "line": h["line"]}}
        for h in HINTS]
assert out == want, out

# throttled: nothing stored, nothing out
out = burst([error("duplex_failed", "voice", "late")], P, clock)
assert len(HINTS) == 3 and out == [], (HINTS, out)

# a store that refuses the insert: nothing out either
carry = {"code": "c", "sender": "s", "line": "s: x", "at": "t", "window": 1, "slot": 0, "slots": 3}
assert ns["step"]({"operation": "insert", "error_code": "store_error"},
                  {"ar_phase": "error", "ar_carry": json.dumps(carry)}, {}, {}, clock) == []
"#);
}

/// Every message the door can put out, over all its lanes: the older lanes'
/// golden inputs, an error stored, throttled and refused, a hint and a read
/// through the store double. Printed as one JSON list.
const EVERY_OUTPUT: &str = r#"
REC = []
_step = ns["step"]
def _recording(*a, **k):
    out = _step(*a, **k)
    REC.extend(out)
    return out
ns["step"] = _recording
"#;

/// C1 of the review: the door's outputs keep the door's OWN contract. The
/// `code` cell checks every emit against `contract.emits` and turns a
/// violation into `contract_violation` with nothing passed on, so a route the
/// contract does not name never leaves the cell. Each output here runs through
/// the compiled contract as the cell checks it (after the standard headers
/// the cell injects), and the routes seen are exactly the declared ones -- a
/// new route without a case here is red as well.
#[test]
fn every_output_of_the_door_keeps_its_declared_contract() {
    if skip() {
        return;
    }
    let program = format!(
        "SCRIPT = {}\n{DRIVER}\n{EVERY_OUTPUT}\n{GOLDEN_CASES}\n{}\nprint(json.dumps(REC))\n",
        sj::to_string(&door_script()).expect("the script serialises"),
        r#"
HINTS.clear()
P = {"error_hint_max_per_window": 2}
burst([error("duplex_failed", "voice", "no credits %d" % i) for i in range(3)], P, clock)
hop, ctx, body = error("duplex_failed", "voice", "x")
body["meta"]["provider_code"] = "insufficient_credits"
burst([(hop, ctx, body)], P, clock)
carry = {"code": "c", "sender": "s", "line": "s: x", "at": "t", "window": 1, "slot": 0, "slots": 3}
ns["step"]({"operation": "insert", "error_code": "store_error"},
           {"ar_phase": "error", "ar_carry": json.dumps(carry)}, {}, {}, clock)
burst([({"route": "in_hint"}, {}, {"messages": [], "hint": {"origin": "probe", "confidence": 5, "line": "l"}}),
       ({"route": "in_read", "read_what": "hints", "read_tag": "t"}, {}, {"messages": []})], {}, clock)
"#
    );
    let out = run_python(&program);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        out.status.success(),
        "the cases did not run:\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let outputs: Vec<Value> = sj::from_str(stdout.trim()).expect("the outputs are json");
    let contract = door()["contract"]["emits"].clone();
    let declared: BTreeSet<String> = contract["hop"]["route"]["values"]
        .as_array()
        .expect("the door names its routes")
        .iter()
        .map(|v| v.as_str().unwrap_or_default().to_string())
        .collect();
    let block: meclaw_core::EmitsBlock =
        sj::from_value(contract).expect("the door's emits block parses");
    let compiled =
        meclaw_core::CompiledEmits::compile(&block).expect("the door's emits block compiles");
    let mut seen = BTreeSet::new();
    for mut m in outputs {
        // as `code/cell.rs` inject_standard_headers does before its check
        let head = m["header"]
            .as_object_mut()
            .expect("every output carries a header");
        head.insert("exit_code".into(), json!(0));
        head.insert("duration_ms".into(), json!(1));
        head.insert("had_stderr".into(), json!(false));
        seen.insert(
            m["header"]["route"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
        if let Err(reason) = meclaw_core::validate_emits(&m, &compiled) {
            panic!("the door puts out what its contract refuses: {reason}\n{m:#}");
        }
    }
    assert_eq!(
        seen, declared,
        "every declared route has a case, and no other is put out"
    );
    assert_eq!(
        door()["contract"]["emits"]["body"]["hint"]["type"],
        "object",
        "`error_hint` carries `hint` and the contract names it"
    );
}

/// I3 of the review: the code in the origin is an identifier, never foreign
/// text. `[A-Za-z0-9_.-]{1,64}` (no `:` -- it is the origin's own separator);
/// a code outside it is `invalid_code`, fail-closed, in the origin, the line,
/// the throttle key and the carry alike.
#[test]
fn a_code_is_an_identifier_or_invalid_code() {
    if skip() {
        return;
    }
    run(r#"
def perr(code, provider, detail="d"):
    hop, ctx, body = error(code, "voice", detail)
    if provider is not None:
        body["meta"]["provider_code"] = provider
    return hop, ctx, body

probe = perr("duplex_failed", "https://u:pw@h.example")
first = ns["step"](*probe, {}, clock)
assert "pw" not in json.dumps(first), first
burst([probe], {}, clock)
assert [h["origin"] for h in HINTS] == ["error:invalid_code"], HINTS
assert "pw" not in json.dumps(HINTS), HINTS

for code, provider, want in (
        ("duplex_failed", "insufficient_quota", "insufficient_quota"),
        ("rate.limit-2", None, "rate.limit-2"),
        ("Timeout_X", "", "Timeout_X"),
        ("bad code", None, "invalid_code"),
        ("x", "naïve", "invalid_code"),
        ("x", "a:b", "invalid_code"),
        # review m3: over 64 is refused, never cut -- the contract's form
        ("x", "q" * 64, "q" * 64),
        ("x", "q" * 65, "invalid_code"),
        ("x", "q" * 80, "invalid_code"),
        # review m2: a provider code that is not a string is no identifier
        ("x", True, "invalid_code"),
        ("x", 42, "invalid_code"),
        ("x", ["a"], "invalid_code")):
    HINTS.clear()
    T[0] += 60000
    burst([perr(code, provider)], {}, clock)
    assert [h["origin"] for h in HINTS] == ["error:" + want], (code, provider, HINTS)
"#);
}

/// M5 of the review: a URL in running text ends before the punctuation that
/// ends the sentence or the quoted JSON around it, so a redacted value no
/// longer takes `, ` or `"}` with it.
#[test]
fn the_redaction_keeps_the_punctuation_after_a_url() {
    if skip() {
        return;
    }
    run(r#"
burst([error("x", "voice",
             'at https://h.example/?api_key=sk-1, then {"url":"https://h.example/?token=t-2"} '
             'and (https://u:p@h.example/x).')], {}, clock)
line = HINTS[0]["line"]
assert line == ('voice: at https://h.example/?api_key=REDACTED, then '
                '{"url":"https://h.example/?token=REDACTED"} and (https://REDACTED@h.example/x).'), line
"#);
}

/// M6 of the review: a detail of blanks only is no detail -- the line falls
/// back to the code after folding, not before.
#[test]
fn a_blank_detail_falls_back_to_the_code() {
    if skip() {
        return;
    }
    run(r#"
burst([error("duplex_failed", "voice", "  \n\t ")], {}, clock)
burst([error("timeout", "voice", None)], {}, clock)
assert [h["line"] for h in HINTS] == ["voice: duplex_failed", "voice: timeout"], HINTS
"#);
}

/// M3 of the review: the throttle never swallows the first error of a
/// window, but a store that refuses the insert loses it, answered by nothing.
/// Wherever the docs promise the first, they name that exception too.
#[test]
fn the_docs_name_the_refused_insert_where_they_promise_the_first_error() {
    if skip() {
        return;
    }
    let hive = read_json(&repo("templates/argus/config.json"));
    let accepts = hive["params"]["contract"]["accepts"]
        .as_array()
        .expect("accepts")
        .iter()
        .find(|a| a["route"] == "in_error")
        .expect("the hive accepts `in_error`")["because"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let readme = std::fs::read_to_string(repo("templates/argus/README.md")).expect("README");
    let mut texts: Vec<(String, String)> = vec![("accepts.in_error".into(), accepts)];
    for line in readme.lines().filter(|l| l.contains("error_hint")) {
        texts.push(("README".into(), line.to_string()));
    }
    let changelog = std::fs::read_to_string(repo("CHANGELOG.md")).unwrap_or_default();
    for line in changelog.lines().filter(|l| l.contains("#1376")) {
        texts.push(("CHANGELOG".into(), line.to_string()));
    }
    let mut promises = 0;
    for (place, text) in &texts {
        if text.contains("never swallow") || text.contains("always stored") {
            promises += 1;
            assert!(
                text.contains("refuses the insert"),
                "{place} promises the first error without the refused insert: {text}"
            );
        }
    }
    assert!(
        promises >= 3,
        "the promise stands in README, accepts and CHANGELOG: {texts:#?}"
    );
}

// ═══════════════════════════════════════════════════════ the booted colony

/// The cell that fails: it puts out what it is handed, an `error` with its
/// code and its detail, or a question on `in_read`.
const FAILING: &str = "import sys, json\n\
                       doc = json.load(sys.stdin)\n\
                       hop = (doc[\"envelope\"].get(\"header\") or {}).get(\"hop\") or {}\n\
                       head = {k: v for k, v in hop.items() if k in (\"route\", \"error_code\") or k.startswith(\"read_\")}\n\
                       out = {\"header\": head}\n\
                       out.update(doc[\"body\"])\n\
                       sys.stdout.write(json.dumps([out]))\n";

fn write_json(p: &std::path::Path, v: &Value) {
    std::fs::create_dir_all(p.parent().expect("a parent")).expect("create the directory");
    std::fs::write(p, sj::to_string_pretty(v).expect("serialise")).expect("write");
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
    std::fs::create_dir_all(dst).expect("create the directory");
    for entry in std::fs::read_dir(src).expect("the source is readable") {
        let entry = entry.expect("directory entry");
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy");
        }
    }
}

/// The tree: the shipped argus, a failing cell, and the edge a parent draws
/// for #1376 -- the cell's `error` onto the argus, restamped `in_error`, the
/// sender named on the edge. Out of the argus the test draws `error_hint`,
/// `read` and `error` to the sink, so a contract violation would show there.
fn build_tree(td: &tempfile::TempDir) {
    let root = td.path();
    write_json(
        &root.join("main/config.json"),
        &json!({
            "cell": {"type": "hive"},
            "params": {"graph": {"edges": [
                {"from": "./voice", "to": "./argus",
                 "condition": "has(hop.route) && hop.route == 'error'",
                 "modifier": {"set_hop": {"route": "'in_error'", "error_sender": "'voice'"}}},
                {"from": "./voice", "to": "./argus",
                 "condition": "has(hop.route) && hop.route == 'in_read'"},
                {"from": "./argus", "to": "/sink",
                 "condition": "has(hop.route) && (hop.route == 'error_hint' || hop.route == 'read' || hop.route == 'error')"}
            ]}}
        }),
    );
    write_json(
        &root.join("main/voice/config.json"),
        &json!({
            "cell": {"type": "code"},
            "params": {"runner": "python3", "script_inline": FAILING,
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
                "purpose": "Fails the way it is told to.",
                "use_when": "Once, in this test.",
                "not_in_scope": "It decides nothing."
            }
        }),
    );
    copy_tree(&shipped().expect("guarded"), &root.join("main/argus"));
    // No cycle runs here (nothing is enabled, nothing arrives on `in_cycle`),
    // so the judge is never called; its endpoint only has to be named.
    std::fs::write(
        root.join(".env"),
        "OPENROUTER_API_KEY=test-key\n\
         ARGUS_JUDGE_BASE_URL=http://127.0.0.1:9/v1\n\
         ARGUS_JUDGE_MODEL=judge/thinker\n\
         ARGUS_JUDGE_PROVIDER=openai\n",
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

async fn send(h: &ColonyHandle, hop: Value, body: Value) {
    h.send(
        MessageBuilder::new(Path::new("/voice"))
            .body(Body::Inline(body))
            .hop(hop.as_object().cloned().expect("a hop is an object"))
            .ttl(64)
            .build(),
    )
    .await;
}

/// The next message at the sink, within the hang marker (never a verdict).
async fn next(rx: &mut mpsc::Receiver<meclaw_core::Message>) -> (String, Value) {
    let m = tokio::time::timeout(meclaw_testing::deadline::HANG, rx.recv())
        .await
        .expect("the argus puts something out within the hang marker")
        .expect("the capture channel stays open");
    let route = m
        .headers
        .hop
        .get("route")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let body = match &m.body {
        Body::Inline(v) => v.clone(),
        other => panic!("an inline body: {other:?}"),
    };
    assert_ne!(
        route, "error",
        "the argus reported an error: {:#?} {body:#}",
        m.headers.hop
    );
    (route, body)
}

/// I1 of the review: a real `error`, over an edge the test draws, through the
/// shipped hive (`. -> ./door -> ./receipts -> ./door -> .`) and the real
/// store: one hint kept with the redacted line, exactly one `error_hint` out,
/// no contract violation, no dead letter.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_error_over_a_drawn_edge_is_kept_and_told_once() {
    if skip() {
        return;
    }
    let td = tempfile::tempdir().expect("a temporary directory");
    build_tree(&td);
    let h = ColonyHandle::new_with_factories_at(&td, factories());
    let (sink_tx, mut rx) = mpsc::channel::<meclaw_core::Message>(64);
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

    send(
        &h,
        json!({"route": "error", "error_code": "duplex_failed"}),
        json!({"messages": [], "meta": {"detail": "duplex start failed at wss://svc:pw-mock@api.example.com/v1"}}),
    )
    .await;
    let line = "voice: duplex start failed at wss://REDACTED@api.example.com/v1";
    let (route, body) = next(&mut rx).await;
    assert_eq!(route, "error_hint", "{body:#}");
    assert_eq!(
        body["hint"],
        json!({"origin": "error:duplex_failed", "confidence": 100, "line": line}),
        "{body:#}"
    );

    // The marker: a read through the same door after it. Whatever else the
    // error put out is at the sink before the answer to the read.
    send(
        &h,
        json!({"route": "in_read", "read_what": "hints", "read_tag": "t"}),
        json!({"messages": []}),
    )
    .await;
    let mut told = 1;
    let rows = loop {
        let (route, body) = next(&mut rx).await;
        match route.as_str() {
            "error_hint" => told += 1,
            "read" => break body["rows"].as_array().cloned().unwrap_or_default(),
            other => panic!("unexpected `{other}` at the sink: {body:#}"),
        }
    };
    while let Ok(m) = rx.try_recv() {
        if m.headers.hop.get("route").and_then(Value::as_str) == Some("error_hint") {
            told += 1;
        }
    }
    assert_eq!(told, 1, "one stored hint is told once");
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0]["origin"], "error:duplex_failed", "{rows:#?}");
    assert_eq!(rows[0]["line"], line, "{rows:#?}");
    assert!(!rows[0].to_string().contains("pw-mock"), "{rows:#?}");

    let dl = h.drain_dead_letters().await;
    assert!(
        dl.is_empty(),
        "no dead letter, no contract violation: {dl:#?}"
    );
    h.shutdown().await;
}
