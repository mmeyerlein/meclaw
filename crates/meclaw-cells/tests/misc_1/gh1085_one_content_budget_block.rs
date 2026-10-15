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

use meclaw_core::serde_json::{self as sj, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const START: &str = "# >>> content-budget v1";
const END: &str = "# <<< content-budget v1";

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

/// (canonical copies seen, findings) over every `config.json` string and
/// every `.py` file under `root`.
fn sweep(root: &Path) -> (usize, Vec<String>) {
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
            for b in blocks_in(&t) {
                match b {
                    Ok(copy) if copy == CANONICAL => copies += 1,
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
