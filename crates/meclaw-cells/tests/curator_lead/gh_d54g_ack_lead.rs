//! D54g ack (GH #1504, contract points 1, 4, 6 and 8) -- the lead orders a
//! side call only for a listed condition, only hears the turn of a running
//! session when told so, and sends an event round to the target its route
//! names.
//!
//! WHY this file exists: in text a turn that needs a tool got two messages,
//! a bridge after 1.6 to 3.3 s and then the answer (measured in the lab of
//! this strand: 20 of 20 turns), because the lead orders a `must_call` side
//! call for every fast target with `tool` in `needs`. And the turn of a
//! running session whose model speaks itself was answered twice: by that
//! model and by the target the lead named. Three knobs of the lead end
//! both, each shipped so that nothing changes:
//!
//! * `side_when` lists the conditions a side call is ordered for (shipped:
//!   `must_call` and `external`); a condition that is not listed gets no
//!   entry in `side`, everything else of the verdict stays.
//! * `live_turn` `listen` (shipped: `call`) names no target for the turn of
//!   a running session: `target ""`, `listen: true`, no side call, no feed,
//!   reason `listen`; a speculative ask of that turn parks. Never for an
//!   event round and never for the round that answers a consult -- those
//!   would stay without an answer.
//! * `event_routes` (shipped empty) names the target of an event round by
//!   its kind (`delegation` today, said by the new head field `event`): the
//!   round goes there without a stamp, reason `event`, with no side call
//!   and no waiting-time feed (the session's own model leads the talk);
//!   fallback and escalation hold as for every target. Without an entry
//!   everything is as before.
//!
//! Every knob is set as the override of an instance, never in the template;
//! the first pin that switches a knob also proves that a set can
//! (`check_override_params`).
//!
//! Runs the pure half of the shipped lead (`support/curator_hive.rs`
//! `cell_scope`, `support/curator_lead.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_lead.rs"]
#[allow(unused_imports)]
use crate::curator_lead; // harness_fold: mod curator_lead;

use curator_hive::*;
use curator_lead::*;
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{Value, json};

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

/// A set's override `over` of the lead cell passes the door of a birth.
fn passes_the_door(over: Value) {
    let parsed = parse_subtree(
        &core_root().join("templates").join("lead"),
        &shipped_registry(),
    )
    .unwrap_or_else(|e| panic!("the shipped lead template parses: {e:?}"));
    let node = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "lead")
        .expect("the lead template has a lead cell");
    if let Err(why) = check_override_params(&node, Some("lead"), "lead", &over) {
        panic!("a set's {over} on lead/lead must pass the door: {why:?}");
    }
}

/// The stamp of a `simple` turn that needs a tool: a fast target with a
/// required tool round (the condition `must_call`).
fn tool_turn() -> Value {
    ack_stamp("simple", &["tool"], &["web"])
}

/// The stamp of a turn whose call leaves the composition (the condition
/// `external`).
fn errand_turn() -> Value {
    ack_stamp("complex", &[], &["web"])
}

/// The head of a turn of a running session.
fn live_head() -> Value {
    ack_head(json!({"live": ACK_LIVE}))
}

/// The head of a delegation round: an event round, its kind named.
fn event_head(extra: Value) -> Value {
    let mut h = ack_head(json!({"turn": ACK_EVENT_TURN, "event": "delegation"}));
    for (k, v) in obj(extra) {
        h[k.as_str()] = v;
    }
    h
}

/// The lead's waiting-time advise on, four seconds for the family `web`.
fn horizon_on() -> Value {
    json!({"horizon_advise": "1", "horizon_defaults_ms": "{\"web\": 4000}"})
}

fn with(mut knobs: Value, extra: Value) -> Value {
    for (k, v) in obj(extra) {
        knobs[k.as_str()] = v;
    }
    knobs
}

fn heard(v: &Value) -> bool {
    v.get("listen") == Some(&json!(true))
}

