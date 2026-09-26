//! GH #841 — a model-initiated `memory_recall` round comes back under its turn.
//!
//! The road, every edge of it shipped:
//!
//! ```text
//! <gen>/talky/dispatcher --tool memory_recall--> <gen>/talky (rim, `./dispatcher -> .`)
//!   --> <gen> (assistant `./talky -> .`, tool_caller talky)
//!   --> assistants (recipe `./scribe -> .`, context.assistant)
//!   --> member door `./assistants -> ./memory-hive` (tool_call, stamps the round)
//!   --> memory-hive/tool ... tool_result
//!   --> member `./memory-hive -> ./assistants` (in_tool) --> <gen> --> <gen>/talky
//!   --> <gen>/talky/collector/assemble: `in_tool`
//! ```
//!
//! Measured before the fix (`member@1.10.0`): the door stamped
//! `"turn_id": "has(hop.turn_id) ? hop.turn_id : ''"`, and the dispatcher's `tool`
//! emission never carries `turn_id` on its hop (it reads nothing but the hop,
//! and stays that way — OR-SN-5). So the hive was asked under `turn_id = ''`,
//! answered under it, and `assemble` parked the result in silence: the round's
//! `assistant` row stayed open and every later turn of the session was deferred
//! behind it. The context of the round DOES carry the key — `collector -> brain`
//! promotes `hop.turn_id` into `context.turn_id` — so the door falls back to it.
//!
//! What this file proves, at the receiver:
//!
//! 1. the door's stamp at the hive's `tool` cell is the round's id, taken off the
//!    context when the hop carries none (and the hop still wins when it does);
//! 2. the result walks home to `assemble` under that id and the round fires
//!    (`round-check`);
//! 3. a result that still arrives without a turn id is parked AND says so on
//!    stderr (it used to be the one silent refusal of the cell).
//!
//! No colony, no model: the templates and the recipe are read off the tree, the
//! router is asked what it would do, and the shipped scripts run over stdin.

#[path = "support/assemble_cell.rs"]
mod assemble_cell;

use assemble_cell::{DISPATCHER, in_phase, run_cell};
use meclaw_colony::config::{EdgeSpec, HiveParams};
use meclaw_colony::edge_table::{Edge, EdgeTable, apply_edges};
use meclaw_core::serde_json::{Map, Value, json};
use meclaw_core::{Headers, Path, Uuid};

const MEMBER: &str = "/m";
const BOX: &str = "/m/assistants";
const GEN: &str = "/m/assistants/scribe";
const HIVE: &str = "/m/memory-hive";

/// The round's key as `collector -> brain` promotes it (a uuid, not the
/// deterministic `<session>#<n>` of the hop — the two are different keys).
const ROUND: &str = "0199aa00-0000-7000-8000-000000000841";

fn repo(rel: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel)
}

fn read_json(rel: &str) -> Value {
    let p = repo(rel);
    let raw = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Every template this file reads, or `false` in a tree that does not carry them
/// (guarded like every template-reading test, GH #49).
fn shipped() -> bool {
    [
        "templates/member/config.json",
        "templates/memory-hive/config.json",
        "templates/assistant/config.json",
        "templates/talky/config.json",
        "templates/collector/config.json",
        DISPATCHER,
        "examples/organism/grow-assistant.json",
    ]
    .iter()
    .all(|rel| repo(rel).is_file())
}

fn hive_edges(rel: &str) -> Vec<EdgeSpec> {
    let params = read_json(rel)["params"].clone();
    let hp: HiveParams = meclaw_core::serde_json::from_value(params)
        .unwrap_or_else(|e| panic!("{rel}: params: {e}"));
    hp.graph.edges
}

fn recipe_edges(rel: &str) -> Vec<EdgeSpec> {
    read_json(rel)["diff"]["add_edges"]
        .as_array()
        .unwrap_or_else(|| panic!("{rel}: no diff.add_edges"))
        .iter()
        .map(|e| {
            meclaw_core::serde_json::from_value(e.clone())
                .unwrap_or_else(|err| panic!("{rel}: add_edges entry {e}: {err}"))
        })
        .collect()
}

fn abs(base: &str, endpoint: &str) -> String {
    match endpoint {
        "." => base.to_string(),
        other => format!("{base}/{}", other.trim_start_matches("./")),
    }
}

fn add_edges(table: &mut EdgeTable, base: &str, specs: &[EdgeSpec], label: &str) {
    for spec in specs {
        let condition = spec.condition.as_ref().map(|src| {
            meclaw_colony::cel_eval::parse_condition(src)
                .unwrap_or_else(|e| panic!("{label}: condition {src:?}: {e}"))
        });
        let modifier = spec.modifier.as_ref().map(|m| {
            meclaw_colony::cel_eval::parse_modifier(m)
                .unwrap_or_else(|(k, e)| panic!("{label}: modifier {k}: {e}"))
        });
        table.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(base, &spec.from)),
            to: Path::new(&abs(base, &spec.to)),
            condition,
            modifier,
            is_default: spec.is_default,
            lane: spec.lane.clone(),
        });
    }
}

