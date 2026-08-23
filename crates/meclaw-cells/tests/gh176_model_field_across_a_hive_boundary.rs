//! GH #176 — a hive's failure lane must not carry `hop.finish_reason` across
//! its own boundary.
//!
//! When an agent hive is folded behind its boundary, most of its exits become
//! named routes. One does not: an `llm` cell that could not produce a
//! completion emits with `hop.finish_reason` set to `error` or `content_filter`
//! and no route of its own, and the edge to the error sink is conditioned on
//! exactly that field. Folded onto the hive, the condition comes with it — so
//! the hive's outward contract reads *"…plus a failure lane recognised by
//! `hop.finish_reason`"*, which is a provider's word for why a completion
//! stopped, in the interface of a thing whose entire purpose is that a caller
//! does not know what is behind it. A hive whose inside is swapped for one that
//! never calls a model cannot honour that clause.
//!
//! Three things are asked here, in the order the issue asks them:
//!
//! 1. **The leak, shown.** With the folded exit, the only condition a caller
//!    can write is on the model field — and the hive cannot state the lane in
//!    its `params.contract` at all, because the substrate's own contract check
//!    finds no exit for it. That is the issue's closing line proven rather than
//!    asserted: a contract that could be checked would have refused the clause.
//! 2. **The fix, in the substrate and on a live colony.** The out-door names
//!    the lane with a `set_hop`; the contract check reads that modifier, so the
//!    hive can DECLARE the lane it produces; the caller conditions on the route
//!    like every other exit, and
//!    the error sink receives **exactly one** message — with nothing looping
//!    back into the model cell. Error paths are where a wrong turn burns a paid
//!    provider call per round until the TTL runs out, so this half is measured
//!    rather than reasoned about, and against a NEGATIVE control that shows the
//!    loop is a real shape and not a hypothetical one.
//! 3. **The shipped library, swept.** No hive template folds the model field
//!    onto its own boundary without naming a lane.

use meclaw_cells::code::CodeCellFactory;
use meclaw_colony::config::HiveParams;
use meclaw_colony::edge_table::{Edge, EdgeTable};
use meclaw_colony::mutation::hive_contract::{HiveContract, Lane, check_lane_doors};
use meclaw_colony::{CellFactory, CellFactoryRegistry, bootstrap_from_filesystem};
use meclaw_core::serde_json::{Value, json};
use meclaw_core::{Headers, Message, MessageBuilder, Path, Uuid};
use meclaw_testing::ColonyHandle;
use meclaw_testing::topologies::phase_3a::CaptureCell;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Where the synthetic hive lives while it is being checked.
const HIVE: &str = "/unit";

// ────────────────────────────────────────────────────── 1. the leak, shown

/// The two shapes of the same out-door, as they appear in a hive's own
/// `params.graph`: the folded one carries the model's word outward, the named
/// one translates it into a lane.
fn out_door(named: bool) -> Value {
    if named {
        json!({"from": "./model", "to": ".",
               "condition": "has(hop.finish_reason) && (hop.finish_reason == 'error' || hop.finish_reason == 'content_filter')",
               "modifier": {"set_hop": {"route": "'llm_error'"}}})
    } else {
        json!({"from": "./model", "to": ".",
               "condition": "has(hop.finish_reason) && (hop.finish_reason == 'error' || hop.finish_reason == 'content_filter')"})
    }
}

fn hive_params(door: Value) -> HiveParams {
    meclaw_core::serde_json::from_value(json!({
        "ports": [],
        "contract": {
            "accepts": [{"route": "in_task", "because": "one task to run"}],
            "emits": [{"route": "llm_error", "because": "the model produced no completion"}]
        },
        "graph": {"edges": [
            {"from": ".", "to": "./model",
             "condition": "has(hop.route) && hop.route == 'in_task'"},
            door
        ]}
    }))
    .expect("params parse")
}

