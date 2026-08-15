//! Statement identity W6 -- judged claim aliases (GitHub #13, ruling Q1 stage 2).
//!
//! W2 made the statement `(canonical_subject, canonical_predicate,
//! canonical_claim)` and bootstrapped the third dimension from BYTE identity of
//! the written claim. That was deliberate and it left one half of the yoga
//! specimen open: "yoga twice a week" and "The user practices yoga." are two
//! wordings of one value, and byte identity reads them as two statements.
//!
//! W6 fills the tables W2 declared. The nightly judge is asked, on the axes it
//! is already shown, which of the open statements SAY THE SAME THING; a yes
//! becomes a `set_alias` on the claim dimension and the existing `canonicalize`
//! nachzug does the rest -- two rewordings collapse into one statement and W2's
//! re-assertion arithmetic takes over, so the older wording becomes history
//! instead of a competing answer.
//!
//! The danger is the mirror image of the value: a WRONG alias merges two real
//! values and destroys the difference between them. Three things stand against
//! it and all three are pinned here: a conservative prompt rule (quantities that
//! differ are never the same value), the refusal list that stops a rejected pair
//! from being re-offered every night, and a revert that is one delete plus one
//! re-derive. The fourth is structural and is the reason this package may ship
//! at all: without a judgement NOTHING merges, which is what the deterministic
//! arm of every pin below measures.
//!
//! Everything here runs the REAL `params.script_inline` of `dream-glue`, so no
//! model is called and nothing costs anything.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/dream-glue/config.json";
const STORE_CONFIG: &str = "../../templates/memory-hive/store/config.json";

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

fn config_of(path: &str) -> serde_json::Value {
    let raw = std::fs::read_to_string(path).expect("config");
    serde_json::from_str(&raw).expect("config json")
}

fn glue_script() -> String {
    resolve_vars(
        config_of(GLUE_CONFIG)["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(glue_script())
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
        "dream-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

const RUN: &str = "r1";
const TO: &str = "2026-08-12T03:00:00Z";

fn store_reply(phase: &str, operation: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": phase,
                        "dream_run": RUN, "dream_to": TO},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

fn judgement(text: &str) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "dream", "mem_phase": "canon-judged",
                        "dream_run": RUN, "dream_to": TO},
            "hop": {"finish_reason": "stop"}
        },
        "messages": [{"origin": "assistant", "type": "text", "text": text}]
    })
}

fn args_of(msg: &serde_json::Value) -> serde_json::Value {
    let text = msg["messages"][0]["text"].as_str().expect("op text");
    serde_json::from_str(text).expect("op args")
}

fn ops_named(msgs: &[serde_json::Value], operation: &str) -> Vec<serde_json::Value> {
    msgs.iter()
        .map(args_of)
        .filter(|a| a["operation"] == operation)
        .collect()
}

/// The rewording specimen of the P8a receipt, plus the quantity that is a real
/// change: two of these three statements mean one value, the third does not.
fn yoga_rows() -> serde_json::Value {
    serde_json::json!([
        {"id":"y1","subject":"user","canonical_subject":"user",
         "predicate":"practices","canonical_predicate":"practices",
         "claim":"yoga twice a week","canonical_claim":"yoga twice a week",
         "valid_from":"2026-01-01T00:00:00Z","recorded_at":"2026-01-01T00:00:00Z"},
        {"id":"y2","subject":"user","canonical_subject":"user",
         "predicate":"practices","canonical_predicate":"practices",
         "claim":"The user practices yoga.","canonical_claim":"The user practices yoga.",
         "valid_from":"2026-02-01T00:00:00Z","recorded_at":"2026-02-01T00:00:00Z"},
        {"id":"y3","subject":"user","canonical_subject":"user",
         "predicate":"practices","canonical_predicate":"practices",
         "claim":"yoga three times a week","canonical_claim":"yoga three times a week",
         "valid_from":"2026-05-01T00:00:00Z","recorded_at":"2026-05-01T00:00:00Z"}
    ])
}

// ------------------------------------------------------- the refusals travel

#[test]
fn the_store_declares_the_claim_alias_tables() {
    // Declared since W2, filled by W6 -- the package writes no schema at all.
    let claim = config_of(STORE_CONFIG)["params"]["canonical"]["facts"]
        .as_array()
        .expect("canonical bindings")
        .iter()
        .find(|b| b["source"] == "claim")
        .expect("the claim binding")
        .clone();
    assert_eq!(claim["aliases"], "claim_aliases");
    assert_eq!(
        claim["rejected"], "claim_rejected_pairs",
        "a refusal that is not written down is asked again tomorrow at full price"
    );
}