#[test]
fn gh_d54g_ack_a_listed_condition_orders_its_side_call_and_an_unlisted_none() {
    if !shipped() {
        return;
    }
    let shipped = ack_shipped(SIDE_WHEN);
    assert_eq!(
        ack_names(&shipped),
        vec!["external".to_string(), "must_call".to_string()],
        "shipped, both conditions are listed: {shipped}"
    );
    passes_the_door(json!({ SIDE_WHEN: "external" }));

    let base_tool = ack_verdict(ack_knobs(json!({})), tool_turn(), ack_head(json!({})));
    let base_errand = ack_verdict(ack_knobs(json!({})), errand_turn(), ack_head(json!({})));
    assert_eq!(ack_sides(&base_tool, "must_call").len(), 1, "{base_tool}");
    assert_eq!(
        ack_sides(&base_errand, "external").len(),
        1,
        "{base_errand}"
    );

    for (listed, must_call, external) in [
        ("must_call,external", 1, 1),
        ("must_call", 1, 0),
        ("external", 0, 1),
    ] {
        let knobs = ack_knobs(json!({ SIDE_WHEN: listed }));
        let tool = ack_verdict(knobs.clone(), tool_turn(), ack_head(json!({})));
        let errand = ack_verdict(knobs, errand_turn(), ack_head(json!({})));
        assert_eq!(
            (
                ack_sides(&tool, "must_call").len(),
                ack_sides(&tool, "external").len()
            ),
            (must_call, 0),
            "side_when {listed:?}: the tool turn's side calls: {tool}"
        );
        assert_eq!(
            (
                ack_sides(&errand, "must_call").len(),
                ack_sides(&errand, "external").len()
            ),
            (0, external),
            "side_when {listed:?}: the errand's side calls: {errand}"
        );
        assert_eq!(
            ack_without(&tool, "side"),
            ack_without(&base_tool, "side"),
            "side_when {listed:?}: everything else of the tool turn's verdict stays"
        );
        assert_eq!(
            ack_without(&errand, "side"),
            ack_without(&base_errand, "side"),
            "side_when {listed:?}: everything else of the errand's verdict stays"
        );
    }
}

#[test]
fn gh_d54g_ack_a_live_turn_is_only_heard_when_told_so() {
    if !shipped() {
        return;
    }
    let shipped = ack_shipped(LIVE_TURN);
    assert_eq!(shipped, json!("call"), "shipped, a live turn is called");
    passes_the_door(json!({ LIVE_TURN: "listen" }));

    for stamp in [ack_stamp("talk", &[], &[]), tool_turn(), errand_turn()] {
        // Shipped and `call`: the verdict of before.
        let base = ack_verdict(ack_knobs(json!({})), stamp.clone(), live_head());
        let called = ack_verdict(
            ack_knobs(json!({ LIVE_TURN: "call" })),
            stamp.clone(),
            live_head(),
        );
        assert_eq!(called, base, "`call` is the verdict of before: {stamp}");
        assert!(!heard(&base), "a called turn is not heard: {base}");

        // `listen`: no target, no side call, no feed -- also with the
        // waiting-time advise on, which a called tool turn would get.
        let listen = with(ack_knobs(json!({ LIVE_TURN: "listen" })), horizon_on());
        let v = ack_verdict(listen.clone(), stamp.clone(), live_head());
        assert!(heard(&v), "{stamp}: the live turn is only heard: {v}");
        assert_eq!(v["target"], json!(""), "{stamp}: no target: {v}");
        assert_eq!(v["side"], json!([]), "{stamp}: no side call: {v}");
        assert_eq!(v["feed"], json!([]), "{stamp}: no feed: {v}");
        assert_eq!(v["reason"], json!("listen"), "{stamp}: {v}");

        // A turn without a handle is no running session's: called as before.
        let text = ack_verdict(listen.clone(), stamp.clone(), ack_head(json!({})));
        let text_base = ack_verdict(
            with(ack_knobs(json!({})), horizon_on()),
            stamp.clone(),
            ack_head(json!({})),
        );
        assert_eq!(
            text, text_base,
            "{stamp}: a turn without a handle is called"
        );

        // Review 1, M-1: a word that is neither `call` nor `listen` tells
        // the lead nothing -- the turn is called as shipped, and the word
        // is said on stderr. The two known words say nothing.
        let (odd, said) = cell_scope(
            "lead",
            ack_knobs(json!({ LIVE_TURN: "listening" })),
            "lead_verdict(ARGS[0], ARGS[1], ARGS[2])",
            json!([stamp, live_head(),
                   {"escalated": "", "consult_pending": {}, "spec": {}, "spec_held": {}}]),
        );
        assert_eq!(odd, base, "{stamp}: an unknown word reads as `call`");
        assert!(
            said.contains("live_turn") && said.contains("listening"),
            "{stamp}: the unknown word is said on stderr: {said:?}"
        );
        for known in ["call", "listen", " Listen "] {
            let (_, said) = cell_scope(
                "lead",
                ack_knobs(json!({ LIVE_TURN: known })),
                "lead_verdict(ARGS[0], ARGS[1], ARGS[2])",
                json!([stamp, live_head(),
                       {"escalated": "", "consult_pending": {}, "spec": {}, "spec_held": {}}]),
            );
            assert!(
                !said.contains("live_turn"),
                "{stamp}: live_turn {known:?} is not reported: {said}"
            );
        }
    }
}

