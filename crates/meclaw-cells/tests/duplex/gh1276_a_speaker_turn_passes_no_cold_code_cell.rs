//! GH #1276 -- a turn from a channel through the shipped firewall to the
//! object tools passes no `code` cell that runs cold without a reason.
//!
//! The road is `support/speaker_road.rs`: the member's channels, the shipped
//! `firewall` and the shipped `objects` hive, every edge the shipped one. One
//! turn of the member in its own chat places an object tool call; the answer
//! is read where the brain gets it. Then the test colony's message log is
//! read through the colony (no database file) and every delivery on the
//! parent chain of that answer is mapped back to the template cell it was
//! grown from. Each `code` cell among them runs `warm` or `resident` or
//! carries a reason in `templates/RUNNER-MODES.md`, and its row says it sits
//! on a turn (`turn|tool|session`).
//!
//! `firewall/screen` must be among them: the positive signal that the chain
//! was read at all (a chain of nothing would judge nothing).

// harness_fold: #[path = "support/gh1276_runner_modes.rs"]
#[allow(unused_imports)]
use crate::runner_modes; // harness_fold: mod runner_modes;
// harness_fold: #[path = "support/speaker_road.rs"]
#[allow(unused_imports)]
use crate::speaker_road; // harness_fold: mod speaker_road;

use meclaw_colony::api_dto::MessageLogFilter;
use meclaw_core::serde_json::json;
use runner_modes::{judge, shipped_cell};
use speaker_road::*;
use std::collections::{BTreeSet, HashMap};

/// Where the road boots the shipped templates.
const MOUNTS: [(&str, &str); 2] = [("/firewall", "firewall"), ("/objects", "objects")];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_speaker_turn_passes_no_cold_code_cell() {
    if !shipped() {
        return;
    }
    let mut road = Road::boot().await;
    let trace = road
        .say(
            "own",
            Some(OWN_CHAT),
            Some(json!(OWN_CHAT)),
            None,
            &format!("TOOL object_confirm {}", json!({"id": OBJECT})),
        )
        .await;
    let answer = next(&mut road.ear, "the object tool's answer").await;
    assert_eq!(answer.trace_id, trace, "the answer of another turn arrived");
    road.h.quiesce().await;
    let rows = road
        .h
        .read_log(MessageLogFilter {
            trace_id: Some(trace.to_string()),
            ..Default::default()
        })
        .await;
    road.shutdown().await;

    let by_id: HashMap<&str, _> = rows.iter().map(|r| (r.id.as_str(), r)).collect();
    let last = rows
        .iter()
        .find(|r| r.to_path == "/ear")
        .unwrap_or_else(|| panic!("no delivery to /ear in the log of trace {trace}"));
    let mut passed = BTreeSet::new();
    let mut chain = Vec::new();
    let mut at = Some(last);
    while let Some(r) = at {
        chain.push(format!("{} -> {}", r.from_path, r.to_path));
        if let Some(cell) = shipped_cell(&MOUNTS, &r.to_path) {
            passed.insert(cell);
        }
        at = r
            .parent_message_id
            .as_deref()
            .and_then(|p| by_id.get(p).copied());
    }
    assert!(
        passed.contains("firewall/screen"),
        "the turn's chain passes the firewall's screen; code cells read: {passed:?}\nchain:\n{}",
        chain.join("\n")
    );
    let breaches = judge(&passed);
    assert!(
        breaches.is_empty(),
        "a speaker turn passes code cells that break the runner-mode rule (GH #1276):\n{}\n\
         code cells on the chain: {passed:?}",
        breaches.join("\n")
    );
}
