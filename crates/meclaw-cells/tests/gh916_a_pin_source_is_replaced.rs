//! GH #916 -- a pinning hive replaces what it pinned, source by source.
//!
//! `in_pin` takes an optional `replace_sources: ["<source>", ...]` (each one
//! by the rule a pin's `source` follows, at most 16). Before the new pins are
//! written, every LIVE pin of those sources ends: its `until` becomes now, no
//! row is deleted -- the same visibility rule as an `until` that ran out. A pin
//! of the new set whose hash already lives stays alive (ending first and then
//! finding "same text, same until, nothing to do" would leave it dead), and a
//! hash whose row was ended lives again through the existing `until` update.
//! `pins: []` with `replace_sources` empties those sources (without the field
//! an empty list is still parked). Without the field the door is unchanged.
//!
//! Measured at the ledger (`pins` rows) and in the next request the hive
//! builds for its model. The hive runs in one process
//! (`support/curator_hive.rs`); the colony case of the door is
//! `gh916_an_app_hears_its_member.rs`.
//!
//! Guarded like every template-reading test (GH #49).

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Map, Value, json};

/// The talky instance of the hive, the way `curator_policy.rs` builds it: the
/// composite's `override_params` over the shipped cells.
fn talky_hive() -> Hive {
    let marker = read_json(&repo("templates/talky/curator/config.json"));
    let mut over: Vec<(String, String, Value)> = Vec::new();
    for (cell, params) in marker["override_params"]
        .as_object()
        .cloned()
        .unwrap_or_default()
    {
        for (k, v) in params.as_object().cloned().unwrap_or_default() {
            over.push((cell.clone(), k, v));
        }
    }
    let over: Vec<(&str, &str, Value)> = over
        .iter()
        .map(|(c, k, v)| (c.as_str(), k.as_str(), v.clone()))
        .collect();
    Hive::with(&over)
}

fn pin_el(text: &str, source: &str) -> Value {
    json!({"type": "pin", "source": source, "text": text})
}

fn pin_door(h: &mut Hive, body: Value) {
    h.lane("in_pin", json!({}), json!({}), body);
}

