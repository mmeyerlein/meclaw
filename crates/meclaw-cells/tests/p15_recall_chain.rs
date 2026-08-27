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
fn several_predecessors_render_as_the_one_that_was_replaced() {
    // Ruling S6 (#296). Two deliberate decisions disagreed here, and this is the
    // one that won: the reader gets the claim this one REPLACED, not the sequence
    // that led to it. The history list still arrives in chain order (ascending
    // valid_from), so the last entry is the immediate predecessor and everything
    // older is the history OF that history -- a question nobody asked this round.
    //
    // The cut is not made here: `history_entries` makes it, and the JSON payload
    // beside this text asks the same function, so the two renderers cannot part
    // again (they had, for two releases). The whole chain is untouched in
    // `recall_diagnostic.candidates[].history` -- pinned in
    // `gh296_the_bundle_is_payload_not_plumbing::a_superseded_claim_still_announces_itself_without_its_chain`.
    let probe = r#"
print(render_candidate_line({
 "kind":"fact","legs":["keyword","temporal"],"text":"vscode",
 "history":[{"claim":"Emacs","from":"2026-07-01T00:00:00Z","until":"2026-07-15T09:00:00Z"},
            {"claim":"Helix","from":"2026-07-15T09:00:00Z","until":"2026-08-08T19:28:00Z"}]}))
"#;
    assert_eq!(
        run_probe(probe),
        "- [fact keyword/temporal] vscode (previously: Helix until 2026-08-08)"
    );
}

