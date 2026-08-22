//! P15 task 1 -- version chains as a pure function inside the shipped recall script.
//!
//! The probe runs the REAL `params.script_inline`, never a copy (P5 pattern, see
//! `workshop/evals/p5-longmemeval/tools/cellrun.py`): the `${VAR:-default}` literals
//! are resolved the way the colony resolves them at instantiation, the module body
//! runs against a stub stdin, and the `park()` exit at its end is swallowed so the
//! probe can call the helpers the script defines.

use std::io::Write;
use std::process::{Command, Stdio};

fn recall_script() -> String {
    let raw = std::fs::read_to_string("../../templates/memory-hive/recall/config.json")
        .expect("recall config");
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    resolve_vars(v["params"]["script_inline"].as_str().unwrap())
}

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty string --
/// the same substitution the colony performs when it instantiates the template.
fn resolve_vars(script: &str) -> String {
    let mut out = String::with_capacity(script.len());
    let mut rest = script;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail
            .find('}')
            .expect("unterminated ${...} in script_inline");
        if let Some((_, default)) = tail[..end].split_once(":-") {
            out.push_str(default);
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Hand a probe program to python3 **on stdin**, never in argv.
///
/// A probe embeds the whole shipped script as a literal, and a single argv
/// string is capped at 128 KiB (`MAX_ARG_STRLEN`). The recall script crossed
/// that line in W2, and the failure mode is an opaque `ArgumentListTooLong`
/// that looks like a broken test rather than like a size limit. `python3 -`
/// reads and compiles the whole program from stdin before it runs a line of
/// it, so the probe's own `sys.stdin` replacement below is unaffected.
fn run_python(src: &str) -> std::process::Output {
    let mut child = Command::new("python3")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    // Dropped, not merely borrowed: python reads until EOF.
    let mut sink = child.stdin.take().expect("stdin");
    sink.write_all(src.as_bytes()).expect("write program");
    drop(sink);
    child.wait_with_output().expect("wait")
}

fn run_probe(probe: &str) -> String {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO('{{\"envelope\": {{}}, \"body\": {{}}, \"params\": {{}}}}')\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'recall', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}"
        ),
        serde_json::to_string(&recall_script()).unwrap(),
        probe
    );
    let out = run_python(&src);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn chain_orders_by_valid_from_and_closes_predecessors() {
    // The ordering claim is unchanged; what closes a predecessor is not. Since
    // W2 (#13, ruling Q1) the two-argument `effective_until` is gone with the
    // axis arithmetic it implemented, and a span ends on a later assertion of
    // the SAME statement -- so the fixture asserts Helix twice and vscode is a
    // statement of its own.
    let probe = r#"
rows = [
 {"id":"b","subject":"user:alex","predicate":"editor","claim":"Helix",
  "valid_from":"2026-08-08T19:28:00Z","valid_until":None,"recorded_at":"2026-08-08T19:28:03Z"},
 {"id":"a","subject":"user:alex","predicate":"editor","claim":"Helix",
  "valid_from":"2026-08-08T19:18:34Z","valid_until":None,"recorded_at":"2026-08-08T19:18:39Z"},
 {"id":"c","subject":"user:alex","predicate":"editor","claim":"vscode",
  "valid_from":"2026-08-08T19:38:00Z","valid_until":None,"recorded_at":"2026-08-08T19:38:03Z"},
]
ch = build_chains(rows)[("user:alex","editor")]
print("|".join(f["claim"] + ":" + f["id"] for f in ch))
print(span_end(ch, 0))
print(span_end(ch, 1))
print(span_end(ch, 2))
"#;
    assert_eq!(
        run_probe(probe),
        "Helix:a|Helix:b|vscode:c\n2026-08-08T19:28:00Z\nNone\nNone"
    );
}

