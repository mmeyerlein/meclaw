//! `who.name` names only what the disclosure releases (PP-S3-10).
//!
//! The `who` body slot of an `affinity/brief` answer (GH #848) names the
//! subject of a brief for a frame to carry. Up to `affinity@3.7.0` its `name`
//! was the record's `display_name` -- or the edge-stamped `counterpart_name`,
//! or the last segment of the subject -- past the disclosure rows of the round:
//! a round released the first name only read the full name in `who`, while
//! every slot head (GH #939) kept to the first name.
//!
//! The coordinator's ruling PP-S3-10 ("who.name follows the disclosure") makes
//! this a contract change of GH #848: the name in `who` is built from the name
//! parts this round was released, by the rule of the slot head minus its last
//! fallback -- the record's `display_name` when every word of it is a released
//! name part, else the released parts, else NO `name` key at all.
//!
//! What is pinned here, through the SHIPPED `brief` script over stdin, one phase
//! per run (the driver of `gh939_a_brief_never_names_more_than_the_disclosure_releases.rs`):
//!
//! (a) the subject in the round, the first name released: `who.name` is the
//!     first name;
//! (b) no name part released, other slots released so the brief is served:
//!     `who` carries no `name`, and `ref`, `identity` and `known` are the ones
//!     it carries with the name released;
//! (c) a refusal before the cut (`audience_not_subset`, the early
//!     `not_disclosed`, `unknown_subject`) carries `who` without `name`; the
//!     late `not_disclosed` after the cut follows the disclosure like the served
//!     brief;
//! (d) a stamped `counterpart_name` never stands as `who.name`, nor anywhere in
//!     the answer.

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::{repo, run_cell};
use serde_json::{Value, json};

const BRIEF: &str = "templates/affinity/brief/config.json";

const SUBJECT: &str = "peer:colA/org1/jonas/-";
const CHANNEL: &str = "peer-friend";
const ASKER: &str = "agent:alpha";

/// A name the edge could stamp on the context; no record holds it.
const STAMPED: &str = "Stamped Counterpart";

/// R2b / GH #49: a tree without the template skips.
fn shipped() -> bool {
    repo(BRIEF).is_file()
}