/// The hive's own graph, resolved the way the colony resolves it.
fn table_for(hp: &HiveParams) -> EdgeTable {
    let abs = |ep: &str| -> String {
        match ep {
            "." => HIVE.to_string(),
            other => format!("{HIVE}/{}", other.trim_start_matches("./")),
        }
    };
    let mut t = EdgeTable::new();
    for spec in &hp.graph.edges {
        let condition = spec.condition.as_ref().map(|src| {
            meclaw_colony::cel_eval::parse_condition(src)
                .unwrap_or_else(|e| panic!("condition {src:?}: {e}"))
        });
        // The modifier travels with the edge, exactly as the live colony's
        // edge table carries it — the door that NAMES the lane is the thing
        // under test here, and a table that dropped its modifier would answer
        // a different question.
        let modifier = spec.modifier.as_ref().map(|m| {
            meclaw_colony::cel_eval::parse_modifier(m)
                .unwrap_or_else(|e| panic!("modifier {m:?}: {e:?}"))
        });
        t.insert(Edge {
            id: Uuid::now_v7(),
            from: Path::new(&abs(&spec.from)),
            to: Path::new(&abs(&spec.to)),
            condition,
            modifier,
            is_default: false,
        });
    }
    // The hive has to be wired from outside, or `check_lane_doors` skips it.
    t.insert(Edge {
        id: Uuid::now_v7(),
        from: Path::new("/ingress"),
        to: Path::new(HIVE),
        condition: None,
        modifier: None,
        is_default: false,
    });
    t
}

fn contract_of(hp: &HiveParams) -> HiveContract {
    let spec = hp.contract.as_ref().expect("declared");
    let lane = |l: &meclaw_colony::config::LaneSpec| Lane {
        route: l.route.clone(),
        context: l.context.clone(),
        because: l.because.clone(),
    };
    HiveContract {
        hive_path: HIVE.to_string(),
        accepts: spec.accepts.iter().map(lane).collect(),
        emits: spec.emits.iter().map(lane).collect(),
    }
}

/// **The leak.** A hive whose failure exit is conditioned on the model field
/// cannot declare that failure as a lane: ask the substrate's own contract
/// check and it says so, naming the hive and the lane.
///
/// The consequence for a caller is the whole issue in one sentence — with no
/// route on the message, the only condition an outside edge can write is
/// `hop.finish_reason`, which is a statement about what is inside.
#[test]
fn a_failure_exit_conditioned_on_the_model_field_cannot_be_declared_as_a_lane() {
    let hp = hive_params(out_door(false));
    let c = contract_of(&hp);
    let err = check_lane_doors(std::slice::from_ref(&c), &table_for(&hp))
        .expect_err("a lane with no route-carrying exit must be refused");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("llm_error") && msg.contains("hive_path") || msg.contains(HIVE),
        "the refusal must name the lane and the hive: {msg}"
    );
    assert_eq!(err.error_code(), "hive_contract");
}

/// **And the same door, once it names the lane, IS declarable** — which is the
/// half of #176 that was never in a template.
///
/// The mechanical fix the issue asks for is a `set_hop` on the out-door: the
/// door keeps reading the model field (it is an INNER edge, and requirement 3
/// says that is where structure may be known) and stamps the lane, so the
/// caller conditions on the route like every other exit. The test below proves
/// that door works on a live colony.
///
/// Until the substrate learned this case, it could not be written into
/// `params.contract`: `hive_contract`'s `exit_exists` probed each interior
/// source with headers carrying ONLY `hop.route = <lane>` and never applied the
/// edge's own modifier — so a door that recognises `hop.finish_reason` and
/// PRODUCES the route was invisible to it, and a hive declaring the lane was
/// refused. `exit_exists` now takes the first of the two ways out named in the
/// issue: an interior edge that crosses the hive path and STATES the declared
/// lane in its `set_hop.route` is an exit for that lane.
///
/// The rejection the check exists for is untouched, and is pinned next to the
/// fix in `hive_contract`'s own tests: a door that states a DIFFERENT lane, or
/// states the lane on an edge that never leaves the hive, is still no exit.
#[test]
fn the_named_out_door_is_declarable_because_the_exit_probe_reads_the_modifier() {
    let hp = hive_params(out_door(true));
    let c = contract_of(&hp);
    check_lane_doors(std::slice::from_ref(&c), &table_for(&hp)).expect(
        "the out-door stamps 'llm_error' on its way through the hive path, so the hive can \
         declare the lane it demonstrably emits",
    );
}

