//! GH #1225 (strand KP, fix round 1) -- names kept, and the ledger's word on
//! what is said.
//!
//! WHY this file exists: the re-check of KP found that the B4 rules lost names
//! the base kept -- a capital behind a determiner was always a noun ("die
//! Irmgard", "im Mai"), and a noun by its form beat the salutation in front of
//! it ("Herrn Hartung", "Frau Bader"); and that a find (`addendum_done`) and a
//! `once` candidate (`used_at`) were said by both of two asks in flight once
//! the child was started anew between them -- the RAM guard was all that held
//! them. Each test below is red on the push of 26d6259ce and names the fix it
//! pins; the last two pin the B6 consequences of a refused tier read and a
//! refused read of who is on the line.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

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

fn round(h: &mut Hive, session: &str, turn: &str, text: &str, reply: &str) {
    let asks = ask(h, session, turn, text);
    assert_eq!(asks.len(), 1, "one ask leaves: {:?} {:?}", h.out, h.stderr);
    let call = h.curate(session, turn, 0, json!([user(text)]), mode(""));
    h.tap(&call, "stop", json!({}), json!([said(reply)]));
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

/// How often the wall carries a block whose body holds `words`.
fn on_the_wall(h: &Hive, words: &str) -> i64 {
    count(
        h,
        &format!(
            "SELECT count(*) FROM wall w JOIN blocks b ON b.hash = w.hash \
             WHERE b.body LIKE '%{words}%'"
        ),
    )
}

fn mentioned(h: &mut Hive, turn: &str) -> Vec<String> {
    let asks = ask(h, "s1", turn, "Was meinst du dazu?");
    assert_eq!(asks.len(), 1, "{:?}", h.stderr);
    let q = asks[0].hop["recall_query"]
        .as_str()
        .unwrap_or("")
        .to_string();
    q.split("mentioned: ")
        .nth(1)
        .and_then(|r| r.split(';').next())
        .and_then(|r| r.split("] ").next())
        .map(|r| r.split(", ").map(str::to_string).collect())
        .unwrap_or_default()
}

fn assert_names(got: &[String], names: &[&str], nouns: &[&str]) {
    for name in names {
        assert!(got.iter().any(|g| g == name), "{name} in {got:?}");
    }
    for noun in nouns {
        assert!(!got.iter().any(|g| g == noun), "{noun} is no name: {got:?}");
    }
}

/// I1: a capital behind a salutation is a name whatever its form ends in --
/// "Hartung" (-ung) and "Bader" (Bad + -er) stay names.
#[test]
fn a_name_behind_a_salutation_beats_its_form() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(
        &mut h,
        "s1",
        "t1",
        "Gestern war ich mit Herrn Hartung beim Arzt.",
        "Und gestern kam Frau Bader vorbei?",
    );
    let got = mentioned(&mut h, "t2");
    assert_names(&got, &["Hartung", "Bader"], &["Herrn", "Frau", "Arzt"]);
}

/// I1: German puts an article in front of a first name and a place ("die
/// Irmgard", "ins Rosengarten") as in front of a noun -- only the list tells
/// them apart, so the name the base kept is kept.
#[test]
fn a_name_behind_an_article_is_kept() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(
        &mut h,
        "s1",
        "t1",
        "Mit der Irmgard geh ich nicht mehr ins Rosengarten.",
        "Hat die Lotte schon angerufen?",
    );
    let got = mentioned(&mut h, "t2");
    assert_names(&got, &["Irmgard", "Rosengarten", "Lotte"], &["der", "die"]);
}

/// I1 (B4 (a)): a weekday or a month is a date wherever it stands -- behind
/// "im" and "jeden" too.
#[test]
fn a_weekday_or_a_month_is_a_date_behind_an_article() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(
        &mut h,
        "s1",
        "t1",
        "Wir haben im Mai geheiratet.",
        "Und jeden Sonntag gehst du zum Friedhof?",
    );
    let got = mentioned(&mut h, "t2");
    assert_names(&got, &["Mai", "Sonntag"], &[]);
}

/// I2 (A1 `delivered`): two asks in flight that both carry a find, with and
/// without the child started anew between their second steps -- the find is
/// on the wall once and marked handed on once.
#[test]
fn a_find_two_asks_carry_is_shown_once_and_marked_once() {
    if !push_shipped() {
        return;
    }
    for restart in [false, true] {
        let mut h = talky(&[]);
        round(&mut h, "s1", "t1", "hi", "hello");
        let asked = gap(&mut h, "s1", "t1", "the ferry", "")[0].clone();
        let (route, c, hop, body) = bundle_lane(&asked, "- Hannah took the ferry Estrela do Norte");
        h.lane(route, c, hop, body);
        assert_eq!(marks_of(&h, "addendum"), 1, "{:?}", h.stderr);
        if restart {
            // No ask before had anything more to read: these are the first two.
            h.restart_at = Some(("push".into(), "ask-b".into(), 2));
        }
        h.out.clear();
        h.lanes_together(vec![
            ask_lane("s1", "t2", "and when does it leave?"),
            ask_lane("s1", "t3", "and when does it come back?"),
        ]);
        assert!(h.restart_at.is_none(), "the restart struck");
        assert_eq!(h.routed("recall").len(), 2, "{:?}", h.stderr);
        assert_eq!(
            marks_of(&h, "addendum_done"),
            1,
            "restart {restart}: {:?}",
            h.stderr
        );
        assert_eq!(on_the_wall(&h, "Estrela do Norte"), 1, "restart {restart}");
    }
}

