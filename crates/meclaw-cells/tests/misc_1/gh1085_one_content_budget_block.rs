//! GH #1085 (R-IG-1) -- the content-budget block is ONE text.
//!
//! A template's code cell runs as `params.script_inline` and imports nothing
//! of its own, so the two helpers every producer sizes its content with --
//! `budget_chars`, a share of the receiving model's window in characters, and
//! `clip`, the cut that says what it showed, of how much, and how to get the
//! rest -- travel as a COPY between two marker lines. A copy that drifts is a
//! producer that sizes or marks differently from every other one, and nothing
//! in a review would see it. So this file finds every copy under `templates/`
//! (the strings of every `config.json`, and any `.py` file) and holds it byte
//! for byte against [`CANONICAL`], the text of the wave design (section 5).
//!
//! It passes with no copy at all and says how many it saw.
//!
//! GH #1526 (d58-3 grenzen, N5): a second block, `content-budget v2`, takes
//! the rate of the target (`budget_chars(input_soft, share, rate=None)`); the
//! cells that hand a window on with its rate carry it, every other copy
//! stays v1 (the frozen curator 1.25.0 among them). Each version is one
//! text, held byte for byte against its canonical form; without a rate v2
//! counts exactly like v1, at the core's three characters a token.

use meclaw_core::serde_json::{self as sj, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const START: &str = "# >>> content-budget v1";
const END: &str = "# <<< content-budget v1";
const START_V2: &str = "# >>> content-budget v2";
const END_V2: &str = "# <<< content-budget v2";

/// The block, from its opening marker line to its closing one, without the
/// newline after the closing marker.
const CANONICAL: &str = r#"# >>> content-budget v1 (R-IG-1, GH #1085)
CHARS_PER_TOKEN = 3


def budget_chars(input_soft, share):
    """Characters a producer may hand a model whose usable window is
    `input_soft` tokens (the catalog row, cut by the curator), for a section
    that takes `share` of that window. None when no window is known: the
    producer then delivers its content whole."""
    try:
        soft = int(float(input_soft))
    except (TypeError, ValueError):
        return None
    if soft <= 0:
        return None
    return int(soft * share * CHARS_PER_TOKEN)


def clip(text, limit, hint="budget of the window"):
    """`text` whole when it fits `limit` (None: no limit); else its head and
    a mark naming what was shown, the total and how to get the rest."""
    text = "" if text is None else str(text)
    if limit is None or len(text) <= limit:
        return text
    limit = max(0, int(limit))
    return text[:limit] + "...[cut: %d of %d chars shown; %s]" % (
        limit, len(text), hint)
# <<< content-budget v1"#;

/// The block that takes the target's rate (GH #1526), from its opening
/// marker line to its closing one.
const CANONICAL_V2: &str = r#"# >>> content-budget v2 (R-IG-1, GH #1085; the target's rate, GH #1526)
CHARS_PER_TOKEN = 3


def chars_per_token_or_core(rate):
    """`rate`, the characters a token of the target's catalog row, when it
    is a number above zero; else CHARS_PER_TOKEN, the core's estimate."""
    try:
        r = float(rate) if rate not in (None, "") and not isinstance(rate, bool) else 0.0
    except (TypeError, ValueError):
        r = 0.0
    return r if 0 < r < float("inf") else CHARS_PER_TOKEN


def budget_chars(input_soft, share, rate=None):
    """Characters a producer may hand a model whose usable window is
    `input_soft` tokens (the catalog row, cut by the curator), for a section
    that takes `share` of that window, at `rate` characters a token (the
    target's row; none known: CHARS_PER_TOKEN). None when no window is known:
    the producer then delivers its content whole."""
    try:
        soft = int(float(input_soft))
    except (TypeError, ValueError):
        return None
    if soft <= 0:
        return None
    return int(soft * share * chars_per_token_or_core(rate))


def clip(text, limit, hint="budget of the window"):
    """`text` whole when it fits `limit` (None: no limit); else its head and
    a mark naming what was shown, the total and how to get the rest."""
    text = "" if text is None else str(text)
    if limit is None or len(text) <= limit:
        return text
    limit = max(0, int(limit))
    return text[:limit] + "...[cut: %d of %d chars shown; %s]" % (
        limit, len(text), hint)
# <<< content-budget v2"#;

/// The cells that hand a window on with its rate and measure with it: each
/// carries v2 (GH #1526, contract point 1).
const V2_CELLS: [&str; 8] = [
    "curator/policy",
    "curator/intake",
    "curator/history",
    "curator/handover",
    "curator/push",
    "curator/reader",
    "objects/brief",
    "talky/route",
];

fn templates_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../templates")
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

/// Every block in `text` between `start` and `end`: `Ok(copy)` for an
/// opening marker with a closing one after it, `Err(why)` for a marker
/// without its partner.
fn blocks_between(text: &str, start: &str, end: &str) -> Vec<Result<String, String>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(start) {
        let tail = &rest[i..];
        match tail.find(end) {
            Some(j) => {
                out.push(Ok(tail[..j + end.len()].to_string()));
                rest = &tail[j + end.len()..];
            }
            None => {
                out.push(Err(
                    "an opening marker without its closing marker".to_string()
                ));
                rest = "";
            }
        }
    }
    let opened = text.matches(start).count();
    let closed = text.matches(end).count();
    if closed > opened {
        out.push(Err(format!(
            "{closed} closing markers for {opened} opening ones"
        )));
    }
    out
}

