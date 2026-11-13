//! GH #1464 (D58 coldgc, contract point 6) -- the rebuild is priced at the
//! cache write without a ladder.
//!
//! WHY this file exists: a rebuild writes the whole rebuilt window into the
//! cache, and a provider charges that write above its input price. The cost
//! rule knew this only for a catalog row with a price ladder (GH #1431); a
//! row with one price and a cache-write price was counted at `cost_in`, so
//! the rule called a rebuild paid back turns before it was. Now the write
//! price of the row counts wherever the row names one; a row that names
//! none, and the four-argument call of before, read as they did.
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

#[test]
fn gh_d58_cold_the_rebuild_is_priced_at_the_cache_write_without_a_ladder() {
    if !shipped() {
        return;
    }
    // Keeping 120k over the rebuilt 90k costs 30 000 x 1 a turn more. The
    // rebuild: 90 000 x 12.5 with the write price of the row (38th turn),
    // 90 000 x 20 with a dearer one (61st), 90 000 x 10 at the input price
    // when the row names none (31st) -- as the call of before, without a row.
    let (got, err) = policy_scope(
        curator_profiles::set_knobs("talky", json!({})),
        "[amortise_after(120000, 90000, 10, 1, \
           {'input_soft': 200000, 'cost_in': 10, 'cost_cached_in': 1, 'cost_cache_write': 12.5}), \
          amortise_after(120000, 90000, 10, 1, \
           {'input_soft': 200000, 'cost_in': 10, 'cost_cached_in': 1, 'cost_cache_write': 20}), \
          amortise_after(120000, 90000, 10, 1, \
           {'input_soft': 200000, 'cost_in': 10, 'cost_cached_in': 1}), \
          amortise_after(120000, 90000, 10, 1, \
           {'input_soft': 200000, 'cost_in': 10, 'cost_cached_in': 1, 'cost_cache_write': 0}), \
          amortise_after(120000, 90000, 10, 1), \
          amortise_after(90000, 90000, 10, 1, {'cost_cache_write': 12.5})]",
        Value::Null,
    );
    assert!(!err.contains("Traceback"), "{err}");
    assert_eq!(got, json!([38, 61, 31, 31, 31, null]), "{got}");
}

/// The mark follows the price: a window the rule of before rebuilt for its
/// cost after 31 turns waits for the 38th when the row names its write price.
#[test]
fn the_cost_mark_waits_for_the_write_price() {
    if !shipped() {
        return;
    }
    let probe = "[curate_mark(120000, ARGS, n)['mark'] for n in (31, 37, 38)]";
    let row = json!({"input_soft": 200000, "input_hard": 1000000,
                     "cost_in": 10, "cost_cached_in": 1, "cost_cache_write": 12.5});
    let (got, err) = policy_scope(
        curator_profiles::set_knobs("consult", json!({"rebuild_to": 0.45})),
        probe,
        row,
    );
    assert!(!err.contains("Traceback"), "{err}");
    assert_eq!(got, json!(["none", "none", "cost"]), "{got}");
}
