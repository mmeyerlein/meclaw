//! GH #949 -- the things a turn names leave the curator as `thing_seen`.
//!
//! The model is offered one more sidecar section, `things`: the distinct named
//! things of the person's world a turn mentions (a device, a place, a project,
//! a pet), entries `{name, kind?, note?}` under `items` -- no count of their
//! own (GH #1085). Since GH #1473 the offer is the objects hive's own half of
//! the menu, not the curator's: the curator offers only `window` and `gap`,
//! registered or not.
//!
//! The section comes back on `in_section` like `window` and `memory`.
//! `./intake` hands it on only when the composition registered it
//! (`sections`: `things` on the lane `thing_seen`, `cap_share` 0.1 -- what
//! the shipped compositions register); a section nobody registered is dropped
//! and said. Registered, it leaves as exactly one `thing_seen`: the
//! splitter's `{messages, section, payload: {items}}`, the round's ids and
//! the round (`context.audience_set`) in the context it came with. The
//! entries travel as the model wrote them; the share of the served model's
//! window counts, the first entry alone over it is clipped with a mark and the
//! rest named in `cut` (GH #1085, OR-IG-11). The form check of an entry and
//! the rule "never without a round" are the receiving hive's since GH #1473:
//! an entry out of its form, a name a second time and a round-less sighting
//! keep no row in the objects hive.
//!
//! The hives run in one process (`support/curator_hive.rs`,
//! `support/objects_hive.rs`). Guarded like every template-reading test
//! (GH #49).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/objects_hive.rs"]
#[allow(unused_imports)]
use crate::objects_hive; // harness_fold: mod objects_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};
use objects_hive as obj;

/// The hive with `things` registered as the shipped compositions register it.
fn hive_on() -> Hive {
    Hive::with(&[(
        "intake",
        "sections",
        json!({"things": {"lane": "thing_seen", "cap_share": 0.1}}),
    )])
}

/// The menu answer to a collector's `in_schemas`.
fn menu(h: &mut Hive) -> Msg {
    h.out.clear();
    h.lane(
        "in_schemas",
        json!({}),
        json!({}),
        json!({"messages": [], "tools": ["*"]}),
    );
    let a = h.routed("tool_schemas");
    assert_eq!(a.len(), 1, "one menu answer: {:?}", h.out);
    a[0].clone()
}

fn sections(m: &Value) -> Vec<String> {
    m["sidecar"]
        .as_array()
        .expect("sidecar[]")
        .iter()
        .map(|o| o["section"].as_str().unwrap_or("").to_string())
        .collect()
}

/// One round of the person, the call that left for the model.
fn call_of(h: &mut Hive) -> Msg {
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user("The new router in the garage keeps dropping.")]),
        mode("Be brief."),
    );
    assert!(
        call.hop
            .get("episode_turn_id")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty()),
        "the call names the person's episode (GH #941): {:?}",
        call.hop
    );
    call
}

/// The section `things` of the call's answer, as the parent's splitter hands
/// it on: the context the brain edge promoted the call's keys into
/// (`episode_turn_id` included, GH #941), under the round `round`.
fn things(h: &mut Hive, call: &Msg, round: Value, payload: Value) {
    things_in(h, call, round, payload, None)
}

/// `things` with the served model's window (`input_soft`, tokens) in context.
fn things_in(h: &mut Hive, call: &Msg, round: Value, payload: Value, window: Option<u64>) {
    let mut ctx = call.context.clone();
    if let Some(w) = window {
        ctx.insert("input_soft".into(), json!(w));
    }
    for k in [
        "curator_call",
        "turn_id",
        "session_id",
        "iter",
        "episode_turn_id",
    ] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert("audience_set".into(), round);
    h.out.clear();
    h.lane(
        "in_section",
        Value::Object(ctx),
        json!({"section": "things"}),
        json!({"messages": [], "section": "things", "payload": payload}),
    );
}

/// What the objects hive keeps of one `thing_seen`: the main name of each row.
fn kept(seen: &Msg) -> (Vec<Value>, Vec<String>) {
    let mut o = obj::Hive::new();
    let mut hop = seen.hop.clone();
    hop.remove("route");
    o.lane(
        "thing_seen",
        Value::Object(seen.context.clone()),
        Value::Object(hop),
        Value::Object(seen.body.clone()),
    );
    (
        o.heads().iter().map(|r| obj::col(r, "aliases")).collect(),
        o.stderr.clone(),
    )
}

