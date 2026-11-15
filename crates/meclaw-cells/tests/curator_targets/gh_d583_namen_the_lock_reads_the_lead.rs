//! d58-3 namen (GH #1525, contract point 6, R-583-1 form B) -- the lock
//! reads the lead.
//!
//! WHY this file exists: the switch of a composition (`templates/lead`)
//! decided by `"memory" in needs` and named its reason `memory` -- a need of
//! one composition written into the code every composition shares. Form B
//! of R-583-1 holds the whole chain to it: no name of a composition in the
//! curator nor in the lead. The lock reads the script of every cell under
//! `templates/lead/` beside those under `templates/curator/`.
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
use meclaw_core::serde_json::json;

const LEAD_AT_CONFIG: &str = "templates/lead/lead/config.json";

#[test]
fn gh_d583_namen_the_lock_reads_the_lead() {
    if !shipped() || !lead_shipped() {
        return;
    }
    let lead: Vec<(String, String)> = curator_sources()
        .into_iter()
        .filter(|(w, _)| w.starts_with(NAMEN_LEAD_ROOT))
        .collect();
    assert!(
        lead.iter().any(|(w, _)| w == LEAD_AT_CONFIG),
        "the lock reads the lead's script: {:?}",
        lead.iter().map(|(w, _)| w).collect::<Vec<_>>()
    );
    let found = lock_findings(&LOCKED_NAMES, &lead, &json!([]), &json!({}));
    assert!(
        found.is_empty(),
        "{} name(s) of a composition in the lead:\n  {}",
        found.len(),
        found.join("\n  ")
    );
}

/// The mutant: a literal "memory" planted in the lead's script makes the
/// lock red.
#[test]
fn gh_d583_namen_the_lock_reads_the_lead_mutant() {
    if !lead_shipped() {
        return;
    }
    let script = lead_script();
    let planted = format!("{script}\nNAMEN_PROBE = \"memory\"\n");
    let src = |code: &str| vec![(LEAD_AT_CONFIG.to_string(), code.to_string())];
    let before = lock_findings(&LOCKED_NAMES, &src(&script), &json!([]), &json!({}));
    let after = lock_findings(&LOCKED_NAMES, &src(&planted), &json!([]), &json!({}));
    assert_eq!(
        after.len(),
        before.len() + 1,
        "the planted literal is one more finding: {after:?}"
    );
    assert!(
        after
            .iter()
            .any(|f| f.starts_with(LEAD_AT_CONFIG) && f.ends_with("'memory'")),
        "the finding names the lead and `memory`: {after:?}"
    );
    // The lock's own sources carry the lead: the planted script stands
    // where `curator_sources` puts the shipped one.
    assert!(
        curator_sources().iter().any(|(w, _)| w == LEAD_AT_CONFIG),
        "curator_sources reads {LEAD_AT_CONFIG}"
    );
}
