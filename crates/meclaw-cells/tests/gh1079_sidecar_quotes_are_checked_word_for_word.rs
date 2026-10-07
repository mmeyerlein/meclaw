//! GH #1079 -- a quote in a sidecar section is checked word for word.
//!
//! A section of the model's block leaves the talky on `sidecar` with
//! `{route, section, turn_id}` and an empty `messages`: the words of the turn it
//! annotates never leave the composite (`templates/talky/README.md`, "The
//! sidecar"). So a receiver that wants to know whether a quoted field is
//! really what the person said cannot look it up -- the check has to run
//! where the words are. Two cells hold them: the splitter holds the answer
//! text, the curator holds the turns and the memory bundle of the round on
//! its wall. The composite's knob `sidecar_verify` names, per section, the
//! fields to check and their source (`turn`, `turn+recall`, `answer`); the
//! splitter stamps the section's rule on its hop (and the answer text beside
//! it when a field names `answer`), the talky routes a stamped section through
//! its curator, and the curator's intake checks it against the round and hands
//! it on -- each failed item as one `reject` without its words, which leaves
//! the talky on its `error` lane.
//!
//! What is pinned here: the knob ships empty and empty is byte for byte the
//! behaviour before it; the answer path does not move; a stamped section is
//! routed through the curator in both composites; a refusal carries no text;
//! and the whole road on three synthetic turns. The intake's rule itself
//! (normal form, sources, modes) is pinned in `curator_cells.rs`.

#[path = "support/curator_hive.rs"]
mod curator_hive;

use curator_hive::*;
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{self as sj, Map, Value, json};
use meclaw_core::{Headers, Path, Uuid};

const SPLITTER: &str = "templates/talky/splitter/config.json";
const ERRORS: &str = "templates/talky/errors/config.json";

fn abs(rel: &str) -> String {
    repo(rel).to_string_lossy().to_string()
}

/// The splitter's params as shipped, the knob set to `knob` or left out.
fn splitter_params(knob: Option<Value>) -> Value {
    let mut p = read_json(&repo(SPLITTER))["params"].clone();
    let o = p.as_object_mut().expect("params");
    o.remove("script_inline");
    match knob {
        Some(k) => {
            o.insert("sidecar_verify".into(), k);
        }
        None => {
            o.remove("sidecar_verify");
        }
    }
    p
}

/// The shipped splitter's stdout, byte for byte, for one brain completion.
fn split_raw(finish: &str, turns: &Value, knob: Option<Value>) -> String {
    let doc = json!({
        "header": {"hop": {"finish_reason": finish},
                   "context": {"session_id": "s-1079", "turn_id": "t-1079"}},
        "messages": turns,
        "params": splitter_params(knob),
    });
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(&abs(SPLITTER)),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "the splitter exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}

/// The splitter's stderr for one brain completion (it still has to succeed).
fn split_stderr(finish: &str, turns: &Value, knob: Option<Value>) -> String {
    let doc = json!({
        "header": {"hop": {"finish_reason": finish},
                   "context": {"session_id": "s-1079", "turn_id": "t-1079"}},
        "messages": turns,
        "params": splitter_params(knob),
    });
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(&abs(SPLITTER)),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "the splitter exited non-zero: {err}");
    err
}

/// The same, parsed: every message the splitter emitted, the answer half first.
fn split(finish: &str, turns: &Value, knob: Option<Value>) -> Vec<Value> {
    match sj::from_str::<Value>(&split_raw(finish, turns, knob)).expect("json") {
        Value::Array(a) => a,
        one => vec![one],
    }
}

fn section_of<'a>(out: &'a [Value], name: &str) -> &'a Value {
    out.iter()
        .find(|m| m["header"]["section"] == json!(name))
        .unwrap_or_else(|| panic!("no `{name}` section left the splitter: {out:#?}"))
}

