//! GH #951 (O.5): the why of an object is its owner's. `object_set` with
//! `slot: 'why'` is written only for the owner of the turn -- `context.speaker`,
//! a `member:` reference standing in the round of the turn (and the param
//! `owner` when it is set). Everybody else, and a turn whose speaker cannot be
//! told, gets `owner_only` and writes nothing; a call without a round is
//! `missing_audience`; and a round the row does not cover cannot tell the row
//! from one that never existed -- `not_found`, byte for byte. The shipped
//! `templates/objects` in one process (`support/objects_hive.rs`).

#[path = "support/objects_hive.rs"]
mod objects_hive;

use meclaw_core::serde_json::json;
use objects_hive::*;

const R: &str = r#"["agent:a","member:p"]"#;
const WIDER: &str = r#"["agent:a","member:p","peer:q"]"#;
const NEVER: &str = "ob-000000000000";

/// An active row learned in `R`; its id.
fn active_row(h: &mut Hive) -> String {
    for t in ["t1", "t2", "t3"] {
        h.thing_seen(Some(R), t, &["Blue Bike"]);
    }
    let row = h.head_in(R);
    assert_eq!(row["state"], json!("active"));
    row["id"].as_str().unwrap().to_string()
}

fn why(id: &str, value: &str) -> meclaw_core::serde_json::Value {
    json!({"id": id, "slot": "why", "value": value})
}

#[test]
fn the_owner_says_why() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let id = active_row(&mut h);
    let (hop, v) = h.tool(
        "object_set",
        why(&id, "for the  commute"),
        Some(R),
        Some("member:p"),
    );
    assert_eq!(v["ok"], json!(true), "{v}");
    assert!(hop.get("error_code").is_none(), "{hop}");
    let row = h.head(&id);
    assert_eq!(col(&row, "slots")["why"], json!("for the commute"));
    assert_eq!(row["rev"], json!(4), "one new rev");
    let sc = h.routed("source_changed");
    assert_eq!(
        sc.last().unwrap().body["version"],
        v["version"],
        "the new version is announced"
    );
    assert!(h.store_errors.is_empty(), "{:?}", h.store_errors);
}

#[test]
fn anybody_else_is_refused_and_writes_nothing() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let id = active_row(&mut h);
    let rows = h.count("objects");
    // An agent of the round, a member outside the round, no speaker at all.
    for speaker in [Some("agent:a"), Some("member:z"), None] {
        let (hop, v) = h.tool("object_set", why(&id, "x"), Some(R), speaker);
        assert_eq!(v["error"]["code"], json!("owner_only"), "{speaker:?}: {v}");
        assert_eq!(hop["error_code"], json!("owner_only"));
    }
    // A platform id is no speaker.
    let ctx = json!({"tool_caller": "talky", "audience_now": R, "audience_set": R,
                     "user_id": "member:p"});
    let (_, v) = h.tool_ctx("object_set", why(&id, "x"), ctx);
    assert_eq!(v["error"]["code"], json!("owner_only"), "{v}");
    // The state is the owner's as well.
    let (_, v) = h.tool(
        "object_set",
        json!({"id": id, "slot": "state", "value": "retired"}),
        Some(R),
        Some("agent:a"),
    );
    assert_eq!(v["error"]["code"], json!("owner_only"), "{v}");
    assert_eq!(h.count("objects"), rows, "a refusal writes nothing");

    // `where` is nobody's privilege: an agent of the round writes it.
    let (_, v) = h.tool(
        "object_set",
        json!({"id": id, "slot": "where", "value": "in the shed"}),
        Some(R),
        Some("agent:a"),
    );
    assert_eq!(v["ok"], json!(true), "{v}");
    assert_eq!(col(&h.head(&id), "slots")["where"], json!("in the shed"));

    // With the param `owner` set, another member of the round is no owner.
    let both = r#"["member:p","member:x"]"#;
    let mut h2 = Hive::with(&[("tools", "owner", json!("member:p"))]);
    for t in ["t1", "t2", "t3"] {
        h2.thing_seen(Some(both), t, &["Blue Bike"]);
    }
    let id2 = h2.head_in(both)["id"].as_str().unwrap().to_string();
    let (_, v) = h2.tool("object_set", why(&id2, "x"), Some(both), Some("member:x"));
    assert_eq!(v["error"]["code"], json!("owner_only"), "{v}");
    let (_, v) = h2.tool("object_set", why(&id2, "x"), Some(both), Some("member:p"));
    assert_eq!(v["ok"], json!(true), "{v}");
}

#[test]
fn a_call_without_a_round_is_missing_audience() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let id = active_row(&mut h);
    let (hop, v) = h.tool("object_set", why(&id, "x"), None, Some("member:p"));
    assert_eq!(v["error"]["code"], json!("missing_audience"), "{v}");
    assert_eq!(hop["error_code"], json!("missing_audience"));
    // A round that is no list of strings is no round.
    let (_, v) = h.tool(
        "object_set",
        why(&id, "x"),
        Some("member:p"),
        Some("member:p"),
    );
    assert_eq!(v["error"]["code"], json!("missing_audience"), "{v}");
    let (_, v) = h.tool("object_brief", json!({"id": id}), None, Some("member:p"));
    assert_eq!(v["error"]["code"], json!("missing_audience"), "{v}");
    assert_eq!(h.head(&id)["rev"], json!(3));
}

#[test]
fn a_round_the_row_does_not_cover_cannot_tell_it_from_nothing() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let id = active_row(&mut h);
    let rows = h.count("objects");
    // `peer:q` was not in the round the row was learned in. The owner asks
    // in the wider round: the answer is the one for an id that never existed.
    let cases = [
        ("object_set", why(&id, "x"), why(NEVER, "x")),
        ("object_confirm", json!({"id": id}), json!({"id": NEVER})),
        ("object_brief", json!({"id": id}), json!({"id": NEVER})),
    ];
    for (tool, hidden, never) in cases {
        let (hop_a, text_a) = h.tool_raw(
            tool,
            hidden,
            Hive::tool_context(Some(WIDER), Some("member:p")),
        );
        let (hop_b, text_b) = h.tool_raw(
            tool,
            never,
            Hive::tool_context(Some(WIDER), Some("member:p")),
        );
        assert_eq!(text_a, text_b, "{tool}: byte for byte");
        assert_eq!(hop_a, hop_b, "{tool}: the hop as well");
        assert_eq!(hop_a["error_code"], json!("not_found"), "{tool}: {text_a}");
        assert!(!text_a.contains(&id), "{tool}: no hint of the id: {text_a}");
    }
    // The covered round still reaches it.
    let (_, v) = h.tool("object_brief", json!({"id": id}), Some(R), None);
    assert_eq!(v["ok"], json!(true), "{v}");
    assert_eq!(h.count("objects"), rows, "nothing was written");
}