#[test]
fn the_menu_offers_things_only_when_asked() {
    if !shipped() || !obj::shipped() {
        return;
    }
    // The curator offers its own two, registered or not (GH #1473).
    let off = menu(&mut Hive::new());
    assert_eq!(
        sections(&Value::Object(off.body.clone())),
        ["window", "gap"]
    );
    let on = menu(&mut hive_on());
    assert_eq!(on.body, off.body, "a registration does not change the menu");
    // The offer is the objects hive's: one optional section, a list under
    // `items` -- an object, because the splitter drops a bare list under a
    // top-level key. No count of its own (GH #1085): the share of the window
    // the composition registers counts, at the curator.
    let mut o = obj::Hive::new();
    o.lane("in_schemas", json!({}), json!({}), json!({"tools": ["*"]}));
    let answers = o.routed("tool_schemas");
    assert_eq!(answers.len(), 1, "one menu answer: {:?}", o.out);
    let body = Value::Object(answers[0].body.clone());
    assert_eq!(sections(&body), ["things"]);
    let offer = body["sidecar"][0].clone();
    assert_eq!(offer["required"], json!(false));
    assert_eq!(offer["schema"]["type"], "object");
    let items = &offer["schema"]["properties"]["items"];
    assert_eq!(items["type"], "array");
    assert!(
        items.get("maxItems").is_none(),
        "no count of its own: {items}"
    );
    let mut fields: Vec<&String> = items["items"]["properties"]
        .as_object()
        .expect("an entry's fields")
        .keys()
        .collect();
    fields.sort();
    assert_eq!(fields, ["kind", "name", "note"]);
    let said = offer["instruction"].as_str().expect("an instruction");
    assert!(
        !said.contains("at most") && said.contains("as short as useful") && said.is_ascii(),
        "the instruction is the hive's generic English sentence: {said}"
    );
}

#[test]
fn a_things_section_leaves_as_one_thing_seen() {
    if !shipped() || !obj::shipped() {
        return;
    }
    let hive = read_json(&repo("templates/curator/config.json"));
    assert!(
        hive["params"]["contract"]["emits"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l["route"] == "thing_seen"),
        "the hive declares the route it hands the things out on"
    );
    let mut h = hive_on();
    let call = call_of(&mut h);
    // Over the old 80/32/200 (GH #1085): whole, nothing cut. The entries go
    // as the model wrote them -- their form is the objects hive's to check.
    let n81 = "n".repeat(81);
    let k33 = "k".repeat(33);
    let x201 = "x".repeat(201);
    let sent = json!({"items": [
        {"name": "router", "kind": "device", "note": "keeps dropping"},
        {"name": "  garage ", "kind": "place"},
        {"name": "Router", "kind": "device"},
        {"name": ""},
        {"name": n81},
        {"name": "lamp", "kind": k33},
        {"name": "desk", "note": x201},
        {"name": "plant", "kind": "pot", "note": "green", "extra": 1},
        "a sentence",
        {"name": 7}
    ]});
    things(&mut h, &call, json!(ROUND_E), sent.clone());
    let seen = h.routed("thing_seen");
    assert_eq!(seen.len(), 1, "exactly one: {:?} {:?}", h.out, h.stderr);
    assert_eq!(
        h.out.len(),
        1,
        "and nothing beside it (no `sidecar`): {:?}",
        h.out
    );
    let m = &seen[0];
    assert_eq!(
        m.body["payload"], sent,
        "the entries as written, nothing cut to fit"
    );
    assert_eq!(m.hop["section"], "things");
    assert_eq!(m.context["audience_set"], ROUND_E, "the round rides along");
    // The round's ids ride in the context the brain edge promoted.
    assert_eq!(m.context["turn_id"], call.hop["turn_id"]);
    assert_eq!(m.context["session_id"], "s1");
    assert_eq!(m.context["episode_turn_id"], call.hop["episode_turn_id"]);
    // What `./intake` declares it emits.
    let emits = &cell_config("intake")["contract"]["emits"];
    assert!(
        emits["hop"]["route"]["values"]
            .as_array()
            .unwrap()
            .contains(&json!("thing_seen"))
    );
    for k in m.body.keys() {
        assert!(
            emits["body"][k.as_str()].is_object(),
            "intake declares body.{k}"
        );
    }
    // The receiving hive checks the form (review M2: the whole checked list,
    // as broad as the curator's check before GH #1473): `"a sentence"`,
    // `{"name": 7}` and `{"name": ""}` are left out whole, the field `extra`
    // is not kept, a name counts once (`Router`). The lengths are the objects
    // hive's own: its gate keeps a name up to 80 characters, a kind up to 32
    // and a note up to 200 (`objects/gate`), so `n81` falls and the long
    // kind and note are emptied there.
    let checked = obj::pure(
        "gate",
        "clean_items(ARGS)",
        m.body["payload"]["items"].clone(),
    );
    assert_eq!(
        checked,
        json!([
            {"key": "router", "name": "router", "kind": "device", "note": "keeps dropping"},
            {"key": "garage", "name": "garage", "kind": "place", "note": ""},
            {"key": "lamp", "name": "lamp", "kind": "", "note": ""},
            {"key": "desk", "name": "desk", "kind": "", "note": ""},
            {"key": "plant", "name": "plant", "kind": "pot", "note": "green"}
        ]),
        "the objects hive's form check of what the curator handed on"
    );
    let (names, said) = kept(m);
    let mut names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            r#"["desk"]"#,
            r#"["garage"]"#,
            r#"["lamp"]"#,
            r#"["plant"]"#,
            r#"["router"]"#
        ],
        "the objects hive keeps one row per entry in its form, a name once: {said:?}"
    );
    // No count of its own (until 1.11.x the first 8): all, in the order written.
    let ten: Vec<Value> = (0..10)
        .map(|i| json!({"name": format!("thing {i}")}))
        .collect();
    things(&mut h, &call, json!(ROUND_E), json!({ "items": ten }));
    let seen = h.routed("thing_seen");
    assert_eq!(seen.len(), 1);
    let names: Vec<&str> = seen[0].body["payload"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        (0..10).map(|i| format!("thing {i}")).collect::<Vec<_>>()
    );
    assert!(
        seen[0].body.get("cut").is_none(),
        "nothing cut without a window"
    );
    // Nothing in its form: nothing kept.
    for payload in [
        json!({"items": []}),
        json!({"items": [{"name": ""}]}),
        json!({}),
    ] {
        things(&mut h, &call, json!(ROUND_E), payload.clone());
        let seen = h.routed("thing_seen");
        assert_eq!(seen.len(), 1, "{payload}: {:?}", h.out);
        let (names, _) = kept(&seen[0]);
        assert!(names.is_empty(), "{payload}: {names:?}");
    }
}

