//! GH #1255 -- the sent answer reaches the checker the talky declares.
//!
//! At low reasoning effort the talky's model writes no `claims` section, so a
//! conscience that checks a sent answer against its sources got no input at
//! all: measured in the soul2me run T3-03 (17.10.2026), 12 of 67 spoken turns
//! were unfaithful and 8 of them were free answers with no claim listed. The
//! splitter holds the answer text; its knob `answer_check` names the sections
//! whose checker gets it on EVERY answer, written section or not. One message
//! per named section leaves on the talky's own lane `answer_check`, AFTER the
//! answer half of the same split -- the answer path never waits for it -- and
//! nothing else carries the text out of the composite.
//!
//! What is pinned here: an empty knob is byte for byte the splitter before it;
//! an answer without `claims` (no block, a block without the section, an
//! unreadable block, a cut completion) still hands its text to the checker,
//! after the answer; the composite routes the lane out of its rim and nowhere
//! else, and a member's edge on it reaches the checker it binds; no section
//! leaving on `sidecar`, and no undeclared section, carries the answer text; and
//! one level up, the `assistant` carries the lane out of its own rim from both
//! surfaces, so a member binds the checker at the assistant's path.

use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{self as sj, Value, json};
use meclaw_core::{Headers, Path, Uuid};
use std::path::PathBuf;

const SPLITTER: &str = "templates/talky/splitter/config.json";
const TALKY: &str = "templates/talky/config.json";
const ASSISTANT: &str = "templates/assistant/config.json";

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

/// R2b / GH #49: a tree without the template skips.
fn shipped() -> bool {
    repo(SPLITTER).is_file()
}

