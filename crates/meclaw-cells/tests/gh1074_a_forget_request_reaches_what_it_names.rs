//! GH #1074 -- a forget request is honoured even when the closer skips it, and
//! the fact a forgotten turn left behind is forgotten with it.
//!
//! Measured after #1039 (a six-month synthetic boundary run, 10 forget probes,
//! all 10 answered with the forgotten value):
//! - the closer emitted `forget` for 3 of 10 member turns that plainly asked
//!   ("Please forget what I told you about ..."); the other 7 got
//!   `"forget": []` and nothing was marked;
//! - in the 3 honoured requests both episodes were marked, but the fact the
//!   secret left stayed open and carried the value into the bundle: the fact
//!   search filtered `recorded_at <= request.happened_at`, two different clocks
//!   (write time against the turn's own time), and the secret sat in an
//!   earlier session, so the closer had no id of it to cite.
//!
//! The locks, with invented people only:
//! - a member turn whose own sentence asks to forget is a request without the
//!   closer, the words come from that sentence; a statement ("I always forget
//!   ..."), a peer, the agent and a bare "forget it" ask nothing;
//! - a request the closer cited is searched once, never twice;
//! - the facts extracted from a forgotten episode carry the mark by provenance,
//!   and so do the beliefs made of them;
//! - the fact search window is the fact's own time (`valid_from`).
//!
//! Review K2-R8 (the shipped functions run over each sentence below):
//! - C1: a negated German imperative is a reminder, it marked exactly what the member
//!   wanted kept; a request is a sentence of its own, a small grammar (address,
//!   polite frame, imperative, the thing it names) and never a prefix anywhere
//!   in the sentence (I1: "I just forget my pills", "Will you forget ...?");
//! - I2: the thing is named after the verb -- "Could you", "Hey <name>," and
//!   the frame of the telling are no part of an AND search;
//! - I3: one word is a request where the sentence says it was told, bounded
//!   by eight rows;
//! - I4: only the speaker's own facts, and of the request's own turn only the
//!   facts that carry a word of it;
//! - M1: a later turn of the same session is never reached; a full provenance
//!   page leaves a journal line.
//!
//! Deep review of kf89 (D1, N1-N3):
//! - D1: the correction idiom ("forget what I said, <the new value>") is no
//!   request to forget the new value -- a continuation clause ends the thing,
//!   `forget-o` needs every word, and a continued request reaches neither its
//!   own turn's facts nor its own episode;
//! - N1: "that about X" is a telling, one word is enough;
//! - N2: the report counts a request where it marked rows; each search answer
//!   reports itself (marked, too broad, nothing found);
//! - N3: the episode search reaches the own side only, never a peer's turn.
//!
//! The sentences are language data: they live in `fixtures/forget_<code>.jsonl`
//! (one file per language), each case naming the group of the lock it belongs
//! to. The grammar itself is the cell's `params.forget_lang`.

#[path = "support/memory_hive_subject.rs"]
mod support;

use meclaw_core::serde_json::{self, Value, json};
use meclaw_testing::code_wire::{code_stdin, run_shipped_script};
use meclaw_testing::{emit_all, shipped_script};
use support::*;

const CLOSE: &str = "../../templates/memory-hive/close-glue/config.json";
const AUDIENCE: &str = r#"["agent:a","member:e"]"#;
const FORGET_EN: &str = include_str!("fixtures/forget_en.jsonl");
const FORGET_DE: &str = include_str!("fixtures/forget_de.jsonl");

/// The fixture cases of one lock group, every language.
fn cases(test: &str) -> Vec<Value> {
    let found: Vec<Value> = FORGET_EN
        .lines()
        .chain(FORGET_DE.lines())
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).expect("fixture line"))
        .filter(|c| c["test"] == test)
        .collect();
    assert!(!found.is_empty(), "no fixture case for {test}");
    found
}

fn text_of(c: &Value) -> String {
    c["text"].as_str().expect("text").to_string()
}

/// The cell's own params: the script reads the grammar of a request from them
/// (`forget_lang`), exactly as the substrate hands them over.
fn params() -> Value {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), CLOSE);
    let cfg: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("config")).expect("json");
    cfg["params"].clone()
}

