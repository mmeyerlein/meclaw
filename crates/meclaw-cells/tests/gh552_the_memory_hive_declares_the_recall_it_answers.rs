//! GH #552 — the DRIFT LOCK of the new declaration source: `memory_recall` is
//! declared, answered and enforced by the memory hive itself.
//!
//! Until this wave the name was served by the COLLECTOR (GH #512): a private
//! composite edge carried the call into `collector/assemble`, the answer came
//! back on a second private lane, and the schema the model read was a HAND-TYPED
//! projection of another template's `in_query` contract, held to it by nothing
//! but a test. Two more copies of that projection shipped beside it — a seed row
//! in `talky`'s brain and a paragraph of README prose — and each one could drift
//! on its own.
//!
//! Now the hive that enforces the rules declares them: `memory-hive/schemas`
//! answers `in_schemas` with one `{name, description, parameters}`, and
//! `memory-hive/tool` turns a `tool_call` into the hive's own `in_query` and the
//! bundle back into a `tool_result`. The dispatcher routes the name like any
//! other tool name, the member rim carries it, and no cell types a schema it
//! does not answer.
//!
//! # What this file asserts
//!
//! 1. **The declaration exists, and it is the ONLY one.** The hive's `schemas`
//!    cell answers `memory_recall`; and — the negative half `docs/development-rules.md`
//!    § 8 demands of a migration — `self_tool_menu()` in the shipped assembler no
//!    longer carries the name, and `talky`'s brain seed no longer carries the row.
//! 2. **What the model names reaches the recall port under the contract's own
//!    name** — measured through the shipped adapter, not asserted.
//! 3. **What the model is NOT asked for is filled beside it**: the tier off the
//!    adapter's own `params.tier`, `recall_as_of` off the hive edge, and
//!    `audience_now` / `channel` off every member edge that opens the road.
//! 4. **The way to the menu line is drawn**: the level that draws the edge
//!    declares the name, the `schemas` question reaches the hive, and the answer
//!    comes back stamped with an answerer of its own.
//! 5. **The negative control is red**: a level that draws the edge and does not
//!    declare the name offers its model a menu without `memory_recall` in it, and
//!    the predicate of claim 4 says so.
//!
//! # What moved, and where each old copy went
//!
//! | old copy | now |
//! |---|---|
//! | `self_tool_menu()`'s `memory_recall` branch (`collector/assemble`) | `templates/memory-hive/schemas/config.json`, `SCHEMAS` |
//! | `templates/talky/brain/seed/system.jsonl` line 2 | gone — the menu tick asks the hive |
//! | `templates/collector/README.md` § "The memory tool" prose | `templates/memory-hive/README.md` § "The recall tool" |
//! | the half-open-window warning `self_tool_menu()` RECORDED | written once, in the description the hive serves |

use meclaw_core::serde_json::{Value, json};
use meclaw_testing::{code_stdin, run_shipped_script, shipped_script};
use std::collections::BTreeSet;

const HIVE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/memory-hive/config.json"
);
const HIVE_SCHEMAS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/memory-hive/schemas/config.json"
);
const HIVE_TOOL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/memory-hive/tool/config.json"
);
const MEMBER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/member/config.json"
);
const ASSEMBLE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/collector/assemble/config.json"
);
const BRAIN_SEED: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/talky/brain/seed/system.jsonl"
);
const ASSISTANT_TALKY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../templates/assistant/talky/config.json"
);

const MEM: &str = "memory_recall";

/// The MODEL's half of the contract: (schema parameter, the hop key the adapter
/// puts it on, the `in_query` context key it becomes).
///
/// The middle column is not decoration: the adapter emits its ask on `hop.*` and
/// the hive's own edge promotes those into `context.*`, so a mapping that named
/// only the two ends could be broken in the middle and still look whole.
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
        "configuration: the adapter's own `params.tier`, put on the hop by the ask — a \
         model that could choose its own tier could ask for a depth the instance was \
         tuned away from",
    ),
    (
        "recall_as_of",
        "the hive's `./tool -> ./recall` edge sets `''`, which the recall cell reads as \
         `now`; nothing upstream may pin the as-of of a live question",
    ),
    (
        "audience_now",
        "the member edge, out of `context.audience_set`: who is present is the gate on \
         every read and cannot be a model argument — a recall without an audience is \
         refused (`missing_audience`), never answered unfiltered",
    ),
    (
        "channel",
        "the member edge, out of `context.channel`: where the question is asked, refused \
         as `missing_channel` when absent",
    ),
];

