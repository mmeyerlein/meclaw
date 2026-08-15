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
//! The fix is both halves of the issue's direction, and neither of them destroys
//! the plan/fact distinction -- it MOVES:
//!
//! 1. the intention lives on the STATEMENT, in `fact_kind: foresight`, which is
//!    the column the foresight leg has always read. The predicate is free to
//!    name the matter, so a plan and the fact it is about share an axis. Every
//!    reader that could tell them apart while the predicate said `plans_to_*`
//!    still can: the bundle renders `(planned)`, the nightly currency question
//!    is shown `"intent": "planned"` on the statement.
//! 2. the extractor has to SEE the sibling while it decides. The replacement
//!    window used to be the eight most recently touched axes; recency is a good
//!    guess about which axes matter and a bad one about which axis a given turn
//!    is ABOUT. The page now fetches a POOL and the prompt phase -- the only
//!    phase that holds the turns -- picks the window out of it by subject matter.
//!
//! Everything here runs the REAL `params.script_inline` of `extract-glue`,
//! `recall` and `dream-glue` against injected store replies, so no model is
//! called and nothing costs anything. Whether an extractor USES the discipline
//! is a model property and belongs to the end-of-wave measurement; the free
//! scenario C22 proves the chain end to end on a real colony.

use std::io::Write;
use std::process::{Command, Stdio};

const EXTRACT_CONFIG: &str = "../../templates/memory-hive/extract-glue/config.json";
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
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(&meclaw_testing::code_stdin_bytes(&doc))
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "script exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("message array")
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

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

// ============================================================ extract-glue

const BATCH: &str = "b1";
/// `MEMORY_EXTRACT_WINDOW_AXES` of the template: what the prompt SHOWS.
const WINDOW_MAX_AXES: usize = 8;

fn emit_x(doc: serde_json::Value) -> Vec<serde_json::Value> {
    run(&script_of(EXTRACT_CONFIG), doc)
}

fn x_reply(phase: &str, operation: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "extract", "mem_phase": phase, "batch_id": BATCH},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

/// One open fact row in the shape the window page projects.
fn fact_row(id: &str, predicate: &str, claim: &str, from: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "subject": "user", "canonical_subject": "user",
        "predicate": predicate, "canonical_predicate": predicate,
        "claim": claim, "canonical_claim": claim,
        "valid_from": from, "recorded_at": from
    })
}

/// One pool entry in the shape `open_statements` parks it.
fn pool_axis(predicate: &str, id: &str, claim: &str, since: &str) -> serde_json::Value {
    serde_json::json!({
        "subject": "user", "predicate": predicate,
        "statements": [{"id": id, "claim": claim, "since": since}]
    })
}

/// The 5K corpus as the page fetches it: nine axes the conversation touched
/// recently, and the personal best -- older than every one of them, and the only
/// one the turns are about. Recency alone cuts it off the end of the window.
fn crowded_pool() -> Vec<serde_json::Value> {
    let mut pool: Vec<serde_json::Value> = (0..9)
        .rev()
        .map(|i| {
            pool_axis(
                &format!("axis_{i}"),
                &format!("a{i}"),
                &format!("filler value {i}"),
                &format!("2026-06-{:02}T00:00:00Z", i + 1),
            )
        })
        .collect();
    pool.push(pool_axis(
        "personal_record",
        "f1",
        "personal best time of 27:12 in the charity 5K run",
        "2026-03-01T00:00:00Z",
    ));
    pool
}

/// The two turns of the 5K case, the second one in a later session.
fn running_turns() -> serde_json::Value {
    serde_json::json!([
        {"episode_id": "e1", "sender": "user",
         "content": "I ran the charity 5K and set a personal best of 27:12."},
        {"episode_id": "e2", "sender": "user",
         "content": "Next spring I want to beat that personal best and run 25:50."}
    ])
}

/// Everything the prompt phase emits for a batch that meets `pool` and `items`.
fn prompt_round(pool: &[serde_json::Value], items: serde_json::Value) -> Vec<serde_json::Value> {
    let rows = serde_json::json!([
        {"key": BATCH, "kind": "batch", "payload": items.to_string()},
        {"key": BATCH, "kind": "vocab", "payload": "{\"user\": [\"personal_record\"]}"},
        {"key": BATCH, "kind": "window-pool",
         "payload": serde_json::Value::from(pool.to_vec()).to_string()}
    ]);
    emit_x(x_reply("prompt", "select", rows))
}

