//! GH #1225 (strand KP) -- the push holds what the ledger holds.
//!
//! WHY this file exists: `curator/push` runs `resident`, and its RAM decided
//! what the ledger alone may decide (A1): the ask was parked in a state row
//! and in `_MEM["asks"]`, so a failed read of that row in a fresh child parked
//! the round's only memory ask (B5); a gap and a find were "already being
//! looked up" for one child only, so a twin after a restart asked twice and
//! said a duplex find twice; every failed read was taken for an empty one --
//! a find whose marks could not be read was dropped (B6); two writes in
//! flight inserted the hygiene slot and the tier twice (OR-KF-3); the gap and
//! a short id's words were cut without a mark (B7); the limits of the
//! question were literals (B8). Each test below is red on the push of
//! curator 0.64.3 and names the fix it pins.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

fn push_shipped() -> bool {
    repo("templates/curator/push/config.json").is_file()
}

fn talky(over: &[(&str, &str, Value)]) -> Hive {
    let mut all = vec![("policy", "role", json!("talky"))];
    all.extend(over.iter().cloned());
    Hive::with(&all)
}

fn ask_lane(session: &str, turn: &str, text: &str) -> (&'static str, Value, Value, Value) {
    (
        "in_recall_ask",
        json!({"session_id": session, "channel": "test", "audience_set": ROUND_E}),
        json!({"phase": "recall", "turn_id": turn, "session_id": session, "iter": "0",
               "recall_query": text, "memory_tier": "1",
               "recall_window_from": "", "recall_window_to": ""}),
        json!({"messages": [user(text)]}),
    )
}

fn ask(h: &mut Hive, session: &str, turn: &str, text: &str) -> Vec<Msg> {
    h.out.clear();
    let (route, c, hop, body) = ask_lane(session, turn, text);
    h.lane(route, c, hop, body);
    h.routed("recall")
}

fn round(h: &mut Hive, session: &str, turn: &str, text: &str, reply: &str) -> Msg {
    let asks = ask(h, session, turn, text);
    assert_eq!(asks.len(), 1, "one ask leaves: {:?} {:?}", h.out, h.stderr);
    let call = h.curate(session, turn, 0, json!([user(text)]), mode(""));
    h.tap(&call, "stop", json!({}), json!([said(reply)]));
    call
}

fn gap_lane(
    session: &str,
    turn: &str,
    text: &str,
    engine: &str,
) -> (&'static str, Value, Value, Value) {
    (
        "in_section",
        json!({"session_id": session, "turn_id": turn, "iter": "0", "curator_call": "c-1",
               "channel": "test", "engine": engine, "audience_set": ROUND_E}),
        json!({"section": "gap"}),
        json!({"messages": [], "section": "gap", "payload": {"payload": text}}),
    )
}

fn gap(h: &mut Hive, session: &str, turn: &str, text: &str, engine: &str) -> Vec<Msg> {
    h.out.clear();
    let (route, c, hop, body) = gap_lane(session, turn, text, engine);
    h.lane(route, c, hop, body);
    h.routed("recall")
}

fn bundle_lane(ask: &Msg, text: &str) -> (&'static str, Value, Value, Value) {
    let mut ctx = ask.context.clone();
    ctx.insert("gap_ask".into(), ask.hop["gap_ask"].clone());
    (
        "in_gap_bundle",
        Value::Object(ctx),
        json!({}),
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                             "text": text}]}),
    )
}

fn count(h: &Hive, sql: &str) -> i64 {
    h.rows(sql)[0][0].as_i64().unwrap_or(-1)
}

fn marks_of(h: &Hive, kind: &str) -> i64 {
    count(
        h,
        &format!("SELECT count(*) FROM marks WHERE kind = '{kind}'"),
    )
}

fn addenda_in(call: &Msg) -> usize {
    texts(call)
        .iter()
        .filter(|t| t.contains("[addendum to your last answer"))
        .count()
}

