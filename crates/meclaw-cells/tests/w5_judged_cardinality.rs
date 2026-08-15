//! Statement identity W5 -- cardinality becomes a JUDGED property of the
//! canonical predicate (GitHub #13, ruling Q3 option C).
//!
//! Until W5 an axis enumerated for exactly two reasons: it stood on the curated
//! seed list, or two of its facts started at one instant in two different
//! sessions (the learned rule, guarded since W1). Ruling Q3 puts a third source
//! between them and fixes the precedence for good:
//!
//! ```text
//! seed (authority) > judged (persisted, attributed) > learned-with-session-guard
//! ```
//!
//! The judged half is a small additive table -- `canonical_predicate`, `verdict`,
//! `source`, `decided_at` -- filled by the nightly round, and `source` is the
//! column that makes the ruling's own sentence true: "why does this axis
//! enumerate" must be answerable from the data.
//!
//! Since W2 the answer is a PRESENTATION tie-breaker and nothing else: a wrong
//! verdict can fail to mark an outdated value, it can no longer end a true one.
//! That is why this package may ship on a judgement at all.
//!
//! Everything here runs the REAL `params.script_inline` of the two `code` cells
//! against injected store replies and injected judgements, so no model is called
//! and nothing costs anything.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/dream-glue/config.json";
const RECALL_CONFIG: &str = "../../templates/memory-hive/recall/config.json";
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

fn script_of(path: &str) -> String {
    resolve_vars(
        config_of(path)["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

fn glue_script() -> String {
    script_of(GLUE_CONFIG)
}

fn recall_script() -> String {
    script_of(RECALL_CONFIG)
}

/// Run a script with a real stdin document and return the emitted messages.
fn emit_of(script: &str, doc: serde_json::Value) -> Vec<serde_json::Value> {
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
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "output is not a message array ({e}): {}",
            String::from_utf8_lossy(&out.stdout)
        )
    })
}

fn emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    emit_of(&glue_script(), doc)
}

const RUN: &str = "r1";
const TO: &str = "2026-08-12T03:00:00Z";

/// One store reply as the edge delivers it.
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

/// The judge's answer as the return edge delivers it.
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

/// Every op a judgement produced against one table, in order.
fn ops_on(msgs: &[serde_json::Value], table: &str) -> Vec<serde_json::Value> {
    msgs.iter()
        .map(args_of)
        .filter(|a| a["table"] == table)
        .collect()
}

// ---------------------------------------------------------------- the table

#[test]
fn the_store_declares_the_judged_cardinality_table() {
    // Ruling Q3 in its literal shape: predicate, verdict, source, decided_at.
    // `source` is the mandatory one -- a verdict nobody can attribute cannot be
    // reverted, and "why does this axis enumerate" would be unanswerable.
    let store = config_of(STORE_CONFIG);
    let table = &store["params"]["schema"]["predicate_cardinality"];
    assert!(
        table.is_object(),
        "the store template does not declare predicate_cardinality: {}",
        store["params"]["schema"]
    );
    for column in ["canonical_predicate", "verdict", "source", "decided_at"] {
        assert_eq!(
            table[column], "text",
            "the cardinality table is missing {column}: {table}"
        );
    }
}

// -------------------------------------------------- the round reads it first

#[test]
fn the_round_reads_the_judged_verdicts_before_it_scans() {
    // The night has to know what it already decided, for one reason that is not
    // about tokens: the budget is small and the busiest predicates are always the
    // same ones. A round that re-asks what it settled last night never reaches
    // the tail of the vocabulary at all.
    let msgs = emit(store_reply("apply", "insert", serde_json::json!([])));
    let args = args_of(&msgs[0]);
    assert_eq!(args["operation"], "select");
    assert_eq!(
        args["table"], "predicate_cardinality",
        "the canonicalisation round starts at the judged verdicts: {args}"
    );
    assert_eq!(
        msgs[0]["header"]["phase"], "canon-card",
        "the reply has to come back on a phase of its own"
    );
}

