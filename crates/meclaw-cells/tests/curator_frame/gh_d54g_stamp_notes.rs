//! D54g stamp (GH #1506, contract points 6 to 9) -- a class note may key on
//! class and move; a move steers nothing but the speaking; the `route` mark
//! says how sure the early verdict was.
//!
//! WHY this file exists: the early router's stamp may carry a move -- a name
//! out of a list the composition hands it, for HOW the speaker answers this
//! turn. The curator turns a stamp class into a turn note (`class_notes`,
//! keyed by the class alone until now). A move needs its own words: an entry
//! keyed `<class>/<move>` wins over the class's entry when the stamp carries
//! that move; without such an entry the class's entry stands, without either
//! there is no note. The curator knows no move by name -- the key is the
//! composition's. And a move is about speaking only: it picks no target,
//! opens and closes no tool and changes no way, so of a `<class>/<move>`
//! entry only the text counts, and the question to the switch never carries
//! the move.
//!
//! The `route` mark of the ledger (`{call, path, way, reason?}`) is the one
//! place a running colony keeps what the early verdict did. It gains the
//! stamp's trail -- first verdict, confidence, fallback reason, whether a
//! second judge sharpened it, that judge's verdict and how long it took --
//! so the bars can be calibrated from the ledger. Never the person's words.
//!
//! THE NAMES THESE PINS FIX (strand ruling OR-d54g.stamp.1, one block with
//! `gh_d54g_stamp_route.rs`): the stamp's move reaches the curate seam as
//! `hop.move`, its trail as `hop.trail` (the JSON text of an object, as
//! `needs_yes` travels); a `class_notes` key with a move is
//! `<class>/<move>`; the params of `talky/route` are `sure`, `sharpen`,
//! `second_judge`, `second_wait_ms` and `moves`.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;
// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use curator_hive::*;
use curator_target::*;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{Value, json};

const NOTE_CLASS: &str = "Answer in two short sentences.";
const NOTE_MOVE: &str = "Say back what you heard before anything else.";
const NOTE_OTHER: &str = "Give the fact first, then one sentence of warmth.";

/// The notes of a composition: one class, the same class with a move, and
/// another class with that move.
///
/// WHY classes the harness's lead routes nowhere: a class note reaches the
/// speaker through the notes block of the target the call is cut for
/// (GH #1512 bound the turn's row to its target). The harness wires the lead
/// the way talky does (`LEAD_WIRING`: `talk` to `brain`, `simple` to `quick`,
/// `complex` and `research` away), so a turn stamped `talk` is cut for
/// `brain`, not for `seat`, and `seat`'s block never shows. Measured on
/// 9fdd650fd3: with `talk` and `simple` here, no note stood in the window of
/// any case. The classes below have no route, so the call stays on `seat`,
/// the one target whose profile carries a notes share -- the form of
/// `gh_d58_frame_a_class_note_follows_the_stamp_class`.
fn notes() -> Value {
    json!({"chat": {"text": NOTE_CLASS},
           "chat/mirror": {"text": NOTE_MOVE},
           "aside/mirror": {"text": NOTE_OTHER}})
}

/// One curate of `turn` for the target `seat` with the stamp's keys `stamp`
/// on the hop (`path`, `move`, ...); the one call.
fn curate_stamped(h: &mut Hive, turn: &str, words: &str, stamp: Value) -> Msg {
    let calls = curate_for(h, Some("seat"), turn, 0, json!([user(words)]), stamp);
    the_call(h, &calls)
}

fn in_window(call: &Msg, text: &str) -> bool {
    call.body
        .get("messages")
        .is_some_and(|m| m.to_string().contains(text))
}

fn close(h: &mut Hive, call: &Msg) {
    answer(h, call, "stop", warm_usage(), json!([said("Fine.")]));
}

/// The `route` mark of `call`.
fn route_mark(h: &Hive, call: &Msg) -> Value {
    marks_of_kind(h, "route")
        .into_iter()
        .find(|m| m["call"] == call.hop["curator_call"])
        .unwrap_or_else(|| panic!("no route mark of the call: {:?}", h.stderr))
}

