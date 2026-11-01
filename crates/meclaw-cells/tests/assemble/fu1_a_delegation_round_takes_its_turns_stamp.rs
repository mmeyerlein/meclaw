//! GH #1275: a delegation round and its turn.
//!
//! (a) A round `in_delegation` opened is keyed by this cell and no route cell
//! stamps it, so without more it leaves with path "" and the curator sends it
//! the brain's way. It belongs to the turn that was open when the model
//! delegated (`hop.turn_id`, the label of its key): while `route_wait_ms` is
//! above 0 the turn-open reads that turn's route stamp in the same bundle and
//! parks it as the round's own leg-route beside the window leg, so the
//! curator's `path_routes` apply (simple -> quick, complex -> consult). No
//! stamp found (the turn's round already ended, or its stamp is still out):
//! the round leaves as before.
//!
//! GH #1279: the inheritance stands behind its own knob,
//! `delegation_inherits_stamp`. "0" (shipped) is 0.76.6: a delegation round
//! leaves with path "" whatever `route_wait_ms` says; a set switches it on.
//!
//! (b) `delegation_answer_mode`: what the advise mode tells the brain about a
//! delegation's answer. "fact" (shipped) is today's text byte for byte;
//! "context" asks for the result as a `context` section under the
//! delegation's id -- the live model words it -- and a `fact` only for a short
//! status sentence while a delegation runs long.
//! GH #1279: `delegation_rule` replaces the built-in rule of
//! "context" with a set's own text, word for word.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::*;
use serde_json::{Value, json};

const OPEN_TURN: &str = "s1#3";
const FRESH: &str = "2999-01-01T00:00:00.000000Z";

fn knob() -> Vec<(&'static str, Value)> {
    vec![
        ("memory_tier", json!("1")),
        ("route_wait_ms", json!(150)),
        ("delegation_inherits_stamp", json!("1")),
    ]
}

fn delegation() -> Value {
    lane(
        "in_delegation",
        json!({"turn_id": OPEN_TURN}),
        json!({"delegation_id": "dlg-1", "engine": "duplex"}),
        json!([{"origin": "user", "type": "text", "text": "when is my appointment?"}]),
    )
}

fn stamp(path: &str) -> Value {
    json!({"path": path, "families": ["calendar"], "needs": ["tool"]})
}

fn leg(key: &str, role: &str, payload: Value) -> Value {
    json!({"turn_id": key, "iter": 0, "role": role, "turn": payload.to_string(),
           "fired": 0, "recorded_at": FRESH})
}

fn delegation_window(key: &str) -> Value {
    leg(
        key,
        "leg-window",
        json!({"turns": [{"role": "delegation", "text": "when is my appointment?",
                          "consult_id": "dlg-1"}],
               "deferred": 0, "deferred_turns": []}),
    )
}

fn seam(out: &[Value]) -> Option<&Value> {
    out.iter().find(|m| {
        let r = m["header"]["route"].as_str();
        r == Some("curate") || r == Some("answer")
    })
}

/// The turn-open reply of a delegation round: its own row read back, and the
/// open turn's stamp as the turn-open read found it (`found`).
fn opened(key: &str, found: Value) -> Value {
    bundle_reply(
        "turn-open",
        key,
        &[
            ("c-open-round", json!([])),
            (
                "c-open-win",
                json!([{"id": "r1", "session_id": SESSION, "turn_id": key,
                        "role": "delegation", "content": "when is my appointment?",
                        "recorded_at": FRESH, "deferred": 0, "consult_id": "dlg-1"}]),
            ),
            ("c-open-scope", json!([])),
            ("c-open-roster", json!([])),
            ("c-open-route", found),
        ],
    )
}