#[test]
fn candidate_is_the_current_fact_and_carries_its_history() {
    // W2 (#13, ruling Q2): the closure on the superseded row now has to NAME its
    // successor, because that is what the projection follows. Before W2 the
    // stored columns were decoration here and the hit was collapsed onto
    // whatever started last on the axis, which is exactly the arithmetic that
    // ended 636 foresight facts once W1 thawed the bucket axes.
    let probe = r#"
axis_rows = [
 {"id":"a","subject":"user:alex","predicate":"editor","claim":"Helix",
  "valid_from":"2026-08-08T19:18:34Z","valid_until":None,
  "recorded_at":"2026-08-08T19:18:39Z","expired_at":"2026-08-08T19:28:00Z",
  "superseded_by":"b"},
 {"id":"b","subject":"user:alex","predicate":"editor","claim":"vscode",
  "valid_from":"2026-08-08T19:28:00Z","valid_until":None,
  "recorded_at":"2026-08-08T19:28:03Z","expired_at":None},
]
# the hit was the SUPERSEDED fact -- it has to project onto the current one
out = project_fact_candidate("a", axis_rows)
print(out["id"], out["claim"], len(out["history"]),
      out["history"][0]["claim"], out["history"][0]["until"])
"#;
    assert_eq!(run_probe(probe), "b vscode 1 Helix 2026-08-08T19:28:00Z");
}

#[test]
fn projection_is_stable_when_the_hit_is_already_current() {
    let probe = r#"
axis_rows = [
 {"id":"b","subject":"s","predicate":"p","claim":"now","valid_from":"2026-01-02T00:00:00Z",
  "valid_until":None,"recorded_at":"2026-01-02T00:00:00Z","expired_at":None},
]
out = project_fact_candidate("b", axis_rows)
print(out["id"], out["claim"], out["history"])
"#;
    assert_eq!(run_probe(probe), "b now []");
}

#[test]
fn chain_tie_break_is_deterministic() {
    // Same `valid_from`, same `recorded_at` -> the id decides, and it decides the
    // same way for both input orders.
    let probe = r#"
mk = lambda i: {"id":i,"subject":"s","predicate":"p","claim":i,
                "valid_from":"2026-01-01T00:00:00Z","valid_until":None,
                "recorded_at":"2026-01-01T00:00:00Z"}
one = "|".join(f["claim"] for f in build_chains([mk("z"), mk("a")])[("s","p")])
two = "|".join(f["claim"] for f in build_chains([mk("a"), mk("z")])[("s","p")])
print(one); print(two)
"#;
    assert_eq!(run_probe(probe), "a|z\na|z");
}

#[test]
fn identical_valid_from_is_coexistence_not_supersession() {
    // Ruling O-1: two facts that start at the same instant are BOTH true --
    // one child fact does not end because a second child fact was recorded.
    // (The predicate spelling is deliberately not English: an axis is a KEY the
    // memory learned, and the chain arithmetic never reads it as language.)
    let probe = r#"
rows = [
 {"id":"mika","subject":"user:alex","predicate":"hat Sohn","claim":"Mika",
  "valid_from":"2026-01-01T00:00:00Z","valid_until":None,"recorded_at":"2026-01-01T00:00:01Z"},
 {"id":"noa","subject":"user:alex","predicate":"hat Sohn","claim":"Noa",
  "valid_from":"2026-01-01T00:00:00Z","valid_until":None,"recorded_at":"2026-01-01T00:00:02Z"},
]
ch = build_chains(rows)[("user:alex","hat Sohn")]
print(span_end(ch, 0), span_end(ch, 1))
a = project_fact_candidate("mika", rows)
b = project_fact_candidate("noa", rows)
print(a["id"], a["claim"], a["history"])
print(b["id"], b["claim"], b["history"])
"#;
    assert_eq!(run_probe(probe), "None None\nmika Mika []\nnoa Noa []");
}

