//! GH #1469 (D58 errand) -- a failed errand leaves its outcome in the window.
//!
//! WHY this file exists: the call of an errand that comes back with
//! `finish_reason` "error" left nothing behind. The ledger held the
//! `(delegation)` row alone; the person had heard "I am passing that on",
//! and the model of the next turn knew nothing of the failure (bank find
//! F6). With `errand_outcome` "1" the first curate of the session after the
//! failure files one row: it names the errand by the text of its
//! `(delegation)` row and says `failed`, with the code only in the narrow
//! form of a code (OR-BD-6: never the words of a provider). The row stands
//! behind the errand in the window of that very call. The rule hangs on the
//! kind of the row: it holds under every role the policy knows.

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;
// harness_fold: #[path = "support/curator_errand.rs"]
#[allow(unused_imports)]
use crate::curator_errand; // harness_fold: mod curator_errand;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use chrono::Duration;
use curator_errand::*;
use curator_hive::*;

/// The example value sets of the former roles (R-58-15: the curator keeps
/// none; `support/curator_profiles.rs` holds them), the bare curator without
/// a reported profile (`""`, the shipped values) first.
fn roles() -> Vec<String> {
    std::iter::once("")
        .chain(curator_profiles::VALUE_SETS.iter().copied())
        .map(|r| r.to_string())
        .collect()
}

#[test]
fn a_failed_errand_leaves_its_outcome_in_the_window() {
    if !shipped() {
        return;
    }
    let mut missed: Vec<String> = Vec::new();
    let roles = roles();
    assert!(roles.len() > 1, "the example value sets: {roles:?}");
    for role in &roles {
        let mut h = hive(Some("1"), &[]);
        if !role.is_empty() {
            h.report_profile(curator_profiles::profile(role));
        }
        let e = hand_on(&mut h, start(), "p01", "aaaa1111", 5);
        fails(
            &mut h,
            start() + Duration::seconds(20),
            &e,
            "upstream_error",
        );
        // The deadline lies ahead: the failure alone ends the errand.
        let call = ask(&mut h, start() + Duration::minutes(1), "t2");
        let lines = outcome_lines(&call, &e);
        if lines.len() != 1 {
            missed.push(format!(
                "role {role:?}: one message says the outcome of the errand, got {lines:?} in {:?}",
                texts(&call)
            ));
            continue;
        }
        let line = &lines[0];
        if !line.contains(FAILED) || line.contains(NO_ANSWER) || !line.contains("upstream_error") {
            missed.push(format!(
                "role {role:?}: the message says failed and the code: {line:?}"
            ));
        }
        let (errand, outcome) = (place(&call, &e.text), place(&call, line));
        if errand.is_none() || errand >= outcome {
            missed.push(format!(
                "role {role:?}: the outcome stands behind the errand: {errand:?} {outcome:?}"
            ));
        }
        let rows = outcome_rows(&h, &e);
        if rows.len() != 1 || !line.ends_with(&rows[0]) {
            missed.push(format!(
                "role {role:?}: the message is the one outcome row of the wall: {rows:?} {line:?}"
            ));
        }
    }
    assert!(missed.is_empty(), "{}", missed.join("\n"));
}

#[test]
fn a_code_that_is_no_code_stays_out_of_the_row() {
    if !shipped() {
        return;
    }
    let mut h = hive(Some("1"), &[]);
    let e = hand_on(&mut h, start(), "p01", "aaaa1111", 5);
    fails(
        &mut h,
        start() + Duration::seconds(20),
        &e,
        "HTTP 502: the gateway said no",
    );
    let call = ask(&mut h, start() + Duration::minutes(1), "t2");
    let lines = outcome_lines(&call, &e);
    assert_eq!(lines.len(), 1, "one outcome: {:?}", texts(&call));
    assert!(lines[0].contains(FAILED), "{lines:?}");
    assert!(
        !lines[0].contains("502") && !lines[0].contains("gateway"),
        "the words of a provider are no code and stay out: {lines:?}"
    );
}