#[test]
fn gh_d54g_ack_a_speculative_ask_of_a_heard_turn_parks() {
    if !shipped() {
        return;
    }
    let spec_head = ack_head(json!({"live": ACK_LIVE, "spec": "1"}));
    let stamp = ack_stamp("simple", &[], &[]);

    // The control: with speculation on, a called turn's speculative ask is
    // a call on the speculation's target.
    let called = ack_verdict(
        ack_knobs(json!({"quick_speculative": "1"})),
        stamp.clone(),
        spec_head.clone(),
    );
    assert_eq!(
        (called["spec"].clone(), called["target"].clone()),
        (json!("call"), json!("quick_spec")),
        "the control speculates: {called}"
    );

    let listen = ack_knobs(json!({"quick_speculative": "1", LIVE_TURN: "listen"}));
    let v = ack_verdict(listen.clone(), stamp.clone(), spec_head);
    assert_eq!(
        v["spec"],
        json!("parked"),
        "the speculative ask of a heard turn parks: {v}"
    );
    assert_ne!(
        v["target"],
        json!("quick_spec"),
        "no speculation call leaves for a heard turn: {v}"
    );

    // A speculative ask of a turn without a handle speculates as before.
    let text = ack_verdict(listen, stamp, ack_head(json!({"spec": "1"})));
    assert_eq!(
        (text["spec"].clone(), text["target"].clone()),
        (json!("call"), json!("quick_spec")),
        "a turn without a handle speculates: {text}"
    );
}

#[test]
fn gh_d54g_ack_a_turn_that_answers_a_consult_is_never_only_heard() {
    if !shipped() {
        return;
    }
    let listen = ack_knobs(json!({ LIVE_TURN: "listen" }));
    let stamp = ack_stamp("talk", &[], &[]);

    // The turn path of the running session is heard ...
    let turn = ack_verdict(listen.clone(), stamp.clone(), live_head());
    assert!(heard(&turn), "the turn path is heard: {turn}");

    // ... the round that answers a consult of the same session never: the
    // advice would stay without an answer.
    for class in ["", "ask"] {
        let head = ack_head(json!({"live": ACK_LIVE, "consult_id": "k-c-t0",
                                   "consult_class": class}));
        let v = ack_verdict(listen.clone(), stamp.clone(), head.clone());
        assert!(!heard(&v), "consult_class {class:?}: never only heard: {v}");
        assert_eq!(
            (v["target"].clone(), v["reason"].clone()),
            (json!("brain"), json!("answer")),
            "consult_class {class:?}: the answer round has its target: {v}"
        );
        let base = ack_verdict(ack_knobs(json!({})), stamp.clone(), head);
        assert_eq!(v, base, "consult_class {class:?}: the verdict of before");
    }
}