/// `(source, text, until)` of every `pins` row, by source and text.
fn ledger(h: &Hive) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = h
        .rows(
            "SELECT p.source, b.body, p.until FROM pins p JOIN blocks b ON b.hash = p.hash \
             ORDER BY p.source, b.body",
        )
        .into_iter()
        .map(|r| {
            let body: Value =
                meclaw_core::serde_json::from_str(r[1].as_str().unwrap_or("{}")).unwrap();
            (
                r[0].as_str().unwrap_or("").to_string(),
                body["text"].as_str().unwrap_or("").to_string(),
                r[2].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

fn live(h: &Hive, source: &str) -> Vec<String> {
    ledger(h)
        .into_iter()
        .filter(|(s, _, u)| s == source && u.is_empty())
        .map(|(_, t, _)| t)
        .collect()
}

fn ended(h: &Hive, source: &str) -> Vec<String> {
    ledger(h)
        .into_iter()
        .filter(|(s, _, u)| s == source && !u.is_empty())
        .map(|(_, t, _)| t)
        .collect()
}

/// Two sources with pins: `probe` holds `one` and `two`, `other` holds `third`.
fn seeded() -> Hive {
    let mut h = Hive::new();
    pin_door(
        &mut h,
        json!({"pins": [
            {"text": "one", "source": "probe"},
            {"text": "two", "source": "probe"},
            {"text": "third", "source": "other"}
        ]}),
    );
    assert_eq!(ledger(&h).len(), 3, "{:?}", h.stderr);
    h
}

#[test]
fn without_the_field_the_door_writes_what_it_wrote() {
    if !shipped() {
        return;
    }
    let mut h = seeded();
    h.ledger_ops.clear();
    pin_door(
        &mut h,
        json!({"pins": [{"text": "four", "source": "probe"}]}),
    );
    // Nothing ends: every pin stays alive, the new one joins.
    assert_eq!(live(&h, "probe"), ["four", "one", "two"]);
    assert_eq!(live(&h, "other"), ["third"]);
    // The door's own reads and writes are the ones it made before the field
    // existed: the parked payload carries `lane` and `pins` only, nothing
    // reads the pins of a source, nothing updates a `until`.
    let ops: Vec<&Value> = h
        .ledger_ops
        .iter()
        .filter(|(from, _)| from == "intake")
        .map(|(_, a)| a)
        .collect();
    let parked = ops
        .iter()
        .find(|a| a["operation"] == "insert" && a["table"] == "state")
        .expect("step A parks the payload");
    let stash: Value = meclaw_core::serde_json::from_str(
        parked["row"]["value"].as_str().expect("the parked value"),
    )
    .unwrap();
    let keys: Vec<&String> = stash.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["lane", "pins"], "{stash}");
    assert!(
        !ops.iter().any(|a| a["where"].get("source").is_some()),
        "no read by source without the field: {ops:?}"
    );
    assert!(
        !ops.iter().any(|a| a["operation"] == "update"),
        "no until moved without the field: {ops:?}"
    );
}

#[test]
fn replace_sources_ends_exactly_the_named_sources() {
    if !shipped() {
        return;
    }
    let mut h = seeded();
    pin_door(
        &mut h,
        json!({"pins": [
            {"text": "one", "source": "probe"},
            {"text": "five", "source": "probe"}
        ], "replace_sources": ["probe"]}),
    );
    // `one` was in the new set and lived: it stays alive. `two` ends, `five`
    // joins, `other` is untouched, and no row is gone.
    assert_eq!(live(&h, "probe"), ["five", "one"], "{:?}", h.stderr);
    assert_eq!(ended(&h, "probe"), ["two"]);
    assert_eq!(live(&h, "other"), ["third"]);
    assert_eq!(ledger(&h).len(), 4);
    // The end is a time, and it is the past by the next reader's clock.
    let until = h.rows(
        "SELECT p.until FROM pins p JOIN blocks b ON b.hash = p.hash \
         WHERE b.body LIKE '%\"two\"%'",
    );
    let until = until[0][0].as_str().unwrap().to_string();
    let t = chrono::DateTime::parse_from_rfc3339(&until).expect("until is RFC 3339");
    assert!(t <= chrono::Utc::now(), "{until}");
}

#[test]
fn an_ended_pin_pinned_again_lives_again() {
    if !shipped() {
        return;
    }
    let mut h = seeded();
    pin_door(
        &mut h,
        json!({"pins": [{"text": "five", "source": "probe"}],
               "replace_sources": ["probe"]}),
    );
    assert_eq!(live(&h, "probe"), ["five"]);
    pin_door(
        &mut h,
        json!({"pins": [{"text": "two", "source": "probe"},
                        {"text": "five", "source": "probe"}],
               "replace_sources": ["probe"]}),
    );
    assert_eq!(live(&h, "probe"), ["five", "two"], "{:?}", h.stderr);
    assert_eq!(ended(&h, "probe"), ["one"]);
    assert_eq!(ledger(&h).len(), 4, "a new until, never a new row");
}

#[test]
fn an_empty_set_with_replace_sources_empties_them() {
    if !shipped() {
        return;
    }
    let mut h = seeded();
    pin_door(&mut h, json!({"pins": [], "replace_sources": ["probe"]}));
    assert!(live(&h, "probe").is_empty(), "{:?}", h.stderr);
    assert_eq!(ended(&h, "probe"), ["one", "two"]);
    assert_eq!(live(&h, "other"), ["third"]);
    // Without the field an empty list still writes nothing.
    let mut h = seeded();
    pin_door(&mut h, json!({"pins": []}));
    assert_eq!(live(&h, "probe"), ["one", "two"]);
    assert!(
        h.stderr.join("").contains("nothing written"),
        "{:?}",
        h.stderr
    );
}

#[test]
fn a_bad_replace_sources_refuses_the_message() {
    if !shipped() {
        return;
    }
    let many: Vec<String> = (0..17).map(|i| format!("s{i}")).collect();
    for bad in [
        json!(["model"]),
        json!(["probe", "no/segment"]),
        json!([""]),
        json!("probe"),
        json!(many),
    ] {
        let mut h = seeded();
        pin_door(
            &mut h,
            json!({"pins": [{"text": "five", "source": "probe"}],
                   "replace_sources": bad.clone()}),
        );
        assert_eq!(
            live(&h, "probe"),
            ["one", "two"],
            "{bad}: nothing ended, nothing written"
        );
        assert!(
            h.stderr.join("").contains("replace_sources"),
            "{bad}: the refusal is said: {:?}",
            h.stderr
        );
    }
}

#[test]
fn the_next_request_carries_only_the_new_set() {
    if !shipped() {
        return;
    }
    let mut h = talky_hive();
    turn(&mut h, "s1", "t1", "a", "b", json!({}));
    pin_door(
        &mut h,
        json!({"pins": [{"text": "old", "source": "probe"},
                        {"text": "kept", "source": "probe"}]}),
    );
    // A rebuild moves the first set into the system part.
    let c = h.curate("s1", "t2", 0, json!([user("c")]), mode("x"));
    h.tap(
        &c,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("d")]),
    );
    h.fire(&last_add(&h));
    let c = h.curate("s1", "t3", 0, json!([user("e")]), mode("x"));
    let mut first = Map::new();
    for t in ["old", "kept"] {
        first.insert(id(&pin_el(t, "probe")), json!({"text": t}));
    }
    assert_eq!(
        c.body["system"]["pinned"],
        json!({"$replace": true, "probe": first})
    );
    h.tap(
        &c,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("f")]),
    );
    // The second set replaces the first; `kept` is repeated unchanged.
    pin_door(
        &mut h,
        json!({"pins": [{"text": "kept", "source": "probe"},
                        {"text": "new", "source": "probe"}],
               "replace_sources": ["probe"]}),
    );
    assert_eq!(live(&h, "probe"), ["kept", "new"], "{:?}", h.stderr);
    assert_eq!(ended(&h, "probe"), ["old"]);
    // Warm: the new pin where it arrived, the old one nowhere in the request.
    let c = h.curate("s1", "t4", 0, json!([user("g")]), mode("x"));
    let shown = texts(&c);
    assert!(
        shown.iter().any(|t| t.ends_with("[pinned by probe] new")),
        "{shown:?}"
    );
    assert!(!shown.iter().any(|t| t.contains("old")), "{shown:?}");
    // Rebuilt: the system part holds the second set only.
    h.tap(
        &c,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:01:00Z"}),
        json!([said("h")]),
    );
    h.fire(&last_add(&h));
    let c = h.curate("s1", "t5", 0, json!([user("i")]), mode("x"));
    let mut second = Map::new();
    for t in ["kept", "new"] {
        second.insert(id(&pin_el(t, "probe")), json!({"text": t}));
    }
    assert_eq!(
        c.body["system"]["pinned"],
        json!({"$replace": true, "probe": second})
    );
    assert!(!texts(&c).iter().any(|t| t.contains("pinned by")));
}

fn id(el: &Value) -> String {
    short_id(el)
}