/// What the hive's door hands `./brief` on the tool lane, with `extra_ctx`
/// promoted beside the round.
fn door(slots: Value, round: &[&str], extra_ctx: Value) -> Value {
    let round = json!(round).to_string();
    let mut ctx = json!({"asker": ASKER, "channel_node": CHANNEL,
                         "audience_set": round, "aff_phase": "", "aff_carry": "",
                         "aff_subscriber": ""});
    for (k, v) in extra_ctx.as_object().unwrap() {
        ctx[k] = v.clone();
    }
    json!({
        "header": {"hop": {"route": "in_brief"}, "context": ctx},
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-946",
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

/// One disclosure row per field path, released to `audience` (and to the
/// `audience_set` given, `null` for none).
fn released_to(paths: &[&str], audience: &str, audience_set: Value) -> Vec<Value> {
    paths
        .iter()
        .map(|p| {
            json!({"field_path": p, "mode": "share",
                   "decided_at": "2026-09-25T00:00:00Z",
                   "audience": audience, "audience_set": audience_set})
        })
        .collect()
}

/// Released to everyone.
fn released(paths: &[&str]) -> Vec<Value> {
    released_to(paths, "*", Value::Null)
}

/// Jonas Peter Berg, called Jo, recorded as "Jonas Berg" -- or no record at all
/// when `record` is false.
fn answer_of(op: &Value, disclosure: &[Value], record: bool) -> Value {
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
        ("select", "entities") if !record => json!([]),
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
        ("select", "disclosure") => Value::Array(disclosure.to_vec()),
        ("traverse", _) => json!([]),
        other => panic!("the brief asked the store for something unexpected: {other:?} {op}"),
    }
}

/// One brief, run to its answer, the subject in the round. Returns (all
/// emissions, answer).
fn brief(
    slots: Value,
    disclosure: &[Value],
    record: bool,
    extra_ctx: Value,
) -> (Vec<Value>, Value) {
    let mut doc = door(slots, &[ASKER, SUBJECT], extra_ctx);
    let mut all = Vec::new();
    for _ in 0..12 {
        let (out, _) = run_cell(BRIEF, &[], doc.clone());
        all.extend(out.iter().cloned());
        if let Some(ans) = out.iter().find(|m| m["header"]["route"] == "answer") {
            return (all, ans.clone());
        }
        let next: Vec<&Value> = out
            .iter()
            .filter(|m| {
                m["header"]["route"] == "astore" && m["header"]["phase"].as_str() != Some("audit")
            })
            .collect();
        assert_eq!(next.len(), 1, "one store read per phase: {out:?}");
        doc = store_reply(next[0], answer_of(&op_of(next[0]), disclosure, record));
    }
    panic!("the lane never answered");
}

fn audit_reason(all: &[Value]) -> String {
    all.iter()
        .filter(|m| m["header"]["route"] == "astore")
        .map(op_of)
        .find(|op| op["table"] == "audit")
        .map(|op| {
            op["row"]["reason_code"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .expect("an audit row")
}

/// `who` without its `name`: what stays when nothing names the subject.
fn unnamed(who: &Value) -> Value {
    let mut w = who.clone();
    w.as_object_mut()
        .unwrap_or_else(|| panic!("a `who` block: {who}"))
        .remove("name");
    w
}

const PERSON: &[&str] = &[
    "aieos.motivations",
    "aieos.interests",
    "aieos.linguistics.text_style",
    "mx.relations",
];

fn with_first_name() -> Vec<Value> {
    released(&[&["aieos.identity.names.first"][..], PERSON].concat())
}

// ═════════════════════════════════════ (a) the first name only

#[test]
fn a_round_given_the_first_name_reads_the_first_name_in_who() {
    if !shipped() {
        return;
    }
    let (all, ans) = brief(
        json!(["identity", "peer"]),
        &with_first_name(),
        true,
        json!({}),
    );
    assert_eq!(audit_reason(&all), "", "the brief is served: {ans}");
    assert!(ans.get("system").is_some(), "the brief is served: {ans}");
    assert_eq!(ans["who"]["name"], "Jonas", "{ans}");
    assert_eq!(ans["who"]["known"], true, "{ans}");
    let who = ans["who"].to_string();
    for w in ["Berg", "Peter", "\"Jo\""] {
        assert!(!who.contains(w), "`{w}` was released to nobody: {who}");
    }
}

// ═════════════════════════════════════════════ (b) no name part at all

#[test]
fn a_round_given_no_name_part_reads_no_name_in_who() {
    if !shipped() {
        return;
    }
    let (all, ans) = brief(
        json!(["identity", "peer"]),
        &released(PERSON),
        true,
        json!({}),
    );
    assert_eq!(audit_reason(&all), "", "the brief is served: {ans}");
    assert!(ans.get("system").is_some(), "the brief is served: {ans}");
    let who = ans["who"]
        .as_object()
        .unwrap_or_else(|| panic!("the subject is in the round, so `who` rides: {ans}"));
    assert!(
        !who.contains_key("name"),
        "no name part released, no `name` key: {ans}"
    );
    let (_, named) = brief(
        json!(["identity", "peer"]),
        &with_first_name(),
        true,
        json!({}),
    );
    assert_eq!(
        ans["who"],
        unnamed(&named["who"]),
        "`ref`, `identity` and `known` do not depend on the name"
    );
    for w in ["Jonas", "Berg"] {
        assert!(
            !ans["who"].to_string().contains(w),
            "`{w}` was released to nobody: {ans}"
        );
    }
}

// ═══════════════════════════════════════ (c) refusals before and after the cut

#[test]
fn a_refusal_before_the_cut_carries_who_without_a_name() {
    if !shipped() {
        return;
    }
    let only_the_asker = released_to(&["*"], ASKER, json!("[\"agent:alpha\"]"));
    let cases: Vec<(Vec<Value>, bool, &str)> = vec![
        (only_the_asker, true, "audience_not_subset"),
        (vec![], true, "not_disclosed"),
        (released(&["*"]), false, "unknown_subject"),
    ];
    for (disclosure, record, reason) in cases {
        let (all, ans) = brief(json!(["identity", "peer"]), &disclosure, record, json!({}));
        assert_eq!(audit_reason(&all), reason, "{ans}");
        assert!(ans.get("system").is_none(), "{reason}: a refusal: {ans}");
        let who = ans["who"]
            .as_object()
            .unwrap_or_else(|| panic!("{reason}: the subject is in the round: {ans}"));
        let mut keys: Vec<&str> = who.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["identity", "known", "ref"],
            "{reason}: nothing was released yet, so nothing names: {ans}"
        );
        assert_eq!(ans["who"]["known"], json!(record), "{reason}: {ans}");
        assert!(!ans.to_string().contains("Jonas"), "{reason}: {ans}");
    }
}

#[test]
fn a_refusal_after_the_cut_names_what_the_round_was_released() {
    if !shipped() {
        return;
    }
    // Released, but only on a field the record does not carry: the late
    // `not_disclosed` with no name part in the cut view.
    let (all, ans) = brief(
        json!(["identity", "peer"]),
        &released(&["mx.nothing_here"]),
        true,
        json!({}),
    );
    assert_eq!(audit_reason(&all), "not_disclosed", "{ans}");
    assert!(ans["who"].is_object(), "{ans}");
    assert!(ans["who"].get("name").is_none(), "{ans}");
    // The first name released, but no slot asked for renders it: the late
    // `not_disclosed` reads the first name in `who`, and no other.
    let (all, ans) = brief(
        json!(["channel"]),
        &released(&["aieos.identity.names.first"]),
        true,
        json!({}),
    );
    assert_eq!(audit_reason(&all), "not_disclosed", "{ans}");
    assert!(ans.get("system").is_none(), "{ans}");
    assert_eq!(ans["who"]["name"], "Jonas", "{ans}");
    assert!(!ans.to_string().contains("Berg"), "{ans}");
}

// ═══════════════════════════════════════ (d) a stamped counterpart name

#[test]
fn a_stamped_counterpart_name_never_stands_as_who_name() {
    if !shipped() {
        return;
    }
    let stamp = json!({"counterpart_name": STAMPED});
    let cases: Vec<(Value, Vec<Value>, bool, Option<&str>)> = vec![
        (
            json!(["identity", "peer"]),
            with_first_name(),
            true,
            Some("Jonas"),
        ),
        (json!(["identity", "peer"]), released(PERSON), true, None),
        (json!(["identity", "peer"]), vec![], true, None),
        (json!(["identity", "peer"]), vec![], false, None),
    ];
    for (slots, disclosure, record, name) in cases {
        let (_, ans) = brief(slots, &disclosure, record, stamp.clone());
        assert!(ans["who"].is_object(), "the subject is in the round: {ans}");
        assert_eq!(
            ans["who"].get("name").and_then(Value::as_str),
            name,
            "`who.name` is what the round was released, never the stamp: {ans}"
        );
        assert!(
            !ans.to_string().contains(STAMPED),
            "the stamped name stands nowhere in the answer: {ans}"
        );
    }
}