#[test]
fn gh_d54g_ack_an_event_round_goes_to_the_target_its_route_names() {
    if !shipped() {
        return;
    }
    let shipped = ack_shipped(EVENT_ROUTES);
    assert_eq!(shipped, json!(""), "shipped, no event has a route");
    passes_the_door(json!({ EVENT_ROUTES: "delegation:quick" }));

    let routed = ack_knobs(json!({ EVENT_ROUTES: "delegation:quick" }));
    // Whatever the round is stamped with -- nothing, a path of its own, a
    // path that would leave the composition: the route's target, no stamp.
    for stamp in [
        ack_stamp("", &[], &[]),
        ack_stamp("talk", &[], &[]),
        errand_turn(),
    ] {
        let v = ack_verdict(
            routed.clone(),
            stamp.clone(),
            event_head(json!({"live": ACK_LIVE})),
        );
        assert_eq!(
            v["target"],
            json!("quick"),
            "{stamp}: the route's target: {v}"
        );
        assert_eq!(v["reason"], json!("event"), "{stamp}: {v}");
        assert_eq!(
            v["departs"],
            json!({}),
            "{stamp}: the round does not leave: {v}"
        );
        assert_eq!(
            v["fallback"],
            json!("brain"),
            "{stamp}: the fallback of the target holds as for every target: {v}"
        );
    }

    // A name the lead has never heard is a target all the same: the lead
    // checks no names, the composition wires them.
    let v = ack_verdict(
        ack_knobs(json!({ EVENT_ROUTES: "delegation:north" })),
        ack_stamp("talk", &[], &[]),
        event_head(json!({})),
    );
    assert_eq!(
        (
            v["target"].clone(),
            v["reason"].clone(),
            v["fallback"].clone()
        ),
        (json!("north"), json!("event"), json!("")),
        "{v}"
    );

    // An event round is never only heard: the delegation would stay
    // without an answer.
    let v = ack_verdict(
        with(routed.clone(), json!({ LIVE_TURN: "listen" })),
        ack_stamp("talk", &[], &[]),
        event_head(json!({"live": ACK_LIVE})),
    );
    assert!(!heard(&v), "an event round is never only heard: {v}");
    assert_eq!(v["target"], json!("quick"), "{v}");

    // The escalation of the route's target goes to its fallback.
    let v = ack_verdict(
        routed.clone(),
        ack_stamp("", &[], &[]),
        event_head(json!({"escalate": "no_tool_call"})),
    );
    assert_eq!(
        (v["target"].clone(), v["from"].clone(), v["reason"].clone()),
        (
            json!("brain"),
            json!("quick"),
            json!("escalate:no_tool_call")
        ),
        "the escalation holds as for every target: {v}"
    );

    // The turn's own round of the same composition goes by its stamp.
    let v = ack_verdict(routed, ack_stamp("talk", &[], &[]), live_head());
    assert_eq!(
        (v["target"].clone(), v["reason"].clone()),
        (json!("brain"), json!("path")),
        "a turn's own round is no event: {v}"
    );
}

#[test]
fn gh_d54g_ack_an_event_round_gets_no_side_call_and_no_waiting_feed() {
    if !shipped() {
        return;
    }
    let knobs = with(
        ack_knobs(json!({ EVENT_ROUTES: "delegation:quick" })),
        horizon_on(),
    );

    // The control: the same stamp on a turn's own round of the running
    // session orders the side call and the waiting-time advise.
    let turn = ack_verdict(knobs.clone(), tool_turn(), live_head());
    assert_eq!(
        ack_sides(&turn, "must_call").len(),
        1,
        "the control: {turn}"
    );
    assert_ne!(turn["feed"], json!([]), "the control: {turn}");

    // The event round on the same fast target, the same needs: neither.
    let v = ack_verdict(
        knobs.clone(),
        tool_turn(),
        event_head(json!({"live": ACK_LIVE})),
    );
    assert_eq!(v["target"], json!("quick"), "{v}");
    assert_eq!(
        v["side"],
        json!([]),
        "an event round gets no side call: {v}"
    );
    assert_eq!(
        v["feed"],
        json!([]),
        "an event round gets no waiting feed: {v}"
    );

    // Nor when its stamp would leave the composition (`external`).
    let v = ack_verdict(knobs, errand_turn(), event_head(json!({"live": ACK_LIVE})));
    assert_eq!(v["side"], json!([]), "{v}");
    assert_eq!(v["feed"], json!([]), "{v}");
}

#[test]
fn gh_d54g_ack_an_event_round_without_a_route_goes_as_today() {
    if !shipped() {
        return;
    }
    let shipped = ack_shipped(EVENT_ROUTES);
    for routes in [shipped, json!("other:quick")] {
        let knobs = with(
            ack_knobs(json!({ EVENT_ROUTES: routes.clone() })),
            horizon_on(),
        );
        let before = with(ack_knobs(json!({})), horizon_on());
        for stamp in [
            ack_stamp("", &[], &[]),
            ack_stamp("talk", &[], &[]),
            tool_turn(),
            errand_turn(),
        ] {
            // The same round, once with its kind named and once as the
            // head of before (no `event`).
            let named = event_head(json!({"live": ACK_LIVE}));
            let mut unnamed = obj(named.clone());
            unnamed.remove("event");
            let v = ack_verdict(knobs.clone(), stamp.clone(), named);
            let was = ack_verdict(before.clone(), stamp.clone(), Value::Object(unnamed));
            assert_eq!(
                v, was,
                "event_routes {routes}: a delegation round without a route goes as before: {stamp}"
            );
            assert_ne!(v["reason"], json!("event"), "{stamp}: {v}");
        }
    }
}

