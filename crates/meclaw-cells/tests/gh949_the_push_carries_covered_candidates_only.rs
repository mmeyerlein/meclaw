//! GH #949 -- the push carries only the candidates the round of its ask may see.
//!
//! `./intake` keeps the push candidates other hives hand the curator in the
//! ledger table `candidates`, each under the round it came in (text in
//! `blocks`, kind `candidate`). `curator/push` hands the ones a turn carries
//! to the MODEL: as ONE `memory_recall` pair -- the call's `query` the
//! triggers the person's words hit, the result a frame line and the part
//! `[<text>; <text>]` within its share of the window (a twentieth of the
//! state `input_soft`, R-IG-1; every candidate without one) -- on the list of
//! pairs it gives `./intake` (`in_addendum`) for the round of the ask, ahead
//! of the round's first turn, the road of a gap's find. The memory question
//! (`recall_query`) never carries them: it reaches the memory alone, which
//! keeps only its last sentence once it is over `query_safe_chars`, and the
//! model never sees it (review of commit 968b57653, OR-BC.K.2). Pinned here,
//! through the shipped hive (`support/curator_hive.rs`: the shipped scripts,
//! the hive's own edges, the ledger as a real store), measured at the
//! receiver -- the wall `./intake` wrote, and the call that left for the
//! model:
//!
//! * the question of every turn is byte for byte the question without
//!   candidates;
//! * the round never widens: {e,a} is carried what {e,a}, a round holding it
//!   and `*` are for, never what {e,b} or {e} alone are for; a round-less ask
//!   (and the declared empty round) only what declares no round (`[]`) or
//!   names `*`; an ended candidate never; the pair stands under the round;
//! * a `once` candidate is carried exactly once, and the ledger says so;
//! * a candidate is not carried again within `candidate_cooldown` turns, and
//!   every showing is a pair of its own;
//! * the budget takes whole candidates, best first, marks only those, and says
//!   the ones it left out behind the part (R-IG-1, GH #1085);
//! * `candidate_push` "0" is off -- not even a read of the table;
//! * a turn that carries no candidate leaves byte for byte as curator 1.5.0
//!   sent it and hands `./intake` no pair: with an empty table, with rows
//!   that do not pass, with the candidates off, and with a ledger that has no
//!   such table at all.
//!
//! Red before the fix (968b57653): the question opens with the part
//! (`question ... byte for byte`) and the wall holds no candidate pair.
//!
//! The candidate rows are sown straight into the ledger the way the door
//! files them (canonical audience, raw trigger list), so this lock pins the
//! push alone; the door is `gh949_a_candidate_never_widens_its_round`.

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

const EA: &str = r#"["member:a","member:e"]"#;
const EB: &str = r#"["member:b","member:e"]"#;
const EAC: &str = r#"["member:a","member:c","member:e"]"#;
const E_ONLY: &str = r#"["member:e"]"#;
const ALL: &str = r#"["*"]"#;
const NO_ROUND: &str = "[]";

/// The first line of the result of a candidate pair: what the model is told
/// the notes are (generic, the push's `CANDIDATE_FRAME`).
const FRAME: &str =
    "[addendum -- notes handed to this conversation for this round; not said by the person]";
/// The id of a candidate pair: this prefix and 16 hex digits.
const PAIR_ID: &str = "call_candidates_";

/// The curator of a talky -- the role whose push enriches the question --
/// with `(cell, param, value)` overrides on top.
fn talky(over: &[(&str, &str, Value)]) -> Hive {
    let mut all = vec![("policy", "role", json!("talky"))];
    all.extend(over.iter().cloned());
    Hive::with(&all)
}

fn has_candidates_table(h: &Hive) -> bool {
    !h.rows("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'candidates'")
        .is_empty()
}

