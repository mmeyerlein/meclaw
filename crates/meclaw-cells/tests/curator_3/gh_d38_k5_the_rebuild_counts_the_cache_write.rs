//! GH #1431 (D38 K5, R-HK-16, OR-38-9) -- the rebuild counts the cache write.
//!
//! WHY this file exists: a provider that caches charges the write of the new
//! prefix above its input price (5m 125 %, 1h 200 %), and a rebuild writes
//! the whole rebuilt window. The rule of before counted the rebuild at
//! `cost_in`; with a ladder on the tap it counts it at the cache-write price
//! of the step the AIM falls in -- so a 1-h cache (the set's keepalive,
//! OR-38-9) pays a rebuild back later than a 5-m cache, and a ladder without
//! a write price falls back to the step's input price. The four-argument
//! call of before stays what it was.
//!
//! Runs the pure half of the shipped policy (`support/curator_hive.rs`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/curator_profiles.rs"]
#[allow(unused_imports)]
use crate::curator_profiles; // harness_fold: mod curator_profiles;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// The ladder of a 120k window at the step (read 50 / cached 5) with a base
/// step of read 10 / cached 1 and the base cache-write price `write`
/// (`None` = the ladder names none).
fn limits(write: Option<f64>) -> String {
    let w = write
        .map(|w| format!(", 'cost_cache_write': {w}"))
        .unwrap_or_default();
    format!(
        "{{'input_soft': 200000, 'input_hard': 950000, 'cost_in': 50, 'cost_cached_in': 5, \
         'price_tiers': [{{'above_input': 0, 'cost_in': 10, 'cost_cached_in': 1{w}}}, \
         {{'above_input': 100000, 'cost_in': 50, 'cost_out': 250, 'cost_cached_in': 5, \
         'cost_cache_write': 62.5}}]}}"
    )
}

/// Keeping 120k at the step over the rebuilt 90k costs 0.51 c a turn more.
/// The rebuild: 90 000 x 10 = 0.9 c without a write price (paid back on the
/// second turn), x 12.5 = 1.125 c with the 5-m price (third), x 20 = 1.8 c
/// with the 1-h price (fourth). The worked example of GH #1038 stays three.
#[test]
fn the_rebuild_counts_the_cache_write() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        curator_profiles::set_knobs("talky", json!({})),
        &format!(
            "[amortise_after(120000, 90000, 50, 5, {}), \
             amortise_after(120000, 90000, 50, 5, {}), \
             amortise_after(120000, 90000, 50, 5, {}), \
             amortise_after(500000, 100000, 10, 1)]",
            limits(None),
            limits(Some(12.5)),
            limits(Some(20.0))
        ),
        Value::Null,
    );
    assert!(!err.contains("Traceback"), "{err}");
    assert_eq!(got, json!([2, 3, 4, 3]), "{got}");
}

/// With the ladder the write price of the AIM's step counts, not the held
/// window's: an aim above the step is written at the step's price.
#[test]
fn the_write_price_is_the_aims_step() {
    if !shipped() {
        return;
    }
    // Held 300k at the step (cached 5), rebuilt to 150k, still at the step:
    // keeping costs (300 000 - 150 000) x 5 = 0.75 c a turn more; the rebuild
    // writes 150 000 x 62.5 = 9.375 c -> paid back on the 13th turn.
    let (got, err) = policy_scope(
        curator_profiles::set_knobs("talky", json!({})),
        &format!(
            "amortise_after(300000, 150000, 50, 5, {})",
            limits(Some(12.5))
        ),
        Value::Null,
    );
    assert!(!err.contains("Traceback"), "{err}");
    assert_eq!(got, json!(13), "{got}");
}
