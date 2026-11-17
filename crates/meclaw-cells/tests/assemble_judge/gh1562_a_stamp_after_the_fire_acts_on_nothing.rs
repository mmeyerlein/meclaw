//! GH #1562 (b), OR-54g.latesafe.3 -- ONE bound of the wait, and a stamp past
//! it acts on nothing but the rim.
//!
//! WHY this file exists: the route stamp had two bounds, `late_ms` of the
//! route cell (1500 as shipped) and `route_wait_ms` of the collector (2000 in
//! the member set). Measured on a member colony: decide took 1585 ms, the
//! collector still waited, and the route cell had already cut the verdict as
//! `late` -- the risk the decider answered never reached the window. Now the
//! route cell cuts nothing: it stamps every verdict when it comes, and the
//! collector's `route_wait_ms` is the one bound (pin iii). A stamp that lands
//! after the turn fired must then act on nothing in the collector -- no second
//! assembly, no path, family, need, note or choice on a later iteration of the
//! turn, no stamp for a delegation it opens, and a route-hold notice that holds
//! nothing -- while its rim copy still carries the risk to the guard (pin iv;
//! the rim copy is pinned in `gh1562_a_late_stamp_keeps_its_risk`). The answer
//! never waits for it (R-TR-27): the turn fired without it.
//!
//! The mechanism: the fire marks the stamp it was cut with (`fired` 1, a store
//! message of its own, `collect-route-done`, in the same multi-send right after
//! `collect-done` -- ordered, not one bundle); a later seam and the delegation
//! read take only a marked stamp once the window leg is fired.
//!
//! Runs the shipped `collector/assemble` script and the talky template at the
//! mutation door (`support/assemble_cell.rs`); no model, no clock: every moment
//! is fixed data in the round rows.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use serde_json::{Value, json};

const TURN: &str = "route-channel#0001";
const FRESH: &str = "2999-01-01T00:00:00.000000Z";

fn knob() -> Vec<(&'static str, Value)> {
    vec![("route_wait_ms", json!(150))]
}

fn row(role: &str, payload: Value, fired: i64, iter: i64) -> Value {
    json!({"turn_id": TURN, "iter": iter, "role": role, "turn": payload.to_string(),
           "fired": fired, "recorded_at": FRESH})
}

fn window(fired: i64) -> Value {
    row(
        "leg-window",
        json!({"turns": [{"role": "user", "text": "I can't go on like this",
                          "consult_id": ""}],
               "deferred": 0, "deferred_turns": []}),
        fired,
        0,
    )
}

fn memory() -> Value {
    row("leg-memory", json!({"system": {}, "messages": []}), 0, 0)
}

/// A judged stamp that names everything a seam could carry.
fn stamp() -> Value {
    json!({"turn_id": TURN, "path": "simple", "p": 0.9, "intent": "chat",
           "needs": ["tool"], "families": ["web"], "duration": 2400,
           "choices": {"risk": {"value": "acute", "p": 0.97, "source": "model"}},
           "turn_notes": [{"id": "route.risk", "text": "CRISIS: give the number now."}]})
}

fn route_row(fired: i64) -> Value {
    row("leg-route", stamp(), fired, 0)
}

fn curates(out: &[Value]) -> Vec<&Value> {
    out.iter()
        .filter(|m| m["header"]["route"] == "curate")
        .collect()
}

/// The re-entry of the tool round: iteration 0 asked one tool, its result is in.
fn tool_round(route: Value) -> Vec<Value> {
    let call = json!([{"origin": "assistant", "type": "tool_call", "id": "call-1",
                       "text": "{\"name\":\"web_search\",\"arguments\":\"{}\"}"}]);
    let res = json!({"origin": "tool", "type": "tool_result", "id": "call-1",
                     "text": "sunny"});
    let rows = json!([
        window(1),
        memory(),
        route,
        row("assistant", call, 0, 0),
        row("tool", res, 0, 0)
    ]);
    assemble(
        &knob(),
        bundle_reply("round-check", TURN, &[("c-round-check-read", rows)]),
    )
}

