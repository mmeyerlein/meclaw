//! D58 note (GH #1477, contract point 3) -- a kept speculation uses up and
//! answers for the note its window showed.
//!
//! WHY this file exists: when the spec answer wins, the stored normal call
//! never goes (`unsent_ops`); the person hears the answer of the spec call,
//! whose window went to the model at once (plan review W1). That window
//! showed the note, so the booking happens where the speculation is kept --
//! `spec_act` `settle` with `keep` when the answer comes after the stamp, the
//! policy's `kept` branch when it came before. Without it the note would
//! stay open and stand in the next turn again.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/note_once.rs"]
#[allow(unused_imports)]
use crate::note_once; // harness_fold: mod note_once;

use curator_hive::*;
use meclaw_core::serde_json::json;
use note_once::*;

const TEXT: &str = "Say the bus runs late today.";

#[test]
fn a_kept_speculation_receipts_what_it_showed() {
    if !shipped() {
        return;
    }
    for stamp_first in [true, false] {
        let mut h = once_spec_hive();
        post(&mut h, json!([once("n1", TEXT, Some("r-1"))]));
        let spec = spec_call(&mut h, "t1", "when does the bus come");
        assert!(
            shows(&spec, TEXT),
            "stamp_first={stamp_first}: the spec window shows it: {:?}",
            texts(&spec)
        );
        assert!(receipts(&h).is_empty(), "not kept yet: {:?}", h.out);
        assert_eq!(used(&h, "n1"), "", "not kept yet");
        let mut seen = Vec::new();
        if stamp_first {
            seam(&mut h, "t1", "simple", "when does the bus come");
            seen.extend(receipts(&h));
            spec_end(
                &mut h,
                &spec,
                "stop",
                json!({}),
                json!([said("At ten past.")]),
            );
        } else {
            spec_end(
                &mut h,
                &spec,
                "stop",
                json!({}),
                json!([said("At ten past.")]),
            );
            seen.extend(receipts(&h));
            seam(&mut h, "t1", "simple", "when does the bus come");
        }
        assert_eq!(
            h.routed("spec_release").len() + h.routed("quick").len(),
            1,
            "stamp_first={stamp_first}: the kept answer goes: {:?} {:?}",
            h.out,
            h.stderr
        );
        assert!(
            h.routed("quick").is_empty(),
            "stamp_first={stamp_first}: the stored call never goes: {:?}",
            h.out
        );
        seen.extend(receipts(&h));
        assert_eq!(
            seen.len(),
            1,
            "stamp_first={stamp_first}: one receipt: {seen:?} {:?}",
            h.stderr
        );
        assert_eq!(field(&seen[0], "receipt"), "r-1");
        assert_eq!(field(&seen[0], "turn_id"), "t1");
        assert_eq!(used(&h, "n1"), "s1#t1", "stamp_first={stamp_first}");

        // The next turn does not show it again.
        let c = h.curate("s1", "t2", 0, json!([user("thanks")]), mode("Be brief."));
        assert!(
            !shows(&c, TEXT),
            "stamp_first={stamp_first}: {:?}",
            texts(&c)
        );
        assert_eq!(h.emit_violations, Vec::<String>::new());
    }
}
