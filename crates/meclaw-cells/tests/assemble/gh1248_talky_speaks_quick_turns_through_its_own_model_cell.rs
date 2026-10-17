//! GH #1248 (T4-09) -- talky speaks its quick turns through a model cell of its own.
//!
//! WHY this file exists: the curator routes a turn its stamp calls `talk` or
//! `simple` onto the way `quick` (GH #1247), and until this bundle that way
//! reached `./brain` on the seam's own edge. The owner's line (R-WP-1, R-WP-9):
//! a second model cell `./quick` behind the curator, reasoning off, born on
//! the brain's start value -- it takes the tier `quick` of the model registry
//! once an instance subscribes it there. What this file pins, read off the
//! shipped tree:
//!
//! 1. `./quick` is an `llm` cell born on the brain's start value, with reasoning
//!    off, a need in prose and the app id every llm cell sends (GH #1251);
//! 2. the curator's `quick` reaches `./quick` and the brain's edge no longer
//!    takes it; `./quick` answers like the brain -- splitter, tap on `in_llm`,
//!    errors, a refused push out to `.`;
//! 3. a model push addressed to `.../quick` takes its own door, and the brain's
//!    door lets it pass; the credential lane opens at `./quick`.

// harness_fold: #[path = "support/assemble_cell.rs"]
#[allow(unused_imports)]
use crate::assemble_cell; // harness_fold: mod assemble_cell;

use assemble_cell::config_of;
use serde_json::{Value, json};

const TALKY: &str = "templates/talky/config.json";
const QUICK: &str = "templates/talky/quick/config.json";
const BRAIN: &str = "templates/talky/brain/config.json";

fn edge<'a>(edges: &'a [Value], from: &str, to: &str, needle: &str) -> &'a Value {
    edges
        .iter()
        .find(|e| {
            e["from"] == from
                && e["to"] == to
                && e["condition"].as_str().unwrap_or("").contains(needle)
        })
        .unwrap_or_else(|| panic!("talky has no edge {from} -> {to} on {needle}"))
}

fn at_of(lanes: &Value, route: &str) -> Vec<String> {
    lanes
        .as_array()
        .expect("lanes")
        .iter()
        .find(|l| l["route"] == route)
        .and_then(|l| l["at"].as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn talky_speaks_quick_turns_through_its_own_model_cell() {
    // 1. the cell
    let quick = config_of(QUICK);
    let brain = config_of(BRAIN);
    assert_eq!(quick["cell"]["type"], "llm", "{quick}");
    assert_eq!(
        quick["params"]["model"], brain["params"]["model"],
        "born on the brain's start value"
    );
    assert_eq!(
        quick["params"]["reasoning"],
        json!({"enabled": false}),
        "reasoning off: the first token decides"
    );
    for k in ["x_title", "http_referer"] {
        assert!(
            !quick["params"][k].as_str().unwrap_or_default().is_empty(),
            "every llm cell sends an app id (GH #1251): `{k}` of ./quick is set"
        );
        assert_eq!(
            quick["params"][k], brain["params"][k],
            "the quick voice names its app like the brain does (GH #1251): {k}"
        );
    }
    let need = quick["params"]["requirement"].as_str().unwrap_or_default();
    assert!(
        !need.is_empty() && need != brain["params"]["requirement"].as_str().unwrap_or(""),
        "the quick voice states its own need: {need}"
    );

    // 2. the curator's `quick` reaches `./quick`, and `./quick` answers like the brain
    let talky = config_of(TALKY);
    let edges = talky["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone();
    let to_quick = edge(&edges, "./curator", "./quick", "'quick'");
    assert_eq!(to_quick["modifier"]["restore_ttl"], true, "{to_quick}");
    assert_eq!(
        to_quick["modifier"]["set_context"],
        edge(&edges, "./curator", "./brain", "'brain'")["modifier"]["set_context"],
        "the quick call carries the context of a brain call"
    );
    let to_brain = edge(&edges, "./curator", "./brain", "'brain'");
    assert!(
        !to_brain["condition"]
            .as_str()
            .unwrap_or("")
            .contains("'quick'"),
        "the brain no longer takes the quick way: {to_brain}"
    );
    edge(&edges, "./quick", "./splitter", "'stop'");
    let tap = edge(&edges, "./quick", "./curator", "finish_reason");
    assert_eq!(tap["modifier"]["set_hop"]["route"], "'in_llm'", "{tap}");
    edge(&edges, "./quick", "./errors", "'error'");
    let refused = edge(&edges, "./quick", ".", "refused_subscriber");
    assert_eq!(
        refused["modifier"]["set_hop"]["route"], "'model_refused'",
        "{refused}"
    );

    // 3. the model door and the credential lane
    let door = edge(&edges, ".", "./quick", "'in_model'");
    assert!(
        door["condition"]
            .as_str()
            .unwrap_or("")
            .contains("endsWith('/quick')"),
        "{door}"
    );
    assert!(
        edge(&edges, ".", "./brain", "'in_model'")["condition"]
            .as_str()
            .unwrap_or("")
            .contains("endsWith('/quick')"),
        "the brain's door lets a push for the quick voice pass"
    );
    let contract = &talky["params"]["contract"];
    assert!(
        at_of(&contract["accepts"], "in_sealed").contains(&"./quick".to_string()),
        "the sealed credential reaches ./quick"
    );
    assert!(
        at_of(&contract["emits"], "credential_request").contains(&"./quick".to_string()),
        "./quick asks for its credential"
    );
}
