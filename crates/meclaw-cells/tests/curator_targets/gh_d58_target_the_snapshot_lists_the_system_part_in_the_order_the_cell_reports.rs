//! D58 target (GH #1483, contract point 2) -- the snapshot lists the system
//! part in the order the cell reports.
//!
//! WHY this file exists: the model cell lays the families of the system part
//! before the provider in its own `system_order`, then the rest
//! alphabetically, and a provider's cache prefix is decided exactly there.
//! The snapshot row of a call (`sent`) listed the system part by path, so a
//! reader had to know the cell's parameter to put the request together
//! again. Now the cell reports its order in the package line, the curator
//! holds it per target and writes `sent` in that order; without a reported
//! line the order stays by path and the row says so (`cut.system_order`).
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_target.rs"]
#[allow(unused_imports)]
use crate::curator_target; // harness_fold: mod curator_target;

use curator_hive::*;
use curator_target::*;
use meclaw_core::serde_json::json;

/// The families of `paths`, each once, in their first order.
fn families(paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in paths {
        let f = p.split('.').next().unwrap_or_default().to_string();
        if !out.contains(&f) {
            out.push(f);
        }
    }
    out
}

/// The order the reported `order` makes of `seen`: the reported families
/// that are there, then the rest alphabetically.
fn by_report(order: &[&str], seen: &[String]) -> Vec<String> {
    let mut out: Vec<String> = order
        .iter()
        .filter(|f| seen.iter().any(|s| s == *f))
        .map(|f| f.to_string())
        .collect();
    let mut rest: Vec<String> = seen.iter().filter(|f| !out.contains(f)).cloned().collect();
    rest.sort();
    out.extend(rest);
    out
}

#[test]
fn gh_d58_target_the_snapshot_lists_the_system_part_in_the_order_the_cell_reports() {
    if !shipped() {
        return;
    }
    // Without a reported line: by path, as before -- and the row says so.
    let mut bare = Hive::with(&[("policy", "snapshot_window", json!("1"))]);
    let c = say(&mut bare, None, "t1", "Hello there.");
    let seen = families(&sent_system_paths(&bare, &c));
    let mut sorted = seen.clone();
    sorted.sort();
    assert_eq!(seen, sorted, "no reported line: by path: {seen:?}");
    assert!(
        seen.len() >= 2,
        "check of the setup: the system part has several families: {seen:?} {:?}",
        bare.stderr
    );
    assert_eq!(
        cut_of(&bare, &c)["system_order"],
        json!("path"),
        "the row says the order is the path's: {}",
        cut_of(&bare, &c)
    );

    // The reported order: the last family by path first, then the first.
    let order = [seen[seen.len() - 1].as_str(), seen[0].as_str()];
    let mut h = Hive::with(&[("policy", "snapshot_window", json!("1"))]);
    announce(&mut h, "seat", roomy("test/seat"), json!({}), json!(order));
    let c = say(&mut h, Some("seat"), "t1", "Hello there.");
    let got = families(&sent_system_paths(&h, &c));
    assert_eq!(
        got,
        by_report(&order, &got),
        "the reported order first, the rest alphabetically: {got:?} {:?}",
        h.stderr
    );
    assert_ne!(
        got, sorted,
        "check of the setup: the reported order is not the path's"
    );
    assert_eq!(
        cut_of(&h, &c)["system_order"],
        json!("reported"),
        "the row says whose order it is: {}",
        cut_of(&h, &c)
    );
}