/// One candidate as the door files it: its text a `candidate` block, its row
/// keyed by source and id, the audience canonical, the triggers the raw JSON
/// list. Returns the block hash.
#[allow(clippy::too_many_arguments)]
fn sow(
    h: &mut Hive,
    id: &str,
    text: &str,
    audience: &str,
    triggers: &[&str],
    until: &str,
    once: bool,
    priority: i64,
) -> String {
    assert!(
        has_candidates_table(h),
        "the ledger declares the table `candidates` (GH #949)"
    );
    let el = json!({"type": "candidate", "source": "app-test", "text": text});
    let body = canonical(&el);
    let hash = sha256_hex(&body);
    let n: i64 =
        h.db.query_row(
            "SELECT COUNT(*) FROM blocks WHERE hash = ?1",
            [&hash],
            |r| r.get(0),
        )
        .unwrap();
    if n == 0 {
        h.db.execute(
            "INSERT INTO blocks (hash, kind, chars, body, first_seen) \
             VALUES (?1, 'candidate', ?2, ?3, '2026-10-01T10:00:00.000000Z')",
            rusqlite::params![hash, text.chars().count() as i64, body],
        )
        .unwrap();
    }
    let rows: i64 =
        h.db.query_row("SELECT COUNT(*) FROM candidates", [], |r| r.get(0))
            .unwrap();
    // Each row younger than the one before it.
    let at = format!("2026-10-01T10:00:{:02}.000000Z", rows % 60);
    h.db.execute(
        "INSERT INTO candidates (source, cand_id, hash, triggers, until, once, used_at, \
         last_seq, priority, audience_set, at) \
         VALUES ('app-test', ?1, ?2, ?3, ?4, ?5, '', 0, ?6, ?7, ?8)",
        rusqlite::params![
            id,
            hash,
            json!(triggers).to_string(),
            until,
            i64::from(once),
            priority,
            audience,
            at
        ],
    )
    .unwrap();
    hash
}

/// The collector's ask of one turn spoken in `round` (`null`: a round nobody
/// declared); every `recall` that left the hive for it.
fn ask_as(h: &mut Hive, round: &Value, session: &str, turn: &str, text: &str) -> Vec<Msg> {
    h.out.clear();
    h.lane(
        "in_recall_ask",
        json!({"session_id": session, "channel": "test", "audience_set": round}),
        json!({"phase": "recall", "turn_id": turn, "session_id": session, "iter": "0",
               "recall_query": text, "memory_tier": "1",
               "recall_window_from": "", "recall_window_to": ""}),
        json!({"messages": [user(text)]}),
    );
    h.routed("recall")
}

/// The question that left for one ask -- exactly one leaves, always.
fn question(h: &mut Hive, round: &Value, session: &str, turn: &str, text: &str) -> String {
    let asks = ask_as(h, round, session, turn, text);
    assert_eq!(
        asks.len(),
        1,
        "the ask always leaves: {:?} {:?}",
        h.out,
        h.stderr
    );
    asks[0].hop["recall_query"]
        .as_str()
        .unwrap_or("")
        .to_string()
}

/// One candidate pair as the wall holds it.
#[derive(Debug)]
struct Shown {
    /// The candidates' part, `[<text>; <text>]` -- the result after its frame.
    part: String,
    /// The `query` of the call: the triggers the person's words hit.
    query: String,
    /// The id call and result share.
    id: String,
    /// The audience of the two wall rows.
    audience: String,
}

