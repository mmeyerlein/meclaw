//! D54g listennote (GH #1538) -- a heard turn of a running session still
//! leaves its note for the memory, and says nothing.
//!
//! WHY this file exists: with the lead's `live_turn` `listen` the turn of a
//! running session is only heard -- `target` "", no side call, no feed --,
//! because the session's own model has answered it (GH #1504). That also
//! took away the one model call that wrote the section `memory`: a turn
//! like "my knee has hurt since yesterday" left no wellbeing fact and no
//! followup (in the set before the live form both came, through the
//! splitter, the curator's `in_section`, `sidecar`, `assistant` and
//! `./followup`). The lead's knob `listen_sections` (shipped empty, so off)
//! names a target and a gate over the enum values of the route stamp's
//! choices -- values the route model judged, no word list. A heard turn
//! whose stamp meets the gate gets ONE silent call of kind `sections`: the
//! model writes only the sections, its answer reaches neither the
//! collector nor a channel nor the wall; its section `memory` takes the
//! road of every section. Off, under the gate, or with a fallback value:
//! the turn is heard as before, no call.
//!
//! Every knob is set as the override of an instance. The hive in one
//! process (`support/curator_hive.rs`), the composite's shipped edges, its
//! splitter's shipped script, the hive `assistant` with its cell
//! `./followup`; no model.

// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_live.rs"]
#[allow(unused_imports)]
use crate::curator_live; // harness_fold: mod curator_live;

use assistant_followup::{Assistant, NOON, curator, live_notes, out_of, session_ends};
use curator_hive::*;
use curator_live::{LIVE, Passed, live_keys, through};
use meclaw_colony::mutation::subtree::{check_override_params, parse_subtree};
use meclaw_colony::templates::{TemplateEntry, TemplatesRegistry, scan_templates_dir};
use meclaw_core::serde_json::{self as sj, Value, json};

/// The knob of the lead this strand adds.
const KNOB: &str = "listen_sections";
/// The kind of the silent call, in its head and in the context.
const KIND: &str = "sections";
/// The target the pins name.
const TARGET: &str = "quick_bridge";
/// The session of the running call, and its turn.
const SESSION: &str = "phone-session-1";
const TURN: &str = "phone-turn-1";
/// What the person said, and the fact a model takes from it.
const SAID: &str = "My knee has hurt since yesterday.";
const CLAIM: &str = "The person has had knee pain since yesterday.";

/// The knob's value: the target and the gate over the stamp's choices.
fn knob() -> Value {
    json!(
        json!({"target": TARGET,
               "when": {"depth": ["important"], "risk": ["strained", "acute"]}})
        .to_string()
    )
}

/// A choice of the route stamp, judged by the model (`source` "model") or
/// its declared fallback.
fn choice(value: &str, source: &str) -> Value {
    json!({"value": value, "p": if source == "model" { 0.9 } else { 0.0 }, "source": source})
}

/// The stamp's choices as the collector puts them on the seam: the JSON text
/// of the object.
fn choices(depth: Value, risk: Value) -> Value {
    json!(json!({"depth": depth, "risk": risk}).to_string())
}

fn met() -> Value {
    choices(choice("important", "model"), choice("strained", "model"))
}

/// The hive of a composite with a running session whose turns are only heard
/// (`live_turn` "listen"), the live feed on, the curator wired for notes and
/// the section `memory`, `over` on top.
fn hearing(over: &[(&str, &str, Value)]) -> Hive {
    let mut all: Vec<(&str, &str, Value)> = vec![
        ("policy", "live_feed", json!("1")),
        ("lead", "live_turn", json!("listen")),
    ];
    all.extend(over.iter().cloned());
    curator(&all).reporting_composite("talky")
}

/// The heard turn of the running session, its stamp's choices on the seam
/// (`None`: a stamp without them).
fn heard_turn(h: &mut Hive, choices: Option<Value>) {
    h.out.clear();
    let mut hop = json!({"session_id": SESSION, "turn_id": TURN, "iter": "0", "phase": "",
                         "path": "talk", "needs": "", "families": ""});
    let mut ctx = json!({"session_id": SESSION, "turn_id": TURN, "iter": "0",
                         "channel": "test", "audience_set": ROUND_E, "path": "talk"});
    for (k, v) in obj(live_keys(LIVE)) {
        hop[k.as_str()] = v.clone();
        ctx[k.as_str()] = v;
    }
    if let Some(c) = choices {
        hop["choices"] = c;
    }
    h.lane(
        "in_curate",
        ctx,
        hop,
        json!({"messages": [user(SAID), said("Oh no, I am sorry to hear that.")]}),
    );
}

