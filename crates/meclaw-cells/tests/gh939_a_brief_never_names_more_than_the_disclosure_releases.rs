//! GH #939 -- a brief never names more than the disclosure releases.
//!
//! `affinity/brief` renders every `system.*` slot it serves with a head line,
//! `affinity brief on <name> (<kind>) -- <slot>`, and the tool lane's receipt
//! line starts with the same head. Up to `affinity@3.7.0` that name was the
//! record's `display_name`, read straight off the `entities` row -- past the
//! disclosure rows of the round. A round that was released the first name only
//! read the last name in the `text` of EVERY slot it was served, the
//! `identity_short` slot included, whose whole point (GH #935) is to say no more
//! than the full slot of the same round.
//!
//! The head is now built from the name parts the disclosure released to this
//! round: the record's `display_name` only when every word of it is a released
//! name part (so a fully released name reads as before, byte for byte), else the
//! released parts themselves, else the subject reference the asker spelled.
//!
//! What is pinned here, through the SHIPPED `brief` script over stdin, one phase
//! per run (the driver of `gh935_the_short_identity_says_no_more_than_the_whole.rs`):
//!
//! (a) released the first name only: the `text` of no `system.*` slot -- all
//!     five person slots asked for, `identity_short` among them -- and not the
//!     receipt line names the last name, the middle name or the nickname; the
//!     head names the released first name;
//! (b) no name part released: neither the first nor the last name stands in
//!     any `text` or in the receipt line -- for the person slots and for a
//!     `brain` request, whose leaves are not rendered but whose receipt line
//!     carries the head all the same;
//! (c) the whole name released: the head is the record's `display_name`, as it
//!     was before;
//! (d) the `who` body slot (GH #848) of the SERVED brief follows the same rule as
//!     every refusal: it rides only when the subject is in the round. A subject
//!     outside the round is served without `who`, so the record's `display_name`
//!     stands nowhere in the body; a subject in the round is named by it, as
//!     `templates/affinity/README.md` § Who is speaking says.

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::{repo, run_cell};
use serde_json::{Value, json};

const BRIEF: &str = "templates/affinity/brief/config.json";

const SUBJECT: &str = "peer:colA/org1/jonas/-";
const CHANNEL: &str = "peer-friend";
const ASKER: &str = "agent:alpha";

/// R2b / GH #49: a tree without the template skips.
fn shipped() -> bool {
    repo(BRIEF).is_file()
}

/// What the hive's door hands `./brief` on the tool lane.
fn door(slots: Value, round: &[&str]) -> Value {
    let round = json!(round).to_string();
    json!({
        "header": {"hop": {"route": "in_brief"},
                   "context": {"asker": ASKER, "channel_node": CHANNEL,
                               "audience_set": round, "aff_phase": "", "aff_carry": "",
                               "aff_subscriber": ""}},
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-939",
                      "text": json!({"subject": SUBJECT, "slots": slots}).to_string()}]
    })
}

fn op_of(m: &Value) -> Value {
    serde_json::from_str(m["messages"][0]["text"].as_str().unwrap()).unwrap()
}

/// The store's answer, the way `./store -> ./brief` delivers it.
fn store_reply(asked: &Value, rows: Value) -> Value {
    let h = &asked["header"];
    json!({
        "header": {
            "hop": {"operation": op_of(asked)["operation"]},
            "context": {
                "affinity_origin": "brief",
                "aff_phase": h["phase"], "aff_subject": h["subject"],
                "aff_audience": h["audience"], "aff_channel": h["channel"],
                "aff_slots": h["slots"], "aff_carry": h["carry"],
                "aff_subscriber": h["subscriber"]
            }
        },
        "messages": [{"origin": "tool", "type": "tool_result", "id": "s1",
                      "text": rows.to_string()}]
    })
}