#[test]
fn the_round_reads_the_refused_claim_pairs_before_it_scans() {
    // The read chain W5 opened, one table longer. The refusals matter more here
    // than the token they save: the axes of the currency question are offered
    // every night whether or not a pair on them was settled, so without this the
    // judge gets an unbounded number of retries at a merge that destroys a value.
    let after_card = emit(store_reply("canon-card", "select", serde_json::json!([])));
    assert_eq!(after_card[0]["header"]["phase"], "canon-refused-fetch");

    let fetch = emit(store_reply(
        "canon-refused-fetch",
        "insert",
        serde_json::json!([]),
    ));
    let args = args_of(&fetch[0]);
    assert_eq!(args["operation"], "select");
    assert_eq!(args["table"], "claim_rejected_pairs");
    assert_eq!(fetch[0]["header"]["phase"], "canon-refused");

    let parked = emit(store_reply(
        "canon-refused",
        "select",
        serde_json::json!([{"left_value": "yoga twice a week",
                            "right_value": "yoga three times a week"}]),
    ));
    let park = args_of(&parked[0]);
    assert_eq!(park["table"], "scratch");
    assert_eq!(park["row"]["kind"], "canon-refused");
    assert_eq!(
        park["row"]["payload"], "[[\"yoga three times a week\", \"yoga twice a week\"]]",
        "the pair is UNORDERED in the store and ordered here, so two runs over one \
         store build one payload"
    );
    assert_eq!(
        parked[0]["header"]["phase"], "canon-scan-fetch",
        "and the fact scan follows the last of the two reads"
    );
}

/// The judge call the round builds from a parked scan, a parked verdict map and
/// a parked refusal list.
fn asked(refused: serde_json::Value) -> serde_json::Value {
    let scan = args_of(&emit(store_reply("canon-scan", "select", yoga_rows()))[0]);
    let msgs = emit(store_reply(
        "canon-ask",
        "select",
        serde_json::json!([
            {"key": RUN, "kind": "canon-scan", "payload": scan["row"]["payload"]},
            {"key": RUN, "kind": "canon-pairs", "payload": "[]"},
            {"key": RUN, "kind": "canon-card", "payload": "{}"},
            {"key": RUN, "kind": "canon-refused", "payload": refused.to_string()}
        ]),
    ));
    assert_eq!(msgs[0]["header"]["route"], "judge", "no call was made");
    serde_json::from_str(
        msgs[0]["messages"][0]["text"]
            .as_str()
            .expect("payload text"),
    )
    .expect("payload json")
}

#[test]
fn a_refusal_travels_with_the_axis_it_was_made_on() {
    let payload = asked(serde_json::json!([
        ["yoga twice a week", "yoga three times a week"],
        ["lives in zone-a", "lives in zone-b"]
    ]));
    assert_eq!(
        payload["axes"][0]["known_different"],
        serde_json::json!([["yoga twice a week", "yoga three times a week"]]),
        "the refusal of THIS axis is shown and the one of another axis is not -- \
         a payload that carried every refusal of the store would grow without \
         bound on exactly the memories that have been running longest: {payload}"
    );
}

#[test]
fn the_prompt_asks_the_rewording_question_and_names_the_trap() {
    let msgs = emit(store_reply(
        "canon-ask",
        "select",
        serde_json::json!([
            {"key": RUN, "kind": "canon-scan",
             "payload": args_of(&emit(store_reply("canon-scan", "select", yoga_rows()))[0])
                        ["row"]["payload"]},
            {"key": RUN, "kind": "canon-pairs", "payload": "[]"},
            {"key": RUN, "kind": "canon-card", "payload": "{}"},
            {"key": RUN, "kind": "canon-refused", "payload": "[]"}
        ]),
    ));
    let text = msgs[0]["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions");
    assert!(
        text.contains(
            r#""same_value":[{"subject":"","predicate":"","canonical":"","alias":"","reason":""}]"#
        ),
        "the answer shape has to carry the section: {text}"
    );
    for anchor in [
        "5. `same_value`",
        "twice a week",
        "three times a week",
        "known_different",
        "\"claim\"",
    ] {
        assert!(
            text.contains(anchor),
            "the rewording paragraph is missing {anchor:?}"
        );
    }
}

// -------------------------------------------------------- the alias is written

fn same_value(items: &str) -> Vec<serde_json::Value> {
    emit(judgement(&format!("{{\"same_value\": [{items}]}}")))
}

