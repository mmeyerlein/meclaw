//! D58 lead (GH #1474, contract points 1, 2 and 8) -- the curator holds no
//! choice of a model.
//!
//! WHY this file exists: R-58-9 Nr. 5. The switch (`./lead` with its store
//! `./lead_state`) sat inside the curator hive, so every composite with a
//! curator carried a model choice, whether it chose between targets or not.
//! It leaves as the template `lead`, a hive of its own: no cell of the
//! curator is the lead or its store, no edge of the curator reaches them, no
//! cell of the curator declares a knob of the choice, and the curator speaks
//! to a lead only across its own boundary -- the three lanes out
//! (`lead_ask`, `lead_spec`, `lead_note`) in `contract.emits` and on an
//! edge to the hive path, the verdict home on `in_lead` in
//! `contract.accepts` and on an edge to `./policy`.
//!
//! Reads the shipped templates; no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_lead.rs"]
#[allow(unused_imports)]
use crate::curator_lead; // harness_fold: mod curator_lead;

use curator_hive::*;
use curator_lead::*;
use meclaw_core::serde_json::Value;

fn mentions(e: &Value, route: &str) -> bool {
    e["condition"]
        .as_str()
        .is_some_and(|c| c.contains(&format!("'{route}'")))
}

#[test]
fn gh_d58_lead_the_curator_holds_no_model_choice() {
    if !shipped() {
        return;
    }
    // The cells are gone from the curator ...
    for gone in ["lead", "lead_state"] {
        assert!(
            !repo(&format!("templates/curator/{gone}")).exists(),
            "templates/curator/{gone} still ships: the switch sits inside the curator"
        );
    }
    // ... and live in the hive `lead`.
    let hive = lead_hive();
    assert_eq!(hive["cell"]["type"], "hive", "templates/lead is a hive");
    for cell in LEAD_HIVE_CELLS {
        lead_hive_cell(cell);
    }
    let lead = lead_hive_cell("lead");
    for knob in CHOICE_KNOBS {
        assert!(
            lead["params"].get(knob).is_some(),
            "the lead declares `{knob}`"
        );
    }

    // No edge of the curator reaches a cell of the switch.
    let curator = read_json(&repo("templates/curator/config.json"));
    let edges = edges_of(&curator);
    for e in &edges {
        for end in ["from", "to"] {
            let p = e[end].as_str().unwrap_or("");
            assert!(
                p != "./lead" && p != "./lead_state",
                "the curator still draws an edge to the switch: {e}"
            );
        }
        assert!(
            !e.to_string().contains("curator-lead"),
            "an edge of the curator names the switch's station: {e}"
        );
    }

    // No cell of the curator declares a knob of the choice.
    let dir = repo("templates/curator");
    let mut cells = 0;
    for entry in std::fs::read_dir(&dir).expect("templates/curator") {
        let p = entry.expect("a dir entry").path().join("config.json");
        if !p.is_file() {
            continue;
        }
        cells += 1;
        let cfg = read_json(&p);
        for knob in CHOICE_KNOBS {
            assert!(
                cfg["params"].get(knob).is_none(),
                "{} declares `{knob}`: the curator holds a model choice",
                p.display()
            );
        }
        if let Some(s) = cfg["params"]["script_inline"].as_str() {
            assert!(
                !s.contains("def lead_verdict("),
                "{} decides a verdict of the switch",
                p.display()
            );
        }
    }
    assert!(cells >= 10, "the curator's cells were read ({cells})");

    // The boundary: three lanes out, one home.
    let emits = contract_routes(&curator, "emits");
    let accepts = contract_routes(&curator, "accepts");
    for lane in TO_LEAD {
        assert!(
            emits.iter().any(|r| r == lane),
            "the curator's contract emits `{lane}`: {emits:?}"
        );
        assert!(
            edges
                .iter()
                .any(|e| e["from"] == "./policy" && e["to"] == "." && mentions(e, lane)),
            "an edge `./policy -> .` carries `{lane}` out of the curator"
        );
    }
    assert!(
        accepts.iter().any(|r| r == FROM_LEAD),
        "the curator's contract accepts `{FROM_LEAD}`: {accepts:?}"
    );
    assert!(
        edges
            .iter()
            .any(|e| e["from"] == "." && e["to"] == "./policy" && mentions(e, FROM_LEAD)),
        "an edge `. -> ./policy` brings `{FROM_LEAD}` home"
    );
    // As shipped, no lead is wired.
    let policy = cell_config("policy");
    assert_eq!(
        policy["params"][LEAD_KNOB], "0",
        "the curator ships without a lead (`{LEAD_KNOB}` \"0\")"
    );
}
