//! 0.3.x follow-up F6 -- a predicate names the subject matter, never the speech
//! act (GitHub #67).
//!
//! The one wrong answer of the 0.2.0 measurement run survived the whole statement
//! identity track. The question asks for a 5K personal best, gold is 25:50, the
//! answer is 27:12, and the store says why:
//!
//! ```text
//! user | has_experience | "setting a personal best time of 27:12 ..." | open
//! user | plans_to_beat  | "personal best time of 25:50"               | open
//! ```
//!
//! Two failures, both at MINT time. The two values sit on two canonical
//! predicates, so the currency question -- which groups by (subject, predicate)
//! -- can never see them in one axis entry. And the newer value was minted as a
//! speech act: `plans_to_beat` is not a claim about the personal best at all.
//! Stable across two independent extraction runs, so it is a property of the
//! lane rather than a bad day.
//!
//! The fix was both halves of the issue's direction, and neither of them
//! destroys the plan/fact distinction -- it MOVES:
//!
//! 1. the intention lives on the STATEMENT, in `fact_kind: foresight`, which is
//!    the column the foresight leg has always read. The predicate is free to
//!    name the matter, so a plan and the fact it is about share an axis. Every
//!    reader that could tell them apart while the predicate said `plans_to_*`
//!    still can: the bundle renders `(planned)`, the nightly currency question
//!    is shown `"intent": "planned"` on the statement.
//! 2. the extractor had to SEE the sibling while it decided, which was a
//!    property of the BATCH PROMPT: the window page fetched a pool and the
//!    prompt phase picked the shown window out of it by subject matter.
//!
//! **Half 2 is retired (wave 5, GitHub #298).** Per-turn extraction has no batch
//! prompt, no window page and no subject-matter selection -- the front model is
//! standing in the turn it is annotating, so there is no window to choose for it
//! and `matter_tokens` has no caller. The eight cases that pinned that selection
//! (and the `matter_tokens` probe under it) are gone with the mechanism; what the
//! model is TOLD about naming the matter rather than the speech act is a property
//! of the shipped extraction contract now, not of a prompt this lane builds.
//!
//! What remains here is half 1, which is untouched by the wave: the marker on the
//! statement, from the store column through the recall bundle to the nightly
//! currency question, plus the seed file that states the rule's authority.
//! Everything runs the REAL `params.script_inline` of `recall` and `dream-glue`
//! against injected store replies, so no model is called and nothing costs
//! anything.

use std::io::Write;
use std::process::{Command, Stdio};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";
const DREAM_CONFIG: &str = "../../templates/memory-hive/dream-glue/config.json";

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

fn script_of(path: &str) -> String {
    let raw = std::fs::read_to_string(path).expect("template config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run a real script with a real stdin document; returns the emitted messages.
fn run(script: &str, doc: serde_json::Value) -> Vec<serde_json::Value> {
    let out = run_script_on_stdin(script, &meclaw_testing::code_stdin(&doc).to_string());
    assert!(
        out.status.success(),
        "script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("message array")
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

/// Run a shipped script over a real stdin document, handing the script to
/// python3 **on stdin** instead of in argv.
///
/// A single argv string is capped at 128 KiB (`MAX_ARG_STRLEN`) and the shipped
/// scripts have grown to within a few KB of that line, so `python3 -c <whole
/// script>` is a harness that breaks on size rather than on behaviour (GH #279,
/// precedent 89a522e4). stdin carries the program, so the document rides inside
/// it and is put under `sys.stdin` before the script runs. From there the script
/// executes exactly as `python3 -c` ran it: same `__main__` globals, same
/// stdout, same exit status.
fn run_script_on_stdin(script: &str, stdin_doc: &str) -> std::process::Output {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "exec(compile(_script, 'cell', 'exec'), globals())\n"
        ),
        serde_json::to_string(script).unwrap(),
        serde_json::to_string(stdin_doc).unwrap(),
    );
    run_python(&src)
}

/// Run a probe against the module body of a script. The `park()` exit at its end
/// is swallowed, so the probe can call the helpers the script defines.
fn probe(config: &str, name: &str, body: &str) -> String {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO('{{\"envelope\": {{}}, \"body\": {{}}, \"params\": {{}}}}')\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, '{}', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}"
        ),
        serde_json::to_string(&script_of(config)).unwrap(),
        name,
        body
    );
    let out = run_python(&src);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