fn route_mark(out: &[Value]) -> Option<Value> {
    out.iter()
        .filter(|m| {
            m["header"]["route"] == "cstore" && m["header"]["phase"] == "collect-route-done"
        })
        .flat_map(calls_of)
        .map(|(_, a)| a)
        .next()
}

#[test]
fn gh1562_the_fire_marks_the_stamp_it_was_cut_with() {
    // A stamp in the set: the turn fires with it and marks it, in the same
    // multi-send as the window leg.
    let out = assemble(
        &knob(),
        bundle_reply(
            "collect",
            TURN,
            &[("c-collect-read", json!([window(0), memory(), route_row(0)]))],
        ),
    );
    assert_eq!(curates(&out).len(), 1, "the turn fires once: {out:?}");
    assert_eq!(hop_str(curates(&out)[0], "path"), "simple", "{out:?}");
    let mark = route_mark(&out).unwrap_or_else(|| panic!("no stamp mark: {out:?}"));
    assert_eq!(mark["operation"], "update", "{mark}");
    assert_eq!(mark["set"], json!({"fired": 1}), "{mark}");
    assert_eq!(
        mark["where"],
        json!({"turn_id": TURN, "role": "leg-route", "fired": 0}),
        "{mark}"
    );

    // No stamp in the set (review M2): the window leg is older than
    // `route_wait_ms`, the turn fires without a stamp and marks nothing -- a
    // stamp that lands between the read-back and a mark must stay unmarked.
    let mut aged = window(0);
    aged["recorded_at"] = json!("2000-01-01T00:00:00.000000Z");
    let rows = json!([aged, memory()]);
    let out = assemble(
        &knob(),
        bundle_reply("collect", TURN, &[("c-collect-read", rows.clone())]),
    );
    // It takes the strike's claim first (GH #1564, re-check m3).
    let out = then_age_claim(&knob(), out, rows);
    assert_eq!(curates(&out).len(), 1, "the turn fires without it: {out:?}");
    assert!(
        route_mark(&out).is_none(),
        "a fire without a stamp marks nothing: {out:?}"
    );
}

#[test]
fn gh1562_a_stamp_after_the_fire_acts_on_nothing() {
    // The turn fired without its stamp (the window leg is fired, no mark):
    // the stamp's own arrival reads a complete set and assembles nothing.
    let out = assemble(
        &knob(),
        bundle_reply(
            "collect",
            TURN,
            &[("c-collect-read", json!([window(1), memory(), route_row(0)]))],
        ),
    );
    assert!(
        curates(&out).is_empty() && route_mark(&out).is_none(),
        "no second assembly, no mark for a stamp after the fire: {out:?}"
    );
    assert!(
        out.iter().all(|m| m["header"]["route"] != "recall"),
        "no recall for a turn that left: {out:?}"
    );
    // A route-hold notice after the fire (a sharpening that began late) holds
    // nothing either.
    let hold = row("route-hold", json!({"until_ms": i64::MAX / 2}), 0, 0);
    let out = assemble(
        &knob(),
        bundle_reply(
            "collect",
            TURN,
            &[("c-collect-read", json!([window(1), memory(), hold]))],
        ),
    );
    assert!(
        curates(&out).is_empty(),
        "a late hold assembles nothing: {out:?}"
    );

    // Seam it >= 1: the tool round of the fired turn re-enters. An unmarked
    // stamp (landed after the fire) reaches no part of it ...
    let out = tool_round(route_row(0));
    let c = curates(&out);
    assert_eq!(c.len(), 1, "the round re-enters once: {out:?}");
    assert_eq!(hop_str(c[0], "iter"), "1", "{out:?}");
    for key in ["path", "families", "needs", "turn_notes"] {
        assert_eq!(
            hop_str(c[0], key),
            "",
            "a stamp after the fire sets no `{key}` on a later iteration: {}",
            c[0]["header"]
        );
    }
    assert!(
        c[0]["header"].get("choices").is_none(),
        "nor its choices: {}",
        c[0]["header"]
    );
    // ... while the stamp the turn fired with (marked) stays on every seam of
    // the round, as before.
    let out = tool_round(route_row(1));
    let c = curates(&out);
    assert_eq!(hop_str(c[0], "path"), "simple", "{out:?}");
    assert_eq!(hop_str(c[0], "families"), "web", "{out:?}");
    assert!(
        hop_str(c[0], "turn_notes").contains("route.risk"),
        "{out:?}"
    );
}