// ─────────────────────────────────────────────────────────── the shipped tree

/// R2b / GH #49: a tree without the template SKIPS instead of failing.
fn shipped(path: &str) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    meclaw_core::serde_json::from_str(&raw).ok()
}

fn edges(tree: &Value) -> Vec<Value> {
    tree["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn run(path: &str, doc: &Value) -> Vec<Value> {
    let out = run_shipped_script(&shipped_script(path), &doc.to_string());
    assert!(
        out.status.success(),
        "{path} exited non-zero: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    match meclaw_core::serde_json::from_slice(&out.stdout).expect("json out") {
        Value::Array(a) => a,
        other => vec![other],
    }
}

/// The declaration as the hive hands it out, asked for with `["*"]` — the same
/// request a collector's menu tick makes.
fn hive_declarations() -> Option<Vec<Value>> {
    shipped(HIVE_SCHEMAS)?;
    let out = run(
        HIVE_SCHEMAS,
        &code_stdin(&json!({
            "target": "/main/memory/schemas",
            "header": {"hop": {"route": "in_schemas"}, "context": {}},
            "ttl": 64,
            "tools": ["*"],
            "params": {},
        })),
    );
    Some(
        out[0]["schemas"]
            .as_array()
            .expect("the answer carries a `schemas` list")
            .clone(),
    )
}

/// One `memory_recall` call as the member rim delivers it into the hive.
fn tool_call(args: &Value, params: &Value) -> Value {
    code_stdin(&json!({
        "target": "/main/memory/tool",
        "header": {"context": {"audience_now": "[\"alex\"]", "channel": "c1",
                               "session_id": "s1", "turn_id": "t1"},
                   "hop": {"route": "tool_call", "tool_name": MEM,
                           "tool_call_id": "m1"}},
        "ttl": 64,
        "messages": [{"origin": "assistant", "type": "tool_call", "id": "m1",
                      "text": args.to_string()}],
        "params": params,
    }))
}

/// The menu one shipped collector produces, as the GH #529 merge writes it:
/// one foreign answerer's row plus the names this cell answers itself.
///
/// Measured rather than read, because what matters is not whether a string
/// appears in a script but which names reach the model.
fn collector_menu() -> Option<Value> {
    shipped(ASSEMBLE)?;
    let rows = json!([{"answerer": "tools",
                       "tools": json!([{"name": "a_tool", "description": "what a_tool does",
                                        "parameters": {"type": "object", "properties": {}}}])
                            .to_string()}]);
    let out = run(
        ASSEMBLE,
        &code_stdin(&json!({
            "target": "/main/collector",
            "header": {"hop": {"route": "cstore", "operation": "bundle"},
                       "context": {"col_phase": "menu-merge"}},
            "ttl": 64,
            "messages": [{"id": "c-menu-all", "type": "tool_result",
                          "text": rows.to_string()}],
            "results": [{"tool_call_id": "c-menu-all", "operation": "select"}],
            "params": {"tools": ["a_tool"]},
        })),
    );
    out.into_iter()
        .find(|m| m["header"]["route"] == json!("menu"))
}

/// The body of `self_tool_menu()` in the shipped assembler — the function that
/// IS the collector's declaration source, cut out of the script.
///
/// Only the DECLARATION literal is measured, never a mention: the ambient leg
/// still synthesises a `memory_recall` call further down (`recall_call`,
/// GH #278), because a recall bundle travelling as a tool result needs a tool
/// call in front of it, and the docstring of the function says where the name
/// went. Naming a tool somebody else answers is fine; declaring it is the thing
/// that moved.
fn the_collectors_own_menu() -> Option<String> {
    let src = shipped_script(ASSEMBLE);
    let start = src.find("def self_tool_menu():")?;
    let rest = &src[start..];
    let end = rest[1..]
        .find("\ndef ")
        .map(|i| i + 1)
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

// ══════════════ 1. the declaration exists, and it is the only one in the tree

#[test]
fn the_hive_declares_the_recall_and_nothing_else_does() {
    let Some(schemas) = hive_declarations() else {
        return;
    };
    let names: BTreeSet<String> = schemas
        .iter()
        .map(|s| {
            s["name"]
                .as_str()
                .expect("a declaration carries its name")
                .to_string()
        })
        .collect();
    assert_eq!(
        names,
        BTreeSet::from([MEM.to_string()]),
        "the memory hive declares exactly the one tool it answers — `{MEM}`, and \
         nothing else: `thread_recall` reads the COLLECTOR's own slate and is declared \
         where it is answered (ruling 31.08.)"
    );
    let decl = &schemas[0];
    assert!(
        decl["description"]
            .as_str()
            .is_some_and(|d| d.contains("window")),
        "the description names the window, because the half-open rule the recall cell \
         ENFORCES is now written where it is enforced: {decl:#?}"
    );
    let props = decl["parameters"]["properties"]
        .as_object()
        .expect("the schema declares properties");
    let declared: BTreeSet<String> = props.keys().cloned().collect();
    let mapped: BTreeSet<String> = ASKED_OF_THE_MODEL
        .iter()
        .map(|(schema, _, _)| (*schema).to_string())
        .collect();
    assert_eq!(
        declared, mapped,
        "the hive's schema and the mapping table of this file have drifted apart. A \
         parameter the model can name that no `in_query` context key receives is an \
         argument that dies at the port"
    );
    assert_eq!(
        decl["parameters"]["required"],
        json!(["query"]),
        "a recall with no question at all is the session-boot request of spec D.1, and \
         no model makes that one: the query is the one required argument"
    );
}

#[test]
fn the_collector_no_longer_types_the_schema_it_does_not_answer() {
    let Some(menu) = collector_menu() else {
        return;
    };
    assert_eq!(
        menu["header"]["menu_self"], "thread_recall",
        "the collector answers ONE name itself now. `{MEM}` is served by the member's \
         memory, which is the hive that enforces the rules the schema states — a cell \
         that answers a call it cannot enforce the rules of will drift from them: {menu:#?}"
    );
    assert!(
        menu["system"]["tools"][MEM].is_null(),
        "`{MEM}` still reaches the model as a leaf of THIS cell's menu. It belongs to the \
         answerer, and this cell is not it: {menu:#?}"
    );
    let Some(src) = the_collectors_own_menu() else {
        return;
    };
    assert!(
        !src.contains(&format!("\"name\": \"{MEM}\"")),
        "`self_tool_menu()` still TYPES the schema. `docs/development-rules.md` § 8 asks \
         for this half of a migration explicitly: the old source is not merely unused, it \
         is gone:\n{src}"
    );
    assert!(
        src.contains("THREAD_RECALL"),
        "`thread_recall` must stay: it reads this collector's OWN slate, no other cell \
         may read that table, and it is declared where it is answered:\n{src}"
    );
}

#[test]
fn the_brain_seed_no_longer_carries_the_second_hand_typed_copy() {
    let Ok(raw) = std::fs::read_to_string(BRAIN_SEED) else {
        return;
    };
    let slots: Vec<String> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| meclaw_core::serde_json::from_str::<Value>(l).ok())
        .filter_map(|r| r["slot_path"].as_str().map(str::to_string))
        .collect();
    assert!(
        !slots.contains(&format!("tools.{MEM}")),
        "the brain seed still carries a `tools.{MEM}` row. A seed is written once, at \
         birth, and the first menu tick replaces the whole `system.tools` subtree — so \
         the row could only ever be a second copy of a schema somebody else owns: {slots:?}"
    );
}

// ═════════════════════ 2. what the model names reaches the port under its name

#[test]
fn what_the_model_names_reaches_the_recall_port_under_the_contracts_own_name() {
    if shipped(HIVE_TOOL).is_none() {
        return;
    }
    let mut args = meclaw_core::serde_json::Map::new();
    for (schema, _, _) in ASKED_OF_THE_MODEL {
        args.insert(schema.to_string(), json!(format!("probe::{schema}")));
    }
    let out = run(HIVE_TOOL, &tool_call(&Value::Object(args), &json!({})));
    let ask = out
        .iter()
        .find(|m| m["header"]["route"] == json!("ask"))
        .expect("the adapter asks the hive's own recall cell");
    for (schema, hop, _) in ASKED_OF_THE_MODEL {
        assert_eq!(
            ask["header"][hop],
            json!(format!("probe::{schema}")),
            "the model's `{schema}` must leave on `hop.{hop}` — that is the key the \
             hive's own edge promotes into the contract's context: {ask:#?}"
        );
    }
}

#[test]
fn a_refusal_comes_back_as_a_tool_result_under_the_hives_own_reason() {
    if shipped(HIVE_TOOL).is_none() {
        return;
    }
    let out = run(
        HIVE_TOOL,
        &code_stdin(&json!({
            "target": "/main/memory/tool",
            "header": {"context": {"memory_call_id": "m1"},
                       "hop": {"route": "reject", "reject_reason": "half_open_window"}},
            "ttl": 64,
            "messages": [{"origin": "user", "type": "text",
                          "text": "name both bounds or neither"}],
            "params": {},
        })),
    );
    let res = out
        .iter()
        .find(|m| m["header"]["route"] == json!("tool_result"))
        .expect(
            "a refusal is ANSWERED, never dropped: an unanswered call stalls the \
                 asking round until its idle window runs out",
        );
    assert_eq!(
        res["header"]["error_code"], "half_open_window",
        "the hive's own `reject_reason` becomes the result's `error_code`, verbatim — a \
         second vocabulary for the same four refusals would be a translation nobody \
         maintains: {res:#?}"
    );
    assert_eq!(
        res["header"]["tool_call_id"], "m1",
        "the refusal answers under the ORIGINAL call id, or the round waits for a call \
         nobody can answer: {res:#?}"
    );
}

// ═════════════════ 3. what the model is not asked for is filled beside it

#[test]
fn the_tier_is_configuration_and_rides_the_same_ask() {
    if shipped(HIVE_TOOL).is_none() {
        return;
    }
    let out = run(
        HIVE_TOOL,
        &tool_call(
            &json!({"query": "what did we decide?"}),
            &json!({"tier": "2"}),
        ),
    );
    let ask = out
        .iter()
        .find(|m| m["header"]["route"] == json!("ask"))
        .expect("the call is asked into the hive");
    assert_eq!(
        ask["header"]["memory_tier"], "2",
        "the tier comes off the hive's own knob, never off the call — a schema that \
         offered it would let a model ask for a depth the instance was tuned away from: \
         {ask:#?}"
    );
}

#[test]
fn the_hive_edge_pins_the_as_of_of_a_live_question() {
    let Some(hive) = shipped(HIVE) else {
        return;
    };
    let edge = edges(&hive)
        .into_iter()
        .find(|e| e["from"] == json!("./tool") && e["to"] == json!("./recall"))
        .expect("the adapter reaches the recall cell on an edge of this hive");
    assert_eq!(
        edge["modifier"]["set_context"]["recall_as_of"],
        json!("''"),
        "the ask carries an EMPTY as-of, which the recall cell reads as `now`: a tool \
         round asks what memory holds now, and nothing upstream may pin that: {edge:#?}"
    );
    for (schema, hop, ctx) in ASKED_OF_THE_MODEL {
        assert!(
            edge["modifier"]["set_context"][ctx].is_string(),
            "the model's `{schema}` arrives on `hop.{hop}` and has to be promoted into \
             `context.{ctx}`, or the recall cell never sees it: {edge:#?}"
        );
    }
}

#[test]
fn every_member_door_into_the_memory_stamps_the_round() {
    let Some(member) = shipped(MEMBER) else {
        return;
    };
    let doors: Vec<Value> = edges(&member)
        .into_iter()
        .filter(|e| {
            let route = &e["modifier"]["set_hop"]["route"];
            route == &json!("'in_query'") || route == &json!("'tool_call'")
        })
        .collect();
    assert!(
        doors.len() >= 2,
        "the member draws BOTH doors into its memory — the ambient leg's `in_query` and \
         the tool road's `tool_call`. Found: {doors:#?}"
    );
    for door in &doors {
        let set = door["modifier"]["set_context"]
            .as_object()
            .unwrap_or_else(|| panic!("a door into the memory promotes context: {door:#?}"));
        for key in ["audience_now", "channel"] {
            let why = FILLED_BESIDE_THE_MODEL
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, why)| *why)
                .unwrap_or_default();
            assert!(
                set.contains_key(key),
                "the door {} -> {} does not set `{key}` ({why}): {door:#?}",
                door["from"],
                door["to"]
            );
        }
    }
}