fn tool_calls(out: &[Value]) -> Vec<(String, Value)> {
    let mut found = Vec::new();
    for m in out {
        let phase = m["header"]["phase"]
            .as_str()
            .or_else(|| m["header"]["route"].as_str())
            .unwrap_or_default()
            .to_string();
        for x in m["messages"].as_array().into_iter().flatten() {
            if x["type"] == "tool_call"
                && let Some(op) = x["text"]
                    .as_str()
                    .and_then(|t| serde_json::from_str::<Value>(t).ok())
            {
                found.push((phase.clone(), op));
            }
        }
    }
    found
}

fn parked(kind: &str, rows: Value) -> Value {
    json!({"key": "close:s-2", "kind": kind, "created_at": "2026-01-01T00:00:00Z",
           "payload": json!({"rows": rows, "truncated": false}).to_string()})
}

fn turn(id: &str, sender: &str, speaker: &str, text: &str, at: &str) -> Value {
    json!({"id": id, "session_id": "s-2", "sender": sender, "speaker": speaker,
           "content": text, "happened_at": at, "audience_set": AUDIENCE,
           "channel": "direct:e"})
}

/// A later session: the secret was told days before, so this round shows no
/// record of it -- only the request.
fn later_round() -> Vec<Value> {
    vec![
        turn(
            "r1",
            "user",
            "member:e",
            "Please forget what I told you about the money my neighbour owes me. \
             I do not want you to keep that.",
            "2026-02-10T09:00:00Z",
        ),
        // a statement, not a request
        turn(
            "r2",
            "user",
            "member:e",
            "I always forget where I put my reading glasses.",
            "2026-02-10T09:01:00Z",
        ),
        // never mind, nothing named
        turn(
            "r3",
            "user",
            "member:e",
            "Forget it, the bus is late again.",
            "2026-02-10T09:02:00Z",
        ),
        // only the member forgets: never a peer, never the agent
        turn(
            "r4",
            "peer",
            "b77e01c2",
            "Please forget what I told you about the garden shed key.",
            "2026-02-10T09:03:00Z",
        ),
        turn(
            "r5",
            "assistant",
            "agent:a",
            "Of course, I will forget what you told me about the money your neighbour owes you.",
            "2026-02-10T09:04:00Z",
        ),
        // the same rule in German
        turn(
            "r6",
            "user",
            "member:e",
            &text_of(&cases("later-round")[0]),
            "2026-02-10T09:05:00Z",
        ),
    ]
}

fn apply_round(verdict: Value, turns: Vec<Value>) -> Vec<Value> {
    apply_round_with(verdict, turns, json!([]))
}

/// The params with the fallback switched on. It ships off (R-TR-25: until the
/// mute semantics replace forgetting), and every `apply_round` case pins what
/// it does where it runs, so each of them sets the switch itself;
/// `the_fallback_is_off_by_default` reads the shipped params.
fn fallback_params() -> Value {
    let mut p = params();
    p["forget_fallback"] = json!(true);
    p
}

fn apply_round_with(verdict: Value, turns: Vec<Value>, facts: Value) -> Vec<Value> {
    apply_round_params(verdict, turns, facts, fallback_params())
}

fn apply_round_params(
    verdict: Value,
    turns: Vec<Value>,
    facts: Value,
    params: Value,
) -> Vec<Value> {
    let mut v = verdict;
    v["close_group"] = json!(0);
    let rows = json!([
        parked("turns", Value::Array(turns)), parked("facts", facts),
        parked("topics", json!([])), parked("exceptions", json!([])),
        {"key": "close:s-2", "kind": "verdict", "created_at": "2026-01-01T00:00:00Z",
         "payload": v.to_string()}
    ]);
    emit_all(
        &shipped_script(CLOSE),
        &json!({
            "params": params,
            "header": {"context": {"mem_phase": "close-apply", "session_id": "s-2",
                                   "close_group": "0"},
                       "hop": {"operation": "select"}},
            "messages": [{"origin": "tool", "type": "tool_result", "text": rows.to_string()}]
        }),
    )
}

fn searches(out: &[Value]) -> Vec<(String, Value)> {
    tool_calls(out)
        .into_iter()
        .filter(|(_, o)| o["operation"] == "search")
        .collect()
}

fn report(out: &[Value]) -> Value {
    out.iter()
        .find(|m| m["header"]["route"] == "close_report")
        .expect("a report")["header"]
        .clone()
}

