//! #1279 -- a turn nobody can read yet is asked back, the
//! asking is learnt, and the quick voice has a prompt of its own.
//!
//! WHY this file exists: a turn whose words can be meant in more than one way
//! was answered on a guess -- the stamp fell back to `talk` silently, or the
//! quick voice spoke before anybody knew what was meant. With a set that
//! leads the class `clarify`, the route stamps such a turn `clarify`; the
//! curator gives it to the brain alone (no quick voice, a spec answer
//! dropped, no tool, so nothing opens a window), with one turn note: ask how
//! it is meant, guess nothing. The question and the person's next words go to
//! the affinity as ONE proposal, so the picture of how this person asks grows
//! from the asking. Beside it the quick voice reads a slot of its own,
//! `instructions.quick` (switch `quick_instructions_slot`), which the brain
//! -- it reads the whole `instructions` family -- must never get, and with a
//! `place`/`timezone` a `frame` of the moment. Defaults change nothing.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};

/// A curate of turn `turn` stamped `path`, with `system` slots when given.
fn curate_on(h: &mut Hive, turn: &str, path: &str, ask: &str, system: Value) {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": path, "families": ""});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": path});
    let mut body = json!({"messages": [user(ask)]});
    if system.is_object() {
        body["system"] = system;
    }
    h.lane("in_curate", ctx, hop, body);
}

/// The collector's early curate of a turn (#1275): `spec` "1", no path.
fn spec_curate(h: &mut Hive, turn: &str, ask: &str) -> Msg {
    h.out.clear();
    let hop = json!({"session_id": "s1", "turn_id": turn, "iter": "0", "phase": "",
                     "path": "", "spec": "1"});
    let ctx = json!({"session_id": "s1", "turn_id": turn, "iter": "0",
                     "channel": "test", "audience_set": ROUND_E, "path": ""});
    h.lane("in_curate", ctx, hop, json!({"messages": [user(ask)]}));
    let quick = h.routed("quick");
    assert_eq!(quick.len(), 1, "one spec call: {:?} {:?}", h.out, h.stderr);
    quick[0].clone()
}

/// The quick voice's answer to a spec call on `in_spec_llm`.
fn spec_answer(h: &mut Hive, call: &Msg, text: &str) {
    h.out.clear();
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert("spec".into(), json!("1"));
    h.lane(
        "in_spec_llm",
        Value::Object(ctx),
        json!({"finish_reason": "stop", "model": "test-model", "tokens_prompt": 10}),
        json!({"messages": [said(text)]}),
    );
}

/// The entry talky's curator node sets for its class `clarify` in the
/// policy's `class_notes` (GH #1478: the class is the composition's, the
/// curator ships none).
fn talky_class_notes() -> Value {
    let node = read_json(&repo("templates/talky/curator/config.json"));
    node["override_params"]["policy"]["class_notes"].clone()
}

fn note_text() -> String {
    talky_class_notes()["clarify"]["text"]
        .as_str()
        .expect("talky's clarify note")
        .to_string()
}

