//! GH #1225 (P1, R-KF-4/5, OR-KF-8) -- a quote is checked at its own source.
//!
//! WHY this file exists: `sidecar_verify` looked for a quote only in the
//! user/peer rows of the running turn. A round of advice, a delegation, a
//! timer or an app has none of those, so every fact of it was
//! `quote_not_in_source` and, under `mode` "drop", gone without a trace; had
//! it passed, the memory filed it under the person whatever it quoted -- the
//! advice of the reasoning core and the model's own promises included. Now an
//! item names its source (`person`, `peer:<ref>`, `agent:<name>`) and the rule
//! token `source` checks the quote in the rows of exactly that source: the
//! person's rows of the turn and of the episode the turn answers, a peer's
//! rows by `speaker_ref`, the advice rows for `agent:cogny`, the framed rows
//! of a timer or an app, the answer and the earlier final answers for
//! `agent:self`. A frame counts only with an id in the form of a name
//! (`[app garden]`); a person who merely starts with `[timer ]` stays the
//! person. A missing, unknown or wrong source is refused with its reason
//! and a `verify_reject` mark -- never re-filed, never silent -- and a failed
//! read of the rows is `store_error`, never a missing quote. These are the
//! intake's unit locks; the road through the splitter and the memory is
//! pinned by `gh1225`'s extract-glue locks.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The rule a talky stamps when `memory` checks its facts at their source.
const RULE: &str = r#"{"items":"facts","quote":"source"}"#;
const ADVICE: &str = "[advice from your reasoning core, consult c1]\nThe person might enjoy a trip to the Baltic Sea this summer.";

fn hive() -> Hive {
    Hive::with(&[("intake", "pass_sections", json!("memory"))])
}

/// The `memory` section of the answer to `call`, stamped with [`RULE`] and
/// the answer text, as the talky edge hands it to `in_section`.
fn section_in(h: &mut Hive, call: &Msg, payload: Value, answer: &str) {
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
    h.out.clear();
    h.lane(
        "in_section",
        Value::Object(ctx),
        json!({"section": "memory", "turn_id": call.hop["turn_id"], "sidecar_verify": RULE}),
        json!({"messages": [], "section": "memory", "payload": payload,
               "verify_answer": answer}),
    );
}

fn fact(quote: &str, source: Option<&str>) -> Value {
    let mut f = json!({"predicate": "likes", "value": "x", "quote": quote});
    if let Some(s) = source {
        f["source"] = json!(s);
    }
    f
}

/// The quotes of the facts that passed, each with its `verified`.
fn passed(h: &Hive) -> Vec<(String, Value)> {
    let sides = h.routed("sidecar");
    assert_eq!(sides.len(), 1, "{:?} {:?}", h.out, h.stderr);
    sides[0].body["payload"]["facts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|f| {
            (
                f["quote"].as_str().unwrap_or("").to_string(),
                f["verified"].clone(),
            )
        })
        .collect()
}

fn reasons(h: &Hive) -> Vec<String> {
    h.routed("reject")
        .iter()
        .map(|m| m.hop["reject_reason"].as_str().unwrap_or("").to_string())
        .collect()
}

/// The `verify_reject` marks, as their JSON values, in write order.
fn reject_marks(h: &Hive) -> Vec<Value> {
    h.rows("SELECT value FROM marks WHERE kind = 'verify_reject' ORDER BY seq")
        .into_iter()
        .map(|r| meclaw_core::serde_json::from_str(r[0].as_str().unwrap()).unwrap())
        .collect()
}

fn q(s: &str) -> (String, Value) {
    (s.to_string(), json!(["quote"]))
}