fn back(phase: &str, op: &str, rows: Value) -> Vec<Value> {
    emit_all(
        &shipped_script(CLOSE),
        &json!({
            "params": params(),
            "header": {"context": {"mem_phase": phase, "session_id": "s-2"},
                       "hop": {"operation": op}},
            "messages": [{"origin": "tool", "type": "tool_result", "text": rows.to_string()}]
        }),
    )
}

#[test]
fn a_member_request_the_closer_skipped_is_still_honoured() {
    let out = apply_round(json!({"nothing_to_add": true, "forget": []}), later_round());
    let found = searches(&out);
    let mut phases: Vec<&str> = found.iter().map(|(p, _)| p.as_str()).collect();
    phases.sort();
    assert_eq!(
        phases,
        vec![
            "forget-e|r1",
            "forget-e|r6",
            "forget-f|r1",
            "forget-f|r6",
            "forget-o|r1",
            "forget-o|r6",
            "forget-s|r1",
            "forget-s|r6"
        ],
        "the two member requests and nothing else: {found:?}"
    );
    let ep = |id: &str| {
        found
            .iter()
            .find(|(p, _)| p == &format!("forget-e|{id}"))
            .expect("episodes")
            .1
            .clone()
    };
    // the words of the asking sentence, never the second one, never filler
    assert_eq!(
        ep("r1")["match"],
        "\"money\" AND \"neighbour\" AND \"owes\""
    );
    assert_eq!(ep("r6")["match"], "\"schulden\" AND \"bank\"");
    assert_eq!(
        ep("r1")["where"]["happened_at"],
        json!({"lte": "2026-02-10T09:00:00Z"})
    );
    assert_eq!(ep("r1")["limit"], 64);
    // review N2: searched, not yet forgotten -- the answers report the marks
    assert_eq!(report(&out)["forgotten"], 0);
    assert_eq!(report(&out)["forget_searched"], 2);
    assert_the_declaration_admits(CLOSE, &out);
}

/// R-TR-25: the shipped switch is off, so the two member requests the case
/// above searches are left alone where the closer cites neither -- with the
/// key missing, null or blanked alike -- and a request the closer cites is
/// still honoured.
#[test]
fn the_fallback_is_off_by_default() {
    let skipped = json!({"nothing_to_add": true, "forget": []});
    let mut missing = params();
    missing
        .as_object_mut()
        .expect("params")
        .remove("forget_fallback");
    let mut null = params();
    null["forget_fallback"] = Value::Null;
    let mut blank = params();
    blank["forget_fallback"] = json!(" ");
    for (name, p) in [
        ("shipped", params()),
        ("missing", missing),
        ("null", null),
        ("blank", blank),
    ] {
        let out = apply_round_params(skipped.clone(), later_round(), json!([]), p);
        assert!(
            forget_calls(&out).is_empty(),
            "{name}: an uncited request marks nothing: {:?}",
            forget_calls(&out)
        );
        assert_eq!(report(&out)["forgotten"], 0, "{name}");
        assert_eq!(report(&out)["forget_searched"], 0, "{name}");
    }
    assert_eq!(
        params()["forget_fallback"],
        json!(false),
        "the switch ships off"
    );

    let cited = apply_round_params(
        json!({"forget": [{"episode_id": "r1", "words": "neighbour owes money",
                           "fact_ids": []}]}),
        later_round(),
        json!([]),
        params(),
    );
    let mut phases: Vec<String> = searches(&cited).into_iter().map(|(p, _)| p).collect();
    phases.sort();
    assert_eq!(
        phases,
        vec!["forget-e|r1", "forget-f|r1", "forget-o|r1", "forget-s|r1"],
        "the cited request alone, never the uncited r6"
    );
    assert_eq!(report(&cited)["forget_searched"], 1);
}

#[test]
fn a_request_the_closer_cited_is_searched_once() {
    let out = apply_round(
        json!({"forget": [{"episode_id": "r1", "words": "neighbour owes money",
                           "fact_ids": []}]}),
        later_round()[..1].to_vec(),
    );
    let found = searches(&out);
    assert_eq!(found.len(), 4, "{found:?}");
    let ep = &found
        .iter()
        .find(|(p, _)| p == "forget-e|r1")
        .expect("episodes")
        .1;
    assert_eq!(
        ep["match"], "\"neighbour\" AND \"owes\" AND \"money\"",
        "the closer's words win where it gave them"
    );
    assert_eq!(report(&out)["forgotten"], 0);
    assert_eq!(report(&out)["forget_searched"], 1);
}