/// `text` with what a run makes fresh replaced: a lowercase `8-4-4-4-12` UUID
/// becomes `<uuid>`, an RFC 3339 instant (`YYYY-MM-DDTHH:MM:SS[.f]Z`) `<at>`.
fn fresh_masked(text: &str) -> String {
    let b = text.as_bytes();
    let hex = |c: &u8| c.is_ascii_digit() || (b'a'..=b'f').contains(c);
    let digits = |s: &[u8], n: usize| s.len() >= n && s[..n].iter().all(u8::is_ascii_digit);
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        let s = &b[i..];
        let uuid = s.len() >= 36
            && s[..36].iter().enumerate().all(|(k, c)| {
                if [8, 13, 18, 23].contains(&k) {
                    *c == b'-'
                } else {
                    hex(c)
                }
            });
        if uuid {
            out.push_str("<uuid>");
            i += 36;
            continue;
        }
        let at = digits(s, 4)
            && s.get(4) == Some(&b'-')
            && digits(&s[5..], 2)
            && s.get(7) == Some(&b'-')
            && digits(&s[8..], 2)
            && s.get(10) == Some(&b'T');
        if at {
            let mut j = 11;
            while j < s.len() && (s[j].is_ascii_digit() || s[j] == b':' || s[j] == b'.') {
                j += 1;
            }
            if s.get(j) == Some(&b'Z') {
                j += 1;
            }
            out.push_str("<at>");
            i += j;
            continue;
        }
        let ch = text[i..].chars().next().expect("a char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn system_of(call: &Msg) -> Value {
    call.body.get("system").cloned().unwrap_or(json!({}))
}

/// The slots of a set that renders a quick prompt beside the brain's.
fn prompts() -> Value {
    json!({"instructions": {"speaker": {"text": "BRAIN PROMPT"},
                            "quick": {"text": "QUICK PROMPT"},
                            "mode": {"text": "Be brief."}}})
}

#[test]
fn clarify_stamp_drops_quick_and_opens_no_window() {
    if !shipped() {
        return;
    }
    let mut h = Hive::with(&[
        ("policy", "class_notes", talky_class_notes()),
        ("lead", "quick_speculative", json!("1")),
        ("lead", "spec_tools", json!("web")),
    ])
    .reporting_composite("talky");
    h.check_emits = true;
    let spec = spec_curate(&mut h, "t1", "the choir in Dresden, search please");
    spec_answer(&mut h, &spec, "SPECGUESS the choir sings on Friday");
    assert!(h.out.is_empty(), "a held answer goes nowhere: {:?}", h.out);

    curate_on(
        &mut h,
        "t1",
        "clarify",
        "the choir in Dresden, search please",
        json!(null),
    );
    let brain = h.routed("brain");
    assert_eq!(
        brain.len(),
        1,
        "a clarify turn is the brain's: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(h.routed("quick").is_empty(), "no quick voice, no bridge");
    assert!(
        h.routed("spec_release").is_empty(),
        "the spec answer is dropped"
    );
    assert!(
        !h.out
            .iter()
            .any(|m| sj::to_string(&m.body).unwrap().contains("SPECGUESS")),
        "nothing of the guess leaves: {:?}",
        h.out
    );
    assert_eq!(
        brain[0].body["tool_scope"],
        json!({"allow": []}),
        "a question calls no tool and opens nothing"
    );
    let note = note_text();
    assert!(
        brain[0]
            .messages()
            .iter()
            .any(|m| m["text"].as_str().is_some_and(|t| t.contains(&note))),
        "the one turn note asks how it is meant: {:?}",
        brain[0].messages()
    );
    assert!(
        h.state("turn_system:s1:t1").is_empty(),
        "no frozen system part of the turn"
    );
    let marks = h.rows("SELECT value FROM marks WHERE kind = 'spec'");
    assert_eq!(marks.len(), 1, "{marks:?}");
    let m: Value = sj::from_str(marks[0][0].as_str().unwrap()).unwrap();
    assert_eq!(m["verdict"], json!("dropped"));
    assert_eq!(m["reason"], json!("path:clarify"));
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);

    // The channel's deny stays on the empty menu.
    let mut d =
        Hive::with(&[("policy", "class_notes", talky_class_notes())]).reporting_composite("talky");
    d.out.clear();
    d.lane(
        "in_curate",
        json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "channel": "test",
               "audience_set": ROUND_E, "path": "clarify"}),
        json!({"session_id": "s1", "turn_id": "t1", "iter": "0", "phase": "",
               "path": "clarify"}),
        json!({"messages": [user("that one")],
               "tool_scope": {"allow": ["web", "orga"], "deny": ["bash"]}}),
    );
    assert_eq!(
        d.routed("brain")[0].body["tool_scope"],
        json!({"allow": [], "deny": ["bash"]})
    );
}

