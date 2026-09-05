//! GH #411 — the correlation of a recall travels as CONTEXT, because a hop does
//! not survive the memory hive.
//!
//! A returning answer has to say which question it answers. The caller puts the
//! id on the HOP, and a hop does not survive: the `recall` cell forms its own,
//! and only context travels. Before #411 the shipped member promoted everything
//! the recall shape names EXCEPT the call id — the bundle came back correct and
//! uncorrelatable, the collector filed it as the ambient leg, and the brain saw
//! "tool result lost".
//!
//! **The promotion moved with the call it correlates (GH #552).** It used to sit
//! on the member's two doors into `./memory-hive`, because the caller was a
//! collector one composite away that served `memory_recall` itself. It is now the
//! memory hive's own door: a `tool_call` arrives with `hop.tool_call_id`, the
//! door promotes it into `context.memory_call_id`, and that is what the answer
//! finds its way home by — through `./recall` and back out through `./tool`,
//! which files the result under the original id. The member's two `in_query`
//! doors carry no call id any more, and must not: the lane they open has ONE
//! meaning again, the ambient leg of a turn, and a correlation key on it was the
//! second meaning #552 removed.
//!
//! Facts about the FILES, checked with no colony and no runtime, in the
//! `gh302_org_is_a_namespace` style: read the shipped templates off the tree, so
//! that dropping the promotion goes red here instead of losing a tool result two
//! minutes later in a live round.

use serde_json::Value;

const MEMBER_CONFIG: &str = "../../templates/member/config.json";
const HIVE_CONFIG: &str = "../../templates/memory-hive/config.json";

fn config(path: &str) -> Value {
    let raw = std::fs::read_to_string(path).expect("read the shipped config");
    serde_json::from_str(&raw).expect("a shipped config is JSON")
}

fn edges(config: &Value) -> Vec<&Value> {
    config["params"]["graph"]["edges"]
        .as_array()
        .expect("the template declares params.graph.edges")
        .iter()
        .collect()
}

#[test]
fn the_hives_own_tool_door_promotes_the_call_id_into_context() {
    let hive = config(HIVE_CONFIG);
    let doors: Vec<&Value> = edges(&hive)
        .into_iter()
        .filter(|e| e["from"] == "." && e["to"] == "./tool")
        .collect();
    assert_eq!(
        doors.len(),
        1,
        "the hive has exactly one door into its tool adapter: {doors:#?}"
    );
    let promoted = doors[0]["modifier"]["set_context"]["memory_call_id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "the `tool_call` door must promote the call id into context \
                 (GH #411): {:#}",
                doors[0]
            )
        });
    assert!(
        promoted.contains("hop.tool_call_id"),
        "the promotion reads the id off the hop, where the dispatcher put it \
         (got {promoted:?})"
    );
    assert!(
        promoted.contains("has(hop.tool_call_id)"),
        "an absent id must promote as empty — a CEL error on a missing key \
         would kill the lane instead (got {promoted:?})"
    );
}

#[test]
fn the_ambient_doors_carry_no_correlation_at_all() {
    let member = config(MEMBER_CONFIG);
    let doors: Vec<&Value> = edges(&member)
        .into_iter()
        .filter(|e| {
            e["to"] == "./memory-hive"
                && e["modifier"]["set_hop"]["route"]
                    .as_str()
                    .is_some_and(|r| r.contains("in_query"))
        })
        .collect();
    assert_eq!(
        doors.len(),
        2,
        "the member has exactly two recall doors into ./memory-hive \
         (from ./assistants on 'recall', from . on 'in_recall'): {doors:#?}"
    );
    for door in doors {
        assert!(
            door["modifier"]["set_context"]["memory_call_id"].is_null(),
            "the `in_query` lane has ONE meaning since GH #552 — the ambient leg \
             of a turn. A correlation key on it is the second meaning, and the \
             hive's `bundle` exit now branches on exactly this key to tell a tool \
             round from the ambient one: {door:#}"
        );
    }
}

#[test]
fn the_bundle_exits_of_the_hive_branch_on_that_key() {
    let hive = config(HIVE_CONFIG);
    let exits: Vec<&Value> = edges(&hive)
        .into_iter()
        .filter(|e| e["from"] == "./recall")
        .collect();
    let to_rim: Vec<&&Value> = exits.iter().filter(|e| e["to"] == ".").collect();
    let to_tool: Vec<&&Value> = exits.iter().filter(|e| e["to"] == "./tool").collect();
    assert_eq!(
        to_rim.len(),
        2,
        "bundle and reject leave the hive: {to_rim:#?}"
    );
    assert_eq!(
        to_tool.len(),
        1,
        "and both go to the adapter instead while a call is open: {to_tool:#?}"
    );
    for e in &to_rim {
        let cond = e["condition"].as_str().expect("a condition");
        assert!(
            cond.contains("context.memory_call_id"),
            "the rim exit must exclude an open tool round EXPLICITLY — this hive \
             already has two default edges and a third piece of default semantics \
             is a debugging cost nobody can afford: {cond}"
        );
    }
    let cond = to_tool[0]["condition"].as_str().expect("a condition");
    assert!(
        cond.contains("context.memory_call_id != ''"),
        "and the adapter edge is its complement: {cond}"
    );
}
