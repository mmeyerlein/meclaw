//! GH #949 -- a push candidate never reaches further than the round it came in.
//!
//! The curator takes push candidates on ONE lane, `in_candidate` (`./intake`):
//! `{source, candidates: [{id, text, audience_set?, triggers?, until?, once?,
//! priority?}], withdraw?: [id]}`. The round is the message's
//! `context.audience_set`, and it is fail-closed: without one nothing is
//! written (`missing_audience`). A candidate without an audience of its own
//! takes the round; one with an audience must stay inside it -- every member it
//! names is in the round, `*` only when the round holds `*`, the empty set (the
//! round-less conversation's) only in an empty round -- or it alone is refused
//! (`audience_widened`). A field out of its bounds refuses its candidate
//! (`invalid_input`), a broken body the whole message.
//!
//! Each kept candidate is a block of kind `candidate` and ONE row of the
//! ledger table `candidates` per `(source, id)`, written again in place: when
//! its text, `once` or triggers move, the push's own marks on it (`used_at`,
//! `last_seq`) start over, otherwise they stay. `withdraw` ends one now. The
//! receipt `candidate_ack` leaves only when the sender asked for it (`hop.ack`
//! "1"): three curators behind one fan-out must not leave three dead letters
//! per push.
//!
//! Also here (C1 finding W M-6/M-7): the tap's state read in `./intake` names
//! only the parked tap. Since GH #943 the policy keeps a round's last and armed
//! call as `last_call:<rk>` / `armed_call:<rk>`; the bare keys that read still
//! asked for were read by nobody.
//!
//! The hive runs in one process (`support/curator_hive.rs`): the shipped
//! scripts, the shipped edges, the ledger an in-memory SQLite driven by the
//! store's own dispatcher. Guarded like every template-reading test (GH #49).

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// The rounds as TEXT, out of canonical order -- the form the colony carries
/// them in -- and as the ledger keeps them.
const ROUND_EA: &str = r#"["member:e","member:a"]"#;
const CANON_EA: &str = r#"["member:a","member:e"]"#;
const CANON_E: &str = r#"["member:e"]"#;
const STAR: &str = r#"["*"]"#;
const SOURCE: &str = "probe-app";

/// One message on the door; the hive's earlier output is cleared first.
fn door(h: &mut Hive, round: Value, hop: Value, body: Value) {
    h.out.clear();
    h.lane("in_candidate", json!({"audience_set": round}), hop, body);
}

/// `door` asking for the receipt, which must be the one message that leaves.
fn door_acked(h: &mut Hive, round: &str, body: Value) -> Msg {
    door(h, json!(round), json!({"ack": "1"}), body);
    let acks = h.routed("candidate_ack");
    assert_eq!(
        acks.len(),
        1,
        "one push asked for its receipt, one receipt: {:?} {:?}",
        h.out,
        h.stderr
    );
    acks[0].clone()
}

