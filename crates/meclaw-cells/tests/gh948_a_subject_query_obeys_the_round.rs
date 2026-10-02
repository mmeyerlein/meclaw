//! GH #948 -- a subject question obeys the round.
//!
//! `in_query {subject}` hands another part of the colony the newest facts of
//! one canonical subject -- not expired, not replaced, newest first, at most
//! `limit` -- under the same set rule every reader of this hive is held to:
//! the asking round (`context.audience_now`) must be a subset of the round a
//! fact was recorded in, `*` is universal, and a fact without participants is
//! readable by nobody. A round declared EMPTY (`[]`, an asker acting outside
//! any conversation) reads only what was released to everybody; a question
//! with no round at all is refused (`missing_audience`). The filter runs in the
//! store (`covers`), before the limit, so a hidden fact never takes the place
//! of a visible one, and the script checks every row again.

#[path = "support/memory_hive_subject.rs"]
mod support;

use meclaw_core::serde_json::{Value, json};
use support::*;

const SUBJECT: &str = "ob-1a2b3c4d5e6f";

fn at(day: u32) -> String {
    format!("2026-09-{day:02}T10:00:00Z")
}

fn seeded() -> Vec<Value> {
    let mut expired = fact("f-exp", "Firewall X", SUBJECT, ROUND_EA, &at(9));
    expired["expired_at"] = json!(at(10));
    let mut replaced = fact("f-sup", "Firewall X", SUBJECT, ROUND_EA, &at(10));
    replaced["superseded_by"] = json!("f-ea");
    let mut unread = fact("f-null", "Firewall X", SUBJECT, ROUND_EA, &at(11));
    unread["audience_set"] = Value::Null;
    vec![
        fact("f-ea", "Firewall X", SUBJECT, ROUND_EA, &at(1)),
        fact("f-eb", "Firewall X", SUBJECT, ROUND_EB, &at(2)),
        fact("f-star", "the firewall", SUBJECT, r#"["*"]"#, &at(3)),
        fact("f-empty", "Firewall X", SUBJECT, "[]", &at(4)),
        fact(
            "f-wide",
            "Firewall X",
            SUBJECT,
            r#"["agent:a","agent:b","member:e"]"#,
            &at(5),
        ),
        fact("f-other", "Router", "ob-ffffffffffff", ROUND_EA, &at(6)),
        fact("f-hidden", "Shed", "ob-000000000001", ROUND_EB, &at(7)),
        expired,
        replaced,
        unread,
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gh948_a_subject_query_obeys_the_round() {
    if !shipped() {
        eprintln!("gh948: the memory hive is not in this tree, skipped (GH #49)");
        return;
    }
    let td = tempfile::TempDir::new().expect("tempdir");
    build(&td, &seeded());
    let (h, mut rx) = boot(&td).await;
    let mut seen = Vec::new();

    // Round {e,a}: its own fact, the wider round it was part of, the universal
    // one -- newest first; never {e,b}, never `[]`, never NULL, never expired or
    // replaced, never another subject.
    h.send(in_subject(json!({"subject": SUBJECT}), Some(ROUND_EA)))
        .await;
    let b = subject_answer(&mut rx, &mut seen, SUBJECT).await;
    assert_eq!(
        claims(&b),
        vec!["note f-wide", "note f-star", "note f-ea"],
        "bundle: {b}"
    );
    assert_eq!(b["answers"], "direct");
    assert_eq!(b["complete"], json!(true));
    assert_eq!(b["candidates"][0]["kind"], "fact");
    assert_eq!(b["candidates"][0]["predicate"], "has_note");

    // A round declared empty reads only what was released to everybody. A
    // fact recorded without participants stays readable by nobody (R2 of the
    // read path): this hive never writes one (GH #933), so such a row is
    // provenance lost, not a conversation held alone.
    h.send(in_subject(json!({"subject": SUBJECT}), Some("[]")))
        .await;
    let b = subject_answer(&mut rx, &mut seen, SUBJECT).await;
    assert_eq!(claims(&b), vec!["note f-star"], "roundless: {b}");

    // The limit cuts the visible list, and the answer says it may be longer.
    h.send(in_subject(
        json!({"subject": SUBJECT, "limit": 1}),
        Some(ROUND_EA),
    ))
    .await;
    let b = subject_answer(&mut rx, &mut seen, SUBJECT).await;
    assert_eq!(claims(&b), vec!["note f-wide"]);
    assert_eq!(b["complete"], json!(false));

    // The subject is matched in the store's normal form.
    h.send(in_subject(
        json!({"subject": "OB-1A2B3C4D5E6F"}),
        Some(ROUND_EB),
    ))
    .await;
    let b = subject_answer(&mut rx, &mut seen, SUBJECT).await;
    assert_eq!(claims(&b), vec!["note f-wide", "note f-star", "note f-eb"]);

    // A subject whose only fact the round may not read answers exactly like a
    // subject nobody ever mentioned: no existence oracle.
    h.send(in_subject(
        json!({"subject": "ob-000000000001"}),
        Some(ROUND_EA),
    ))
    .await;
    let hidden = subject_answer(&mut rx, &mut seen, "ob-000000000001").await;
    h.send(in_subject(
        json!({"subject": "ob-000000000404"}),
        Some(ROUND_EA),
    ))
    .await;
    let unknown = subject_answer(&mut rx, &mut seen, "ob-000000000404").await;
    let strip = |mut v: Value| {
        v.as_object_mut().expect("bundle").remove("subject");
        v.as_object_mut().expect("bundle").remove("as_of");
        v
    };
    assert_eq!(hidden["answers"], "none");
    assert_eq!(strip(hidden), strip(unknown));

    // No round at all: refused, and the refusal finds its asker.
    h.send(in_subject(json!({"subject": SUBJECT}), None)).await;
    let r = next_on(&mut rx, &mut seen, "reject", |m| {
        hop_str(m, "recall_caller") == "objects"
    })
    .await;
    assert_eq!(hop_str(&r, "reject_reason"), "missing_audience");
    // The hive's round-trip markers end at its rim, on answers and refusals.
    for key in ["recall_subject", "mem_phase", "store_origin", "recall_id"] {
        assert!(
            !r.headers.context.contains_key(key),
            "{key} left the hive: {:?}",
            r.headers.context
        );
    }
    h.shutdown().await;
}

/// The script checks every row the store hands back again: a row the store
/// filter let through by mistake never reaches the answer.
#[test]
fn gh948_the_script_checks_every_row_again() {
    if !shipped() {
        return;
    }
    let script = meclaw_testing::shipped_script(RECALL);
    let asked = json!({"subject": SUBJECT, "limit": 10}).to_string();
    let row = |id: &str, aud: Value, exp: Value, sup: Value, subj: &str| {
        json!({"id": id, "subject": "Firewall X", "canonical_subject": subj,
               "predicate": "has_note", "canonical_predicate": "has_note",
               "claim": format!("note {id}"), "fact_kind": "world",
               "valid_from": at(1), "valid_until": null, "recorded_at": at(1),
               "expired_at": exp, "superseded_by": sup, "confidence": 70,
               "channel": "c", "audience_set": aud, "source": ""})
    };
    let rows = json!([
        row("ok", json!(ROUND_EA), Value::Null, Value::Null, SUBJECT),
        row("eb", json!(ROUND_EB), Value::Null, Value::Null, SUBJECT),
        row("empty", json!("[]"), Value::Null, Value::Null, SUBJECT),
        row("junk", json!("not json"), Value::Null, Value::Null, SUBJECT),
        row("exp", json!(ROUND_EA), json!(at(2)), Value::Null, SUBJECT),
        row("sup", json!(ROUND_EA), Value::Null, json!("ok"), SUBJECT),
        row("blank-exp", json!(ROUND_EA), json!(""), json!(""), SUBJECT),
        row(
            "other",
            json!(ROUND_EA),
            Value::Null,
            Value::Null,
            "ob-ffffffffffff"
        ),
    ]);
    let out = meclaw_testing::emit_all(
        &script,
        &json!({"header": {"context": {"mem_phase": "subj", "store_origin": "recall",
                                       "recall_id": "r-1", "recall_subject": asked,
                                       "audience_now": ROUND_EA},
                           "hop": {"operation": "select", "rows_affected": 8}},
                "messages": [{"origin": "tool", "type": "tool_result", "id": "r-subj",
                              "text": rows.to_string()}]}),
    );
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["header"]["route"], "bundle");
    assert_eq!(out[0]["header"]["recall_id"], "r-1");
    let text = out[0]["system"]["memory"]["bundle"]["text"]
        .as_str()
        .expect("bundle text");
    let b: Value = meclaw_core::serde_json::from_str(text).expect("bundle json");
    assert_eq!(b["subject"], SUBJECT);
    assert_eq!(claims(&b), vec!["note ok", "note blank-exp"]);
    assert_the_declaration_admits(RECALL, &out);

    // GH #948 review I-3: rows the script drops do not count towards a full
    // page. A store page of hidden facts answers exactly like a subject nobody
    // named -- `complete` included, or it would say that something is there.
    let answer = |round: &str, page: Value| -> Value {
        let out = meclaw_testing::emit_all(
            &script,
            &json!({"header": {"context": {"mem_phase": "subj", "store_origin": "recall",
                                           "recall_id": "r-2",
                                           "recall_subject": json!({"subject": SUBJECT, "limit": 2})
                                               .to_string(),
                                           "audience_now": round},
                               "hop": {"operation": "select", "rows_affected": 2}},
                    "messages": [{"origin": "tool", "type": "tool_result", "id": "r-subj",
                                  "text": page.to_string()}]}),
        );
        assert_the_declaration_admits(RECALL, &out);
        let mut b: Value = meclaw_core::serde_json::from_str(
            out[0]["system"]["memory"]["bundle"]["text"]
                .as_str()
                .expect("bundle text"),
        )
        .expect("bundle json");
        b.as_object_mut().expect("bundle").remove("as_of");
        b
    };
    let hidden = json!([
        row("h-1", json!(ROUND_EA), Value::Null, Value::Null, SUBJECT),
        row("h-2", json!(ROUND_EA), Value::Null, Value::Null, SUBJECT),
    ]);
    let blank_round = r#"["","member:e"]"#;
    let b = answer(blank_round, hidden.clone());
    assert_eq!(b["answers"], "none");
    assert_eq!(b["complete"], json!(true), "{b}");
    assert_eq!(b, answer(blank_round, json!([])));
    assert_eq!(answer(ROUND_EB, hidden), answer(ROUND_EB, json!([])));
}

/// The request as it leaves the script for the store: one select, the filter
/// in the store and before the limit.
#[test]
fn gh948_the_question_is_one_store_select() {
    if !shipped() {
        return;
    }
    let script = meclaw_testing::shipped_script(RECALL);
    let ask = |round: &str| {
        meclaw_testing::emit_all(
            &script,
            &json!({"header": {"context": {"audience_now": round, "recall_caller": "objects"},
                               "hop": {"route": "in_query", "phase": "recall"}},
                    "subject": "  OB-1A2B3C4D5E6F ", "limit": 5, "messages": []}),
        )
    };
    let out = ask(ROUND_EA);
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["header"]["route"], "rsubj");
    assert_eq!(out[0]["header"]["phase"], "subj");
    let carried: Value = meclaw_core::serde_json::from_str(
        out[0]["header"]["recall_subject"]
            .as_str()
            .expect("carried"),
    )
    .expect("json");
    assert_eq!(carried, json!({"subject": SUBJECT, "limit": 5}));
    let op: Value =
        meclaw_core::serde_json::from_str(out[0]["messages"][0]["text"].as_str().expect("op"))
            .expect("op json");
    assert_eq!(op["operation"], "select");
    assert_eq!(op["table"], "facts");
    assert_eq!(op["limit"], 5);
    assert_eq!(
        op["where"],
        json!({"canonical_subject": SUBJECT,
               "expired_at": {"or_null": {"eq": ""}},
               "superseded_by": {"or_null": {"eq": ""}},
               "audience_set": {"covers": ["agent:a", "member:e"]}})
    );
    assert_eq!(
        op["order_by"],
        json!([{"col": "recorded_at", "dir": "desc"}, {"col": "id", "dir": "asc"}])
    );
    // A round declared empty asks for the universal release only -- and so
    // does a round with a blank name (GH #948 review I-3): a blank name is in
    // no stored round, so the script reads only `*` for it, and a store asked
    // with the blank dropped would answer wider than the script may.
    for round in ["[]", r#"["","member:e"]"#] {
        let out = ask(round);
        let op: Value =
            meclaw_core::serde_json::from_str(out[0]["messages"][0]["text"].as_str().expect("op"))
                .expect("op json");
        assert_eq!(
            op["where"]["audience_set"],
            json!({"covers": ["*"]}),
            "round {round}"
        );
        assert_the_declaration_admits(RECALL, &out);
    }
}
