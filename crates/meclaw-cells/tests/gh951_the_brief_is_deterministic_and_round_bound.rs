//! GH #951 (O.4/O.6): the brief of an object is deterministic and reaches
//! only the rounds its row covers. The pure half of the shipped `./brief`
//! (`render`, loaded by `ast`), and the whole hive in one process
//! (`support/objects_hive.rs`) for the push and the node source.
//!
//! 1. **The same row and the same facts give the same bytes**, whatever the
//!    key order of the stored JSON.
//! 2. **`who` is a count, never a name or a reference.**
//! 3. **At most 600 characters, by whole parts.**
//! 4. **A round the row does not cover** gets no brief from the tools and no
//!    hint that the object exists; the push carries the row's round.
//! 5. **The push follows events, once per news**: a promotion announces the
//!    version and asks memory for facts; the facts bring a `candidate` (the
//!    brief, the aliases as triggers, the row's round) and an `alias`; the same
//!    facts again bring nothing, an `alias_ack` never brings an `alias`.
//!    Memory's side is the shape of GH #948: `facts` and `alias` carry
//!    `messages: []`, the member's door stamps only the round and the caller,
//!    the facts are `candidates[]` of the bundle JSON in
//!    `system.memory.bundle.text`, and a refusal keeps the facts last stored.
//!    Memory takes at most 32 aliases per `in_alias`: a longer set goes out
//!    in pieces. Its refusal names no subject: the member's `facts` door
//!    keeps the id in `context.objects_subject` and its way back restates it
//!    as `hop.subject`, so a refusal still brings the brief. That restated
//!    subject is THE subject of an answer: a bundle about another subject
//!    changes no brief, and an answer without it is dropped (review O I-2).
//! 7. **A round never names nobody**: an empty or non-string entry is dropped
//!    from a round, and a round with nothing left is `[]`.
//! 6. **The graph reads the current version only**: `outline` and `links` of
//!    the announced version are the nodes and edges the announcement counted;
//!    an older version is `stale_version`.

#[path = "support/objects_hive.rs"]
mod objects_hive;

use meclaw_colony::cel_eval::{
    apply_modifier, evaluate_condition, parse_condition, parse_modifier,
};
use meclaw_colony::config::ModifierSpec;
use meclaw_core::Headers;
use meclaw_core::serde_json::{self as sj, Value, json};
use objects_hive::*;

const R: &str = r#"["agent:a","member:p"]"#;
const WIDER: &str = r#"["agent:a","member:p","peer:q"]"#;
const NEVER: &str = "ob-000000000000";
const DOC: &str = "fh-0123456789ab#sec:x";
const RELATED: &str = "ob-00000000000a";

fn stored(slots: Value) -> Value {
    json!({"id": "ob-0123456789ab", "rev": 4, "type": "thing",
           "aliases": json!(["Blue Bike", "the bike"]).to_string(),
           "state": "active", "audience_set": R, "slots": slots.to_string(),
           "refs": json!({"doc": [], "related": []}).to_string()})
}

fn render(row: &Value, facts: Value) -> String {
    pure("brief", "render(ARGS[0], ARGS[1])", json!([row, facts]))
        .as_str()
        .expect("a text")
        .to_string()
}

#[test]
fn the_same_row_and_facts_render_the_same_bytes() {
    if !shipped() {
        return;
    }
    let facts = json!(["it has a bell", "it was bought in May"]);
    let row = stored(json!({"what": "a red city bike", "where": "in the shed",
                            "who": ["member:p", "member:x"], "when": "",
                            "why": "for the commute", "how": ""}));
    let a = render(&row, facts.clone());
    let b = render(&row, facts.clone());
    assert_eq!(a, b, "two runs, one text");
    // The same row stored with its keys in another order.
    let mut other = row.clone();
    other["slots"] = json!(
        r#"{"how":"","why":"for the commute","who":["member:p","member:x"],"when":"","where":"in the shed","what":"a red city bike"}"#
    );
    assert_eq!(render(&other, facts.clone()), a);
    assert_eq!(
        a,
        "thing: Blue Bike\nwhat: a red city bike\nwhere: in the shed\nwho: 2 people\n\
         why: for the commute\nfact: it has a bell\nfact: it was bought in May"
    );
    // At most three facts.
    let four = json!(["one", "two", "three", "four"]);
    assert!(!render(&row, four).contains("four"));
}