/// The candidate pair `./intake` wrote into the wall for one turn, read at the
/// receiver: the turn's rows in `seq` order whose element carries a candidate
/// id. `None` when there is none; more than one pair fails here.
fn shown(h: &Hive, session: &str, turn: &str) -> Option<Shown> {
    let els: Vec<(Value, String, String)> = h
        .rows(&format!(
            "SELECT b.body, w.audience_set, w.kind FROM wall w JOIN blocks b ON b.hash = w.hash \
             WHERE w.session_id = '{session}' AND w.turn_id = '{turn}' ORDER BY w.seq, w.nth"
        ))
        .into_iter()
        .filter_map(|r| {
            let el: Value = sj::from_str(r[0].as_str()?).ok()?;
            if !el["id"].as_str()?.starts_with(PAIR_ID) {
                return None;
            }
            Some((
                el,
                r[1].as_str().unwrap_or("").to_string(),
                r[2].as_str().unwrap_or("").to_string(),
            ))
        })
        .collect();
    if els.is_empty() {
        return None;
    }
    assert_eq!(els.len(), 2, "one candidate pair per turn: {els:?}");
    let ((call, aud, kind_c), (result, aud_r, kind_r)) = (&els[0], &els[1]);
    assert_eq!(
        (call["origin"].as_str(), call["type"].as_str()),
        (Some("assistant"), Some("tool_call")),
        "the call first: {els:?}"
    );
    assert_eq!(
        (result["origin"].as_str(), result["type"].as_str()),
        (Some("tool"), Some("tool_result")),
        "then its result: {els:?}"
    );
    assert_eq!(call["id"], result["id"], "one id, one pair");
    let id = call["id"].as_str().unwrap_or("").to_string();
    let hex = &id[PAIR_ID.len()..];
    assert!(
        hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "`{PAIR_ID}<16 hex>`: {id}"
    );
    // A `memory_recall` pair, so `./intake` files it as the evidence pairs
    // are filed: never an episode, never a call of the model's own.
    assert_eq!(
        (kind_c.as_str(), kind_r.as_str()),
        ("recall", "recall"),
        "{els:?}"
    );
    assert_eq!(aud, aud_r, "one round for both halves");
    let f: Value = sj::from_str(call["text"].as_str().unwrap_or("")).expect("a function");
    assert_eq!(f["name"], "memory_recall", "{f}");
    let args: Value = sj::from_str(f["arguments"].as_str().unwrap_or("")).expect("arguments");
    let text = result["text"].as_str().unwrap_or("");
    let part = text
        .strip_prefix(&format!("{FRAME}\n"))
        .unwrap_or_else(|| panic!("the result opens with its frame: {text:?}"));
    Some(Shown {
        part: part.to_string(),
        query: args["query"].as_str().unwrap_or("").to_string(),
        id,
        audience: aud.clone(),
    })
}

/// One ask of a turn whose wall holds no participant row yet, so that the
/// question without candidates is the person's words themselves: asserts the
/// question is exactly those, and returns the part the turn's pair carries
/// ("" for no pair).
fn carried(h: &mut Hive, round: &Value, session: &str, turn: &str, text: &str) -> String {
    assert_eq!(
        question(h, round, session, turn, text),
        text,
        "{session}/{turn}: the question byte for byte as without candidates"
    );
    shown(h, session, turn).map(|s| s.part).unwrap_or_default()
}

/// A whole turn the way talky runs it: the ask, the round once its bundle
/// came home, the answer on the tap. Returns the question and the call that
/// left for the model.
fn turn_as(
    h: &mut Hive,
    round: &Value,
    session: &str,
    turn: &str,
    text: &str,
    reply: &str,
) -> (String, Msg) {
    let asked = question(h, round, session, turn, text);
    h.out.clear();
    h.lane(
        "in_curate",
        json!({"session_id": session, "turn_id": turn, "iter": "0", "channel": "test",
               "audience_set": round}),
        json!({"session_id": session, "turn_id": turn, "iter": "0", "phase": ""}),
        json!({"messages": [user(text)], "system": mode("")}),
    );
    let calls = h.routed("brain");
    assert_eq!(
        calls.len(),
        1,
        "one round, one call: {:?} {:?}",
        h.out,
        h.stderr
    );
    let call = calls[0].clone();
    h.tap(&call, "stop", json!({}), json!([said(reply)]));
    (asked, call)
}

/// `(cand_id, used_at, last_seq)` of every candidate, by id.
fn marks_of(h: &Hive) -> Vec<(String, String, i64)> {
    h.rows("SELECT cand_id, used_at, last_seq FROM candidates ORDER BY cand_id")
        .into_iter()
        .map(|r| {
            (
                r[0].as_str().unwrap_or("").to_string(),
                r[1].as_str().unwrap_or("").to_string(),
                r[2].as_i64().unwrap_or(0),
            )
        })
        .collect()
}

/// Every ledger operation of the push on the table `candidates`.
fn candidate_ops(h: &Hive) -> Vec<Value> {
    h.ledger_ops
        .iter()
        .filter(|(who, op)| who == "push" && op["table"] == "candidates")
        .map(|(_, op)| op.clone())
        .collect()
}

// ============================================================ the knobs

#[test]
fn the_push_declares_its_candidate_knobs() {
    if !shipped() {
        return;
    }
    let push = cell_config("push");
    for (knob, default, ty) in [
        ("candidate_push", json!("1"), "string"),
        ("candidate_cooldown", json!(5), "number"),
    ] {
        assert_eq!(push["params"][knob], default, "params.{knob}");
        let s = &push["contract"]["settings"][knob];
        assert_eq!(s["type"], ty, "settings.{knob}");
        assert_eq!(s["default"], default, "settings.{knob}");
        assert!(
            s["description"].as_str().is_some_and(|d| !d.is_empty()),
            "settings.{knob} says what it does"
        );
    }
}