/// Jonas Peter Berg, called Jo, recorded as "Jonas Berg": four name parts, a
/// channel style, an agent's own brain -- each released or not per case.
fn answer_of(op: &Value, released: &[&str]) -> Value {
    let aieos = json!({
        "identity": {"names": {"first": "Jonas", "middle": "Peter", "last": "Berg",
                               "nickname": "Jo"}},
        "motivations": {"core_drive": "keep the choir singing"},
        "interests": {"hobbies": ["choir"]},
        "linguistics": {"text_style": {"formality": "casual"}}
    });
    let mx = json!({"brain": {"identity": {"soul": "a calm voice"}}});
    let entity = json!({"entity_id": SUBJECT, "kind": "person", "display_name": "Jonas Berg",
                        "status": "active", "aieos": aieos.to_string(), "mx": mx.to_string()});
    match (
        op["operation"].as_str().unwrap_or_default(),
        op["table"].as_str().unwrap_or_default(),
    ) {
        ("select", "entities") => {
            let mut row = json!({});
            for c in op["columns"].as_array().expect("columns") {
                let c = c.as_str().unwrap();
                row[c] = entity.get(c).cloned().unwrap_or(Value::Null);
            }
            json!([row])
        }
        ("select", "trust") => {
            json!([{"level": "known", "decided_at": "2026-09-25T00:00:00.000000Z"}])
        }
        ("select", "disclosure") => Value::Array(
            released
                .iter()
                .map(|p| {
                    json!({"field_path": p, "mode": "share",
                           "decided_at": "2026-09-25T00:00:00Z",
                           "audience": "*", "audience_set": null})
                })
                .collect(),
        ),
        ("traverse", _) => json!([]),
        other => panic!("the brief asked the store for something unexpected: {other:?} {op}"),
    }
}

/// One brief, run to its answer, the subject in the round.
fn brief(slots: Value, released: &[&str]) -> Value {
    brief_in(slots, released, &[ASKER, SUBJECT])
}

/// One brief, run to its answer, for the round given.
fn brief_in(slots: Value, released: &[&str], round: &[&str]) -> Value {
    let mut doc = door(slots, round);
    for _ in 0..12 {
        let (out, _) = run_cell(BRIEF, &[], doc.clone());
        if let Some(ans) = out.iter().find(|m| m["header"]["route"] == "answer") {
            return ans.clone();
        }
        let next: Vec<&Value> = out
            .iter()
            .filter(|m| {
                m["header"]["route"] == "astore" && m["header"]["phase"].as_str() != Some("audit")
            })
            .collect();
        assert_eq!(next.len(), 1, "one store read per phase: {out:?}");
        doc = store_reply(next[0], answer_of(&op_of(next[0]), released));
    }
    panic!("the lane never answered");
}

/// The receipt line of the tool result (the head plus the slot list).
fn receipt(ans: &Value) -> String {
    let text = ans["messages"][0]["text"]
        .as_str()
        .expect("tool_result text");
    text.split('\n').next().unwrap_or_default().to_string()
}

/// Every `text` leaf of every `system.*` slot, by slot.
fn slot_texts(ans: &Value) -> Vec<(String, String)> {
    ans["system"]
        .as_object()
        .unwrap_or_else(|| panic!("a `system` is served: {ans}"))
        .iter()
        .filter_map(|(slot, doc)| doc["text"].as_str().map(|t| (slot.clone(), t.to_string())))
        .collect()
}

fn assert_never_named(ans: &Value, withheld: &[&str]) {
    let texts = slot_texts(ans);
    for (slot, text) in &texts {
        for w in withheld {
            assert!(
                !text.contains(w),
                "`{w}` was released to nobody, yet the `text` of `system.{slot}` names it: {text}"
            );
        }
    }
    let line = receipt(ans);
    for w in withheld {
        assert!(
            !line.contains(w),
            "`{w}` was released to nobody, yet the receipt line names it: {line}"
        );
    }
}

const PERSON: &[&str] = &[
    "aieos.motivations",
    "aieos.interests",
    "aieos.linguistics.text_style",
    "mx.relations",
];

// ═════════════════════════════════════ (a) the first name only