#[test]
fn an_earlier_hit_on_a_late_coexistence_axis_stays_itself() {
    // Ruling O-3: multivaluedness belongs to the WHOLE axis, wherever the
    // coexistence sits -- here at the END of the chain, with the hit on the
    // strictly earlier fact. Ruling history: O-1 projected such a hit onto the
    // smallest id of the newest set; under O-3 that tie-break survives only for
    // the degenerate one-member set, i.e. the functional axis.
    //
    // The two sessions on the coexisting pair are the W1 session guard (#13,
    // ruling Q3): the same instant counts as coexistence only when two
    // DIFFERENT conversations stated it. O-3's claim is unchanged, the fixture
    // now carries the origin it always implied.
    let probe = r#"
rows = [
 {"id":"a","subject":"s","predicate":"p","claim":"old","session_id":"s1",
  "valid_from":"2026-01-01T00:00:00Z","valid_until":None,"recorded_at":"2026-01-01T00:00:00Z"},
 {"id":"c","subject":"s","predicate":"p","claim":"new-c","session_id":"s3",
  "valid_from":"2026-02-01T00:00:00Z","valid_until":None,"recorded_at":"2026-02-01T00:00:00Z"},
 {"id":"b","subject":"s","predicate":"p","claim":"new-b","session_id":"s2",
  "valid_from":"2026-02-01T00:00:00Z","valid_until":None,"recorded_at":"2026-02-01T00:00:00Z"},
]
ch = build_chains(rows)[("s","p")]
print(axis_is_multivalued(ch), span_end(ch, 0))
hit_a = project_fact_candidate("a", rows)
print(hit_a["id"], hit_a["claim"], hit_a["history"])
print(project_fact_candidate("b", rows)["id"], project_fact_candidate("c", rows)["id"])
"#;
    assert_eq!(run_probe(probe), "True None\na old []\nb c");
}

#[test]
fn a_multivalued_axis_never_derives_supersession() {
    // Ruling O-3: coexistence anywhere in the chain marks the axis as an
    // ENUMERATION, for good. A third son written months later does not end the
    // two already there -- on such an axis no supersession is derived at all.
    // The two sessions are the W1 session guard, same as in the case above.
    let probe = r#"
rows = [
 {"id":"a","subject":"s","predicate":"hat Sohn","claim":"Mika","session_id":"s1",
  "valid_from":"2026-01-01T00:00:00Z","valid_until":None,"recorded_at":"2026-01-01T00:00:01Z"},
 {"id":"b","subject":"s","predicate":"hat Sohn","claim":"Noa","session_id":"s2",
  "valid_from":"2026-01-01T00:00:00Z","valid_until":None,"recorded_at":"2026-01-01T00:00:02Z"},
 {"id":"c","subject":"s","predicate":"hat Sohn","claim":"Nova",
  "valid_from":"2026-06-01T00:00:00Z","valid_until":None,"recorded_at":"2026-06-01T00:00:00Z"},
]
ch = build_chains(rows)[("s","hat Sohn")]
print(axis_is_multivalued(ch), span_end(ch, 0), span_end(ch, 1), span_end(ch, 2))
for i in ("a", "b", "c"):
    o = project_fact_candidate(i, rows)
    print(o["id"], o["claim"], o["history"])
"#;
    assert_eq!(
        run_probe(probe),
        "True None None None\na Mika []\nb Noa []\nc Nova []"
    );
}

#[test]
fn the_rendered_line_carries_the_history_of_a_superseded_claim() {
    // Ruling O-5: the projection already hands the candidate its predecessors,
    // but the model reads the TEXT block, not the JSON next to it -- so a line
    // that renders the claim alone silently drops the answer to "and what was
    // it before?".
    let probe = r#"
print(render_candidate_line({
 "kind":"fact","legs":["temporal"],"text":"vscode",
 "history":[{"id":"a","claim":"Helix","from":"2026-08-08T19:18:34Z",
             "until":"2026-08-08T19:28:00Z"}]}))
"#;
    assert_eq!(
        run_probe(probe),
        "- [fact temporal] vscode (previously: Helix until 2026-08-08)"
    );
}