#[test]
fn the_judged_verdicts_are_parked_and_the_scan_follows() {
    // The store answers one result set per message, so the verdicts meet the
    // scan the way everything meets in this lane: parked under the run key,
    // read back at `canon-ask` with the rest.
    let msgs = emit(store_reply(
        "canon-card",
        "select",
        serde_json::json!([{"canonical_predicate": "collects", "verdict": "multi",
                            "source": "judge:r0", "decided_at": "2026-08-11T03:00:00Z"}]),
    ));
    let args = args_of(&msgs[0]);
    assert_eq!(args["operation"], "insert");
    assert_eq!(args["table"], "scratch");
    assert_eq!(args["row"]["kind"], "canon-card");
    let parked: serde_json::Value =
        serde_json::from_str(args["row"]["payload"].as_str().expect("payload")).expect("json");
    assert_eq!(
        parked,
        serde_json::json!({"collects": "multi"}),
        "the parked shape is the MAP the read stack needs, not the rows: \
         a payload the next phase has to re-derive is a second rule"
    );
    // The read chain grew a second table in W6 (the refused claim pairs), so what
    // the park fires is the next READ rather than the scan directly; the scan
    // still hangs on the last of them (`w6_claim_aliases.rs` pins the whole
    // chain). What this pin owns is that the verdicts are parked as a map before
    // anything else happens.
    assert_eq!(
        msgs[0]["header"]["phase"], "canon-refused-fetch",
        "and the park is what carries the round forward"
    );
}

#[test]
fn a_store_without_the_table_still_runs_the_round() {
    // The migration creates the table, but a colony whose store cell is older
    // than the declaration answers with an error outcome instead of rows. That
    // must cost the night its cardinality question and nothing else -- the two
    // identity questions and the currency question have no stake in it.
    let msgs = emit(store_reply(
        "canon-card",
        "select",
        serde_json::json!({"error": "unknown table"}),
    ));
    let args = args_of(&msgs[0]);
    assert_eq!(args["row"]["kind"], "canon-card");
    assert_eq!(
        args["row"]["payload"], "{}",
        "no verdicts is an empty map, never a broken round"
    );
}

#[test]
fn the_scan_still_reads_the_columns_the_currency_question_needs() {
    // W3's pin, one hop further down the chain: the fact scan now hangs on the
    // park of the judged verdicts instead of on the belief apply. The claim it
    // carries is unchanged -- without `canonical_claim` the lane cannot tell a
    // re-assertion from a change, and without `valid_from` the judge cannot say
    // which of two statements is the later one.
    //
    // One hop further still since GH #73: the identity questions read the CLOSED
    // rows before the fact page is asked for, so the page is emitted from
    // `canon-closed` -- which with nothing closed to report emits only the page.
    let msgs = emit(store_reply("canon-closed", "select", serde_json::json!([])));
    let args = args_of(&msgs[0]);
    assert_eq!(args["operation"], "select");
    assert_eq!(args["table"], "facts");
    let columns = args["columns"].as_array().expect("columns").clone();
    for needed in [
        "id",
        "canonical_subject",
        "canonical_predicate",
        "claim",
        "canonical_claim",
        "valid_from",
    ] {
        assert!(
            columns.iter().any(|c| c == needed),
            "the scan lost {needed}: {}",
            args["columns"]
        );
    }
}

// ------------------------------------------------------------- the question

/// One open fact row in the shape the canonicalisation scan projects.
fn row(id: &str, subject: &str, predicate: &str, claim: &str, from: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "subject": subject, "canonical_subject": subject,
        "predicate": predicate, "canonical_predicate": predicate,
        "claim": claim, "canonical_claim": claim,
        "valid_from": from, "recorded_at": from
    })
}

/// A store with every shape the cardinality question has to tell apart: an
/// unlisted predicate with two values on one axis (the question), a SEEDED one
/// with two values (the authority answered it), and an unlisted one carrying a
/// single statement per axis (nothing to decide).
fn card_rows() -> serde_json::Value {
    serde_json::json!([
        row(
            "a1",
            "user",
            "collects",
            "collects vinyl",
            "2026-01-01T00:00:00Z"
        ),
        row(
            "a2",
            "user",
            "collects",
            "collects stamps",
            "2026-02-01T00:00:00Z"
        ),
        row(
            "b1",
            "user",
            "has_child",
            "has a child named ada",
            "2026-01-01T00:00:00Z"
        ),
        row(
            "b2",
            "user",
            "has_child",
            "has a child named ben",
            "2026-02-01T00:00:00Z"
        ),
        row(
            "c1",
            "user",
            "commutes_by",
            "commutes by bike",
            "2026-01-01T00:00:00Z"
        ),
        row(
            "c2",
            "site:a",
            "commutes_by",
            "commutes by tram",
            "2026-02-01T00:00:00Z"
        )
    ])
}