#[test]
fn who_is_a_count_never_a_name() {
    if !shipped() {
        return;
    }
    let row = stored(
        json!({"what": "", "where": "", "who": ["member:p", "affinity:x7"],
                            "when": "", "why": "", "how": ""}),
    );
    let text = render(&row, json!([]));
    assert!(text.contains("who: 2 people"), "{text}");
    assert!(
        !text.contains("member:") && !text.contains("affinity:"),
        "{text}"
    );
}

#[test]
fn a_brief_holds_at_most_600_characters_of_whole_parts() {
    if !shipped() {
        return;
    }
    let long = "x".repeat(400);
    let row = stored(json!({"what": long, "where": long, "who": [], "when": long,
                            "why": long, "how": long}));
    let facts = json!(["a fact"]);
    let text = render(&row, facts.clone());
    assert!(text.chars().count() <= 600, "{}", text.len());
    let parts = pure("brief", "parts_of(ARGS[0], ARGS[1])", json!([row, facts]));
    let parts: Vec<&str> = parts
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    let lines: Vec<&str> = text.split('\n').collect();
    assert_eq!(
        lines,
        parts[..lines.len()].to_vec(),
        "whole parts, in order"
    );
    assert_eq!(lines.len(), 2, "the type line and one slot fit: {text}");
}

#[test]
fn the_object_graph_is_one_text_in_two_cells() {
    if !shipped() {
        return;
    }
    // `./push` counts what `./source` answers: one definition, two copies.
    assert_eq!(
        block("push", "object-graph"),
        block("source", "object-graph")
    );
}

/// An active row in `R` named "Blue Bike"; its id.
fn active_row(h: &mut Hive) -> String {
    for t in ["t1", "t2", "t3"] {
        h.thing_seen(Some(R), t, &["Blue Bike"]);
    }
    h.head_in(R)["id"].as_str().unwrap().to_string()
}

#[test]
fn a_round_without_cover_gets_no_brief_and_no_hint() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let id = active_row(&mut h);
    let (_, v) = h.tool("object_brief", json!({"id": id}), Some(R), None);
    assert_eq!(v["ok"], json!(true), "{v}");
    assert_eq!(
        v["text"],
        json!(render(&h.head(&id), json!([]))),
        "the tool renders with ./brief"
    );
    let (_, hidden) = h.tool_raw(
        "object_brief",
        json!({"id": id}),
        Hive::tool_context(Some(WIDER), None),
    );
    let (_, never) = h.tool_raw(
        "object_brief",
        json!({"id": NEVER}),
        Hive::tool_context(Some(WIDER), None),
    );
    assert_eq!(hidden, never, "no hint of existence");
    // `find` does not see it either, by name or by type.
    for q in ["bike", "thing"] {
        let (_, v) = h.tool("object_find", json!({"q": q}), Some(WIDER), None);
        assert_eq!(v["items"], json!([]), "{q}: {v}");
        let (_, v) = h.tool("object_find", json!({"q": q}), Some(R), None);
        assert_eq!(v["items"][0]["id"], json!(id), "{q}: {v}");
        assert_eq!(v["items"][0]["name"], json!("Blue Bike"));
    }
    // Every push carries the row's round, and only that one.
    for m in h.routed("source_changed") {
        assert_eq!(m.body["audience_set"], json!(R));
    }
    for m in h.routed("facts") {
        assert_eq!(m.hop["audience_now"], json!(R));
    }
}