/// An answer that carries a ```sidecar block.
fn answer_with(prose: &str, block: &str) -> Value {
    json!([said(&format!("{prose}\n\n```sidecar\n{block}\n```"))])
}

const NEEDS_BLOCK: &str = r#"{"needs": {"needs": [{"need": "connection", "quote": "miss Werner's patient ways"}, {"need": "solitude", "quote": "I like being alone"}]}, "memory": {"nothing_new": true, "facts": [], "topic": {"movement": "continue"}}}"#;

// ------------------------------------------------------------ the edges

fn table(base: &str, rel: &str) -> EdgeTable {
    let params = read_json(&repo(rel))["params"].clone();
    let hp: HiveParams = sj::from_value(params).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let mut t = EdgeTable::new();
    for spec in &hp.graph.edges {
        let spec: &EdgeSpec = spec;
        let at = |ep: &str| match ep {
            "." => base.to_string(),
            other => format!("{base}/{}", other.trim_start_matches("./")),
        };
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&at(&spec.from)),
            to: Path::new(&at(&spec.to)),
            condition: spec.condition.as_ref().map(|src| {
                meclaw_colony::cel_eval::parse_condition(src)
                    .unwrap_or_else(|e| panic!("{rel}: condition {src:?}: {e}"))
            }),
            modifier: spec.modifier.as_ref().map(|m| {
                meclaw_colony::cel_eval::parse_modifier(m)
                    .unwrap_or_else(|(k, e)| panic!("{rel}: modifier {k}: {e}"))
            }),
            is_default: spec.is_default,
            lane: spec.lane.clone(),
            tap: spec.tap,
        });
    }
    t
}

fn deliveries(t: &EdgeTable, from: &str, hop: Value) -> Vec<(String, Map<String, Value>)> {
    let hs = Headers::from_parts(
        obj(json!({"session_id": "s1", "turn_id": "t1", "curator_call": "k"})),
        obj(hop),
    );
    apply_edges(t, &Path::new(from), &hs)
        .into_iter()
        .map(|d| (d.target.as_str().to_string(), d.headers_out.hop.clone()))
        .collect()
}

fn targets(d: &[(String, Map<String, Value>)]) -> Vec<String> {
    d.iter().map(|(to, _)| to.clone()).collect()
}

/// The condition of the one edge `from -> to` whose condition holds `needle`.
fn condition(rel: &str, from: &str, to: &str, needle: &str) -> String {
    let cfg = read_json(&repo(rel));
    let found: Vec<String> = cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .filter(|e| e["from"] == json!(from) && e["to"] == json!(to))
        .filter_map(|e| e["condition"].as_str().map(str::to_string))
        .filter(|c| c.contains(needle))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "{rel}: {from} -> {to} on {needle}: {found:?}"
    );
    found[0].clone()
}

// ------------------------------------------------------------ 1. empty is the old behaviour

