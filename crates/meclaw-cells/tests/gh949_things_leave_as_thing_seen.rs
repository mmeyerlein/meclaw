//! GH #949 -- the things a turn names leave the curator as `thing_seen`.
//!
//! The curator offers the model it serves one more sidecar section, `things`:
//! the distinct named things of the person's world a turn mentions (a device,
//! a place, a project, a pet), at most 8 entries `{name, kind?, note?}` under
//! `items`. The offer is off unless `./schemas` `things_section` is "1"; its
//! instruction is the param `things_instruction` (empty: a generic sentence of
//! the cell's own). Off, the menu is the menu of curator 1.5.0.
//!
//! The section comes back on `in_section` like `window` and `memory`.
//! `./intake` carries the same param and drops the section when it is not "1"
//! (a section nobody offered is not let through). On, it checks the form --
//! `name` 1 to 80 characters, `kind` at most 32, `note` at most 200; an entry
//! out of its form is left out whole, a name counts once -- and hands the rest
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
    let mut ctx = call.context.clone();
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
    // On: one more optional section, a list of at most 8 under `items` -- an
    // object, because the splitter drops a bare list under a top-level key.
    let on = menu(&mut hive_on());
    assert_eq!(sections(&on), ["window", "gap", "things"]);
    let offer = on.body["sidecar"][2].clone();
    assert_eq!(offer["required"], json!(false));
    assert_eq!(offer["schema"]["type"], "object");
    let items = &offer["schema"]["properties"]["items"];
    assert_eq!(items["type"], "array");
    assert_eq!(items["maxItems"], json!(8));
    let mut fields: Vec<&String> = items["items"]["properties"]
        .as_object()
        .expect("an entry's fields")
        .keys()
        .collect();
    fields.sort();
    assert_eq!(fields, ["kind", "name", "note"]);
    let said = offer["instruction"].as_str().expect("an instruction");
    assert!(
        said.contains("at most 8") && said.is_ascii(),
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
    let n80 = "n".repeat(80);
    let k32 = "k".repeat(32);
    let x200 = "x".repeat(200);
    things(
        &mut h,
        &call,
        json!(ROUND_E),
        json!({"items": [
            {"name": "router", "kind": "device", "note": "keeps dropping"},
            {"name": "  garage ", "kind": "place"},
            {"name": "Router", "kind": "device"},
            {"name": ""},
            {"name": "n".repeat(81)},
            {"name": n80},
            {"name": "lamp", "kind": "k".repeat(33)},
            {"name": "desk", "note": "x".repeat(201)},
            {"name": "plant", "kind": k32, "note": x200, "extra": 1},
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
            {"name": n80},
            {"name": "plant", "kind": k32, "note": x200}
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
    // At most 8, in the order written.
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
        (0..8).map(|i| format!("thing {i}")).collect::<Vec<_>>()
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
