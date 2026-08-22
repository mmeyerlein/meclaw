//! GH #281 — the readable half tells a FACT from an UTTERANCE.
//!
//! The text a model reads used to be one flat list in which every row opened
//! with the same bracket: `- [fact keyword] …`, `- [episode graph] …`. Two
//! things were wrong with that. The bracket mixed WHAT a row is (a derived,
//! dated statement vs. a sentence somebody actually said) with WHICH retrieval
//! leg nominated it — and a leg name answers nothing a reader asked. And a past
//! QUESTION — `what is my favourite colour?` — rendered in the same shape as an
//! asserted fact, which is the failure this issue is named after: a question
//! read back as an answer.
//!
//! So the kind becomes the SECTION. Facts stand under a header that says they
//! are extracted, canonical and dated; utterances stand under one that says
//! they are verbatim and uninterpreted, each opening with who said it and when.
//! Nothing is lost: `render_candidate_line` keeps every byte of its output and
//! becomes the diagnostic renderer, which is what R3, W1, R7, #67 and #15 are
//! pinned against.

use std::io::Write;
use std::process::{Command, Stdio};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string — the same substitution the colony performs on instantiation.
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

fn recall_script() -> String {
    let raw = std::fs::read_to_string(RECALL_CONFIG).expect("recall config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
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

/// Run the shipped script against a real stdin document; return the emissions.
fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let out = run_script_on_stdin(
        &recall_script(),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
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

/// Run a probe against the module body of the real script — the same idiom the
/// other renderer pins (`r3_axis_rendering`, `w1_currency_marker`) use. The
/// `park()` exit at the end of the script is swallowed so the probe can call the
/// helpers it defines.
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

/// A fact row as the hydration select returns it.
fn fact(id: &str, predicate: &str, claim: &str, valid_from: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "session_id": "s-1", "subject": "user",
                       "predicate": predicate, "claim": claim,
                       "canonical_subject": "user", "canonical_predicate": predicate,
                       "canonical_claim": claim, "valid_from": valid_from,
                       "valid_until": serde_json::Value::Null,
                       "recorded_at": valid_from,
                       "expired_at": serde_json::Value::Null,
                       "superseded_by": serde_json::Value::Null,
                       "episode_id": format!("ep-of-{id}"),
                       "fact_kind": "state", "confidence": 90})
}

/// An episode row as the hydration select returns it.
fn episode(id: &str, sender: &str, content: &str, happened_at: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "session_id": "s-1", "sender": sender,
                       "content": content, "happened_at": happened_at,
                       "recorded_at": happened_at})
}