#[test]
fn delegation_round_inherits_stamp_of_open_turn() {
    // The turn-open asks for the open turn's stamp in its own bundle.
    let out = assemble(&knob(), delegation());
    let open = in_phase(&out, "turn-open");
    let key = hop_str(open, "turn_id");
    assert!(
        key.starts_with("s1#3~"),
        "the round is keyed by its turn: {key}"
    );
    let ask = call(open, "c-open-route");
    assert_eq!(ask["operation"], "select", "{ask}");
    assert_eq!(ask["table"], "round", "{ask}");
    assert_eq!(
        ask["where"],
        json!({"turn_id": OPEN_TURN, "role": "leg-route"}),
        "{ask}"
    );

    // The reply parks the stamp as the round's own leg, in the window leg's
    // bundle, in front of the read-back.
    let out = assemble(
        &knob(),
        opened(&key, json!([{"turn": stamp("simple").to_string()}])),
    );
    let collect = in_phase(&out, "collect");
    let ids: Vec<String> = calls_of(collect).into_iter().map(|(id, _)| id).collect();
    let row = call(collect, "c-collect-route")["row"].clone();
    assert_eq!(row["role"], "leg-route", "{row}");
    assert_eq!(row["turn_id"], key, "{row}");
    let parked: Value = serde_json::from_str(row["turn"].as_str().expect("turn")).expect("json");
    assert_eq!(parked, stamp("simple"), "the stamp as the turn has it");
    assert_eq!(
        ids.last().map(String::as_str),
        Some("c-collect-read"),
        "the read sees the stamp: {ids:?}"
    );

    // The fan-in of the round carries the turn's path, needs and families; a
    // simple turn does not wait for the memory, a complex one does.
    let rows = json!([
        delegation_window(&key),
        leg(&key, "leg-route", stamp("simple"))
    ]);
    let out = assemble(
        &knob(),
        bundle_reply("collect", &key, &[("c-collect-read", rows)]),
    );
    let s = seam(&out).unwrap_or_else(|| panic!("simple fires on window + stamp: {out:?}"));
    assert_eq!(hop_str(s, "path"), "simple");
    assert_eq!(hop_str(s, "needs"), "tool");
    assert_eq!(hop_str(s, "families"), "calendar");
    let rows = json!([
        delegation_window(&key),
        leg(&key, "leg-route", stamp("complex"))
    ]);
    let out = assemble(
        &knob(),
        bundle_reply("collect", &key, &[("c-collect-read", rows)]),
    );
    assert!(
        seam(&out).is_none(),
        "complex waits for the memory: {out:?}"
    );

    // No stamp found: the window leg parks alone, the round leaves as before.
    let out = assemble(&knob(), opened(&key, json!([])));
    let collect = in_phase(&out, "collect");
    assert!(
        calls_of(collect)
            .iter()
            .all(|(id, _)| id != "c-collect-route"),
        "nothing to inherit: {collect}"
    );

    // Off (route_wait_ms 0): no stamp asked for, the turn-open of before.
    let out = assemble(&[], delegation());
    let open = in_phase(&out, "turn-open");
    assert!(
        calls_of(open).iter().all(|(id, _)| id != "c-open-route"),
        "no route wait, no stamp read: {open}"
    );
}

fn duplex_curate(over: &[(&str, Value)]) -> (Value, String) {
    let key = "s1#3~99999999999999~abcd1234";
    let mut doc = bundle_reply(
        "collect",
        key,
        &[(
            "c-collect-read",
            json!([
                delegation_window(key),
                leg(key, "leg-memory", json!({"system": {}, "messages": []}))
            ]),
        )],
    );
    doc["header"]["context"]["engine"] = json!("duplex");
    let stdout = run_cell_stdout(ASSEMBLE, over, doc.clone()).0;
    let out = assemble(over, doc);
    let s = seam(&out).unwrap_or_else(|| panic!("the delegation round fires: {out:?}"));
    let mode = s["system"]["instructions"]["mode"]["text"]
        .as_str()
        .expect("mode text")
        .to_string();
    (json!(mode), stdout)
}

#[test]
fn delegation_answer_mode_param() {
    let base = [("memory_tier", json!("1"))];
    let (shipped, shipped_bytes) = duplex_curate(&base);
    let shipped = shipped.as_str().unwrap().to_string();
    assert!(
        shipped.contains("ALWAYS answer a delegation with a `fact`"),
        "the shipped mode is today's text: {shipped}"
    );
    let (_, fact_bytes) = duplex_curate(&[
        ("memory_tier", json!("1")),
        ("delegation_answer_mode", json!("fact")),
    ]);
    assert_eq!(fact_bytes, shipped_bytes, "\"fact\" is today's bytes");

    let (ctx, _) = duplex_curate(&[
        ("memory_tier", json!("1")),
        ("delegation_answer_mode", json!("context")),
    ]);
    let ctx = ctx.as_str().unwrap().to_string();
    assert!(
        !ctx.contains("ALWAYS answer a delegation with a `fact`"),
        "{ctx}"
    );
    assert!(
        ctx.contains("answer a delegation with a `context` section"),
        "the result is a context section: {ctx}"
    );
    assert!(ctx.contains("delegation_id"), "under its id: {ctx}");
    assert!(
        ctx.contains("delegation_grace_ms"),
        "a `fact` only as a status sentence before the grace: {ctx}"
    );
    // Everything around the delegation rule stays as it is.
    assert!(ctx.starts_with("## mode: advise\n"), "{ctx}");
    assert!(ctx.ends_with("never a greeting as `fact`."), "{ctx}");

    // The knob ships at today's text and says so in all three places.
    let cfg = config_of(ASSEMBLE);
    assert_eq!(cfg["params"]["delegation_answer_mode"], json!("fact"));
    assert_eq!(
        cfg["contract"]["settings"]["delegation_answer_mode"]["default"],
        json!("fact")
    );
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    assert!(
        src.contains("_str(\"delegation_answer_mode\", \"fact\")"),
        "the script's fallback"
    );
}