#[test]
fn a_promotion_pushes_the_brief_once() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let id = active_row(&mut h);
    let head = h.head(&id);
    let sc = h.routed("source_changed");
    assert_eq!(sc.len(), 1, "{:?}", h.out);
    let version = sc[0].body["version"].as_str().unwrap().to_string();
    assert_eq!(version.len(), 12);
    assert_eq!(
        sc[0].fields(),
        json!({"source": id, "version": version, "path": "", "fmt": "object",
               "parser": "objects", "mark": "", "nodes": 1, "links": 0, "tomb": false,
               "audience_set": R})
    );
    let fa = h.routed("facts");
    assert_eq!(fa.len(), 1);
    assert_eq!(
        Value::Object(fa[0].hop.clone()),
        json!({"route": "facts", "audience_now": R, "subject": id, "recall_caller": "objects"})
    );
    assert_eq!(fa[0].fields(), json!({"subject": id, "limit": 3}));
    // GH #948: memory's `in_query {subject}` takes `messages: []` in its body.
    assert_eq!(fa[0].body["messages"], json!([]), "{:?}", fa[0].body);

    // Memory answers (newest first, as `in_query {subject}` hands them): the
    // brief goes to the curator, the aliases to memory.
    let answer = facts_answer(&id, &["it was bought in May", "it has a bell"]);
    let before = h.out.len();
    h.lane(
        "in_facts",
        json!({"audience_set": R}),
        json!({"recall_caller": "objects", "recall_id": "r1", "subject": id}),
        answer.clone(),
    );
    let new: Vec<Msg> = h.out[before..].to_vec();
    let routes: Vec<&str> = new.iter().map(Msg::route).collect();
    assert_eq!(routes, ["candidate", "alias"], "{new:?}");
    let text = render(&head, json!(["it was bought in May", "it has a bell"]));
    assert_eq!(new[0].hop["audience_set"], json!(R), "the row's round");
    assert_eq!(
        new[0].fields(),
        json!({"source": "objects",
               "candidates": [{"id": id, "text": text, "triggers": ["Blue Bike"],
                               "once": false}]})
    );
    assert_eq!(
        new[1].fields(),
        json!({"aliases": [{"alias": "Blue Bike", "canonical": id}], "force": false})
    );
    // GH #948: `in_alias` takes `messages: []`; `alias_tag` comes back on the ack.
    assert_eq!(new[1].body["messages"], json!([]));
    assert_eq!(new[1].hop["alias_tag"], json!(id));

    // The same facts again: no news, nothing pushed.
    let before = h.out.len();
    h.lane(
        "in_facts",
        json!({"audience_set": R}),
        json!({"recall_caller": "objects", "subject": id}),
        answer.clone(),
    );
    assert_eq!(h.out.len(), before, "{:?}", &h.out[before..]);
    // An acknowledgement never causes an `alias`.
    h.lane(
        "alias_ack",
        json!({}),
        json!({"error_code": "", "error_key": "", "alias_tag": id, "episode_id": ""}),
        json!({"done": 0,
               "refused": [{"alias": "Blue Bike", "error_code": "alias_taken",
                            "canonical": "ob-00000000000f"}]}),
    );
    assert_eq!(h.out.len(), before);
    assert!(
        h.stderr.is_empty(),
        "alias_taken is expected: {:?}",
        h.stderr
    );

    // A new alias is news: a new version, and on the facts both pushes.
    let (_, v) = h.tool(
        "object_set",
        json!({"id": id, "slot": "alias", "value": "the bike"}),
        Some(R),
        Some("member:p"),
    );
    assert_eq!(v["ok"], json!(true), "{v}");
    assert_eq!(h.routed("source_changed").len(), 2);
    let before = h.out.len();
    h.lane(
        "in_facts",
        json!({"audience_set": R}),
        json!({"recall_caller": "objects", "subject": id}),
        answer,
    );
    let routes: Vec<String> = h.out[before..]
        .iter()
        .map(|m| m.route().to_string())
        .collect();
    assert_eq!(routes, ["candidate", "alias"]);
    assert_eq!(
        h.out.last().unwrap().body["aliases"],
        json!([{"alias": "Blue Bike", "canonical": id},
               {"alias": "the bike", "canonical": id}])
    );
    assert!(h.store_errors.is_empty(), "{:?}", h.store_errors);
}