/// Every model call or errand that left since `out` was cleared.
fn calls(h: &Hive) -> Vec<Msg> {
    h.out
        .iter()
        .filter(|m| m.route() == "model_call" || m.route() == "errand")
        .cloned()
        .collect()
}

/// The values of every mark of `kind`, oldest first.
fn marks(h: &Hive, kind: &str) -> Vec<Value> {
    h.rows(&format!(
        "SELECT value FROM marks WHERE kind = '{kind}' ORDER BY seq"
    ))
    .into_iter()
    .map(|r| sj::from_str(r[0].as_str().unwrap_or("null")).unwrap_or(Value::Null))
    .collect()
}

/// The bodies of the wall's blocks of the turn.
fn wall_of_turn(h: &Hive) -> Vec<String> {
    h.rows(&format!(
        "SELECT b.body FROM wall w JOIN blocks b ON b.hash = w.hash \
         WHERE w.turn_id = '{TURN}' ORDER BY w.seq"
    ))
    .iter()
    .map(|r| r[0].as_str().unwrap_or("").to_string())
    .collect()
}

/// What the model of the silent call writes: the block, nothing else.
fn sections_answer() -> String {
    let memory = json!({"memory": {
        "nothing_new": false,
        "facts": [{"subject": "the person", "predicate": "has_symptom", "claim": CLAIM,
                   "quote": SAID, "source": "person", "stance": "asserted",
                   "fact_kind": "experience", "valid_from": "2026-11-08",
                   "confidence": 0.9, "wellbeing": true}],
        "topic": {"movement": "continue"}}});
    format!("```sidecar\n{memory}\n```")
}

/// The composite's splitter, its shipped script and params, on the answer of
/// `call` (the context as the composite's edge `./curator -> ./<target>`
/// leaves it, the model cell's hop): its emissions.
fn split(call: &Msg, text: &str) -> Vec<Value> {
    split_as(call, text, "stop", &[])
}

/// [`split`] with the completion's `finish` reason and the params `over` set
/// on top of the shipped ones (an instance's override).
fn split_as(call: &Msg, text: &str, finish: &str, over: &[(&str, Value)]) -> Vec<Value> {
    let config = read_json(&repo("templates/talky/splitter/config.json"));
    let mut params = obj(config["params"].clone());
    for (k, v) in over {
        params.insert((*k).to_string(), v.clone());
    }
    let script = params
        .remove("script_inline")
        .and_then(|s| s.as_str().map(str::to_string))
        .expect("the splitter's script");
    let lifted = lifted_context(call);
    let doc = json!({
        "envelope": {"header": {"context": lifted,
                                "hop": {"finish_reason": finish, "model": "test-model"}},
                     "target": "/x/talky/splitter", "reply_to": ""},
        "body": {"messages": [said(text)]},
        "params": params,
    })
    .to_string();
    let out = run_python(&script, &doc);
    assert!(
        out.status.success(),
        "the splitter: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    match sj::from_slice::<Value>(&out.stdout).expect("the splitter writes JSON") {
        Value::Array(a) => a,
        one => vec![one],
    }
}

/// The context of `call`'s answer: the composite's edge to the call's target
/// lifts the call's head into it (`curator_call`, `turn_id`, `session_id`,
/// `iter`, `call_kind`, ...).
fn lifted_context(call: &Msg) -> Value {
    let target = call.hop.get("target").and_then(Value::as_str).unwrap_or("");
    let passed = through(
        "talky",
        "./curator",
        Value::Object(call.context.clone()),
        Value::Object(call.hop.clone()),
    );
    let to = format!("./{target}");
    let p = passed
        .into_iter()
        .find(|p| p.to == to)
        .unwrap_or_else(|| panic!("the call reaches {to} through talky's edges"));
    Value::Object(p.context)
}

/// The context of `call`'s answer had the call gone to the model cell
/// `target` instead.
fn lifted_to(call: &Msg, target: &str) -> Value {
    let mut c = call.clone();
    c.hop.insert("target".to_string(), json!(target));
    lifted_context(&c)
}