#[test]
fn the_fact_search_window_is_the_facts_own_time() {
    let out = apply_round(json!({"forget": []}), later_round()[..1].to_vec());
    let facts = &searches(&out)
        .into_iter()
        .find(|(p, _)| p == "forget-f|r1")
        .expect("facts")
        .1;
    assert_eq!(
        facts["match"],
        "claim : \"money\" AND claim : \"neighbour\" AND claim : \"owes\""
    );
    // `recorded_at` is the write clock; a fact extracted later from an older turn,
    // or an imported history, is never `<=` the request's own time.
    assert_eq!(
        facts["where"]["valid_from"],
        json!({"lte": "2026-02-10T09:00:00Z"})
    );
    assert!(
        facts["where"].get("recorded_at").is_none(),
        "no write-clock window: {facts}"
    );
    // review K2-R8: the speaker's own facts, other sessions here, this one in
    // `forget-s` up to the request
    assert_eq!(facts["where"]["source"], json!({"or_null": {"eq": ""}}));
    assert_eq!(
        facts["where"]["session_id"],
        json!({"or_null": {"neq": "s-2"}})
    );
}

#[test]
fn the_facts_of_a_forgotten_episode_carry_the_mark() {
    // The episodes the search found come back: they are marked, and the facts
    // extracted from them are looked up by provenance.
    let out = back(
        "forget-e|r1",
        "search",
        json!([{"id": "e-secret", "rank": -1.0}, {"id": "r1", "rank": -0.5}]),
    );
    let calls = tool_calls(&out);
    assert_eq!(calls[0].1["operation"], "update");
    assert_eq!(calls[0].1["table"], "episodes");
    let lookup = calls
        .iter()
        .find(|(_, o)| o["operation"] == "select" && o["table"] == "facts")
        .unwrap_or_else(|| panic!("no provenance lookup: {calls:?}"));
    assert_eq!(lookup.0, "forget-p|r1");
    // review K2-R8 I4: never by provenance from the request's own turn, and
    // never a fact another speaker stated
    assert_eq!(lookup.1["where"]["episode_id"], json!({"in": ["e-secret"]}));
    assert_eq!(lookup.1["where"]["source"], json!({"or_null": {"eq": ""}}));
    assert_eq!(
        lookup.1["where"]["closure_source"],
        json!({"or_null": {"eq": ""}})
    );
    assert_the_declaration_admits(CLOSE, &out);

    // The facts come back: marked like every forgotten row, never deleted, and
    // the beliefs made of them carry the same mark.
    let marked = back("forget-p|r1", "select", json!([{"id": "f-owes"}]));
    let calls = tool_calls(&marked);
    assert!(!calls.iter().any(|(_, o)| o["operation"] == "delete"));
    let fact = calls
        .iter()
        .find(|(_, o)| o["operation"] == "update" && o["table"] == "facts")
        .unwrap_or_else(|| panic!("no fact mark: {calls:?}"));
    assert_eq!(fact.1["where"]["id"], json!({"in": ["f-owes"]}));
    assert_eq!(fact.1["set"]["closure_source"], "forget_request:r1");
    assert!(fact.1["set"]["expired_at"].is_string());
    let belief = calls
        .iter()
        .find(|(_, o)| o["table"] == "beliefs")
        .unwrap_or_else(|| panic!("no belief mark: {calls:?}"));
    assert_eq!(
        belief.1["where"]["source_fact_ids"],
        json!({"covers": ["f-owes"]})
    );
    assert_the_declaration_admits(CLOSE, &marked);

    // Nothing extracted from them: nothing to mark.
    assert!(tool_calls(&back("forget-p|r1", "select", json!([]))).is_empty());
}

// ───────────────────────────────────────────────── review K2-R8: the locks

/// One member turn, nothing named by the closer.
fn one(text: &str) -> Vec<Value> {
    apply_round(
        json!({"nothing_to_add": true, "forget": []}),
        vec![turn("r1", "user", "member:e", text, "2026-02-10T09:00:00Z")],
    )
}