/// Contract point 6: a stamp with a move takes the entry of class and move,
/// else the entry of the class, else none.
#[test]
fn gh_d54g_stamp_a_class_note_may_key_on_class_and_move() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[("policy", "class_notes", notes())]);
    announce_profile(&mut h, "seat", json!({"notes_share": 0.02, "speaks": true}));
    let mut failures = Vec::new();
    // (turn, stamp, the note that stands, the notes that do not).
    let cases = [
        (
            "t1",
            json!({"path": "chat", "move": "mirror"}),
            Some(NOTE_MOVE),
            "class and move: the entry of both wins",
        ),
        (
            "t2",
            json!({"path": "chat", "move": "steady"}),
            Some(NOTE_CLASS),
            "a move without an entry: the entry of the class",
        ),
        (
            "t3",
            json!({"path": "chat"}),
            Some(NOTE_CLASS),
            "no move: the entry of the class, as before",
        ),
        (
            "t4",
            json!({"path": "aside", "move": "mirror"}),
            Some(NOTE_OTHER),
            "a class with an entry for the move only",
        ),
        (
            "t5",
            json!({"path": "aside"}),
            None,
            "that class without the move: no entry, no note",
        ),
        (
            "t6",
            json!({"path": "other", "move": "mirror"}),
            None,
            "neither the class nor class and move: no note",
        ),
        (
            "t7",
            json!({"path": "chat", "move": "mirror", "consult_id": "k-errand-1"}),
            None,
            "the turn that answers an errand: no class note, with a move too",
        ),
    ];
    for (turn, stamp, want, why) in cases {
        let c = curate_stamped(&mut h, turn, "It was a long day.", stamp);
        for note in [NOTE_CLASS, NOTE_MOVE, NOTE_OTHER] {
            let stands = in_window(&c, note);
            if stands != (want == Some(note)) {
                failures.push(format!(
                    "{turn} ({why}): {note:?} in the window is {stands}: {:?}",
                    h.stderr
                ));
            }
        }
        close(&mut h, &c);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The scope the composite brings on the curates of the scope half.
fn scope_in() -> Value {
    json!({"allow": ["web", "weather"], "deny": ["bash"]})
}

/// One curate of `turn` for `target` with the composite's scope in the body
/// and the stamp's keys `stamp` on the hop; the one call.
fn curate_scoped(h: &mut Hive, target: &str, turn: &str, stamp: Value) -> Msg {
    h.out.clear();
    let mut hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                         "target": target});
    for (k, v) in obj(stamp) {
        hop[k.as_str()] = v;
    }
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E});
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user("It was a long day.")], "system": system_tree(),
               "tool_scope": scope_in()}),
    );
    let calls = model_calls(h);
    the_call(h, &calls)
}