#[test]
fn clarify_answer_becomes_affinity_hint() {
    if !shipped() {
        return;
    }
    let mut h =
        Hive::with(&[("policy", "class_notes", talky_class_notes())]).reporting_composite("talky");
    h.check_emits = true;
    curate_on(
        &mut h,
        "t1",
        "clarify",
        "the choir in Dresden, search please",
        json!(null),
    );
    let ask = h.routed("brain")[0].clone();
    h.tap(
        &ask,
        "stop",
        json!({}),
        json!([said("Do you mean its concerts or its website?")]),
    );
    assert!(h.routed("propose").is_empty(), "the pair is not whole yet");
    assert!(
        h.state("clarify:s1").contains("\"t1\""),
        "{}",
        h.state("clarify:s1")
    );

    curate_on(&mut h, "t2", "talk", "its concerts", json!(null));
    let p = h.routed("propose");
    assert_eq!(p.len(), 1, "ONE proposal: {:?} {:?}", h.out, h.stderr);
    let turn = &p[0].messages()[0];
    assert_eq!(
        turn["type"],
        json!("tool_call"),
        "affinity's in_propose form"
    );
    let args: Value = sj::from_str(turn["text"].as_str().unwrap()).unwrap();
    assert_eq!(args["op"], json!("propose"));
    assert_eq!(args["entity_ref"], json!("member:e"), "the round's person");
    assert_eq!(args["field_path"], json!("mx.hints.clarify"));
    assert!(
        args["source_ref"]
            .as_str()
            .is_some_and(|s| s.contains("s1#t1"))
    );
    assert_eq!(
        args["value"]["question"],
        json!("Do you mean its concerts or its website?")
    );
    assert!(
        args["value"]["answer"]
            .as_str()
            .is_some_and(|a| a.contains("its concerts")),
        "{args}"
    );
    assert_eq!(
        args["value"]["class"],
        json!("talk"),
        "the next stamp's class"
    );
    assert_eq!(h.routed("brain").len(), 1, "the turn itself goes on");
    assert!(h.state("clarify:s1").is_empty(), "the row is taken");
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);

    // Exactly once: the next turn proposes nothing.
    let call = h.routed("brain")[0].clone();
    h.tap(
        &call,
        "stop",
        json!({}),
        json!([said("They sing on Friday.")]),
    );
    curate_on(&mut h, "t3", "talk", "thanks", json!(null));
    assert!(h.routed("propose").is_empty(), "{:?}", h.out);
}

/// The fast cells of talky with `instructions.only` and `frame` on: what
/// the knob `quick_instructions_slot` "1" did before (OR-d58-2.target.53).
fn only_on() -> Vec<(&'static str, Value)> {
    let on = json!({"instructions": {"only": true}, "frame": true});
    vec![
        ("quick", on.clone()),
        ("quick_spec", on.clone()),
        ("quick_bridge", on),
    ]
}