/// The episode search of request `r1`: its phase, its match and its limit.
fn episode_search(out: &[Value]) -> Option<(String, Value, Value)> {
    searches(out)
        .into_iter()
        .find(|(p, _)| p.starts_with("forget-e|r1"))
        .map(|(p, o)| (p, o["match"].clone(), o["limit"].clone()))
}

fn forget_calls(out: &[Value]) -> Vec<(String, Value)> {
    tool_calls(out)
        .into_iter()
        .filter(|(p, _)| p.starts_with("forget"))
        .collect()
}

/// A reply handed back, with the journal (stderr) the script wrote.
fn back_with_journal(phase: &str, op: &str, rows: Value) -> (Vec<Value>, String) {
    let flat = json!({
        "params": params(),
        "header": {"context": {"mem_phase": phase, "session_id": "s-2"},
                   "hop": {"operation": op}},
        "messages": [{"origin": "tool", "type": "tool_result", "text": rows.to_string()}]
    });
    let run = run_shipped_script(&shipped_script(CLOSE), &code_stdin(&flat).to_string());
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let out = match serde_json::from_slice::<Value>(&run.stdout).expect("json") {
        Value::Array(a) => a,
        other => vec![other],
    };
    (out, String::from_utf8_lossy(&run.stderr).into_owned())
}

#[test]
fn the_four_counter_examples_of_the_review_mark_nothing() {
    let turns = cases("counter")
        .iter()
        .enumerate()
        .map(|(i, c)| {
            turn(
                &format!("r{i}"),
                "user",
                "member:e",
                &text_of(c),
                &format!("2026-02-10T09:0{i}:00Z"),
            )
        })
        .collect();
    let out = apply_round(json!({"nothing_to_add": true, "forget": []}), turns);
    assert!(forget_calls(&out).is_empty(), "{:?}", forget_calls(&out));
    assert_eq!(report(&out)["forgotten"], 0);
    assert_eq!(report(&out)["forget_searched"], 0);
}

#[test]
fn a_reminder_asks_for_nothing() {
    for c in cases("reminder") {
        let text = text_of(&c);
        let out = one(&text);
        assert!(
            forget_calls(&out).is_empty(),
            "{text}: {:?}",
            forget_calls(&out)
        );
        // a closer that cites a reminder is not taken at its word
        if let Some(words) = c["words"].as_str() {
            let out = apply_round(
                json!({"forget": [{"episode_id": "r1", "words": words, "fact_ids": []}]}),
                vec![turn(
                    "r1",
                    "user",
                    "member:e",
                    &text,
                    "2026-02-10T09:00:00Z",
                )],
            );
            assert!(forget_calls(&out).is_empty(), "{:?}", forget_calls(&out));
        }
    }
}

/// A request read from the member's own sentence names its thing: the episode
/// search carries exactly the fixture's `want`, bounded by the full page.
#[test]
fn the_asking_words_open_the_sentence_and_name_the_thing() {
    for c in cases("statement") {
        let text = text_of(&c);
        let out = one(&text);
        assert!(
            forget_calls(&out).is_empty(),
            "{text}: {:?}",
            forget_calls(&out)
        );
    }
    for c in cases("named") {
        let text = text_of(&c);
        let (phase, m, limit) = episode_search(&one(&text)).unwrap_or_else(|| panic!("{text}"));
        assert_eq!(m, c["want"], "{text}");
        assert_eq!(
            (phase.as_str(), limit),
            ("forget-e|r1", json!(64)),
            "{text}"
        );
    }
}

#[test]
fn a_request_may_name_its_thing_in_one_word() {
    for c in cases("one-word") {
        let text = text_of(&c);
        let (phase, m, limit) = episode_search(&one(&text)).unwrap_or_else(|| panic!("{text}"));
        assert_eq!(m, c["want"], "{text}");
        assert_eq!(
            (phase.as_str(), limit),
            ("forget-e|r1|8", json!(8)),
            "{text}"
        );
    }
    // eight rows of one word are too broad, seven are marked
    let rows = |n: usize| Value::Array((0..n).map(|i| json!({"id": format!("e{i}")})).collect());
    let (out, journal) = back_with_journal("forget-e|r1|8", "search", rows(8));
    assert!(tool_calls(&out).is_empty(), "{:?}", tool_calls(&out));
    assert!(journal.contains("forget_too_broad"), "{journal}");
    let calls = tool_calls(&back("forget-e|r1|8", "search", rows(7)));
    assert_eq!(calls[0].1["operation"], "update");
    assert_eq!(calls[0].1["set"]["closure_source"], "forget_request:r1");
}