/// The scan payload, parked for the meeting point.
fn parked_scan(rows: serde_json::Value) -> serde_json::Value {
    let msgs = emit(store_reply("canon-scan", "select", rows));
    let args = args_of(&msgs[0]);
    assert_eq!(args["row"]["kind"], "canon-scan");
    serde_json::from_str(args["row"]["payload"].as_str().expect("payload")).expect("scan payload")
}

#[test]
fn the_scan_offers_the_predicates_whose_cardinality_is_still_open() {
    let scan = parked_scan(card_rows());
    let card = scan["cardinality"].as_array().expect("cardinality section");
    let names: Vec<&str> = card
        .iter()
        .map(|c| c["predicate"].as_str().expect("predicate"))
        .collect();
    assert_eq!(
        names,
        vec!["collects"],
        "one axis with two open values is the question; a SEEDED predicate is \
         already answered by the authority and a predicate whose axes carry one \
         statement each has nothing to decide: {card:?}"
    );
    assert_eq!(
        card[0]["values"],
        serde_json::json!(["collects stamps", "collects vinyl"]),
        "the judge decides on the VALUES, so the values travel -- sorted, because \
         two runs over one store have to build the same payload"
    );
}

#[test]
fn a_store_with_nothing_to_decide_builds_the_payload_w4_built() {
    // Additive by construction, the property W4 established for its own key: the
    // section exists only when there is a question, so a night on a store whose
    // cardinality is settled receipts byte for byte what it receipted before.
    let scan = parked_scan(serde_json::json!([
        row(
            "b1",
            "user",
            "has_child",
            "has a child named ada",
            "2026-01-01T00:00:00Z"
        ),
        row(
            "b2",
            "user",
            "has_child",
            "has a child named ben",
            "2026-02-01T00:00:00Z"
        )
    ]));
    assert!(
        scan.get("cardinality").is_none(),
        "an empty question is no key at all: {scan}"
    );
}