/// GH #1279: `delegation_rule` is the `context` mode's rule as
/// text. Set, it stands in the built-in rule's place word for word and the
/// rest of the mode text stays as it was; "" is the built-in rule byte for
/// byte; in the mode "fact" it changes nothing.
#[test]
fn delegation_rule_replaces_the_context_rule() {
    let cfg = config_of(ASSEMBLE);
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    let head = "DELEGATION_RULE_CONTEXT = \"\"\"";
    let start = src.find(head).expect("the built-in context rule") + head.len();
    let built_in = &src[start..start + src[start..].find("\"\"\"").expect("its end")];

    let context = [
        ("memory_tier", json!("1")),
        ("delegation_answer_mode", json!("context")),
    ];
    let (ctx, ctx_bytes) = duplex_curate(&context);
    let ctx = ctx.as_str().unwrap().to_string();
    assert!(ctx.contains(built_in), "the built-in rule stands: {ctx}");

    let rule =
        "Answer with facts, status and the next step; a dropped errand says `Status: dropped`.";
    let (set, _) = duplex_curate(&[
        ("memory_tier", json!("1")),
        ("delegation_answer_mode", json!("context")),
        ("delegation_rule", json!(rule)),
    ]);
    let set = set.as_str().unwrap().to_string();
    assert!(set.contains(rule), "the set's rule stands: {set}");
    assert!(
        !set.contains(built_in),
        "in the built-in rule's place: {set}"
    );
    assert_eq!(
        set,
        ctx.replace(built_in, rule),
        "everything around the rule stays as it is"
    );

    let (_, empty_bytes) = duplex_curate(&[
        ("memory_tier", json!("1")),
        ("delegation_answer_mode", json!("context")),
        ("delegation_rule", json!("")),
    ]);
    assert_eq!(empty_bytes, ctx_bytes, "\"\" is the built-in rule's bytes");

    let (_, fact_bytes) = duplex_curate(&[("memory_tier", json!("1"))]);
    let (_, fact_set_bytes) = duplex_curate(&[
        ("memory_tier", json!("1")),
        ("delegation_answer_mode", json!("fact")),
        ("delegation_rule", json!(rule)),
    ]);
    assert_eq!(
        fact_set_bytes, fact_bytes,
        "in the mode \"fact\" the rule is unread"
    );

    // The knob ships empty and says so in all three places.
    assert_eq!(cfg["params"]["delegation_rule"], json!(""));
    assert_eq!(
        cfg["contract"]["settings"]["delegation_rule"]["default"],
        json!("")
    );
    assert!(
        src.contains("_str(\"delegation_rule\", \"\")"),
        "the script's fallback"
    );
}

#[test]
fn delegation_round_inherits_stamp_only_with_its_knob() {
    // `route_wait_ms` above 0 and the knob shipped ("0"): no stamp is read, the
    // round leaves with path "" as in 0.76.6 (#1279).
    let waits = [("memory_tier", json!("1")), ("route_wait_ms", json!(150))];
    let out = assemble(&waits, delegation());
    let open = in_phase(&out, "turn-open");
    assert!(
        calls_of(open).iter().all(|(id, _)| id != "c-open-route"),
        "the knob is off, no stamp read: {open}"
    );
    let off = [
        ("memory_tier", json!("1")),
        ("route_wait_ms", json!(150)),
        ("delegation_inherits_stamp", json!("0")),
    ];
    let open_off = assemble(&off, delegation());
    let open_off = in_phase(&open_off, "turn-open");
    assert!(
        calls_of(open_off)
            .iter()
            .all(|(id, _)| id != "c-open-route"),
        "\"0\" is the shipped value: {open_off}"
    );
    // On: the open turn's stamp is read in the turn-open bundle.
    let out = assemble(&knob(), delegation());
    let open = in_phase(&out, "turn-open");
    assert!(
        calls_of(open).iter().any(|(id, _)| id == "c-open-route"),
        "the knob is on, the stamp is read: {open}"
    );

    // The knob ships off and says so in all three places.
    let cfg = config_of(ASSEMBLE);
    assert_eq!(cfg["params"]["delegation_inherits_stamp"], json!("0"));
    assert_eq!(
        cfg["contract"]["settings"]["delegation_inherits_stamp"]["default"],
        json!("0")
    );
    let src = cfg["params"]["script_inline"].as_str().expect("script");
    assert!(
        src.contains("_str(\"delegation_inherits_stamp\", \"0\")"),
        "the script's fallback"
    );
}

/// live-2 #1370: the set writes switches as JSON `true`; the collector reads
/// it as on (the presenter's `knob()` rule: true, "1", "true", "on").
#[test]
fn delegation_inherits_stamp_reads_json_true() {
    let on = [
        ("memory_tier", json!("1")),
        ("route_wait_ms", json!(150)),
        ("delegation_inherits_stamp", json!(true)),
    ];
    let out = assemble(&on, delegation());
    let open = in_phase(&out, "turn-open");
    assert!(
        calls_of(open).iter().any(|(id, _)| id == "c-open-route"),
        "true is on, the stamp is read: {open}"
    );
}