#[test]
fn a_round_given_the_first_name_reads_no_other_name_in_any_slot_text() {
    if !shipped() {
        return;
    }
    let mut released = vec!["aieos.identity.names.first"];
    released.extend_from_slice(PERSON);
    let ans = brief(
        json!([
            "identity",
            "identity_short",
            "peer",
            "relationship",
            "channel"
        ]),
        &released,
    );
    let texts = slot_texts(&ans);
    let served: Vec<&str> = texts.iter().map(|(s, _)| s.as_str()).collect();
    for slot in [
        "identity",
        "identity_short",
        "peer",
        "relationship",
        "channel",
    ] {
        assert!(
            served.contains(&slot),
            "`system.{slot}` is served and rendered: {ans}"
        );
    }
    assert_never_named(&ans, &["Berg", "Peter", "\"Jo\""]);
    for (slot, text) in &texts {
        assert!(
            text.starts_with(&format!("affinity brief on Jonas (person) -- {slot}")),
            "the head names the released first name: {text}"
        );
    }
    assert!(
        receipt(&ans).starts_with("affinity brief on Jonas (person) for agent:alpha"),
        "{ans}"
    );
}

// ═════════════════════════════════════════════ (b) no name part at all

#[test]
fn a_round_given_no_name_reads_no_name_in_any_slot_text() {
    if !shipped() {
        return;
    }
    let ans = brief(
        json!(["identity", "peer", "relationship", "channel"]),
        PERSON,
    );
    assert!(!slot_texts(&ans).is_empty(), "something is served: {ans}");
    assert_never_named(&ans, &["Jonas", "Berg", "Peter", "\"Jo\""]);
}

#[test]
fn a_brain_brief_given_no_name_names_nobody_in_its_receipt_line() {
    if !shipped() {
        return;
    }
    let ans = brief(json!(["brain"]), &["mx.brain"]);
    assert_eq!(
        ans["system"]["identity"]["soul"]["text"], "a calm voice",
        "the brain leaf is served verbatim: {ans}"
    );
    let line = receipt(&ans);
    for w in ["Jonas", "Berg"] {
        assert!(!line.contains(w), "`{w}` was released to nobody: {line}");
    }
}

// ═══════════════════════════════════════════ (c) the whole name, as before

#[test]
fn a_round_given_the_whole_name_reads_the_record_name_as_before() {
    if !shipped() {
        return;
    }
    let mut released = vec!["aieos.identity.names"];
    released.extend_from_slice(PERSON);
    let ans = brief(json!(["identity", "peer"]), &released);
    for (slot, text) in slot_texts(&ans) {
        assert!(
            text.starts_with(&format!("affinity brief on Jonas Berg (person) -- {slot}")),
            "a fully released name keeps the record's display name: {text}"
        );
    }
}

// ═══════════════════════════════════════ (d) `who` on the served brief

#[test]
fn a_served_brief_on_a_subject_outside_the_round_names_nobody_in_who() {
    if !shipped() {
        return;
    }
    for released in [
        PERSON.to_vec(),
        [&["aieos.identity.names.first"][..], PERSON].concat(),
    ] {
        let ans = brief_in(json!(["identity", "peer"]), &released, &[ASKER]);
        assert!(!slot_texts(&ans).is_empty(), "the brief is served: {ans}");
        assert!(
            ans.get("who").is_none_or(Value::is_null),
            "the subject is not in the round, so the served brief carries no `who`: {ans}"
        );
        let body = ans.to_string();
        assert!(
            !body.contains("Berg"),
            "the last name was released to nobody, yet the body names it: {body}"
        );
    }
}

#[test]
fn a_served_brief_on_a_subject_in_the_round_names_it_in_who_as_gh848() {
    if !shipped() {
        return;
    }
    let mut released = vec!["aieos.identity.names.first"];
    released.extend_from_slice(PERSON);
    let ans = brief(json!(["identity", "peer"]), &released);
    assert_eq!(ans["who"]["name"], "Jonas Berg", "{ans}");
    assert_eq!(ans["who"]["known"], true, "{ans}");
}