#[test]
fn gh1562_a_delegation_inherits_only_the_stamp_its_turn_fired_with() {
    // The delegation's turn-open reads the stamp of its turn -- only a marked
    // one, so a stamp that landed after the fire opens no delegation's way.
    // (The read itself: `fu1_a_delegation_round_takes_its_turns_stamp`.)
    let script = config_of("templates/collector/assemble/config.json")["params"]["script_inline"]
        .as_str()
        .expect("script")
        .to_string();
    assert!(
        script.contains(r#""where": {"turn_id": label, "role": "leg-route", "fired": 1}"#),
        "the delegation read takes the marked stamp alone"
    );
}

fn core_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn shipped_registry() -> TemplatesRegistry {
    let scanned = scan_templates_dir(&core_root().join("templates")).unwrap_or_default();
    TemplatesRegistry::from_entries(
        scanned
            .into_iter()
            .map(|s| TemplateEntry {
                template_id: format!("scan-{}", s.name),
                name: s.name,
                version: s.version,
                filesystem_path: s.filesystem_path,
            })
            .collect(),
    )
}

#[test]
fn gh1562_there_is_one_bound_and_late_ms_changes_nothing() {
    // Pin (iii), OR-54g-261: the route cell still declares `late_ms` (null as
    // shipped) so a set that names it is accepted, but the value changes
    // nothing -- a slow verdict is the same stamp with and without it -- and
    // the cell says so on stderr, never in silence.
    let route = config_of("templates/talky/route/config.json");
    assert_eq!(
        route["params"]["late_ms"],
        Value::Null,
        "late_ms is declared, null as shipped"
    );
    let parsed = parse_subtree(&repo("templates/talky"), &shipped_registry())
        .expect("the shipped talky parses");
    let cell = parsed
        .cells
        .iter()
        .find(|c| c.rel_path == "route")
        .expect("talky has ./route");
    check_override_params(cell, Some("route"), "talky", &json!({"late_ms": 1500}))
        .expect("a set with late_ms is accepted");

    let slow = || {
        let mut doc = lane(
            "in_decision",
            json!({"finish_reason": "stop",
                   "asked_at": (now_ms() - 600_000).to_string()}),
            json!({"turn_id": "c-1"}),
            json!([]),
        );
        doc["decision"] = json!({"answers": {
            "path": {"choice": "simple", "p": 0.91},
            "intent": {"choice": "question", "p": 0.8},
            "needs.tool": {"yes": 0.9},
            "families.web": {"yes": 0.95}}, "missing": [], "model": "d"});
        doc
    };
    let stamp_of = |out: Vec<Value>| -> Value {
        let mut s = out
            .into_iter()
            .find(|m| m["header"]["route"] == "stamp")
            .unwrap_or_else(|| panic!("no stamp"))["stamp"]
            .clone();
        s.as_object_mut().expect("stamp").remove("duration");
        s
    };
    let (plain, plain_err) = run_cell("templates/talky/route/config.json", &[], slow());
    let (set, set_err) = run_cell(
        "templates/talky/route/config.json",
        &[("late_ms", json!(1500))],
        slow(),
    );
    let warn = "route: late_ms is ignored; route_wait_ms is the one bound";
    assert!(!plain_err.contains(warn), "no warning unset: {plain_err}");
    assert!(set_err.contains(warn), "the set value is named: {set_err}");
    let (plain, set) = (stamp_of(plain), stamp_of(set));
    assert_eq!(plain["path"], "simple", "{plain}");
    assert_eq!(set, plain, "late_ms changes nothing");
    // The one bound is the collector's.
    let collector = config_of("templates/collector/assemble/config.json");
    assert!(
        collector["params"].get("route_wait_ms").is_some(),
        "route_wait_ms is the one bound of the wait"
    );
}