// ============================================================ the brain

/// The plan's acceptance: the turn whose words name the trigger carries the
/// candidate to the model -- in the call that left for the brain, as a pair
/// ahead of the person's words of that turn -- and its memory question is
/// the person's words alone.
#[test]
fn the_turn_carries_the_pair_to_the_brain() {
    if !shipped() {
        return;
    }
    let mut h = talky(&[("push", "candidate_cooldown", json!(0))]);
    sow(
        &mut h,
        "fw",
        "ticket 7 is open",
        EA,
        &["Firewall X"],
        "",
        false,
        5,
    );
    let words = "die firewall x h\u{e4}ngt";
    let (asked, call) = turn_as(&mut h, &json!(EA), "s1", "t1", words, "Looking into it.");
    assert_eq!(
        asked, words,
        "the question byte for byte as without candidates"
    );
    let msgs = call.messages();
    let at = msgs
        .iter()
        .position(|m| {
            m["type"] == "tool_result" && m["id"].as_str().is_some_and(|i| i.starts_with(PAIR_ID))
        })
        .unwrap_or_else(|| panic!("no candidate pair in the call to the brain: {msgs:?}"));
    // With the talky's `short_ids` a tool result begins with its `[#<id>] `.
    let text = msgs[at]["text"].as_str().unwrap_or("");
    assert!(
        text.ends_with(&format!("{FRAME}\n[ticket 7 is open]")),
        "the result carries the frame and the part: {text:?}"
    );
    assert!(at > 0, "{msgs:?}");
    assert_eq!(
        msgs[at - 1]["type"],
        "tool_call",
        "the call in front of its result"
    );
    assert_eq!(msgs[at - 1]["id"], msgs[at]["id"]);
    let f: Value = sj::from_str(msgs[at - 1]["text"].as_str().unwrap_or("")).expect("a function");
    assert_eq!(f["name"], "memory_recall");
    assert_eq!(
        f["arguments"].as_str(),
        Some(r#"{"query": "Firewall X"}"#),
        "the call asks for the trigger the person said"
    );
    let own = msgs
        .iter()
        .position(|m| {
            m["origin"] == "user" && m["text"].as_str().is_some_and(|t| t.ends_with(words))
        })
        .unwrap_or_else(|| panic!("the round's own words: {msgs:?}"));
    assert!(at < own, "the pair opens the round, ahead of its words");
    let s = shown(&h, "s1", "t1").expect("the pair stands in the wall");
    assert_eq!(s.audience, EA, "under the round of the turn");
}

// ============================================================ the round

/// The round never widens (affinity rule, in the store as `covers`): {e,a}
/// is carried what {e,a}, {e,a,c} and `*` are for -- best first -- and
/// never {e,b}, {e} alone, `[]` or an ended one; {e,b} its own and `*`; a
/// round nobody declared, and the declared empty round, only `*` and `[]`.
/// The pair stands under the round of the ask (`[]` for none).
#[test]
fn a_round_gets_only_the_candidates_that_cover_it() {
    if !shipped() {
        return;
    }
    let mut h = talky(&[("push", "candidate_cooldown", json!(0))]);
    sow(
        &mut h,
        "ea",
        "for e and a",
        EA,
        &[],
        "2999-01-01T00:00:00Z",
        false,
        7,
    );
    sow(&mut h, "eac", "for e, a and c", EAC, &[], "", false, 6);
    sow(&mut h, "all", "for everyone", ALL, &[], "", false, 5);
    sow(&mut h, "none", "for no round", NO_ROUND, &[], "", false, 4);
    sow(&mut h, "eb", "for e and b", EB, &[], "", false, 9);
    sow(&mut h, "e", "for e alone", E_ONLY, &[], "", false, 9);
    sow(
        &mut h,
        "old",
        "ended",
        EA,
        &[],
        "2000-01-01T00:00:00Z",
        false,
        9,
    );
    for (round, session, turn, part, aud, what) in [
        (
            json!(EA),
            "s1",
            "t1",
            "[for e and a; for e, a and c; for everyone]",
            EA,
            "{e,a}",
        ),
        (
            json!(EB),
            "s1",
            "t2",
            "[for e and b; for everyone]",
            EB,
            "{e,b}",
        ),
        (
            Value::Null,
            "s2",
            "t1",
            "[for everyone; for no round]",
            NO_ROUND,
            "a round nobody declared",
        ),
        (
            json!(NO_ROUND),
            "s3",
            "t1",
            "[for everyone; for no round]",
            NO_ROUND,
            "the declared empty round",
        ),
    ] {
        assert_eq!(
            carried(&mut h, &round, session, turn, "hello there"),
            part,
            "{what}"
        );
        let s = shown(&h, session, turn).expect("a pair");
        assert_eq!(s.audience, aud, "{what}: the pair stands under the round");
        assert_eq!(s.query, "", "{what}: no trigger, an empty query");
    }
    // The store already holds a hidden row back: every read of the push
    // carries the round in its `where`.
    for op in candidate_ops(&h)
        .iter()
        .filter(|op| op["operation"] == "select")
    {
        assert!(
            op["where"]["audience_set"].is_object(),
            "a candidates read without its round: {op}"
        );
    }
}

/// A trigger carries its candidate only on a turn whose words say it; the
/// call asks for the triggers that were said -- each once, in the order of
/// the candidates (best first) and of their own lists.
#[test]
fn a_trigger_carries_a_candidate_only_on_a_turn_that_says_it() {
    if !shipped() {
        return;
    }
    let mut h = talky(&[("push", "candidate_cooldown", json!(0))]);
    sow(
        &mut h,
        "fw",
        "ticket 7 is open",
        EA,
        &["Firewall X"],
        "",
        false,
        5,
    );
    let round = json!(EA);
    assert_eq!(
        carried(&mut h, &round, "s1", "t1", "die firewall x h\u{e4}ngt"),
        "[ticket 7 is open]"
    );
    assert_eq!(shown(&h, "s1", "t1").expect("a pair").query, "Firewall X");
    assert_eq!(
        carried(&mut h, &round, "s1", "t2", "Firewall Xenon is fine"),
        "",
        "no part of a word"
    );
    assert_eq!(
        carried(&mut h, &round, "s1", "t3", "Firewall X again?"),
        "[ticket 7 is open]",
        "said again, carried again (no cooldown here)"
    );
    // A second candidate whose triggers name the same words in another case
    // and one more; a third without any.
    sow(
        &mut h,
        "pr",
        "the printer is new",
        EA,
        &["Printer", "firewall x"],
        "",
        false,
        4,
    );
    sow(&mut h, "any", "always on", EA, &[], "", false, 1);
    assert_eq!(
        carried(&mut h, &round, "s1", "t4", "the printer and firewall x"),
        "[ticket 7 is open; the printer is new; always on]"
    );
    assert_eq!(
        shown(&h, "s1", "t4").expect("a pair").query,
        "Firewall X, Printer",
        "the triggers said, each once"
    );
}

// ============================================================ once and cooldown

/// Without a cooldown, so that only `once` holds it back.
#[test]
fn a_once_candidate_is_carried_exactly_once() {
    if !shipped() {
        return;
    }
    let mut h = talky(&[("push", "candidate_cooldown", json!(0))]);
    sow(&mut h, "o", "say it once", EA, &[], "", true, 5);
    sow(&mut h, "r", "say it always", EA, &[], "", false, 1);
    let round = json!(EA);
    assert_eq!(
        carried(&mut h, &round, "s1", "t1", "hi"),
        "[say it once; say it always]"
    );
    let m = marks_of(&h);
    assert!(!m[0].1.is_empty(), "a used `once` has its used_at: {m:?}");
    assert!(m[0].2 > 0, "a carried candidate has its last_seq: {m:?}");
    assert!(m[1].1.is_empty(), "no used_at without `once`: {m:?}");
    for n in 2..=4 {
        assert_eq!(
            carried(&mut h, &round, "s1", &format!("t{n}"), "hi"),
            "[say it always]",
            "a used `once` is never carried again"
        );
    }
}

/// `candidate_cooldown` 5: carried on the first turn, not on the next four,
/// again on the sixth -- the turns counted are the person's turns the round
/// saw after it was carried, as `./intake` files them. Each showing is a
/// pair of its own (its own id), though the part is the same.
#[test]
fn a_candidate_waits_its_cooldown() {
    if !shipped() {
        return;
    }
    let mut h = talky(&[]);
    sow(&mut h, "r", "keep in mind", EA, &[], "", false, 5);
    let round = json!(EA);
    // Lower-case words without a number: the question without candidates is
    // the words themselves, whatever the wall holds.
    let names = ["one", "two", "three", "four", "five", "six", "seven"];
    let mut carried_on = Vec::new();
    let mut ids = Vec::new();
    for (n, name) in (1..=7).zip(names) {
        let turn = format!("t{n}");
        let words = format!("turn {name}");
        match carried(&mut h, &round, "s1", &turn, &words).as_str() {
            "" => {}
            "[keep in mind]" => {
                carried_on.push(n);
                ids.push(shown(&h, "s1", &turn).expect("a pair").id);
            }
            other => panic!("{turn}: an unexpected part {other:?}"),
        }
        // The person's words reach the wall after the ask, as the round does.
        h.row_under(
            Some(EA),
            chrono::Utc::now(),
            "s1",
            &turn,
            "user",
            &user(&words),
            0,
        );
    }
    assert_eq!(carried_on, [1, 6], "not again within 5 turns");
    assert_ne!(ids[0], ids[1], "every showing is a pair of its own");
}

// ============================================================ the budget

/// Whole candidates, best first: with 40 characters -- a twentieth of the
/// window this hive serves, `input_soft` 267 tokens (R-IG-1) -- the
/// priority-9 text (25 with its brackets) goes, the priority-8 one would not
/// fit beside it and stays out whole, said behind the part with its length,
/// the short priority-1 one still fits. Only what was carried is marked.
#[test]
fn the_budget_cuts_by_priority() {
    if !shipped() {
        return;
    }
    let mut h = talky(&[("push", "candidate_cooldown", json!(0))]);
    h.db.execute(
        "INSERT INTO state (key, value) VALUES ('input_soft', '267')",
        [],
    )
    .unwrap();
    sow(
        &mut h,
        "p9",
        "alpha alpha alpha alpha",
        EA,
        &[],
        "",
        false,
        9,
    );
    sow(
        &mut h,
        "p8",
        "beta beta beta beta beta",
        EA,
        &[],
        "",
        false,
        8,
    );
    sow(&mut h, "p1", "gamma", EA, &[], "", false, 1);
    let part = carried(&mut h, &json!(EA), "s1", "t1", "hi");
    assert_eq!(
        part,
        "[alpha alpha alpha alpha; gamma] ...[dropped: 1 candidates (24 chars) over budget]"
    );
    let (whole, _) = part.split_once(" ...[dropped:").expect("the mark");
    assert!(
        whole.chars().count() <= 40,
        "the part keeps its budget: {part}"
    );
    let marked: Vec<(String, bool)> = marks_of(&h)
        .into_iter()
        .map(|(id, _, seq)| (id, seq > 0))
        .collect();
    assert_eq!(
        marked,
        [
            ("p1".to_string(), true),
            ("p8".to_string(), false),
            ("p9".to_string(), true)
        ]
    );
}

#[test]
fn candidate_push_zero_switches_them_off() {
    if !shipped() {
        return;
    }
    let mut h = talky(&[("push", "candidate_push", json!("0"))]);
    sow(&mut h, "o", "say it once", ALL, &[], "", true, 9);
    h.ledger_ops.clear();
    assert_eq!(carried(&mut h, &json!(EA), "s1", "t1", "hi"), "");
    assert_eq!(candidate_ops(&h), Vec::<Value>::new(), "not even a read");
    assert_eq!(marks_of(&h)[0].1, "", "nothing used");
}

// ============================================================ the question

/// The pair is the whole difference: a turn that carries candidates asks the
/// memory byte for byte as the same turn of a hive without any -- the
/// question `curator_push.rs` `the_push_follows_the_role` pins, and the whole
/// message, hop and body.
#[test]
fn a_carried_candidate_leaves_the_question_alone() {
    if !shipped() {
        return;
    }
    let round = json!(EA);
    let mut sent = Vec::new();
    for with in [false, true] {
        let mut h = talky(&[]);
        if with {
            sow(&mut h, "o", "Lisbon is far", EA, &[], "", true, 9);
            sow(&mut h, "w", "Ask Rita", EA, &["where"], "", false, 5);
        }
        let (first, _) = turn_as(
            &mut h,
            &round,
            "s1",
            "t1",
            "My sister Hannah moved to Porto.",
            "Porto, how lovely.",
        );
        assert_eq!(first, "My sister Hannah moved to Porto.", "with {with}");
        let asks = ask_as(&mut h, &round, "s1", "t2", "where again?");
        assert_eq!(asks.len(), 1, "with {with}: the ask always leaves");
        assert_eq!(
            asks[0].hop["recall_query"], "[mentioned: Hannah, Porto] where again?",
            "with {with}: the question of curator 1.5.0"
        );
        let parts: Vec<String> = ["t1", "t2"]
            .iter()
            .map(|t| shown(&h, "s1", t).map(|s| s.part).unwrap_or_default())
            .collect();
        if with {
            assert_eq!(parts, ["[Lisbon is far]", "[Ask Rita]"], "the pairs");
        } else {
            assert_eq!(parts, ["", ""], "no candidates, no pair");
        }
        sent.push(json!({"hop": asks[0].hop, "body": asks[0].body}));
    }
    assert_eq!(sent[1], sent[0], "byte for byte the ask without candidates");
}

// ============================================================ nothing to carry

/// A turn that carries no candidate leaves exactly as curator 1.5.0 sent it
/// -- the question `curator_push.rs` `the_push_follows_the_role` pins, and
/// the whole message, hop and body -- whether the table is empty, holds only
/// rows that do not pass (another round, ended, used, a trigger not said),
/// the candidates are off, or the ledger has no such table at all (the read
/// is refused and the ask leaves all the same). None of them writes a row or
/// hands `./intake` a pair.
#[test]
fn a_turn_without_candidates_leaves_as_before() {
    if !shipped() {
        return;
    }
    let round = json!(EA);
    let mut sent = Vec::new();
    for track in ["empty", "passing none", "off", "no table"] {
        let mut h = if track == "off" {
            talky(&[("push", "candidate_push", json!("0"))])
        } else {
            talky(&[])
        };
        match track {
            "passing none" => {
                sow(&mut h, "eb", "for e and b", EB, &[], "", false, 9);
                sow(&mut h, "e", "for e alone", E_ONLY, &[], "", false, 9);
                sow(
                    &mut h,
                    "old",
                    "ended",
                    EA,
                    &[],
                    "2000-01-01T00:00:00Z",
                    false,
                    9,
                );
                sow(&mut h, "fw", "ticket 7", EA, &["Firewall X"], "", false, 9);
                sow(&mut h, "used", "used", EA, &[], "", true, 9);
                h.db.execute(
                    "UPDATE candidates SET used_at = '2026-10-01T10:00:00.000000Z' \
                     WHERE cand_id = 'used'",
                    [],
                )
                .unwrap();
            }
            "no table" => {
                h.db.execute_batch("DROP TABLE IF EXISTS candidates")
                    .unwrap();
                h.ledger_may_refuse = true;
            }
            _ => {}
        }
        turn_as(
            &mut h,
            &round,
            "s1",
            "t1",
            "My sister Hannah moved to Porto.",
            "Porto, how lovely.",
        );
        let asks = ask_as(&mut h, &round, "s1", "t2", "where again?");
        assert_eq!(asks.len(), 1, "{track}: the ask always leaves: {:?}", h.out);
        assert_eq!(
            asks[0].hop["recall_query"], "[mentioned: Hannah, Porto] where again?",
            "{track}: the question of curator 1.5.0"
        );
        for t in ["t1", "t2"] {
            assert!(
                shown(&h, "s1", t).is_none(),
                "{track}/{t}: no candidate, no pair"
            );
        }
        let writes: Vec<Value> = candidate_ops(&h)
            .into_iter()
            .filter(|op| op["operation"] != "select")
            .collect();
        assert_eq!(writes, Vec::<Value>::new(), "{track}: nothing written");
        if track == "off" {
            assert_eq!(candidate_ops(&h), Vec::<Value>::new(), "off: no read");
        }
        sent.push((track, json!({"hop": asks[0].hop, "body": asks[0].body})));
    }
    for (track, msg) in &sent[1..] {
        assert_eq!(
            msg, &sent[0].1,
            "{track}: byte for byte the empty table's ask"
        );
    }
}