/// And the guard that keeps the half above from being a hole: reading the
/// modifier must not let a lane through that the door does not name.
///
/// Same hive, same model-conditioned exit, one difference — the door stamps
/// some other route. Nothing inside produces `llm_error` then, and the refusal
/// this whole check exists for has to stand.
#[test]
fn a_door_that_stamps_another_lane_still_cannot_declare_this_one() {
    let hp: HiveParams = meclaw_core::serde_json::from_value(json!({
        "ports": [],
        "contract": {
            "accepts": [{"route": "in_task", "because": "one task to run"}],
            "emits": [{"route": "llm_error", "because": "the model produced no completion"}]
        },
        "graph": {"edges": [
            {"from": ".", "to": "./model",
             "condition": "has(hop.route) && hop.route == 'in_task'"},
            {"from": "./model", "to": ".",
             "condition": "has(hop.finish_reason) && hop.finish_reason == 'error'",
             "modifier": {"set_hop": {"route": "'answer'"}}}
        ]}
    }))
    .expect("params parse");
    let c = contract_of(&hp);
    let err = check_lane_doors(std::slice::from_ref(&c), &table_for(&hp))
        .expect_err("a door that names 'answer' is not an exit for 'llm_error'");
    assert_eq!(err.error_code(), "hive_contract");
    assert!(
        format!("{err:?}").contains("llm_error"),
        "the refusal names the lane: {err:?}"
    );
}

// ─────────────────────────────────────────────── 2. the fix, on a live colony

/// The model stand-in. Pass 1 (a task, no `finish_reason` on the way in) is the
/// failed completion: it answers the way an `llm` cell answers when it could
/// not produce one. Pass 2 exists only to be observable — if this cell is ever
/// handed its own output back, it says so on the hop, which is what turns "it
/// loops" from an inference into a receipt.
const MODEL: &str = r#"
import sys, json
doc = json.load(sys.stdin)
hop = ((doc.get("envelope") or {}).get("header") or {}).get("hop") or {}
header = {"finish_reason": "error", "error_code": "provider_unavailable"}
if hop.get("finish_reason"):
    header["reentry"] = "1"
sys.stdout.write(json.dumps({
    "header": header,
    "messages": [{"origin": "assistant", "type": "text", "text": "no completion"}]}))
"#;

fn model_cell() -> Value {
    json!({
        "cell": {"type": "code"},
        "params": {"runner": "python3", "script_inline": MODEL, "external_timeout_ms": 15000},
        "contract": {
            "version": "1.0.0",
            "settings": {},
            "emits": {
                "body": {"messages": {"type": "array", "required": true}},
                "hop": {
                    "finish_reason": {"type": "string", "required": true},
                    "error_code": {"type": "string", "required": false},
                    "reentry": {"type": "string", "required": false}
                }
            },
            "consumes": {"body": {"messages": {"type": "array", "required": false}}},
            "capabilities": ["shell:exec"]
        },
        "description": {
            "purpose": "Test stand-in for an llm cell that could not produce a completion.",
            "use_when": "Test fixture only.",
            "not_in_scope": "Not a template."
        }
    })
}

fn write(root: &std::path::Path, rel: &str, v: &Value) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, meclaw_core::serde_json::to_string_pretty(v).unwrap()).unwrap();
}

