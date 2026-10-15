//! `restore_ttl` takes the substrate's runaway guard out of the game on purpose
//! (GH #82): a restoring edge declares its loop legitimate, so the bound for
//! that loop is the iteration counter in its own condition and nothing else.
//! The substrate refuses an unconditional restoring edge at config load
//! (`BootstrapError::EdgeTtlRestoreUnconditional`) -- these tests say the same
//! thing one level up, off the shipped tree, and add the part the substrate
//! cannot check: that the condition really counts.

use meclaw_core::serde_json::Value;
use std::path::PathBuf;

fn builder_hive() -> Value {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/builder/config.json");
    meclaw_core::serde_json::from_str(&std::fs::read_to_string(p).expect("builder hive"))
        .expect("parses")
}

fn edges() -> Vec<Value> {
    builder_hive()["params"]["graph"]["edges"]
        .as_array()
        .expect("edges")
        .clone()
}

/// Does this condition count something?
///
/// RECALIBRATED with the repair lane: a counter reaches a restoring edge in one
/// of two compartments, and which one is not a matter of taste. The round
/// counter travels in `context`, because every hop of a round is on the loop's
/// own chain. The repair counter cannot: a receipt is rendered by
/// `templates/submit/gate` out of its own parked row, so it arrives on a FOREIGN
/// chain that carries none of the loop's context (measured -- the surviving keys
/// are `sub_carry`/`sub_origin`/`sub_phase` and nothing else). `weave` therefore
/// counts the refusals from the rows it wrote itself and hands the total back as
/// `hop.repairs`. Both are counters on the edge that spends them; only the
/// compartment differs.
fn counts_something(c: &str) -> bool {
    (c.contains("int(context.") || c.contains("int(hop.")) && c.contains('<')
}

#[test]
fn every_restoring_edge_carries_a_counting_condition() {
    let mut restoring = 0;
    for e in edges() {
        if e["modifier"]["restore_ttl"] != Value::Bool(true) {
            continue;
        }
        restoring += 1;
        let c = e["condition"].as_str().unwrap_or("");
        assert!(
            counts_something(c),
            "a restoring edge is bounded by its own counter, and this one is \
             not: {c}"
        );
    }
    assert!(
        restoring >= 1,
        "the loop pays for one round at a time, or it does not fit the budget: \
         ~12 hops per round against a default ttl of 64"
    );
}

#[test]
fn the_re_entry_edge_increments_what_it_bounds() {
    // Select the ROUND re-entry edge by the counter it sets, not by position:
    // there are two restoring edges now, and `find` took whichever the file
    // happened to list first.
    let e = edges()
        .into_iter()
        .find(|e| {
            e["modifier"]["restore_ttl"] == Value::Bool(true)
                && e["modifier"]["set_context"]["iter"].is_string()
        })
        .expect("a re-entry edge that carries the round counter");
    let inc = e["modifier"]["set_context"]["iter"]
        .as_str()
        .expect("the edge owns the counter, no cell does");
    assert!(
        inc.contains("int(context.iter)") && inc.contains("+ 1"),
        "no cell increments and no cell calls the composer directly -- the \
         graph owns both actions: {inc}"
    );
}

#[test]
fn the_capped_round_has_a_destination_that_answers() {
    let capped: Vec<Value> = edges()
        .into_iter()
        .filter(|e| {
            let c = e["condition"].as_str().unwrap_or("");
            c.contains("int(context.iter)") && c.contains(">=")
        })
        .collect();
    assert!(
        !capped.is_empty(),
        "a lane that is not exhaustive parks the turn: the weave has spent its \
         fire guard and nothing else will emit"
    );
}

#[test]
fn the_repair_lane_is_bounded_separately_from_the_round_lane() {
    // RECALIBRATED: the bound reads `hop.repairs`, not `context.repairs`. The
    // counter the loop set does not survive the submitter's parked-and-popped
    // receipt, so `int(context.repairs)` was not merely the wrong compartment --
    // on an absent key it is a CEL error, and a failed condition SKIPS the edge:
    // the repair message went `no_route` and the build died silently. `weave`
    // counts its own receipt rows and stamps the total on the hop instead.
    let repair = edges().into_iter().any(|e| {
        e["condition"]
            .as_str()
            .unwrap_or("")
            .contains("int(hop.repairs)")
    });
    assert!(
        repair,
        "a model that burned its round budget on searches must still have a \
         repair left for the refusal it can actually fix"
    );
}
