//! GH #843 — a completion cut on `length` leaves talky without its sidecar, and
//! every answer says how it ended.
//!
//! Measured before the fix (`talky@5.3.0`, `collector@4.3.0`):
//!
//! - `./brain -> ./collector` on `finish_reason == 'length'` went PAST the
//!   splitter, so the raw model text — sidecar block included — left on
//!   `answer`. A consumer forwarding answers across a trust boundary forwarded
//!   the member's private memory annotations with them.
//! - the splitter hard-coded `{"finish_reason": "stop"}` on the answer it cut
//!   (script line 209), and `collector/assemble` rebuilt the hop in `head()`
//!   without the reason: a truncated answer looked complete.
//!
//! The ruling (OR-SN-6): ONE grammar cuts the sidecar — the splitter's — so
//! `length` goes through it (`./brain -> ./splitter`, then
//! `./splitter -> ./collector` as `in_answer`; never into the dispatcher), the
//! splitter passes the reason through, and the collector stamps
//! `finish_reason` on EVERY answer (`""` where it does not know one) plus
//! `truncated "1"` on a cut one. That covers cogny as well, whose `length` edge
//! still goes straight to its collector because cogny has no splitter.
//!
//! Measured at the seams: the router is asked where a message goes, and the
//! shipped scripts run over stdin.

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::{ASSEMBLE, lane, on_route, run_cell};
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Headers, Path, Uuid};

const SPLITTER: &str = "templates/talky/splitter/config.json";
const TALKY: &str = "/t";
const COGNY: &str = "/c";

const BLOCK: &str = "```sidecar\n{\"memory\": {\"facts\": [{\"subject\": \"alex\", \
     \"predicate\": \"likes\", \"object\": \"tea\"}]}}\n```";

fn shipped() -> bool {
    [
        "templates/talky/config.json",
        "templates/cogny/config.json",
        SPLITTER,
        ASSEMBLE,
    ]
    .iter()
    .all(|rel| assemble_cell::repo(rel).is_file())
}

fn table(base: &str, rel: &str) -> EdgeTable {
    let params = assemble_cell::config_of(rel)["params"].clone();
    let hp: HiveParams = meclaw_core::serde_json::from_value(params)
        .unwrap_or_else(|e| panic!("{rel}: params: {e}"));
    let mut t = EdgeTable::new();
    for spec in &hp.graph.edges {
        let spec: &EdgeSpec = spec;
        let abs = |ep: &str| match ep {
            "." => base.to_string(),
            other => format!("{base}/{}", other.trim_start_matches("./")),
        };
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(&spec.from)),
            to: Path::new(&abs(&spec.to)),
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
        });
    }
    t
}

fn map(v: Value) -> Map<String, Value> {
    v.as_object().cloned().expect("object")
}

fn round_ctx() -> Value {
    json!({"session_id": "s1", "turn_id": "round-843", "iter": "1"})
}

/// Where one message goes from `from`: `(target, hop out)` per delivery.
fn deliveries(t: &EdgeTable, from: &str, hop: Value) -> Vec<(String, Map<String, Value>)> {
    let hs = Headers::from_parts(map(round_ctx()), map(hop));
    apply_edges(t, &Path::new(from), &hs)
        .into_iter()
        .map(|d| (d.target.as_str().to_string(), d.headers_out.hop.clone()))
        .collect()
}

/// A cut completion as the llm cell hands it on: the reason on the hop, the
/// text — ending in a sidecar the model began — in the body.
fn cut_completion() -> Value {
    json!({
        "header": {"context": round_ctx(),
                   "hop": {"finish_reason": "length", "tokens_completion": 512}},
        "messages": [{"origin": "assistant", "type": "text",
                      "text": format!("Tea it is, noted.\n\n{BLOCK}")}]
    })
}

fn answer_of(out: &[Value]) -> &Value {
    on_route(out, "answer")
}

fn in_answer(hop: Map<String, Value>, text: &str) -> Vec<Value> {
    let mut hop = hop;
    hop.remove("route");
    run_cell(
        ASSEMBLE,
        &[],
        lane(
            "in_answer",
            Value::Object(hop),
            json!({"turn_id": "round-843", "iter": "1"}),
            json!([{"origin": "assistant", "type": "text", "text": text}]),
        ),
    )
    .0
}

#[test]
fn a_length_finish_goes_through_the_splitter_and_never_into_the_dispatcher() {
    if !shipped() {
        return;
    }
    let t = table(TALKY, "templates/talky/config.json");
    let from_brain = deliveries(&t, "/t/brain", json!({"finish_reason": "length"}));
    let targets: Vec<&str> = from_brain.iter().map(|(to, _)| to.as_str()).collect();
    assert_eq!(
        targets,
        vec!["/t/splitter"],
        "a cut completion has one road out of the brain: the splitter, which is the one \
         grammar that cuts the sidecar (OR-SN-6)"
    );

    let from_splitter = deliveries(&t, "/t/splitter", json!({"finish_reason": "length"}));
    let targets: Vec<&str> = from_splitter.iter().map(|(to, _)| to.as_str()).collect();
    assert_eq!(
        targets,
        vec!["/t/collector"],
        "and out of the splitter straight to the collector — `length` must never reach the \
         dispatcher, whose answer path it is not"
    );
    assert_eq!(from_splitter[0].1["route"], "in_answer");

    // The normal roads are untouched.
    for finish in ["stop", "tool_calls"] {
        let d = deliveries(&t, "/t/splitter", json!({"finish_reason": finish}));
        let targets: Vec<&str> = d.iter().map(|(to, _)| to.as_str()).collect();
        assert_eq!(
            targets,
            vec!["/t/dispatcher"],
            "`{finish}` still dispatches"
        );
    }
    let d = deliveries(&t, "/t/brain", json!({"finish_reason": "content_filter"}));
    let targets: Vec<&str> = d.iter().map(|(to, _)| to.as_str()).collect();
    assert_eq!(targets, vec!["/t/errors"], "content_filter stays an error");
}

