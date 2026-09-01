//! GH #552 — INTERIM PIN: the hand-typed `memory_recall` schema against the
//! memory hive's own `in_query` contract.
//!
//! Today `memory_recall` is served by the COLLECTOR out of its own recall port
//! (GH #512): the dispatcher carries a tool edge that points at the collector,
//! the bundle returns on a second private lane, and the schema the model sees is
//! typed by hand in `self_tool_menu()` — a manual projection of ANOTHER
//! template's `in_query` contract, with nothing pinning the two together. That
//! missing pin is the drift GH #464 was opened for and GH #552 will remove: once
//! the hive answers `in_schemas` itself, the declaration and the contract are one
//! artefact and this file goes with the hand-typed menu entry. Until then, this
//! is the pin.
//!
//! The contract names seven context keys on `in_query`. They are filled from two
//! sides, and the split is the whole content of this file:
//!
//! | contract key (`in_query`) | filled by | how |
//! |---|---|---|
//! | `recall_query`       | the MODEL | schema `query`, onto `hop.recall_query` |
//! | `recall_window_from` | the MODEL | schema `window_from`, onto `hop.recall_window_from` |
//! | `recall_window_to`   | the MODEL | schema `window_to`, onto `hop.recall_window_to` |
//! | `memory_tier`        | the CELL  | the collector's `memory_call_tier` knob — configuration, never a model argument |
//! | `recall_as_of`       | the EDGE  | the shipped `in_query` edges set `''`, and the recall cell reads that as "now" |
//! | `audience_now`       | the EDGE  | out of `context.audience_set` — who is present, the gate on every read |
//! | `channel`            | the EDGE  | out of `context.channel` — where the question is asked |
//!
//! What each half asserts:
//!
//! 1. **Every schema key maps onto a contract key.** A parameter the model can
//!    name that no context key receives is an argument that dies at the port.
//! 2. **Every contract key is either asked of the model or filled beside it.**
//!    A key that is neither would arrive empty on every recall, and on
//!    `audience_now` / `channel` "empty" is a REFUSAL (`missing_audience`,
//!    `missing_channel`), not a wider answer.
//! 3. **What the model names reaches the recall port under the contract's own
//!    name** — measured through the shipped assembler, not asserted.
//! 4. **What the model never names is filled by the shipped wiring** — the tier
//!    off the collector's knob, the other three off EVERY `in_query` edge the
//!    member template draws.
//! 5. **The shape of the ask is the shipped one**: one required argument, two
//!    optional bounds. `query` is required because a recall with no question is
//!    the session-boot request and no model makes that one; the bounds are
//!    optional because an empty window is a point query, the hive's own default.
//!
//! 6. **The one other hand-typed copy says the same thing.** `talky`'s brain
//!    ships the declaration as a SEED row (`brain/seed/system.jsonl`) — what a
//!    fresh agent's `system.tools` holds before the first menu tick replaces the
//!    subtree. Two hand-typed copies of one schema is exactly the drift this
//!    file exists for, so they are compared as values, not trusted.
//!
//! # What this file RECORDS rather than repairs
//!
//! The two bounds are declared as INDEPENDENTLY optional, and `read_window` in
//! the recall cell refuses a half-open window — the request leaves through the
//! reject port as `half_open_window` (`p15_recall_window.rs`). A model that
//! names one bound therefore buys a refusal it cannot see coming. Saying so in
//! the schema changes what the model reads, which on a released template is a
//! version digit plus the ref cascade behind it, for one sentence GH #552 will
//! write ONCE in the hive that enforces it. So case 5 pins today's shape, the
//! finding is written down in the schema's own docstring, and the repair
//! belongs to the rebuild.

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{code_stdin, run_shipped_script, shipped_script};
use std::collections::BTreeSet;

const ASSEMBLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/collector/assemble/config.json"
);
const HIVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/memory-hive/config.json"
);
const MEMBER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/member/config.json"
);
const BRAIN_SEED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/talky/brain/seed/system.jsonl"
);

const MEM: &str = "memory_recall";

