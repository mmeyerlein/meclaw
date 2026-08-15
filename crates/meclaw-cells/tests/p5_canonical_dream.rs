//! 0.2.0 P5 -- the nightly canonicalisation round of the dream lane.
//!
//! Rulings Q1 (the GC is the persisting twin of the extractor, judged by a
//! top-tier model), Q3 (aliases first, then ONE canonicalize) and Q5 (the fuzzy
//! index proposes, the GC judges). GitHub #24.
//!
//! The mechanism this file pins is the one thing about a nightly job that cannot
//! be observed by watching it run: given the same store, the round has to emit
//! the same ops in the same order, and a judgement has to reach the store as
//! aliases and refusals rather than as an edit. Everything here runs the REAL
//! `params.script_inline` of `dream-glue` against injected store replies, so no
//! model is called and nothing costs anything.
//!
//! What is deliberately NOT here: whether the judge answers well. That is a
//! model property, it belongs to the paid run (scenario C6), and the free
//! scenario C5 proves the same chain end to end on a real colony with the
//! judgement injected.

use std::io::Write;
use std::process::{Command, Stdio};

const GLUE_CONFIG: &str = "../../templates/memory-hive/dream-glue/config.json";

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

fn glue_script() -> String {
    let raw = std::fs::read_to_string(GLUE_CONFIG).expect("dream-glue config");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("config json");
    resolve_vars(
        v["params"]["script_inline"]
            .as_str()
            .expect("script_inline"),
    )
}

/// Run the real script with a real stdin document and return the emitted messages.
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

/// One store reply as the edge delivers it: the run keys in the context, the
/// operation in the hop, the projected rows as the first message's text.
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

