//! GH #279 — the bundle stops describing itself as a search result.
//!
//! Measured on a live agent: the same store, the same fact, the same channel,
//! and only the framing changed. Handed a block that opened with
//! `MEMORY (tier 1, 18 candidates, RRF over keyword,semantic,temporal)` and
//! tagged every row with the legs that found it, the agent answered that the
//! value was "not reliably known". It was not ignoring the bundle — it read it,
//! attributed the row correctly, and DISCOUNTED it, because `candidates`,
//! `RRF`, ranks, scores and leg tags are the vocabulary of a search result, and
//! a search result is a set of maybes.
//!
//! So the readable half stops speaking that language. It opens by saying what
//! the document IS and as of when, and the two things that genuinely qualify
//! the ANSWER — the question was shortened, the list was cut short — are stated
//! as sentences a reader can act on rather than as machine bookkeeping.
//!
//! Nothing is lost: `recall_diagnostic["text"]` carries the old rendering byte
//! for byte, which is the debuggable form #279 explicitly wants to keep. The
//! two documents have two readers and they must not be the same string.

use std::io::Write;
use std::process::{Command, Stdio};

const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";

/// The four legs, by name — the words that must not reach the reader.
const LEGS: [&str; 4] = ["keyword", "semantic", "graph", "temporal"];

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

/// A fact row as the hydration select returns it.
fn fact(id: &str, subject: &str, claim: &str, valid_from: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "session_id": "s-1", "subject": subject,
                       "predicate": "prefers", "claim": claim,
                       "canonical_subject": subject, "canonical_predicate": "prefers",
                       "canonical_claim": claim, "valid_from": valid_from,
                       "valid_until": serde_json::Value::Null,
                       "recorded_at": valid_from,
                       "expired_at": serde_json::Value::Null,
                       "superseded_by": serde_json::Value::Null,
                       "episode_id": format!("ep-of-{id}"),
                       "fact_kind": "state", "confidence": 90})
}

fn episode(id: &str, content: &str, happened_at: &str) -> serde_json::Value {
    serde_json::json!({"id": id, "session_id": "s-1", "sender": "user",
                       "content": content, "happened_at": happened_at,
                       "recorded_at": happened_at})
}

/// The fused scratch payload: two legs present, nothing capped. The empty
/// `leg_capped` is deliberate — `complete_reason` NAMES the leg a cap ended
/// (#280), the one sentence in which a leg name is written FOR the reader, and
/// the assertions below are about everything else.
fn fused() -> serde_json::Value {
    serde_json::json!({
        "candidates": [
            {"kind": "fact", "id": "f-1", "score": 0.09, "legs": ["keyword"]},
            {"kind": "episode", "id": "e-1", "score": 0.075,
             "legs": ["keyword", "semantic"]},
            {"kind": "fact", "id": "f-2", "score": 0.06, "legs": ["semantic"]}
        ],
        "legs_present": ["keyword", "semantic"],
        "leg_sizes": {"keyword": 2, "semantic": 2, "graph": 0, "temporal": 0},
        "leg_capped": {},
        "semantic_degraded": false
    })
}

fn one_episode() -> serde_json::Value {
    serde_json::json!([episode(
        "e-1",
        "we talked about editors",
        "2026-01-02T09:00:00.000000Z"
    )])
}

fn two_facts() -> serde_json::Value {
    serde_json::json!([
        fact(
            "f-1",
            "person:example",
            "helix",
            "2026-01-05T09:00:00.000000Z"
        ),
        fact(
            "f-2",
            "person:other",
            "kakoune",
            "2026-01-06T09:00:00.000000Z"
        )
    ])
}

/// The `t1-emit` document of a tier-1 request.
fn doc_of(query: &str, fused: serde_json::Value) -> serde_json::Value {
    let rows = serde_json::json!([
        {"request_id": "r1", "leg": "fused", "payload": fused.to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-ep", "payload": one_episode().to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-fact", "payload": two_facts().to_string(), "fired": 1},
        {"request_id": "r1", "leg": "hyd-axis", "payload": "[]", "fired": 1}
    ]);
    serde_json::json!({
        "header": {
            "context": {"mem_phase": "t1-emit", "recall_id": "r1", "memory_tier": "1",
                        "recall_query": query,
                        "recall_as_of": "2026-08-12T00:00:00Z",
                        "recall_window_from": "", "recall_window_to": ""},
            "hop": {"operation": "select"}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                      "text": rows.to_string()}]
    })
}

fn emit_doc() -> serde_json::Value {
    doc_of("what do I prefer?", fused())
}

fn bundle_message(msgs: &[serde_json::Value]) -> serde_json::Value {
    msgs.iter()
        .find(|m| m["header"]["route"] == "bundle")
        .expect("a tier-1 request emits a bundle message")
        .clone()
}

/// The text a MODEL reads — the rendered turn, not the JSON beside it.
fn rendered(msg: &serde_json::Value) -> String {
    msg["messages"][0]["text"]
        .as_str()
        .expect("rendered turn")
        .to_string()
}

/// The bundle JSON the same message carries.
fn bundle(msg: &serde_json::Value) -> serde_json::Value {
    serde_json::from_str(
        msg["system"]["memory"]["bundle"]["text"]
            .as_str()
            .expect("bundle text"),
    )
    .expect("bundle json")
}