#[test]
fn with_the_param_off_the_section_is_dropped() {
    if !shipped() {
        return;
    }
    // Shipped, the curator registers nothing (GH #1473).
    assert_eq!(cell_config("intake")["params"]["sections"], json!({}));
    let mut h = Hive::new();
    let call = call_of(&mut h);
    h.stderr.clear();
    things(
        &mut h,
        &call,
        json!(ROUND_E),
        json!({"items": [{"name": "router"}]}),
    );
    assert!(
        h.out.is_empty(),
        "no `thing_seen`, no `sidecar`: {:?}",
        h.out
    );
    assert!(
        h.stderr.iter().any(|l| l.contains("things")),
        "the drop is said: {:?}",
        h.stderr
    );
    assert_eq!(
        h.rows("SELECT COUNT(*) FROM marks WHERE kind = 'missing_audience'")[0][0],
        json!(0),
        "an unregistered section leaves no mark"
    );
}

#[test]
fn without_a_round_nothing_is_seen() {
    if !shipped() || !obj::shipped() {
        return;
    }
    let mut h = hive_on();
    let call = call_of(&mut h);
    for round in [Value::Null, json!("")] {
        things(
            &mut h,
            &call,
            round.clone(),
            json!({"items": [{"name": "router"}]}),
        );
        // The curator hands the section on; the objects hive keeps nothing
        // of a round-less turn (GH #1473: the rule is the keeper's).
        let seen = h.routed("thing_seen");
        assert_eq!(seen.len(), 1, "round {round}: {:?}", h.out);
        let (names, said) = kept(&seen[0]);
        assert!(
            names.is_empty(),
            "round {round}: never without a round: {names:?}"
        );
        assert!(
            said.iter().any(|l| l.contains("without a round")),
            "round {round}: the drop is said: {said:?}"
        );
    }
}

#[test]
fn things_over_the_budget_of_the_window_are_clipped_and_named() {
    if !shipped() {
        return;
    }
    // GH #1085 (OR-IG-11): the section takes the registered tenth of the
    // served model's window -- `input_soft` 3000 tokens: 300 tokens, 900
    // characters.
    let mut h = hive_on();
    let call = call_of(&mut h);
    let ten: Vec<Value> = (0..10)
        .map(|i| json!({"name": format!("thing {i}"), "note": "x".repeat(300)}))
        .collect();
    things_in(
        &mut h,
        &call,
        json!(ROUND_E),
        json!({ "items": ten }),
        Some(3000),
    );
    let seen = h.routed("thing_seen");
    assert_eq!(seen.len(), 1, "{:?} {:?}", h.out, h.stderr);
    let m = &seen[0];
    let names: Vec<&str> = m.body["payload"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["thing 0", "thing 1"],
        "whole entries, in order, while they fit"
    );
    assert_eq!(
        m.body["cut"],
        json!("...[dropped: 8 things (2456 chars) over budget]"),
        "the rest is named in the content, never left out in silence"
    );
    assert_eq!(
        m.hop["cuts"],
        json!([{"what": "things", "shown": 614, "total": 3070, "unit": "chars"}])
    );
    // One entry alone over the budget: its head and a mark (note first).
    let big = "y".repeat(5000);
    things_in(
        &mut h,
        &call,
        json!(ROUND_E),
        json!({"items": [{"name": "big", "kind": "k", "note": big}]}),
        Some(1000),
    );
    let seen = h.routed("thing_seen");
    let t = &seen[0].body["payload"]["items"][0];
    assert_eq!(t["name"], "big");
    assert_eq!(t["kind"], "k");
    let note = t["note"].as_str().unwrap();
    assert!(
        note.starts_with(&"y".repeat(296))
            && note.ends_with("...[cut: 296 of 5000 chars shown; budget of the window]"),
        "{note}"
    );
    assert!(
        seen[0].body.get("cut").is_none(),
        "nothing dropped: {:?}",
        seen[0].body
    );
}