#[test]
fn quick_reads_instructions_quick_slot() {
    if !shipped() {
        return;
    }
    // Off (the default, no `instructions.only`): the family stays and the
    // fast cell's own slot stands beside it. OR-d58-2.target.50: the
    // profile's text holds only while the slot is empty -- the composite
    // brings the slot here, so its words stand (the knob of before put the
    // fixed text over them).
    let mut off = Hive::with(&[]).reporting_composite("talky");
    curate_on(&mut off, "t1", "simple", "weather in Berlin?", prompts());
    let q = off.routed("quick");
    assert_eq!(q.len(), 1, "{:?} {:?}", off.out, off.stderr);
    let ins = &system_of(&q[0])["instructions"];
    assert_eq!(ins["speaker"]["text"], json!("BRAIN PROMPT"), "{ins}");
    assert_eq!(
        ins["quick"]["text"],
        json!("QUICK PROMPT"),
        "the slot beside the family: {ins}"
    );
    // Without the slot in the system part the profile's text fills it.
    let mut bare = Hive::with(&[]).reporting_composite("talky");
    let mut no_slot = prompts();
    if let Some(i) = no_slot["instructions"].as_object_mut() {
        i.remove("quick");
    }
    curate_on(&mut bare, "t1", "simple", "weather in Berlin?", no_slot);
    let q = bare.routed("quick");
    assert_eq!(q.len(), 1, "{:?} {:?}", bare.out, bare.stderr);
    let ins = &system_of(&q[0])["instructions"];
    assert!(
        ins["quick"]["text"]
            .as_str()
            .is_some_and(|t| t.starts_with("When you call a tool")),
        "the profile's text in the empty slot: {ins}"
    );
    assert!(
        system_of(&q[0]).get("frame").is_none(),
        "no frame by default"
    );

    // On: the slot is the quick voice's instructions.
    // OR-d58-2.target.62: the switch of before is the profile's
    // `instructions.only` and `frame` on the fast cells.
    let mut on = Hive::with(&[
        ("policy", "place", json!("Berlin")),
        ("policy", "timezone", json!("Europe/Berlin")),
    ])
    .reporting_composite_with("talky", &only_on());
    curate_on(&mut on, "t1", "simple", "weather in Berlin?", prompts());
    let q = on.routed("quick");
    assert_eq!(q.len(), 1, "{:?} {:?}", on.out, on.stderr);
    let sys = system_of(&q[0]);
    let ins = &sys["instructions"];
    assert_eq!(ins["quick"]["text"], json!("QUICK PROMPT"), "{ins}");
    assert!(
        ins.get("speaker").is_none(),
        "the brain's prompt stays out: {ins}"
    );
    assert_eq!(
        ins["mode"]["text"],
        json!("Be brief."),
        "session slots stay"
    );
    let frame = sys["frame"]["text"].as_str().unwrap_or_default();
    assert!(frame.contains("place: Berlin"), "{frame}");
    assert!(frame.contains("(Europe/Berlin)"), "{frame}");

    // A bridge reaches `./quick` too and reads the same slot.
    curate_on(&mut on, "t2", "complex", "plan my week", prompts());
    let b = on.routed("quick");
    assert_eq!(b.len(), 1, "{:?} {:?}", on.out, on.stderr);
    assert_eq!(b[0].hop["call_kind"], json!("side"));
    let bins = &system_of(&b[0])["instructions"];
    assert_eq!(bins["quick"]["text"], json!("QUICK PROMPT"), "{bins}");
    assert!(bins.get("speaker").is_none(), "{bins}");
    assert!(bins.get("bridge").is_some(), "{bins}");
}

#[test]
fn brain_never_sees_instructions_quick() {
    if !shipped() {
        return;
    }
    for switch in ["0", "1"] {
        let patches = if switch == "1" { only_on() } else { Vec::new() };
        let mut h = Hive::with(&[("policy", "timezone", json!("Europe/Berlin"))])
            .reporting_composite_with("talky", &patches);
        for (turn, path) in [("t1", "talk"), ("t2", ""), ("t3", "clarify")] {
            curate_on(&mut h, turn, path, "hello there", prompts());
            let brain = h.routed("brain");
            assert_eq!(
                brain.len(),
                1,
                "{switch} {path}: {:?} {:?}",
                h.out,
                h.stderr
            );
            let sys = system_of(&brain[0]);
            let ins = &sys["instructions"];
            assert_eq!(ins["speaker"]["text"], json!("BRAIN PROMPT"), "{ins}");
            assert!(
                ins.get("quick").is_none(),
                "switch {switch}, path {path:?}: the brain got the quick prompt: {ins}"
            );
            assert!(sys.get("frame").is_none(), "no frame for the brain: {sys}");
            assert!(
                !sj::to_string(&brain[0].body)
                    .unwrap()
                    .contains("QUICK PROMPT"),
                "nowhere in the brain's call"
            );
            let call = brain[0].clone();
            h.tap(&call, "stop", json!({}), json!([said("hi")]));
        }
    }
}

