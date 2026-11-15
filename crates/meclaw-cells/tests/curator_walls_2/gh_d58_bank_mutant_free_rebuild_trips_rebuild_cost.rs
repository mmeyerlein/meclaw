//! GH #1465 (D58, the bank; contract 2) -- sabotage mutant: a curator that takes a rebuild for free
//!
//! WHY this file exists: the figure `rebuild_cost` of the watch had no mutant.
//! A sound curator knows what a rebuild costs and orders one when it pays
//! (two of ten calls here); the broken one takes a rebuild for free and
//! orders one at every call, and the cents of the calls that rebuilt add up.
//! The trail is the one a curator with the broken invariant writes; the
//! hive's own `stats` door counts it, and the argus meter -- the shipped
//! script, fed that very answer -- must raise the alert of a role goal on
//! `rebuild_cost`. The sound trail beside it raises nothing, so the alert is
//! the invariant's, not the threshold's. The role of the goal is a
//! parameter. Deterministic: no model anywhere.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_watch.rs"]
#[allow(unused_imports)]
use crate::curator_watch; // harness_fold: mod curator_watch;
// harness_fold: #[path = "gh927/harness.rs"]
#[allow(unused_imports)]
use crate::harness; // harness_fold: mod harness;

use curator_hive::{Hive, shipped};
use curator_watch::{CALLS, ROLES, alerted, call, curate, goal, mark, watch};
use harness::can_run;
use meclaw_core::serde_json::json;

const ALERT_AT: u64 = 100;

/// Ten calls of thirty cents each. The sound curator rebuilds on two of
/// them; the broken one rebuilds on every call.
fn trail(h: &Hive, sound: bool) {
    for i in 0..CALLS {
        let order = if !sound || i == 3 || i == 7 {
            "target"
        } else {
            "none"
        };
        call(h, i);
        mark(h, i, "curate", &curate(i, order, 60_000, 50_000, 30));
    }
}

#[test]
fn mutant_free_rebuild_trips_rebuild_cost() {
    if !shipped() || !can_run() {
        return;
    }
    for role in ROLES {
        let mut sound = Hive::new();
        trail(&sound, true);
        let (figure, out) = watch(&mut sound, "rebuild", goal(role, "rebuild_cost", ALERT_AT));
        assert_eq!(figure["cost_cents_sum"], json!(60), "{role}: {figure}");
        assert_eq!(
            alerted(&out),
            None,
            "{role}: a sound curator stays under: {out:?}"
        );

        let mut broken = Hive::new();
        trail(&broken, false);
        let (figure, out) = watch(&mut broken, "rebuild", goal(role, "rebuild_cost", ALERT_AT));
        assert_eq!(figure["cost_cents_sum"], json!(300), "{role}: {figure}");
        let alert = alerted(&out).unwrap_or_else(|| {
            panic!(
                "{role}: the watch trips on rebuild_cost (cost_cents_sum 60 sound, 300 broken, \
                 alert_at {ALERT_AT}): {figure} {out:?}"
            )
        });
        assert_eq!(alert["metric"], "rebuild_cost", "{alert}");
        assert_eq!(alert["role"], role, "{alert}");
    }
}
