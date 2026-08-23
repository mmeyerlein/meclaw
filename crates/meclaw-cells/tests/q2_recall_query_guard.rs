//! GH #88 — the query hygiene guard of the recall lane.
//!
//! The defect: the recall cell applied no hygiene to `recall_query` before the
//! tier-1 leg fan. The semantic leg embedded the caller's query verbatim,
//! however long, and the keyword leg capped the FTS tokens at the FIRST 24 —
//! in the common contamination shape exactly the wrong half, because a tool
//! preamble sits at the head and the real question at the tail. MemPalace
//! measured that shape in production at R@10 89.8 % → 1.0 % (mempalace#333).
//!
//! The guard is deterministic, LLM-free, and runs at request entry before the
//! leg fan — no store change, no new cell, no model call. Two halves are pinned
//! here: the pure function (`sanitize_query`, `fts_match`) and the two places a
//! caller can observe its verdict (the tier-0 and the tier-1 bundle).
//!
//! The probe harness is deliberately duplicated from `p15_recall_window.rs`,
//! for the same reason stated there: the REAL `params.script_inline` runs,
//! `${VAR:-default}` is resolved the way the colony resolves it at
//! instantiation, and the module body runs against a stub stdin.

use std::io::Write;
use std::process::{Command, Stdio};

fn recall_script() -> String {
    let raw = std::fs::read_to_string("../../templates/memory-hive/recall/config.json")
        .expect("recall config");
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    resolve_vars(v["params"]["script_inline"].as_str().unwrap())
}

/// `${VAR:-default}` becomes the default, a bare `${VAR}` becomes the empty
/// string — the same substitution the colony performs at instantiation.
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

/// Runs the module body against an empty stdin (it parks) and then evaluates
/// `probe` against the module globals.
fn run_probe(probe: &str) -> String {
    run_with_stdin(&stdin_doc(&serde_json::json!({})).to_string(), probe)
}

/// The three-object stdin document the substrate builds: its own fields under
/// `envelope`, the message slots under `body`, the cell's configuration under
/// `params`. The helpers below still write the message flat, so this is the one
/// place that knows the wire shape.
fn stdin_doc(flat: &serde_json::Value) -> serde_json::Value {
    let mut envelope = serde_json::Map::new();
    let mut slots = serde_json::Map::new();
    for (k, v) in flat.as_object().expect("a flat message object") {
        if k == "header" {
            envelope.insert(k.clone(), v.clone());
        } else {
            slots.insert(k.clone(), v.clone());
        }
    }
    serde_json::json!({"envelope": envelope, "body": slots, "params": {}})
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

fn run_with_stdin(stdin: &str, probe: &str) -> String {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO({})\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'recall', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "_emitted = _sink.getvalue()\n",
            "{}"
        ),
        serde_json::to_string(&recall_script()).unwrap(),
        serde_json::to_string(stdin).unwrap(),
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

/// Drives one real hop of the cell: `body` goes in as stdin, whatever the cell
/// emitted comes back out parsed.
fn run_hop(body: &serde_json::Value) -> serde_json::Value {
    let raw = run_with_stdin(&stdin_doc(body).to_string(), "sys.stdout.write(_emitted)\n");
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("emitted was not JSON ({e}): {raw}"))
}

/// A realistic contamination: tool preamble first, the actual question last.
const CONTAMINATED: &str = "You are a helpful assistant with access to a long-term memory tool. \
     Call the recall tool whenever the user refers to anything said before. \
     Always cite the candidate ids you relied on and never invent a fact that \
     is not in the bundle. Welchen Lieblingseditor nutzt alex?";
const BARE_QUESTION: &str = "Welchen Lieblingseditor nutzt alex?";

// ---------------------------------------------------------------- the guard

/// The healthy case, and the one the acceptance criterion calls byte-identical:
/// a short clean query is returned unchanged and carries NO verdict at all.
#[test]
fn a_healthy_query_passes_the_guard_untouched_and_without_a_verdict() {
    let probe = r#"
print(sanitize_query("Welchen Lieblingseditor nutzt alex?"))
print(sanitize_query(""))
print(sanitize_query(None))
# exactly at the threshold is still healthy
at_limit = "x" * 200
print(sanitize_query(at_limit)[0] == at_limit, sanitize_query(at_limit)[1])
"#;
    assert_eq!(
        run_probe(probe),
        "('Welchen Lieblingseditor nutzt alex?', None)\n\
         ('', None)\n\
         ('', None)\n\
         True None"
    );
}