#[test]
fn the_facts_are_read_from_the_memory_bundle() {
    if !shipped() {
        return;
    }
    // GH #948: memory answers `in_query {subject}` with a JSON text in
    // `system.memory.bundle.text`; its `candidates[]` are the facts, newest
    // first. Only facts count, a predicate stands in front of its text, a
    // repeat counts once, three at most, in the order memory gave them.
    let id = "ob-0123456789ab";
    let bundle = json!({"subject": id, "as_of": "2026-10-02T10:00:00.000000Z",
                        "answers": "direct", "complete": false,
                        "complete_reason": "the newest 3 facts; older ones exist",
                        "candidates": [
                            {"kind": "fact", "text": "blue", "subject": id, "predicate": "color"},
                            {"kind": "episode", "text": "we talked about it", "who": "member:p"},
                            {"kind": "fact", "text": "it  has a\nbell", "subject": id},
                            {"kind": "fact", "text": "blue", "subject": id, "predicate": "color"},
                            {"kind": "fact", "text": "May", "subject": id, "predicate": "bought_in"},
                            {"kind": "fact", "text": "a fourth", "subject": id}]});
    let body = json!({"system": {"memory": {"bundle": {"text": bundle.to_string()}}},
                      "messages": [{"origin": "tool", "type": "tool_result", "id": "recall",
                                    "text": "- ob-0123456789ab color: green"}]});
    assert_eq!(
        pure("push", "facts_of(bundle_of(ARGS))", body),
        json!(["color: blue", "it has a bell", "bought_in: May"])
    );
    // No bundle -- a refusal, or a body of another shape -- is no answer:
    // never the text turn's lines, never a `facts[]` of the body.
    let refused = json!({"facts": ["a fact"],
                         "messages": [{"origin": "tool", "type": "tool_result",
                                       "id": "r-reject", "text": "- a fact"}]});
    assert_eq!(pure("push", "bundle_of(ARGS)", refused), Value::Null);
    let empty = facts_bundle(id, &[]);
    assert_eq!(empty["answers"], json!("none"));
    assert_eq!(
        pure("push", "facts_of(ARGS)", empty),
        json!([]),
        "an empty answer is news: no facts"
    );
}

#[test]
fn a_refused_question_keeps_the_stored_facts() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let id = active_row(&mut h);
    let head = h.head(&id);
    // The first answer is a refusal (memory's recall names its code on
    // `hop.reject_reason`, GH #948) and carries no bundle; the hop's
    // `subject` names the object. The brief goes out without facts.
    let refusal = json!({"messages": [{"origin": "tool", "type": "tool_result",
                                       "id": "r-reject",
                                       "text": "recall rejected: store_refused"}]});
    let before = h.out.len();
    h.lane(
        "in_facts",
        json!({"audience_set": R}),
        json!({"recall_caller": "objects", "reject_reason": "store_refused", "subject": id}),
        refusal.clone(),
    );
    let new: Vec<Msg> = h.out[before..].to_vec();
    let routes: Vec<&str> = new.iter().map(Msg::route).collect();
    assert_eq!(routes, ["candidate", "alias"], "{new:?}");
    assert_eq!(
        new[0].body["candidates"][0]["text"],
        json!(render(&head, json!([])))
    );
    // An answer with facts, then a refusal again: the refusal is no news, the
    // stored facts stand and nothing is pushed.
    h.lane(
        "in_facts",
        json!({"audience_set": R}),
        json!({"recall_caller": "objects", "subject": id}),
        facts_answer(&id, &["it has a bell"]),
    );
    // An answer with facts is news.
    let pushed = h.routed("candidate");
    assert_eq!(pushed.len(), 2, "{:?}", h.out);
    assert_eq!(
        pushed[1].body["candidates"][0]["text"],
        json!(render(&head, json!(["it has a bell"])))
    );
    let before = h.out.len();
    h.lane(
        "in_facts",
        json!({"audience_set": R}),
        json!({"recall_caller": "objects", "reject_reason": "store_refused", "subject": id}),
        refusal,
    );
    assert_eq!(h.out.len(), before, "{:?}", &h.out[before..]);
    // A refusal that names no object anywhere is dropped with a line.
    h.lane(
        "in_facts",
        json!({"audience_set": R}),
        json!({"recall_caller": "objects", "reject_reason": "missing_audience"}),
        json!({}),
    );
    assert_eq!(h.out.len(), before);
    assert!(
        h.stderr
            .iter()
            .any(|l| l.contains("without an object subject")),
        "{:?}",
        h.stderr
    );
}