/// `(id, error_code)` of every refusal a receipt names, in body order.
fn refused(ack: &Msg) -> Vec<(String, String)> {
    ack.body["refused"]
        .as_array()
        .unwrap_or_else(|| panic!("`refused` is a list: {:?}", ack.body))
        .iter()
        .map(|r| {
            (
                r["id"].as_str().unwrap_or("").to_string(),
                r["error_code"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// One row of `candidates`, with the text of the block it names.
#[derive(Debug, Clone, PartialEq)]
struct Row {
    source: String,
    id: String,
    text: String,
    audience: String,
    triggers: Value,
    until: String,
    once: i64,
    priority: i64,
    used_at: String,
    last_seq: i64,
}

fn ledger(h: &Hive) -> Vec<Row> {
    h.rows(
        "SELECT c.source, c.cand_id, c.hash, b.kind, b.body, c.audience_set, c.triggers, \
         c.until, c.once, c.priority, c.used_at, c.last_seq \
         FROM candidates c LEFT JOIN blocks b ON b.hash = c.hash \
         ORDER BY c.source, c.cand_id",
    )
    .into_iter()
    .map(|r| {
        let s = |i: usize| r[i].as_str().unwrap_or("").to_string();
        // The text is a block like a pin's (GH #892): kind `candidate`, the
        // canonical JSON of `{source, text, type}`, keyed by its sha256 --
        // what `./push` reads it back by.
        assert_eq!(s(3), "candidate", "the block of a candidate: {r:?}");
        let body: Value = sj::from_str(&s(4)).expect("the block body is JSON");
        assert_eq!(
            body,
            json!({"source": s(0), "text": body["text"], "type": "candidate"}),
            "the block element: {r:?}"
        );
        assert_eq!(
            s(2),
            sha256_hex(&canonical(&body)),
            "the row names its block"
        );
        Row {
            source: s(0),
            id: s(1),
            text: body["text"].as_str().unwrap_or("").to_string(),
            audience: s(5),
            triggers: sj::from_str(&s(6)).expect("the triggers are a JSON list"),
            until: s(7),
            once: r[8].as_i64().expect("once is a number"),
            priority: r[9].as_i64().expect("priority is a number"),
            used_at: s(10),
            last_seq: r[11].as_i64().expect("last_seq is a number"),
        }
    })
    .collect()
}

fn row(h: &Hive, id: &str) -> Row {
    ledger(h)
        .into_iter()
        .find(|r| r.source == SOURCE && r.id == id)
        .unwrap_or_else(|| panic!("no candidate {id}: {:?}", ledger(h)))
}

fn ids(h: &Hive) -> Vec<String> {
    ledger(h).into_iter().map(|r| r.id).collect()
}

/// The ledger operations `./intake` sent since the last clear.
fn intake_ops(h: &Hive) -> Vec<Value> {
    h.ledger_ops
        .iter()
        .filter(|(from, _)| from == "intake")
        .map(|(_, a)| a.clone())
        .collect()
}

/// The push's own marks on a candidate, set the way `./push` sets them.
fn mark_shown(h: &Hive, id: &str) {
    h.db.execute(
        "UPDATE candidates SET used_at = '2026-01-01T00:00:00.000000Z', last_seq = 7 \
         WHERE cand_id = ?1",
        [id],
    )
    .unwrap();
}

#[test]
fn without_a_round_nothing_is_written() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let body = json!({"source": SOURCE,
                      "candidates": [{"id": "c1", "text": "Ask how the move went."}]});
    // Asked for its receipt: the refusal of the whole body, nothing kept.
    for round in [Value::Null, json!("")] {
        door(&mut h, round.clone(), json!({"ack": "1"}), body.clone());
        let acks = h.routed("candidate_ack");
        assert_eq!(
            acks.len(),
            1,
            "a round-less push that asked for its receipt gets one (round {round}): {:?} {:?}",
            h.out,
            h.stderr
        );
        assert_eq!(acks[0].hop["error_code"], "missing_audience");
        assert_eq!(acks[0].body["source"], SOURCE);
        assert_eq!(acks[0].body["stored"], json!(0));
        assert_eq!(acks[0].body["refused"], json!([]));
        assert_eq!(h.out.len(), 1, "the receipt and nothing else: {:?}", h.out);
    }
    // Not asked: nothing leaves, and the refusal is said.
    h.stderr.clear();
    door(&mut h, Value::Null, json!({}), body);
    assert!(h.out.is_empty(), "no receipt nobody asked for: {:?}", h.out);
    assert!(
        h.stderr.iter().any(|l| l.contains("missing_audience")),
        "the refusal is said: {:?}",
        h.stderr
    );
    // Nothing of any of them reached the ledger.
    assert!(
        intake_ops(&h).is_empty(),
        "a round-less push parks nothing and reads nothing: {:?}",
        intake_ops(&h)
    );
    assert!(ledger(&h).is_empty());
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM blocks WHERE kind = 'candidate'")[0][0],
        json!(0)
    );
}

#[test]
fn a_candidate_takes_or_narrows_its_round_and_never_widens_it() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let ack = door_acked(
        &mut h,
        ROUND_EA,
        json!({"source": SOURCE, "candidates": [
            {"id": "wide", "text": "For e, a and c.",
             "audience_set": ["member:e", "member:a", "member:c"]},
            {"id": "narrow", "text": "For e alone.", "audience_set": ["member:e"]},
            {"id": "plain", "text": "For the round."},
            {"id": "star", "text": "For everyone.", "audience_set": ["*"]},
            {"id": "nobody", "text": "For the round-less.", "audience_set": []},
            {"id": "as-text", "text": "The round as text.",
             "audience_set": "[\"member:a\",\"member:e\"]"},
            {"id": "other", "text": "For b.", "audience_set": ["member:b"]}
        ]}),
    );
    assert_eq!(ack.hop["error_code"], "", "the body itself holds");
    assert_eq!(ack.body["stored"], json!(3), "{:?}", ack.body);
    let widened = |id: &str| (id.to_string(), "audience_widened".to_string());
    assert_eq!(
        refused(&ack),
        vec![
            widened("wide"),
            widened("star"),
            widened("nobody"),
            widened("other")
        ],
        "a candidate naming anyone outside the round -- `*` in a round without it, the \
         round-less audience in a declared round -- is refused, it alone"
    );
    assert_eq!(ids(&h), ["as-text", "narrow", "plain"]);
    assert_eq!(
        row(&h, "narrow").audience,
        CANON_E,
        "narrower stays narrower"
    );
    assert_eq!(
        row(&h, "plain").audience,
        CANON_EA,
        "none of its own: the round"
    );
    assert_eq!(
        row(&h, "as-text").audience,
        CANON_EA,
        "canonical, like a pin's"
    );
    for text in [
        "For e, a and c.",
        "For everyone.",
        "For the round-less.",
        "For b.",
    ] {
        assert!(
            h.rows(&format!(
                "SELECT hash FROM blocks WHERE body LIKE '%{text}%'"
            ))
            .is_empty(),
            "a refused candidate leaves no block: {text}"
        );
    }
    // A round that holds `*` may name anyone, `*` included.
    let ack = door_acked(
        &mut h,
        STAR,
        json!({"source": SOURCE, "candidates": [
            {"id": "star", "text": "For everyone.", "audience_set": ["*"]},
            {"id": "inherits-star", "text": "Takes the star round."},
            {"id": "e-under-star", "text": "For e under a star round.",
             "audience_set": ["member:e"]}
        ]}),
    );
    assert_eq!(ack.body["stored"], json!(3), "{:?}", ack.body);
    assert_eq!(refused(&ack), Vec::<(String, String)>::new());
    assert_eq!(row(&h, "star").audience, STAR);
    assert_eq!(row(&h, "inherits-star").audience, STAR);
    assert_eq!(row(&h, "e-under-star").audience, CANON_E);
}

#[test]
fn the_bounds_refuse_a_candidate_or_the_whole_body() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let c = |id: &str, extra: Value| -> Value {
        let mut v = json!({"id": id, "text": "Say this."});
        for (k, x) in extra.as_object().cloned().unwrap_or_default() {
            v[k] = x;
        }
        v
    };
    let id64 = "i".repeat(64);
    let id65 = "i".repeat(65);
    let sixteen: Vec<String> = (0..16).map(|i| format!("word{i}")).collect();
    let seventeen: Vec<String> = (0..17).map(|i| format!("word{i}")).collect();
    let body = json!({"source": SOURCE, "candidates": [
        c(&id64, json!({})),
        c(&id65, json!({})),
        c("", json!({})),
        {"text": "No id at all."},
        c("empty", json!({"text": ""})),
        c("blank", json!({"text": "   "})),
        c("text-600", json!({"text": "t".repeat(600)})),
        c("text-601", json!({"text": "t".repeat(601)})),
        {"id": "no-text"},
        c("trig-16", json!({"triggers": sixteen})),
        c("trig-17", json!({"triggers": seventeen})),
        c("trig-64", json!({"triggers": ["w".repeat(64)]})),
        c("trig-65", json!({"triggers": ["w".repeat(65)]})),
        c("trig-punct", json!({"triggers": ["?!"]})),
        c("trig-dots", json!({"triggers": ["firewall", "..."]})),
        c("trig-blank", json!({"triggers": [" "]})),
        c("trig-not-list", json!({"triggers": "firewall"})),
        c("prio-0", json!({"priority": 0})),
        c("prio-9", json!({"priority": 9})),
        c("prio-10", json!({"priority": 10})),
        c("prio-neg", json!({"priority": -1})),
        c("prio-text", json!({"priority": "5"})),
        c("once-text", json!({"once": "yes"})),
        c("until-bad", json!({"until": "tomorrow"})),
        c("aud-bad", json!({"audience_set": "not json"})),
        "just a sentence"
    ]});
    let ack = door_acked(&mut h, ROUND_EA, body);
    assert_eq!(ack.hop["error_code"], "", "per candidate, not the body");
    let bad = |id: &str| (id.to_string(), "invalid_input".to_string());
    assert_eq!(
        refused(&ack),
        vec![
            // An over-long id is named back cut to its bound.
            bad(&id64),
            bad(""),
            bad(""),
            bad("empty"),
            bad("blank"),
            bad("text-601"),
            bad("no-text"),
            bad("trig-17"),
            bad("trig-65"),
            bad("trig-punct"),
            bad("trig-dots"),
            bad("trig-blank"),
            bad("trig-not-list"),
            bad("prio-10"),
            bad("prio-neg"),
            bad("prio-text"),
            bad("once-text"),
            bad("until-bad"),
            bad("aud-bad"),
            bad("")
        ]
    );
    assert_eq!(ack.body["stored"], json!(6));
    let mut want = vec![
        id64.clone(),
        "prio-0".to_string(),
        "prio-9".to_string(),
        "text-600".to_string(),
        "trig-16".to_string(),
        "trig-64".to_string(),
    ];
    want.sort();
    assert_eq!(ids(&h), want, "exactly the candidates inside their bounds");
    assert_eq!(row(&h, "prio-0").priority, 0);
    assert_eq!(row(&h, "prio-9").priority, 9);
    assert_eq!(row(&h, "trig-16").triggers.as_array().unwrap().len(), 16);

    // A broken body refuses the whole message: nothing parked, nothing read.
    let before = ledger(&h);
    let many: Vec<Value> = (0..33)
        .map(|i| json!({"id": format!("m{i}"), "text": "One of many."}))
        .collect();
    for (why, body) in [
        (
            "33 candidates",
            json!({"source": SOURCE, "candidates": many}),
        ),
        (
            "the model's own source",
            json!({"source": "model", "candidates": [c("x", json!({}))]}),
        ),
        (
            "a source that is no name",
            json!({"source": "a/b", "candidates": [c("x", json!({}))]}),
        ),
        ("no source", json!({"candidates": [c("x", json!({}))]})),
        (
            "candidates not a list",
            json!({"source": SOURCE, "candidates": "x"}),
        ),
        (
            "withdraw not a list",
            json!({"source": SOURCE, "withdraw": "x"}),
        ),
        ("neither list", json!({"source": SOURCE})),
    ] {
        h.ledger_ops.clear();
        let ack = door_acked(&mut h, ROUND_EA, body.clone());
        assert_eq!(ack.hop["error_code"], "invalid_input", "{why}");
        assert_eq!(ack.body["stored"], json!(0), "{why}");
        assert!(intake_ops(&h).is_empty(), "{why}: {:?}", intake_ops(&h));
        // Not asked, the same refusal leaves nothing behind.
        door(&mut h, json!(ROUND_EA), json!({}), body);
        assert!(h.out.is_empty(), "{why}: {:?}", h.out);
    }
    assert_eq!(ledger(&h), before, "no broken body wrote a row");
}

#[test]
fn defaults_a_repeated_id_and_a_past_until() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let ack = door_acked(
        &mut h,
        ROUND_EA,
        json!({"source": SOURCE, "candidates": [
            {"id": "plain", "text": "No trigger."},
            {"id": "trig", "text": "With triggers.", "triggers": ["Firewall X", "router"]},
            {"id": "trig-once", "text": "Once, with a trigger.", "triggers": ["x1"],
             "once": true},
            {"id": "dup", "text": "first"},
            {"id": "past", "text": "Over already.", "until": "2020-01-01T00:00:00Z"},
            {"id": "dup", "text": "second"},
            {"id": "dup-bad", "text": "kept?"},
            {"id": "dup-bad", "text": ""}
        ]}),
    );
    assert_eq!(ack.body["stored"], json!(5), "{:?}", ack.body);
    assert_eq!(
        refused(&ack),
        vec![("dup-bad".to_string(), "invalid_input".to_string())],
        "a repeated id is its LAST entry, refused when that one is"
    );
    let plain = row(&h, "plain");
    assert_eq!(
        (
            plain.once,
            plain.priority,
            plain.triggers.clone(),
            plain.until.clone()
        ),
        (1, 5, json!([]), String::new()),
        "without triggers once, priority 5, open"
    );
    assert_eq!(
        (plain.used_at.as_str(), plain.last_seq),
        ("", 0),
        "never shown"
    );
    let trig = row(&h, "trig");
    assert_eq!(trig.once, 0, "with triggers it may come again");
    assert_eq!(
        trig.triggers,
        json!(["Firewall X", "router"]),
        "raw as given"
    );
    assert_eq!(row(&h, "trig-once").once, 1, "`once` as the sender set it");
    assert_eq!(row(&h, "dup").text, "second", "the last entry wins");
    assert_eq!(
        ledger(&h).iter().filter(|r| r.id == "dup").count(),
        1,
        "one row per id"
    );
    assert_eq!(
        row(&h, "past").until,
        "2020-01-01T00:00:00Z",
        "an `until` in the past is kept as given -- the push never shows it"
    );
    assert!(
        ledger(&h).iter().all(|r| r.id != "dup-bad"),
        "nothing of a refused last entry"
    );
}