/// (canonical copies seen, findings) over every `config.json` string and
/// every `.py` file under `root`.
fn sweep(root: &Path) -> (usize, Vec<String>) {
    sweep_for(root, START, END, CANONICAL)
}

/// [`sweep`] for one version of the block.
fn sweep_for(root: &Path, start: &str, end: &str, canonical: &str) -> (usize, Vec<String>) {
    let mut files = Vec::new();
    walk(root, &mut files);
    let (mut copies, mut findings) = (0, Vec::new());
    for f in files {
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
            for b in blocks_between(&t, start, end) {
                match b {
                    Ok(copy) if copy == canonical => copies += 1,
                    Ok(_) => findings.push(format!(
                        "{}: a copy of the content-budget block differs from the canonical text",
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
    let (copies, findings) = sweep(&templates_root());
    eprintln!("content-budget v1: {copies} canonical copies under templates/");
    assert!(findings.is_empty(), "{findings:#?}");
}

#[test]
fn a_drifted_copy_and_a_lone_marker_are_findings() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cell = dir.path().join("t/cell");
    std::fs::create_dir_all(&cell).expect("dirs");
    let drifted = CANONICAL.replace("CHARS_PER_TOKEN = 3", "CHARS_PER_TOKEN = 4");
    assert_ne!(drifted, CANONICAL, "the probe changes the copy");
    let script = format!("import sys\n{CANONICAL}\n\n{drifted}\n\n{START}\n");
    let cfg = sj::json!({"params": {"script_inline": script}});
    std::fs::write(cell.join("config.json"), cfg.to_string()).expect("write");
    let (copies, findings) = sweep(dir.path());
    assert_eq!(copies, 1, "the canonical copy counts: {findings:?}");
    assert_eq!(findings.len(), 2, "{findings:#?}");
    assert!(findings[0].contains("differs"), "{findings:?}");
    assert!(findings[1].contains("without its closing"), "{findings:?}");
}

/// The canonical text does what the design says it does: a share of the
/// window at three characters a token, nothing without a window, and a cut
/// that names what it showed, the total and the way to the rest.
#[test]
fn the_canonical_block_sizes_and_marks_as_designed() {
    let probe = format!(
        "{CANONICAL}\n\
         assert budget_chars(120000, 0.25) == 90000\n\
         assert budget_chars('250000', 0.1) == 75000\n\
         assert budget_chars(None, 0.1) is None\n\
         assert budget_chars('', 0.1) is None\n\
         assert budget_chars(0, 0.5) is None\n\
         assert budget_chars(-5, 0.5) is None\n\
         assert clip('abc', None) == 'abc'\n\
         assert clip('abc', 3) == 'abc'\n\
         assert clip(None, 4) == ''\n\
         assert clip('abcdef', 3) == 'abc...[cut: 3 of 6 chars shown; budget of the window]'\n\
         assert clip('abcdef', 2, 'read with from=9') == 'ab...[cut: 2 of 6 chars shown; read with from=9]'\n\
         print('ok')\n"
    );
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
        .write_all(probe.as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "ok",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// GH #1526: every copy of v2 is its canonical text, and the cells that
/// measure with the target's rate carry it.
#[test]
fn every_copy_of_the_v2_block_is_its_canonical_text() {
    let (copies, findings) = sweep_for(&templates_root(), START_V2, END_V2, CANONICAL_V2);
    eprintln!("content-budget v2: {copies} canonical copies under templates/");
    assert!(findings.is_empty(), "{findings:#?}");
    for cell in V2_CELLS {
        let (n, f) = sweep_for(&templates_root().join(cell), START_V2, END_V2, CANONICAL_V2);
        assert!(f.is_empty(), "{cell}: {f:#?}");
        assert_eq!(n, 1, "{cell} carries the block that takes a rate");
    }
}

/// GH #1526 (lock): without a rate v2 counts exactly like v1 -- the core's
/// three characters a token (`content_budget.rs`) -- and with one it counts
/// at that rate; a rate that is no number above zero is none.
#[test]
fn the_v2_block_without_a_rate_counts_as_v1() {
    let probe = format!(
        "{CANONICAL_V2}\n\
         assert budget_chars(120000, 0.25) == 90000\n\
         assert budget_chars('250000', 0.1) == 75000\n\
         assert budget_chars(None, 0.1) is None\n\
         assert budget_chars('', 0.1) is None\n\
         assert budget_chars(0, 0.5) is None\n\
         assert budget_chars(-5, 0.5) is None\n\
         assert budget_chars(1000, 0.1, None) == 300\n\
         assert budget_chars(1000, 0.1, 4) == 400\n\
         assert budget_chars(1000, 0.1, '4') == 400\n\
         assert budget_chars(1000, 0.1, 3.5) == 350\n\
         for bad in (0, -1, '', 'x', True, float('nan'), float('inf'), [4]):\n\
         \x20   assert budget_chars(1000, 0.1, bad) == 300, bad\n\
         assert budget_chars(None, 0.1, 4) is None\n\
         assert clip('abcdef', 3) == 'abc...[cut: 3 of 6 chars shown; budget of the window]'\n\
         print('ok')\n"
    );
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
        .write_all(probe.as_bytes())
        .expect("write");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "ok",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