/// Contract points 7 and 8: the move steers the speaking and nothing else --
/// same target, same way, same tools, nothing for the switch --, and the
/// `route` mark carries the stamp's trail beside the way.
#[test]
fn gh_d54g_stamp_a_move_never_changes_target_tools_or_way() {
    if !shipped() {
        return;
    }
    let mut failures = Vec::new();

    // The speaking half: the move's note stands, target, way and mark stay.
    let mut h = Hive::with(&[("policy", "class_notes", notes())]);
    announce_profile(&mut h, "seat", json!({"notes_share": 0.02, "speaks": true}));
    let plain = curate_stamped(&mut h, "t1", "It was a long day.", json!({"path": "chat"}));
    let plain_mark = route_mark(&h, &plain);
    close(&mut h, &plain);
    let moved = curate_stamped(
        &mut h,
        "t2",
        "It was a long day.",
        json!({"path": "chat", "move": "mirror"}),
    );
    if !in_window(&moved, NOTE_MOVE) {
        failures.push(format!(
            "the move steers the speaking: its note is not in the window: {:?}",
            h.stderr
        ));
    }
    if way_of(&moved) != way_of(&plain) || moved.hop.get("target") != plain.hop.get("target") {
        failures.push(format!(
            "the move changed where the call goes: {} / {:?} without, {} / {:?} with",
            way_of(&plain),
            plain.hop.get("target"),
            way_of(&moved),
            moved.hop.get("target")
        ));
    }
    let moved_mark = route_mark(&h, &moved);
    for key in ["path", "way"] {
        if moved_mark[key] != plain_mark[key] {
            failures.push(format!(
                "the move changed `{key}` of the route mark: {plain_mark} / {moved_mark}"
            ));
        }
    }
    // The question to the switch never carries the move.
    for ask in h.routed("lead_ask") {
        if ask.body.get("ask").is_some_and(|a| a.get("move").is_some()) {
            failures.push(format!("the switch is asked with the move: {:?}", ask.body));
        }
    }
    close(&mut h, &moved);

    // The same comparison on a class with a route (re-check 1, J1): `chat`
    // has none, so its turn stays on `seat` whatever key reaches the lead,
    // and the pair above cannot see a move that picks the target. `talk` goes
    // to `brain` under the harness's `LEAD_WIRING`; a move keyed into the
    // lead's choice (`talk/mirror`) would land on `seat` (replayed against
    // the shipped `lead_ask()` and `route_and_reason()`). No note is asked
    // for here: `brain` announces no notes share.
    let mut r = Hive::with(&[("policy", "class_notes", notes())]);
    announce_profile(&mut r, "seat", json!({"notes_share": 0.02, "speaks": true}));
    let routed = curate_scoped(&mut r, "seat", "t1", json!({"path": "talk"}));
    close(&mut r, &routed);
    let routed_moved = curate_scoped(
        &mut r,
        "seat",
        "t2",
        json!({"path": "talk", "move": "mirror"}),
    );
    close(&mut r, &routed_moved);
    if routed.hop.get("target") != Some(&json!(MAIN_TARGET)) {
        failures.push(format!(
            "check of the setup: a turn stamped `talk` is cut for `{MAIN_TARGET}`: {:?}",
            routed.hop.get("target")
        ));
    }
    if way_of(&routed_moved) != way_of(&routed)
        || routed_moved.hop.get("target") != routed.hop.get("target")
    {
        failures.push(format!(
            "the move changed where a routed call goes: {} / {:?} without, {} / {:?} with",
            way_of(&routed),
            routed.hop.get("target"),
            way_of(&routed_moved),
            routed_moved.hop.get("target")
        ));
    }
    if routed_moved.body.get("tool_scope") != routed.body.get("tool_scope") {
        failures.push(format!(
            "the move changed the tools of a routed call: {:?} without, {:?} with",
            routed.body.get("tool_scope"),
            routed_moved.body.get("tool_scope")
        ));
    }

    // Contract point 8: without a trail the mark is the mark of before ...
    let mut keys: Vec<&str> = plain_mark
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    keys.sort_unstable();
    if keys != ["call", "path", "way"] {
        failures.push(format!(
            "a stamp without a trail leaves the mark of before: {plain_mark}"
        ));
    }
    // ... and with one it says what the early verdict did, beside the way.
    let trail = json!({"first": "clarify", "p": 0.97, "fallback": "", "sharpened": true,
                       "second": "talk", "second_ms": 812});
    let words = "It was a long day, truly.";
    let sharp = curate_stamped(
        &mut h,
        "t3",
        words,
        json!({"path": "chat", "trail": trail.to_string()}),
    );
    let mark = route_mark(&h, &sharp);
    for (key, want) in obj(trail) {
        if mark[key.as_str()] != want {
            failures.push(format!("the route mark says `{key}` {want}: {mark}"));
        }
    }
    for key in ["path", "way"] {
        if mark[key] != plain_mark[key] {
            failures.push(format!(
                "the trail changed `{key}` of the route mark: {plain_mark} / {mark}"
            ));
        }
    }
    if mark.to_string().contains(words) {
        failures.push(format!("the route mark carries the person's words: {mark}"));
    }
    close(&mut h, &sharp);

    // ... and only names of the question (review R1 M1): the route cell's own
    // trail, of a decider that answered with the person's sentence as its
    // class, reaches the mark without it -- `first` is "" and the fallback
    // reason stands.
    let said = "Please put the drive to my mother in the calendar for two o'clock.";
    let asked_at = (assemble_cell::now_ms() + 3_600_000).to_string();
    let mut decided = assemble_cell::lane(
        "in_decision",
        json!({"finish_reason": "stop", "asked_at": asked_at, "state": said}),
        json!({"turn_id": "t4"}),
        json!([]),
    );
    decided["decision"] = json!({"answers": {"path": {"choice": said, "p": 0.97}},
                                 "missing": [], "model": "d"});
    let (out, _) = assemble_cell::run_cell(
        "templates/talky/route/config.json",
        &[("sure", json!({"path": 0.9}))],
        decided,
    );
    let trail = out
        .iter()
        .find(|m| m["header"]["route"] == "stamp")
        .map(|m| m["stamp"]["trail"].clone())
        .unwrap_or_else(|| panic!("the route cell stamps: {out:?}"));
    if trail["first"] != json!("") || trail["fallback"] != json!("unsure") {
        failures.push(format!("the trail names what was not offered: {trail}"));
    }
    let free = curate_stamped(
        &mut h,
        "t4",
        words,
        json!({"path": "chat", "trail": trail.to_string()}),
    );
    let mark = route_mark(&h, &free);
    if mark.to_string().contains(said) || mark["first"] != json!("") {
        failures.push(format!("the route mark carries free text: {mark}"));
    }

    // The tool half: `tools` of an entry of class and move does not count.
    let mut h = Hive::with(&[(
        "policy",
        "class_notes",
        json!({"chat/mirror": {"text": NOTE_MOVE, "tools": "none"},
               "vague": {"text": NOTE_CLASS, "tools": "none"}}),
    )]);
    announce_profile(&mut h, "wall", json!({"system": "delta", "tools": "all"}));
    let c = curate_scoped(
        &mut h,
        "wall",
        "t1",
        json!({"path": "chat", "move": "mirror"}),
    );
    if c.body.get("tool_scope") != Some(&scope_in()) {
        failures.push(format!(
            "a move closed a tool: the scope is {:?}",
            c.body.get("tool_scope")
        ));
    }
    close(&mut h, &c);
    // Check of the setup: a class's own `tools` "none" does empty the scope.
    let c = curate_scoped(&mut h, "wall", "t2", json!({"path": "vague"}));
    assert_eq!(
        c.body.get("tool_scope"),
        Some(&json!({"allow": [], "deny": ["bash"]})),
        "check of the setup: the class's entry closes the tools"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn shipped_registry() -> TemplatesRegistry {
    let scanned = scan_templates_dir(&repo("templates")).unwrap_or_default();
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

/// `None` when the set's `over` on `template`/`cell` passes the door, else
/// why not.
fn refused(template: &str, cell: &str, over: Value) -> Option<String> {
    let parsed = parse_subtree(&repo(&format!("templates/{template}")), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped {template} template parses: {e:?}"));
    let node = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == cell)
        .unwrap_or_else(|| panic!("{template} has a {cell} cell"));
    check_override_params(&node, Some(cell), template, &over)
        .err()
        .map(|why| format!("a set's {over} on {template}/{cell} must pass the door: {why:?}"))
}

/// Contract points 1, 2, 4, 5 and 9: bars, the list of moves, the second
/// judge and its wait bound are the composition's -- declared by the route
/// with a neutral default, so a set can turn them.
#[test]
fn gh_d54g_stamp_a_set_override_of_the_new_params_passes_the_door() {
    if !shipped() {
        return;
    }
    let mut failures = Vec::new();
    let route = read_json(&repo("templates/talky/route/config.json"));
    for (name, neutral) in [
        ("sure", Some(json!({}))),
        ("sharpen", Some(json!([]))),
        ("second_judge", Some(json!(""))),
        ("moves", Some(json!({}))),
        ("second_wait_ms", None),
    ] {
        let param = &route["params"][name];
        let setting = &route["contract"]["settings"][name];
        if param.is_null() {
            failures.push(format!("the route declares no param `{name}`"));
        }
        if neutral.as_ref().is_some_and(|n| param != n) {
            failures.push(format!("the route ships `{name}` neutral: {param}"));
        }
        if name == "second_wait_ms" && !param.is_number() {
            failures.push(format!("the route ships `{name}` as a number: {param}"));
        }
        if setting.is_null() || &setting["default"] != param {
            failures.push(format!(
                "the setting `{name}` names the shipped default {param}: {setting}"
            ));
        }
        if !setting["description"]
            .as_str()
            .is_some_and(|d| !d.is_empty())
        {
            failures.push(format!("the setting `{name}` says what it does: {setting}"));
        }
    }
    let over = json!({
        "sure": {"path": 0.92, "move": 0.9, "second": 0.9},
        "sharpen": ["path"],
        "second_judge": "judge_two",
        "second_wait_ms": 4000,
        "moves": {"mirror": "the person shares a feeling and wants to be heard"}
    });
    if let Some(why) = refused("talky", "route", over) {
        failures.push(why);
    }
    // A note keyed by class and move is a `class_notes` entry like any other.
    if let Some(why) = refused(
        "curator",
        "policy",
        json!({"class_notes": {"talk/mirror": {"text": NOTE_MOVE}}}),
    ) {
        failures.push(why);
    }
    assert!(
        refused("talky", "route", json!({"sure_never_declared": {}})).is_some(),
        "check of the setup: an undeclared param is refused"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
