//! GH #512 — a cell declares exactly the tools it answers, and the collector
//! answers none.
//!
//! Since GH #464 a collector's tool menu is asked for rather than typed:
//! `params.tools` names what the agent uses, the answerers send the
//! declarations back, and the collector merges them into the menu.
//!
//! GH #512 added the names the collector answered ITSELF to that menu:
//! `thread_recall`, served out of the collector's own round table, which no
//! tools hive could declare. `memory_recall` was the second until GH #552 moved
//! it to the member's memory hive, which declares and answers it. GH #889
//! (R-27-1) took the first away as well: the curator owns the window, the
//! collector keeps no round table to recall from, and so it declares nothing
//! of its own — `menu_self` stays empty
//! (`gh464_a_cell_that_uses_tools_declares_them.rs` measures the menu itself).
//!
//! What this file still pins is the rule that decided both removals: the
//! declaration and the edge are one statement. A composite that routed a tool
//! name into its collector would hand the model a call that reaches a lane
//! nothing answers, and a knob that switched such a lane is a knob the
//! collector does not have. Both shipped composites therefore route no tool
//! name into their collector and set neither of the retired switches.

use meclaw_core::serde_json::{Value, json};

fn templates_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates")
}

/// R2b / GH #49: a tree without the template SKIPS instead of failing.
fn shipped(name: &str) -> bool {
    templates_root().join(name).join("config.json").exists()
}

fn read_json(p: &std::path::Path) -> Value {
    let raw = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    meclaw_core::serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

const MEM: &str = "memory_recall";
const THREAD: &str = "thread_recall";

fn routes_into_the_collector(composite: &str, tool: &str) -> bool {
    let cfg = read_json(&templates_root().join(composite).join("config.json"));
    cfg["params"]["graph"]["edges"]
        .as_array()
        .expect("a composite has edges")
        .iter()
        .any(|e| {
            e["to"] == json!("./collector")
                && e["condition"]
                    .as_str()
                    .is_some_and(|c| c.contains(&format!("hop.tool_name == '{tool}'")))
        })
}

fn collector_override(composite: &str, key: &str) -> Option<Value> {
    let cfg = read_json(
        &templates_root()
            .join(composite)
            .join("collector/config.json"),
    );
    cfg["override_params"]["assemble"]
        .get(key)
        .filter(|v| !v.is_null())
        .cloned()
}

/// The declaration and the edge are one statement, and since GH #889 both
/// halves are empty for the collector: neither composite routes a tool name
/// into it, and neither sets a switch for a lane it no longer has. It was
/// "route `thread_recall` and leave its switch on" until `talky@6.0.0` /
/// `cogny@5.2.0`; the edge and the switch left in the same breath, exactly as
/// `memory_recall`'s did in GH #552.
#[test]
fn the_shipped_composites_route_no_tool_into_their_collector() {
    for composite in ["talky", "cogny"] {
        if !shipped(composite) {
            continue;
        }
        for tool in [THREAD, MEM] {
            assert!(
                !routes_into_the_collector(composite, tool),
                "{composite} still routes `{tool}` into its collector, which serves no tool \
                 of its own since GH #889 — the call would reach a lane nothing answers"
            );
        }
        for knob in ["thread_recall", "thread_recall_budget", "memory_call_tier"] {
            assert_eq!(
                collector_override(composite, knob),
                None,
                "{composite} still sets `{knob}`, a knob the collector does not have any more"
            );
        }
    }
}