#[test]
fn sidecar_verify_empty_knob_is_byte_identical() {
    if !shipped() {
        return;
    }
    assert_eq!(
        read_json(&repo(SPLITTER))["params"]["sidecar_verify"],
        json!({}),
        "the knob ships empty"
    );
    // The answer shapes of gh379, gh534 and gh1036: a block of two sections,
    // the legacy fence, no block, a block that does not read, a block beside a
    // call, a cut completion, a bare string section.
    let fixtures: Vec<(&str, Value)> = vec![
        ("stop", answer_with("Sure, I will.", NEEDS_BLOCK)),
        (
            "stop",
            json!([said(
                "Noted.\n\n```memory\n{\"facts\": [], \"nothing_new\": true}\n```"
            )]),
        ),
        ("stop", json!([said("No block here at all.")])),
        (
            "stop",
            json!([said("Oops.\n\n```sidecar\n{\"needs\": {\"needs\": [\n```")]),
        ),
        (
            "tool_calls",
            json!([
                said(&format!("Let me look.\n\n```sidecar\n{NEEDS_BLOCK}\n```")),
                tool_call("c1", "web_search")
            ]),
        ),
        ("length", answer_with("Cut short", NEEDS_BLOCK)),
        (
            "stop",
            answer_with("Okay.", r#"{"fact": "The meeting is on Tuesday."}"#),
        ),
    ];
    for (finish, turns) in &fixtures {
        let without = split_raw(finish, turns, None);
        assert_eq!(
            split_raw(finish, turns, Some(json!({}))),
            without,
            "{finish}: an empty knob is no knob"
        );
        assert_eq!(
            split_raw(finish, turns, Some(json!({"claims": {"quote": "turn"}}))),
            without,
            "{finish}: a knob for a section the block does not carry touches nothing"
        );
        assert!(
            !without.contains("sidecar_verify") && !without.contains("verify_answer"),
            "{without}"
        );
    }
    // The intake: a section without the hop key goes on exactly as before.
    let mut h = Hive::with(&[("intake", "pass_sections", json!("memory"))]);
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user("I miss Werner's patient ways.")]),
        mode("Be brief."),
    );
    h.out.clear();
    let payload = json!({"facts": [{"subject": "person", "predicate": "misses",
                                    "object": "Werner", "quote": "words nobody said"}],
                         "topic": {"movement": "start", "name": "Werner"}});
    h.section(&call, "memory", payload.clone());
    let sides = h.routed("sidecar");
    assert_eq!(sides.len(), 1, "{:?}", h.out);
    assert_eq!(
        sides[0].body,
        obj(json!({"messages": [], "section": "memory", "payload": payload}))
    );
    assert_eq!(
        sides[0].hop,
        obj(json!({"route": "sidecar", "section": "memory"}))
    );
    assert!(h.routed("reject").is_empty(), "{:?}", h.out);
    h.out.clear();
    h.section(
        &call,
        "needs",
        json!({"needs": [{"need": "connection", "quote": "nobody said this"}]}),
    );
    assert!(
        h.out.is_empty(),
        "an unchecked section of none of the curator's is dropped, as before: {:?}",
        h.out
    );
}

// ------------------------------------------------------------ 2. the answer path