/// B5 + B6 (a)(b): every read of the ask's first step refused, in a child
/// started anew before it -- the ask leaves all the same, says what it was
/// built without, and is sized by the fallback row, never the old sizes.
#[test]
fn an_ask_whose_reads_all_failed_leaves_from_a_fresh_child() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(
        &mut h,
        "s1",
        "t1",
        "My sister Hannah moved to Porto.",
        "Porto, lovely.",
    );
    h.ledger_may_refuse = true;
    h.refuse_phase = Some("ask".into());
    h.restart_at = Some(("push".into(), "ask".into(), 2));
    let asks = ask(&mut h, "s1", "t2", "Where does she live now?");
    assert!(
        h.restart_at.is_none() && h.refuse_phase.is_none(),
        "both struck"
    );
    assert_eq!(asks.len(), 1, "the ask leaves: {:?} {:?}", h.out, h.stderr);
    let degraded: Vec<&str> = asks[0].hop["recall_degraded"]
        .as_array()
        .unwrap_or_else(|| panic!("no recall_degraded: {:?}", asks[0].hop))
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for leg in ["a-wall", "a-state", "a-topic"] {
        assert!(degraded.contains(&leg), "{leg} named: {degraded:?}");
    }
    assert_eq!(
        asks[0].hop["recall_input_soft"], "250000",
        "the fallback row's window"
    );
    assert!(
        asks[0].hop["recall_query"]
            .as_str()
            .unwrap_or("")
            .ends_with("Where does she live now?")
    );
}

/// B5: the second step in a child started anew between the steps builds the
/// same question as one child does.
#[test]
fn the_second_step_in_a_fresh_child_asks_the_same() {
    if !push_shipped() {
        return;
    }
    let mut queries = Vec::new();
    for restart in [false, true] {
        let mut h = talky(&[]);
        round(
            &mut h,
            "s1",
            "t1",
            "My sister Hannah moved to Porto.",
            "Porto, lovely.",
        );
        if restart {
            // The first round had nothing more to read: this is the first `ask-b`.
            h.restart_at = Some(("push".into(), "ask-b".into(), 1));
        }
        let asks = ask(&mut h, "s1", "t2", "Where does she live now?");
        assert!(h.restart_at.is_none(), "the restart struck");
        assert_eq!(asks.len(), 1, "{:?}", h.stderr);
        assert!(
            asks[0].hop.get("recall_degraded").is_none(),
            "{:?}",
            asks[0].hop
        );
        queries.push(asks[0].hop["recall_query"].clone());
    }
    assert_eq!(queries[0], queries[1]);
    assert!(
        queries[0].as_str().unwrap_or("").contains("Hannah"),
        "{queries:?}"
    );
}

/// OR-KF-3: two asks in flight before the first hygiene slot and the first
/// tier: one row each, and no write of either fails.
#[test]
fn two_asks_in_flight_write_one_slot_and_one_tier() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    h.lanes_together(vec![
        ask_lane("s1", "t1", "hello there"),
        ask_lane("s1", "t2", "hello again"),
    ]);
    assert_eq!(h.routed("recall").len(), 2, "{:?}", h.stderr);
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM slots WHERE path = 'instructions.hygiene'"
        ),
        1
    );
    assert_eq!(
        count(&h, "SELECT count(*) FROM state WHERE key = 'push:tier'"),
        1
    );
}

/// A1 (`gaps`): the same gap twice in flight, the child started anew between
/// the two -- one mark, one lookup.
#[test]
fn a_twin_gap_after_a_restart_is_looked_up_once() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "hi", "hello");
    // Run 1 of the push's lanes was the round's ask; the twin is run 3.
    h.restart_at = Some(("push".into(), "".into(), 3));
    h.out.clear();
    h.lanes_together(vec![
        gap_lane("s1", "t1", "the ferry", ""),
        gap_lane("s1", "t1", "the ferry", ""),
    ]);
    assert!(h.restart_at.is_none(), "the restart struck");
    assert_eq!(marks_of(&h, "gap"), 1, "{:?}", h.stderr);
    assert_eq!(h.routed("recall").len(), 1, "{:?}", h.out);
}