#[test]
fn gh_d54g_ack_fresh_knobs_change_nothing() {
    if !shipped() {
        return;
    }
    // Each knob is declared three times with one default: the script's, the
    // param's, the setting's.
    let side_when = ack_shipped(SIDE_WHEN);
    let live_turn = ack_shipped(LIVE_TURN);
    let event_routes = ack_shipped(EVENT_ROUTES);
    assert_eq!(
        ack_names(&side_when),
        vec!["external".to_string(), "must_call".to_string()]
    );
    assert_eq!(
        (live_turn.clone(), event_routes.clone()),
        (json!("call"), json!(""))
    );
    passes_the_door(
        json!({ SIDE_WHEN: side_when, LIVE_TURN: live_turn, EVENT_ROUTES: event_routes }),
    );

    let fresh = with(ack_knobs(json!({})), horizon_on());
    // A knob the instance does not carry at all: the script's own default.
    let unset = with(
        fresh.clone(),
        json!({ SIDE_WHEN: null, LIVE_TURN: null, EVENT_ROUTES: null }),
    );
    let heads = [
        ack_head(json!({})),
        live_head(),
        ack_head(json!({"live": ACK_LIVE, "spec": "1"})),
        ack_head(json!({"live": ACK_LIVE, "consult_id": "k-c-t0"})),
        event_head(json!({"live": ACK_LIVE})),
        event_head(json!({})),
    ];
    let want_keys: Vec<String> = ACK_VERDICT_KEYS.iter().map(|k| k.to_string()).collect();
    for head in &heads {
        for stamp in [
            ack_stamp("", &[], &[]),
            ack_stamp("talk", &[], &[]),
            tool_turn(),
            errand_turn(),
        ] {
            let v = ack_verdict(fresh.clone(), stamp.clone(), head.clone());
            assert_eq!(
                ack_keys(&v),
                want_keys,
                "fresh knobs add no field to the verdict: {head} {stamp}"
            );
            assert_ne!(v["reason"], json!("listen"), "{head} {stamp}: {v}");
            assert_ne!(v["reason"], json!("event"), "{head} {stamp}: {v}");
            assert_eq!(
                ack_verdict(unset.clone(), stamp.clone(), head.clone()),
                v,
                "the script's default is the shipped value: {head} {stamp}"
            );
        }
    }

    // The two side calls of before, each under its condition.
    let tool = ack_verdict(fresh.clone(), tool_turn(), live_head());
    assert_eq!(
        (tool["target"].clone(), ack_sides(&tool, "must_call").len()),
        (json!("quick"), 1),
        "{tool}"
    );
    assert_ne!(tool["feed"], json!([]), "{tool}");
    let errand = ack_verdict(fresh, errand_turn(), live_head());
    assert_eq!(
        (
            errand["target"].clone(),
            ack_sides(&errand, "external").len()
        ),
        (json!(""), 1),
        "{errand}"
    );
}

#[test]
fn gh_d54g_ack_a_round_keyed_as_an_event_is_never_only_heard() {
    if !shipped() {
        return;
    }
    // Review 1, I-1 (OR-d54g.ack.13): the question names the kind of an
    // event round only for a delegation, but a round is keyed as an event
    // also when it follows an answered advice. Such a round carries no
    // `event` and no `consult_id`; only heard, it would stay without an
    // answer. The lead reads the form of the key, never the words.
    let listen = ack_knobs(json!({ LIVE_TURN: "listen" }));
    for stamp in [ack_stamp("talk", &[], &[]), tool_turn(), errand_turn()] {
        let keyed = ack_head(json!({"live": ACK_LIVE, "turn": ACK_EVENT_TURN}));
        assert!(
            keyed.get("event").is_none() && keyed["consult_id"] == json!(""),
            "check of the setup: the round names no event and answers no errand: {keyed}"
        );
        let v = ack_verdict(listen.clone(), stamp.clone(), keyed.clone());
        assert!(
            !heard(&v),
            "{stamp}: a round keyed as an event is never only heard: {v}"
        );
        assert_ne!(v["reason"], json!("listen"), "{stamp}: {v}");
        let base = ack_verdict(ack_knobs(json!({})), stamp.clone(), keyed);
        assert_eq!(
            v, base,
            "{stamp}: the round gets the verdict it gets without `listen`"
        );

        // The counter-probe: the same head under a turn's own key is heard,
        // and so is a key that only looks like an event's (no deadline in
        // its second to last part).
        for turn in ["t1", "voice~turn~1"] {
            let own = ack_head(json!({"live": ACK_LIVE, "turn": turn}));
            let v = ack_verdict(listen.clone(), stamp.clone(), own);
            assert!(
                heard(&v),
                "{stamp}: the turn {turn:?} of the running session is heard: {v}"
            );
        }
    }
    // The round has a target: the one of every round without a path of its
    // own route, as before.
    let v = ack_verdict(
        listen,
        ack_stamp("talk", &[], &[]),
        ack_head(json!({"live": ACK_LIVE, "turn": ACK_EVENT_TURN})),
    );
    assert_eq!(v["target"], json!("brain"), "the round is called: {v}");
}