/// A hearing hive and the silent call its heard turn over the gate sent.
fn silent_call() -> (Hive, Msg) {
    let mut h = hearing(&[("lead", KNOB, knob())]);
    heard_turn(&mut h, Some(met()));
    let sent = calls(&h);
    assert_eq!(sent.len(), 1, "one silent call: {:?} {:?}", h.out, h.stderr);
    let call = sent[0].clone();
    assert_eq!(call.hop["call_kind"], json!(KIND), "{call:?}");
    (h, call)
}

/// `ctx` without the call's kind: the same answer as an ordinary turn's.
fn ordinary(ctx: Value) -> Value {
    let mut c = obj(ctx);
    c.remove("call_kind");
    Value::Object(c)
}

/// Every emission of the splitter on `answer`, run through the composite's
/// edges out of `./splitter` with the context `ctx`.
fn out_of_splitter(emitted: &[Value], ctx: &Value) -> Vec<(Passed, Value)> {
    let mut passed = Vec::new();
    for m in emitted {
        for p in through("talky", "./splitter", ctx.clone(), m["header"].clone()) {
            passed.push((p, m.clone()));
        }
    }
    passed
}

/// The section `memory` (one pass to `./curator`) through the curator's
/// `in_section` of `h`, out of talky and into `assistant`: the followup's note
/// reaches the surface that did not hear the turn at once.
fn memory_reaches_the_followup(h: &mut Hive, sections: &[(Passed, Value)]) {
    let memory: Vec<_> = sections
        .iter()
        .filter(|(p, _)| p.to == "./curator" && p.hop.get("section") == Some(&json!("memory")))
        .collect();
    assert_eq!(
        memory.len(),
        1,
        "the section `memory` goes to the curator once: {sections:?}"
    );
    let (p, m) = memory[0];
    let mut body = m.clone();
    body.as_object_mut().expect("a body").remove("header");
    h.out.clear();
    h.lane(
        "in_section",
        Value::Object(p.context.clone()),
        Value::Object(p.hop.clone()),
        body,
    );
    let fwd = h.routed("sidecar");
    assert_eq!(
        fwd.len(),
        1,
        "the curator forwards the section once: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        Value::Object(fwd[0].body.clone())
            .to_string()
            .contains(CLAIM),
        "with the fact: {:?}",
        fwd[0]
    );
    let at_rim = out_of("./talky", &fwd[0]);
    assert_eq!(at_rim.len(), 1, "talky hands the section out once");
    let mut a = Assistant::on_at(NOON);
    a.leave("./talky", at_rim[0].clone());
    assert_eq!(
        live_notes(&a.notes_into("./talky-chat")).len(),
        1,
        "the followup's note: {:?} {:?}",
        a.into,
        a.stderr
    );
}

