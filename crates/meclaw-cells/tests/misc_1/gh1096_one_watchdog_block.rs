//! GH #1096 (R-AG-1) -- the watchdog block is ONE text, and it speaks the
//! timer's contract.
//!
//! A template's code cell runs as `params.script_inline` and imports nothing
//! of its own, so the helpers that turn "something becomes due" into a timer
//! order -- `once`, `retrigger`, `calendar`, `cancel` -- travel as a COPY
//! between two marker lines. The canonical text lives in the tree as
//! `docs/watchdog-v1.py`, the file the repositories outside it copy from too.
//! This file finds every copy under `templates/` (the strings of every
//! `config.json`, and any `.py` file) and holds it byte for byte against that
//! file, and it runs the ops the block builds through the timer's OWN parser,
//! so a block that drifted from the timer's contract is red here rather than a
//! watchdog that never fires.

use meclaw_cells::timer::op::TimerOp;
use meclaw_cells::timer::schedule::ScheduleKind;
use meclaw_core::serde_json::{self as sj, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const START: &str = "# >>> watchdog v1";
const END: &str = "# <<< watchdog v1";

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// The block, from its opening marker line to its closing one, without the
/// newline after the closing marker.
fn canonical() -> String {
    let raw = std::fs::read_to_string(repo("docs/watchdog-v1.py")).expect("docs/watchdog-v1.py");
    let text = raw.strip_suffix('\n').unwrap_or(&raw).to_string();
    assert!(
        text.starts_with(START) && text.ends_with(END) && text.matches(START).count() == 1,
        "docs/watchdog-v1.py is the block and nothing else"
    );
    text
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn strings_of(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| strings_of(x, out)),
        Value::Object(o) => o.values().for_each(|x| strings_of(x, out)),
        _ => {}
    }
}

/// Every block in `text`: `Ok(copy)` for an opening marker with a closing one
/// after it, `Err(why)` for a marker without its partner.
fn blocks_in(text: &str) -> Vec<Result<String, String>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(START) {
        let tail = &rest[i..];
        match tail.find(END) {
            Some(j) => {
                out.push(Ok(tail[..j + END.len()].to_string()));
                rest = &tail[j + END.len()..];
            }
            None => {
                out.push(Err(
                    "an opening marker without its closing marker".to_string()
                ));
                rest = "";
            }
        }
    }
    let opened = text.matches(START).count();
    let closed = text.matches(END).count();
    if closed > opened {
        out.push(Err(format!(
            "{closed} closing markers for {opened} opening ones"
        )));
    }
    out
}