#[test]
fn the_cut_answer_leaves_talky_without_its_sidecar_and_names_its_reason() {
    if !shipped() {
        return;
    }
    let (out, _) = run_cell(SPLITTER, &[], cut_completion());
    let answer = out
        .iter()
        .find(|m| m["header"].get("route").is_none())
        .unwrap_or_else(|| panic!("the splitter cut no answer: {out:?}"));
    let text = answer["messages"][0]["text"].as_str().expect("text");
    assert!(
        !text.contains("```") && !text.contains("facts"),
        "the sidecar is cut on the length path too: {text:?}"
    );
    assert_eq!(
        answer["header"]["finish_reason"], "length",
        "the splitter passes the reason through instead of claiming `stop`"
    );
    assert!(
        out.iter().any(|m| m["header"]["route"] == "sidecar"),
        "the block still leaves on its own lane: {out:?}"
    );

    // Splitter -> collector, as the tree routes it, then the shipped assembler.
    let t = table(TALKY, "templates/talky/config.json");
    let d = deliveries(&t, "/t/splitter", answer["header"].clone());
    assert_eq!(d.len(), 1, "one road for the answer: {d:?}");
    assert_eq!(d[0].0, "/t/collector");
    // And the block's own emission never rides the new length edge into the
    // collector as a second `in_answer`: its header carries route/section and
    // no finish_reason (review of T1, finding 4).
    let sidecar = out
        .iter()
        .find(|m| m["header"]["route"] == "sidecar")
        .expect("the sidecar emission");
    let s = deliveries(&t, "/t/splitter", sidecar["header"].clone());
    assert!(!s.is_empty(), "the sidecar has a road of its own: {s:?}");
    assert!(
        s.iter().all(|(to, _)| to != "/t/collector"),
        "the sidecar must not reach the collector: {s:?}"
    );
    let emitted = in_answer(d[0].1.clone(), text);
    let a = answer_of(&emitted);
    assert_eq!(a["header"]["finish_reason"], "length");
    assert_eq!(
        a["header"]["truncated"], "1",
        "a cut answer says so, so a consumer can refuse or mark it"
    );
    assert!(
        !a["messages"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("```"),
        "no sidecar on the answer that leaves"
    );
}

#[test]
fn a_normal_answer_names_stop_and_is_not_truncated() {
    if !shipped() {
        return;
    }
    let mut hop = Map::new();
    hop.insert("finish_reason".into(), json!("stop"));
    let emitted = in_answer(hop, "Tea it is.");
    let a = answer_of(&emitted);
    assert_eq!(a["header"]["finish_reason"], "stop");
    assert_eq!(
        a["header"]["truncated"], "",
        "present and empty, never absent: a missing hop key fails a CEL modifier"
    );
}

#[test]
fn an_answer_that_knows_no_reason_carries_it_empty() {
    if !shipped() {
        return;
    }
    // The interim sentence the dispatcher lets out beside a bundle carries no
    // finish reason of its own.
    let mut hop = Map::new();
    hop.insert("interim".into(), json!("1"));
    let emitted = in_answer(hop, "One moment.");
    let a = answer_of(&emitted);
    assert_eq!(a["header"]["finish_reason"], "");
    assert_eq!(a["header"]["truncated"], "");

    // A store refusal is reported on `answer` too, and knows no reason either.
    let refused = run_cell(
        ASSEMBLE,
        &[],
        lane(
            "",
            json!({"operation": "select", "error_code": "store_busy"}),
            json!({"turn_id": "round-843", "col_phase": "turn-open"}),
            json!([]),
        ),
    )
    .0;
    let a = answer_of(&refused);
    assert_eq!(a["header"]["degraded"], "1", "the premise: a store report");
    assert_eq!(a["header"]["finish_reason"], "");
    assert_eq!(a["header"]["truncated"], "");
}

#[test]
fn the_cogny_length_edge_is_marked_as_well() {
    if !shipped() {
        return;
    }
    let t = table(COGNY, "templates/cogny/config.json");
    let d = deliveries(&t, "/c/brain", json!({"finish_reason": "length"}));
    let targets: Vec<&str> = d.iter().map(|(to, _)| to.as_str()).collect();
    assert_eq!(
        targets,
        vec!["/c/collector"],
        "cogny has no splitter; its length edge stays"
    );
    assert_eq!(d[0].1["route"], "in_answer");
    let emitted = in_answer(d[0].1.clone(), "The analysis so far");
    let a = answer_of(&emitted);
    assert_eq!(a["header"]["finish_reason"], "length");
    assert_eq!(a["header"]["truncated"], "1");
}
