//! GH #1225 (P1) -- every sidecar fact names its source and is checked there.
//!
//! Measured before this change (review of the curator, P1): the word-for-word
//! check of GH #1079 looked a quote up in the person rows of the CURRENT round
//! only. A round with no person row of its own -- the reasoning core's advice
//! coming back under a turn id of its own, a timer or an app starting a round
//! -- refused every fact of its answer (`quote_not_in_source`, `mode drop`).
//! And a fact that did get through was filed by `extract-glue` under the
//! newest person turn: a quote of the advice, or of the model's own answer,
//! became something the person said.
//!
//! The source is now named where it is known, by the model that read the
//! frames (`templates/memory-hive/schemas`, the `memory` offer): `person`,
//! `peer:<ref>` or `agent:<name>` (`self`, `cogny`, `voice`, `app:<id>`,
//! `timer:<id>`). The rule token `source` checks a quoted field against the
//! rows of exactly that source, a refusal leaves a `verify_reject` mark, and
//! `extract-glue` files the fact under the source it named -- never moved to
//! another one.
//!
//! Every script below is the REAL shipped `params.script_inline`: the talky
//! splitter, the curator hive of `support/curator_hive.rs`, the talky error
//! drain and the memory hive's ingress, run over stdin; no colony, no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const SPLITTER: &str = "templates/talky/splitter/config.json";
const ERRORS: &str = "templates/talky/errors/config.json";
const EXTRACT: &str = "templates/memory-hive/extract-glue/config.json";
const SCHEMAS: &str = "templates/memory-hive/schemas/config.json";

const PERSON_SAID: &str = "My sister Hannah moved to Porto last spring.";
const ADVICE: &str = "[advice from your reasoning core, consult c-1225]\nHannah lives near the river in Porto now; ask whether the new flat has a garden.";
const TIMER: &str = "[timer t-1225] Remind the person to phone Hannah about the garden.";

fn abs(rel: &str) -> String {
    repo(rel).to_string_lossy().to_string()
}

/// The rule a set turns on: the quote of every fact at the fact's own source.
fn knob() -> Value {
    json!({"memory": {"items": "facts", "quote": "source"}})
}

/// The curator of a talky: the memory section passes it (talky's override).
fn talky_curator() -> Hive {
    Hive::with(&[("intake", "pass_sections", json!("memory"))])
}

fn fact(claim: &str, quote: &str, source: Option<&str>) -> Value {
    let mut f = json!({"subject": "Hannah", "predicate": "lives_in", "claim": claim,
                       "quote": quote, "fact_kind": "world", "valid_from": null});
    if let Some(s) = source {
        f["source"] = json!(s);
    }
    f
}

/// The same rule with `mode` `mark`: a refused item stays in the section,
/// carrying `failed` (re-check KD-Q I-2).
fn marking_knob() -> Value {
    json!({"memory": {"items": "facts", "quote": "source", "mode": "mark"}})
}

/// The talky splitter over one answer that carries a `memory` section, the
/// knob set: the section message it hands its curator.
fn split_memory(prose: &str, facts: Value) -> Value {
    split_memory_under(prose, facts, knob())
}

fn split_memory_under(prose: &str, facts: Value, rule: Value) -> Value {
    let block = json!({"memory": {"facts": facts,
                                  "topic": {"movement": "continue", "name": "",
                                            "source": "person"}}});
    let doc = json!({
        "header": {"hop": {"finish_reason": "stop"},
                   "context": {"session_id": "s1", "turn_id": "t"}},
        "messages": [said(&format!("{prose}\n\n```sidecar\n{block}\n```"))],
        "params": {"sidecar_verify": rule, "id_sections": ["window", "gap", "memory"]},
    });
    let out = meclaw_testing::emit_all(&meclaw_testing::shipped_script(&abs(SPLITTER)), &doc);
    out.into_iter()
        .find(|m| m["header"]["section"] == json!("memory"))
        .expect("the splitter hands on the memory section")
}

