//! GH #848 — affinity names who is speaking: one short participant reference per
//! identity, the same in every channel, and a `who` block on every brief answer
//! that names a subject.
//!
//! Measured at `affinity@3.5.0`: `gate` mints `entity:<uuid4-hex>` (32 characters,
//! `gate/config.json:7`), the brief answers about one subject but never names it in
//! a form a frame could carry, and a counterpart affinity has never stored gets a
//! bare `nothing is disclosed to this audience` -- no reference, no name. A model in
//! a channel with several participants therefore cannot tell who is speaking beyond
//! a free-text mark, and memory cannot attribute what was said.
//!
//! What is pinned here, all of it through the SHIPPED `brief` script over stdin, one
//! phase per run, with the store's answers handed back the way the hive's internal
//! `./store -> ./brief` edge hands them back:
//!
//! 1. a known entity -> `who.known true`, `name` = its `display_name`, `ref` = the
//!    first 8 hex characters of `sha256(identity)`;
//! 2. an unknown subject `peer:colA/org1/jonas/-` -> `who.known false`, the same
//!    function for `ref`, `name` = the last non-empty segment (`jonas`);
//! 3. two requests from two channels (and with another prefix on the subject) ->
//!    the same `ref`: a pure function of the identity, org-wide without any
//!    coordination between affinity instances;
//! 4. another identity in the own store with the same 8-character reference ->
//!    12 characters, for both of them;
//! 5. `audience_not_subset` -> a refusal WITH `who`, and without any slot content:
//!    `who` carries exactly four keys, and nothing of the record rides along;
//!    `unknown_subject` (a release, no active row) names the subject the same way;
//!    but a subject OUTSIDE the round gets a refusal WITHOUT `who` -- whether the
//!    record knows it or not, so the refusal is word for word the one it was before
//!    3.6.0 and neither a `display_name` nor the bare fact of a record leaks (fix
//!    round 1, review I-1);
//! 6. `no_round` and `no_subject` -> no `who`;
//! 7. no store write in any of the cases -- the only insert is the brief's own
//!    audit row, as before; the reference is computed, never stored;
//! 8. the push lane is untouched: no extra read, no `who`;
//! 9. the README carries the function verbatim and the script carries it too;
//! 10. the participant read is ordered, so which 5 000 rows it sees does not depend
//!     on the store's row order (review m-2).

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::{config_of, repo, run_cell};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const BRIEF: &str = "templates/affinity/brief/config.json";
const README: &str = "templates/affinity/README.md";
const DENIED: &str = "nothing is disclosed to this audience";

/// Two identities whose sha256 share the first 8 hex characters (`2a549df5`),
/// found by brute force; their 12-character references differ.
const TWIN_A: &str = "colA/org1/p607/-";
const TWIN_B: &str = "colA/org1/p42847/-";

fn sha_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ─────────────────────────────────────────────────────────────── the lane

/// What the hive's door hands `./brief`: a tool call, the asker and the round
/// promoted onto context, the channel node beside them.
fn door(subject: &str, channel: &str, round: Option<&[&str]>, extra_ctx: Value) -> Value {
    let mut ctx = json!({"asker": "agent:alpha", "channel_node": channel,
                         "aff_phase": "", "aff_carry": "", "aff_subscriber": ""});
    if let Some(r) = round {
        ctx["audience_set"] = json!(serde_json::to_string(r).unwrap());
    }
    for (k, v) in extra_ctx.as_object().unwrap() {
        ctx[k] = v.clone();
    }
    json!({
        "header": {"hop": {"route": "in_brief"}, "context": ctx},
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "call-1",
                      "text": json!({"subject": subject, "slots": ["identity", "peer"]})
                                  .to_string()}]
    })
}

fn op_of(m: &Value) -> Value {
    serde_json::from_str(m["messages"][0]["text"].as_str().unwrap()).unwrap()
}