/// A1 (`found`): the same find twice in flight, the child started anew between
/// the two -- one addendum, and a duplex call hears it once.
#[test]
fn a_twin_find_after_a_restart_is_kept_once_and_said_once() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "hi", "hello");
    let asked = gap(&mut h, "s1", "t1", "the ferry", "duplex")[0].clone();
    h.restart_at = Some(("push".into(), "".into(), 4));
    h.out.clear();
    let find = "- Hannah took the ferry Estrela do Norte";
    h.lanes_together(vec![bundle_lane(&asked, find), bundle_lane(&asked, find)]);
    assert!(h.restart_at.is_none(), "the restart struck");
    assert_eq!(marks_of(&h, "addendum"), 1, "{:?}", h.stderr);
    assert_eq!(h.routed("sidecar").len(), 1, "said once: {:?}", h.out);
}

/// B6 (c) + E6: a find whose marks could not be read is held, never dropped;
/// the next message of its round looks at it again, and the window of the
/// round after shows it exactly once.
#[test]
fn a_find_whose_marks_failed_is_held_and_shown_once() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "hi", "hello");
    let asked = gap(&mut h, "s1", "t1", "the ferry", "")[0].clone();
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("h-marks".into(), "store_error".into())];
    let (route, c, hop, body) = bundle_lane(&asked, "- Hannah took the ferry Estrela do Norte");
    h.lane(route, c, hop, body);
    assert!(h.refuse_legs.is_empty(), "the refusal struck");
    h.ledger_may_refuse = false;
    assert_eq!(marks_of(&h, "addendum"), 0);
    assert_eq!(marks_of(&h, "held"), 1, "held: {:?}", h.stderr);
    assert_eq!(
        count(
            &h,
            "SELECT count(*) FROM state WHERE key LIKE 'pending:push:gap-%'"
        ),
        1,
        "the state keeps the find"
    );
    // The next message of the round: the find is looked at again and kept.
    let first = round(&mut h, "s1", "t2", "and when does it leave?", "At nine.");
    assert_eq!(addenda_in(&first), 0, "kept now, shown in the round after");
    assert_eq!(marks_of(&h, "addendum"), 1, "{:?}", h.stderr);
    assert_eq!(marks_of(&h, "held_done"), 1);
    let mut shown = 0;
    for (turn, words) in [("t3", "thanks"), ("t4", "and the return?")] {
        let call = round(&mut h, "s1", turn, words, "Sure.");
        shown = shown.max(addenda_in(&call));
        assert!(addenda_in(&call) <= 1, "never twice in one window");
    }
    assert_eq!(shown, 1, "shown once");
    assert_eq!(marks_of(&h, "addendum_done"), 1, "handed on once");
}

/// B6: the blocks of a pending find could not be read -- it waits, unmarked,
/// and the next ask hands it on.
#[test]
fn a_find_whose_blocks_failed_waits_for_the_next_ask() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "hi", "hello");
    let asked = gap(&mut h, "s1", "t1", "the ferry", "")[0].clone();
    let (route, c, hop, body) = bundle_lane(&asked, "- Hannah took the ferry Estrela do Norte");
    h.lane(route, c, hop, body);
    assert_eq!(marks_of(&h, "addendum"), 1);
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("b-blocks".into(), "store_error".into())];
    let asks = ask(&mut h, "s1", "t2", "and when does it leave?");
    assert!(h.refuse_legs.is_empty(), "the refusal struck");
    h.ledger_may_refuse = false;
    assert_eq!(asks.len(), 1);
    assert_eq!(
        marks_of(&h, "addendum_done"),
        0,
        "nothing marked as handed on"
    );
    let call = round(&mut h, "s1", "t3", "thanks", "Sure.");
    assert_eq!(addenda_in(&call), 1, "the next ask hands it on");
    let done = h.rows("SELECT value FROM marks WHERE kind = 'addendum_done'");
    assert_eq!(done.len(), 1);
    let v: Value = sj::from_str(done[0][0].as_str().unwrap_or("{}")).unwrap_or_default();
    assert_eq!(v["delivered"], 1, "{v}");
}