/// Review D1 (kf89 deep review), the dealbreaker: "forget what I said, <the
/// new value>" corrects -- it never forgets the new value. Before the fix the
/// words came from the whole sentence (new value included), `forget-o` marked
/// any fact of the turn with ONE of them -- the corrected fact -- and the
/// episode search marked the turn itself.
#[test]
fn a_correction_never_forgets_the_new_value() {
    for c in cases("correction") {
        let text = text_of(&c);
        let new: Vec<String> = c["new"]
            .as_array()
            .expect("new")
            .iter()
            .map(|w| format!("\"{}\"", w.as_str().expect("word")))
            .collect();
        // the closer skipped it: the words come from before the comma only --
        // the old value may be searched, the new one never, and the turn that
        // carries the new value is reached by no search
        let out = one(&text);
        for (phase, op) in forget_calls(&out) {
            assert!(!phase.starts_with("forget-o"), "{text}: {phase}");
            let m = op["match"].as_str().unwrap_or_default();
            assert!(
                !new.iter().any(|w| m.contains(w.as_str())),
                "{text}: {phase} searches the new value: {m}"
            );
            if phase.starts_with("forget-e") {
                assert_eq!(op["where"]["id"], json!({"neq": "r1"}), "{text}");
            }
        }
        // the closer cited it with the new value's words: the turn that carries
        // the new value is reached by no search, neither its facts nor itself
        let cited = new.join(" ").replace('"', "");
        let out = apply_round(
            json!({"forget": [{"episode_id": "r1", "words": cited, "fact_ids": []}]}),
            vec![turn(
                "r1",
                "user",
                "member:e",
                &text,
                "2026-02-10T09:00:00Z",
            )],
        );
        for (phase, op) in forget_calls(&out) {
            assert!(!phase.starts_with("forget-o"), "{text}: {phase}");
            if phase.starts_with("forget-e") {
                assert_eq!(op["where"]["id"], json!({"neq": "r1"}), "{text}");
            }
        }
    }
    // a request that goes on past its thing keeps its own turn: no `forget-o`,
    // and the episode search leaves the turn that carries the new value
    let out = one(
        "Please forget what I told you about the old bank loan, the new loan is with the credit union now.",
    );
    let calls = forget_calls(&out);
    assert!(
        calls.iter().all(|(p, _)| !p.starts_with("forget-o")),
        "{calls:?}"
    );
    let (_, m, _) = episode_search(&out).expect("a request");
    assert_eq!(m, "\"old\" AND \"bank\" AND \"loan\"");
    let ep = calls
        .iter()
        .find(|(p, _)| p.starts_with("forget-e"))
        .expect("episodes");
    assert_eq!(ep.1["where"]["id"], json!({"neq": "r1"}));
    // without a continuation `forget-o` needs every word of the request
    let out = one("Please forget what I told you about the bank loan.");
    let o = forget_calls(&out)
        .into_iter()
        .find(|(p, _)| p.starts_with("forget-o"))
        .expect("forget-o");
    assert_eq!(o.1["match"], "claim : \"bank\" AND claim : \"loan\"");
}

/// Review N2: the close report counts what was MARKED; every search answer
/// reports itself -- marked, too broad or nothing found.
#[test]
fn a_forget_counts_where_rows_are_marked() {
    let rows = |n: usize| Value::Array((0..n).map(|i| json!({"id": format!("e{i}")})).collect());
    let reported = |out: &[Value]| report_of(out, "forget");
    let r = reported(&back("forget-e|r1|8", "search", rows(8)));
    assert_eq!(
        (
            r["forgotten"].clone(),
            r["forget_too_broad"].clone(),
            r["forget_unmatched"].clone()
        ),
        (json!(0), json!(1), json!(0))
    );
    let r = reported(&back("forget-f|r1", "search", rows(0)));
    assert_eq!(
        (r["forgotten"].clone(), r["forget_unmatched"].clone()),
        (json!(0), json!(1))
    );
    let r = reported(&back("forget-e|r1", "search", rows(3)));
    assert_eq!(
        (r["forgotten"].clone(), r["request"].clone()),
        (json!(3), json!("r1"))
    );
    let r = reported(&back("forget-o|r1", "search", rows(1)));
    assert_eq!(
        (r["forgotten"].clone(), r["search"].clone()),
        (json!(1), json!("o"))
    );
}