/// The MODEL's half of the contract: (schema parameter, the hop key the
/// collector puts it on, the `in_query` context key it becomes).
///
/// The middle column is not decoration: the collector emits its recall request
/// on `hop.*` and the port edge promotes those into `context.*`, so a mapping
/// that named only the two ends could be broken in the middle and still look
/// whole.
const ASKED_OF_THE_MODEL: [(&str, &str, &str); 3] = [
    ("query", "recall_query", "recall_query"),
    ("window_from", "recall_window_from", "recall_window_from"),
    ("window_to", "recall_window_to", "recall_window_to"),
];

/// The contract keys the model is deliberately NOT asked for, and what fills
/// each one instead. A key may leave this list only by entering the schema.
const FILLED_BESIDE_THE_MODEL: [(&str, &str); 4] = [
    (
        "memory_tier",
        "configuration: the collector's `memory_call_tier` knob, put on the hop by \
         `recall_ask` — a model that could choose its own tier could ask for a depth \
         the instance was tuned away from",
    ),
    (
        "recall_as_of",
        "the shipped `in_query` edges set `''`, which the recall cell reads as `now`; \
         nothing upstream of the hive may pin the as-of of a live question",
    ),
    (
        "audience_now",
        "the edge, out of `context.audience_set`: who is present is the gate on every \
         read and cannot be a model argument — a recall without an audience is refused \
         (`missing_audience`), never answered unfiltered",
    ),
    (
        "channel",
        "the edge, out of `context.channel`: where the question is asked, refused as \
         `missing_channel` when absent",
    ),
];

// ─────────────────────────────────────────────────────────── the shipped tree

/// R2b / GH #49: a tree without the template SKIPS instead of failing.
fn shipped(path: &str) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    meclaw_core::serde_json::from_str(&raw).ok()
}

/// The context keys `memory-hive` declares on its `in_query` lane — the
/// contract this schema is a projection of.
fn contract_keys() -> Option<BTreeSet<String>> {
    let hive = shipped(HIVE)?;
    let lane = hive["params"]["contract"]["accepts"]
        .as_array()
        .expect("the hive declares an accepts list")
        .iter()
        .find(|a| a["route"] == json!("in_query"))
        .expect("the hive accepts `in_query`")
        .clone();
    Some(
        lane["context"]
            .as_array()
            .expect("`in_query` names its context keys")
            .iter()
            .map(|k| k.as_str().expect("a context key is a string").to_string())
            .collect(),
    )
}

fn run(doc: &Value) -> Vec<Value> {
    let out = run_shipped_script(&shipped_script(ASSEMBLE), &doc.to_string());
    assert!(
        out.status.success(),
        "the assembler exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    match meclaw_core::serde_json::from_slice(&out.stdout).expect("json out") {
        Value::Array(a) => a,
        other => vec![other],
    }
}

/// The `memory_recall` declaration as it reaches the model: the menu merge of
/// GH #529 run over one foreign answerer's row, with the names this collector
/// serves itself appended. Unwrapped from the provider envelope, so what is
/// measured is the schema rather than the wrapping.
fn recall_declaration() -> Option<Value> {
    shipped(ASSEMBLE)?;
    let rows = json!([{"answerer": "tools",
                       "tools": json!([{"name": "a_tool", "description": "what a_tool does",
                                        "parameters": {"type": "object", "properties": {}}}])
                            .to_string()}]);
    let out = run(&code_stdin(&json!({
        "target": "/main/collector",
        "header": {"hop": {"route": "cstore", "operation": "bundle"},
                   "context": {"col_phase": "menu-merge"}},
        "ttl": 64,
        "messages": [{"id": "c-menu-all", "type": "tool_result",
                      "text": rows.to_string()}],
        "results": [{"tool_call_id": "c-menu-all", "operation": "select"}],
        "params": {"tools": ["a_tool"]},
    })));
    let menu = out
        .iter()
        .find(|m| m["header"]["route"] == json!("menu"))
        .expect("the merge writes one menu");
    let leaf = menu["system"]["tools"][MEM]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("`{MEM}` must be a `text` leaf of the menu: {menu:#?}"));
    let decl: Value = meclaw_core::serde_json::from_str(leaf).expect("a leaf holds json");
    Some(decl["function"].clone())
}