#[test]
fn sidecar_verify_does_not_touch_answer_path() {
    if !shipped() {
        return;
    }
    let knob = json!({"needs": {"quote": "turn"}, "memory": {"said": "answer"}});
    let turns = answer_with("Sure, I will.", NEEDS_BLOCK);
    let plain = split("stop", &turns, None);
    let checked = split("stop", &turns, Some(knob.clone()));
    assert_eq!(
        checked[0], plain[0],
        "the answer half is the same with the knob as without it"
    );
    assert_eq!(checked.len(), plain.len(), "{checked:#?}");
    // The knob was on: the sections carry their rule, and the answer text
    // rides beside the one that checks against it -- on the section, never on
    // the answer half.
    assert_eq!(
        section_of(&checked, "needs")["header"]["sidecar_verify"],
        json!(r#"{"quote":"turn"}"#)
    );
    assert_eq!(
        section_of(&checked, "memory")["verify_answer"],
        json!("Sure, I will.")
    );
    for name in ["needs", "memory"] {
        assert_eq!(
            section_of(&checked, name)["payload"],
            section_of(&plain, name)["payload"],
            "{name}: the splitter checks nothing itself"
        );
    }
    // A tool round and a cut completion: byte for byte.
    let tool = json!([
        said(&format!("Let me look.\n\n```sidecar\n{NEEDS_BLOCK}\n```")),
        tool_call("c1", "web_search")
    ]);
    assert_eq!(
        split_raw("tool_calls", &tool, Some(knob.clone())),
        split_raw("tool_calls", &tool, None)
    );
    let cut = split("length", &turns, Some(knob));
    assert_eq!(cut[0], split("length", &turns, None)[0]);
    // The talky's answer edge did not move, and the answer half takes it alone.
    assert_eq!(
        condition(
            "templates/talky/config.json",
            "./splitter",
            "./dispatcher",
            "finish_reason"
        ),
        "has(hop.finish_reason) && (hop.finish_reason == 'stop' || hop.finish_reason == 'tool_calls')"
    );
    let t = table("/t", "templates/talky/config.json");
    assert_eq!(
        targets(&deliveries(&t, "/t/splitter", checked[0]["header"].clone())),
        vec!["/t/dispatcher"]
    );
}

#[test]
fn sidecar_verify_stamps_the_rule_and_the_answer_text() {
    if !shipped() {
        return;
    }
    let turns = json!([said(&format!(
        "Werner was patient [#0123456789ab] with you.\n\n```sidecar\n{}\n```",
        r#"{"needs": {"needs": []}, "claims": {"claims": []}, "memory": {"facts": []}}"#
    ))]);
    let knob = json!({
        "claims": {"said": "answer", "quote": "turn", "mode": "mark"},
        "needs": {"quote": "turn+recall"},
        // A rule that names no source checks nothing and stamps nothing.
        "memory": {"quote": "somewhere", "mode": "mark"}
    });
    for form in [knob.clone(), json!(knob.to_string())] {
        let out = split("stop", &turns, Some(form));
        let claims = section_of(&out, "claims");
        assert_eq!(
            claims["header"]["sidecar_verify"],
            json!(r#"{"mode":"mark","quote":"turn","said":"answer"}"#),
            "the section's rule, canonical"
        );
        assert_eq!(
            claims["verify_answer"],
            json!("Werner was patient with you."),
            "the answer as the person reads it: the block cut, the ids out"
        );
        let needs = section_of(&out, "needs");
        assert_eq!(
            needs["header"]["sidecar_verify"],
            json!(r#"{"quote":"turn+recall"}"#)
        );
        assert!(
            needs.get("verify_answer").is_none(),
            "no `answer` field, no answer text: {needs}"
        );
        let memory = section_of(&out, "memory");
        assert!(memory["header"].get("sidecar_verify").is_none(), "{memory}");
        assert!(memory.get("verify_answer").is_none(), "{memory}");
    }
    // The contract says it, and the core's copy is the same contract
    // (`cognys_splitter_is_talkys`).
    let cfg = read_json(&repo(SPLITTER));
    let c = &cfg["contract"];
    assert_eq!(c["version"], "1.1.0");
    assert_eq!(c["settings"]["sidecar_verify"]["type"], "object");
    assert_eq!(c["settings"]["sidecar_verify"]["default"], json!({}));
    assert_eq!(c["emits"]["hop"]["sidecar_verify"]["type"], "string");
    assert_eq!(c["emits"]["body"]["verify_answer"]["type"], "string");
}

/// `verified` and `failed` are the curator's stamp. In a composite that
/// checks (the knob is not empty) a model that writes them itself must not get
/// them past the splitter -- not on the checked array (the curator overwrites
/// that one anyway), not on another array of a checked section, not on the
/// payload itself, and not on a section the knob does not name: a reader that
/// trusts the stamp cannot tell where it came from, so it may only ever come
/// from the check.
#[test]
fn sidecar_verify_a_models_own_stamp_never_leaves_the_splitter() {
    if !shipped() {
        return;
    }
    let block = r#"{"needs": {"verified": true, "needs": [{"need": "connection", "quote": "miss Werner's patient ways", "verified": ["quote"]}], "note": [{"why": "said so", "failed": []}]}, "topics": {"topics": [{"name": "garden", "verified": ["name"], "failed": [{"field": "name"}]}], "count": 1}, "advice": "Ask about the garden."}"#;
    let turns = answer_with("He sounds like he was a kind man.", block);
    let out = split("stop", &turns, Some(json!({"needs": {"quote": "turn"}})));
    assert_eq!(
        section_of(&out, "needs")["payload"],
        json!({"needs": [{"need": "connection", "quote": "miss Werner's patient ways"}],
               "note": [{"why": "said so"}]}),
        "the checked section leaves without the model's stamp"
    );
    assert_eq!(
        section_of(&out, "topics")["payload"],
        json!({"topics": [{"name": "garden"}], "count": 1}),
        "a section the knob does not name leaves without it too"
    );
    assert_eq!(
        section_of(&out, "advice")["payload"],
        json!({"payload": "Ask about the garden."}),
        "a bare string section is the model's string, byte for byte"
    );
    // Empty, the knob changes nothing: the fields are the model's then, and
    // nothing downstream reads them as proof.
    let plain = split("stop", &turns, None);
    assert_eq!(
        section_of(&plain, "topics")["payload"]["topics"][0]["verified"],
        json!(["name"])
    );
}

/// A typo in the knob is said, never swallowed: an operator who wrote
/// `"turns"` for `"turn"` believes the section is checked while nothing is
/// stamped. Each part of a rule the splitter cannot use -- a rule that is no
/// object, a field whose source is none of the three, a `mode` that is
/// neither `drop` nor `mark`, an `items` that is no name -- gets one stderr
/// line naming the section and the part, and the rest of the rule still
/// counts.
#[test]
fn sidecar_verify_a_typo_in_the_knob_is_said() {
    if !shipped() {
        return;
    }
    let turns = answer_with("He sounds like he was a kind man.", NEEDS_BLOCK);
    let err = split_stderr(
        "stop",
        &turns,
        Some(json!({"needs": {"quote": "turns"}, "topics": "turn",
                    "memory": {"claim": "turn", "mode": "flag", "items": 3}})),
    );
    for said in [
        "needs.quote",
        "turns",
        "topics",
        "memory.mode",
        "flag",
        "memory.items",
    ] {
        assert!(err.contains(said), "stderr names `{said}`: {err}");
    }
    let out = split(
        "stop",
        &turns,
        Some(json!({"needs": {"quote": "turns"}, "memory": {"claim": "turn", "mode": "flag"}})),
    );
    assert!(
        section_of(&out, "needs")["header"]
            .get("sidecar_verify")
            .is_none(),
        "a rule with no usable field is still not stamped"
    );
    assert!(
        section_of(&out, "memory")["header"]
            .get("sidecar_verify")
            .is_some(),
        "the usable rest of a rule still counts"
    );
    // A clean knob says nothing.
    assert_eq!(
        split_stderr("stop", &turns, Some(json!({"needs": {"quote": "turn"}}))),
        ""
    );
}

#[test]
fn sidecar_verify_routes_a_checked_section_through_the_curator() {
    if !shipped() {
        return;
    }
    let rule = r#"{"quote":"turn"}"#;
    for (base, rel) in [
        ("/t", "templates/talky/config.json"),
        ("/c", "templates/cogny/config.json"),
    ] {
        let t = table(base, rel);
        let from = format!("{base}/splitter");
        let curator = format!("{base}/curator");
        // Stamped: through the curator, once, never also out of the rim.
        for section in ["needs", "memory", "window"] {
            let d = deliveries(
                &t,
                &from,
                json!({"route": "sidecar", "section": section, "turn_id": "t1",
                       "sidecar_verify": rule}),
            );
            assert_eq!(targets(&d), vec![curator.clone()], "{rel} {section}");
            assert_eq!(d[0].1["route"], "in_section");
        }
        // Not stamped, or stamped empty: the way it always went.
        for hop in [
            json!({"route": "sidecar", "section": "needs", "turn_id": "t1"}),
            json!({"route": "sidecar", "section": "needs", "turn_id": "t1",
                   "sidecar_verify": ""}),
        ] {
            assert_eq!(
                targets(&deliveries(&t, &from, hop)),
                vec![base.to_string()],
                "{rel}"
            );
        }
    }
    // The talky's curator hands its refusals to its error drain, which leaves on
    // `error`: no new lane, nothing for a parent to rewire.
    let t = table("/t", "templates/talky/config.json");
    let d = deliveries(
        &t,
        "/t/curator",
        json!({"route": "reject", "reject_reason": "quote_not_in_source"}),
    );
    assert_eq!(targets(&d), vec!["/t/errors"]);
    assert_eq!(
        targets(&deliveries(&t, "/t/errors", json!({"route": "error"}))),
        vec!["/t"]
    );
    // A checked section leaves the curator on the talky's own sidecar port.
    assert_eq!(
        targets(&deliveries(
            &t,
            "/t/curator",
            json!({"route": "sidecar", "section": "needs", "turn_id": "t1"})
        )),
        vec!["/t"]
    );
}

// ------------------------------------------------------------ 3. a refusal is a count, not a copy

/// The section `name` on `in_section` as the talky's edge hands it: the
/// splitter's hop with the lane set, the call's context with the person's
/// episode of the turn.
fn hand_in(h: &mut Hive, call: &Msg, split_msg: &Value) {
    let mut ctx = call.context.clone();
    for k in ["curator_call", "turn_id", "session_id", "iter"] {
        ctx.insert(k.into(), call.hop[k].clone());
    }
    ctx.insert(
        "episode_turn_id".into(),
        call.hop
            .get("episode_turn_id")
            .cloned()
            .unwrap_or_else(|| json!("")),
    );
    let mut hop = split_msg["header"].clone();
    hop.as_object_mut().expect("hop").remove("route");
    let mut body = split_msg.clone();
    body.as_object_mut().expect("body").remove("header");
    h.out.clear();
    h.lane("in_section", Value::Object(ctx), hop, body);
}

fn strings_of(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| strings_of(x, out)),
        Value::Object(o) => o.values().for_each(|x| strings_of(x, out)),
        _ => {}
    }
}

#[test]
fn sidecar_verify_reject_carries_no_text() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let call = h.curate(
        "s1",
        "t1",
        0,
        json!([user("Please call the clinic for me.")]),
        mode("Be brief."),
    );
    let items = json!({"claims": [
        {"claim": "a purple elephant lives upstairs", "quote": "purple elephant on the moon",
         "said": "booked a flight to Lisbon"},
        {"claim": "no quote at all, only a claim about the neighbour"}
    ]});
    let block = json!({"claims": items}).to_string();
    let out = split(
        "stop",
        &answer_with("I will call the clinic tomorrow.", &block),
        Some(json!({"claims": {"quote": "turn", "said": "answer"}})),
    );
    hand_in(&mut h, &call, section_of(&out, "claims"));
    let rejects = h.routed("reject");
    assert_eq!(rejects.len(), 2, "{:?}", h.out);
    let mut words = Vec::new();
    strings_of(&items, &mut words);
    for r in &rejects {
        assert_eq!(r.body, obj(json!({"messages": []})), "{r:?}");
        for k in [
            "reject_reason",
            "section",
            "field",
            "session_id",
            "turn_id",
            "episode_turn_id",
        ] {
            assert!(r.hop.contains_key(k), "{k}: {r:?}");
        }
        assert_eq!(r.hop["section"], "claims");
        assert_eq!(r.hop["session_id"], "s1");
        let all = json!({"hop": r.hop, "body": r.body, "context": r.context}).to_string();
        for w in &words {
            assert!(!all.contains(w.as_str()), "`{w}` left in a refusal: {all}");
        }
        // Through the talky's error drain: one report, and still no words.
        let report = meclaw_testing::emit_all(
            &meclaw_testing::shipped_script(&abs(ERRORS)),
            &json!({"header": {"hop": r.hop, "context": r.context}, "messages": []}),
        );
        assert_eq!(report.len(), 1, "{report:?}");
        let head = &report[0]["header"];
        assert_eq!(head["route"], "error");
        assert_eq!(head["error_source"], "sidecar_verify");
        assert_eq!(head["error_code"], r.hop["reject_reason"]);
        assert_eq!(head["reject_reason"], r.hop["reject_reason"]);
        assert_eq!(head["section"], "claims");
        assert_eq!(head["field"], r.hop["field"]);
        assert_eq!(head["episode_turn_id"], r.hop["episode_turn_id"]);
        let text = report[0]["messages"][0]["text"].as_str().expect("text");
        assert!(
            text.starts_with("talky reject: source=sidecar_verify reason="),
            "{text}"
        );
        let all = report[0].to_string();
        for w in &words {
            assert!(!all.contains(w.as_str()), "`{w}` left in a report: {all}");
        }
    }
    assert_eq!(
        reasons_of(&rejects),
        vec![("quote", "quote_not_in_source"), ("quote", "quote_missing")]
    );
    // The keeper's refusal through the same drain -- it names a `reject_reason`
    // of its own: byte for byte as before.
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(&abs(ERRORS)),
        &meclaw_testing::code_stdin(&json!({
            "header": {"hop": {"route": "reject", "reject_reason": "store_refused",
                               "store_error": "query_timeout",
                               "session_id": "s", "turn_id": "t"}, "context": {}},
            "messages": []
        }))
        .to_string(),
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        r#"[{"header": {"route": "error", "error_code": "query_timeout", "error_source": "session-keeper", "session_id": "s", "turn_id": "t"}, "messages": [{"origin": "assistant", "type": "text", "text": "talky error: source=session-keeper code=query_timeout session=s turn=t"}]}]"#
    );
}