fn shipped_registry() -> TemplatesRegistry {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scanned = scan_templates_dir(&root.join("templates")).unwrap_or_default();
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
fn gh_d54g_listennote_a_heard_turn_with_the_knob_writes_its_note_and_says_nothing() {
    if !shipped() {
        return;
    }
    let mut h = hearing(&[("lead", KNOB, knob())]);
    heard_turn(&mut h, Some(met()));

    // ONE silent call to the knob's target: kind `sections`, no tool.
    let sent = calls(&h);
    assert_eq!(
        sent.len(),
        1,
        "a heard turn over the gate sends one call: {:?} {:?}",
        h.out,
        h.stderr
    );
    let call = sent[0].clone();
    assert_eq!(call.route(), "model_call", "{call:?}");
    assert_eq!(call.hop["target"], json!(TARGET), "{call:?}");
    assert_eq!(call.hop["call_kind"], json!(KIND), "{call:?}");
    assert_eq!(
        call.body.get("tool_scope"),
        Some(&json!({"allow": []})),
        "the silent call has no tool: {call:?}"
    );
    let system = call.body.get("system").cloned().unwrap_or(Value::Null);
    let silent = &system["instructions"]["sidecar"]["silent"];
    assert!(
        silent["text"]
            .as_str()
            .is_some_and(|t| !t.trim().is_empty()),
        "the core's instruction stands in the section contract: {system}"
    );
    assert!(
        marks(&h, "turn_parked").is_empty(),
        "a turn with its silent call is not parked: {:?}",
        marks(&h, "turn_parked")
    );

    // The model's answer: nothing of it enters the wall.
    let answer = sections_answer();
    h.out.clear();
    h.tap(&call, "stop", json!({}), json!([said(&answer)]));
    let wall = wall_of_turn(&h);
    assert!(
        !wall
            .iter()
            .any(|b| b.contains(CLAIM) || b.contains("sidecar")),
        "the silent answer stays out of the wall: {wall:?}"
    );
    assert!(
        wall.iter().any(|b| b.contains(SAID)),
        "the person's words stay in the history: {wall:?}"
    );
    // ... and no partial of it leaves the composite toward a channel, on
    // whichever model cell the knob names: `brain` and `quick` carry a
    // partial edge to the rim (39, 40), the control proves it is there.
    for target in ["brain", "quick", TARGET] {
        let ctx = lifted_to(&call, target);
        let partial = json!({"route": "partial", "text": "x"});
        for p in through(
            "talky",
            &format!("./{target}"),
            ctx.clone(),
            partial.clone(),
        ) {
            assert_ne!(
                p.to, ".",
                "a partial of the silent call on {target} leaves: edge {}",
                p.edge
            );
        }
        if target != TARGET {
            assert!(
                through("talky", &format!("./{target}"), ordinary(ctx), partial)
                    .iter()
                    .any(|p| p.to == "."),
                "an ordinary partial of {target} reaches the rim"
            );
        }
    }

    // The splitter takes the block apart; the answer reaches neither the
    // dispatcher nor the collector nor the rim, the section goes to the
    // curator.
    let mut sections = Vec::new();
    for m in split(&call, &answer) {
        let ctx = lifted_context(&call);
        let hop = m["header"].clone();
        for p in through("talky", "./splitter", ctx, hop) {
            assert!(
                p.to == "./curator",
                "the silent answer reaches {} (edge {}): {m}",
                p.to,
                p.edge
            );
            sections.push((p, m.clone()));
        }
    }
    let memory: Vec<_> = sections
        .iter()
        .filter(|(p, _)| p.hop.get("section") == Some(&json!("memory")))
        .collect();
    assert_eq!(
        memory.len(),
        1,
        "the section `memory` goes to the curator once: {sections:?}"
    );
    let (p, m) = memory[0];
    let mut body = m.clone();
    body.as_object_mut().expect("a body").remove("header");
    h.out.clear();
    h.lane(
        "in_section",
        Value::Object(p.context.clone()),
        Value::Object(p.hop.clone()),
        body,
    );
    let fwd = h.routed("sidecar");
    assert_eq!(
        fwd.len(),
        1,
        "the curator forwards the section once: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(
        Value::Object(fwd[0].body.clone())
            .to_string()
            .contains(CLAIM),
        "with the fact: {:?}",
        fwd[0]
    );

    // `assistant` hears it: the note into a surface that did not hear it at
    // once, held for the hearing surface until its session ends.
    let at_rim = out_of("./talky", &fwd[0]);
    assert_eq!(at_rim.len(), 1, "talky hands the section out once");
    let mut a = Assistant::on_at(NOON);
    a.leave("./talky", at_rim[0].clone());
    assert!(
        a.notes_into("./talky").is_empty(),
        "held for the hearing surface while its session runs: {:?}",
        a.into
    );
    assert_eq!(
        live_notes(&a.notes_into("./talky-chat")).len(),
        1,
        "the followup's note: {:?} {:?}",
        a.into,
        a.stderr
    );
    session_ends(&mut a, "./talky", SESSION);
    assert_eq!(
        live_notes(&a.notes_into("./talky")).len(),
        1,
        "the held note reaches the hearing surface at its session's end: {:?}",
        a.into
    );
}

#[test]
fn gh_d54g_listennote_without_the_knob_a_heard_turn_calls_nothing() {
    if !shipped() {
        return;
    }
    for over in [
        vec![],
        vec![("lead", KNOB, json!(""))],
        vec![("lead", KNOB, json!("0"))],
    ] {
        let mut h = hearing(&over);
        heard_turn(&mut h, Some(met()));
        assert!(
            calls(&h).is_empty(),
            "{over:?}: no call: {:?} {:?}",
            calls(&h),
            h.stderr
        );
        assert_eq!(
            marks(&h, "turn_parked"),
            vec![json!({"reason": "listen", "iter": 0})],
            "{over:?}: the turn is heard as before"
        );
    }
}

#[test]
fn gh_d54g_listennote_under_the_gate_a_heard_turn_calls_nothing() {
    if !shipped() {
        return;
    }
    let below = [
        // A value outside the gate, on either key.
        Some(choices(
            choice("light", "model"),
            choice("strained", "model"),
        )),
        Some(choices(
            choice("important", "model"),
            choice("none", "model"),
        )),
        // The value inside, but no judgement of the model: its fallback.
        Some(choices(
            choice("important", "fallback"),
            choice("strained", "model"),
        )),
        // A key missing, and no choices at all.
        Some(json!(
            json!({"depth": choice("important", "model")}).to_string()
        )),
        None,
    ];
    for c in below {
        let mut h = hearing(&[("lead", KNOB, knob())]);
        heard_turn(&mut h, c.clone());
        assert!(
            calls(&h).is_empty(),
            "{c:?}: no call under the gate: {:?} {:?}",
            calls(&h),
            h.stderr
        );
        assert_eq!(
            marks(&h, "turn_parked"),
            vec![json!({"reason": "listen", "iter": 0})],
            "{c:?}: the turn is heard as before"
        );
    }
    // A turn that is not heard (no running session) is called as before,
    // whatever its choices: the knob only reaches a heard turn.
    let mut h = hearing(&[("lead", KNOB, knob())]);
    h.out.clear();
    h.lane(
        "in_curate",
        json!({"session_id": "text-1", "turn_id": "text-turn-1", "iter": "0",
               "channel": "test", "audience_set": ROUND_E, "path": "talk"}),
        json!({"session_id": "text-1", "turn_id": "text-turn-1", "iter": "0", "phase": "",
               "path": "talk", "needs": "", "families": "", "choices": met()}),
        json!({"messages": [user(SAID)]}),
    );
    let sent = calls(&h);
    assert_eq!(sent.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert!(
        sent[0].hop.get("call_kind").is_none(),
        "a turn of its own is no silent call: {:?}",
        sent[0]
    );
}

#[test]
fn gh_d54g_listennote_a_set_override_switches_the_knob_on() {
    if !shipped() {
        return;
    }
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    // Shipped off: the template's own value is empty.
    let shipped_value = read_json(&repo("templates/lead/lead/config.json"))["params"][KNOB].clone();
    assert_eq!(shipped_value, json!(""), "the knob ships off");
    let parsed = parse_subtree(&root.join("templates").join("lead"), &shipped_registry())
        .unwrap_or_else(|e| panic!("the shipped lead template parses: {e:?}"));
    let cell = parsed
        .cells
        .into_iter()
        .find(|c| c.rel_path == "lead")
        .expect("the lead hive has its cell lead");
    let over = json!({KNOB: knob()});
    if let Err(why) = check_override_params(&cell, Some("lead"), "lead", &over) {
        panic!("a set's {over} on lead/lead must pass the door: {why:?}");
    }
    // ... and the override, on an instance, switches the silent call on.
    let mut h = hearing(&[("lead", KNOB, knob())]);
    heard_turn(&mut h, Some(met()));
    assert_eq!(calls(&h).len(), 1, "{:?}", h.stderr);
}

/// D54g listennote fix round R1 (I1, M1): the silent call hands out only its
/// section `memory`. An answer the model cut (`length`) reaches neither the
/// collector nor the dispatcher nor the rim, its sections only the curator.
#[test]
fn gh_d54g_listennote_a_cut_silent_answer_reaches_no_collector() {
    if !shipped() {
        return;
    }
    let (_h, call) = silent_call();
    let ctx = lifted_context(&call);
    let emitted = split_as(&call, &sections_answer(), "length", &[]);
    for (p, m) in out_of_splitter(&emitted, &ctx) {
        assert_eq!(
            p.to, "./curator",
            "the cut silent answer reaches {} (edge {}): {m}",
            p.to, p.edge
        );
    }
    assert!(
        out_of_splitter(&emitted, &ordinary(ctx))
            .iter()
            .any(|(p, _)| p.to == "./collector"),
        "an ordinary cut answer reaches the collector"
    );
}

/// I1 (a): with the composite's `answer_check` set, the silent call's answer
/// (a sentence beside its block) sends no check out of talky -- neither
/// straight to the rim (without sources) nor through the curator (with
/// `answer_check_sources`), and the curator lets none out either.
#[test]
fn gh_d54g_listennote_a_silent_answer_sends_no_answer_check() {
    if !shipped() {
        return;
    }
    let (_h, call) = silent_call();
    let ctx = lifted_context(&call);
    let answer = format!("I have noted that.\n{}", sections_answer());
    for over in [
        vec![("answer_check", json!(["claims"]))],
        vec![
            ("answer_check", json!(["claims"])),
            ("answer_check_sources", json!("turn+recall")),
        ],
    ] {
        let emitted = split_as(&call, &answer, "stop", &over);
        assert!(
            emitted
                .iter()
                .any(|m| m["header"]["route"] == json!("answer_check")),
            "{over:?}: the splitter writes its check: {emitted:?}"
        );
        for (p, m) in out_of_splitter(&emitted, &ctx) {
            assert!(
                p.to != "." && p.hop.get("route") != Some(&json!("in_answer_check")),
                "{over:?}: the silent answer's check leaves to {} (edge {}): {m}",
                p.to,
                p.edge
            );
        }
        assert!(
            out_of_splitter(&emitted, &ordinary(ctx.clone()))
                .iter()
                .any(
                    |(p, _)| (p.to == "." && p.hop.get("route") == Some(&json!("answer_check")))
                        || p.hop.get("route") == Some(&json!("in_answer_check"))
                ),
            "{over:?}: an ordinary answer's check leaves"
        );
    }
    // A check the curator hands on stays inside for a silent call, too.
    let check = json!({"route": "answer_check", "section": "claims", "turn_id": TURN});
    for p in through("talky", "./curator", ctx.clone(), check.clone()) {
        assert_ne!(p.to, ".", "the curator's check leaves: edge {}", p.edge);
    }
    assert!(
        through("talky", "./curator", ordinary(ctx), check)
            .iter()
            .any(|p| p.to == "."),
        "an ordinary check leaves the curator"
    );
}

/// I1 (b): an error or a content filter of the silent call on `brain` says
/// nothing to the rim; an ordinary brain error still reaches `./errors`.
#[test]
fn gh_d54g_listennote_a_failed_silent_call_says_no_error() {
    if !shipped() {
        return;
    }
    let (_h, call) = silent_call();
    let ctx = lifted_to(&call, "brain");
    for finish in ["error", "content_filter"] {
        let hop = json!({"finish_reason": finish});
        for p in through("talky", "./brain", ctx.clone(), hop.clone()) {
            assert!(
                p.to != "./errors" && p.to != ".",
                "a silent {finish} reaches {} (edge {})",
                p.to,
                p.edge
            );
        }
        assert!(
            through("talky", "./brain", ordinary(ctx.clone()), hop)
                .iter()
                .any(|p| p.to == "./errors"),
            "an ordinary {finish} of brain reaches ./errors"
        );
    }
}

/// I1 (c): a silent answer with `memory` and `claims`: `memory` reaches the
/// followup, `claims` leaves talky on no edge -- unchecked (edge 55) or
/// checked by `sidecar_verify` (edge 58).
#[test]
fn gh_d54g_listennote_a_silent_answer_hands_out_only_memory() {
    if !shipped() {
        return;
    }
    let claims = json!({"items": [{"claim": CLAIM, "quote": SAID}]});
    let block = {
        let text = sections_answer();
        let inner = text
            .trim_start_matches("```sidecar\n")
            .trim_end_matches("\n```");
        let mut v: Value = sj::from_str(inner).expect("the block");
        v["claims"] = claims;
        format!("```sidecar\n{v}\n```")
    };
    for over in [
        vec![],
        vec![("sidecar_verify", json!({"claims": {"quote": "turn"}}))],
    ] {
        let (mut h, call) = silent_call();
        let ctx = lifted_context(&call);
        let emitted = split_as(&call, &block, "stop", &over);
        assert!(
            emitted
                .iter()
                .any(|m| m["header"]["section"] == json!("claims")),
            "{over:?}: the splitter writes `claims`: {emitted:?}"
        );
        let passed = out_of_splitter(&emitted, &ctx);
        for (p, m) in &passed {
            assert!(
                m["header"]["section"] != json!("claims"),
                "{over:?}: the silent `claims` leaves to {} (edge {})",
                p.to,
                p.edge
            );
        }
        assert!(
            out_of_splitter(&emitted, &ordinary(ctx))
                .iter()
                .any(|(_, m)| m["header"]["section"] == json!("claims")),
            "{over:?}: an ordinary answer's `claims` leaves the splitter"
        );
        memory_reaches_the_followup(&mut h, &passed);
    }
}
