//! D58 feed (GH #1481, plan `u-feed.md` contract point 7, review F-W2) --
//! off takes no order.
//!
//! WHY this file exists: the lane from outside is new and the composition
//! decides whether a domain may feed its running sessions. The curator hive
//! wires `in_feed` to `./policy` and declares it in `contract.accepts`; the
//! knob `feed_in` on `curator/policy`, shipped "0", keeps it shut: an order
//! is parked -- nothing sent, nothing booked, nothing claimed, not one
//! operation at the store. Shut means the knob, not a missing edge.

// harness_fold: #[path = "support/curator_feed.rs"]
#[allow(unused_imports)]
use crate::curator_feed; // harness_fold: mod curator_feed;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_feed::*;
use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

#[test]
fn gh_d58_feed_off_takes_no_order() {
    if !shipped() {
        return;
    }
    assert_eq!(
        cell_config("policy")["params"][KNOB],
        json!("0"),
        "curator/policy ships `{KNOB}` off"
    );
    let hive = read_json(&repo("templates/curator/config.json"));
    let edges = hive["params"]["graph"]["edges"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        edges.iter().any(|e| e["from"] == json!(".")
            && e["to"] == json!("./policy")
            && e["condition"]
                .as_str()
                .is_some_and(|c| c.contains(&format!("'{LANE}'")))),
        "the hive wires `{LANE}` to ./policy"
    );
    let accepts: Vec<Value> = hive["params"]["contract"]["accepts"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        accepts.iter().any(|l| l["route"] == json!(LANE)),
        "the hive declares `{LANE}` in contract.accepts"
    );

    let mut h = Hive::new();
    h.check_emits = true;
    let ops = h.ledger_ops.len();
    let mut hop = order_head(Some(FEED_LIVE), "t1", "fact");
    hop["once"] = json!("k1");
    order(&mut h, hop, fact_body("The heating is fixed."));
    assert!(feeds(&h).is_empty(), "nothing sent: {:?}", h.out);
    assert!(feed_marks(&h).is_empty(), "nothing booked");
    assert!(claims(&h).is_empty(), "nothing claimed");
    assert_eq!(
        h.ledger_ops.len(),
        ops,
        "not one operation at the store: {:?}",
        &h.ledger_ops[ops..]
    );
    assert!(h.emit_violations.is_empty(), "{:?}", h.emit_violations);
}