/// The judge call the round builds out of a parked scan plus a parked verdict map.
fn asked(scan: &serde_json::Value, judged: &serde_json::Value) -> serde_json::Value {
    let msgs = emit(store_reply(
        "canon-ask",
        "select",
        serde_json::json!([
            {"key": RUN, "kind": "canon-scan", "payload": scan.to_string()},
            {"key": RUN, "kind": "canon-pairs", "payload": "[]"},
            {"key": RUN, "kind": "canon-card", "payload": judged.to_string()}
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
fn a_predicate_the_store_already_judged_is_not_asked_again() {
    // The P5 lesson one dimension over: a verdict that is written down is not
    // bought a second time. Here it is not only about tokens -- the budget is
    // small and the ranking is stable, so re-asking the head means the tail is
    // never reached.
    let scan = parked_scan(card_rows());
    let payload = asked(&scan, &serde_json::json!({"collects": "multi"}));
    assert!(
        payload.get("cardinality").is_none(),
        "the only open question was already decided: {payload}"
    );
    let open = asked(&scan, &serde_json::json!({}));
    assert_eq!(
        open["cardinality"][0]["predicate"], "collects",
        "and without a stored verdict it IS asked: {open}"
    );
}

#[test]
fn the_cardinality_question_is_budgeted() {
    // Same shape as every other section of this payload: a memory that has been
    // running for years must not be able to grow the one prompt of the night.
    let mut rows = vec![];
    for i in 0..12 {
        for v in 0..2 {
            rows.push(row(
                &format!("f{i}{v}"),
                "user",
                &format!("relation_{i:02}"),
                &format!("value {i} number {v}"),
                &format!("2026-0{}-0{}T00:00:00Z", i % 9 + 1, v + 1),
            ));
        }
    }
    let scan = parked_scan(serde_json::Value::Array(rows));
    let payload = asked(&scan, &serde_json::json!({}));
    let card = payload["cardinality"].as_array().expect("cardinality");
    assert_eq!(
        card.len(),
        8,
        "MEMORY_CANON_MAX_CARD bounds the section: {card:?}"
    );
}

#[test]
fn the_cardinality_question_stands_on_its_own() {
    // A store nobody drifted, with no candidate pairs and no multi-statement
    // axis, can still owe an answer about a relation -- so the "no question, no
    // call" guard has to count this section too, or the verdict would never be
    // asked for on exactly the stores that are otherwise quiet.
    let scan = serde_json::json!({
        "predicates": {"collects": ["user"]}, "context": {}, "axes": [],
        "cardinality": [{"predicate": "collects", "values": ["a", "b"]}]
    });
    let payload = asked(&scan, &serde_json::json!({}));
    assert_eq!(payload["cardinality"][0]["predicate"], "collects");
}

// ----------------------------------------------------------- the read stack

/// Run a probe against a script, with the script's own globals in scope.
fn probe(script: &str, name: &str, body: &str) -> String {
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
        serde_json::to_string(script).unwrap(),
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
        "{name} stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Two facts of one axis at ONE instant from TWO sessions -- the shape the
/// learned rule reads as an enumeration since W1.
fn learned_multi_chain(predicate: &str) -> String {
    format!(
        r#"
rows = [
 {{"id":"a","subject":"user:u1","predicate":"{predicate}","canonical_predicate":"{predicate}",
  "claim":"value one","valid_from":"2026-01-01T00:00:00Z","valid_until":None,
  "recorded_at":"2026-01-01T00:00:00Z","episode_id":"e1","session_id":"s1"}},
 {{"id":"b","subject":"user:u1","predicate":"{predicate}","canonical_predicate":"{predicate}",
  "claim":"value two","valid_from":"2026-01-01T00:00:00Z","valid_until":None,
  "recorded_at":"2026-01-02T00:00:00Z","episode_id":"e2","session_id":"s2"}}
]
chain = build_chains(rows)[('user:u1','{predicate}')]
"#
    )
}

/// Both copies of the lifted chain block answer the same question, so every pin
/// of the read stack runs against both -- that is what the drift lock is for.
fn on_both(body: &str) -> Vec<(String, String)> {
    [(recall_script(), "recall"), (glue_script(), "dream-glue")]
        .into_iter()
        .map(|(script, name)| (name.to_string(), probe(&script, name, body)))
        .collect()
}

#[test]
fn the_read_stack_puts_the_seed_in_front_of_a_judgement() {
    // Ruling Q3, the top of the precedence. The curated list is the AUTHORITY: a
    // judged verdict cannot move `has_child` off the enumerating half, and it
    // cannot move `favorite_color` onto it either. The write path refuses to
    // store such a verdict at all (`the_seed_list_is_never_overwritten_by_a_
    // judgement`); this is the same precedence proven from the reading end, for
    // the store that somehow carries one anyway.
    for (predicate, judged, expected) in [
        ("has_child", "single", "True"),
        ("favorite_color", "multi", "False"),
    ] {
        let body = learned_multi_chain(predicate)
            + &format!("print(axis_is_multivalued(chain, {{'{predicate}': '{judged}'}}))\n");
        for (name, got) in on_both(&body) {
            assert_eq!(
                got, expected,
                "{name}: a judgement overruled the seed on {predicate}"
            );
        }
    }
}

#[test]
fn a_judgement_stands_in_front_of_the_learned_rule() {
    // The middle of the precedence, and the half that buys something: the
    // learned rule reads two values of one instant from two sessions as an
    // enumeration, and a judge that has seen the relation can say otherwise.
    let body = learned_multi_chain("collects")
        + "print(axis_is_multivalued(chain, {'collects': 'single'}),\n"
        + "      axis_is_multivalued(chain, {'collects': 'multi'}),\n"
        + "      axis_is_multivalued(chain, {}))\n";
    for (name, got) in on_both(&body) {
        assert_eq!(
            got, "False True True",
            "{name}: seed > judged > learned is not the order the reader applies"
        );
    }
}

#[test]
fn a_judgement_reaches_an_axis_the_learned_rule_says_nothing_about() {
    // Two values, two instants, one session: no coexistence evidence at all, so
    // before W5 the axis was functional by default. A stored verdict is the only
    // thing that can make it enumerate -- which is the case ruling Q3 exists for.
    let body = r#"
rows = [
 {"id":"a","subject":"user:u1","predicate":"collects","canonical_predicate":"collects",
  "claim":"collects vinyl","valid_from":"2026-01-01T00:00:00Z","valid_until":None,
  "recorded_at":"2026-01-01T00:00:00Z","episode_id":"e1","session_id":"s1"},
 {"id":"b","subject":"user:u1","predicate":"collects","canonical_predicate":"collects",
  "claim":"collects stamps","valid_from":"2026-02-01T00:00:00Z","valid_until":None,
  "recorded_at":"2026-02-01T00:00:00Z","episode_id":"e2","session_id":"s1"}
]
chain = build_chains(rows)[('user:u1','collects')]
print(axis_is_multivalued(chain, {}), axis_is_multivalued(chain, {'collects': 'multi'}))
"#;
    for (name, got) in on_both(body) {
        assert_eq!(got, "False True", "{name}: the judged layer never fired");
    }
}

#[test]
fn a_verdict_the_reader_does_not_understand_changes_nothing() {
    // The map comes from a store column, and a column can carry anything. An
    // unknown word is not a third answer: the reader falls through to the rule
    // it had before, which is the conservative direction.
    let body = learned_multi_chain("collects")
        + "print(axis_is_multivalued(chain, {'collects': 'sometimes'}),\n"
        + "      axis_is_multivalued(chain, {'other': 'single'}),\n"
        + "      axis_is_multivalued(chain, None))\n";
    for (name, got) in on_both(&body) {
        assert_eq!(got, "True True True", "{name}: a stray verdict was read");
    }
}

#[test]
fn the_judged_verdict_decides_presentation_and_nothing_else() {
    // W2's rule stands: the cardinality answer is a tie-breaker of the READ path
    // and never a source of supersession. A `multi` verdict on an axis carrying
    // a re-assertion must not stop the chain from closing the older assertion,
    // and a `single` verdict must not close anything either.
    let body = r#"
rows = [
 {"id":"a","subject":"user:u1","predicate":"collects","canonical_predicate":"collects",
  "claim":"collects vinyl","canonical_claim":"collects vinyl",
  "valid_from":"2026-01-01T00:00:00Z","valid_until":None,
  "recorded_at":"2026-01-01T00:00:00Z","session_id":"s1"},
 {"id":"b","subject":"user:u1","predicate":"collects","canonical_predicate":"collects",
  "claim":"collects vinyl","canonical_claim":"collects vinyl",
  "valid_from":"2026-02-01T00:00:00Z","valid_until":None,
  "recorded_at":"2026-02-01T00:00:00Z","session_id":"s1"},
 {"id":"c","subject":"user:u1","predicate":"collects","canonical_predicate":"collects",
  "claim":"collects stamps","canonical_claim":"collects stamps",
  "valid_from":"2026-03-01T00:00:00Z","valid_until":None,
  "recorded_at":"2026-03-01T00:00:00Z","session_id":"s1"}
]
print(derive_supersessions(rows))
"#;
    let got = probe(&glue_script(), "dream-glue", body);
    assert_eq!(
        got, "[('a', '2026-02-01T00:00:00Z', 'b'), ('b', None, None), ('c', None, None)]",
        "the write path does not consult the cardinality at all, and it must not \
         start to: a wrong verdict has to stay unable to end a true value"
    );
}

// ------------------------------------------------- the read path fetches it

const RECALL: &str = "q1";

/// One store reply on the recall lane, as the edge delivers it.
fn recall_reply(phase: &str, operation: &str, rows: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "header": {
            "context": {"store_origin": "recall", "mem_phase": phase, "recall_id": RECALL,
                        "recall_query": "collects", "memory_tier": "1",
                        "recall_as_of": "2026-08-01T00:00:00Z"},
            "hop": {"operation": operation, "rows_affected": 1}
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "r", "text": rows.to_string()}]
    })
}

fn recall_emit(doc: serde_json::Value) -> Vec<serde_json::Value> {
    emit_of(&recall_script(), doc)
}

#[test]
fn the_hydration_asks_for_the_judged_verdicts() {
    // The verdicts are a LEG of the hydration fan, not a round trip of their
    // own: the ids are in hand at that moment, the ops go out together and the
    // fan-in that already waits for the axis page waits for one more row. A
    // serial select here would cost a hop on the lane that runs while somebody
    // is waiting for an answer.
    let msgs = recall_emit(recall_reply(
        "t1-hyd-fact",
        "select",
        serde_json::json!([{"id": "a", "subject": "user", "canonical_subject": "user",
                            "predicate": "collects", "canonical_predicate": "collects",
                            "claim": "collects vinyl"}]),
    ));
    let card = msgs
        .iter()
        .map(args_of)
        .find(|a| a["table"] == "predicate_cardinality")
        .expect("the hydration does not read the judged verdicts");
    assert_eq!(card["operation"], "select");
    assert_eq!(
        card["where"],
        serde_json::json!({"canonical_predicate": {"in": ["collects"]}}),
        "only the relations of this bundle -- the table is the store's, the \
         question is this request's"
    );
}

#[test]
fn a_bundle_without_a_fact_hit_still_fans_in() {
    // The empty branch is where a fan-in dies: a gate waiting for a leg nobody
    // parks parks forever, and the request answers nothing at all.
    let msgs = recall_emit(recall_reply("t1-hyd-fact", "select", serde_json::json!([])));
    let legs: Vec<serde_json::Value> = msgs
        .iter()
        .map(args_of)
        .filter(|a| a["table"] == "recall_scratch" && a["operation"] == "insert")
        .map(|a| a["row"]["leg"].clone())
        .collect();
    assert!(
        legs.contains(&serde_json::json!("card")),
        "the cardinality leg has to be parked empty, not left out: {legs:?}"
    );
}

#[test]
fn the_projection_reads_the_verdict_of_the_bundle() {
    // The whole stack in one probe, on the shape the verdict actually changes:
    // an axis whose older assertion the chain has closed. Judged `single` (or no
    // verdict at all) collapses the hit onto the statement still standing and
    // hands over the history; judged `multi` says the axis has no current value
    // to collapse onto, so every hit stays its own answer.
    let rows = r#"
axis = [
 {"id":"a","subject":"user","canonical_subject":"user","predicate":"collects",
  "canonical_predicate":"collects","claim":"collects vinyl","canonical_claim":"collects vinyl",
  "valid_from":"2026-01-01T00:00:00Z","recorded_at":"2026-01-01T00:00:00Z","session_id":"s1"},
 {"id":"b","subject":"user","canonical_subject":"user","predicate":"collects",
  "canonical_predicate":"collects","claim":"collects vinyl","canonical_claim":"collects vinyl",
  "valid_from":"2026-05-01T00:00:00Z","recorded_at":"2026-05-01T00:00:00Z","session_id":"s2"}
]
"#;
    let body = format!(
        "{rows}print(project_fact_candidate('a', axis, {{}})['id'],\n\
         \x20     project_fact_candidate('a', axis, {{'collects': 'single'}})['id'],\n\
         \x20     project_fact_candidate('a', axis, {{'collects': 'multi'}})['id'])\n"
    );
    assert_eq!(
        probe(&recall_script(), "recall", &body),
        "b b a",
        "the judged verdict never reached the projection"
    );
}

// --------------------------------------------------------------- the write

/// The judgement, in the answer shape the instructions name.
fn card_judgement(items: &str) -> Vec<serde_json::Value> {
    emit(judgement(&format!("{{\"cardinality\": [{items}]}}")))
}

#[test]
fn a_cardinality_verdict_becomes_an_attributed_row() {
    let msgs = card_judgement(
        r#"{"predicate": "collects", "verdict": "multi",
            "reason": "the values coexist, a new collection ends no earlier one"}"#,
    );
    let ops = ops_on(&msgs, "predicate_cardinality");
    assert_eq!(
        ops.len(),
        2,
        "the upsert of a store without joins is a delete plus an insert: {ops:?}"
    );
    assert_eq!(
        ops[0],
        serde_json::json!({"operation": "delete", "table": "predicate_cardinality",
                           "where": {"canonical_predicate": "collects"}}),
        "the old verdict goes first, or two rows would answer one question"
    );
    assert_eq!(
        ops[1],
        serde_json::json!({"operation": "insert", "table": "predicate_cardinality",
                           "row": {"canonical_predicate": "collects", "verdict": "multi",
                                   "source": "judge:r1", "decided_at": TO}}),
        "`source` is the mandatory column of ruling Q3: it names the author AND the \
         night, so 'why does this axis enumerate' is answerable from the data and one \
         night is reverted with one `where`"
    );
}

#[test]
fn the_seed_list_is_never_overwritten_by_a_judgement() {
    // Precedence as a REFUSAL rather than as a read-time rule: a seeded predicate
    // never becomes a row at all, so no reader can ever be handed a judged verdict
    // that contradicts the authority. The read stack puts the seed first as well
    // (`the_read_stack_puts_the_seed_in_front_of_a_judgement`) -- one precedence,
    // proven on both sides.
    for seeded in ["has_child", "favorite_color"] {
        let msgs = card_judgement(&format!(
            r#"{{"predicate": "{seeded}", "verdict": "single",
                 "reason": "looks like one value at a time to me"}}"#
        ));
        assert!(
            ops_on(&msgs, "predicate_cardinality").is_empty(),
            "a judgement overruled the curated authority on {seeded}"
        );
    }
}