/// The section on `in_section` as the talky's edge hands it: the splitter's
/// hop, the call's context with the person's episode of the round.
fn hand_in(h: &mut Hive, call: &Msg, split_msg: &Value) {
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert(
        "episode_turn_id".into(),
        call.hop
            .get("episode_turn_id")
            .cloned()
            .unwrap_or_else(|| json!("")),
    );
    let mut hop = split_msg["header"].clone();
    hop.as_object_mut().expect("hop").remove("route");
    let mut body = split_msg.clone();
    body.as_object_mut().expect("body").remove("header");
    h.out.clear();
    h.lane("in_section", Value::Object(ctx), hop, body);
}

/// The checked section: its facts, the reasons of its refusals in order, and
/// the `verify_reject` marks the ledger holds.
struct Checked {
    facts: Vec<Value>,
    payload: Value,
    reasons: Vec<String>,
    rejects: Vec<Msg>,
    marks: Vec<Value>,
}

fn check(h: &mut Hive, call: &Msg, prose: &str, facts: Value) -> Checked {
    check_under(h, call, prose, facts, knob())
}

fn check_under(h: &mut Hive, call: &Msg, prose: &str, facts: Value, rule: Value) -> Checked {
    let msg = split_memory_under(prose, facts, rule);
    assert!(
        msg["header"]["sidecar_verify"].is_string(),
        "the splitter stamps the source rule: {msg}"
    );
    hand_in(h, call, &msg);
    let sides = h.routed("sidecar");
    assert_eq!(sides.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let payload = sides[0].body["payload"].clone();
    let rejects = h.routed("reject");
    let reasons = rejects
        .iter()
        .map(|r| r.hop["reject_reason"].as_str().unwrap_or("").to_string())
        .collect();
    let marks = h
        .rows("SELECT value FROM marks WHERE kind = 'verify_reject' ORDER BY seq")
        .into_iter()
        .map(|r| sj::from_str(r[0].as_str().unwrap_or("null")).unwrap_or(Value::Null))
        .collect();
    Checked {
        facts: payload["facts"].as_array().cloned().unwrap_or_default(),
        payload,
        reasons,
        rejects,
        marks,
    }
}

// ------------------------------------------------------------ the memory side

const AUDIENCE: &str = "[\"member:e\"]";

fn args_of(msg: &Value) -> Value {
    let text = msg["messages"][0]["text"].as_str().unwrap_or("null");
    sj::from_str(text).unwrap_or(Value::Null)
}

fn step(phase: &str, op: &str, batch: &str, rows: Value) -> Vec<Value> {
    meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(&abs(EXTRACT)),
        &json!({
            "header": {"context": {"mem_phase": phase, "batch_id": batch, "session_id": "s1",
                                   "audience_set": AUDIENCE, "channel": "test"},
                       "hop": {"operation": op, "rows_affected": 1}},
            "messages": [{"origin": "tool", "type": "tool_result", "id": "r",
                          "text": rows.to_string()}]
        }),
    )
}

fn ep(id: &str, sender: &str, speaker: &str, at: &str) -> Value {
    json!({"id": id, "turn_id": format!("s1#{id}"), "session_id": "s1", "sender": sender,
           "speaker": speaker, "recorded_at": at, "happened_at": at, "channel": "test",
           "audience_set": AUDIENCE})
}

/// The person's turn, the answer to it and the answer of the agent round after
/// it: the episodes a round without a person row of its own binds against.
fn page() -> Value {
    json!([
        ep("ep-x2", "assistant", "agent:e", "2026-10-15T10:00:04Z"),
        ep("ep-x1", "assistant", "agent:e", "2026-10-15T10:00:02Z"),
        ep("ep-u1", "user", "member:e", "2026-10-15T10:00:01Z"),
    ])
}