fn phase_of(m: &Value) -> String {
    m["header"]["phase"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The store's answer, the way `./store -> ./brief` delivers it: the emitted hop
/// keys promoted to `context.aff_*` by the `./brief -> ./store` edge, the store's
/// own `hop.operation`, the rows as JSON in one tool_result.
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

/// A store holding `entities` and `disclosure` rows, answering the brief's reads.
struct Store {
    entities: Vec<Value>,
    disclosure: Vec<Value>,
}

impl Store {
    fn answer(&self, op: &Value) -> Value {
        let table = op["table"].as_str().unwrap_or_default();
        let wher = &op["where"];
        match (op["operation"].as_str().unwrap_or_default(), table) {
            ("select", "entities") => {
                let want = wher["entity_id"].as_str();
                let active = wher["status"].as_str();
                let cols: Vec<String> = op["columns"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| c.as_str().unwrap().to_string())
                    .collect();
                Value::Array(
                    self.entities
                        .iter()
                        .filter(|e| want.is_none_or(|w| e["entity_id"] == w))
                        .filter(|e| active.is_none_or(|s| e["status"] == s))
                        .map(|e| {
                            let mut row = json!({});
                            for c in &cols {
                                row[c] = e.get(c).cloned().unwrap_or(Value::Null);
                            }
                            row
                        })
                        .collect(),
                )
            }
            ("select", "trust") => json!([]),
            ("select", "disclosure") => Value::Array(self.disclosure.clone()),
            ("traverse", _) => json!([]),
            other => panic!("the brief asked the store for something unexpected: {other:?} {op}"),
        }
    }
}

/// Every emission of one brief, run to its answer. Returns (all emissions, answer).
fn walk(first: Value, store: &Store) -> (Vec<Value>, Value) {
    let mut all = Vec::new();
    let mut doc = first;
    for _ in 0..12 {
        let (out, _) = run_cell(BRIEF, &[], doc.clone());
        all.extend(out.iter().cloned());
        if let Some(ans) = out.iter().find(|m| m["header"]["route"] == "answer") {
            return (all, ans.clone());
        }
        let next: Vec<&Value> = out
            .iter()
            .filter(|m| m["header"]["route"] == "astore" && phase_of(m) != "audit")
            .collect();
        assert_eq!(next.len(), 1, "one store read per phase: {out:?}");
        doc = store_reply(next[0], store.answer(&op_of(next[0])));
    }
    panic!("the lane never answered: {all:?}");
}

fn entity(id: &str, name: &str) -> Value {
    json!({"entity_id": id, "kind": "person", "display_name": name, "status": "active",
           "aieos": json!({"identity": {"names": {"first": name}}}).to_string(),
           "mx": "{}"})
}

fn shared_to_everyone() -> Vec<Value> {
    vec![
        json!({"field_path": "*", "mode": "share", "decided_at": "2026-09-25T00:00:00Z",
                "audience": "*", "audience_set": null}),
    ]
}

const ROUND: &[&str] = &["agent:alpha", "peer:colA/org1/jonas/-"];

/// No write reaches the store: the only non-read op is the brief's own audit row.
fn assert_reads_only(all: &[Value]) {
    for m in all.iter().filter(|m| m["header"]["route"] == "astore") {
        let op = op_of(m);
        let kind = op["operation"].as_str().unwrap_or_default();
        let ok =
            matches!(kind, "select" | "traverse") || (kind == "insert" && op["table"] == "audit");
        assert!(ok, "the brief wrote into the store: {op}");
        let row = op["row"].to_string();
        assert!(
            !row.contains("\"ref\""),
            "the reference is computed, never stored: {op}"
        );
    }
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

// ─────────────────────────────────────────────────────────────── the pins

#[test]
fn a_known_entity_is_named_by_its_display_name() {
    let store = Store {
        entities: vec![entity("peer:colA/org1/jonas/-", "Jonas Berg")],
        disclosure: shared_to_everyone(),
    };
    let (all, ans) = walk(
        door(
            "peer:colA/org1/jonas/-",
            "peer-friend:colA",
            Some(ROUND),
            json!({}),
        ),
        &store,
    );
    assert_reads_only(&all);
    let identity = "colA/org1/jonas/-";
    assert_eq!(
        ans["who"],
        json!({"ref": &sha_hex(identity)[..8], "name": "Jonas Berg",
               "identity": identity, "known": true}),
        "the served brief names its subject: {ans}"
    );
    assert!(
        ans.get("system").is_some(),
        "and still carries the pack: {ans}"
    );
}

#[test]
fn an_unknown_subject_gets_the_same_function_and_its_last_segment() {
    let store = Store {
        entities: vec![],
        disclosure: vec![],
    };
    let (all, ans) = walk(
        door(
            "peer:colA/org1/jonas/-",
            "peer-friend:colA",
            Some(ROUND),
            json!({}),
        ),
        &store,
    );
    assert_reads_only(&all);
    assert_eq!(
        ans["who"],
        json!({"ref": &sha_hex("colA/org1/jonas/-")[..8], "name": "jonas",
               "identity": "colA/org1/jonas/-", "known": false}),
        "a stranger has a reference before affinity ever stored them: {ans}"
    );
    assert!(
        ans.get("system").is_none(),
        "nothing released, nothing packed: {ans}"
    );
    assert_eq!(ans["messages"][0]["text"], DENIED);
}

#[test]
fn a_stamped_counterpart_name_wins_over_the_segment_and_is_capped() {
    let store = Store {
        entities: vec![],
        disclosure: vec![],
    };
    let long = "J".repeat(80);
    let (_, ans) = walk(
        door(
            "peer:colA/org1/jonas/-",
            "peer-friend:colA",
            Some(ROUND),
            json!({"counterpart_name": long}),
        ),
        &store,
    );
    assert_eq!(
        ans["who"]["name"],
        json!("J".repeat(64)),
        "capped at 64: {ans}"
    );
}

#[test]
fn the_same_identity_has_the_same_reference_in_every_channel() {
    let store = Store {
        entities: vec![],
        disclosure: vec![],
    };
    let (_, a) = walk(
        door(
            "peer:colA/org1/jonas/-",
            "peer-friend:colA",
            Some(ROUND),
            json!({}),
        ),
        &store,
    );
    let (_, b) = walk(
        door(
            "peer:colA/org1/jonas/-",
            "telegram",
            Some(&["peer:colA/org1/jonas/-"]),
            json!({}),
        ),
        &store,
    );
    // Another prefix on the same identity: the prefix says which road the subject
    // came by, the identity says who it is -- and the round holds that identity
    // (under `peer:`), so the refusal still names it.
    let (_, c) = walk(
        door(
            "member:colA/org1/jonas/-",
            "display-desk",
            Some(ROUND),
            json!({}),
        ),
        &store,
    );
    assert_eq!(a["who"]["ref"], b["who"]["ref"]);
    assert_eq!(a["who"]["ref"], c["who"]["ref"]);
    assert_eq!(c["who"]["identity"], json!("colA/org1/jonas/-"));
    assert_eq!(a["who"]["ref"].as_str().unwrap().len(), 8);
}

#[test]
fn a_collision_in_the_own_store_takes_twelve_characters() {
    assert_eq!(
        sha_hex(TWIN_A)[..8],
        sha_hex(TWIN_B)[..8],
        "the fixture collides"
    );
    assert_ne!(sha_hex(TWIN_A)[..12], sha_hex(TWIN_B)[..12]);
    // Each twin asks in a round that holds it: a refusal names only a subject in
    // the round (I-1), and every brief here is a refusal.
    let round_a = [format!("peer:{TWIN_A}")];
    let round_b = [format!("peer:{TWIN_B}")];
    let ra: Vec<&str> = round_a.iter().map(String::as_str).collect();
    let rb: Vec<&str> = round_b.iter().map(String::as_str).collect();
    // Only one of the twins stored: the stranger collides with it, and both
    // sides see the collision.
    let one = Store {
        entities: vec![entity(&format!("peer:{TWIN_A}"), "A")],
        disclosure: vec![],
    };
    let (all, b) = walk(
        door(
            &format!("peer:{TWIN_B}"),
            "peer-friend:colA",
            Some(rb.as_slice()),
            json!({}),
        ),
        &one,
    );
    assert_reads_only(&all);
    assert_eq!(b["who"]["ref"], json!(&sha_hex(TWIN_B)[..12]), "{b}");
    assert_eq!(b["who"]["known"], json!(false));
    // The stored one alone has no twin in the store: 8.
    let (_, a) = walk(
        door(
            &format!("peer:{TWIN_A}"),
            "peer-friend:colA",
            Some(ra.as_slice()),
            json!({}),
        ),
        &one,
    );
    assert_eq!(a["who"]["ref"], json!(&sha_hex(TWIN_A)[..8]), "{a}");
    // Both stored: both 12.
    let both = Store {
        entities: vec![
            entity(&format!("peer:{TWIN_A}"), "A"),
            entity(&format!("peer:{TWIN_B}"), "B"),
        ],
        disclosure: vec![],
    };
    let (_, a2) = walk(
        door(
            &format!("peer:{TWIN_A}"),
            "peer-friend:colA",
            Some(ra.as_slice()),
            json!({}),
        ),
        &both,
    );
    let (_, b2) = walk(
        door(
            &format!("peer:{TWIN_B}"),
            "peer-friend:colA",
            Some(rb.as_slice()),
            json!({}),
        ),
        &both,
    );
    assert_eq!(a2["who"]["ref"], json!(&sha_hex(TWIN_A)[..12]));
    assert_eq!(b2["who"]["ref"], json!(&sha_hex(TWIN_B)[..12]));
}

#[test]
fn a_refusal_for_the_round_names_the_subject_and_nothing_else() {
    // Released to the agent alone; the round has the counterpart in it.
    let store = Store {
        entities: vec![entity("peer:colA/org1/jonas/-", "Jonas Berg")],
        disclosure: vec![json!({"field_path": "*", "mode": "share",
                                "decided_at": "2026-09-25T00:00:00Z",
                                "audience": "agent:alpha",
                                "audience_set": "[\"agent:alpha\"]"})],
    };
    let (all, ans) = walk(
        door(
            "peer:colA/org1/jonas/-",
            "peer-friend:colA",
            Some(ROUND),
            json!({}),
        ),
        &store,
    );
    assert_reads_only(&all);
    assert_eq!(audit_reason(&all), "audience_not_subset");
    let who = ans["who"].as_object().expect("a who block on the refusal");
    let mut keys: Vec<&str> = who.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["identity", "known", "name", "ref"],
        "four keys, no slot: {ans}"
    );
    assert_eq!(ans["who"]["name"], json!("Jonas Berg"));
    assert!(
        ans.get("system").is_none(),
        "no slot content on a refusal: {ans}"
    );
    assert_eq!(
        ans["messages"][0]["text"], DENIED,
        "one sentence, nothing appended"
    );
    let body = ans.as_object().unwrap();
    let mut top: Vec<&str> = body.keys().map(String::as_str).collect();
    top.sort_unstable();
    assert_eq!(top, ["header", "messages", "who"], "{ans}");
}

/// The body of a refusal without its header: what the asker reads.
fn body_of(ans: &Value) -> Value {
    let mut b = ans.clone();
    b.as_object_mut().unwrap().remove("header");
    b
}

#[test]
fn a_subject_outside_the_round_is_refused_without_who_known_or_not() {
    let known = "peer:colA/org1/jonas/-";
    // Nobody in this round is Jonas: the asker alone, then the asker and another agent.
    let solo: &[&str] = &["agent:alpha"];
    let pair: &[&str] = &["agent:alpha", "agent:beta"];
    let cases: Vec<(&str, Store, &[&str], &str)> = vec![
        (
            known,
            Store {
                entities: vec![entity(known, "Jonas Berg")],
                disclosure: vec![],
            },
            solo,
            "not_disclosed",
        ),
        (
            known,
            Store {
                entities: vec![entity(known, "Jonas Berg")],
                disclosure: vec![json!({"field_path": "*", "mode": "share",
                                        "decided_at": "2026-09-25T00:00:00Z",
                                        "audience": "agent:alpha",
                                        "audience_set": "[\"agent:alpha\"]"})],
            },
            pair,
            "audience_not_subset",
        ),
        // Known only under another prefix, released, no row of its own: the
        // same-identity branch would hand out the other row's display_name.
        (
            known,
            Store {
                entities: vec![entity("member:colA/org1/jonas/-", "Jonas Berg")],
                disclosure: shared_to_everyone(),
            },
            solo,
            "unknown_subject",
        ),
        // Released, but only on a field the record does not carry: the late
        // `not_disclosed` of the entity phase (empty pack), the third call
        // site of `refusal_who` (review of M1, m-5).
        (
            known,
            Store {
                entities: vec![entity(known, "Jonas Berg")],
                disclosure: vec![json!({"field_path": "mx.nothing_here", "mode": "share",
                                        "decided_at": "2026-09-25T00:00:00Z",
                                        "audience": "*", "audience_set": null})],
            },
            solo,
            "not_disclosed",
        ),
        // A stranger outside the round: the same refusal, so its shape is no
        // oracle for whether the record knows somebody.
        (
            known,
            Store {
                entities: vec![],
                disclosure: vec![],
            },
            solo,
            "not_disclosed",
        ),
    ];
    let mut bodies = Vec::new();
    for (subject, store, round, reason) in cases {
        let (all, ans) = walk(door(subject, "telegram", Some(round), json!({})), &store);
        assert_reads_only(&all);
        assert_eq!(audit_reason(&all), reason, "{ans}");
        assert!(
            ans.get("who").is_none(),
            "{reason}: a subject outside the round is not named: {ans}"
        );
        assert!(!ans.to_string().contains("Jonas Berg"), "{ans}");
        bodies.push(body_of(&ans));
    }
    for b in &bodies[1..] {
        assert_eq!(
            b, &bodies[0],
            "every refusal about an absent subject reads the same"
        );
    }
}

#[test]
fn an_unknown_subject_refusal_names_the_subject_in_the_round() {
    // A release exists, the active row does not: `unknown_subject`, after the read.
    let stranger = Store {
        entities: vec![],
        disclosure: shared_to_everyone(),
    };
    let (all, ans) = walk(
        door(
            "peer:colA/org1/jonas/-",
            "peer-friend:colA",
            Some(ROUND),
            json!({}),
        ),
        &stranger,
    );
    assert_reads_only(&all);
    assert_eq!(audit_reason(&all), "unknown_subject");
    assert_eq!(
        ans["who"],
        json!({"ref": &sha_hex("colA/org1/jonas/-")[..8], "name": "jonas",
               "identity": "colA/org1/jonas/-", "known": false}),
        "{ans}"
    );
    assert!(ans.get("system").is_none(), "{ans}");
    assert_eq!(ans["messages"][0]["text"], DENIED);
    // Known under another prefix, in the round: the record's name.
    let alias = Store {
        entities: vec![entity("member:colA/org1/jonas/-", "Jonas Berg")],
        disclosure: shared_to_everyone(),
    };
    let (all, ans) = walk(
        door(
            "peer:colA/org1/jonas/-",
            "peer-friend:colA",
            Some(ROUND),
            json!({}),
        ),
        &alias,
    );
    assert_eq!(audit_reason(&all), "unknown_subject");
    assert_eq!(ans["who"]["known"], json!(true), "{ans}");
    assert_eq!(ans["who"]["name"], json!("Jonas Berg"), "{ans}");
}

#[test]
fn the_participant_read_is_ordered_and_bounded() {
    let (out, _) = run_cell(
        BRIEF,
        &[],
        door(
            "peer:colA/org1/jonas/-",
            "peer-friend:colA",
            Some(ROUND),
            json!({}),
        ),
    );
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(phase_of(&out[0]), "who");
    let op = op_of(&out[0]);
    assert_eq!(op["table"], "entities");
    assert_eq!(op["limit"], 5000);
    assert_eq!(
        op["order_by"],
        json!([{"col": "entity_id", "dir": "asc"}]),
        "the same rows in every run: {op}"
    );
}

#[test]
fn no_round_and_no_subject_carry_no_who() {
    let store = Store {
        entities: vec![],
        disclosure: vec![],
    };
    let (all, ans) = walk(
        door(
            "peer:colA/org1/jonas/-",
            "peer-friend:colA",
            None,
            json!({}),
        ),
        &store,
    );
    assert_reads_only(&all);
    assert_eq!(audit_reason(&all), "no_round");
    assert!(
        ans.get("who").is_none(),
        "no_round stays without who: {ans}"
    );
    let (_, ans) = walk(door("", "peer-friend:colA", Some(ROUND), json!({})), &store);
    assert!(
        ans.get("who").is_none(),
        "no_subject stays without who: {ans}"
    );
}

#[test]
fn the_push_lane_is_untouched() {
    let doc = door(
        "entity:alex",
        "*",
        Some(&["agent:alpha"]),
        json!({"aff_subscriber": "/org/alex/assistants/aiden/talky"}),
    );
    let (out, _) = run_cell(BRIEF, &[], doc);
    assert_eq!(out.len(), 1);
    assert_eq!(
        phase_of(&out[0]),
        "trust",
        "a push reads no participant list: {out:?}"
    );
}

#[test]
fn the_function_is_published_verbatim() {
    let script = config_of(BRIEF)["params"]["script_inline"]
        .as_str()
        .unwrap()
        .to_string();
    let readme = std::fs::read_to_string(repo(README)).unwrap();
    let start = script
        .find("def participant_ref(")
        .expect("participant_ref in the brief script");
    let end = script[start..]
        .find("\n\n\n")
        .map(|e| start + e)
        .expect("end of participant_ref");
    let func = &script[start..end];
    assert!(
        readme.contains(func),
        "the README carries the function exactly as the brief runs it:\n{func}"
    );
    let ident_start = script.find("def identity_of(").expect("identity_of");
    let ident_end = script[ident_start..]
        .find("\n\n\n")
        .map(|e| ident_start + e)
        .unwrap();
    assert!(
        readme.contains(&script[ident_start..ident_end]),
        "identity_of verbatim too"
    );
}