#[test]
fn the_text_and_the_json_name_the_same_one_predecessor() {
    // The whole point of S6 in one probe: a four-deep chain, and both halves of
    // the message name the LAST entry -- the claim this one replaced -- because
    // both asked `history_entries`. The third line is the counter-proof in the
    // small: the candidate record handed in is not modified, so what the trace
    // carries is untouched by what the renderers show.
    let probe = r#"
import json
c = {"kind":"fact","legs":["temporal"],"subject":"person:example",
     "predicate":"favorite_editor","text":"helix",
     "valid_from":"2026-04-01T00:00:00Z",
     "history":[{"claim":"vim","until":"2026-02-01T00:00:00Z"},
                {"claim":"emacs","until":"2026-03-01T00:00:00Z"},
                {"claim":"kakoune","until":"2026-04-01T00:00:00Z"}]}
print(render_candidate_line(c))
print(json.dumps(payload_candidate(c)["previously"], sort_keys=True))
print(len(history_entries(c)), len(c["history"]))
"#;
    assert_eq!(
        run_probe(probe),
        "- [fact temporal] person:example favorite_editor: helix \
         (previously: kakoune until 2026-04-01)\n\
         [{\"claim\": \"kakoune\", \"until\": \"2026-04-01\"}]\n\
         1 3"
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

// ==================================================================== GH #295
// A tier-0 recall asks the store ONCE.
//
// Before this task the three fixed legs cost nine store round trips: three
// selects, three `recall_scratch` inserts to park the projections, a select to
// see whether all three had landed, a guarded update to elect the one hop that
// may fire, and a last select to read the parked payloads back. The fan-in was
// bookkeeping for a fan-out that the store can answer in one message (GH #295,
// bundle), and the parked rows were state the round did not need.
//
// The two tests below run the SHIPPED script over real stdin documents, so the
// count they measure is the count the hive pays.

/// Run the real `script_inline` over a real stdin document and return what it
/// emitted. Unlike [`run_probe`] nothing is stubbed: the document decides which
/// branch runs, and the emission is the answer.
fn run_recall(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'recall', 'exec'), globals())\n"
        ),
        serde_json::to_string(&recall_script()).unwrap(),
        serde_json::to_string(&meclaw_testing::code_stdin(&doc).to_string()).unwrap(),
    );
    let out = run_python(&src);
    assert!(
        out.status.success(),
        "recall exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

/// The asking round of a tier-0 request. One member asking one agent in one
/// room — the read path is fail-closed on it since #244, and the fixture rows
/// below carry a hidden sibling in each leg so the gate is measured, not
/// assumed.
fn tier0_context() -> serde_json::Value {
    serde_json::json!({
        "memory_tier": "0", "session_id": "s-21",
        "recall_query": "what do we know?",
        "audience_now": r#"["member:user","agent:assistant"]"#,
        "channel": "c-21", "memory_holder": "user:alex"
    })
}

/// The rows the store hands back per leg. Each leg carries one row the asking
/// round may see and one it may not; the hidden foresight row shares the axis of
/// the visible one, which is what earns it the `supersession_unknown` marker.
fn leg_rows(id: &str) -> serde_json::Value {
    match id {
        "r-leg-episodes" => serde_json::json!([
            {"id": "e1", "session_id": "s-21", "sender": "user",
             "content": "The editor of choice is vscode.",
             "happened_at": "2026-08-09T09:00:00.000000Z",
             "recorded_at": "2026-08-09T09:00:01.000000Z",
             "channel": "c-21", "audience_set": r#"["member:user","agent:assistant"]"#},
            {"id": "e2", "session_id": "s-21", "sender": "user", "content": "A private aside.",
             "happened_at": "2026-08-09T08:00:00.000000Z",
             "recorded_at": "2026-08-09T08:00:01.000000Z",
             "channel": "c-other", "audience_set": r#"["member:other"]"#}
        ]),
        "r-leg-beliefs" => serde_json::json!([
            {"id": "b1", "holder": "user:alex",
             "statement": "alex prefers keyboard driven tools", "confidence": 0.9,
             "active": 1, "updated_at": "2026-08-09T07:00:00.000000Z",
             "audience_set": r#"["member:user","agent:assistant"]"#},
            {"id": "b2", "holder": "user:alex", "statement": "stale", "confidence": 0.4,
             "active": 0, "updated_at": "2026-08-08T07:00:00.000000Z",
             "audience_set": r#"["member:user","agent:assistant"]"#}
        ]),
        _ => serde_json::json!([
            {"id": "f1", "subject": "user:alex", "canonical_subject": "user:alex",
             "predicate": "plans", "canonical_predicate": "plans",
             "claim": "ship the release on friday", "fact_kind": "foresight",
             "valid_from": "2026-08-10T00:00:00.000000Z", "valid_until": null,
             "expired_at": null, "confidence": 0.8, "channel": "c-21",
             "audience_set": r#"["member:user","agent:assistant"]"#},
            {"id": "f2", "subject": "user:alex", "canonical_subject": "user:alex",
             "predicate": "plans", "canonical_predicate": "plans",
             "claim": "hidden newer plan", "fact_kind": "foresight",
             "valid_from": "2026-08-11T00:00:00.000000Z", "valid_until": null,
             "expired_at": null, "confidence": 0.8, "channel": "c-other",
             "audience_set": r#"["member:other"]"#}
        ]),
    }
}

/// The store's answer to the legs message, in the shape the `store` cell really
/// builds for N ops (GH #295, W4 T19): schema-pure `tool_result` turns in call
/// order, the per-leg metadata beside them in the body's top-level `results[]`
/// slot, and a header describing the bundle as a whole.
fn bundle_reply(call: &serde_json::Value, refused_leg: Option<(&str, &str)>) -> serde_json::Value {
    let mut turns = Vec::new();
    let mut results = Vec::new();
    for turn in call["messages"].as_array().expect("tool_call turns") {
        let id = turn["id"].as_str().expect("tool_call id");
        let args: serde_json::Value =
            serde_json::from_str(turn["text"].as_str().expect("args")).expect("args json");
        let operation = args["operation"].as_str().expect("operation").to_string();
        match refused_leg {
            Some((leg, code)) if leg == id => {
                turns.push(serde_json::json!({
                    "origin": "tool", "type": "tool_result", "id": id,
                    "text": "no such column: digest"
                }));
                results.push(serde_json::json!({
                    "tool_call_id": id, "operation": operation, "rows_affected": 0,
                    "duration_ms": 1, "error_code": code
                }));
            }
            _ => {
                let rows = leg_rows(id);
                turns.push(serde_json::json!({
                    "origin": "tool", "type": "tool_result", "id": id,
                    "text": rows.to_string()
                }));
                results.push(serde_json::json!({
                    "tool_call_id": id, "operation": operation,
                    "rows_affected": rows.as_array().expect("rows").len(),
                    "duration_ms": 1
                }));
            }
        }
    }
    let errors = if refused_leg.is_some() { 1 } else { 0 };
    let rows: i64 = results
        .iter()
        .map(|r| r["rows_affected"].as_i64().unwrap_or_default())
        .sum();
    let mut context = tier0_context();
    context["mem_phase"] = call["header"]["phase"].clone();
    context["recall_id"] = call["header"]["recall_id"].clone();
    context["store_origin"] = serde_json::json!("recall");
    serde_json::json!({
        "header": {
            "context": context,
            "hop": {"operation": "bundle", "rows_affected": rows, "duration_ms": 3,
                    "bundle_errors": errors}
        },
        "messages": turns,
        "results": results
    })
}

/// The tier-0 bundle this fixture produces — byte for byte what the nine-round-
/// trip chain produced for the same rows before GH #295. The projections did not
/// move: the hidden episode and the inactive belief are gone, and the visible
/// foresight fact is marked because its axis lost a version this round may not
/// see.
const TIER0_BUNDLE: &str = concat!(
    r#"{"beliefs": [{"confidence": 0.9, "id": "b1", "statement": "alex prefers keyboard driven tools"}], "#,
    r#""episodes": [{"content": "The editor of choice is vscode.", "happened_at": "2026-08-09T09:00:00.000000Z", "id": "e1", "sender": "user"}], "#,
    r#""foresight": [{"claim": "ship the release on friday", "id": "f1", "predicate": "plans", "subject": "user:alex", "supersession_unknown": true, "valid_from": "2026-08-10T00:00:00.000000Z"}], "#,
    r#""query": "what do we know?", "tier": 0, "token_estimate": 96}"#
);

/// The done-when of #295: the three fixed legs cost ONE store round trip, the
/// round parks nothing in `recall_scratch`, and the bundle is the one the nine
/// round trips used to build.
#[test]
fn a_tier_zero_recall_costs_one_store_round_trip() {
    let request = serde_json::json!({
        "header": {"context": tier0_context(), "hop": {"phase": "recall"}},
        "messages": [{"origin": "user", "type": "text", "id": "q", "text": "what do we know?"}]
    });
    let asked = run_recall(request);
    let store_ops = |msgs: &[serde_json::Value]| -> usize {
        msgs.iter()
            .filter(|m| m["header"]["route"] == "rstore")
            .count()
    };
    assert_eq!(
        store_ops(&asked),
        1,
        "a tier-0 request must reach the store once: {}",
        serde_json::to_string(&asked).unwrap()
    );
    assert_eq!(asked.len(), 1, "and emit nothing else beside it");
    let call = &asked[0];
    assert_eq!(call["header"]["phase"], "legs");
    let ids: Vec<&str> = call["messages"]
        .as_array()
        .expect("turns")
        .iter()
        .map(|t| t["id"].as_str().expect("id"))
        .collect();
    assert_eq!(
        ids,
        ["r-leg-episodes", "r-leg-beliefs", "r-leg-foresight"],
        "the one message carries all three legs, in leg order"
    );

    let answered = run_recall(bundle_reply(call, None));
    assert_eq!(
        store_ops(&answered),
        0,
        "the bundle answers the round outright — nothing goes back to the store: {}",
        serde_json::to_string(&answered).unwrap()
    );
    assert_eq!(answered.len(), 1);
    assert_eq!(answered[0]["header"]["route"], "bundle");
    assert_eq!(
        answered[0]["system"]["memory"]["bundle"]["text"], TIER0_BUNDLE,
        "the bundle content moved"
    );
    assert_eq!(
        answered[0]["messages"][0]["text"],
        "MEMORY (tier 0, deterministic bundle)\n\
         - belief: alex prefers keyboard driven tools\n\
         - open: ship the release on friday (currency unknown: cannot vouch that this still holds)\n\
         - user: The editor of choice is vscode."
    );

    // (c) nothing was parked: no op of this round names `recall_scratch`.
    let spoken = serde_json::to_string(&(&asked, &answered)).unwrap();
    assert!(
        !spoken.contains("recall_scratch"),
        "a tier-0 round must write no scratch row: {spoken}"
    );
}

/// The #343 guard, widened for the bundle (project ruling 2026-08-22, option C).
/// A bundle is not a transaction, so one leg can be refused while its siblings
/// carry rows — and a bundle with a refused leg is exactly the "memory knows
/// nothing" bundle #343 exists to prevent. Tier 0 keeps today's strictness: any
/// refused leg is terminal on the same `reject` lane, and the phase does not
/// advance.
#[test]
fn a_bundle_with_a_refused_leg_stops_the_recall() {
    let request = serde_json::json!({
        "header": {"context": tier0_context(), "hop": {"phase": "recall"}},
        "messages": [{"origin": "user", "type": "text", "id": "q", "text": "what do we know?"}]
    });
    let call = run_recall(request).remove(0);
    let out = run_recall(bundle_reply(
        &call,
        Some(("r-leg-beliefs", "query_timeout")),
    ));
    let spoken = serde_json::to_string(&out).unwrap();
    assert_eq!(out.len(), 1, "one terminal message: {spoken}");
    assert_eq!(out[0]["header"]["route"], "reject", "{spoken}");
    assert_eq!(
        out[0]["header"]["reject_reason"], "store_refused",
        "{spoken}"
    );
    assert_eq!(
        out[0]["header"]["store_error"], "query_timeout",
        "the reject names the refused leg's error_code: {spoken}"
    );
    assert_eq!(
        out[0]["header"]["store_operation"], "select",
        "and the op that leg ran: {spoken}"
    );
    assert!(
        !out.iter().any(|m| m["header"]["route"] == "bundle"),
        "a poisoned bundle must never be answered as a bundle: {spoken}"
    );
}