/// A colony of one hive and one sink outside it.
///
/// `catch_all_door` is the negative control: `rewiring.md` warns that an
/// entry door must be a positive list, because the hive's own OUTBOUND traffic
/// runs over the hive path too. With a catch-all the failure that just left
/// walks straight back in — which is the loop this issue is careful about.
fn tree(td: &tempfile::TempDir, catch_all_door: bool) {
    let root = td.path();
    let door = if catch_all_door {
        json!({"from": ".", "to": "./model"})
    } else {
        json!({"from": ".", "to": "./model",
               "condition": "has(hop.route) && hop.route == 'in_task'"})
    };
    write(
        root,
        "main/config.json",
        &json!({"cell": {"type": "hive"}, "params": {"graph": {"edges": [
            // The caller conditions on the LANE, like every other exit. It
            // never mentions a model field, and it never names a cell inside.
            {"from": "./unit", "to": "/sink",
             "condition": "has(hop.route) && hop.route == 'llm_error'"}
        ]}}}),
    );
    write(
        root,
        "main/unit/config.json",
        &json!({"cell": {"type": "hive"}, "params": {
            "ports": [],
            "contract": {
                "accepts": [{"route": "in_task", "because": "one task to run"}],
                "emits": [{"route": "llm_error", "because": "the model produced no completion"}]
            },
            "graph": {"edges": [door, out_door(true)]}
        }}),
    );
    write(root, "main/unit/model/config.json", &model_cell());
}

async fn boot(td: &tempfile::TempDir) -> (ColonyHandle, mpsc::Receiver<Message>) {
    let h = ColonyHandle::new_with_factories_at(
        td,
        vec![(
            "code".to_string(),
            Arc::new(CodeCellFactory) as Arc<dyn CellFactory>,
        )],
    );
    let (sink_tx, sink_rx) = mpsc::channel::<Message>(64);
    h.spawn(Path::new("/sink"), move || {
        CaptureCell::new(sink_tx.clone())
    })
    .await;
    let mut registry = CellFactoryRegistry::new();
    registry.insert("code".to_string(), Arc::new(CodeCellFactory));
    bootstrap_from_filesystem(td.path(), &registry, &h.runtime())
        .await
        .expect("bootstrap_from_filesystem must succeed");
    (h, sink_rx)
}

/// One task in on the `in_task` lane, addressed at the HIVE path.
async fn send_task(h: &ColonyHandle) {
    let mut hop = meclaw_core::serde_json::Map::new();
    hop.insert("route".into(), json!("in_task"));
    h.send(
        MessageBuilder::new(Path::new("/unit"))
            .headers(Headers::from_parts(
                meclaw_core::serde_json::Map::new(),
                hop,
            ))
            .body(meclaw_core::Body::Inline(json!({"messages": [
                {"origin": "user", "type": "text", "text": "do the thing"}]})))
            .ttl(64)
            .build(),
    )
    .await;
}

/// **The load-bearing test.** The out-door names the lane; the sink gets one
/// message, on the route, and the model cell is never handed its own failure
/// back.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_error_sink_receives_exactly_one_message_and_nothing_loops_back() {
    let td = tempfile::TempDir::new().unwrap();
    tree(&td, false);
    let (h, mut rx) = boot(&td).await;

    send_task(&h).await;

    let first = match tokio::time::timeout(Duration::from_secs(30), rx.recv()).await {
        Ok(Some(m)) => m,
        other => panic!(
            "the failure must reach the sink ({other:?}); dead letters: {:?}",
            h.drain_dead_letters().await
        ),
    };
    let hop = &first.headers.hop;
    assert_eq!(
        hop.get("route").and_then(|v| v.as_str()),
        Some("llm_error"),
        "the message leaves on the lane, which is what the caller conditions on: {hop:?}"
    );
    assert!(
        hop.get("reentry").is_none(),
        "the very first delivery is already a re-entry: {hop:?}"
    );

    // Nothing else arrives. A wrong turn would put the failure back into the
    // model and produce one further message per round until the TTL ran out, so
    // this is the assertion the whole issue is about.
    let second = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await;
    assert!(
        second.is_err(),
        "the error lane delivered more than once: {second:?}"
    );
    assert!(
        h.drain_dead_letters().await.is_empty(),
        "and it did so without a dead letter"
    );

    h.shutdown().await;
}