#[test]
fn pushed_again_it_is_one_row_that_keeps_its_marks_unless_it_moved() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let first = json!({"id": "c1", "text": "Ask about the trip.", "triggers": ["trip"],
                       "priority": 3});
    let push = |h: &mut Hive, round: &str, c: Value| {
        let ack = door_acked(h, round, json!({"source": SOURCE, "candidates": [c]}));
        assert_eq!(ack.body["stored"], json!(1), "{:?}", ack.body);
    };
    push(&mut h, ROUND_EA, first.clone());
    mark_shown(&h, "c1");
    // The same again: nothing to write, the push's marks stay.
    h.ledger_ops.clear();
    push(&mut h, ROUND_EA, first.clone());
    assert!(
        intake_ops(&h)
            .iter()
            .all(|a| a["table"] != "candidates" || a["operation"] == "select"),
        "the same candidate again writes nothing: {:?}",
        intake_ops(&h)
    );
    let kept = |h: &Hive| {
        let r = row(h, "c1");
        (r.used_at, r.last_seq)
    };
    let marks = ("2026-01-01T00:00:00.000000Z".to_string(), 7);
    let fresh = (String::new(), 0);
    assert_eq!(kept(&h), marks);
    // Priority and round move the row, not its marks.
    let mut v = first.clone();
    v["priority"] = json!(8);
    push(&mut h, ROUND_EA, v.clone());
    assert_eq!(row(&h, "c1").priority, 8);
    assert_eq!(kept(&h), marks, "a new priority keeps the marks");
    push(&mut h, ROUND_E, v.clone());
    assert_eq!(
        row(&h, "c1").audience,
        CANON_E,
        "the latest door decides the round"
    );
    assert_eq!(kept(&h), marks, "a new round keeps the marks");
    // Text, triggers or `once` make it a new candidate to the push.
    v["text"] = json!("Ask about the trip to the coast.");
    push(&mut h, ROUND_E, v.clone());
    assert_eq!(row(&h, "c1").text, "Ask about the trip to the coast.");
    assert_eq!(kept(&h), fresh, "a new text starts over");
    mark_shown(&h, "c1");
    v["triggers"] = json!(["trip", "coast"]);
    push(&mut h, ROUND_E, v.clone());
    assert_eq!(kept(&h), fresh, "new triggers start over");
    mark_shown(&h, "c1");
    v["once"] = json!(true);
    push(&mut h, ROUND_E, v.clone());
    assert_eq!(row(&h, "c1").once, 1);
    assert_eq!(kept(&h), fresh, "a new `once` starts over");
    assert_eq!(
        ledger(&h).len(),
        1,
        "one row per (source, id): {:?}",
        ledger(&h)
    );
    // The same id of another source is another candidate.
    let ack = door_acked(
        &mut h,
        ROUND_E,
        json!({"source": "other-app", "candidates": [first]}),
    );
    assert_eq!(ack.body["stored"], json!(1));
    assert_eq!(ledger(&h).len(), 2);
    // The store holds the key too: one unique index over (source, cand_id).
    let index = &cell_config("ledger")["params"]["indexes"];
    let on_candidates: Vec<&Value> = index
        .as_object()
        .expect("named indexes")
        .values()
        .filter(|i| i["table"] == "candidates")
        .collect();
    assert_eq!(
        on_candidates,
        [&json!({"table": "candidates", "on": ["source", "cand_id"], "unique": true})],
        "the ledger keys a candidate by (source, cand_id)"
    );
}