/// The rows the memory store is asked to insert into `facts` for one checked
/// `memory` payload: the whole inline road of `extract-glue` -- park, bind,
/// meet, apply -- over `page`. Empty when nothing is filed; the receipts of
/// every step ride along.
fn file(payload: &Value, page: Value) -> (Vec<Value>, Vec<Value>) {
    let mut receipts = Vec::new();
    let parked_out = meclaw_testing::emit_all(
        &meclaw_testing::shipped_script(&abs(EXTRACT)),
        &json!({
            "header": {"context": {"mem_phase": "inline", "batch_id": "", "session_id": "s1",
                                   "audience_set": AUDIENCE, "channel": "test"},
                       "hop": {"route": "sidecar", "section": "memory"}},
            "section": "memory", "payload": payload, "messages": []
        }),
    );
    let Some(park) = parked_out
        .iter()
        .find(|m| m["header"]["phase"] == "inline-turn")
    else {
        return (vec![], parked_out);
    };
    let bid = park["header"]["batch_id"]
        .as_str()
        .expect("bid")
        .to_string();
    let parked = args_of(park)["row"].clone();
    let bind = step("inline-bind", "select", &bid, page.clone());
    let Some(next) = bind.iter().find(|m| m["header"]["phase"] == "inline-apply") else {
        return (vec![], bind);
    };
    let key = next["header"]["batch_id"]
        .as_str()
        .expect("key")
        .to_string();
    let meet = step("inline-apply", "select", &key, json!([parked]));
    receipts.extend(meet.iter().cloned());
    let Some(staged) = meet
        .iter()
        .map(args_of)
        .find(|a| a["table"] == "scratch" && a["row"]["kind"] == "payload")
        .map(|a| a["row"]["payload"].as_str().unwrap_or("{}").to_string())
    else {
        return (vec![], receipts);
    };
    let eps_out = step("known-eps", "select", "inline:b1", page);
    let eps = args_of(&eps_out[0])["row"]["payload"].clone();
    let rows = json!([
        {"key": "inline:b1", "kind": "payload", "payload": staged},
        {"key": "inline:b1", "kind": "known", "payload": "[]"},
        {"key": "inline:b1", "kind": "episodes", "payload": eps},
    ]);
    let apply = step("apply", "select", "inline:b1", rows);
    let facts = apply
        .iter()
        .map(args_of)
        .filter(|a| a["operation"] == "insert" && a["table"] == "facts")
        .map(|a| a["row"].clone())
        .collect();
    receipts.extend(apply);
    (facts, receipts)
}