#[test]
fn fu2_policy_defaults_byte_identical() {
    if !shipped() {
        return;
    }
    // The params ship at today's behaviour and say so in the contract.
    let policy = cell_config("policy");
    // OR-d58-2.target.62: the switch `quick_instructions_slot` left the
    // curator (the profile's `instructions.only` took its place).
    assert!(policy["params"].get("quick_instructions_slot").is_none());
    assert!(
        policy["contract"]["settings"]
            .get("quick_instructions_slot")
            .is_none()
    );
    for (k, v) in [
        ("place", json!("")),
        ("timezone", json!("")),
        ("class_notes", json!({})),
    ] {
        assert_eq!(policy["params"][k], v, "{k}");
        assert_eq!(policy["contract"]["settings"][k]["default"], v, "{k}");
    }
    // A run without a clarify stamp: the same calls with the knobs shipped and
    // with the knobs absent (null), and nothing of #1279 in the ledger.
    let run = |over: &[(&str, &str, Value)]| {
        let mut h = Hive::with(over).reporting_composite("talky");
        let mut seen = Vec::new();
        for (turn, path) in [
            ("t1", "talk"),
            ("t2", "simple"),
            ("t3", "complex"),
            ("t4", ""),
        ] {
            curate_on(&mut h, turn, path, "what about tomorrow?", prompts());
            for m in &h.out {
                seen.push(json!({"route": m.route(), "system": m.body.get("system"),
                                 "tool_scope": m.body.get("tool_scope")}));
            }
            for call in h.routed("brain").into_iter().chain(h.routed("quick")) {
                h.tap(&call, "stop", json!({}), json!([said("fine")]));
            }
        }
        assert!(
            h.rows("SELECT key FROM state WHERE key LIKE 'clarify:%'")
                .is_empty()
        );
        assert!(h.routed("propose").is_empty());
        seen
    };
    let shipped_run = run(&[]);
    let absent_run = run(&[
        ("policy", "place", Value::Null),
        ("policy", "timezone", Value::Null),
        ("policy", "class_notes", Value::Null),
    ]);
    // A consult's pending line names its fresh id and its start: the two
    // runs differ there by construction, nowhere else.
    assert_eq!(
        fresh_masked(&sj::to_string(&shipped_run).unwrap()),
        fresh_masked(&sj::to_string(&absent_run).unwrap())
    );
    assert!(
        !sj::to_string(&shipped_run).unwrap().contains("\"frame\""),
        "no frame without place/timezone"
    );
}

/// live-2 #1370: the switch set as JSON `true` (the set's form) is on -- one
/// rule for every switch of the wave, the presenter's `knob()`. Since
/// OR-d58-2.target.62 the switch is the profile's `instructions.only`.
#[test]
fn quick_instructions_slot_reads_json_true() {
    if !shipped() {
        return;
    }
    let on = json!({"instructions": {"only": true}});
    let mut on = Hive::with(&[]).reporting_composite_with(
        "talky",
        &[
            ("quick", on.clone()),
            ("quick_spec", on.clone()),
            ("quick_bridge", on),
        ],
    );
    curate_on(&mut on, "t1", "simple", "weather in Berlin?", prompts());
    let q = on.routed("quick");
    assert_eq!(q.len(), 1, "{:?} {:?}", on.out, on.stderr);
    let ins = &system_of(&q[0])["instructions"];
    assert_eq!(
        ins["quick"]["text"],
        json!("QUICK PROMPT"),
        "true is on: {ins}"
    );
    assert!(
        ins.get("speaker").is_none(),
        "true is on: the slot replaces the family: {ins}"
    );
}