#[test]
fn a_rewording_verdict_becomes_one_alias_and_one_re_derive() {
    // The whole mechanism of W6 in one op pair. `set_alias` on the CLAIM
    // dimension plus the `canonicalize` this round already ends with -- no new
    // column, no new table, no second code path. The re-derive that follows is
    // what turns the merged statement into the re-assertion arithmetic of W2.
    let msgs = same_value(
        r#"{"subject": "user", "predicate": "practices",
            "canonical": "yoga twice a week", "alias": "The user practices yoga.",
            "reason": "the second wording says the same thing without the frequency"}"#,
    );
    let aliases = ops_named(&msgs, "set_alias");
    assert_eq!(
        aliases,
        vec![serde_json::json!({
            "operation": "set_alias", "table": "facts", "column": "claim",
            "alias": "The user practices yoga.", "canonical": "yoga twice a week",
            "recorded_at": TO
        })],
        "one alias, on the claim dimension, in the shape the other two dimensions \
         already use: {msgs:?}"
    );
    let redraws = ops_named(&msgs, "canonicalize");
    assert_eq!(
        redraws.len(),
        1,
        "ONE re-derive at the end of the round, over every dimension -- a second \
         one would let a reader see a half-written judgement"
    );
}

#[test]
fn a_refusal_on_the_claim_dimension_is_written_down() {
    // Ruling Q1 stage 2 asks for the P5 pattern on this dimension too: the `no`
    // is a verdict like the `yes`, and one that is not written down is asked
    // again tomorrow -- on the dimension where a wrong `yes` destroys a value.
    let msgs = emit(judgement(
        r#"{"different": [{"dimension": "claim", "left": "yoga twice a week",
                           "right": "yoga three times a week"}]}"#,
    ));
    assert_eq!(
        ops_named(&msgs, "reject_pair"),
        vec![serde_json::json!({
            "operation": "reject_pair", "table": "facts", "column": "claim",
            "left": "yoga twice a week", "right": "yoga three times a week",
            "recorded_at": TO
        })],
        "the refusal list of the claim dimension stayed empty: {msgs:?}"
    );
}

#[test]
fn a_rewording_verdict_that_cannot_be_read_back_is_dropped() {
    // Same discipline as the closures and the cardinality verdicts, and it is
    // sharper here: a claim alias moves the SUPERSESSION unit, so a merge nobody
    // can read back is a change to the answers of this memory that nobody can
    // justify or revert.
    for bad in [
        r#"{"canonical": "yoga twice a week", "alias": "The user practices yoga.", "reason": ""}"#,
        r#"{"canonical": "yoga twice a week", "alias": "", "reason": "same thing"}"#,
        r#"{"canonical": "", "alias": "yoga twice a week", "reason": "same thing"}"#,
        r#"{"canonical": "yoga twice a week", "alias": "yoga twice a week",
            "reason": "a value is itself"}"#,
        r#""yoga twice a week""#,
    ] {
        let msgs = same_value(bad);
        assert!(
            ops_named(&msgs, "set_alias").is_empty(),
            "this rewording verdict should have been dropped: {bad}"
        );
    }
}

#[test]
fn the_rewording_reason_is_folded_into_the_run_receipt() {
    let msgs = same_value(
        r#"{"subject": "user", "predicate": "practices",
            "canonical": "yoga twice a week", "alias": "The user practices yoga.",
            "reason": "one is the other without the frequency"}"#,
    );
    let parked: Vec<serde_json::Value> = msgs
        .iter()
        .map(args_of)
        .filter(|a| a["table"] == "scratch" && a["row"]["kind"] == "canon-claims")
        .collect();
    assert_eq!(parked.len(), 1, "the reason was not parked: {msgs:?}");
    let run = emit(store_reply(
        "apply-run",
        "select",
        serde_json::json!([
            {"key": RUN, "kind": "verdicts", "payload": "{}"},
            {"key": RUN, "kind": "beliefs", "payload": "[]"},
            {"key": RUN, "kind": "canon-claims", "payload": parked[0]["row"]["payload"]}
        ]),
    ));
    let closed = run
        .iter()
        .map(args_of)
        .find(|a| a["table"] == "consolidation_log")
        .expect("the run has to close");
    let verdicts: serde_json::Value =
        serde_json::from_str(closed["set"]["verdicts"].as_str().expect("verdicts")).expect("json");
    assert_eq!(
        verdicts["same_value"][0]["alias"], "The user practices yoga.",
        "a merge nobody can read back cannot be reverted either: {verdicts}"
    );
}

// ------------------------------------------------ what the alias then DOES