/// The predicates of the window the prompt phase PARKED -- the set guard rail 3
/// checks a `replaces` against.
fn shown_axes(msgs: &[serde_json::Value]) -> Vec<String> {
    let parked = msgs
        .iter()
        .map(args_of)
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == "window")
        .expect("the shown window is parked");
    let window: serde_json::Value =
        serde_json::from_str(parked["row"]["payload"].as_str().expect("payload")).expect("window");
    window
        .as_array()
        .expect("axes")
        .iter()
        .map(|a| a["predicate"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn instructions(msgs: &[serde_json::Value]) -> String {
    msgs.iter()
        .find(|m| m["header"]["route"] == "extract")
        .expect("the extractor call")["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions text")
        .to_string()
}

#[test]
fn the_axes_a_turn_is_about_are_the_ones_the_extractor_is_shown() {
    // The defect itself, in the shape the store had it. The sibling axis exists,
    // it is open, and the page fetched it -- it just sits at the far end of the
    // recency order, because nine other axes were touched more recently. That is
    // the whole of "the currency question can never see them together": the
    // extractor was never shown the value it was about to update.
    let pool = crowded_pool();
    assert_eq!(
        pool.len(),
        10,
        "the fixture has to be bigger than the offered window or it proves nothing"
    );
    let index = pool
        .iter()
        .position(|a| a["predicate"] == "personal_record")
        .expect("the sibling axis is in the pool");
    assert!(
        index >= WINDOW_MAX_AXES,
        "recency alone must NOT reach this axis, else the pin is tautological"
    );

    let msgs = prompt_round(&pool, running_turns());
    let shown = shown_axes(&msgs);
    assert_eq!(
        shown.len(),
        WINDOW_MAX_AXES,
        "the shown window is still capped: {shown:?}"
    );
    assert_eq!(
        shown[0], "personal_record",
        "the axis the turns are ABOUT is shown FIRST, not cut off the end: {shown:?}"
    );
    let text = instructions(&msgs);
    assert!(
        text.contains("personal best time of 27:12 in the charity 5K run"),
        "the value the turn updates has to be IN the prompt -- an extractor that \
         cannot see it mints a sibling axis for it:\n{text}"
    );
}

#[test]
fn an_axis_no_turn_mentions_keeps_the_place_recency_gave_it() {
    // The invariance half: the second sort is stable, so a batch whose turns
    // match no stored value is shown byte for byte the window it was shown
    // before this package -- the recency prefix of the pool, in order.
    let pool = crowded_pool();
    let msgs = prompt_round(
        &pool,
        serde_json::json!([{"episode_id": "e9", "sender": "user",
                            "content": "We had pasta for dinner tonight."}]),
    );
    let shown = shown_axes(&msgs);
    let recency: Vec<String> = pool[..WINDOW_MAX_AXES]
        .iter()
        .map(|a| a["predicate"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        shown, recency,
        "nothing matched, so nothing may move: the window is the recency prefix"
    );
}

#[test]
fn the_page_fetches_a_pool_and_the_prompt_shows_a_window_out_of_it() {
    // The split this package introduces, read off the two phases. The PAGE parks
    // `window-pool` (recency, twice the offered budget) and never asks which
    // axes matter; the PROMPT parks `window` (subject matter, the offered
    // budget) because it is the first phase that holds the turns at all.
    let rows: Vec<serde_json::Value> = (0..12)
        .map(|i| {
            fact_row(
                &format!("p{i}"),
                &format!("axis_{i:02}"),
                &format!("value {i}"),
                &format!("2026-0{}-01T00:00:00Z", i % 9 + 1),
            )
        })
        .collect();
    let page = emit_x(x_reply(
        "window-page",
        "select",
        serde_json::Value::Array(rows),
    ));
    let kinds: Vec<&str> = page
        .iter()
        .map(args_of)
        .filter(|a| a["table"] == "scratch")
        .map(|a| a["row"]["kind"].as_str().unwrap_or_default().to_string())
        .map(|s| Box::leak(s.into_boxed_str()) as &str)
        .collect();
    assert_eq!(
        kinds,
        vec!["window-pool"],
        "the page parks the pool and nothing else"
    );
    let pool: serde_json::Value = serde_json::from_str(
        page.iter()
            .map(args_of)
            .find(|a| a["table"] == "scratch")
            .expect("the pool")["row"]["payload"]
            .as_str()
            .expect("payload"),
    )
    .expect("pool");
    let pool = pool.as_array().expect("axes").clone();
    assert_eq!(
        pool.len(),
        12,
        "twelve axes fit under the pool cap, so the page keeps all of them"
    );

    let msgs = prompt_round(&pool, running_turns());
    assert_eq!(
        shown_axes(&msgs).len(),
        WINDOW_MAX_AXES,
        "and the prompt spends exactly the offered budget on them"
    );
}

#[test]
fn what_may_be_closed_is_what_was_shown_and_never_what_was_only_fetched() {
    // Guard rail 3 (ruling Q2 rail 3) under the new split, and the reason the
    // SHOWN window is the row that gets parked: the apply phase checks every
    // `replaces` against that row, so a selection that widened it would let the
    // extractor close a statement it was never shown.
    let pool = crowded_pool();
    let msgs = prompt_round(&pool, running_turns());
    let shown = shown_axes(&msgs);
    let text = instructions(&msgs);
    for axis in pool.iter() {
        let predicate = axis["predicate"].as_str().expect("predicate");
        let id = axis["statements"][0]["id"].as_str().expect("id");
        let rendered = text.contains(&format!("id {id} | user / {predicate}"));
        assert_eq!(
            rendered,
            shown.iter().any(|p| p == predicate),
            "the parked window and the rendered block disagree about {predicate}"
        );
    }
    assert!(
        shown.len() < pool.len(),
        "the fixture must drop at least one fetched axis or this proves nothing"
    );
}

#[test]
fn the_prompt_says_that_a_speech_act_is_not_an_axis() {
    // The other half of the fix, and the deterministic half of it: the rule and
    // its examples are in the block the extractor reads. What a model does with
    // them is measured at the end of the wave; that the discipline is STATED is
    // measured here.
    let text = instructions(&prompt_round(&crowded_pool(), running_turns()));
    assert!(
        text.contains("A PREDICATE NAMES THE SUBJECT MATTER, NEVER THE SPEECH ACT"),
        "the rule is missing:\n{text}"
    );
    for named in ["plans_to_beat", "wants_to_move_to", "hopes_to_visit"] {
        assert!(
            text.contains(named),
            "the rule needs the NAMED shape it refuses, the way every other rule \
             of this prompt carries one -- missing {named}:\n{text}"
        );
    }
    assert!(
        text.contains("`personal_record`, claim") || text.contains("\"personal_record\", claim"),
        "and the subject-matter form to use instead:\n{text}"
    );
    assert!(
        text.contains("`fact_kind: foresight`") || text.contains("`fact_kind` to `foresight`"),
        "the intention has to be told where it DOES belong, or the rule reads as \
         'drop the intention':\n{text}"
    );
}

#[test]
fn an_intention_about_a_shown_statement_is_told_where_it_belongs() {
    // The window block is where the extractor is looking at a concrete value, so
    // it is where the rule has to be repeated in operational form: same subject,
    // same predicate, `foresight`, and NO `replaces` -- an intention does not end
    // the fact it is about.
    let text = instructions(&prompt_round(&crowded_pool(), running_turns()));
    let block = text
        .split("Statements this memory holds OPEN right now")
        .nth(1)
        .expect("the replacement block");
    assert!(
        block.contains("PLANS, WANTS or HOPES"),
        "the replacement block never mentions an intention:\n{block}"
    );
    assert!(
        block.contains("Leave `replaces` empty"),
        "an intention that closed the fact it is about would be the same defect \
         with the sign flipped:\n{block}"
    );
}

#[test]
fn the_naming_rule_does_not_depend_on_a_window() {
    // The rule is about the PREDICATE, not about a replacement, so it stands on
    // a batch that meets no open statement at all -- the inline ingress, and the
    // very first batch of a fresh memory, which is exactly where a private axis
    // gets minted and never noticed.
    let msgs = prompt_round(&[], running_turns());
    assert_eq!(msgs.len(), 1, "no window, no park, one call");
    let text = instructions(&msgs);
    assert!(text.contains("A PREDICATE NAMES THE SUBJECT MATTER, NEVER THE SPEECH ACT"));
    assert!(
        !text.contains("`replaces`"),
        "a prompt without a window still invites no replacement:\n{text}"
    );
}

#[test]
fn the_matter_of_a_text_is_its_content_words() {
    // The selection has to be reproducible by hand, so its one non-obvious
    // function is pinned directly: lower case, split on everything that is not a
    // letter or a digit, drop what is shorter than four characters, drop the
    // stop words. Short tokens are dropped rather than stopped because a
    // three-letter word that survives a stop list ("5k", "run") matches half a
    // memory and ranks nothing.
    let out = probe(
        EXTRACT_CONFIG,
        "extract-glue",
        "print('|'.join(sorted(matter_tokens(\
         'Next spring I want to beat that personal best and run 25:50!'))))\n\
         print('|'.join(sorted(matter_tokens('Meine Lieblingsfarbe ist Blau'))))\n\
         print('|'.join(sorted(matter_tokens(None))))\n",
    );
    let mut lines = out.lines();
    assert_eq!(
        lines.next().unwrap_or_default(),
        "beat|best|personal|spring",
        "`want`, `that`, `next` and `and` are stop words, `run`, `to`, `I` and \
         `25`/`50` are too short"
    );
    assert_eq!(
        lines.next().unwrap_or_default(),
        "blau|lieblingsfarbe",
        "the claim stays in the language of the turn, so the matter has to work \
         in it too"
    );
    assert_eq!(
        lines.next().unwrap_or_default(),
        "",
        "no text, no matter -- never an exception"
    );
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
    let rendered = msgs[0]["messages"][0]["text"].as_str().expect("rendered");
    assert!(
        rendered.contains("personal best time of 25:50 (planned)"),
        "the model reads THIS text, so the marker has to stand in it:\n{rendered}"
    );
    assert!(
        !rendered.contains("charity 5K run (planned)"),
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
