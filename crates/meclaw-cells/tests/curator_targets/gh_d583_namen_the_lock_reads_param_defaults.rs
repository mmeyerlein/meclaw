//! d58-3 namen (GH #1525, contract point 6, R-583-1 form B) -- the lock
//! reads the param defaults of the curator and the lead.
//!
//! WHY this file exists: the names of a composition hid in knob defaults --
//! `errand_slot` "consult" at the policy and the intake, `agent_sources`
//! "advice:cogny,delegation:voice" at the intake, `side_slot` "bridge" at the
//! lead -- where the script reader of the lock never looked. A bare hive ran
//! on a composition's vocabulary, and the policy's own table test had to swap
//! the names out (`neutral_params`). The lock now reads, as its third source,
//! every string and every key of the params of each cell of both hives
//! (`script_inline`, `contract` and `sandbox` left out), split into words at
//! `[^a-z0-9_-]`. A default is a value with no composition behind it, so it
//! names none; the composition sets its own.
//!
//! Reads the shipped tree (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::{Value, json};

fn one(params: Value) -> Vec<(String, Value)> {
    vec![("probe/config.json".to_string(), params)]
}

#[test]
fn gh_d583_namen_the_lock_reads_param_defaults() {
    if !shipped() || !lead_shipped() {
        return;
    }
    let sources = namen_param_sources();
    for want in [
        "templates/curator/policy/config.json",
        "templates/curator/intake/config.json",
        "templates/lead/lead/config.json",
    ] {
        assert!(
            sources.iter().any(|(w, _)| w == want),
            "check of the setup: the params of {want} are read: {:?}",
            sources.iter().map(|(w, _)| w).collect::<Vec<_>>()
        );
    }
    let found = namen_param_findings(&LOCKED_NAMES, &sources);
    assert!(
        found.is_empty(),
        "{} name(s) of a composition in a param default of the curator or the lead:\n  {}",
        found.len(),
        found.join("\n  ")
    );
}

/// The mutant: a default with "consult" makes the lock red -- as a whole
/// value, as a word of a longer text and as a key; the params the lock does
/// not read as defaults stay unread, and a word that only contains a name
/// is none.
#[test]
fn gh_d583_namen_the_lock_reads_param_defaults_mutant() {
    let hit = |params: Value| namen_param_findings(&LOCKED_NAMES, &one(params));
    for planted in [
        json!({"errand_slot": "consult"}),
        json!({"pending": "a consult runs; wait for it"}),
        json!({"slots": ["memo", "consult"]}),
        json!({"routes": {"consult": "x"}}),
        // Fix round (review M6): a name at the start of a sentence.
        json!({"note": "Consult the backend"}),
    ] {
        let found = hit(planted.clone());
        assert!(
            found.iter().any(|f| f.contains("'consult'")),
            "a default with \"consult\" trips the lock: {planted} -> {found:?}"
        );
    }
    let found = hit(json!({"need": "memory"}));
    assert!(
        found.iter().any(|f| f.contains("'memory'")),
        "the list carries `memory`: {found:?}"
    );
    let found = hit(json!({"needs": ["orga"]}));
    assert!(
        found.iter().any(|f| f.contains("'orga'")),
        "the list carries `orga`: {found:?}"
    );
    for clean in [
        json!({"errand_slot": ""}),
        json!({"script_inline": "X = 'consult'", "contract": {"settings": {"consult": {}}},
               "sandbox": {"consult": true}}),
        json!({"id_field": "consult_id"}),
    ] {
        let found = hit(clean.clone());
        assert!(
            found.is_empty(),
            "no name of the lock in {clean}: {found:?}"
        );
    }
}