/// Run a probe with the script's own globals in scope.
fn probe(body: &str) -> String {
    let src = format!(
        concat!(
            "import sys, io\n",
            "_script = {}\n",
            "sys.stdin = io.StringIO('{{\"envelope\": {{}}, \"body\": {{}}, \"params\": {{}}}}')\n",
            "_sink, _real = io.StringIO(), sys.stdout\n",
            "sys.stdout = _sink\n",
            "try:\n",
            "    exec(compile(_script, 'dream-glue', 'exec'), globals())\n",
            "except SystemExit:\n",
            "    pass\n",
            "finally:\n",
            "    sys.stdout = _real\n",
            "{}"
        ),
        serde_json::to_string(&glue_script()).unwrap(),
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

/// The three yoga rows as the chain sees them, with the canonical claim of the
/// rewording as a parameter: the WRITTEN claim never changes, only the identity
/// the store derived for it.
fn yoga_chain(canonical_of_rewording: &str) -> String {
    format!(
        r#"
rows = [
 {{"id":"y1","subject":"user","canonical_subject":"user","predicate":"practices",
  "canonical_predicate":"practices","claim":"yoga twice a week",
  "canonical_claim":"yoga twice a week","valid_from":"2026-01-01T00:00:00Z",
  "recorded_at":"2026-01-01T00:00:00Z"}},
 {{"id":"y2","subject":"user","canonical_subject":"user","predicate":"practices",
  "canonical_predicate":"practices","claim":"The user practices yoga.",
  "canonical_claim":"{canonical_of_rewording}","valid_from":"2026-02-01T00:00:00Z",
  "recorded_at":"2026-02-01T00:00:00Z"}},
 {{"id":"y3","subject":"user","canonical_subject":"user","predicate":"practices",
  "canonical_predicate":"practices","claim":"yoga three times a week",
  "canonical_claim":"yoga three times a week","valid_from":"2026-05-01T00:00:00Z",
  "recorded_at":"2026-05-01T00:00:00Z"}}
]
print(derive_supersessions(rows))
"#
    )
}

#[test]
fn without_a_judgement_nothing_merges_and_nothing_closes() {
    // The deterministic arm, and it is the reason a judged merge may ship at all:
    // byte identity of the claim is the day-one canonical value (W2), so three
    // wordings are three statements and none of them ends another. Whatever the
    // judge gets wrong, THIS is what the store does without it.
    assert_eq!(
        probe(&yoga_chain("The user practices yoga.")),
        "[('y1', None, None), ('y2', None, None), ('y3', None, None)]",
        "the deterministic path merged two wordings by itself"
    );
}

#[test]
fn a_merged_wording_becomes_history_of_the_same_statement() {
    // What the alias buys, one step after `canonicalize` pulled the column: the
    // rewording and the original are ONE statement now, so W2's re-assertion
    // arithmetic closes the older assertion with the newer one -- and the
    // quantity that really is a different value stays open next to it.
    assert_eq!(
        probe(&yoga_chain("yoga twice a week")),
        "[('y1', '2026-02-01T00:00:00Z', 'y2'), ('y2', None, None), ('y3', None, None)]",
        "the merged wording did not become an assertion of the same statement"
    );
}

#[test]
fn the_revert_is_the_alias_row_and_the_next_re_derive() {
    // P2's revert, unchanged one dimension over: delete the alias row, let
    // `canonicalize` fall the column back onto the written claim, and the next
    // re-derive emits (None, None) for the pair -- which the caller writes,
    // because it writes every difference. Structurally identical to the two arms
    // above, and that is the point: the merge is data, not a rewritten row.
    let merged = probe(&yoga_chain("yoga twice a week"));
    let reverted = probe(&yoga_chain("The user practices yoga."));
    assert_ne!(merged, reverted);
    assert!(
        reverted.contains("('y1', None, None)"),
        "after the revert the closure has to be withdrawn to NULL: {reverted}"
    );
}

#[test]
fn the_keyword_index_keeps_reading_the_written_claim() {
    // W2 flank 3, held: the FTS declaration indexes `claim`, never
    // `canonical_claim`. The claim is the text a question searches, and an index
    // over the alias-resolved twin would lose the original wording the moment a
    // merge lands -- exactly when the store gained knowledge. The recall proof on
    // a real colony is scenario C17.
    let fts = config_of(STORE_CONFIG)["params"]["fts"]["facts"]
        .as_array()
        .expect("fts declaration")
        .clone();
    assert!(
        fts.iter().any(|c| c == "claim"),
        "the keyword leg lost the written claim: {fts:?}"
    );
    assert!(
        !fts.iter().any(|c| c == "canonical_claim"),
        "the keyword leg was moved onto the merged identity, which costs recall on \
         the original wording: {fts:?}"
    );
}

#[test]
fn an_axis_nobody_refused_anything_on_looks_exactly_as_it_did() {
    // Additive by construction, the property every key of this payload has: a
    // night on a store without refusals builds byte for byte what W5 built.
    let payload = asked(serde_json::json!([]));
    assert!(
        payload["axes"][0].get("known_different").is_none(),
        "an empty refusal list is no key at all: {}",
        payload["axes"][0]
    );
}