#[test]
fn withdraw_ends_a_candidate_now() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    door_acked(
        &mut h,
        ROUND_EA,
        json!({"source": SOURCE, "candidates": [
            {"id": "c1", "text": "One."}, {"id": "c2", "text": "Two."}]}),
    );
    let before = chrono::Utc::now();
    let ack = door_acked(
        &mut h,
        ROUND_EA,
        json!({"source": SOURCE, "candidates": [], "withdraw": ["c1", "never-pushed"]}),
    );
    assert_eq!(ack.hop["error_code"], "");
    let until = row(&h, "c1").until;
    let at = chrono::DateTime::parse_from_rfc3339(&until)
        .unwrap_or_else(|e| panic!("`until` is a time ({e}): {until}"))
        .with_timezone(&chrono::Utc);
    assert!(
        at >= before - chrono::Duration::seconds(1) && at <= chrono::Utc::now(),
        "withdrawn means over now: {until}"
    );
    assert_eq!(row(&h, "c2").until, "", "the other one stands");
    assert_eq!(ids(&h), ["c1", "c2"], "withdraw writes no row of its own");
    // A body of nothing but `withdraw` is a body.
    door_acked(
        &mut h,
        ROUND_EA,
        json!({"source": SOURCE, "withdraw": ["c2"]}),
    );
    assert_ne!(row(&h, "c2").until, "");
    // Withdrawn in the message it came in: kept, and over at once.
    door_acked(
        &mut h,
        ROUND_EA,
        json!({"source": SOURCE, "candidates": [{"id": "c3", "text": "Three."}],
               "withdraw": ["c3"]}),
    );
    assert_ne!(row(&h, "c3").until, "");
}

