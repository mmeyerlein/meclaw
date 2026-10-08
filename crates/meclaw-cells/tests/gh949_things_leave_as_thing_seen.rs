//! GH #949 -- the things a turn names leave the curator as `thing_seen`.
//!
//! The curator offers the model it serves one more sidecar section, `things`:
//! the distinct named things of the person's world a turn mentions (a device,
//! a place, a project, a pet), entries `{name, kind?, note?}` under `items` --
//! no count of their own (GH #1085). The offer is off unless `./schemas` `things_section` is "1"; its
//! instruction is the param `things_instruction` (empty: a generic sentence of
//! the cell's own). Off, the menu is the menu of curator 1.5.0.
//!
//! The section comes back on `in_section` like `window` and `memory`.
//! `./intake` carries the same param and drops the section when it is not "1"
//! (a section nobody offered is not let through). On, it checks the form --
//! `name` a non-empty string, `kind` and `note` strings; an entry out of its
//! form is left out whole, a name counts once; no length of their own: the
//! section takes a tenth of the served model's window, the first entry alone
//! over it is clipped with a mark and the rest named in `cut` (GH #1085,
//! OR-IG-11) -- and hands the rest
//! on as exactly one `thing_seen` `{items, turn_id, episode_turn_id?,
//! session_id}` with the round in `context.audience_set`. Never without a
//! round: a section of a round-less turn is dropped and marked
//! `missing_audience`.
//!
//! The hive runs in one process (`support/curator_hive.rs`). Guarded like
//! every template-reading test (GH #49).

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The hive with the section switched on at both cells.
fn hive_on() -> Hive {
    Hive::with(&[
        ("schemas", "things_section", json!("1")),
        ("intake", "things_section", json!("1")),
    ])
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

fn sections(m: &Msg) -> Vec<String> {
    m.body["sidecar"]
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

#[test]
fn the_menu_offers_things_only_when_asked() {
    if !shipped() {
        return;
    }
    for cell in ["schemas", "intake"] {
        assert_eq!(
            cell_config(cell)["params"]["things_section"],
            "0",
            "{cell}: off unless an instance switches it on"
        );
        assert!(
            cell_config(cell)["contract"]["settings"]["things_section"].is_object(),
            "{cell} declares the setting"
        );
    }
    assert_eq!(cell_config("schemas")["params"]["things_instruction"], "");
    // Off: the sections of curator 1.5.0, and "0" said out loud is the same.
    let off = menu(&mut Hive::new());
    assert_eq!(sections(&off), ["window", "gap"]);
    let zero = menu(&mut Hive::with(&[(
        "schemas",
        "things_section",
        json!("0"),
    )]));
    assert_eq!(zero.body, off.body, "\"0\" is the default menu, whole");
    assert_eq!(zero.hop, off.hop);
    // On: one more optional section, a list under `items` -- an object,
    // because the splitter drops a bare list under a top-level key. No count of
    // its own (GH #1085): the section's share of the window counts at intake.
    let on = menu(&mut hive_on());
    assert_eq!(sections(&on), ["window", "gap", "things"]);
    let offer = on.body["sidecar"][2].clone();
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
        "the default instruction is the cell's generic English sentence: {said}"
    );
    // The words are the instance's to choose.
    let mine = "Name the gadgets the person mentions.";
    let own = menu(&mut Hive::with(&[
        ("schemas", "things_section", json!("1")),
        ("schemas", "things_instruction", json!(mine)),
    ]));
    assert_eq!(own.body["sidecar"][2]["instruction"], mine);
}

#[test]
fn a_things_section_leaves_as_one_thing_seen() {
    if !shipped() {
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
    // Over the old 80/32/200 (GH #1085): whole, nothing cut, nothing left out.
    let n81 = "n".repeat(81);
    let k33 = "k".repeat(33);
    let x201 = "x".repeat(201);
    things(
        &mut h,
        &call,
        json!(ROUND_E),
        json!({"items": [
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
        ]}),
    );
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
        m.body["items"],
        json!([
            {"name": "router", "kind": "device", "note": "keeps dropping"},
            {"name": "garage", "kind": "place"},
            {"name": n81},
            {"name": "lamp", "kind": k33},
            {"name": "desk", "note": x201},
            {"name": "plant", "kind": "pot", "note": "green"}
        ]),
        "the entries in their form, a name once, nothing cut to fit"
    );
    assert_eq!(m.context["audience_set"], ROUND_E, "the round rides along");
    assert_eq!(m.body["turn_id"], call.hop["turn_id"]);
    assert_eq!(m.body["session_id"], "s1");
    assert_eq!(m.body["episode_turn_id"], call.hop["episode_turn_id"]);
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
    // No count of its own (until 1.11.x the first 8): all, in the order written.
    let ten: Vec<Value> = (0..10)
        .map(|i| json!({"name": format!("thing {i}")}))
        .collect();
    things(&mut h, &call, json!(ROUND_E), json!({ "items": ten }));
    let seen = h.routed("thing_seen");
    assert_eq!(seen.len(), 1);
    let names: Vec<&str> = seen[0].body["items"]
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
    // Nothing in its form: nothing seen.
    for payload in [
        json!({"items": []}),
        json!({"items": [{"name": ""}]}),
        json!({}),
    ] {
        things(&mut h, &call, json!(ROUND_E), payload.clone());
        assert!(h.out.is_empty(), "{payload}: {:?}", h.out);
    }
}

#[test]
fn with_the_param_off_the_section_is_dropped() {
    if !shipped() {
        return;
    }
    assert_eq!(cell_config("intake")["params"]["things_section"], "0");
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
        "an unoffered section leaves no mark"
    );
}

#[test]
fn without_a_round_nothing_is_seen() {
    if !shipped() {
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
        assert!(
            h.routed("thing_seen").is_empty(),
            "round {round}: never without a round: {:?}",
            h.out
        );
        assert!(h.out.is_empty(), "round {round}: {:?}", h.out);
    }
    // Said once in the ledger, as a row of no round.
    assert_eq!(
        h.rows(
            "SELECT session_id, turn_id, value, audience_set FROM marks \
             WHERE kind = 'missing_audience'"
        ),
        vec![vec![
            json!("s1"),
            call.hop["turn_id"].clone(),
            json!("things"),
            json!("[]")
        ]]
    );
}

#[test]
fn things_over_the_budget_of_the_window_are_clipped_and_named() {
    if !shipped() {
        return;
    }
    // GH #1085 (OR-IG-11): the section takes a tenth of the served model's
    // window -- `input_soft` 3000 tokens: 300 tokens, 900 characters.
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
    let names: Vec<&str> = m.body["items"]
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
    let t = &seen[0].body["items"][0];
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