/// The same declaration as `talky`'s brain seeds it — the provider envelope
/// this time, because that is what a `system.tools` leaf holds.
fn seeded_declaration() -> Option<Value> {
    let raw = std::fs::read_to_string(BRAIN_SEED).ok()?;
    let row = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            meclaw_core::serde_json::from_str::<Value>(l).expect("a seed line is one json object")
        })
        .find(|r| r["slot_path"] == json!(format!("tools.{MEM}")))?;
    Some(
        meclaw_core::serde_json::from_str(
            row["value"]["text"]
                .as_str()
                .expect("a system slot holds a `text` leaf"),
        )
        .expect("the seeded leaf holds json"),
    )
}

/// One message as a port edge delivers it: the lane on the hop, the turn in
/// context.
fn memory_call(args: &Value) -> Value {
    code_stdin(&json!({
        "target": "/main/collector",
        "header": {"context": {"session_id": "s1", "turn_id": "t1", "iter": "0"},
                   "hop": {"route": "in_memory_call"}},
        "ttl": 64,
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "m1",
                      "text": args.to_string()}],
        "params": {},
    }))
}

// ════════════════════════════════ 1. every schema key maps onto a contract key

#[test]
fn every_schema_key_maps_onto_a_contract_key() {
    let (Some(decl), Some(contract)) = (recall_declaration(), contract_keys()) else {
        return;
    };
    let declared: BTreeSet<String> = decl["parameters"]["properties"]
        .as_object()
        .expect("the schema declares properties")
        .keys()
        .cloned()
        .collect();
    let mapped: BTreeSet<String> = ASKED_OF_THE_MODEL
        .iter()
        .map(|(schema, _, _)| (*schema).to_string())
        .collect();
    assert_eq!(
        declared, mapped,
        "the hand-typed schema and the mapping table of this file have drifted \
         apart (GH #552). A parameter the model can name that no `in_query` \
         context key receives is an argument that dies at the port — add the \
         wiring and the row, or take the parameter out"
    );
    for (schema, _, ctx) in ASKED_OF_THE_MODEL {
        assert!(
            contract.contains(ctx),
            "schema `{schema}` maps onto `{ctx}`, which `memory-hive` does not \
             name on `in_query` any more: {contract:?}"
        );
    }
}

// ══════════════ 2. every contract key is asked of the model or filled beside it

#[test]
fn every_contract_key_is_asked_of_the_model_or_filled_beside_it() {
    let Some(contract) = contract_keys() else {
        return;
    };
    let mut covered: BTreeSet<String> = ASKED_OF_THE_MODEL
        .iter()
        .map(|(_, _, ctx)| (*ctx).to_string())
        .collect();
    for (key, _) in FILLED_BESIDE_THE_MODEL {
        assert!(
            covered.insert(key.to_string()),
            "`{key}` is listed as filled beside the model AND as asked of it — one \
             filler per key, or nobody can say which value arrives"
        );
    }
    assert_eq!(
        covered, contract,
        "the `in_query` contract and this file's two lists have drifted apart \
         (GH #552). A contract key that is neither asked of the model nor filled \
         beside it arrives EMPTY on every recall — and on `audience_now` / \
         `channel` empty is a refusal, not a wider answer"
    );
}

// ═════════════════════ 3. what the model names reaches the port under its name

#[test]
fn what_the_model_names_reaches_the_recall_port_under_the_contracts_own_name() {
    if shipped(ASSEMBLE).is_none() {
        return;
    }
    let mut args = meclaw_core::serde_json::Map::new();
    for (schema, _, _) in ASKED_OF_THE_MODEL {
        args.insert(schema.to_string(), json!(format!("probe::{schema}")));
    }
    let out = run(&memory_call(&Value::Object(args)));
    let ask = out
        .iter()
        .find(|m| m["header"]["route"] == json!("recall"))
        .expect("the call is served on the collector's own recall port");
    for (schema, hop, _) in ASKED_OF_THE_MODEL {
        assert_eq!(
            ask["header"][hop],
            json!(format!("probe::{schema}")),
            "the model's `{schema}` must leave on `hop.{hop}` — that is the key \
             the port edge promotes into the contract's context: {ask:#?}"
        );
    }
}