// ================================================================== recall

#[test]
fn a_planned_statement_renders_as_planned_in_the_bundle() {
    // The constraint that binds this whole package: a plan and a fact may now
    // share an axis, so the bundle is the place that has to keep the plan from
    // reading as an accomplished fact. The marker sits directly behind the value
    // it qualifies, before the span and before the history.
    let out = probe(
        RECALL_CONFIG,
        "recall",
        "base = {\"kind\": \"fact\", \"legs\": [\"keyword\"], \
         \"text\": \"personal best time of 25:50\"}\n\
         print(render_candidate_line(dict(base, intent=\"planned\")))\n\
         print(render_candidate_line(dict(base, intent=\"planned\", \
         span={\"from\": \"2027-03-01T00:00:00Z\", \"until\": None}, \
         history=[{\"claim\": \"25:00\", \"until\": \"2026-08-10T00:00:00Z\"}])))\n",
    );
    assert_eq!(
        out,
        "- [fact keyword] personal best time of 25:50 (planned)\n\
         - [fact keyword] personal best time of 25:50 (planned) [2027-03-01 -> open] \
         (previously: 25:00 until 2026-08-10)"
    );
}

#[test]
fn a_statement_that_happened_renders_byte_for_byte_as_it_did_before() {
    // O-5's standing promise, one annotation later: a marker that is absent
    // costs no byte. The pinned bundle texts of T1-T8 move the moment this stops
    // holding.
    let out = probe(
        RECALL_CONFIG,
        "recall",
        "print(render_candidate_line({\"kind\": \"fact\", \"legs\": [\"keyword\"], \
         \"text\": \"27:12\"}))\n\
         print(render_candidate_line({\"kind\": \"fact\", \"legs\": [\"keyword\"], \
         \"text\": \"27:12\", \"intent\": \"\"}))\n",
    );
    assert_eq!(out, "- [fact keyword] 27:12\n- [fact keyword] 27:12");
}

