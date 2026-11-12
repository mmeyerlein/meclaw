//! GH #1464 (D58 coldgc, contract points 2 and 3) -- an expired cache
//! collects under the aim.
//!
//! WHY this file exists: a warm window only grows at its end, so the garbage
//! of a conversation stays in it until a rebuild. A rebuild was ordered by a
//! line crossed; a conversation that never crosses one carried its garbage
//! for good. When the cache of the round runs out the next call pays the
//! whole window anyway -- with `gc_on_cold` "1" the stroke of the clock
//! collects first, far under every aim, without a model, and says so: one
//! `gc` mark with the class counted and one mark of the trigger `cold`.
//!
//! Runs the shipped curator hive (`support/curator_cold.rs`); no model.

// harness_fold: #[path = "support/curator_cold.rs"]
#[allow(unused_imports)]
use crate::curator_cold; // harness_fold: mod curator_cold;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_cold::*;
use curator_hive::*;
use meclaw_core::serde_json::json;

#[test]
fn gh_d58_cold_an_expired_cache_collects_under_the_aim() {
    if !shipped() {
        return;
    }
    let (mut t, old, young) = garbage_talk("talky", &cold_knobs(Some("1")));
    assert!(!rebuilt(&t), "check of the setup: no rebuild so far");
    cache_expires(&mut t);

    assert!(
        t.h.summ.is_empty(),
        "a cold collection asks no model: {:?}",
        t.h.stderr
    );
    assert!(
        rebuilt(&t),
        "the collection wrote its plan: {:?}",
        t.h.stderr
    );
    assert_eq!(
        shrunk(&t),
        vec![short_id(&old)],
        "the result of the older tool round is collected, the younger one stays raw \
         (`keep_rounds` 1): {:?}",
        t.h.stderr
    );
    assert!(
        !shrunk(&t).contains(&short_id(&young)),
        "the result of the last tool round stays raw"
    );

    let gc = marks_of(&t.h, "gc");
    assert_eq!(gc.len(), 1, "one `gc` mark for the collection: {gc:?}");
    assert_eq!(gc[0]["classes"]["shrunk_tools"], json!(1), "{gc:?}");
    let cold = cold_marks(&t.h);
    assert_eq!(
        cold.len(),
        1,
        "one mark of the trigger `cold`: {:?}",
        marks_of(&t.h, "curate")
    );

    // The window of the next call shows the cut.
    let window = next_window(&mut t, "t5").join("\n");
    assert!(
        !window.contains(&result_text(1)),
        "the collected result is no longer sent whole"
    );
    assert!(
        window.contains(&result_text(2)),
        "the result of the last tool round is sent whole"
    );
}