fn report_of(out: &[Value], phase: &str) -> Value {
    out.iter()
        .find(|m| m["header"]["route"] == "close_report" && m["header"]["phase"] == phase)
        .unwrap_or_else(|| panic!("a {phase} report: {out:?}"))["header"]
        .clone()
}

/// Review N3: the episode search of a request reaches the own side only -- a
/// peer's turn in a shared room that carries every word stays theirs.
#[test]
fn a_peer_episode_is_never_forgotten() {
    let out = one("Please forget what I told you about the bank loan.");
    let ep = forget_calls(&out)
        .into_iter()
        .find(|(p, _)| p.starts_with("forget-e"))
        .expect("episodes");
    assert_eq!(ep.1["where"]["sender"], json!({"or_null": {"neq": "peer"}}));
}

#[test]
fn only_the_speakers_own_facts_are_forgotten() {
    let request = "Please forget what I told you about the money my neighbour owes me.";
    let found = searches(&one(request));
    let own = &found
        .iter()
        .find(|(p, _)| p == "forget-o|r1")
        .expect("the request's own facts")
        .1;
    // Review D1: without a continuation `forget-o` needs every word of the
    // request, like `forget-f` (`a_correction_never_forgets_the_new_value`).
    assert_eq!(
        own["match"],
        "claim : \"money\" AND claim : \"neighbour\" AND claim : \"owes\""
    );
    assert_eq!(own["where"]["episode_id"], json!({"eq": "r1"}));
    for (phase, op) in &found {
        if op["table"] == "facts" {
            assert_eq!(
                op["where"]["source"],
                json!({"or_null": {"eq": ""}}),
                "{phase}"
            );
        }
    }
    // a closer that names a peer's fact by id marks the own one only
    let mut peer = fact(
        "f-peer",
        "peer b77e01c2",
        "peer b77e01c2",
        AUDIENCE,
        "2026-01-01T00:00:00Z",
    );
    peer["source"] = json!("b77e01c2");
    let mine = fact(
        "f-own",
        "member:e",
        "member:e",
        AUDIENCE,
        "2026-01-01T00:00:00Z",
    );
    let out = apply_round_with(
        json!({"forget": [{"episode_id": "r1", "words": "neighbour owes money",
                           "fact_ids": ["f-peer", "f-own"]}]}),
        vec![turn(
            "r1",
            "user",
            "member:e",
            request,
            "2026-02-10T09:00:00Z",
        )],
        json!([peer, mine]),
    );
    let marks: Vec<Value> = tool_calls(&out)
        .into_iter()
        .filter(|(_, o)| o["operation"] == "update" && o["table"] == "facts")
        .map(|(_, o)| o["where"]["id"].clone())
        .collect();
    assert_eq!(marks, vec![json!({"in": ["f-own"]})]);
}

#[test]
fn a_later_turn_of_the_session_is_not_reached() {
    let out = apply_round(
        json!({"nothing_to_add": true, "forget": []}),
        vec![
            turn(
                "r0",
                "user",
                "member:e",
                "The bank debt is still open.",
                "2026-02-10T09:00:00Z",
            ),
            turn(
                "r1",
                "user",
                "member:e",
                "Please forget what I told you about the bank debt.",
                "2026-02-10T09:05:00Z",
            ),
            turn(
                "r7",
                "user",
                "member:e",
                "The bank debt from 2020 is paid off now.",
                "2026-02-10T09:10:00Z",
            ),
        ],
    );
    let found = searches(&out);
    let of = |phase: &str| {
        found
            .iter()
            .find(|(p, _)| p == phase)
            .unwrap_or_else(|| panic!("{phase}: {found:?}"))
            .1
            .clone()
    };
    // this session only up to the request, every other one by `valid_from`
    assert_eq!(
        of("forget-s|r1")["where"]["episode_id"],
        json!({"in": ["r0", "r1"]})
    );
    assert_eq!(
        of("forget-f|r1")["where"]["session_id"],
        json!({"or_null": {"neq": "s-2"}})
    );
    // a full provenance page is marked and said
    let page = Value::Array(
        (0..256)
            .map(|i| json!({"id": format!("f{i:03}")}))
            .collect(),
    );
    let (out, journal) = back_with_journal("forget-p|r1", "select", page);
    assert!(journal.contains("forget_provenance_truncated"), "{journal}");
    assert_eq!(tool_calls(&out)[0].1["table"], "facts");
}

