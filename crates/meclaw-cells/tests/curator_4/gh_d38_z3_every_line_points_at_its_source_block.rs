//! GH #1440 (D38 Z3, OR-38-42 T2) -- every line of the brief points at its
//! newest source block, `[#<12 hex>]`, and the pointers never reach a voice.
//!
//! WHY this file exists: a brief line is a claim about what was said; with
//! the short id of the block it came from, the model can fetch the words
//! (`history_read`) instead of trusting the line. The pointer comes from the
//! summarizer, which reads every transcript line under its id, and is checked
//! in code: with the transcript (GH #1517), a line without an id or with an id
//! that names no row of the transcript is dropped (`brief_lines` with
//! `transcript_ids`) and counted in `summary_rejected` -- it names no source
//! the model read, and pointing it at the newest row made a pointer nobody
//! wrote. This file pins that: a cited source stays, a line without an id, a
//! line citing a block outside the transcript and a line with an unknown id
//! are dropped, and the three are counted. What stays points at a source.
//! The ids stay out of everything spoken or seeded into a call, as they do
//! today (GH #1345, `./handover` `without_ids` before a renewed session's
//! block; the `history.handover` leaf keeps them for the model).
//!
//! Stub summarizer (`support/curator_hive.rs` `Hive::answer`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

fn stored_text(h: &Hive) -> String {
    let rows = h.rows(
        "SELECT b.body FROM summaries s JOIN blocks b ON b.hash = s.hash \
         ORDER BY s.rowid DESC LIMIT 1",
    );
    let body: Value = sj::from_str(rows[0][0].as_str().expect("a block body")).expect("json");
    body["text"].as_str().unwrap_or_default().to_string()
}

#[test]
fn every_line_points_at_its_source_block() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "keep_recent", json!(1)),
        ("policy", "summary_form", json!("brief")),
    ]);
    turn(&mut h, "s1", "t1", "q1", "a1", json!({}));
    turn(&mut h, "s1", "t2", "q2", "a2", json!({}));
    turn(
        &mut h,
        "s1",
        "t3",
        "q3",
        "a3",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
    );
    h.fire(&last_add(&h));
    let (q1, a2) = (short_id(&user("q1")), short_id(&said("a2")));
    // a3 is the raw turn the rebuild keeps: a real block, but no source.
    let a3 = short_id(&said("a3"));
    // GH #1496: each line is dated with a day its transcript shows.
    let day = h.transcript_day();
    h.answer(
        &format!(
            "- person ({day}) hannah/lives_in: Hannah lives in Lisbon. [asserted] [#{q1}]\n\
             - person ({day}) garden/size: The garden is small. [asserted]\n\
             - agent:self ({day}) offer/visit: A visit was offered. [#{a3}]\n\
             - topic ({day}): the garden [#ffffffffffff]\n\
             - open ({day}): the garden fence [#{a2}]"
        ),
        "stop",
    );
    let text = stored_text(&h);
    assert_eq!(
        text,
        format!(
            "- person ({day}) hannah/lives_in: Hannah lives in Lisbon. [asserted] [#{q1}]\n\
             - open ({day}): the garden fence [#{a2}]"
        ),
        "a cited source stays; none, a block outside the transcript and an unknown id \
         are dropped (GH #1517)"
    );
    let marks: Vec<Value> = h
        .rows("SELECT value FROM marks WHERE kind = 'summary_rejected'")
        .into_iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or_default()).expect("json"))
        .collect();
    assert_eq!(
        marks,
        vec![json!({"kept": 2, "dropped": 3})],
        "the three are counted"
    );
    let sources: Value = sj::from_str(
        h.rows("SELECT sources FROM summaries")[0][0]
            .as_str()
            .expect("sources"),
    )
    .expect("json");
    let ids: Vec<String> = sources
        .as_array()
        .expect("a list")
        .iter()
        .map(|s| s.as_str().unwrap_or_default()[..12].to_string())
        .collect();
    for line in text.lines() {
        let id = line
            .rsplit("[#")
            .next()
            .and_then(|t| t.strip_suffix(']'))
            .unwrap_or_default();
        assert!(ids.iter().any(|s| s == id), "{line:?} points at a source");
    }
}

const LOADER: &str = r#"
import ast, io, json, sys
inp = json.load(sys.stdin)
sys.stdin = io.StringIO("")
src, params = inp["src"], inp["params"]
keep = [n for n in ast.parse(src).body
        if isinstance(n, (ast.Import, ast.ImportFrom, ast.FunctionDef))
        or (isinstance(n, ast.Assign)
            and all(isinstance(t, ast.Name) and t.id.isupper() for t in n.targets))]
scope = {"P": params, "ctx": {}, "hop": {},
         "doc": {"params": params, "body": {}, "envelope": {}}}
for n in keep:
    exec(compile(ast.Module(body=[n], type_ignores=[]), "handover", "exec"), scope)
scope["ARGS"] = inp.get("args")
print(json.dumps(eval(inp["probe"], scope)))
"#;

/// GH #1345: the block a renewed session's call is seeded with goes through
/// `without_ids`; a brief inside it leaves no pointer, and its words stay.
#[test]
fn the_pointers_never_reach_a_voice() {
    if !shipped() {
        return;
    }
    let brief = "- topic (2026-11-05): the garden [#0a1b2c3d4e5f]\n\
                 - person (2026-11-05) hannah/lives_in: Hannah moved to Porto. [asserted] \
                 [superseded 2026-11-04] [#1a2b3c4d5e6f]";
    let mut params = cell_config("handover")["params"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    params.remove("script_inline");
    let doc = json!({"src": script_of("handover"), "params": params, "args": brief,
                     "probe": "without_ids(RENEW_HEAD + '\\n' + handover_text((), (), '', ARGS, ()))"});
    let out = run_python(LOADER, &doc.to_string());
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seeded: Value = sj::from_slice(&out.stdout).expect("json");
    let seeded = seeded.as_str().expect("a block");
    assert!(!seeded.contains("[#"), "no pointer is seeded: {seeded}");
    assert!(!seeded.contains("0a1b2c3d4e5f"), "{seeded}");
    assert!(
        seeded.contains("Hannah moved to Porto."),
        "the words stay: {seeded}"
    );
}