// ════════════════════ 4. what the model never names is filled by the wiring

#[test]
fn the_tier_is_configuration_and_rides_the_same_request() {
    if shipped(ASSEMBLE).is_none() {
        return;
    }
    let mut doc = memory_call(&json!({"query": "what did we decide?"}));
    doc["params"] = json!({"memory_call_tier": "2"});
    let out = run(&doc);
    let ask = out
        .iter()
        .find(|m| m["header"]["route"] == json!("recall"))
        .expect("the call is served on the recall port");
    assert_eq!(
        ask["header"]["memory_tier"], "2",
        "the tier comes off the instance's own knob, never off the call — a schema \
         that offered it would let a model ask for a depth the instance was tuned \
         away from: {ask:#?}"
    );
}

#[test]
fn the_shipped_in_query_edges_fill_what_the_model_is_not_asked_for() {
    let Some(member) = shipped(MEMBER) else {
        return;
    };
    let edges: Vec<Value> = member["params"]["graph"]["edges"]
        .as_array()
        .expect("the member declares edges")
        .iter()
        .filter(|e| e["modifier"]["set_hop"]["route"] == json!("'in_query'"))
        .cloned()
        .collect();
    assert!(
        !edges.is_empty(),
        "no shipped edge turns anything into `in_query` any more — the member is \
         where every question against a member's memory passes through"
    );
    for edge in &edges {
        let set = edge["modifier"]["set_context"]
            .as_object()
            .unwrap_or_else(|| panic!("an `in_query` edge promotes context: {edge:#?}"));
        // `memory_tier` is in this list too: the edge carries it across from the
        // hop the collector stamped, and WHERE the value comes from is pinned one
        // test up. What is measured here is only that the edge promotes the key.
        for (key, why) in FILLED_BESIDE_THE_MODEL {
            assert!(
                set.contains_key(key),
                "the `in_query` edge {} -> {} does not set `{key}` ({why}): {edge:#?}",
                edge["from"],
                edge["to"]
            );
        }
    }
}

// ═══════════════════════════ 5. one required question, two optional bounds

#[test]
fn the_question_is_required_and_the_window_is_not() {
    let Some(decl) = recall_declaration() else {
        return;
    };
    let props = decl["parameters"]["properties"]
        .as_object()
        .expect("the schema declares properties");
    let required: BTreeSet<String> = decl["parameters"]["required"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        required,
        BTreeSet::from(["query".to_string()]),
        "a recall with no question at all is the session-boot request of spec D.1, \
         and no model makes that one: the query is the one required argument. The \
         window is not required either way — an empty window is a point query, the \
         memory hive's own default"
    );
    for (name, prop) in props {
        assert_eq!(
            prop["type"], "string",
            "`{name}` must be a string: every one of these arrives at the recall \
             port as a hop key, and a hop key is text — a schema promising a number \
             or an object promises a conversion nobody performs"
        );
        assert!(
            prop["description"].as_str().is_some_and(|d| !d.is_empty()),
            "`{name}` carries a description: the model chooses its arguments out of \
             this text and out of nothing else"
        );
    }
}

// ══════════════════════ 6. the other hand-typed copy says the same thing

#[test]
fn the_seeded_copy_of_the_schema_and_the_served_one_are_the_same_schema() {
    let (Some(served), Some(seeded)) = (recall_declaration(), seeded_declaration()) else {
        return;
    };
    assert_eq!(
        seeded["function"], served,
        "`talky`'s brain seed and the collector's own menu carry DIFFERENT \
         `{MEM}` schemas. The seed is what a fresh agent's `system.tools` holds \
         until the first menu tick replaces the subtree, so the two are what one \
         agent reads in its first minutes and afterwards — one schema, typed \
         twice, is the drift GH #552 removes by letting the memory hive declare \
         the tool itself"
    );
}