/// The measured shape: boilerplate head, real question tail. The guard keeps
/// the LAST sentence ending in `?` and says which step it applied.
#[test]
fn a_contaminated_query_collapses_onto_the_question_at_its_tail() {
    let probe = format!(
        "q = {}\n\
         clean, verdict = sanitize_query(q)\n\
         print(len(q) > 200)\n\
         print(clean)\n\
         print(verdict[\"step\"], verdict[\"to_chars\"], verdict[\"from_chars\"] == len(q))\n",
        serde_json::to_string(CONTAMINATED).unwrap()
    );
    assert_eq!(
        run_probe(&probe),
        format!(
            "True\n{BARE_QUESTION}\nquestion {} True",
            BARE_QUESTION.len()
        )
    );
}

/// No question mark anywhere — the tail sentence is the fallback, not the head.
#[test]
fn a_contaminated_statement_falls_back_to_its_tail_sentence() {
    let probe = r#"
q = ("Systemhinweis: du bist ein Assistent mit Speicherzugriff. "
     "Nutze das recall-Tool bei jedem Rueckbezug auf frueher Gesagtes. "
     "Zitiere immer die Kandidaten-Ids und erfinde niemals einen Fakt. "
     "Alex sucht seinen Lieblingseditor.")
clean, verdict = sanitize_query(q)
print(len(q) > 200)
print(clean)
print(verdict["step"])
"#;
    assert_eq!(
        run_probe(probe),
        "True\nAlex sucht seinen Lieblingseditor.\ntail-sentence"
    );
}

/// A blob without a single sentence end is the unbounded consumer in its purest
/// form. It is cut from the TAIL and the result is bounded — that bound is the
/// whole point of the guard.
#[test]
fn a_blob_without_sentence_ends_is_truncated_from_the_tail() {
    let probe = r#"
q = "junk " * 100 + "lieblingseditor"
clean, verdict = sanitize_query(q)
print(len(q), verdict["step"], len(clean))
# what survives is a SUFFIX of what came in -- the cut is from the tail
print(clean.endswith("lieblingseditor"), q.endswith(clean))
# and the bound holds for every input, however long
print(max(len(sanitize_query("z" * n)[0]) for n in (0, 199, 201, 5000, 100000)))
"#;
    assert_eq!(run_probe(probe), "515 truncate 250\nTrue True\n250");
}

/// Issue #88 point 3: the keyword leg's token cap takes the tokens of the
/// surviving TAIL. The head cap kept exactly the wrong half.
#[test]
fn the_keyword_token_cap_takes_the_tail_not_the_head() {
    let probe = r#"
toks = " ".join("tok%03d" % i for i in range(30))
m = fts_match(toks)
print(m.count(" OR ") + 1)
print(m.split(" OR ")[0], m.split(" OR ")[-1])
# a query below the cap is untouched by this
print(fts_match("welchen Lieblingseditor nutzt alex"))
"#;
    assert_eq!(
        run_probe(probe),
        "24\n\"tok006\"* \"tok029\"*\n\
         \"welchen\"* OR \"Lieblingseditor\"* OR \"nutzt\"* OR \"alex\"*"
    );
}

/// The acceptance criterion of the issue, verbatim: the contaminated query
/// reaches the FTS matcher as the bare question does — tail tokens in, head
/// tokens out.
#[test]
fn the_contaminated_query_matches_exactly_what_the_bare_question_matches() {
    let probe = format!(
        "dirty = fts_match(sanitize_query({})[0])\n\
         bare = fts_match({})\n\
         print(dirty == bare)\n\
         print(bare)\n\
         print(\"assistant\" in dirty.lower(), \"recall\" in dirty.lower())\n",
        serde_json::to_string(CONTAMINATED).unwrap(),
        serde_json::to_string(BARE_QUESTION).unwrap()
    );
    assert_eq!(
        run_probe(&probe),
        "True\n\"Welchen\"* OR \"Lieblingseditor\"* OR \"nutzt\"* OR \"alex\"*\nFalse False"
    );
}