#[test]
fn the_receipt_leaves_only_when_asked_in_the_declared_form() {
    if !shipped() {
        return;
    }
    let hive = read_json(&repo("templates/curator/config.json"));
    let lanes = |k: &str| -> Vec<String> {
        hive["params"]["contract"][k]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l["route"].as_str().unwrap().to_string())
            .collect()
    };
    assert!(lanes("accepts").contains(&"in_candidate".to_string()));
    assert!(lanes("emits").contains(&"candidate_ack".to_string()));
    let mut h = Hive::new();
    let body = json!({"source": SOURCE, "candidates": [{"id": "c1", "text": "One."}]});
    for hop in [json!({}), json!({"ack": "0"}), json!({"ack": ""})] {
        door(&mut h, json!(ROUND_EA), hop.clone(), body.clone());
        assert!(h.out.is_empty(), "{hop}: no receipt unasked: {:?}", h.out);
    }
    assert_eq!(ids(&h), ["c1"], "kept all the same");
    let ack = door_acked(&mut h, ROUND_EA, body);
    assert_eq!(ack.route(), "candidate_ack");
    assert_eq!(ack.hop["error_code"], "");
    let mut keys: Vec<&str> = ack.body.keys().map(String::as_str).collect();
    keys.sort();
    assert_eq!(keys, ["messages", "refused", "source", "stored"]);
    assert_eq!(ack.body["source"], SOURCE);
    // What `./intake` declares it emits (the colony holds a cell to it).
    let emits = &cell_config("intake")["contract"]["emits"];
    assert!(
        emits["hop"]["route"]["values"]
            .as_array()
            .unwrap()
            .contains(&json!("candidate_ack")),
        "intake declares the route"
    );
    for k in &keys {
        assert!(emits["body"][*k].is_object(), "intake declares body.{k}");
    }
    assert!(
        emits["hop"]["error_code"].is_object(),
        "intake declares hop.error_code"
    );
    // The round rides on, untouched.
    assert_eq!(ack.context["audience_set"], ROUND_EA);
}

#[test]
fn the_tap_reads_only_the_parked_tap() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = turn(&mut h, "s1", "t1", "Hello there.", "Hi.", json!({}));
    let reads: Vec<Value> = intake_ops(&h)
        .into_iter()
        .filter(|a| a["operation"] == "select" && a["table"] == "state")
        .collect();
    let named: Vec<String> = reads
        .iter()
        .flat_map(|a| match &a["where"]["key"] {
            Value::String(s) => vec![s.clone()],
            other => other["in"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|k| k.as_str().unwrap_or("").to_string())
                .collect(),
        })
        .collect();
    assert!(
        named.iter().any(|k| k.starts_with("pending:tap-")),
        "the tap still reads its parked payload: {named:?}"
    );
    for bare in ["armed_call", "last_call"] {
        assert!(
            !named.iter().any(|k| k == bare),
            "`./intake` asks the state for the bare `{bare}`, which nobody reads since \
             GH #943 keyed it by round: {reads:?}"
        );
    }
    // The policy still records the round's last call off the tap.
    assert_eq!(
        h.state_in("last_call", ROUND_E),
        call.hop["curator_call"].as_str().unwrap()
    );
}