#[test]
fn several_predecessors_render_chronologically_in_one_line() {
    // The history list arrives in chain order (ascending valid_from), and the
    // rendering preserves it: the reader gets the sequence, not a set.
    let probe = r#"
print(render_candidate_line({
 "kind":"fact","legs":["keyword","temporal"],"text":"vscode",
 "history":[{"claim":"Emacs","from":"2026-07-01T00:00:00Z","until":"2026-07-15T09:00:00Z"},
            {"claim":"Helix","from":"2026-07-15T09:00:00Z","until":"2026-08-08T19:28:00Z"}]}))
"#;
    assert_eq!(
        run_probe(probe),
        "- [fact keyword/temporal] vscode \
         (previously: Emacs until 2026-07-15; Helix until 2026-08-08)"
    );
}

#[test]
fn a_candidate_without_history_renders_exactly_as_before() {
    // Backwards compatibility is the price of O-5: every line that has nothing
    // to add must stay byte-identical, or the pinned bundle texts of T1-T8 move
    // for no reason.
    let probe = r#"
print(render_candidate_line({"kind":"fact","legs":["temporal"],"text":"vscode","history":[]}))
print(render_candidate_line({"kind":"episode","legs":["keyword"],"text":"Nein, doch vscode."}))
"#;
    assert_eq!(
        run_probe(probe),
        "- [fact temporal] vscode\n- [episode keyword] Nein, doch vscode."
    );
}

#[test]
fn a_window_candidate_renders_the_span_it_was_valid_for() {
    // Window mode does not collapse the chain (ruling B), so the answer is the
    // SEQUENCE of versions -- which is unreadable unless each line says when it
    // held. An open end is named, never rendered as an empty gap.
    let probe = r#"
print(render_candidate_line({
 "kind":"fact","legs":["temporal"],"text":"Helix","history":[],
 "span":{"from":"2026-08-08T19:18:34Z","until":"2026-08-08T19:28:00Z"}}))
print(render_candidate_line({
 "kind":"fact","legs":["temporal"],"text":"vscode","history":[],
 "span":{"from":"2026-08-08T19:28:00Z","until":None}}))
"#;
    assert_eq!(
        run_probe(probe),
        "- [fact temporal] Helix [2026-08-08 -> 2026-08-08]\n\
         - [fact temporal] vscode [2026-08-08 -> open]"
    );
}

#[test]
fn a_functional_axis_closes_only_a_re_assertion() {
    // O-3 was a pure extension of O-1 and W2 narrows both: an axis that never
    // saw coexistence is still not an enumeration, and it still supersedes -- but
    // only where the two rows say the SAME thing. The second arm is the whole
    // package in one line: `vscode` starting later does not end `Helix`, because
    // being later is not an argument about a different statement.
    let probe = r#"
def rows_for(second):
    return [
     {"id":"a","subject":"s","predicate":"editor","claim":"Helix",
      "valid_from":"2026-08-08T19:18:34Z","valid_until":None,
      "recorded_at":"2026-08-08T19:18:39Z"},
     {"id":"b","subject":"s","predicate":"editor","claim":second,
      "valid_from":"2026-08-08T19:28:00Z","valid_until":None,
      "recorded_at":"2026-08-08T19:28:03Z"},
    ]

for second in ("Helix", "vscode"):
    rows = rows_for(second)
    ch = build_chains(rows)[("s","editor")]
    o = project_fact_candidate("a", rows)
    print(second, axis_is_multivalued(ch), span_end(ch, 0), span_end(ch, 1),
          o["id"], o["claim"], len(o["history"]))
"#;
    assert_eq!(
        run_probe(probe),
        "Helix False 2026-08-08T19:28:00Z None b Helix 1\n\
         vscode False None None a Helix 0"
    );
}