// ------------------------------------------------- the verdict is not silent

/// The guard sits at REQUEST ENTRY, before the leg fan (same place the
/// half-open window is rejected, R5): both keyword legs and the embed hop of
/// the semantic leg see the CLEAN query, and none of them ever sees the head.
#[test]
fn both_tier1_legs_are_fanned_out_on_the_clean_query() {
    // The asking round travels with the request (#244): this is one person
    // asking one agent in one room, and the read path refuses a question
    // without it. Not `["*"]` -- a universal round would let the fan-out
    // assertion below pass against a read path with no gate at all.
    let body = serde_json::json!({
        "header": {"context": {"recall_query": CONTAMINATED, "memory_tier": "1",
                               "audience_now": r#"["member:user","agent:assistant"]"#,
                               "channel": "c-q2"}},
        "messages": []
    });
    let out = run_hop(&body);
    let ops = out.as_array().expect("a fan-out");
    let mut seen_match = 0;
    let mut seen_embed = 0;
    for op in ops {
        let text = op["messages"][0]["text"].as_str().unwrap();
        let args: serde_json::Value = serde_json::from_str(text).unwrap();
        if let Some(m) = args["match"].as_str() {
            seen_match += 1;
            assert_eq!(
                m, "\"Welchen\"* OR \"Lieblingseditor\"* OR \"nutzt\"* OR \"alex\"*",
                "the keyword leg runs on the surviving tail"
            );
        }
        if let Some(q) = args["query"]["text"].as_str() {
            seen_embed += 1;
            assert_eq!(q, BARE_QUESTION, "the semantic leg embeds the clean query");
        }
    }
    assert_eq!(seen_match, 2, "episodes + facts keyword leg");
    assert_eq!(seen_embed, 1, "the embed hop");
}

