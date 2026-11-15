//! GH #1465 (D58, the bank; contract 2) -- sabotage mutant: a curator that takes the cache for warm at every call
//!
//! WHY this file exists: the figure `cold_pct` of the watch had no mutant.
//! A sound curator knows when the cache went cold and rebuilds then, where a
//! new window costs nothing more: only the two calls after a pause are cold.
//! The broken one takes the cache for warm at every call, lets those moments
//! pass and rebuilds at its target with the cache warm -- the call after
//! each such rebuild is cold too.
//! The trail is the one a curator with the broken invariant writes; the
//! hive's own `stats` door counts it, and the argus meter -- the shipped
//! script, fed that very answer -- must raise the alert of a role goal on
//! `cold_pct`. The sound trail beside it raises nothing, so the alert is
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

const ALERT_AT: u64 = 30;

/// Ten calls with a measured cache. The calls 4 and 8 follow a pause and
/// are cold for both curators. The broken one rebuilds at its target on the
/// calls 1 and 5, with the cache warm: the calls 2 and 6 are cold too.
fn trail(h: &Hive, sound: bool) {
    for i in 0..CALLS {
        let cold = i == 4 || i == 8 || (!sound && (i == 2 || i == 6));
        let order = if !sound && (i == 1 || i == 5) {
            "target"
        } else {
            "none"
        };
        call(h, i);
        let cached = if cold { 0 } else { 50_000 };
        mark(h, i, "curate", &curate(i, order, 60_000, cached, 30));
    }
}

#[test]
fn mutant_always_warm_trips_cold_pct() {
    if !shipped() || !can_run() {
        return;
    }
    for role in ROLES {
        let mut sound = Hive::new();
        trail(&sound, true);
        let (figure, out) = watch(&mut sound, "window", goal(role, "cold_pct", ALERT_AT));
        assert_eq!(figure["cold_pct"], json!(20), "{role}: {figure}");
        assert_eq!(
            alerted(&out),
            None,
            "{role}: a sound curator stays under: {out:?}"
        );

        let mut broken = Hive::new();
        trail(&broken, false);
        let (figure, out) = watch(&mut broken, "window", goal(role, "cold_pct", ALERT_AT));
        assert_eq!(figure["cold_pct"], json!(40), "{role}: {figure}");
        let alert = alerted(&out).unwrap_or_else(|| {
            panic!(
                "{role}: the watch trips on cold_pct (cold_pct 20 sound, 40 broken, \
                 alert_at {ALERT_AT}): {figure} {out:?}"
            )
        });
        assert_eq!(alert["metric"], "cold_pct", "{alert}");
        assert_eq!(alert["role"], role, "{alert}");
    }
}