#[test]
fn a_bundle_about_another_subject_changes_no_brief() {
    if !shipped() {
        return;
    }
    // Review O I-2 (audience): the member's way back restates the ASKED
    // subject on `hop.subject` (`context.objects_subject`, kept by the `facts`
    // door). That one wins. A bundle naming another subject is this row's
    // question answered with another row's facts -- read under this row's
    // round, briefed under the other row's -- so its facts are dropped with a
    // line and the brief keeps the facts last stored (none here).
    let mut h = Hive::new();
    let id = active_row(&mut h);
    let head = h.head(&id);
    let other = "ob-00000000000f";
    let foreign = "a fact read under another round";
    let before = h.out.len();
    h.lane(
        "in_facts",
        json!({"audience_set": R}),
        json!({"recall_caller": "objects", "subject": id}),
        facts_answer(other, &[foreign]),
    );
    let new: Vec<Msg> = h.out[before..].to_vec();
    assert!(
        new.iter()
            .all(|m| !sj::to_string(&m.body).unwrap_or_default().contains(foreign)),
        "no foreign fact leaves: {new:?}"
    );
    let pushed: Vec<&Msg> = new.iter().filter(|m| m.route() == "candidate").collect();
    assert_eq!(pushed.len(), 1, "{new:?} / {:?}", h.stderr);
    assert_eq!(
        pushed[0].body["candidates"],
        json!([{"id": id, "text": render(&head, json!([])), "triggers": ["Blue Bike"],
                "once": false}]),
        "the asked row's brief, without the foreign facts"
    );
    assert!(
        h.stderr.iter().any(|l| l.contains("names another subject")),
        "{:?}",
        h.stderr
    );
    assert!(
        h.heads().iter().all(|r| r["id"] != json!(other)),
        "no row of the other subject appears"
    );

    // Without the asked subject on the hop an answer is dropped: the bundle's
    // own `subject` never names the row it lands on.
    let before = h.out.len();
    h.lane(
        "in_facts",
        json!({"audience_set": R}),
        json!({"recall_caller": "objects"}),
        facts_answer(&id, &["it has a bell"]),
    );
    assert_eq!(h.out.len(), before, "{:?}", &h.out[before..]);
}

#[test]
fn a_store_refusal_of_an_alias_is_sent_again_twice_at_most() {
    if !shipped() {
        return;
    }
    // OR-BC-75 (memory as built): an entry refused `store_refused` is a
    // refused closing read, and binding the same spelling to the same id again
    // is idempotent -- so the objects send exactly those spellings again,
    // under the same id, at most twice (the round counted in `alias_tag`).
    // `alias_taken` stays silent; nothing else leaves.
    let id = "ob-0123456789ab";
    let mut h = Hive::new();
    let refused = json!({"done": 1, "refused": [
        {"alias": "the bike", "error_code": "store_refused"},
        {"alias": "Blue Bike", "error_code": "alias_taken"}]});
    let ack = |tag: String| {
        json!({"error_code": "store_refused", "error_key": "select", "alias_tag": tag,
               "episode_id": ""})
    };
    let mut tag = id.to_string();
    for round in 1..=2 {
        let before = h.out.len();
        h.lane("alias_ack", json!({}), ack(tag.clone()), refused.clone());
        let again: Vec<Msg> = h.out[before..].to_vec();
        assert_eq!(again.len(), 1, "round {round}: {again:?}");
        assert_eq!(again[0].route(), "alias");
        assert_eq!(
            again[0].fields(),
            json!({"aliases": [{"alias": "the bike", "canonical": id}], "force": false}),
            "round {round}: only the refused spelling, to the same id"
        );
        assert_eq!(again[0].body["messages"], json!([]));
        tag = again[0].hop["alias_tag"].as_str().unwrap().to_string();
        assert_eq!(tag, format!("{id}#r{round}"));
    }
    let before = h.out.len();
    h.lane("alias_ack", json!({}), ack(tag), refused);
    assert_eq!(h.out.len(), before, "a third refusal is only a line");
    assert!(
        h.stderr.iter().any(|l| l.contains("store_refused")),
        "{:?}",
        h.stderr
    );
    // A tag that is no object's id sends nothing.
    let before = h.out.len();
    h.lane(
        "alias_ack",
        json!({}),
        ack("not-an-id".into()),
        json!({"done": 0, "refused": [{"alias": "x", "error_code": "store_refused"}]}),
    );
    assert_eq!(h.out.len(), before);
}