#[test]
fn the_payload_header_asserts_instead_of_counting() {
    let msg = bundle_message(&emit(emit_doc()));
    let text = rendered(&msg);
    let head = text.lines().next().expect("a first line");

    // The vocabulary of a search result, in the one line that frames everything
    // under it. A reader who is told they are looking at 3 of N ranked
    // candidates reads every row below as a maybe.
    for word in ["candidates", "RRF", "rank", "score"] {
        assert!(
            !head.contains(word),
            "the opening line still speaks like a search result ({word}): {head}"
        );
    }
    // And the positive half: it says WHAT the document is and AS OF WHEN, which
    // is the one piece of framing a reader can actually use.
    assert_eq!(
        head, "WHAT THIS MEMORY HOLDS (as of 2026-08-12)",
        "the opening line names the document and its as-of day: {text}"
    );

    // #281 pinned the leg names out of the ROWS; the header was where they
    // lived last. After this package the readable half carries none anywhere.
    for leg in LEGS {
        assert!(
            !text.contains(leg),
            "which leg found a candidate is retrieval bookkeeping — {leg} is in \
             the text the model reads:\n{text}"
        );
    }

    // The rows themselves are still there — otherwise the assertions above pass
    // on an empty document and prove nothing.
    assert!(
        text.contains("helix") && text.contains("we talked about editors"),
        "the answer is still in the text: {text}"
    );
}

#[test]
fn the_diagnostic_form_keeps_every_word_the_payload_gave_up() {
    // The bookkeeping is what makes a bundle debuggable, and #279 says so
    // explicitly: the two documents have two readers, not that one of them goes
    // away. This is the OLD rendering, byte for byte, one slot over.
    let msg = bundle_message(&emit(emit_doc()));
    let diag = msg["recall_diagnostic"]["text"]
        .as_str()
        .expect("the diagnostic carries the old rendering")
        .to_string();

    assert_eq!(
        diag.lines().next().expect("a first line"),
        "MEMORY (tier 1, 3 candidates, RRF over keyword,semantic)",
        "the header the payload gave up, unchanged: {diag}"
    );
    assert_eq!(
        diag.lines().skip(1).collect::<Vec<_>>(),
        vec![
            "- [fact keyword] person:example prefers: helix",
            "- [episode keyword/semantic] we talked about editors",
            "- [fact semantic] person:other prefers: kakoune",
        ],
        "one line per candidate, in the flat ranked form, with the legs that \
         nominated it: {diag}"
    );
}

/// A pasted blob that ends in a sentence, followed by the actual question —
/// the shape the guard was built for (#88). Longer than
/// `MEMORY_QUERY_SAFE_CHARS` (200), so `sanitize_query` fires.
const CONTAMINATED: &str = "Here is the log output I copied out of the terminal \
    before I forgot what I was doing with it, and it goes on for a while because \
    that is what a paste from a terminal does when nobody trims it first. \
    What do I prefer?";

#[test]
fn query_hygiene_is_stated_in_the_payload_without_retrieval_words() {
    // The hygiene verdict is the one piece of bookkeeping that IS payload: it is
    // a statement about THIS answer's reliability — the question the answer was
    // looked up with is not the question that was asked — and not a statement
    // about the machinery. So it stays in the readable half, in words.
    assert!(
        CONTAMINATED.len() > 200,
        "the fixture has to trip the guard: {} chars",
        CONTAMINATED.len()
    );
    let msg = bundle_message(&emit(doc_of(CONTAMINATED, fused())));
    let text = rendered(&msg);

    let sentence = text
        .lines()
        .find(|l| l.contains("shortened"))
        .unwrap_or_else(|| panic!("nothing says the question was shortened:\n{text}"));
    assert!(
        sentence.contains("missing"),
        "and nothing says what that costs the answer: {sentence}"
    );
    // …in words. `query_hygiene` is a key name and `legs` is machinery; a reader
    // who has to decode the sentence is a reader who gets it wrong.
    for word in ["query_hygiene", "legs", "RRF"] {
        assert!(
            !text.contains(word),
            "the sentence still speaks machine ({word}):\n{text}"
        );
    }

    // The caller's half is UNCHANGED in shape — this package moves no field.
    let b = bundle(&msg);
    assert_eq!(b["query_hygiene"]["step"], "question", "bundle: {b}");
    assert_eq!(
        b["query_hygiene"]["from_chars"],
        CONTAMINATED.len(),
        "bundle: {b}"
    );
    assert_eq!(
        b["query_hygiene"]["to_chars"],
        "What do I prefer?".len(),
        "bundle: {b}"
    );
    assert_eq!(b["query"], "What do I prefer?", "bundle: {b}");
}

#[test]
fn a_capped_result_set_says_so_in_the_payload_text() {
    // #280 gave the caller `complete` and `complete_reason`. A model does not
    // read the JSON beside the text, so a list that was cut short reads as the
    // whole answer — which is the wrong answer to every question that counts.
    let mut capped = fused();
    capped["leg_capped"] = serde_json::json!({"semantic": 20});
    let msg = bundle_message(&emit(doc_of("what do I prefer?", capped)));
    let text = rendered(&msg);
    let b = bundle(&msg);

    assert_eq!(b["complete"], false, "bundle: {b}");
    let reason = b["complete_reason"].as_str().expect("a reason");
    assert!(
        reason.contains("capped legs (possibly more exist): semantic at 20 rows"),
        "the cap the fixture set: {reason}"
    );

    let sentence = text
        .lines()
        .find(|l| l.contains(reason))
        .unwrap_or_else(|| panic!("the cut is nowhere in the text:\n{text}"));
    assert!(
        sentence.starts_with("Not everything that matches is here"),
        "and it is stated as something a reader can act on: {sentence}"
    );
    // The sentence stands ABOVE the rows, where a reader meets it before
    // reading anything it qualifies.
    let rows = text
        .lines()
        .position(|l| l.starts_with("FACTS ("))
        .expect("a facts section");
    let cut = text
        .lines()
        .position(|l| l.contains(reason))
        .expect("the sentence");
    assert!(
        cut < rows,
        "the caveat comes before what it qualifies:\n{text}"
    );
}