fn phases(msgs: &[serde_json::Value]) -> Vec<String> {
    msgs.iter()
        .map(|m| {
            m["header"]["phase"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The rows of a drifted store: one relation under two spellings, one entity
/// under two spellings, plus an untouched axis of the same person.
fn drifted_rows() -> serde_json::Value {
    serde_json::json!([
        {"id":"a","subject":"user","canonical_subject":"user",
         "predicate":"favorite editor","canonical_predicate":"favorite editor",
         "claim":"favorite editor is helix"},
        {"id":"b","subject":"user:u1","canonical_subject":"user:u1",
         "predicate":"Lieblingseditor","canonical_predicate":"Lieblingseditor",
         "claim":"favorite editor is vscode"},
        {"id":"c","subject":"user","canonical_subject":"user",
         "predicate":"lives_in","canonical_predicate":"lives_in",
         "claim":"lives in zone-a"}
    ])
}

#[test]
fn the_belief_apply_opens_the_canonicalisation_round() {
    // The round sits AFTER the dreamer's verdict is parked and BEFORE the
    // supersession arithmetic. That order is the point: the arithmetic groups on
    // the canonical columns, so an axis merged tonight fires its chain tonight.
    //
    // Which DOOR the round opens moved with statement identity W5 (#13, ruling
    // Q3): the first read is the judged cardinality, and the fact scan hangs
    // behind it (`w5_judged_cardinality.rs` pins both halves). The claim of this
    // pin is unchanged and is the ordering one -- the belief apply enters the
    // round, not the arithmetic.
    let msgs = emit(store_reply("apply", "insert", serde_json::json!([])));
    assert_eq!(phases(&msgs), vec!["canon-card"]);
    let args = args_of(&msgs[0]);
    assert_eq!(args["operation"], "select");
    assert_ne!(
        args["table"], "facts",
        "the arithmetic scan is not the entry of the round"
    );
    let scan = args_of(
        &emit(store_reply(
            "canon-scan-fetch",
            "insert",
            serde_json::json!([]),
        ))[0],
    );
    assert_eq!(scan["table"], "facts");
    for needed in ["canonical_subject", "canonical_predicate", "claim"] {
        assert!(
            scan["columns"]
                .as_array()
                .expect("columns")
                .iter()
                .any(|c| c == needed),
            "the scan needs {needed}, got {}",
            scan["columns"]
        );
    }
}

#[test]
fn the_scan_parks_the_inventory_and_the_entity_context() {
    let msgs = emit(store_reply("canon-scan", "select", drifted_rows()));
    assert_eq!(phases(&msgs), vec!["canon-cand"]);
    let args = args_of(&msgs[0]);
    assert_eq!(args["table"], "scratch");
    assert_eq!(args["row"]["kind"], "canon-scan");
    let parked: serde_json::Value =
        serde_json::from_str(args["row"]["payload"].as_str().expect("payload")).expect("scan");
    // the inventory is keyed on the IDENTITY of the relation, and it carries the
    // subjects the key is used on -- that context is what lets the judge tell a
    // synonym from a different relation that merely looks alike
    assert_eq!(
        parked["predicates"]["favorite editor"],
        serde_json::json!(["user"])
    );
    assert_eq!(
        parked["predicates"]["Lieblingseditor"],
        serde_json::json!(["user:u1"])
    );
    // and the entity context is the facts of each side, for the pair question
    assert_eq!(
        parked["context"]["user:u1"],
        serde_json::json!(["favorite editor is vscode"])
    );
    assert_eq!(
        parked["context"]["user"],
        serde_json::json!(["favorite editor is helix", "lives in zone-a"])
    );
}

#[test]
fn the_round_asks_the_index_for_pairs_before_it_asks_the_model() {
    // Ruling Q5 made structural: the similarity index is the candidate machine,
    // the model is the judge. The lane cannot merge on a score because it never
    // holds one -- it only ever forwards a question.
    let msgs = emit(store_reply("canon-cand", "insert", serde_json::json!([])));
    assert_eq!(phases(&msgs), vec!["canon-park"]);
    let args = args_of(&msgs[0]);
    assert_eq!(args["operation"], "alias_candidates");
    assert_eq!(args["table"], "facts");
    assert_eq!(args["column"], "subject");
}

/// The parked halves, as the meeting-point select hands them back.
fn meeting_point(predicates: serde_json::Value, pairs: serde_json::Value) -> serde_json::Value {
    let scan = serde_json::json!({
        "predicates": predicates,
        "context": {"user": ["favorite editor is helix"],
                    "user:u1": ["favorite editor is vscode"]}
    });
    serde_json::json!([
        {"key": RUN, "kind": "canon-scan", "payload": scan.to_string()},
        {"key": RUN, "kind": "canon-pairs", "payload": pairs.to_string()}
    ])
}

#[test]
fn both_questions_reach_the_judge_in_one_payload() {
    let pairs = serde_json::json!([{"left": "user", "right": "user:u1", "score": 0.6}]);
    let msgs = emit(store_reply(
        "canon-ask",
        "select",
        meeting_point(
            serde_json::json!({"favorite editor": ["user"], "Lieblingseditor": ["user:u1"]}),
            pairs,
        ),
    ));
    assert_eq!(msgs.len(), 1, "one call a night, not one per question");
    assert_eq!(msgs[0]["header"]["route"], "judge");
    let payload: serde_json::Value =
        serde_json::from_str(msgs[0]["messages"][0]["text"].as_str().expect("payload"))
            .expect("payload json");
    assert_eq!(
        payload["predicates"],
        serde_json::json!([
            {"predicate": "Lieblingseditor", "subjects": ["user:u1"]},
            {"predicate": "favorite editor", "subjects": ["user"]}
        ]),
        "sorted, so two runs over one store put the same question"
    );
    let pair = &payload["entity_pairs"][0];
    assert_eq!(pair["left"], "user");
    assert_eq!(pair["right"], "user:u1");
    // ruling Q5: the FULL context of both nodes, not the score alone
    assert_eq!(
        pair["left_facts"],
        serde_json::json!(["favorite editor is helix"])
    );
    assert_eq!(
        pair["right_facts"],
        serde_json::json!(["favorite editor is vscode"])
    );
    let instructions = msgs[0]["system"]["instructions"]["text"]
        .as_str()
        .expect("instructions");
    assert!(
        instructions.contains("favorite_color") && instructions.contains("has_child"),
        "the core vocabulary is the target anchor of the judgement (ruling Q1)"
    );
    assert!(
        instructions.contains("Never invent a value"),
        "the entity-fidelity rule of ruling Q2 has to be in the judge prompt too"
    );
}

#[test]
fn nothing_to_judge_costs_nothing() {
    // A store with one relation and no candidate pair has no question, and the
    // most expensive model of the hive is not called to confirm that.
    let msgs = emit(store_reply(
        "canon-ask",
        "select",
        meeting_point(
            serde_json::json!({"favorite_editor": ["user"]}),
            serde_json::json!([]),
        ),
    ));
    assert_eq!(phases(&msgs), vec!["sup-scope"]);
    assert_ne!(msgs[0]["header"]["route"], "judge");
}

#[test]
fn a_judgement_becomes_aliases_refusals_and_exactly_one_re_derive() {
    // Ruling Q3's order, made observable: every alias of the run first, as ONE
    // set, then a single canonicalize. A reader never sees half a judgement.
    let msgs = emit(judgement(
        r#"{"predicates":[{"alias":"Lieblingseditor","canonical":"favorite_editor"}],
            "entities":[{"alias":"user:u1","canonical":"user"}],
            "different":[{"dimension":"subject","left":"leon","right":"leona"}]}"#,
    ));
    // The trailing `llm-call` is the judge's line in the books (GH #64): the
    // round is the second model call of the night, and since the run closes
    // several store round trips later its receipt is parked like every other
    // artefact of this lane. It sits BEHIND the re-derive because the chain
    // hangs on the canonicalize -- a book entry, not a step.
    assert_eq!(
        phases(&msgs),
        vec![
            "canon-alias",
            "canon-alias",
            "canon-alias",
            "canon-done",
            "llm-call"
        ]
    );
    let ops: Vec<serde_json::Value> = msgs.iter().map(args_of).collect();
    assert_eq!(
        ops[0],
        serde_json::json!({"operation": "set_alias", "table": "facts", "column": "predicate",
                           "alias": "Lieblingseditor", "canonical": "favorite_editor",
                           "recorded_at": TO}),
        "the relation dimension is named explicitly -- an alias is a statement \
         about exactly one identity"
    );
    assert_eq!(
        ops[1],
        serde_json::json!({"operation": "set_alias", "table": "facts", "column": "subject",
                           "alias": "user:u1", "canonical": "user", "recorded_at": TO})
    );
    assert_eq!(
        ops[2],
        serde_json::json!({"operation": "reject_pair", "table": "facts", "column": "subject",
                           "left": "leon", "right": "leona", "recorded_at": TO}),
        "the refusal is persisted, or the same pair is bought again tomorrow"
    );
    assert_eq!(
        ops[3],
        serde_json::json!({"operation": "canonicalize", "table": "facts"}),
        "one re-derive over BOTH dimensions, after the aliases"
    );
    // and nothing else: no delete, no update on facts. Since W3 (#13) the same
    // judgement may also carry CLOSURES, which are updates -- this input has
    // none, so an identity judgement still writes aliases and refusals only.
    for op in &ops {
        assert!(
            !matches!(op["operation"].as_str(), Some("delete") | Some("update")),
            "the round wrote {op} -- an identity judgement may only ever add an \
             alias or a refusal"
        );
    }
}

#[test]
fn a_fenced_or_prefixed_answer_is_still_read() {
    // The lesson the extraction lane paid for in P15: the first live run of a new
    // llm cell is where the ```json fence shows up, and a lane that only accepts
    // bare JSON turns that into a silent nightly no-op.
    let msgs = emit(judgement(
        "Here is my judgement:\n```json\n{\"entities\":[{\"alias\":\"elvse\",\"canonical\":\"elvese\"}]}\n```",
    ));
    assert_eq!(phases(&msgs), vec!["canon-alias", "canon-done", "llm-call"]);
    assert_eq!(args_of(&msgs[0])["alias"], "elvse");
}

#[test]
fn an_empty_or_self_referential_judgement_writes_nothing_but_the_re_derive() {
    // A self-alias says nothing, and an empty side would move real rows onto an
    // identity nobody named. Both are dropped one by one, never abort the round.
    let msgs = emit(judgement(
        r#"{"predicates":[{"alias":"lives_in","canonical":"lives_in"},
                          {"alias":"","canonical":"favorite_editor"},
                          {"alias":"diet","canonical":""}],
            "entities":["not an object"],
            "different":[{"dimension":"nonsense","left":"a","right":"b"},
                         {"dimension":"subject","left":"a","right":"a"}]}"#,
    ));
    // A judgement the lane drops entirely still books its call (GH #64): the
    // model was asked and answered, and a night whose books hide that reads as
    // cheaper than the provider bill says it was.
    assert_eq!(phases(&msgs), vec!["canon-done", "llm-call"]);
    assert_eq!(args_of(&msgs[0])["operation"], "canonicalize");
}

#[test]
fn a_judge_failure_skips_the_round_instead_of_stalling_the_run() {
    // The round is an improvement, never a precondition: a dead judge must not
    // cost the run its supersession arithmetic, its beliefs or its backfills.
    //
    // Both skip paths still book the call (GH #64). That is the point of putting
    // the receipt on every exit of the phase: a judge that errored or answered in
    // prose was called and billed, and the round is exactly the call the run's
    // books used to hide.
    let mut doc = judgement("irrelevant");
    doc["header"]["hop"]["finish_reason"] = serde_json::json!("error");
    assert_eq!(phases(&emit(doc)), vec!["sup-scope", "llm-call"]);
    assert_eq!(
        phases(&emit(judgement("I could not decide, sorry."))),
        vec!["sup-scope", "llm-call"],
        "an unparseable answer is the same class: skip, do not stall"
    );
}

#[test]
fn the_round_rejoins_the_supersession_arithmetic_through_one_door() {
    // Three ways into the arithmetic (round off, round done, round skipped) and
    // they have to be the SAME op -- otherwise a run materialises different spans
    // depending on how it got there.
    let after_round = emit(store_reply(
        "canon-done",
        "canonicalize",
        serde_json::json!([]),
    ));
    assert_eq!(phases(&after_round), vec!["sup-scope"]);
    let skipped = emit(store_reply(
        "canon-ask",
        "select",
        meeting_point(
            serde_json::json!({"favorite_editor": ["user"]}),
            serde_json::json!([]),
        ),
    ));
    assert_eq!(args_of(&after_round[0]), args_of(&skipped[0]));
}

#[test]
fn the_switch_takes_the_ask_away_and_leaves_the_apply() {
    // MEMORY_CANON_JUDGE=0 is the cost switch: no scan, no candidate feed, no
    // model call -- the run goes straight into the arithmetic, exactly as it did
    // before this package. What it does NOT disable is applying a judgement that
    // was handed in, because applying is arithmetic and asking is what costs.
    let off = glue_script().replace(
        "CANON_JUDGE = \"1\" == \"1\"",
        "CANON_JUDGE = \"0\" == \"1\"",
    );
    assert!(
        off != glue_script(),
        "the switch literal moved -- the scenario suite disables the round the same way"
    );
    let msgs = run_with(&off, store_reply("apply", "insert", serde_json::json!([])));
    assert_eq!(phases(&msgs), vec!["sup-scope"]);
    let judged = run_with(
        &off,
        judgement(r#"{"entities":[{"alias":"elvse","canonical":"elvese"}]}"#),
    );
    assert_eq!(
        phases(&judged),
        vec!["canon-alias", "canon-done", "llm-call"]
    );
}

/// [`emit`] against a patched copy of the script.
fn run_with(script: &str, doc: serde_json::Value) -> Vec<serde_json::Value> {
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
        "dream-glue exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("message array")
}
