//! d58-3 namen (GH #1525, contract points 1, 2 and 6; R-583-1 form B,
//! R-58-7 no. 1) -- no need name in the curator or the lead.
//!
//! WHY this file exists: which needs of a stamp call for a recall (`memory`)
//! or let the state block in (`memory`, `orga`) is the composition's word.
//! The policy (`recall_unneeded`, `recall_sure_no`, `state_of_rows`, the
//! frame literal, the way's reason and leg) and the lead (its need test, its
//! reason token, its delta leg) wrote them as literals, so a composition
//! with another need name needed a code change in the shared hives. They
//! now read the names from params (`recall_needs`, `state_needs`) the
//! composition sets; the lock's list carries both names, and the whole lock
//! -- scripts of both hives, the curator's edges and contract, the params of
//! both hives -- finds neither.
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

#[test]
fn gh_d583_namen_no_need_name_in_curator_or_lead() {
    if !shipped() || !lead_shipped() {
        return;
    }
    for name in NAMEN_NEED_NAMES {
        assert!(
            LOCKED_NAMES.contains(&name),
            "the lock's list carries the need name `{name}`"
        );
    }
    let found = namen_lock_findings(&NAMEN_NEED_NAMES);
    assert!(
        found.is_empty(),
        "{} need name(s) in the curator or the lead:\n  {}",
        found.len(),
        found.join("\n  ")
    );
}