#[test]
fn an_advice_round_files_each_quote_under_its_own_source() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    turn(
        &mut h,
        "s1",
        "r1",
        "My daughter Anna moves to Hamburg in May.",
        "How lovely, a big step for her.",
        json!({}),
    );
    // The reasoning core's advice opens a round of its own: no person row.
    let call = h.curate("s1", "r2", 0, json!([user(ADVICE)]), mode("Be brief."));
    assert_ne!(call.hop["episode_turn_id"], json!(""), "{:?}", call.hop);
    let facts = json!([
        fact("Anna moves to Hamburg in May", Some("person")),
        fact("enjoy a trip to the Baltic Sea", Some("agent:cogny")),
        fact("enjoy a trip to the Baltic Sea", Some("person")),
        fact("Anna moves to Hamburg in May", Some("agent:cogny")),
        fact("Anna moves to Hamburg in May", None),
        fact("Anna moves to Hamburg in May", Some("agent:martian")),
        fact("Anna moves to Hamburg in May", Some("peer:0123abcd")),
        fact("a big step for her", Some("agent:self")),
        fact("maybe a trip to the Baltic Sea", Some("agent:self")),
        fact("maybe a trip to the Baltic Sea", Some("person")),
        fact("Anna moves to Hamburg in May", Some(" Person ")),
    ]);
    section_in(
        &mut h,
        &call,
        json!({"facts": facts, "topic": {"movement": "start", "name": "the move",
                                          "source": "person"}}),
        "Anna's move sounds exciting -- maybe a trip to the Baltic Sea?",
    );
    assert_eq!(
        passed(&h),
        [
            q("Anna moves to Hamburg in May"),
            q("enjoy a trip to the Baltic Sea"),
            q("a big step for her"),
            q("maybe a trip to the Baltic Sea"),
            q("Anna moves to Hamburg in May"),
        ],
        "the person's episode, the advice, the model's own words -- each at its source"
    );
    assert_eq!(
        reasons(&h),
        [
            "quote_not_in_source",
            "quote_not_in_source",
            "source_missing",
            "source_unknown",
            "source_unknown",
            "quote_not_in_source",
        ]
    );
    let marks = reject_marks(&h);
    assert_eq!(
        marks,
        [
            json!({"section": "memory", "field": "quote", "reason": "quote_not_in_source",
                   "source": "person", "item": 2}),
            json!({"section": "memory", "field": "quote", "reason": "quote_not_in_source",
                   "source": "agent:cogny", "item": 3}),
            json!({"section": "memory", "field": "quote", "reason": "source_missing",
                   "source": "", "item": 4}),
            json!({"section": "memory", "field": "quote", "reason": "source_unknown",
                   "source": "", "item": 5}),
            json!({"section": "memory", "field": "quote", "reason": "source_unknown",
                   "source": "peer:0123abcd", "item": 6}),
            json!({"section": "memory", "field": "quote", "reason": "quote_not_in_source",
                   "source": "person", "item": 9}),
        ]
    );
    let all = json!(marks).to_string();
    for w in ["Hamburg", "Baltic", "martian"] {
        assert!(!all.contains(w), "`{w}` left in a mark: {all}");
    }
    // The topic carries its source in its mark (OR-KF-8).
    assert_eq!(
        h.rows("SELECT value FROM marks WHERE kind = 'topic'"),
        vec![vec![json!(
            r#"{"movement":"start","name":"the move","source":"person"}"#
        )]]
    );
}

#[test]
fn a_peer_quote_needs_its_speaker() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let bea = json!({"origin": "peer", "type": "text", "text": "I grew up in Leipzig, near the zoo.",
                     "speaker": "Bea", "speaker_ref": "0123abcd"});
    let call = h.curate("s2", "p1", 0, json!([bea]), mode("Be brief."));
    section_in(
        &mut h,
        &call,
        json!({"facts": [
            fact("grew up in Leipzig", Some("peer:0123abcd")),
            fact("grew up in Leipzig", Some("0123abcd")),
            fact("grew up in Leipzig", Some("peer:89abcdef")),
            fact("grew up in Leipzig", Some("person")),
        ]}),
        "Leipzig is lovely.",
    );
    assert_eq!(
        passed(&h),
        [q("grew up in Leipzig"), q("grew up in Leipzig")],
        "the speaker's rows, the bare reference of before included"
    );
    assert_eq!(reasons(&h), ["source_unknown", "quote_not_in_source"]);
}

#[test]
fn a_timer_round_is_nobodys_words() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    turn(
        &mut h,
        "s3",
        "q1",
        "Remind me to water the roses every evening.",
        "I will remind you.",
        json!({}),
    );
    h.out.clear();
    let call = h.curate(
        "s3",
        "q2",
        0,
        json!([user("[timer t-7] Evening reminder: the roses need water.")]),
        mode("Be brief."),
    );
    // A framed timer turn is its own kind and never the person's episode.
    assert_eq!(
        h.rows("SELECT kind FROM wall WHERE turn_id = 'q2' AND kind <> 'assistant'"),
        vec![vec![json!("timer")]]
    );
    assert!(
        h.routed("turn_write")
            .iter()
            .all(|m| !json!(m.body).to_string().contains("roses need water")),
        "{:?}",
        h.routed("turn_write")
    );
    section_in(
        &mut h,
        &call,
        json!({"facts": [
            fact("the roses need water", Some("agent:timer:t-7")),
            fact("water the roses every evening", Some("person")),
            fact("the roses need water", Some("person")),
            fact("the roses need water", Some("agent:timer:t-8")),
            fact("the roses need water", Some("agent:app:t-7")),
        ]}),
        "Time for the roses.",
    );
    assert_eq!(
        passed(&h),
        [
            q("the roses need water"),
            q("water the roses every evening")
        ]
    );
    assert_eq!(
        reasons(&h),
        [
            "quote_not_in_source",
            "quote_not_in_source",
            "quote_not_in_source"
        ],
        "nothing found where the item says is refused, never silently"
    );
    assert_eq!(reject_marks(&h).len(), 3);
}