fn filed(rows: &[Value]) -> Vec<(String, String, String)> {
    rows.iter()
        .map(|r| {
            (
                r["claim"].as_str().unwrap_or("").to_string(),
                r["source"].as_str().unwrap_or("").to_string(),
                r["episode_id"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// A round of the reasoning core's advice after the person's turn: no person
/// row of its own, the person's episode of the round on its call.
fn advice_round(h: &mut Hive) -> Msg {
    turn(
        h,
        "s1",
        "r1",
        PERSON_SAID,
        "That is a big change for her.",
        json!({}),
    );
    let call = h.curate("s1", "r2", 0, json!([user(ADVICE)]), mode("Be brief."));
    assert!(
        call.hop["episode_turn_id"]
            .as_str()
            .is_some_and(|e| !e.is_empty()),
        "the advice round names the person's episode: {:?}",
        call.hop
    );
    call
}

fn mark_named(marks: &[Value], reason: &str) -> Value {
    marks
        .iter()
        .find(|m| m["reason"] == reason)
        .cloned()
        .unwrap_or_else(|| panic!("no verify_reject mark with reason {reason}: {marks:?}"))
}

fn words_absent(all: &str, words: &[&str]) {
    for w in words {
        assert!(!all.contains(w), "`{w}` left in a refusal: {all}");
    }
}

// ------------------------------------------------------------ pin 1

#[test]
fn a_person_quote_in_an_agent_round_files_under_the_person() {
    if !shipped() {
        return;
    }
    let mut h = talky_curator();
    let call = advice_round(&mut h);
    let c = check(
        &mut h,
        &call,
        "Porto is lovely in spring.",
        json!([fact(
            "Porto",
            "Hannah moved to Porto last spring",
            Some("person")
        )]),
    );
    assert!(c.reasons.is_empty(), "{:?} {:?}", c.rejects, h.stderr);
    assert_eq!(c.facts.len(), 1, "{}", c.payload);
    assert_eq!(c.facts[0]["verified"], json!(["quote"]), "{}", c.payload);
    assert!(c.marks.is_empty(), "{:?}", c.marks);
    let (rows, receipts) = file(&c.payload, page());
    assert_eq!(
        filed(&rows),
        [("Porto".into(), String::new(), "ep-u1".into())],
        "a person's fact is the person's side, on the person's turn: {receipts:?}"
    );
}

// ------------------------------------------------------------ pin 2

#[test]
fn an_agent_quote_files_under_the_agent_never_the_person() {
    if !shipped() {
        return;
    }
    let mut h = talky_curator();
    let call = advice_round(&mut h);
    let c = check(
        &mut h,
        &call,
        "Does the new flat have a garden?",
        json!([fact(
            "a flat near the river",
            "lives near the river in Porto now",
            Some("agent:cogny")
        )]),
    );
    assert!(c.reasons.is_empty(), "{:?} {:?}", c.rejects, h.stderr);
    assert_eq!(c.facts.len(), 1, "{}", c.payload);
    let (rows, receipts) = file(&c.payload, page());
    assert_eq!(
        filed(&rows),
        [(
            "a flat near the river".into(),
            "agent:cogny".into(),
            "ep-u1".into()
        )],
        "{receipts:?}"
    );
    assert_eq!(
        rows[0]["subject"], "Hannah",
        "an agent's fact is never filed as a peer's self-statement: {:?}",
        rows[0]
    );

    // The same words claimed for the person: the advice is nobody's words.
    let mut h = talky_curator();
    let call = advice_round(&mut h);
    let c = check(
        &mut h,
        &call,
        "Does the new flat have a garden?",
        json!([fact(
            "a flat near the river",
            "lives near the river in Porto now",
            Some("person")
        )]),
    );
    assert_eq!(c.reasons, ["quote_not_in_source"], "{:?}", h.out);
    assert!(c.facts.is_empty(), "dropped, mode drop: {}", c.payload);
    let m = mark_named(&c.marks, "quote_not_in_source");
    assert_eq!(
        (&m["section"], &m["field"], &m["source"]),
        (&json!("memory"), &json!("quote"), &json!("person"))
    );
    words_absent(&format!("{:?}", c.marks), &["river", "Porto"]);
    let (rows, _) = file(&c.payload, page());
    assert!(rows.is_empty(), "{rows:?}");
}

// ------------------------------------------------------------ pin 3

#[test]
fn a_wrong_or_missing_source_leaves_a_mark() {
    if !shipped() {
        return;
    }
    let mut h = talky_curator();
    let call = advice_round(&mut h);
    let c = check(
        &mut h,
        &call,
        "How exciting for her.",
        json!([
            fact(
                "Porto",
                "Hannah moved to Porto last spring",
                Some("agent:cogny")
            ),
            fact("Porto", "Hannah moved to Porto last spring", None),
            fact(
                "Porto",
                "Hannah moved to Porto last spring",
                Some("the neighbour")
            )
        ]),
    );
    let mut reasons = c.reasons.clone();
    reasons.sort();
    assert_eq!(
        reasons,
        ["quote_not_in_source", "source_missing", "source_unknown"],
        "{:?}",
        h.out
    );
    assert!(
        c.facts.is_empty(),
        "no fact of a wrong source passes: {}",
        c.payload
    );
    assert_eq!(
        c.marks.len(),
        3,
        "one mark per refusal, never silent: {:?}",
        c.marks
    );
    for reason in ["quote_not_in_source", "source_missing", "source_unknown"] {
        let m = mark_named(&c.marks, reason);
        assert_eq!(
            (&m["section"], &m["field"]),
            (&json!("memory"), &json!("quote")),
            "{m}"
        );
    }
    assert_eq!(
        mark_named(&c.marks, "quote_not_in_source")["source"],
        "agent:cogny"
    );
    words_absent(&format!("{:?}", c.marks), &["Hannah", "Porto", "neighbour"]);
    // Each refusal reaches the talky's error lane, as the #1079 refusals do.
    for r in &c.rejects {
        let report = meclaw_testing::emit_all(
            &meclaw_testing::shipped_script(&abs(ERRORS)),
            &json!({"header": {"hop": r.hop, "context": r.context}, "messages": []}),
        );
        assert_eq!(report.len(), 1, "{report:?}");
        let head = &report[0]["header"];
        assert_eq!(head["error_source"], "sidecar_verify", "{report:?}");
        assert_eq!(head["error_code"], r.hop["reject_reason"]);
        assert_eq!(head["section"], "memory");
        words_absent(&report[0].to_string(), &["Hannah", "Porto", "neighbour"]);
    }
    let (rows, _) = file(&c.payload, page());
    assert!(rows.is_empty(), "{rows:?}");
}

// ------------------------------------------------------------ pin 4

#[test]
fn a_timer_round_checks_a_person_quote_against_the_episode() {
    if !shipped() {
        return;
    }
    let mut h = talky_curator();
    turn(
        &mut h,
        "s1",
        "r1",
        PERSON_SAID,
        "That is a big change for her.",
        json!({}),
    );
    let call = h.curate("s1", "r2", 0, json!([user(TIMER)]), mode("Be brief."));
    let c = check(
        &mut h,
        &call,
        "Time to phone Hannah.",
        json!([
            fact("Porto", "Hannah moved to Porto last spring", Some("person")),
            fact(
                "a phone call",
                "phone Hannah about the garden",
                Some("agent:timer:t-1225")
            ),
            fact(
                "a phone call",
                "phone Hannah about the garden",
                Some("person")
            ),
            fact("Lisbon", "she bought a boat in Lisbon", Some("person"))
        ]),
    );
    assert_eq!(
        c.reasons,
        ["quote_not_in_source", "quote_not_in_source"],
        "the timer's words are not the person's, and a quote said nowhere is refused: {:?}",
        h.out
    );
    assert_eq!(c.marks.len(), 2, "{:?}", c.marks);
    let (rows, receipts) = file(&c.payload, page());
    assert_eq!(
        filed(&rows),
        [
            ("Porto".into(), String::new(), "ep-u1".into()),
            (
                "a phone call".into(),
                "agent:timer:t-1225".into(),
                "ep-u1".into()
            )
        ],
        "{receipts:?}"
    );
}

// ------------------------------------------------------------ pin 6

#[test]
fn the_models_own_words_file_under_agent_self() {
    if !shipped() {
        return;
    }
    let answer = "I will call you tomorrow.";
    for (source, filed_as) in [("agent:self", Some("agent:self")), ("person", None)] {
        let mut h = talky_curator();
        let call = h.curate(
            "s1",
            "r1",
            0,
            json!([user("Will you remind me of the dentist?")]),
            mode("Be brief."),
        );
        let mut f = fact("calls tomorrow", "I will call you tomorrow", Some(source));
        f["subject"] = json!("agent");
        f["predicate"] = json!("calls");
        f["fact_kind"] = json!("foresight");
        let c = check(&mut h, &call, answer, json!([f]));
        let page = json!([
            ep("ep-x1", "assistant", "agent:e", "2026-10-15T10:00:02Z"),
            ep("ep-u1", "user", "member:e", "2026-10-15T10:00:01Z"),
        ]);
        let (rows, receipts) = file(&c.payload, page);
        match filed_as {
            Some(s) => {
                assert!(c.reasons.is_empty(), "{:?} {:?}", c.rejects, h.stderr);
                assert_eq!(
                    filed(&rows),
                    [("calls tomorrow".into(), s.into(), "ep-u1".into())],
                    "{receipts:?}"
                );
            }
            None => {
                assert_eq!(c.reasons, ["quote_not_in_source"], "{:?}", h.out);
                assert_eq!(
                    mark_named(&c.marks, "quote_not_in_source")["source"],
                    "person"
                );
                assert!(
                    rows.is_empty(),
                    "the model's promise is never the person's: {rows:?}"
                );
            }
        }
    }
}

// ------------------------------------------------------------ pin 7

/// Re-check KD-Q I-2: under `mode` `mark` the check keeps a refused item in
/// its section, carrying `failed` -- and the memory files it under no source.
/// The advice's words labelled `person` never become the person's, the
/// person's words labelled `agent:cogny` never the advice; the item the check
/// verified beside them is filed as before, and the refusal is receipted.
#[test]
fn a_kept_refusal_is_filed_under_no_source() {
    if !shipped() {
        return;
    }
    let mut h = talky_curator();
    let call = advice_round(&mut h);
    let c = check_under(
        &mut h,
        &call,
        "Does the new flat have a garden?",
        json!([
            fact(
                "a flat near the river",
                "lives near the river in Porto now",
                Some("person")
            ),
            fact(
                "Porto since spring",
                "Hannah moved to Porto last spring",
                Some("agent:cogny")
            ),
            fact("Porto", "Hannah moved to Porto last spring", Some("person"))
        ]),
        marking_knob(),
    );
    assert_eq!(
        c.reasons,
        ["quote_not_in_source", "quote_not_in_source"],
        "{:?}",
        h.out
    );
    assert_eq!(c.facts.len(), 3, "kept, mode mark: {}", c.payload);
    let kept_refused = c
        .facts
        .iter()
        .filter(|f| f["failed"].as_array().is_some_and(|a| !a.is_empty()))
        .count();
    assert_eq!(kept_refused, 2, "{}", c.payload);
    let (rows, receipts) = file(&c.payload, page());
    assert_eq!(
        filed(&rows),
        [("Porto".into(), String::new(), "ep-u1".into())],
        "only the verified item is filed, a refused one under no source: {receipts:?}"
    );
    let refused: Vec<&Value> = receipts
        .iter()
        .filter(|m| m["header"]["reject_reason"] == "verify_failed")
        .collect();
    assert_eq!(
        refused.len(),
        1,
        "one receipt for the batch, never silent: {receipts:?}"
    );
    let text = refused[0]["messages"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("dropped 2 fact(s)"), "{text}");
    words_absent(text, &["river", "Porto", "Hannah"]);
}

// ------------------------------------------------------------ pin 8

/// Re-check KD-Q M-2: an app or timer id the check reads (`source_form`: 1 to
/// 64 of a-z 0-9 _ . -) is one the memory files under -- the longest one too.
/// Verified by the curator and then dropped as `unknown_source` was the gap.
#[test]
fn the_longest_app_and_timer_ids_the_check_reads_are_filed() {
    if !shipped() {
        return;
    }
    let id = "a".repeat(64);
    for kind in ["app", "timer"] {
        let mut h = talky_curator();
        turn(
            &mut h,
            "s1",
            "r1",
            PERSON_SAID,
            "That is a big change for her.",
            json!({}),
        );
        let opened = format!("[{kind} {id}] Remind the person to phone Hannah about the garden.");
        let call = h.curate("s1", "r2", 0, json!([user(&opened)]), mode("Be brief."));
        let source = format!("agent:{kind}:{id}");
        let c = check(
            &mut h,
            &call,
            "Time to phone Hannah.",
            json!([fact(
                "a phone call",
                "phone Hannah about the garden",
                Some(source.as_str())
            )]),
        );
        assert!(
            c.reasons.is_empty(),
            "{kind}: {:?} {:?}",
            c.rejects,
            h.stderr
        );
        assert_eq!(c.facts.len(), 1, "{kind}: {}", c.payload);
        let (rows, receipts) = file(&c.payload, page());
        assert_eq!(
            filed(&rows),
            [("a phone call".into(), source.clone(), "ep-u1".into())],
            "{kind}: {receipts:?}"
        );
    }
}

/// Re-check KD-Q M-3: an agent's fact held for want of a turn to hang on has
/// a source, and its receipt says so -- never "without a `source`".
#[test]
fn a_held_agent_fact_is_receipted_as_an_agents() {
    if !shipped() {
        return;
    }
    let block = json!({"facts": [fact("Porto", "q one two three", Some("agent:cogny"))]});
    let parked = json!({"key": "inline-named:b7", "kind": "inline",
                        "payload": block.to_string()});
    // The meeting read of a named block whose page vouched for no turn: no
    // answered turn, no reference, no turn it may answer.
    let out = step(
        "inline-apply",
        "select",
        "inline-named:b7||",
        json!([parked]),
    );
    let held: Vec<&Value> = out
        .iter()
        .filter(|m| m["header"]["reject_reason"] == "ambiguous_speaker")
        .collect();
    assert_eq!(held.len(), 1, "{out:?}");
    let text = held[0]["messages"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("1 item(s) naming an agent"), "{text}");
    assert!(!text.contains("without a `source`"), "{text}");
}

// ------------------------------------------------------------ the parts of Q

/// The splitter: `source` is a known source, and a rule that names it carries
/// the answer -- the source of `agent:self` -- beside the section.
#[test]
fn a_source_rule_is_stamped_with_the_answer() {
    if !shipped() {
        return;
    }
    let msg = split_memory(
        "I will call you [#0123456789ab] tomorrow.",
        json!([fact("x", "call you tomorrow", Some("agent:self"))]),
    );
    assert_eq!(
        msg["header"]["sidecar_verify"],
        json!(r#"{"items":"facts","quote":"source"}"#)
    );
    assert_eq!(msg["verify_answer"], json!("I will call you tomorrow."));
    let cfg = read_json(&repo(SPLITTER));
    let described = cfg["contract"]["settings"]["sidecar_verify"].to_string();
    assert!(
        described.contains("`source`"),
        "the knob's contract names the token: {described}"
    );
    assert_eq!(
        cfg["params"]["script_inline"],
        read_json(&repo("templates/cogny/splitter/config.json"))["params"]["script_inline"],
        "cogny's splitter is talky's, byte for byte"
    );
}

/// The offer asks every fact and the topic for its source, and says how a
/// model tells the sources apart in its window.
#[test]
fn the_memory_offer_asks_each_fact_and_the_topic_for_its_source() {
    let offer = meclaw_testing::emit_one(
        &meclaw_testing::shipped_script(&abs(SCHEMAS)),
        &json!({"tools": ["*"], "messages": []}),
    );
    let section = &offer["sidecar"][0];
    let schema = &section["schema"]["properties"];
    for (what, obj) in [
        ("facts", &schema["facts"]["items"]),
        ("topic", &schema["topic"]),
    ] {
        assert_eq!(
            obj["properties"]["source"]["type"], "string",
            "{what}: {obj}"
        );
        assert!(
            obj["required"]
                .as_array()
                .expect("required")
                .iter()
                .any(|k| k == "source"),
            "{what}: shown in the example a model copies: {obj}"
        );
    }
    let rules = section["instruction"].as_str().expect("instruction");
    for form in [
        "`person`",
        "`peer:<ref>`",
        "[peer <ref> · <name>]",
        "`agent:cogny`",
        "[advice from your reasoning core",
        "`agent:voice`",
        "[delegation",
        "`agent:app:<id>`",
        "`agent:timer:<id>`",
        "`agent:self`",
    ] {
        assert!(rules.contains(form), "the rules name {form}: {rules}");
    }
}

/// `extract-glue` files a named fact under exactly the source it names; an
/// item without one walks the old road byte for byte.
#[test]
fn a_named_source_sets_the_fact_column_and_nothing_else() {
    if !shipped() {
        return;
    }
    let peer_page = json!([
        ep("ep-x2", "assistant", "agent:e", "2026-10-15T10:00:04Z"),
        ep("ep-p1", "peer", "3a47fe3e", "2026-10-15T10:00:03Z"),
        ep("ep-u1", "user", "member:e", "2026-10-15T10:00:01Z"),
    ]);
    let payload = json!({"facts": [
        fact("person fact", "q one two three", Some("PERSON")),
        fact("peer fact", "q one two three", Some(" peer:3A47FE3E ")),
        fact("legacy peer fact", "q one two three", Some("3a47fe3e")),
        fact("advice fact", "q one two three", Some("Agent:Cogny")),
        fact("delegation fact", "q one two three", Some("agent:voice")),
        fact("nobody", "q one two three", Some("agent:")),
        fact("nobody either", "q one two three", Some("member:e"))
    ], "topic": {"movement": "continue", "name": "", "source": "person"}});
    let (rows, receipts) = file(&payload, peer_page);
    let mut got = filed(&rows);
    got.sort();
    assert_eq!(
        got,
        [
            ("advice fact".into(), "agent:cogny".into(), "ep-p1".into()),
            (
                "delegation fact".into(),
                "agent:voice".into(),
                "ep-p1".into()
            ),
            ("legacy peer fact".into(), "3a47fe3e".into(), "ep-p1".into()),
            ("peer fact".into(), "3a47fe3e".into(), "ep-p1".into()),
            ("person fact".into(), String::new(), "ep-u1".into()),
        ],
        "{receipts:?}"
    );
    for r in &rows {
        if r["source"].as_str().unwrap_or("").starts_with("agent:") {
            assert_eq!(r["subject"], "Hannah", "{r}");
        }
    }
    let unknown: Vec<&Value> = receipts
        .iter()
        .filter(|m| m["header"]["reject_reason"] == "unknown_source")
        .collect();
    assert_eq!(
        unknown.len(),
        1,
        "the two names nobody answers to, receipted: {receipts:?}"
    );
    assert!(
        unknown[0]["messages"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("dropped 2 fact(s)"),
        "{unknown:?}"
    );
}