/// I2 (A1 `used`): a `once` candidate two asks in flight both pick, with and
/// without the child started anew between their second steps -- said once.
#[test]
fn a_once_candidate_two_asks_pick_is_said_once() {
    if !push_shipped() {
        return;
    }
    for restart in [false, true] {
        let mut h = talky(&[("push", "candidate_cooldown", json!(0))]);
        let el = json!({"type": "candidate", "source": "app-test", "text": "say it once"});
        let body = canonical(&el);
        let hash = sha256_hex(&body);
        h.db.execute(
            "INSERT INTO blocks (hash, kind, chars, body, first_seen) \
             VALUES (?1, 'candidate', 11, ?2, '2026-10-01T10:00:00.000000Z')",
            rusqlite::params![hash, body],
        )
        .unwrap();
        h.db.execute(
            "INSERT INTO candidates (source, cand_id, hash, triggers, until, once, used_at, \
             last_seq, priority, audience_set, at) \
             VALUES ('app-test', 'o', ?1, '[]', '', 1, '', 0, 5, ?2, \
             '2026-10-01T10:00:00.000000Z')",
            rusqlite::params![hash, ROUND_E],
        )
        .unwrap();
        if restart {
            h.restart_at = Some(("push".into(), "ask-b".into(), 2));
        }
        h.out.clear();
        h.lanes_together(vec![
            ask_lane("s1", "t1", "hi"),
            ask_lane("s1", "t2", "hi again"),
        ]);
        assert!(h.restart_at.is_none(), "the restart struck");
        assert_eq!(h.routed("recall").len(), 2, "{:?}", h.stderr);
        assert_eq!(
            on_the_wall(&h, "say it once"),
            1,
            "restart {restart}: {:?}",
            h.stderr
        );
        assert_eq!(
            count(&h, "SELECT count(*) FROM candidates WHERE used_at != ''"),
            1,
            "restart {restart}"
        );
    }
}

/// M3: the gap mark is keyed by the gap, not by what a message read of the
/// tier -- a twin whose tier read was refused and one whose read answered
/// write one mark, and the gap is looked up at most once.
#[test]
fn a_twin_gap_with_another_tier_read_is_marked_once() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "hi", "hello");
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("g-state".into(), "store_error".into())];
    h.out.clear();
    h.lanes_together(vec![
        gap_lane("s1", "t1", "the ferry", ""),
        gap_lane("s1", "t1", "the ferry", ""),
    ]);
    assert!(h.refuse_legs.is_empty(), "the refusal struck");
    h.ledger_may_refuse = false;
    assert_eq!(marks_of(&h, "gap"), 1, "{:?}", h.stderr);
    assert!(h.routed("recall").len() <= 1, "{:?}", h.out);
}

/// M2 (B6): the tier read refused -- the gap is kept, marked as looked up
/// without it, and not looked up.
#[test]
fn a_refused_tier_read_keeps_the_gap_unlooked() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "hi", "hello");
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("g-state".into(), "store_error".into())];
    let asked = gap(&mut h, "s1", "t1", "the ferry", "");
    assert!(h.refuse_legs.is_empty(), "the refusal struck");
    h.ledger_may_refuse = false;
    assert!(asked.is_empty(), "not looked up: {:?}", h.out);
    let v = h.rows("SELECT value FROM marks WHERE kind = 'gap'");
    assert_eq!(v.len(), 1, "{:?}", h.stderr);
    let v = v[0][0].as_str().unwrap_or("");
    assert!(v.contains("\"degraded\":[\"g-state\"]"), "{v}");
    assert!(v.contains("\"searched\":0"), "{v}");
}

/// M2 (B6): who is on the line could not be read -- a duplex find is kept,
/// and no fact is said (fail-closed).
#[test]
fn a_refused_read_of_the_line_says_no_fact() {
    if !push_shipped() {
        return;
    }
    let mut h = talky(&[]);
    round(&mut h, "s1", "t1", "hi", "hello");
    let asked = gap(&mut h, "s1", "t1", "the ferry", "duplex")[0].clone();
    h.ledger_may_refuse = true;
    h.refuse_legs = vec![("j-now".into(), "store_error".into())];
    h.out.clear();
    let (route, c, hop, body) = bundle_lane(&asked, "- Hannah took the ferry Estrela do Norte");
    h.lane(route, c, hop, body);
    assert!(h.refuse_legs.is_empty(), "the refusal struck");
    h.ledger_may_refuse = false;
    assert_eq!(marks_of(&h, "addendum"), 1, "{:?}", h.stderr);
    assert!(h.routed("sidecar").is_empty(), "no fact: {:?}", h.out);
}