/// The grammar of a request is language DATA with one home: the files beside
/// the cell (`close-glue/lang/<code>.json`) are copied byte for byte into
/// `params.forget_lang` and its declared default, and the script carries none
/// of it -- a language is added by a file, never by script text.
#[test]
fn the_grammar_is_the_language_data_beside_the_cell() {
    let dir = format!(
        "{}/../../templates/memory-hive/close-glue",
        env!("CARGO_MANIFEST_DIR")
    );
    let cfg: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{dir}/config.json")).expect("config"),
    )
    .expect("json");
    let mut langs = Vec::new();
    for entry in std::fs::read_dir(format!("{dir}/lang")).expect("lang dir") {
        let path = entry.expect("entry").path();
        let code = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("code")
            .to_string();
        let data: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("lang file"))
            .expect("lang json");
        assert_eq!(
            cfg["params"]["forget_lang"][&code], data,
            "params.forget_lang.{code} drifted from lang/{code}.json -- copy the file into the \
             config (params and contract.settings default)"
        );
        langs.push(code);
    }
    langs.sort();
    assert_eq!(langs, vec!["de", "en"]);
    assert_eq!(
        cfg["params"]["forget_lang"].as_object().map(|o| o.len()),
        Some(langs.len())
    );
    assert_eq!(
        cfg["params"]["forget_lang"],
        cfg["contract"]["settings"]["forget_lang"]["default"]
    );
    let script = cfg["params"]["script_inline"].as_str().expect("script");
    for block in cfg["params"]["forget_lang"]
        .as_object()
        .expect("langs")
        .values()
    {
        for cue in block["cues"].as_array().expect("cues") {
            let cue = cue.as_str().expect("cue");
            assert!(
                !script.contains(cue),
                "the script carries a cue of the language data: {cue}"
            );
        }
    }
}

/// Review KF-F (F1): a comma clause of two words after the thing goes on with
/// the new value as much as a longer one -- "Forget that, Tuesday works." The
/// `correction` cases above carry it; here the closer cites the OLD value of
/// such a sentence, and its own turn, which carries the new one, stays.
#[test]
fn a_short_continuation_keeps_the_turn_of_the_new_value() {
    let out = apply_round(
        json!({"forget": [{"episode_id": "r1", "words": "termin montag", "fact_ids": []}]}),
        vec![turn(
            "r1",
            "user",
            "member:e",
            "Vergiss das mit Montag, Dienstag passt.",
            "2026-02-10T09:00:00Z",
        )],
    );
    let calls = forget_calls(&out);
    assert!(!calls.is_empty(), "the cited request is searched");
    for (phase, op) in &calls {
        assert!(!phase.starts_with("forget-o"), "{phase}");
        if phase.starts_with("forget-e") {
            assert_eq!(op["where"]["id"], json!({"neq": "r1"}));
        }
    }
}

/// Review KF-F (NIT): a list of one-word items whose last one follows the
/// language's `conjunction` is the thing itself, not a continuation -- every
/// name of "Anna, Ben, and Carl" is a word of the request.
#[test]
fn a_list_names_every_item() {
    for c in cases("list") {
        let text = text_of(&c);
        let (_, m, _) = episode_search(&one(&text)).expect("a request");
        assert_eq!(m, c["words"], "{text}");
    }
}

/// Review KF-F (NIT): a typo in a switch is said in the journal and the
/// default holds -- it no longer stops the lane, the session close included.
#[test]
fn a_typo_in_a_switch_keeps_the_default() {
    let mut p = params();
    p["forget_fallback"] = json!("ture");
    let out = apply_round_params(
        json!({"nothing_to_add": true, "forget": []}),
        vec![turn(
            "r1",
            "user",
            "member:e",
            "Please forget what I told you about the bank loan.",
            "2026-02-10T09:00:00Z",
        )],
        json!([]),
        p,
    );
    assert!(!out.is_empty(), "the lane went on");
    assert!(forget_calls(&out).is_empty(), "the default is off: {out:?}");
}