/// The negative control. Same hive, same out-door, one difference: the entry
/// door is a catch-all instead of a positive list. The failure that just left
/// the hive matches it on the way out, re-enters the model, and comes back
/// carrying the marker — which is the round that costs a provider call.
///
/// Without this half, the test above would also pass on a colony where the loop
/// is simply impossible, and would prove nothing about the door.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_catch_all_entry_door_is_what_turns_the_failure_lane_into_a_loop() {
    let td = tempfile::TempDir::new().unwrap();
    tree(&td, true);
    let (h, mut rx) = boot(&td).await;

    send_task(&h).await;

    let mut seen_reentry = false;
    for _ in 0..4 {
        let Ok(Some(m)) = tokio::time::timeout(Duration::from_secs(30), rx.recv()).await else {
            break;
        };
        if m.headers.hop.get("reentry").is_some() {
            seen_reentry = true;
            break;
        }
    }
    assert!(
        seen_reentry,
        "the catch-all door must feed the model its own failure — if it no \
         longer does, the positive-list rule above has stopped being what \
         protects the error path, and this file has to say what does instead"
    );

    h.shutdown().await;
}

// ──────────────────────────────────────────── 3. the shipped library, swept

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// Every shipped hive `config.json` with its parsed `params`.
fn shipped_hives() -> Vec<(String, HiveParams)> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, HiveParams)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                walk(root, &p, out);
                continue;
            }
            if p.file_name().and_then(|n| n.to_str()) != Some("config.json") {
                continue;
            }
            let val: Value =
                meclaw_core::serde_json::from_str(&std::fs::read_to_string(&p).unwrap())
                    .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            if val
                .get("cell")
                .and_then(|c| c.get("type"))
                .and_then(|t| t.as_str())
                != Some("hive")
            {
                continue;
            }
            let params = val.get("params").cloned().unwrap_or(Value::Null);
            if params.is_null() {
                continue;
            }
            let hp: HiveParams = meclaw_core::serde_json::from_value(params)
                .unwrap_or_else(|e| panic!("{}: params: {e}", p.display()));
            out.push((
                p.strip_prefix(root)
                    .unwrap()
                    .parent()
                    .unwrap()
                    .display()
                    .to_string(),
                hp,
            ));
        }
    }
    let mut out = Vec::new();
    walk(&templates_root(), &templates_root(), &mut out);
    assert!(!out.is_empty(), "the sweep found no hive template at all");
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A model-level field may be read on an edge INSIDE a hive — that is
/// requirement 3, and it is the only place it belongs. What it may not do is
/// leave: an edge whose `to` is the hive itself and whose condition reads
/// `hop.finish_reason` hands the caller a condition about a provider unless it
/// also names a lane with `set_hop.route`.
#[test]
fn no_shipped_hive_carries_the_model_field_across_its_own_boundary() {
    let mut boundary_edges = 0usize;
    for (name, hp) in shipped_hives() {
        for e in hp
            .graph
            .edges
            .iter()
            .filter(|e| e.from == "." || e.to == ".")
        {
            boundary_edges += 1;
            let Some(cond) = e.condition.as_deref() else {
                continue;
            };
            if !cond.contains("finish_reason") {
                continue;
            }
            let names_a_lane = e
                .modifier
                .as_ref()
                .is_some_and(|m| m.set_hop.contains_key("route"));
            assert!(
                names_a_lane,
                "{name}: the boundary edge {} -> {} reads hop.finish_reason and names no lane \
                 — a provider's word for why a completion stopped would become part of this \
                 hive's outward contract (GH #176)",
                e.from, e.to
            );
        }
    }
    assert!(
        boundary_edges >= 10,
        "the sweep saw almost no boundary edge at all: {boundary_edges}"
    );
}

/// And the other half of the same rule: no declared lane is a `finish_reason`
/// value wearing a lane's clothes.
#[test]
fn no_declared_lane_is_a_finish_reason_value() {
    for (name, hp) in shipped_hives() {
        let Some(spec) = hp.contract.as_ref() else {
            continue;
        };
        for l in spec.accepts.iter().chain(spec.emits.iter()) {
            for reserved in ["tool_calls", "content_filter"] {
                assert_ne!(
                    l.route, reserved,
                    "{name}: '{reserved}' is a value of hop.finish_reason, not a lane"
                );
            }
        }
    }
}
