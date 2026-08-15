//! Statement identity W1 -- a superseded candidate says so on its own line
//! (GitHub #13, ruling Q6: option B now, the ranking demotion is a separate
//! package).
//!
//! The bundle has carried `previously: ...` since P15: a candidate names the
//! values it replaced. The inverse was missing, and the 0.2.0 P8a run measured
//! what that costs -- the one wrong answer of the judged slice had BOTH values
//! in the bundle, the superseded one ranked first, and nothing marked it. The
//! model reads the rendered text, so the store's own verdict has to stand in it.
//!
//! Superseded candidates stay in the bundle: dropping them would destroy history
//! questions and hide the store's uncertainty. What changes is one annotation,
//! in the one place a candidate becomes a line.

use std::process::Command;

fn script_of(config: &str) -> String {
    let raw = std::fs::read_to_string(config).expect("config");
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    resolve_vars(v["params"]["script_inline"].as_str().unwrap())
}

fn recall_script() -> String {
    script_of("../../templates/memory-hive/recall/config.json")
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

/// Run a probe against the module body of the real script. The `park()` exit at
/// its end is swallowed so the probe can call the helpers the script defines.
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
    let out = Command::new("python3")
        .arg("-c")
        .arg(src)
        .output()
        .expect("python3");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The two versions of one functional axis, with the older one closed by a dream
/// run: `expired_at` is the successor's start, `superseded_by` names it.
const CLOSED_AXIS: &str = r#"
axis = [
 {"id":"old","subject":"user:u1","canonical_subject":"user:u1","predicate":"personal_best_5k",
  "canonical_predicate":"personal_best_5k","claim":"27:12","valid_from":"2023-05-23T00:00:00Z",
  "valid_until":None,"recorded_at":"2023-05-23T00:00:00Z","session_id":"s1",
  "expired_at":"2023-05-30T00:00:00Z","superseded_by":"new"},
 {"id":"new","subject":"user:u1","canonical_subject":"user:u1","predicate":"personal_best_5k",
  "canonical_predicate":"personal_best_5k","claim":"25:50","valid_from":"2023-05-30T00:00:00Z",
  "valid_until":None,"recorded_at":"2023-05-30T00:00:00Z","session_id":"s2",
  "expired_at":None,"superseded_by":None},
]
old, new = axis[0], axis[1]
"#;

#[test]
fn a_closed_statement_names_the_value_that_replaced_it() {
    let probe = String::from(CLOSED_AXIS)
        + "print(superseded_marker(old, axis, \"2026-01-01T00:00:00Z\"))\n";
    assert_eq!(run_probe(&probe), "25:50");
}

#[test]
fn an_open_statement_carries_no_marker_at_all() {
    // The distinction the whole annotation rests on: None means "say nothing",
    // and the empty string means "closed, successor unknown". A marker that
    // could not tell those apart would annotate every line in the bundle.
    let probe = String::from(CLOSED_AXIS)
        + "print(repr(superseded_marker(new, axis, \"2026-01-01T00:00:00Z\")))\n";
    assert_eq!(run_probe(&probe), "None");
}

#[test]
fn a_closure_the_page_cannot_resolve_still_says_that_it_is_closed() {
    // The axis page is bounded (MEMORY_TIER1_AXIS_LIMIT), and W3 will add
    // closures written by a judge that name no successor at all. Losing the
    // marker because the newer VALUE is out of reach would drop the
    // answer-critical half for the sake of the decorative one.
    let probe = String::from(CLOSED_AXIS)
        + "print(repr(superseded_marker(old, [old], \"2026-01-01T00:00:00Z\")))\n";
    assert_eq!(run_probe(&probe), "''");
}

#[test]
fn a_closure_after_the_read_instant_has_not_happened_yet() {
    // Spec D.5: a question about the past is never answered with a fact that did
    // not exist yet -- and that has to hold for the marker too, or an as-of
    // bundle would leak the existence of a later value through its annotation.
    let probe = String::from(CLOSED_AXIS)
        + "print(repr(superseded_marker(old, axis, \"2023-05-25T00:00:00Z\")))\n";
    assert_eq!(run_probe(&probe), "None");
}

#[test]
fn the_rendered_line_carries_the_marker_in_both_forms() {
    let probe = r#"
base = {"kind": "fact", "legs": ["keyword"], "text": "27:12"}
print(render_candidate_line(dict(base, superseded="25:50")))
print(render_candidate_line(dict(base, superseded="")))
"#;
    assert_eq!(
        run_probe(probe),
        "- [fact keyword] 27:12 (superseded by: 25:50)\n\
         - [fact keyword] 27:12 (superseded: yes)"
    );
}

#[test]
fn a_candidate_without_the_marker_renders_exactly_as_before() {
    // O-5's standing promise: an annotation that is absent costs no byte. The
    // pinned bundle texts of T1-T8 move the moment this stops holding.
    let probe = r#"
print(render_candidate_line({"kind": "fact", "legs": ["keyword"], "text": "27:12"}))
print(render_candidate_line({"kind": "episode", "legs": ["semantic", "graph"],
                             "text": "hallo", "seen": 3}))
"#;
    assert_eq!(
        run_probe(probe),
        "- [fact keyword] 27:12\n- [episode semantic/graph] hallo (seen: 3)"
    );
}

#[test]
fn the_marker_sits_between_the_span_and_the_history() {
    // Order is the reading order of one line: what it is, when it held, whether
    // it still holds, what it replaced, how often it was seen. The currency
    // verdict comes before the history because it is the half that changes the
    // answer.
    let probe = r#"
print(render_candidate_line({
    "kind": "fact", "legs": ["temporal"], "text": "27:12",
    "span": {"from": "2023-05-23T00:00:00Z", "until": "2023-05-30T00:00:00Z"},
    "superseded": "25:50",
    "history": [{"claim": "31:04", "until": "2023-05-23T00:00:00Z"}],
    "seen": 2}))
"#;
    assert_eq!(
        run_probe(probe),
        "- [fact temporal] 27:12 [2023-05-23 -> 2023-05-30] (superseded by: 25:50) \
         (previously: 31:04 until 2023-05-23) (seen: 2)"
    );
}

#[test]
fn every_select_behind_a_rendered_fact_carries_both_closure_columns() {
    // The marker reads what the store WROTE, so the columns have to travel:
    // the axis page feeds the projected candidate, the fact hydration feeds the
    // fallback candidate when the page carried no matching axis.
    let script = recall_script();
    for anchor in [
        "AXIS_LIMIT}, \"t1-hyd-axis\"",
        "AXIS_LIMIT}, \"t1-temporal\"",
        "}, \"t1-hyd-fact\"",
    ] {
        let start = script
            .find(anchor)
            .unwrap_or_else(|| panic!("no select at `{anchor}`"));
        let head = &script[..start];
        let select = &head[head
            .rfind("\"operation\": \"select\"")
            .unwrap_or_else(|| panic!("`{anchor}` is not fed by a select"))..];
        for column in ["\"expired_at\"", "\"superseded_by\""] {
            assert!(
                select.contains(column),
                "the select at `{anchor}` does not fetch {column}"
            );
        }
    }
}