/// Every level the round trip crosses: the member, its hive, the container as
/// `examples/organism` wires it, one generation, its surface and the surface's
/// collector.
fn shipped_table() -> EdgeTable {
    let mut t = EdgeTable::new();
    add_edges(
        &mut t,
        MEMBER,
        &hive_edges("templates/member/config.json"),
        "member",
    );
    add_edges(
        &mut t,
        HIVE,
        &hive_edges("templates/memory-hive/config.json"),
        "memory-hive",
    );
    add_edges(
        &mut t,
        BOX,
        &recipe_edges("examples/organism/grow-assistant.json"),
        "grow-assistant",
    );
    add_edges(
        &mut t,
        GEN,
        &hive_edges("templates/assistant/config.json"),
        "assistant",
    );
    add_edges(
        &mut t,
        &format!("{GEN}/talky"),
        &hive_edges("templates/talky/config.json"),
        "talky",
    );
    add_edges(
        &mut t,
        &format!("{GEN}/talky/collector"),
        &hive_edges("templates/collector/config.json"),
        "collector",
    );
    t
}

fn as_map(v: &Value) -> Map<String, Value> {
    v.as_object().cloned().expect("a JSON object")
}

fn ctx_of(h: &Headers, key: &str) -> String {
    h.context
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .into()
}

fn hop_of(h: &Headers, key: &str) -> String {
    h.hop.get(key).and_then(|v| v.as_str()).unwrap_or("").into()
}

/// Follow one message until nothing takes it further, insisting on exactly one
/// addressee per step: a fan-out on this road is two recalls where the model
/// asked one.
fn walk(table: &EdgeTable, from: &str, headers: Headers) -> (Vec<String>, Headers) {
    let mut trace = vec![from.to_string()];
    let mut here = Path::new(from);
    let mut hs = headers;
    for _ in 0..24 {
        let out = apply_edges(table, &here, &hs);
        if out.is_empty() {
            return (trace, hs);
        }
        assert_eq!(
            out.len(),
            1,
            "at {} the message fans out to {:?}",
            here.as_str(),
            out.iter().map(|d| d.target.as_str()).collect::<Vec<_>>()
        );
        let d = out.into_iter().next().expect("checked non-empty");
        here = d.target;
        hs = d.headers_out;
        trace.push(here.as_str().to_string());
    }
    panic!("the walk did not settle in 24 hops: {trace:?}");
}

/// The context a round's dispatcher runs under: what `collector -> brain`
/// promoted (`turn_id`, `session_id`, `iter`) plus what the turn came in with.
fn round_context() -> Value {
    json!({
        "assistant": "scribe",
        "session_id": "S-841",
        "turn_id": ROUND,
        "iter": "1",
        "audience_set": "[\"alex\"]",
        "channel": "chat:1"
    })
}

/// The SHIPPED dispatcher's `tool` emission for one `memory_recall` call, run
/// over stdin — the hop the road starts with is the one the cell writes, not a
/// hand-typed copy of it.
fn dispatcher_tool_emission() -> Value {
    let call = json!({"origin": "assistant", "type": "tool_call", "id": "c-841",
                      "text": json!({"name": "memory_recall",
                                     "arguments": "{\"query\":\"what did we decide\"}"})
                          .to_string()});
    let doc = json!({
        "header": {"context": round_context(), "hop": {"finish_reason": "tool_calls"}},
        "messages": [call]
    });
    let (out, _) = run_cell(DISPATCHER, &[], doc);
    out.into_iter()
        .find(|m| m["header"]["route"] == "tool")
        .unwrap_or_else(|| panic!("the dispatcher emitted no `tool` for memory_recall"))
}