/// A tier-0 bundle carries the query it was built with, so a clamped query
/// without its verdict would be an unexplained difference from what the caller
/// sent. The verdict travels in the JSON and on the hop.
#[test]
fn the_tier0_bundle_reports_the_verdict() {
    let out = run_hop(&tier0_fire(CONTAMINATED));
    let msg = &out.as_array().unwrap()[0];
    let bundle: serde_json::Value =
        serde_json::from_str(msg["system"]["memory"]["bundle"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(bundle["query"], BARE_QUESTION);
    assert_eq!(bundle["query_hygiene"]["step"], "question");
    assert_eq!(bundle["query_hygiene"]["to_chars"], BARE_QUESTION.len());
    assert_eq!(msg["header"]["query_hygiene"], "question");
}

/// …and a healthy query leaves the tier-0 bundle exactly as it was before the
/// guard existed: no field, no header key, no rendered line.
#[test]
fn a_healthy_query_leaves_the_tier0_bundle_byte_identical() {
    let out = run_hop(&tier0_fire(BARE_QUESTION));
    let msg = &out.as_array().unwrap()[0];
    assert_eq!(
        msg["system"]["memory"]["bundle"]["text"].as_str().unwrap(),
        "{\"beliefs\": [], \"episodes\": [], \"foresight\": [], \
         \"query\": \"Welchen Lieblingseditor nutzt alex?\", \
         \"tier\": 0, \"token_estimate\": 0}"
    );
    assert_eq!(
        msg["messages"][0]["text"],
        "MEMORY (tier 0, deterministic bundle)"
    );
    assert_eq!(msg["header"].get("query_hygiene"), None);
}

/// The tier-1 bundle is the one whose CANDIDATES depend on the clamp, so there
/// the verdict is also rendered into the text the model reads — same shape as
/// `window_ignored`.
#[test]
fn the_tier1_bundle_reports_the_verdict_in_json_header_and_text() {
    let out = run_hop(&tier1_emit(CONTAMINATED));
    let msg = &out.as_array().unwrap()[0];
    let bundle: serde_json::Value =
        serde_json::from_str(msg["system"]["memory"]["bundle"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(bundle["query"], BARE_QUESTION);
    assert_eq!(bundle["query_hygiene"]["step"], "question");
    assert_eq!(msg["header"]["query_hygiene"], "question");
    // GH #279 re-point: the verdict still stands in the text the model reads —
    // it is a statement about THIS answer's reliability — but in words rather
    // than as a key name. `question`, `from_chars` and `to_chars` are the
    // caller's half and are asserted above, out of the JSON.
    let text = msg["messages"][0]["text"].as_str().unwrap();
    let sentence = text
        .lines()
        .find(|l| l.contains("shortened"))
        .unwrap_or_else(|| panic!("the model reads the text, not the JSON: {text}"));
    assert!(
        sentence.contains("missing"),
        "and it says what the clamp costs the answer: {sentence}"
    );
    assert!(
        !text.contains("query_hygiene"),
        "a reader who has to decode a key name is a reader who gets it wrong: {text}"
    );
}

/// …and the healthy tier-1 bundle is untouched, down to the rendered lines.
#[test]
fn a_healthy_query_leaves_the_tier1_bundle_byte_identical() {
    let out = run_hop(&tier1_emit(BARE_QUESTION));
    let msg = &out.as_array().unwrap()[0];
    let bundle: serde_json::Value =
        serde_json::from_str(msg["system"]["memory"]["bundle"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(bundle["query"], BARE_QUESTION);
    assert_eq!(bundle.get("query_hygiene"), None);
    assert_eq!(msg["header"].get("query_hygiene"), None);
    // GH #279: the byte-identical rendering of an empty run moved one slot over.
    // It is still exactly this string — the guard's promise was that a healthy
    // query changes nothing, and that promise is kept against the document the
    // string now lives in.
    assert_eq!(
        msg["recall_diagnostic"]["text"],
        "MEMORY (tier 1, 0 candidates, RRF over no leg)"
    );
    // The payload half (#297): a run that found nothing does not hand the model
    // a header over no rows — it says, in one sentence, that this memory holds
    // no answer to the question. Nothing was looked up that could have been
    // floored away here, so the sentence carries no reason clause.
    let rendered = msg["messages"][0]["text"].as_str().unwrap();
    assert_eq!(
        rendered,
        "Nothing in this memory answers this question (as of 2026-08-14)."
    );
    assert_eq!(
        msg["header"]["recall_empty"], "1",
        "and the same verdict travels on the header: {}",
        msg["header"]
    );
    let bundle_answers = &bundle["answers"];
    assert_eq!(
        bundle_answers, "none",
        "…and in the bundle the caller parses: {bundle}"
    );
}

/// The tier-0 `legs` hop: the store's bundle answering all three fixed legs in
/// one reply (GH #295), nothing in any of them.
fn tier0_fire(query: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": {"recall_query": query, "mem_phase": "legs", "recall_id": "r-q2"},
                   "hop": {"operation": "bundle", "rows_affected": 0, "bundle_errors": 0}},
        "messages": [
            {"origin": "tool", "type": "tool_result", "id": "r-leg-episodes", "text": "[]"},
            {"origin": "tool", "type": "tool_result", "id": "r-leg-beliefs", "text": "[]"},
            {"origin": "tool", "type": "tool_result", "id": "r-leg-foresight", "text": "[]"}
        ],
        "results": [
            {"tool_call_id": "r-leg-episodes", "operation": "select", "rows_affected": 0,
             "duration_ms": 0},
            {"tool_call_id": "r-leg-beliefs", "operation": "select", "rows_affected": 0,
             "duration_ms": 0},
            {"tool_call_id": "r-leg-foresight", "operation": "select", "rows_affected": 0,
             "duration_ms": 0}
        ]
    })
}

/// The tier-1 `t1-emit` hop with an empty fan-in — enough to reach the bundle,
/// which is what this pins.
fn tier1_emit(query: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {"context": {"recall_query": query, "memory_tier": "1",
                               "mem_phase": "t1-emit", "recall_id": "r-q2",
                               "recall_as_of": "2026-08-14T00:00:00Z"},
                   "hop": {"operation": "select"}},
        "messages": [{"origin": "tool", "type": "tool_result", "text": "[]"}]
    })
}
