//! D58 target (GH #1483, contract point 7) -- two targets never share a
//! window bound.
//!
//! WHY this file exists: the bound an answer's header reports
//! (`input_soft`) was kept per way (`input_soft:<way>`), so the three fast
//! cells of one composite shared one bound, and the context field
//! `sa_stage` split a composite's two models by hand. Now a bound is kept
//! per target: two targets answer with two bounds, and each keeps its own
//! under `catalog:<target>:input_soft` (OR-d58-2.target.21).
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
use meclaw_core::serde_json::{Value, json};

fn usage(input_soft: i64) -> Value {
    json!({"tokens_prompt": 2000, "tokens_cached": 1000,
           "input_soft": input_soft, "input_hard": 10_000_000,
           "cost_in": 10, "cost_cached_in": 0,
           "cache_expires_at": "2099-01-01T00:00:00Z"})
}

#[test]
fn gh_d58_target_two_targets_never_share_a_window_bound() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    announce_profile(&mut h, "first", json!({}));
    announce_profile(&mut h, "second", json!({}));
    let a = say(&mut h, Some("first"), "t1", TALK[0].0);
    answer(&mut h, &a, "stop", usage(777_000), json!([said(TALK[0].1)]));
    let b = say(&mut h, Some("second"), "t2", TALK[1].0);
    answer(&mut h, &b, "stop", usage(999_000), json!([said(TALK[1].1)]));
    assert_eq!(
        h.state("catalog:first:input_soft"),
        "777000",
        "the first target keeps its own bound: {:?}",
        h.rows("SELECT key, value FROM state WHERE key LIKE '%input_soft%'")
    );
    assert_eq!(
        h.state("catalog:second:input_soft"),
        "999000",
        "the second keeps its own: {:?}",
        h.rows("SELECT key, value FROM state WHERE key LIKE '%input_soft%'")
    );
    // The bare key is the wall's (OR-d58-2.target.16), no target's.
    let stray = h.rows(
        "SELECT key FROM state WHERE key LIKE '%input_soft%' AND key <> 'input_soft' \
         AND key NOT IN ('catalog:first:input_soft', 'catalog:second:input_soft')",
    );
    assert!(stray.is_empty(), "no bound under any other key: {stray:?}");
}