fn reasons_of(rejects: &[Msg]) -> Vec<(&str, &str)> {
    rejects
        .iter()
        .map(|m| {
            (
                m.hop["field"].as_str().unwrap_or(""),
                m.hop["reject_reason"].as_str().unwrap_or(""),
            )
        })
        .collect()
}

// ------------------------------------------------------------ 4. the mini proof

/// Three rounds of one session, the third says "I miss Werner's patient ways";
/// the model's answer to it carries a `needs` section with two quotes, one from
/// the third round and one from the first. Only the third round's counts.
#[test]
fn sidecar_verify_mini_three_turns() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    turn(
        &mut h,
        "s1",
        "r1",
        "I like being alone on Sunday mornings.",
        "That sounds peaceful.",
        json!({}),
    );
    turn(
        &mut h,
        "s1",
        "r2",
        "The garden needs water again.",
        "I will remind you tonight.",
        json!({}),
    );
    let call = h.curate(
        "s1",
        "r3",
        0,
        json!([user("I miss Werner's patient ways.")]),
        mode("Be brief."),
    );
    let block = r#"{"needs": {"needs": [{"need": "connection", "quote": "miss Werner's patient ways"}, {"need": "solitude", "quote": "I like being alone"}]}}"#;
    let out = split(
        "stop",
        &answer_with("He sounds like he was a kind man.", block),
        Some(json!({"needs": {"quote": "turn"}})),
    );
    let needs = section_of(&out, "needs");
    // The talky's own edge takes it to the curator.
    let t = table("/t", "templates/talky/config.json");
    assert_eq!(
        targets(&deliveries(&t, "/t/splitter", needs["header"].clone())),
        vec!["/t/curator"]
    );
    hand_in(&mut h, &call, needs);
    let sides = h.routed("sidecar");
    assert_eq!(sides.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(
        sides[0].body["payload"],
        json!({"needs": [{"need": "connection", "quote": "miss Werner's patient ways",
                          "verified": ["quote"]}]})
    );
    assert_eq!(sides[0].hop["turn_id"], needs["header"]["turn_id"]);
    let rejects = h.routed("reject");
    assert_eq!(
        reasons_of(&rejects),
        vec![("quote", "quote_not_in_source")],
        "the first round's words do not count in the third"
    );
    assert_eq!(rejects[0].hop["section"], "needs");
}