#[test]
fn the_member_door_stamps_only_the_round_and_the_caller() {
    let member = repo("templates/member/config.json");
    if !shipped() || !member.is_file() {
        return;
    }
    // GH #948: `in_query {subject}` reads its question off the body; the
    // context carries the round (`audience_now`) and `recall_caller` and no
    // channel and no recall key -- `recall_subject` is the memory hive's own.
    // GH #951: plus the member's own `objects_subject`, because memory's
    // refusal names no subject and the way back has to restate it. And the
    // five recall keys and `channel`, each the empty string: the `in_query`
    // lane names them as required context and an edge that states the lane
    // promotes them (gh302 `every_edge_that_states_a_lane_promotes_...`);
    // memory recognises the subject question by its body before it reads any.
    let edges = read_json(&member)["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let door = |to: &str, from: &str, route: &str| -> Vec<Value> {
        edges
            .iter()
            .filter(|e| {
                e["from"] == json!(from)
                    && e["to"] == json!(to)
                    && e["condition"]
                        .as_str()
                        .is_some_and(|c| c.contains(&format!("hop.route == '{route}'")))
            })
            .cloned()
            .collect()
    };
    let facts = door("./memory-hive", "./objects", "facts");
    assert_eq!(facts.len(), 1, "{facts:?}");
    assert_eq!(
        facts[0]["modifier"]["set_hop"]["route"],
        json!("'in_query'")
    );
    let mut keys: Vec<String> = facts[0]["modifier"]["set_context"]
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    let want = [
        "audience_now",
        "channel",
        "memory_tier",
        "objects_subject",
        "recall_as_of",
        "recall_caller",
        "recall_query",
        "recall_window_from",
        "recall_window_to",
    ];
    assert_eq!(keys, want);
    for empty in [
        "channel",
        "memory_tier",
        "recall_as_of",
        "recall_query",
        "recall_window_from",
        "recall_window_to",
    ] {
        assert_eq!(
            facts[0]["modifier"]["set_context"][empty],
            json!("''"),
            "{empty}"
        );
    }
    assert_eq!(
        facts[0]["modifier"]["set_context"]["recall_caller"],
        json!("'objects'")
    );
    assert_eq!(
        facts[0]["modifier"]["set_context"]["objects_subject"],
        json!("has(hop.subject) ? hop.subject : ''")
    );
    // Both ways back restate the subject and take the key out of the context.
    for lane in ["bundle", "reject"] {
        let back = door("./objects", "./memory-hive", lane);
        assert_eq!(back.len(), 1, "{lane}: {back:?}");
        assert_eq!(
            back[0]["modifier"]["set_hop"],
            json!({"route": "'in_facts'",
                   "subject": "has(context.objects_subject) ? context.objects_subject : ''"}),
            "{lane}"
        );
        assert_eq!(
            back[0]["modifier"]["delete_context"],
            json!(["objects_subject"]),
            "{lane}"
        );
    }
    // `in_alias` needs no context at all; its acknowledgement comes home.
    let alias = door("./memory-hive", "./objects", "alias");
    assert_eq!(alias.len(), 1, "{alias:?}");
    assert_eq!(
        alias[0]["modifier"]["set_hop"]["route"],
        json!("'in_alias'")
    );
    assert!(
        alias[0]["modifier"].get("set_context").is_none(),
        "{alias:?}"
    );
    assert_eq!(door("./objects", "./memory-hive", "alias_ack").len(), 1);
}

/// The one member edge `from -> to` whose condition holds for the header.
fn member_edge(from: &str, to: &str, context: &Value, hop: &Value) -> Value {
    let member = read_json(&repo("templates/member/config.json"));
    let (c, h) = (obj(context.clone()), obj(hop.clone()));
    let hits: Vec<Value> = member["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|e| {
            e["from"] == json!(from)
                && e["to"] == json!(to)
                && e["condition"].as_str().is_some_and(|x| {
                    let cond = parse_condition(x).unwrap_or_else(|err| panic!("{x}: {err}"));
                    matches!(evaluate_condition(&cond, &c, &h), Ok(true))
                })
        })
        .collect();
    assert_eq!(hits.len(), 1, "{from} -> {to} for {hop}: {hits:?}");
    hits[0].clone()
}

/// The header after `edge`'s modifier, as `(context, hop)`.
fn cross(edge: &Value, context: &Value, hop: &Value) -> (Value, Value) {
    let spec: ModifierSpec = sj::from_value(edge["modifier"].clone()).expect("a modifier spec");
    let m = parse_modifier(&spec).expect("a modifier");
    let h = Headers::from_parts(obj(context.clone()), obj(hop.clone()));
    let out = apply_modifier(&m, &h).expect("the modifier applies");
    (Value::Object(out.context), Value::Object(out.hop))
}

