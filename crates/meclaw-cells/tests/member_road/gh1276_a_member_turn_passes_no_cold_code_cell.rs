//! GH #1276 -- a member's turn with a tool round passes no `code` cell that
//! runs cold without a reason.
//!
//! The road is the member road of GH #929 (`support/gh929_member_road.rs`,
//! entered below the firewall): the shipped `assistant` generation with its
//! talky, session keeper, dispatcher, collector and curator, the shipped
//! `memory-hive`, every `llm` cell on a local stub. Its S3 run
//! (`support/gh929_tool_roads.rs`) is a talky turn whose brain first calls
//! `history_search` and answers after the result -- one turn and one tool
//! round on the same road. Every delivery on the parent chain of the answer
//! is mapped back to the template cell it was grown from; each `code` cell
//! among them runs `warm` or `resident` or carries a reason in
//! `templates/RUNNER-MODES.md`, and its row says it sits on a turn
//! (`turn|tool|session`).
//!
//! `session-keeper/stamp` and `dispatcher` must be among them: the positive
//! signal that the chain was read at all.

// harness_fold: #[path = "support/gh1276_runner_modes.rs"]
#[allow(unused_imports)]
use crate::runner_modes; // harness_fold: mod runner_modes;
// harness_fold: #[path = "mock_openai.rs"]
#[allow(unused_imports)]
use crate::mock_openai; // harness_fold: mod mock_openai;
// harness_fold: #[path = "support/gh929_member_road.rs"]
#[allow(unused_imports)]
use crate::road; // harness_fold: mod road;
// harness_fold: #[path = "support/gh929_tool_roads.rs"]
#[allow(unused_imports)]
use crate::tool_roads; // harness_fold: mod tool_roads;

use road::GENERATION;
use runner_modes::{judge, shipped_cell};
use std::collections::{BTreeSet, HashMap};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_turn_passes_no_cold_code_cell() {
    if !road::shipped() {
        return;
    }
    let mounts = [(GENERATION, "assistant"), ("/memory-hive", "memory-hive")];
    let (_, run, _) = tool_roads::s3_history_call().await;
    let by_id: HashMap<&str, &road::Delivery> =
        run.log.iter().map(|d| (d.id.as_str(), d)).collect();
    let last = run
        .log
        .iter()
        .find(|d| d.to == "/sink")
        .expect("the answer after the tool round left the container");
    let mut passed = BTreeSet::new();
    let mut chain = Vec::new();
    let mut at = Some(last);
    while let Some(d) = at {
        chain.push(d.say());
        if let Some(cell) = shipped_cell(&mounts, &d.to) {
            passed.insert(cell);
        }
        at = d.parent.as_deref().and_then(|p| by_id.get(p).copied());
    }
    for must in ["session-keeper/stamp", "dispatcher"] {
        assert!(
            passed.contains(must),
            "the turn's chain passes `{must}`; code cells read: {passed:?}\nchain:\n{}",
            chain.join("\n")
        );
    }
    let breaches = judge(&passed);
    assert!(
        breaches.is_empty(),
        "a member turn passes code cells that break the runner-mode rule (GH #1276):\n{}\n\
         code cells on the chain: {passed:?}",
        breaches.join("\n")
    );
}