/// The `t1-emit` document: the fan-in of a tier-1 request, with two fact hits of
/// one axis -- what happened, and what is planned.
fn emit_doc() -> serde_json::Value {
    let facts = serde_json::json!([
        {"id": "old", "subject": "user", "canonical_subject": "user",
         "predicate": "personal_record", "canonical_predicate": "personal_record",
         "claim": "personal best time of 27:12 in the charity 5K run",
         "fact_kind": "experience", "valid_from": "2026-03-01T00:00:00Z",
         "valid_until": None::<String>, "expired_at": None::<String>,
         "superseded_by": None::<String>, "confidence": 90},
        {"id": "plan", "subject": "user", "canonical_subject": "user",
         "predicate": "personal_record", "canonical_predicate": "personal_record",
         "claim": "personal best time of 25:50",
         "fact_kind": "foresight", "valid_from": "2026-08-10T00:00:00Z",
         "valid_until": "2027-06-01T00:00:00Z", "expired_at": None::<String>,
         "superseded_by": None::<String>, "confidence": 80}
    ]);
    let fused = serde_json::json!({
        "candidates": [
            {"kind": "fact", "id": "old", "score": 0.05, "legs": ["keyword"]},
            {"kind": "fact", "id": "plan", "score": 0.04, "legs": ["keyword"]}
        ],
        "legs_present": ["keyword"],
        "leg_sizes": {"keyword": 2, "semantic": 0, "graph": 0, "temporal": 0},
        "semantic_degraded": true
    });
    let rows = serde_json::json!([
        {"request_id": "r1", "leg": "fused", "payload": fused.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-ep", "payload": "[]", "fired": 1},
        {"request_id": "r1", "leg": "hyd-fact", "payload": facts.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-axis", "payload": "[]", "fired": 1}
    ]);
    serde_json::json!({
        "header": {
            "context": {"mem_phase": "t1-emit", "recall_id": "r1", "memory_tier": "1",
                        "recall_query": "what is my 5K personal best?",
                        "recall_as_of": "2026-08-20T00:00:00Z",
                        "recall_window_from": "", "recall_window_to": ""},
            "hop": {"operation": "select"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

#[test]
fn only_the_intention_of_a_shared_axis_carries_the_marker() {
    // The projection, not the renderer: `intent` is derived from the STORED
    // `fact_kind` of the candidate that survives, so the bundle cannot mark a
    // plan the store does not call one -- and cannot fail to mark one it does.
    let msgs = run(&script_of(RECALL_CONFIG), emit_doc());
    let bundle: serde_json::Value = serde_json::from_str(
        msgs[0]["system"]["memory"]["bundle"]["text"]
            .as_str()
            .unwrap(),
    )
    .expect("bundle");
    let candidates = bundle["candidates"].as_array().expect("candidates");
    assert_eq!(
        candidates.len(),
        2,
        "both statements are answers of their own"
    );
    for c in candidates {
        let planned = c["text"].as_str().unwrap_or_default().contains("25:50");
        assert_eq!(
            c.get("intent").is_some(),
            planned,
            "the marker followed something other than fact_kind: {c}"
        );
    }
    // #281: the readable half is the PAYLOAD form now -- the row carries the
    // axis, the day the statement started and then the annotations, in the same
    // order the diagnostic line uses. The marker still has to stand in the text
    // the model reads, which is the whole point of this assertion.
    let rendered = msgs[0]["messages"][0]["text"].as_str().expect("rendered");
    assert!(
        rendered.contains("personal best time of 25:50   since 2026-08-10 (planned)"),
        "the model reads THIS text, so the marker has to stand in it:\n{rendered}"
    );
    let happened = rendered
        .lines()
        .find(|l| l.contains("charity 5K run"))
        .unwrap_or_else(|| panic!("the statement that happened is gone:\n{rendered}"));
    assert!(
        !happened.contains("(planned)"),
        "and it may never stand on the statement that happened:\n{rendered}"
    );
}

// =============================================================== dream-glue

const RUN: &str = "r1";
const TO: &str = "2026-08-20T03:00:00Z";

fn d_reply(phase: &str, operation: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": phase,
                        "dream_run": RUN, "dream_to": TO},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

/// One scan row of the canonicalisation round.
fn scan_row(id: &str, claim: &str, kind: &str, from: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "subject": "user", "canonical_subject": "user",
        "predicate": "personal_record", "canonical_predicate": "personal_record",
        "claim": claim, "canonical_claim": claim, "fact_kind": kind,
        "valid_from": from, "recorded_at": from
    })
}

/// The axis entries the scan parks for a set of rows.
fn parked_axes(rows: serde_json::Value) -> serde_json::Value {
    let msgs = run(
        &script_of(DREAM_CONFIG),
        d_reply("canon-scan", "select", rows),
    );
    let args = args_of(&msgs[0]);
    assert_eq!(args["row"]["kind"], "canon-scan");
    let parked: serde_json::Value =
        serde_json::from_str(args["row"]["payload"].as_str().expect("payload")).expect("scan");
    parked["axes"].clone()
}

#[test]
fn the_currency_question_sees_which_statement_is_an_intention() {
    // The price of moving the speech act off the predicate: the two statements
    // now reach the judge in ONE axis entry -- which is the whole point -- and
    // the judge has to be able to tell which of them merely INTENDS something,
    // or it would close a fact because somebody plans otherwise.
    let axes = parked_axes(serde_json::json!([
        scan_row(
            "old",
            "personal best time of 27:12 in the charity 5K run",
            "experience",
            "2026-03-01T00:00:00Z"
        ),
        scan_row(
            "plan",
            "personal best time of 25:50",
            "foresight",
            "2026-08-10T00:00:00Z"
        )
    ]));
    let axes = axes.as_array().expect("axes");
    assert_eq!(
        axes.len(),
        1,
        "ONE axis entry -- this is the grouping the issue is about: {axes:?}"
    );
    assert_eq!(axes[0]["predicate"], "personal_record");
    let statements = axes[0]["statements"].as_array().expect("statements");
    assert_eq!(statements.len(), 2);
    assert!(
        statements[0].get("intent").is_none(),
        "what happened carries no marker: {}",
        statements[0]
    );
    assert_eq!(
        statements[1]["intent"], "planned",
        "and the intention says so: {}",
        statements[1]
    );
}

#[test]
fn an_axis_without_a_plan_is_offered_exactly_as_it_always_was() {
    // The invariance half: the key exists only when it says something, so a
    // memory that holds no plan on a multi-statement axis builds byte for byte
    // the payload it built before this package.
    let axes = parked_axes(serde_json::json!([
        scan_row(
            "a",
            "favorite editor is helix",
            "world",
            "2026-03-01T00:00:00Z"
        ),
        scan_row(
            "b",
            "favorite editor is zed",
            "world",
            "2026-08-10T00:00:00Z"
        )
    ]));
    assert_eq!(
        axes[0]["statements"],
        serde_json::json!([
            {"id": "a", "claim": "favorite editor is helix", "since": "2026-03-01T00:00:00Z",
             "last_asserted": "2026-03-01T00:00:00Z", "assertions": 1},
            {"id": "b", "claim": "favorite editor is zed", "since": "2026-08-10T00:00:00Z",
             "last_asserted": "2026-08-10T00:00:00Z", "assertions": 1}
        ])
    );
}

#[test]
fn the_night_is_told_that_an_intention_never_ends_a_fact() {
    // The F4 discipline: a constraint stands with the question whose ANSWERS it
    // guards, and this one guards `closures`. So it renders with question 3 and
    // never without it -- a night with no currency question must not pay a token
    // for a rule about closures it cannot make.
    let out = probe(
        DREAM_CONFIG,
        "dream-glue",
        "needle = 'is an INTENTION and not something that happened'\n\
         for present in ({'axes'}, {'predicates'}, {'cardinality'}, \
         {'entity_pairs'}, {'axes', 'cardinality'}, set()):\n\
         \x20   text = canon_instructions(present)\n\
         \x20   print(int('axes' in present), int(needle in text), \
         int('3. `axes`' in text))\n",
    );
    for line in out.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(
            cols[0], cols[1],
            "the rule and its question have to stand or fall together: {line}"
        );
        assert_eq!(
            cols[0], cols[2],
            "and both with the section they read: {line}"
        );
    }
}

#[test]
fn the_scan_reads_the_kind_of_every_statement_it_offers() {
    // The marker is derived from a STORED column, so the column has to travel.
    // Without it every statement would look like something that happened.
    //
    // The claim is unchanged; only the phase that EMITS the scan moved. Since GH
    // #73 the identity questions read the closed rows first, so the fact page is
    // emitted one phase later -- from `canon-closed`, which with no closed row to
    // report emits nothing else.
    let msgs = run(
        &script_of(DREAM_CONFIG),
        d_reply("canon-closed", "select", serde_json::json!([])),
    );
    let scan = args_of(&msgs[0]);
    assert_eq!(scan["table"], "facts");
    assert!(
        scan["columns"]
            .as_array()
            .expect("columns")
            .iter()
            .any(|c| c == "fact_kind"),
        "the scan cannot tell a plan from a fact: {}",
        scan["columns"]
    );
}

// ============================================================ the seed file

#[test]
fn the_seed_file_states_the_fate_of_the_speech_act_predicates() {
    // predicate-core.json is the authority for the vocabulary, so the rule that
    // decides which keys may exist at all belongs in it -- and so does the
    // reason no example of it is seeded with a cardinality.
    let raw = std::fs::read_to_string("../../templates/memory-hive/predicate-core.json")
        .expect("predicate-core.json");
    let doc: serde_json::Value = serde_json::from_str(&raw).expect("seed json");
    let rule = &doc["a_predicate_names_the_subject_matter"];
    assert!(
        rule["speech_act_predicates"].is_string(),
        "the class needs a documented fate, not silence"
    );
    let examples = rule["subject_matter_examples"]
        .as_array()
        .expect("the examples");
    assert!(!examples.is_empty());
    for e in examples {
        let instead = e["instead_of"].as_str().expect("instead_of");
        assert!(
            instead.starts_with("plans_to_")
                || instead.starts_with("wants_to_")
                || instead.starts_with("hopes_to_"),
            "an example that is not a speech act teaches the wrong rule: {instead}"
        );
        assert_eq!(
            e["with"], "fact_kind: foresight",
            "the intention has to land somewhere, or the rule reads as 'drop it'"
        );
    }
    let seeded: Vec<&str> = doc["predicates"]
        .as_array()
        .expect("predicates")
        .iter()
        .map(|p| p["predicate"].as_str().unwrap_or_default())
        .collect();
    for p in &seeded {
        assert!(
            !(p.starts_with("plans_to_")
                || p.starts_with("wants_to_")
                || p.starts_with("hopes_to_")),
            "a seeded speech act would make the rule unfollowable: {p}"
        );
    }
    assert!(
        !seeded.contains(&"personal_record"),
        "the example is deliberately NOT seeded: a cardinality nobody measured is \
         irreversible on an over-cap axis (GH #66)"
    );
}