/// A frozen template version: `<name>@<major>.<minor>.<patch>` (R-SA-37), the
/// name of a template directory right under the swept root. Any other `@` in
/// a path is swept like every other directory.
fn is_frozen_version(dir: &str) -> bool {
    let Some((name, version)) = dir.split_once('@') else {
        return false;
    };
    let parts: Vec<&str> = version.split('.').collect();
    !name.is_empty()
        && parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Every block found in the `config.json` strings and `.py` files under `dir`.
fn blocks_under(dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    walk(dir, &mut files);
    let mut out = Vec::new();
    for f in files {
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let mut texts = Vec::new();
        if name == "config.json" {
            let raw = std::fs::read_to_string(&f).expect("config.json");
            let v: Value = sj::from_str(&raw).expect("config.json parses");
            strings_of(&v, &mut texts);
        } else if name.ends_with(".py") {
            texts.push(std::fs::read_to_string(&f).expect(".py"));
        }
        for t in texts {
            out.extend(blocks_in(&t).into_iter().flatten());
        }
    }
    out
}

/// (canonical copies seen, findings) over every `config.json` string and
/// every `.py` file under `root`.
///
/// A frozen version directory (`<name>@<version>`, e.g. `simple-agent@1.1.4`)
/// is not swept: it carries the block of the day it froze, its bytes are held
/// by `scripts/tests/test_frozen_templates.py`, and an old block there still
/// speaks a contract the timer keeps (`at`; #1315 added `after_ms` beside it).
fn sweep(root: &Path, canonical: &str) -> (usize, Vec<String>) {
    let mut files = Vec::new();
    walk(root, &mut files);
    let (mut copies, mut findings) = (0, Vec::new());
    for f in files {
        let frozen = f
            .strip_prefix(root)
            .ok()
            .and_then(|rel| rel.components().next())
            .is_some_and(|c| is_frozen_version(&c.as_os_str().to_string_lossy()));
        if frozen {
            continue;
        }
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let texts: Vec<String> = if name == "config.json" {
            let raw = std::fs::read_to_string(&f).expect("config.json");
            let v: Value =
                sj::from_str(&raw).unwrap_or_else(|e| panic!("{}: not JSON: {e}", f.display()));
            let mut s = Vec::new();
            strings_of(&v, &mut s);
            s
        } else if name.ends_with(".py") {
            vec![std::fs::read_to_string(&f).expect(".py")]
        } else {
            continue;
        };
        for t in texts {
            for b in blocks_in(&t) {
                match b {
                    Ok(copy) if copy == canonical => copies += 1,
                    Ok(_) => findings.push(format!(
                        "{}: a copy of the watchdog block differs from docs/watchdog-v1.py",
                        f.display()
                    )),
                    Err(why) => findings.push(format!("{}: {why}", f.display())),
                }
            }
        }
    }
    (copies, findings)
}

#[test]
fn every_copy_of_the_block_is_the_canonical_text() {
    let canonical = canonical();
    let (copies, findings) = sweep(&repo("templates"), &canonical);
    eprintln!("watchdog v1: {copies} canonical copies under templates/");
    assert!(findings.is_empty(), "{findings:#?}");
    // The shipped users (GH #1096): the access sweep, the affinity push, and
    // since review fix1 the affinity gate (its writes re-arm the push debounce).
    assert!(
        copies >= 3,
        "access/sweep, affinity/push and affinity/gate carry the block: {copies}"
    );
}

#[test]
fn a_drifted_copy_and_a_lone_marker_are_findings() {
    let canonical = canonical();
    let dir = tempfile::tempdir().expect("tempdir");
    let cell = dir.path().join("t/cell");
    std::fs::create_dir_all(&cell).expect("dirs");
    let drifted = canonical.replace("catch_up=True", "catch_up=False");
    assert_ne!(drifted, canonical, "the probe changes the copy");
    let script = format!("import sys\n{canonical}\n\n{drifted}\n\n{START}\n");
    let cfg = sj::json!({"params": {"script_inline": script}});
    std::fs::write(cell.join("config.json"), cfg.to_string()).expect("write");
    let (copies, findings) = sweep(dir.path(), &canonical);
    assert_eq!(copies, 1, "the canonical copy counts: {findings:?}");
    assert_eq!(findings.len(), 2, "{findings:#?}");
    assert!(findings[0].contains("differs"), "{findings:?}");
    assert!(findings[1].contains("without its closing"), "{findings:?}");
}

/// #1315 (OR-agent-2.N2.8): the sweep skips a frozen version directory only.
/// A drifted copy under a directory with an `@` that names no version is
/// still a finding.
#[test]
fn only_a_frozen_version_directory_is_skipped() {
    let canonical = canonical();
    let dir = tempfile::tempdir().expect("tempdir");
    let drifted = canonical.replace("catch_up=True", "catch_up=False");
    for t in ["t@1.0.0/cell", "t@next/cell", "t/c@1.0.0"] {
        let cell = dir.path().join(t);
        std::fs::create_dir_all(&cell).expect("dirs");
        let cfg = sj::json!({"params": {"script_inline": drifted.clone()}});
        std::fs::write(cell.join("config.json"), cfg.to_string()).expect("write");
    }
    let (_, findings) = sweep(dir.path(), &canonical);
    assert_eq!(findings.len(), 2, "{findings:#?}");
    assert!(
        findings.iter().all(|f| !f.contains("t@1.0.0")),
        "the frozen version is skipped: {findings:#?}"
    );
}

/// #1315 (OR-agent-2.N2.8): the frozen `simple-agent@1.1.4` keeps the block
/// of the day it froze, whose debounce names a moment (`at`). The timer still
/// takes it: the lead-time resolution leaves such an op as it is, and it
/// parses as the same one-shot it always was, so v1 stays runnable.
#[test]
fn the_frozen_block_still_speaks_a_contract_the_timer_keeps() {
    let blocks = blocks_under(&repo("templates/simple-agent@1.1.4"));
    assert!(!blocks.is_empty(), "simple-agent@1.1.4 carries the block");
    let now = chrono::DateTime::from_timestamp_millis(1_893_456_000_000).expect("now");
    for block in blocks {
        assert_ne!(block, canonical(), "the frozen copy is the old block");
        let ops = ops_with(
            &block,
            "AT = 1893456000000\n\
             print(json.dumps(retrigger('w', AT, 30000, '../x', {'messages': []})))\n",
        );
        assert_eq!(ops.len(), 1);
        let mut op = ops[0].clone();
        assert!(
            op.get("after_ms").is_none(),
            "the old block names a moment: {op}"
        );
        meclaw_cells::timer::op::resolve_after(&mut op, now).expect("an op without a lead");
        assert_eq!(op, ops[0], "the timer leaves a moment as it came");
        match TimerOp::parse(&op).expect("the timer takes the old op") {
            TimerOp::Add { row, .. } => match row.kind {
                ScheduleKind::At(t) => assert_eq!(t.timestamp_millis(), 1_893_456_030_000),
                other => panic!("a one-shot, not {other:?}"),
            },
            other => panic!("an add, not {other:?}"),
        }
    }
}

/// The ops the block builds, as JSON lines, from a probe appended to it.
fn ops_of(probe: &str) -> Vec<Value> {
    ops_with(&canonical(), probe)
}

/// The ops `block` builds, as JSON lines, from a probe appended to it.
fn ops_with(block: &str, probe: &str) -> Vec<Value> {
    let script = format!("{block}\nimport json\n{probe}\n");
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(script.as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| sj::from_str(l).expect("one op per line"))
        .collect()
}

/// The three forms the contract names are the timer's own ops: a one-shot is
/// an `add` with `at`, `rearm` and `catch_up`, a debounce is the same one-shot
/// pushed out by the quiet time, a calendar point is an `add` with a six-field
/// cron, and a cancel is a `remove` -- each under the one id the name maps to.
#[test]
fn the_block_speaks_the_timer_contract() {
    let ops = ops_of(
        "AT = 1893456000000  # 2030-01-01T00:00:00Z\n\
         print(json.dumps(once('w', AT + 1, '../x', {'messages': []})))\n\
         print(json.dumps(retrigger('w', AT, 30000, '../x', {'messages': []})))\n\
         print(json.dumps(retrigger('w', AT, 0, '../x', {'messages': []})))\n\
         print(json.dumps(calendar('night', '0 0 3 * * *', '../x', {'messages': []})))\n\
         print(json.dumps(cancel('w')))\n\
         print(json.dumps({'same': watchdog_id('w') == watchdog_id('w'), \
                           'apart': watchdog_id('w') != watchdog_id('night')}))\n",
    );
    assert_eq!(ops.len(), 6);
    let at = |v: &Value| match TimerOp::parse(v).expect("parses") {
        TimerOp::Add { row, rearm } => {
            assert!(rearm, "a watchdog replaces its standing order: {v}");
            assert!(
                row.catch_up,
                "a missed watchdog fires on the next start: {v}"
            );
            match row.kind {
                ScheduleKind::At(t) => (row.schedule_id, t.timestamp_millis()),
                other => panic!("a one-shot, not {other:?}"),
            }
        }
        other => panic!("an add, not {other:?}"),
    };
    let (id, once_ms) = at(&ops[0]);
    assert_eq!(
        once_ms, 1_893_456_000_001,
        "the moment travels to the millisecond"
    );
    // #1315 (K-6 b): a debounce carries its lead, not a moment of the
    // caller's clock; the timer resolves `after_ms` on its own clock read
    // (here a fixed one, the AT of the script).
    let timer_now = chrono::DateTime::from_timestamp_millis(1_893_456_000_000).expect("AT");
    let resolved = |v: &Value| {
        assert!(
            v.get("at").is_none(),
            "a debounce names no moment of the caller: {v}"
        );
        let mut v = v.clone();
        meclaw_cells::timer::op::resolve_after(&mut v, timer_now).expect("a lead time");
        v
    };
    let (id2, quiet_ms) = at(&resolved(&ops[1]));
    assert_eq!(id, id2, "one name, one id: re-arming replaces");
    assert_eq!(
        quiet_ms, 1_893_456_030_000,
        "a debounce fires `quiet` after the timer took it"
    );
    let (_, zero_ms) = at(&resolved(&ops[2]));
    assert!(
        zero_ms > 1_893_456_000_000,
        "a zero quiet time is still ahead, never `now`"
    );
    match TimerOp::parse(&ops[3]).expect("calendar parses") {
        TimerOp::Add { row, rearm } => {
            assert!(rearm);
            assert!(!row.catch_up, "the timer refuses catch_up on a cron row");
            assert!(matches!(row.kind, ScheduleKind::Cron(ref c) if c == "0 0 3 * * *"));
        }
        other => panic!("an add, not {other:?}"),
    }
    match TimerOp::parse(&ops[4]).expect("cancel parses") {
        TimerOp::Remove { schedule_id } => assert_eq!(schedule_id, id),
        other => panic!("a remove, not {other:?}"),
    }
    assert_eq!(ops[5], sj::json!({"same": true, "apart": true}));

    // A five-field cron is not the timer's form; the block passes it through
    // and the parser says so, which is why the docstring names six fields.
    let five = ops_of("print(json.dumps(calendar('n', '0 3 * * *', '../x', {})))");
    let err = TimerOp::parse(&five[0]).expect_err("five fields");
    assert!(err.starts_with("cron:"), "{err}");
}