/// B6 + OR-KF-35: a colony without the `candidates` table (the store's
/// catalog code `unknown_table`) has no candidate -- an empty read, the ask
/// leaves whole; a candidate read that timed out is a failed one, and the ask
/// names it.
#[test]
fn a_missing_candidates_table_is_empty_and_a_timeout_is_degraded() {
    if !push_shipped() {
        return;
    }
    for (code, degraded) in [("unknown_table", false), ("query_timeout", true)] {
        let mut h = talky(&[]);
        round(
            &mut h,
            "s1",
            "t1",
            "My sister Hannah moved to Porto.",
            "Porto, lovely.",
        );
        h.ledger_may_refuse = true;
        h.refuse_legs = vec![("a-cand".into(), code.into())];
        let asks = ask(&mut h, "s1", "t2", "Where does she live now?");
        assert!(h.refuse_legs.is_empty(), "{code}: the refusal struck");
        assert_eq!(asks.len(), 1, "{code}: the ask leaves: {:?}", h.stderr);
        let named = asks[0].hop.get("recall_degraded").cloned();
        if degraded {
            assert_eq!(named, Some(json!(["a-cand"])), "{code}: {:?}", asks[0].hop);
        } else {
            assert!(named.is_none(), "{code}: an empty read: {:?}", asks[0].hop);
        }
    }
}

/// B7: a gap is asked whole -- the memory's own guard cuts a question.
#[test]
fn a_gap_is_asked_whole() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "hi", "hello");
    let long = "whether the ferry ".repeat(60).trim().to_string();
    let asks = gap(&mut h, "s1", "t1", &long, "");
    assert_eq!(asks.len(), 1);
    assert_eq!(asks[0].hop["recall_query"], json!(long));
}

/// B7: the words a short id points at are cut with a mark.
#[test]
fn a_short_ids_words_are_cut_with_a_mark() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    let said_first = "My sister Hannah moved to Porto with her two cats and a piano last spring.";
    round(&mut h, "s1", "t1", said_first, "Lovely.");
    let first: String = h.rows("SELECT hash FROM wall WHERE kind = 'user' ORDER BY seq")[0][0]
        .as_str()
        .unwrap()
        .to_string();
    let text = format!("What about [#{}]?", &first[..12]);
    let q = ask(&mut h, "s1", "t2", &text)[0].hop["recall_query"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(q.contains("refers to: My sister Hannah"), "{q}");
    assert!(q.contains('…'), "the cut is marked: {q}");
}

/// B8: the limits of the question are params, the shipped values the defaults.
#[test]
fn the_limits_of_the_question_are_params() {
    if !push_shipped() {
        return;
    }
    let cfg = read_json(&repo("templates/curator/push/config.json"));
    for (k, v) in [
        ("entity_max", 8),
        ("entity_rounds", 3),
        ("wall_page", 48),
        ("ref_max", 3),
        ("ref_chars", 60),
        ("candidate_rows", 128),
    ] {
        assert_eq!(cfg["params"][k], json!(v), "{k}");
        assert_eq!(cfg["contract"]["settings"][k]["default"], json!(v), "{k}");
    }
    let mut h = talky(&[("push", "entity_max", json!(1))]);
    round(
        &mut h,
        "s1",
        "t1",
        "My sister Hannah moved to Porto.",
        "Porto, lovely.",
    );
    let q = ask(&mut h, "s1", "t2", "Where does she live now?")[0].hop["recall_query"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let named = q
        .split("mentioned: ")
        .nth(1)
        .and_then(|r| r.split(';').next())
        .unwrap_or_else(|| panic!("no entities: {q}"));
    assert_eq!(named.split(", ").count(), 1, "{q}");
}