// ═══════════════════════════ 4. the way to the menu line is drawn

/// The predicate of claim 4, over one assistant-level ref marker and one member
/// tree: the level that DRAWS the edge is the level that declares the name.
///
/// Split out so claim 5 can point it at a tree that does not, and see it say no.
fn the_road_is_complete(assistant_talky: &Value, member: &Value) -> Result<(), String> {
    let declared = assistant_talky["override_params"]["collector/assemble"]["tools"]
        .as_array()
        .map(|a| a.iter().any(|n| n == &json!(MEM)))
        .unwrap_or(false);
    if !declared {
        return Err(format!(
            "the level draws the edge and does not declare `{MEM}` in its surface's tool \
             list — the menu tick never asks for it, the hive never answers, and the round \
             looks like a model that chose not to ask its memory"
        ));
    }
    let ask = edges(member).into_iter().any(|e| {
        e["to"] == json!("./memory-hive")
            && e["modifier"]["set_hop"]["route"] == json!("'in_schemas'")
    });
    if !ask {
        return Err(
            "no member edge turns a `schemas` question into the hive's own \
                    `in_schemas`"
                .to_string(),
        );
    }
    let answered = edges(member).into_iter().any(|e| {
        e["from"] == json!("./memory-hive")
            && e["modifier"]["set_hop"]["route"] == json!("'in_menu'")
            && e["modifier"]["set_context"]["tool_answerer"] == json!("'memory'")
    });
    if !answered {
        return Err(
            "the way back carries no `tool_answerer` — the menu merge of GH #529 \
                    keys the store table on it, so an answer without one overwrites the \
                    menu instead of joining it"
                .to_string(),
        );
    }
    Ok(())
}

#[test]
fn the_level_that_draws_the_edge_declares_the_name() {
    let (Some(marker), Some(member)) = (shipped(ASSISTANT_TALKY), shipped(MEMBER)) else {
        return;
    };
    if let Err(why) = the_road_is_complete(&marker, &member) {
        panic!("the shipped assistant level does not carry the recall tool: {why}");
    }
}

// ══════════════════════════════════ 5. the negative control

#[test]
fn a_level_that_does_not_declare_the_name_is_caught() {
    let (Some(mut marker), Some(member)) = (shipped(ASSISTANT_TALKY), shipped(MEMBER)) else {
        return;
    };
    let tools = marker["override_params"]["collector/assemble"]["tools"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    marker["override_params"]["collector/assemble"]["tools"] = Value::Array(
        tools
            .into_iter()
            .filter(|n| n != &json!(MEM))
            .collect::<Vec<_>>(),
    );
    assert!(
        the_road_is_complete(&marker, &member).is_err(),
        "the predicate of claim 4 accepted a level whose surface does not declare `{MEM}`. \
         A drift lock that cannot fail proves nothing"
    );
}