#[test]
fn an_app_round_is_nobodys_words() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    turn(
        &mut h,
        "s5",
        "g1",
        "Tell me when it is time to plant the tulips.",
        "I will.",
        json!({}),
    );
    h.out.clear();
    let call = h.curate(
        "s5",
        "g2",
        0,
        json!([user(
            "[app garden] Planting day: the tulips go in this week."
        )]),
        mode("Be brief."),
    );
    // A framed app turn is its own kind and never the person's episode.
    assert_eq!(
        h.rows("SELECT kind FROM wall WHERE turn_id = 'g2' AND kind <> 'assistant'"),
        vec![vec![json!("app")]]
    );
    assert!(
        h.routed("turn_write")
            .iter()
            .all(|m| !json!(m.body).to_string().contains("tulips go in")),
        "{:?}",
        h.routed("turn_write")
    );
    section_in(
        &mut h,
        &call,
        json!({"facts": [
            fact("the tulips go in this week", Some("agent:app:garden")),
            fact("the tulips go in this week", Some("person")),
            fact("the tulips go in this week", Some("agent:app:yard")),
            fact("the tulips go in this week", Some("agent:timer:garden")),
        ]}),
        "Tulips it is.",
    );
    assert_eq!(passed(&h), [q("the tulips go in this week")]);
    assert_eq!(
        reasons(&h),
        [
            "quote_not_in_source",
            "quote_not_in_source",
            "quote_not_in_source"
        ],
        "an app's words are the app's, never the person's or another frame's"
    );
    assert_eq!(reject_marks(&h).len(), 3);
}

#[test]
fn a_frame_without_a_name_stays_the_persons_words() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let call = h.curate(
        "s6",
        "k1",
        0,
        json!([
            user("[timer ] what is a kitchen timer?"),
            user("[app my garden] how deep do tulips go?")
        ]),
        mode("Be brief."),
    );
    // Only a frame whose id has the form of a name opens an app or timer
    // round; a person who merely starts with the prefix stays the person.
    assert_eq!(
        h.rows("SELECT kind FROM wall WHERE turn_id = 'k1' AND kind <> 'assistant' ORDER BY seq"),
        vec![vec![json!("user")], vec![json!("user")]]
    );
    let writes: Vec<String> = h
        .routed("turn_write")
        .iter()
        .map(|m| json!(m.body).to_string())
        .collect();
    let writes = writes.join("\n");
    assert!(
        writes.contains("what is a kitchen timer") && writes.contains("how deep do tulips go"),
        "{writes}"
    );
    section_in(
        &mut h,
        &call,
        json!({"facts": [
            fact("what is a kitchen timer", Some("person")),
            fact("how deep do tulips go", Some("person")),
            fact("what is a kitchen timer", Some("agent:timer:")),
        ]}),
        "About a hand deep.",
    );
    assert_eq!(
        passed(&h),
        [q("what is a kitchen timer"), q("how deep do tulips go")]
    );
    assert_eq!(reasons(&h).len(), 1, "a nameless frame is no source");
    assert_eq!(reject_marks(&h).len(), 1);
}

#[test]
fn a_failed_read_of_the_source_is_store_error() {
    if !shipped() {
        return;
    }
    // The words of the rows cannot be fetched (step v), then the rows
    // themselves cannot be read (step a): each field under `source` is
    // `store_error`, never a missing quote, and leaves its mark.
    for (phase, sql) in [
        ("v", "ALTER TABLE blocks RENAME TO blocks_gone"),
        ("a", "ALTER TABLE wall RENAME TO wall_gone"),
    ] {
        let mut h = hive();
        turn(
            &mut h,
            "s4",
            "r1",
            "My daughter Anna moves to Hamburg in May.",
            "How lovely.",
            json!({}),
        );
        let call = h.curate("s4", "r2", 0, json!([user(ADVICE)]), mode("Be brief."));
        h.ledger_may_refuse = true;
        h.sql_at_phase = Some((phase.to_string(), sql.to_string()));
        section_in(
            &mut h,
            &call,
            json!({"facts": [
                fact("Anna moves to Hamburg in May", Some("person")),
                fact("enjoy a trip to the Baltic Sea", Some("agent:cogny")),
                fact("Anna moves to Hamburg in May", None),
            ]}),
            "How exciting.",
        );
        assert!(h.sql_at_phase.is_none(), "{phase}: the read failed");
        assert_eq!(passed(&h), Vec::<(String, Value)>::new(), "{phase}");
        assert_eq!(
            reasons(&h),
            ["store_error", "store_error", "store_error"],
            "{phase}: {:?}",
            h.stderr
        );
        let marks = reject_marks(&h);
        assert_eq!(marks.len(), 3, "{phase}");
        assert!(
            marks.iter().all(|m| m["reason"] == "store_error"),
            "{marks:?}"
        );
    }
}

#[test]
fn a_topic_without_its_source_still_moves_and_is_marked() {
    if !shipped() {
        return;
    }
    let mut h = hive();
    let call = h.curate(
        "s5",
        "r1",
        0,
        json!([user("Let us talk about the garden.")]),
        mode("Be brief."),
    );
    section_in(
        &mut h,
        &call,
        json!({"facts": [], "topic": {"movement": "start", "name": "the garden"}}),
        "Gladly.",
    );
    assert_eq!(
        h.rows("SELECT value FROM marks WHERE kind = 'topic'"),
        vec![vec![json!(r#"{"movement":"start","name":"the garden"}"#)]]
    );
    assert_eq!(
        reject_marks(&h),
        [
            json!({"section": "memory", "field": "topic", "reason": "source_missing",
                "source": ""})
        ]
    );
}