#[test]
fn a_refusal_without_a_subject_finds_its_object_at_the_member_door() {
    let member = repo("templates/member/config.json");
    if !shipped() || !member.is_file() {
        return;
    }
    // GH #951: memory's `reject` of `in_query {subject}` names no subject,
    // neither on the hop nor in the body. The member's `facts` door keeps
    // the id in its own context; the way back restates it as `hop.subject`.
    let mut h = Hive::new();
    let id = active_row(&mut h);
    let head = h.head(&id);
    let ask = h.routed("facts").remove(0);
    let ctx = json!({"audience_set": R});
    let hop = Value::Object(ask.hop.clone());
    let (ctx, _) = cross(
        &member_edge("./objects", "./memory-hive", &ctx, &hop),
        &ctx,
        &hop,
    );
    assert_eq!(
        ctx["objects_subject"],
        json!(id),
        "the door keeps the id: {ctx}"
    );
    // Memory refuses; its hop carries the code and the caller, nothing else.
    let hop = json!({"route": "reject", "recall_caller": "objects",
                     "error_code": "missing_audience"});
    let (ctx, hop) = cross(
        &member_edge("./memory-hive", "./objects", &ctx, &hop),
        &ctx,
        &hop,
    );
    assert_eq!(hop["route"], json!("in_facts"));
    assert_eq!(hop["subject"], json!(id), "the way back restates the id");
    assert!(ctx.get("objects_subject").is_none(), "{ctx}");
    let before = h.out.len();
    h.lane(
        "in_facts",
        ctx,
        hop,
        json!({"messages": [{"origin": "tool", "type": "tool_result", "id": "r-reject",
                             "text": "recall rejected: missing_audience"}]}),
    );
    let pushed: Vec<Msg> = h.out[before..]
        .iter()
        .filter(|m| m.route() == "candidate")
        .cloned()
        .collect();
    assert_eq!(pushed.len(), 1, "{:?} / {:?}", &h.out[before..], h.stderr);
    assert_eq!(
        pushed[0].body["candidates"],
        json!([{"id": id, "text": render(&head, json!([])), "triggers": ["Blue Bike"],
                "once": false}]),
        "the brief without facts"
    );
}

#[test]
fn the_aliases_go_to_memory_in_pieces_of_32() {
    if !shipped() {
        return;
    }
    // GH #951: memory's `in_alias` takes at most 32 aliases per message; 40
    // aliases are two messages, 32 and 8, in order, each a whole `in_alias`.
    let names: Vec<String> = (0..40).map(|i| format!("name {i:02}")).collect();
    let mut row = stored(json!({}));
    row["aliases"] = json!(json!(names).to_string());
    let msgs = pure("push", "alias_msgs(ARGS)", row);
    let msgs = msgs.as_array().expect("a list of messages");
    let sizes: Vec<usize> = msgs
        .iter()
        .map(|m| m["aliases"].as_array().map_or(0, Vec::len))
        .collect();
    assert_eq!(sizes, [32, 8], "{msgs:?}");
    let sent: Vec<Value> = msgs
        .iter()
        .flat_map(|m| m["aliases"].as_array().cloned().unwrap_or_default())
        .collect();
    let want: Vec<Value> = names
        .iter()
        .map(|n| json!({"alias": n, "canonical": "ob-0123456789ab"}))
        .collect();
    assert_eq!(sent, want, "every alias once, in order");
    for m in msgs {
        assert_eq!(
            m["header"],
            json!({"route": "alias", "alias_tag": "ob-0123456789ab"})
        );
        assert_eq!(m["force"], json!(false));
        assert_eq!(m["messages"], json!([]));
    }
}

#[test]
fn a_round_never_names_nobody() {
    if !shipped() {
        return;
    }
    // GH #951: memory's `audience_now` must never carry a `""`. An empty or
    // non-string entry is dropped; nothing left is the empty round `[]`
    // (roundless); text that is no list stays no round at all.
    let cases = json!([
        r#"["", "member:p"]"#,
        r#"["member:p", 7, null, " "]"#,
        r#"[""]"#,
        "[]",
        "",
        "x",
        r#"{"a": 1}"#
    ]);
    let want = json!([
        r#"["member:p"]"#,
        r#"["member:p"]"#,
        "[]",
        "[]",
        null,
        null,
        null
    ]);
    for cell in ["gate", "tools"] {
        assert_eq!(
            pure(cell, "[canon_round(x) for x in ARGS]", cases.clone()),
            want,
            "{cell}"
        );
    }
}