/// The `t1-emit` document of a tier-1 request — the tier is what keeps the run
/// on the emit path instead of routing it to the dialectic.
fn doc_of(
    fused: serde_json::Value,
    eps: serde_json::Value,
    facts: serde_json::Value,
) -> serde_json::Value {
    let rows = serde_json::json!([
        {"request_id": "r1", "leg": "fused", "payload": fused.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-ep", "payload": eps.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-fact", "payload": facts.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-axis", "payload": "[]", "fired": 1}
    ]);
    serde_json::json!({
        "header": {
            "context": {"mem_phase": "t1-emit", "recall_id": "r1", "memory_tier": "1",
                        "recall_query": "what is my favourite colour?",
                        "recall_as_of": "2026-08-12T00:00:00Z",
                        "recall_window_from": "", "recall_window_to": ""},
            "hop": {"operation": "select"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

/// The text the model reads.
fn rendered(msgs: &[serde_json::Value]) -> String {
    msgs.iter()
        .find(|m| m["header"]["route"] == "bundle")
        .expect("a tier-1 request emits a bundle message")["messages"][0]["text"]
        .as_str()
        .expect("rendered text")
        .to_string()
}

const FACTS_HEADER: &str = "FACTS (extracted, canonical, dated)";
const SAID_HEADER: &str = "WHAT WAS SAID (verbatim, not interpreted)";

/// Two facts and two turns, one of the turns being a past QUESTION — the shape
/// #281 is named after.
fn mixed() -> serde_json::Value {
    let fused = serde_json::json!({
        "candidates": [
            {"kind": "fact", "id": "f-1", "score": 0.09, "legs": ["keyword"]},
            {"kind": "episode", "id": "e-1", "score": 0.08, "legs": ["keyword", "graph"]},
            {"kind": "fact", "id": "f-2", "score": 0.07, "legs": ["semantic"]},
            {"kind": "episode", "id": "e-2", "score": 0.06, "legs": ["temporal"]}
        ],
        "legs_present": ["keyword", "semantic", "graph", "temporal"],
        "leg_sizes": {"keyword": 2, "semantic": 1, "graph": 1, "temporal": 1},
        "leg_capped": {},
        "semantic_degraded": false
    });
    let facts = serde_json::json!([
        fact(
            "f-1",
            "favorite_color",
            "Blau",
            "2026-01-05T09:00:00.000000Z"
        ),
        fact(
            "f-2",
            "favorite_editor",
            "helix",
            "2026-01-06T09:00:00.000000Z"
        )
    ]);
    let eps = serde_json::json!([
        episode(
            "e-1",
            "user",
            "what is my favourite colour?",
            "2026-01-02T09:00:00.000000Z"
        ),
        episode(
            "e-2",
            "assistant",
            "I have it as Blau.",
            "2026-01-02T09:01:00.000000Z"
        )
    ]);
    doc_of(fused, eps, facts)
}

#[test]
fn facts_and_what_was_said_are_two_sections() {
    // The resolution of #281 in one text: two labelled blocks, the derived
    // statements above, the sentences somebody actually said below — each of
    // them opening with the speaker and the day, so a past question can never
    // be read as an asserted answer.
    let text = rendered(&emit(mixed()));

    let facts_at = text
        .find(FACTS_HEADER)
        .unwrap_or_else(|| panic!("no `{FACTS_HEADER}` header:\n{text}"));
    let said_at = text
        .find(SAID_HEADER)
        .unwrap_or_else(|| panic!("no `{SAID_HEADER}` header:\n{text}"));
    assert!(
        facts_at < said_at,
        "what was derived stands above what was said:\n{text}"
    );

    for claim in ["Blau", "helix"] {
        let at = text
            .find(claim)
            .unwrap_or_else(|| panic!("the fact `{claim}` is gone:\n{text}"));
        assert!(
            facts_at < at && at < said_at,
            "the fact `{claim}` belongs under the first header:\n{text}"
        );
    }
    for said in ["what is my favourite colour?", "I have it as Blau."] {
        let at = text
            .rfind(said)
            .unwrap_or_else(|| panic!("the turn `{said}` is gone:\n{text}"));
        assert!(
            said_at < at,
            "the turn `{said}` belongs under the second header:\n{text}"
        );
    }

    // The failure mode the issue names: a QUESTION from an earlier turn. Its
    // line has to say who asked it and on which day before it says anything
    // else — that is what keeps it from reading as an answer.
    let question = text
        .lines()
        .find(|l| l.contains("what is my favourite colour?"))
        .unwrap_or_else(|| panic!("no line carries the past question:\n{text}"));
    assert_eq!(
        question.trim_start(),
        "user on 2026-01-02: \"what is my favourite colour?\"",
        "the past question is attributed and dated:\n{text}"
    );
}

#[test]
fn a_row_carries_no_leg_name_and_no_kind_prefix() {
    // The bracket is gone from every row. The kind is now the section — which
    // is #281's own resolution — and which leg nominated a candidate is
    // retrieval bookkeeping that travels in `recall_diagnostic`.
    //
    // The scope is the ROWS: the `MEMORY (…)` header above them describes the
    // RUN and not a candidate, and re-pointing that line is Task 7's business.
    let text = rendered(&emit(mixed()));

    for prefix in ["[fact ", "[episode "] {
        assert!(
            !text.contains(prefix),
            "the kind is the section, never a bracket on the row — `{prefix}` \
             is still in the text:\n{text}"
        );
    }
    for line in text.lines().skip(1) {
        for leg in ["keyword", "semantic", "graph", "temporal"] {
            assert!(
                !line.contains(leg),
                "a row names no leg — `{leg}` stands in `{line}`:\n{text}"
            );
        }
    }
}

#[test]
fn a_section_with_no_rows_does_not_appear() {
    // A header is a promise that something follows it. An empty one costs
    // budget and invites the model to wonder what it missed.
    let fused = serde_json::json!({
        "candidates": [{"kind": "fact", "id": "f-1", "score": 0.09, "legs": ["keyword"]}],
        "legs_present": ["keyword"],
        "leg_sizes": {"keyword": 1, "semantic": 0, "graph": 0, "temporal": 0},
        "leg_capped": {},
        "semantic_degraded": true
    });
    let facts = serde_json::json!([fact(
        "f-1",
        "favorite_color",
        "Blau",
        "2026-01-05T09:00:00.000000Z"
    )]);
    let text = rendered(&emit(doc_of(fused, serde_json::json!([]), facts)));
    assert!(text.contains(FACTS_HEADER), "facts only:\n{text}");
    assert!(
        !text.contains(SAID_HEADER),
        "nobody said anything this round:\n{text}"
    );

    let fused = serde_json::json!({
        "candidates": [{"kind": "episode", "id": "e-1", "score": 0.09, "legs": ["keyword"]}],
        "legs_present": ["keyword"],
        "leg_sizes": {"keyword": 1, "semantic": 0, "graph": 0, "temporal": 0},
        "leg_capped": {},
        "semantic_degraded": true
    });
    let eps = serde_json::json!([episode(
        "e-1",
        "user",
        "what is my favourite colour?",
        "2026-01-02T09:00:00.000000Z"
    )]);
    let text = rendered(&emit(doc_of(fused, eps, serde_json::json!([]))));
    assert!(text.contains(SAID_HEADER), "turns only:\n{text}");
    assert!(
        !text.contains(FACTS_HEADER),
        "nothing was extracted this round:\n{text}"
    );
}

#[test]
fn the_annotations_that_change_an_answer_survive_the_move() {
    // Everything that can turn a right answer into a wrong one moves WITH the
    // row: a plan marked as one (#67), the span a statement held (P15), the
    // closure the store recorded (W1), the certainty the audience gate cost
    // (R7), the predecessor a claim replaced (R4) and how often a turn repeated
    // (#15) — in the order `render_candidate_line` has used since O-5, out of
    // the one helper both renderers share.
    //
    // A renderer probe rather than a fixture, because two of these annotations
    // are derived from store state a hydration document cannot carry; that the
    // renderer is WIRED to the bundle is what the three tests above prove.
    let probe = r#"
print("\n".join(render_payload_lines([
    {"kind": "fact", "subject": "user", "predicate": "personal_record",
     "text": "25:50", "valid_from": "2027-03-01T00:00:00Z", "intent": "planned"},
    {"kind": "fact", "subject": "user", "predicate": "favorite_color",
     "text": "Blau", "valid_from": "2026-01-01T00:00:00Z",
     "supersession_unknown": True},
    {"kind": "fact", "subject": "user", "predicate": "favorite_editor",
     "text": "vscode", "valid_from": "2026-08-08T00:00:00Z",
     "superseded": "helix",
     "history": [{"claim": "vim", "until": "2026-08-08T00:00:00Z"}]},
    {"kind": "fact", "subject": "user", "predicate": "lives_in",
     "text": "Berlin", "valid_from": "2023-05-23T00:00:00Z",
     "span": {"from": "2023-05-23T00:00:00Z", "until": "2023-05-30T00:00:00Z"}},
    {"kind": "episode", "sender": "user", "text": "hallo",
     "happened_at": "2026-01-02T09:00:00Z", "seen": 3},
])))
"#;
    assert_eq!(
        run_probe(probe),
        "FACTS (extracted, canonical, dated)\n\
         \x20 user personal_record = 25:50   since 2027-03-01 (planned)\n\
         \x20 user favorite_color = Blau   since 2026-01-01 \
         (currency unknown: cannot vouch that this still holds)\n\
         \x20 user favorite_editor = vscode   since 2026-08-08 \
         (superseded by: helix) (previously: vim until 2026-08-08)\n\
         \x20 user lives_in = Berlin   since 2023-05-23 [2023-05-23 -> 2023-05-30]\n\
         WHAT WAS SAID (verbatim, not interpreted)\n\
         \x20 user on 2026-01-02: \"hallo\" (seen: 3)"
    );
}