fn read_json(rel: &str) -> Value {
    let p = repo(rel);
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    sj::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn said(text: &str) -> Value {
    json!({"origin": "assistant", "type": "text", "text": text})
}

fn tool_call(id: &str, name: &str) -> Value {
    json!({"origin": "assistant", "type": "tool_call", "id": id,
           "text": json!({"name": name, "arguments": "{}"}).to_string()})
}

/// The splitter's params as shipped, `answer_check` set to `knob` or left out.
fn params(knob: Option<Value>) -> Value {
    let mut p = read_json(SPLITTER)["params"].clone();
    let o = p.as_object_mut().expect("params");
    o.remove("script_inline");
    match knob {
        Some(k) => {
            o.insert("answer_check".into(), k);
        }
        None => {
            o.remove("answer_check");
        }
    }
    p
}

/// The shipped splitter's stdout, byte for byte, for one brain completion.
fn split_raw(finish: &str, turns: &Value, knob: Option<Value>) -> String {
    let doc = json!({
        "header": {"hop": {"finish_reason": finish},
                   "context": {"session_id": "s-1255", "turn_id": "t-1255"}},
        "messages": turns,
        "params": params(knob),
    });
    let out = meclaw_testing::run_shipped_script(
        &meclaw_testing::shipped_script(&repo(SPLITTER).to_string_lossy()),
        &meclaw_testing::code_stdin(&doc).to_string(),
    );
    assert!(
        out.status.success(),
        "the splitter exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf-8")
}

/// The same, parsed: every message the splitter emitted, in emission order.
fn split(finish: &str, turns: &Value, knob: Option<Value>) -> Vec<Value> {
    match sj::from_str::<Value>(&split_raw(finish, turns, knob)).expect("json") {
        Value::Array(a) => a,
        one => vec![one],
    }
}

fn answer_with(prose: &str, block: &str) -> Value {
    json!([said(&format!("{prose}\n\n```sidecar\n{block}\n```"))])
}

/// The messages on route `answer_check`, with their position in the split.
fn checks(out: &[Value]) -> Vec<(usize, &Value)> {
    out.iter()
        .enumerate()
        .filter(|(_, m)| m["header"]["route"] == json!("answer_check"))
        .collect()
}

/// The code cell validates `contract.emits` always-on: every hop key and body
/// slot a message carries has to be declared, with its value among `values`.
fn assert_declared(m: &Value) {
    let emits = &read_json(SPLITTER)["contract"]["emits"];
    for (k, v) in m["header"].as_object().expect("header") {
        let decl = &emits["hop"][k];
        assert!(decl.is_object(), "hop.{k} is not declared: {m}");
        if let Some(values) = decl["values"].as_array() {
            assert!(values.contains(v), "hop.{k} = {v} is not declared: {m}");
        }
    }
    for k in m.as_object().expect("message").keys() {
        if k != "header" {
            assert!(
                emits["body"][k].is_object(),
                "body.{k} is not declared: {m}"
            );
        }
    }
}

fn table(base: &str, rel: &str) -> EdgeTable {
    let hp: HiveParams =
        sj::from_value(read_json(rel)["params"].clone()).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let mut t = EdgeTable::new();
    for spec in &hp.graph.edges {
        let spec: &EdgeSpec = spec;
        let at = |ep: &str| match ep {
            "." => base.to_string(),
            other => format!("{base}/{}", other.trim_start_matches("./")),
        };
        let mut e = edge(&at(&spec.from), &at(&spec.to), spec.condition.as_deref());
        e.modifier = spec.modifier.as_ref().map(|m| {
            meclaw_colony::cel_eval::parse_modifier(m)
                .unwrap_or_else(|(k, e)| panic!("{rel}: modifier {k}: {e}"))
        });
        e.is_default = spec.is_default;
        e.lane = spec.lane.clone();
        e.tap = spec.tap;
        t.insert(e);
    }
    t
}

fn edge(from: &str, to: &str, condition: Option<&str>) -> Edge {
    Edge {
        id: Uuid::now_v7(),
        from: Path::new(from),
        to: Path::new(to),
        condition: condition.map(|src| {
            meclaw_colony::cel_eval::parse_condition(src)
                .unwrap_or_else(|e| panic!("condition {src:?}: {e}"))
        }),
        modifier: None,
        is_default: false,
        lane: None,
        tap: false,
    }
}

fn targets(t: &EdgeTable, from: &str, hop: &Value) -> Vec<String> {
    let hs = Headers::from_parts(
        json!({"session_id": "s-1255", "turn_id": "t-1255"})
            .as_object()
            .cloned()
            .expect("context"),
        hop.as_object().cloned().expect("hop"),
    );
    apply_edges(t, &Path::new(from), &hs)
        .into_iter()
        .map(|d| d.target.as_str().to_string())
        .collect()
}

const PROSE: &str = "You have the dentist appointment tomorrow at ten, that works well.";
const MEMORY: &str =
    r#"{"memory": {"nothing_new": true, "facts": [], "topic": {"movement": "continue"}}}"#;

// ------------------------------------------------------------ 1. empty is the old behaviour

#[test]
fn answer_check_empty_knob_is_byte_identical() {
    if !shipped() {
        return;
    }
    let cfg = read_json(SPLITTER);
    assert_eq!(
        cfg["params"]["answer_check"],
        json!([]),
        "the knob ships empty"
    );
    assert_eq!(
        cfg["contract"]["settings"]["answer_check"]["default"],
        json!([]),
        "the contract names the shipped default"
    );
    let fixtures: Vec<(&str, Value)> = vec![
        ("stop", answer_with(PROSE, MEMORY)),
        ("stop", json!([said(PROSE)])),
        (
            "stop",
            json!([said(&format!(
                "{PROSE}\n\n```sidecar\n{{\"claims\": [\n```"
            ))]),
        ),
        ("length", answer_with(PROSE, MEMORY)),
        (
            "tool_calls",
            json!([said("Moment."), tool_call("c1", "web_search")]),
        ),
    ];
    for (finish, turns) in &fixtures {
        let without = split_raw(finish, turns, None);
        for knob in [json!([]), json!(""), json!(" , ")] {
            assert_eq!(
                split_raw(finish, turns, Some(knob.clone())),
                without,
                "{finish}: the knob {knob} is no knob"
            );
        }
        assert!(!without.contains("answer_check"), "{without}");
    }
}

// ------------------------------------------------------------ 2. no claims, still checked

#[test]
fn answer_check_hands_the_answer_to_its_checker_after_the_answer() {
    if !shipped() {
        return;
    }
    let knob = Some(json!(["claims"]));
    let t = table("/m/talky", TALKY);
    // An answer without `claims`, in every shape a low-effort model writes it.
    let fixtures: Vec<(&str, Value, &str)> = vec![
        ("stop", json!([said(PROSE)]), "no block at all"),
        (
            "stop",
            answer_with(PROSE, MEMORY),
            "a block without the section",
        ),
        (
            "stop",
            json!([said(&format!(
                "{PROSE}\n\n```sidecar\n{{\"claims\": [\n```"
            ))]),
            "a block that does not read",
        ),
        (
            "stop",
            answer_with(PROSE, r#"{"claims": "", "memory": {"facts": []}}"#),
            "a section with no words",
        ),
        ("length", answer_with(PROSE, MEMORY), "a cut completion"),
    ];
    for (finish, turns, what) in &fixtures {
        let plain = split(finish, turns, None);
        let out = split(finish, turns, knob.clone());
        assert_eq!(
            out[0], plain[0],
            "{what}: the answer half is the same with the knob as without it"
        );
        assert_eq!(
            &out[..plain.len()],
            &plain[..],
            "{what}: everything the split carried before leaves first and unchanged"
        );
        let c = checks(&out);
        assert_eq!(c.len(), 1, "{what}: one check per named section: {out:#?}");
        let (at, m) = c[0];
        assert!(at > 0, "{what}: the check leaves after the answer half");
        assert_eq!(
            m["header"],
            json!({"route": "answer_check", "section": "claims", "turn_id": "t-1255",
                   "section_written": "0"}),
            "{what}"
        );
        assert_eq!(
            *m,
            json!({"header": m["header"], "messages": [], "section": "claims",
                   "verify_answer": PROSE}),
            "{what}: the answer text as its reader got it, and nothing else"
        );
        assert_declared(m);
        // The answer goes its way alone; the check leaves the rim and only it.
        let answer_to = targets(&t, "/m/talky/splitter", &out[0]["header"]);
        assert!(
            !answer_to.is_empty() && !answer_to.contains(&"/m/talky".to_string()),
            "{what}: {answer_to:?}"
        );
        assert_eq!(
            targets(&t, "/m/talky/splitter", &m["header"]),
            vec!["/m/talky"],
            "{what}: `answer_check` leaves the composite on its own lane"
        );
    }
    // A section the model did write: it travels on `sidecar` as before, and the
    // check says so.
    let out = split(
        "stop",
        &answer_with(PROSE, r#"{"claims": {"claims": []}}"#),
        knob.clone(),
    );
    assert_eq!(
        out.iter()
            .filter(|m| m["header"]["route"] == json!("sidecar"))
            .count(),
        1,
        "{out:#?}"
    );
    assert_eq!(checks(&out)[0].1["header"]["section_written"], json!("1"));
    // A tool round is no answer: nothing is checked.
    let tool = json!([said("Moment."), tool_call("c1", "web_search")]);
    assert_eq!(
        split_raw("tool_calls", &tool, knob.clone()),
        split_raw("tool_calls", &tool, None)
    );
    // An answer that was nothing but its block said nothing to check.
    assert!(checks(&split("stop", &answer_with("", MEMORY), knob)).is_empty());
    // The member binds the checker on the lane: its edge reaches it.
    let mut member = EdgeTable::new();
    member.insert(edge(
        "/m/talky",
        "/m/conscience",
        Some("has(hop.route) && hop.route == 'answer_check' && hop.section == 'claims'"),
    ));
    member.insert(edge(
        "/m/talky",
        "/m/app",
        Some("has(hop.route) && hop.route == 'sidecar'"),
    ));
    let out = split("stop", &json!([said(PROSE)]), Some(json!(["claims"])));
    assert_eq!(
        targets(&member, "/m/talky", &checks(&out)[0].1["header"]),
        vec!["/m/conscience"]
    );
}

// ------------------------------------------------------------ 3. nobody else gets the text

#[test]
fn answer_check_no_undeclared_receiver_gets_the_answer_text() {
    if !shipped() {
        return;
    }
    let t = table("/m/talky", TALKY);
    let turns = answer_with(
        PROSE,
        r#"{"fact": "The appointment is tomorrow.", "claims": {"claims": [{"said": "ten o'clock"}]}, "memory": {"facts": []}}"#,
    );
    for knob in [None, Some(json!(["claims"])), Some(json!("claims, needs"))] {
        let out = split("stop", &turns, knob.clone());
        for m in out.iter().skip(1) {
            if m["header"]["route"] == json!("answer_check") {
                continue;
            }
            let to = targets(&t, "/m/talky/splitter", &m["header"]);
            let leaves = to.contains(&"/m/talky".to_string());
            let carries = m.to_string().contains("dentist appointment");
            assert!(
                !(leaves && carries),
                "{knob:?}: a message leaving the rim on {} carries the answer text: {m}",
                m["header"]["route"]
            );
        }
        let named: Vec<&str> = checks(&out)
            .iter()
            .map(|(_, m)| m["section"].as_str().unwrap_or_default())
            .collect();
        let want: Vec<&str> = match &knob {
            None => vec![],
            Some(k) if k == &json!(["claims"]) => vec!["claims"],
            Some(_) => vec!["claims", "needs"],
        };
        assert_eq!(
            named, want,
            "{knob:?}: only a declared section gets a check, and the `fact` beside it none"
        );
    }
}

// ------------------------------------------------------------ 4. the assistant carries it on

/// The edge of `templates/assistant/config.json` out of `from` that carries `route` to the
/// level's own rim, as shipped.
fn rim_edge(cfg: &Value, from: &str, route: &str) -> Option<Value> {
    let want = format!("has(hop.route) && hop.route == '{route}'");
    cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .iter()
        .find(|e| {
            e["from"] == json!(from) && e["to"] == json!(".") && e["condition"] == json!(want)
        })
        .cloned()
}

/// The member that uses the knob instantiates the talky inside `assistant@…`
/// (re-check T3B, I1: soul2me's assistant node overrides `talky/splitter`), so the
/// checker's edge stands at the ASSISTANT's path. Without a pass-through at that
/// rim the check died inside the level as `no_route`, and the conscience still got
/// nothing. Both surfaces carry it, on the same terms as their `sidecar`; without
/// the knob there is no message to carry.
#[test]
fn answer_check_leaves_the_assistant_rim_from_both_surfaces() {
    if !shipped() || !repo(ASSISTANT).is_file() {
        return;
    }
    let cfg = read_json(ASSISTANT);
    let lane = cfg["params"]["contract"]["emits"]
        .as_array()
        .expect("emits")
        .iter()
        .find(|l| l["route"] == json!("answer_check"))
        .cloned()
        .expect("the assistant declares `answer_check` among its exits");
    assert!(
        lane.get("at").is_none(),
        "a lane of the assistant's own rim, not a connect point: {lane}"
    );
    // Without the knob nothing travels, so the rim carries nothing.
    assert!(checks(&split("stop", &json!([said(PROSE)]), None)).is_empty());
    let out = split("stop", &json!([said(PROSE)]), Some(json!(["claims"])));
    let check = checks(&out)[0].1["header"].clone();
    let t = table("/m/assistant", ASSISTANT);
    for surface in ["./talky", "./talky-chat"] {
        let from = format!("/m/assistant/{}", surface.trim_start_matches("./"));
        assert_eq!(
            targets(&t, &from, &check),
            vec!["/m/assistant"],
            "{surface}: `answer_check` leaves the assistant's rim, and goes nowhere else"
        );
        let pass = rim_edge(&cfg, surface, "answer_check").expect("the pass-through edge");
        let side = rim_edge(&cfg, surface, "sidecar").expect("the sidecar pass-through");
        assert_eq!(
            pass["modifier"], side["modifier"],
            "{surface}: the check crosses the rim on the same terms as the sections"
        );
    }
    // The core's splitter copy raises none, and the level draws nothing for it.
    assert!(rim_edge(&cfg, "./cogny", "answer_check").is_none());
    // The member binds the checker at the assistant's path, and only it gets the text.
    let mut member = EdgeTable::new();
    member.insert(edge(
        "/m/assistant",
        "/m/conscience",
        Some("has(hop.route) && hop.route == 'answer_check' && hop.section == 'claims'"),
    ));
    member.insert(edge(
        "/m/assistant",
        "/m/apps",
        Some("has(hop.route) && hop.route == 'sidecar'"),
    ));
    assert_eq!(
        targets(&member, "/m/assistant", &check),
        vec!["/m/conscience"]
    );
}