#[test]
fn the_graph_reads_the_current_version_only() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let id = active_row(&mut h);
    let first = h.routed("source_changed")[0].body["version"]
        .as_str()
        .unwrap()
        .to_string();
    for (slot, value) in [
        ("where", "in the shed"),
        ("who", "member:p"),
        ("doc", DOC),
        ("related", RELATED),
    ] {
        let (_, v) = h.tool(
            "object_set",
            json!({"id": id, "slot": slot, "value": value}),
            Some(R),
            Some("member:p"),
        );
        assert_eq!(v["ok"], json!(true), "{slot}: {v}");
    }
    // A name is no person reference.
    let (_, v) = h.tool(
        "object_set",
        json!({"id": id, "slot": "who", "value": "Paula Smith"}),
        Some(R),
        Some("member:p"),
    );
    assert_eq!(v["error"]["code"], json!("bad_request"), "{v}");
    let sc = h.routed("source_changed");
    assert_eq!(sc.len(), 5, "one announcement per version");
    let last = sc.last().unwrap().body.clone();
    let version = last["version"].as_str().unwrap().to_string();

    let (_, outline) = h.read("outline", &id, &version);
    assert_eq!(
        outline,
        json!({"ok": true, "file": id, "version": version, "next": "", "nodes": [
            {"anchor": "", "kind": "object", "name": "Blue Bike",
             "oneline": "thing: Blue Bike", "parser": "objects"},
            {"anchor": "slot:where", "kind": "slot", "name": "where", "oneline": "in the shed",
             "parser": "objects"},
            {"anchor": "slot:who", "kind": "slot", "name": "who", "oneline": "1 people",
             "parser": "objects"}]})
    );
    assert_eq!(
        last["nodes"],
        json!(3),
        "the announcement counted the nodes"
    );
    let (_, links) = h.read("links", &id, &version);
    assert_eq!(
        links,
        json!({"ok": true, "file": id, "version": version, "next": "", "links": [
            {"kind": "owner", "from_anchor": "", "target_name": "member:p", "class": "extracted"},
            {"kind": "doc", "from_anchor": "", "target_name": DOC, "class": "extracted"},
            {"kind": "related", "from_anchor": "", "target_name": RELATED,
             "class": "extracted"}]})
    );
    assert_eq!(
        last["links"],
        json!(3),
        "the announcement counted the links"
    );

    let (_, stale) = h.read("outline", &id, &first);
    assert_eq!(stale["ok"], json!(false));
    assert_eq!(stale["error"]["code"], json!("stale_version"), "{stale}");
    assert_eq!(stale["file"], json!(id));
    let (_, unknown) = h.read("links", NEVER, &version);
    assert_eq!(
        unknown["error"]["code"],
        json!("unknown_source"),
        "{unknown}"
    );

    // Retired by the owner: the candidate is withdrawn, the source tombed, and
    // the version the graph held is stale.
    let before = h.out.len();
    let (_, v) = h.tool(
        "object_set",
        json!({"id": id, "slot": "state", "value": "retired"}),
        Some(R),
        Some("member:p"),
    );
    assert_eq!(v["ok"], json!(true), "{v}");
    let new: Vec<Msg> = h.out[before..]
        .iter()
        .filter(|m| m.route() != "tool_result")
        .cloned()
        .collect();
    assert_eq!(new.len(), 2, "{new:?}");
    assert_eq!(new[0].route(), "candidate");
    assert_eq!(new[0].hop["audience_set"], json!(R));
    assert_eq!(
        new[0].fields(),
        json!({"source": "objects", "candidates": [], "withdraw": [id]})
    );
    assert_eq!(new[1].route(), "source_changed");
    assert_eq!(
        new[1].fields(),
        json!({"source": id, "path": "", "tomb": true, "audience_set": R})
    );
    let (_, gone) = h.read("outline", &id, &version);
    assert_eq!(gone["error"]["code"], json!("stale_version"), "{gone}");
}