#[test]
fn a_verdict_that_cannot_be_read_back_is_dropped() {
    // Same discipline as the closures: no reason, no verdict. Plus the two shapes
    // a hallucinating judge produces here -- an invented word instead of one of
    // the two the question offers, and an empty relation.
    for bad in [
        r#"{"predicate": "collects", "verdict": "multi", "reason": ""}"#,
        r#"{"predicate": "collects", "verdict": "sometimes", "reason": "not sure"}"#,
        r#"{"predicate": "", "verdict": "multi", "reason": "some relation enumerates"}"#,
        r#"["collects", "multi"]"#,
    ] {
        let msgs = card_judgement(bad);
        assert!(
            ops_on(&msgs, "predicate_cardinality").is_empty(),
            "this verdict should have been dropped: {bad}"
        );
    }
}

#[test]
fn the_reason_is_folded_into_the_run_receipt() {
    // Where this lane quits every other dream artefact (W3/W4 pattern): the
    // verdict payload of `consolidation_log`, under the run id that `source`
    // names. The key appears only when something was judged, so a night without
    // a cardinality verdict receipts byte for byte what it always did.
    let msgs = card_judgement(
        r#"{"predicate": "collects", "verdict": "multi", "reason": "collections coexist"}"#,
    );
    let parked: Vec<serde_json::Value> = msgs
        .iter()
        .map(args_of)
        .filter(|a| a["table"] == "scratch" && a["row"]["kind"] == "canon-cardinality")
        .collect();
    assert_eq!(parked.len(), 1, "the reason was not parked: {msgs:?}");

    let run = emit(store_reply(
        "apply-run",
        "select",
        serde_json::json!([
            {"key": RUN, "kind": "verdicts", "payload": "{}"},
            {"key": RUN, "kind": "beliefs", "payload": "[]"},
            {"key": RUN, "kind": "canon-cardinality",
             "payload": parked[0]["row"]["payload"]}
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
        verdicts["cardinality"][0]["reason"], "collections coexist",
        "a verdict nobody can read back cannot be reverted either: {verdicts}"
    );
    assert_eq!(verdicts["cardinality"][0]["predicate"], "collects");
}

#[test]
fn a_night_without_a_cardinality_verdict_receipts_what_it_always_did() {
    let run = emit(store_reply(
        "apply-run",
        "select",
        serde_json::json!([
            {"key": RUN, "kind": "verdicts", "payload": "{}"},
            {"key": RUN, "kind": "beliefs", "payload": "[]"}
        ]),
    ));
    let closed = run
        .iter()
        .map(args_of)
        .find(|a| a["table"] == "consolidation_log")
        .expect("the run has to close");
    assert_eq!(
        closed["set"]["verdicts"], "{}",
        "an empty question leaves no key behind"
    );
}

#[test]
fn the_prompt_asks_the_cardinality_question_and_names_its_answer_shape() {
    // One call a night, one payload, sections that give each other context (P5
    // ruling). W5 adds a section, not a call -- and a section the instructions do
    // not describe is a section the judge answers in a shape the lane drops.
    let scan = parked_scan(card_rows());
    let msgs = emit(store_reply(
        "canon-ask",
        "select",
        serde_json::json!([
            {"key": RUN, "kind": "canon-scan", "payload": scan.to_string()},
            {"key": RUN, "kind": "canon-pairs", "payload": "[]"},
            {"key": RUN, "kind": "canon-card", "payload": "{}"}
        ]),
    ));
    let text = msgs[0]["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions");
    assert!(
        text.contains(r#""cardinality":[{"predicate":"","verdict":"","reason":""}]"#),
        "the answer shape has to carry the section: {text}"
    );
    for anchor in [
        "4. `cardinality`",
        "`single`",
        "`multi`",
        "PRESENTED",
        "A verdict without a reason is dropped",
    ] {
        assert!(
            text.contains(anchor),
            "the cardinality paragraph is missing {anchor:?}"
        );
    }
}