/// `lead_verdict` under `knobs` for a session that has decided nothing yet,
/// with what the lead said on stderr while it read its knobs.
fn verdict_and_said(knobs: Value, stamp: Value, head: Value) -> (Value, String) {
    let (v, err) = cell_scope(
        "lead",
        knobs,
        "lead_verdict(ARGS[0], ARGS[1], ARGS[2])",
        json!([stamp, head,
               {"escalated": "", "consult_pending": {}, "spec": {}, "spec_held": {}}]),
    );
    assert!(v.is_object(), "a verdict: {v} {err}");
    (v, err)
}

#[test]
fn gh_d54g_ack_an_empty_or_unknown_side_condition_keeps_the_factory_side_calls() {
    if !shipped() {
        return;
    }
    // Review 1, M-1: `[]` read as "no condition" while `""` and null read
    // as both, and a misspelt name took both side calls away without a
    // word. Empty in every form is the shipped value. A name that is no
    // condition is said on stderr and makes the whole value the shipped one
    // (OR-d54g.ack.18): a typo in a set must never change the behaviour
    // without a word, and the shipped value is the known state.
    let base_tool = ack_verdict(ack_knobs(json!({})), tool_turn(), ack_head(json!({})));
    let base_errand = ack_verdict(ack_knobs(json!({})), errand_turn(), ack_head(json!({})));
    assert_eq!(ack_sides(&base_tool, "must_call").len(), 1, "{base_tool}");
    assert_eq!(
        ack_sides(&base_errand, "external").len(),
        1,
        "{base_errand}"
    );
    let both = |side_when: &Value| {
        let knobs = ack_knobs(json!({ SIDE_WHEN: side_when }));
        let (tool, said_tool) = verdict_and_said(knobs.clone(), tool_turn(), ack_head(json!({})));
        let (errand, _) = verdict_and_said(knobs, errand_turn(), ack_head(json!({})));
        (tool, errand, said_tool)
    };

    // Empty, in every form a set can write it: both, and nothing is said.
    for empty in [
        json!(null),
        json!(""),
        json!("  "),
        json!(","),
        json!([]),
        json!([""]),
    ] {
        let (tool, errand, said) = both(&empty);
        assert_eq!(
            (&tool, &errand),
            (&base_tool, &base_errand),
            "side_when {empty}: empty is the shipped value, both side calls"
        );
        assert!(
            !said.contains("side_when"),
            "side_when {empty}: nothing to say about an empty knob: {said}"
        );
    }

    // A known list says nothing either.
    for known in ["must_call,external", "must_call", "external"] {
        let (_, _, said) = both(&json!(known));
        assert!(
            !said.contains("side_when"),
            "side_when {known:?}: a known condition is not reported: {said}"
        );
    }

    // A name that is no condition, alone: said, and the side calls of the
    // shipped value stay.
    for (unknown, name) in [
        (json!("mustcall"), "mustcall"),
        (json!(["extern"]), "extern"),
    ] {
        let (tool, errand, said) = both(&unknown);
        assert_eq!(
            (&tool, &errand),
            (&base_tool, &base_errand),
            "side_when {unknown}: an unknown name alone keeps the shipped side calls"
        );
        assert!(
            said.contains("side_when") && said.contains(name),
            "side_when {unknown}: the unknown name is said on stderr: {said:?}"
        );
    }

    // Beside a known condition the unknown name does not fall alone: the
    // whole value is the shipped one, both side calls, and the name is said
    // (OR-d54g.ack.18). `must_call,extern` would otherwise order only the
    // side call of `must_call`.
    for (mixed, name) in [
        (json!("mustcall,external"), "mustcall"),
        (json!(["must_call", "extern"]), "extern"),
    ] {
        let (tool, errand, said) = both(&mixed);
        assert_eq!(
            (&tool, &errand),
            (&base_tool, &base_errand),
            "side_when {mixed}: an unknown name beside a known one is the shipped value, both side calls"
        );
        assert!(
            said.contains("side_when") && said.contains(name),
            "side_when {mixed}: the unknown name is said on stderr: {said:?}"
        );
    }
}