#[test]
fn the_door_stamps_the_rounds_id_off_the_context_when_the_hop_carries_none() {
    if !shipped() {
        return;
    }
    let emission = dispatcher_tool_emission();
    assert!(
        emission["header"].get("turn_id").is_none(),
        "the premise: the dispatcher's `tool` hop carries no turn (it reads the hop only, \
         OR-SN-5) — got {emission}"
    );
    let t = shipped_table();
    let start = Headers::from_parts(as_map(&round_context()), as_map(&emission["header"]));
    let (trace, arrived) = walk(&t, &format!("{GEN}/talky/dispatcher"), start);
    assert_eq!(
        trace.last().map(String::as_str),
        Some("/m/memory-hive/tool"),
        "the recall must reach the hive's tool cell through the member's door: {trace:?}"
    );
    assert_eq!(hop_of(&arrived, "route"), "tool_call");
    assert_eq!(
        ctx_of(&arrived, "turn_id"),
        ROUND,
        "the member door must stamp the round the call belongs to — the context carries it \
         when the hop does not; an empty stamp parks the result at the collector (GH #841)"
    );
}

#[test]
fn the_hop_still_wins_over_the_context_at_the_door() {
    if !shipped() {
        return;
    }
    let t = shipped_table();
    let mut hop = as_map(&dispatcher_tool_emission()["header"]);
    hop.insert("turn_id".into(), json!("S-841#3"));
    let (_, arrived) = walk(
        &t,
        &format!("{GEN}/talky/dispatcher"),
        Headers::from_parts(as_map(&round_context()), hop),
    );
    assert_eq!(
        ctx_of(&arrived, "turn_id"),
        "S-841#3",
        "a hop that names its turn stays authoritative (#535); the context is the fallback only"
    );
}

#[test]
fn the_result_comes_home_under_the_round_and_the_round_fires() {
    if !shipped() {
        return;
    }
    let t = shipped_table();
    let up = Headers::from_parts(
        as_map(&round_context()),
        as_map(&dispatcher_tool_emission()["header"]),
    );
    let (_, at_hive) = walk(&t, &format!("{GEN}/talky/dispatcher"), up);

    // The hive's tool cell answers under the call id, in the context it was asked in.
    let mut hop = Map::new();
    hop.insert("route".into(), json!("tool_result"));
    hop.insert("tool_call_id".into(), json!("c-841"));
    let back = Headers::from_parts(at_hive.context.clone(), hop);
    let (trace, home) = walk(&t, "/m/memory-hive/tool", back);
    assert_eq!(
        trace.last().map(String::as_str),
        Some("/m/assistants/scribe/talky/collector/assemble"),
        "the result must walk home into the asking surface's collector: {trace:?}"
    );
    assert_eq!(hop_of(&home, "route"), "in_tool");
    assert_eq!(ctx_of(&home, "turn_id"), ROUND);

    // At the receiver: the shipped assembler, over the headers the walk delivered.
    let doc = json!({
        "header": {"context": Value::Object(home.context.clone()),
                   "hop": Value::Object(home.hop.clone())},
        "messages": [{"origin": "tool", "type": "tool_result", "id": "c-841",
                      "text": "nothing on record"}]
    });
    let (out, _) = run_cell(assemble_cell::ASSEMBLE, &[], doc);
    let park = in_phase(&out, "round-check");
    assert_eq!(
        park["header"]["turn_id"], ROUND,
        "the tool result is filed under the round that asked, and the round checks itself"
    );
}

#[test]
fn a_result_without_a_turn_is_parked_and_says_so() {
    if !shipped() {
        return;
    }
    let doc = assemble_cell::lane(
        "in_tool",
        json!({"tool_call_id": "c-841"}),
        json!({"turn_id": ""}),
        json!([{"origin": "tool", "type": "tool_result", "id": "c-841", "text": "x"}]),
    );
    let (out, stderr) = run_cell(assemble_cell::ASSEMBLE, &[], doc);
    assert!(out.is_empty(), "still parked, nothing emitted: {out:?}");
    assert!(
        stderr.contains("collector: in_tool without a turn id for session s1 -- parked"),
        "the park must be said on stderr, like every other refusal of the cell; got {stderr:?}"
    );
}

#[test]
fn every_park_without_a_turn_says_so() {
    if !shipped() {
        return;
    }
    for (route, messages) in [
        (
            "in_calls",
            json!([{"origin": "assistant", "type": "tool_call", "id": "c1",
                    "text": "{\"name\":\"x\",\"arguments\":\"{}\"}"}]),
        ),
        (
            "in_answer",
            json!([{"origin": "assistant", "type": "text", "text": "hi"}]),
        ),
    ] {
        let doc = assemble_cell::lane(route, json!({}), json!({"turn_id": ""}), messages);
        let (out, stderr) = run_cell(assemble_cell::ASSEMBLE, &[], doc);
        assert!(out.is_empty(), "{route}: still parked: {out:?}");
        assert!(
            stderr.contains(&format!(
                "collector: {route} without a turn id for session s1 -- parked"
            )),
            "{route}: the park must be said on stderr; got {stderr:?}"
        );
    }
}
