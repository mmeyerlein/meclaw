//! D58 note (GH #1477, contract point 3) -- a held fallback that is released
//! uses the note up and answers with its sends.
//!
//! WHY this file exists: a quick turn whose spec answer is not in yet stores
//! its built call in the lead as the fallback; the ledger writes of the call
//! go at once, the sends wait (`lead_spec("fb", ...)`). A `once` note the
//! stored call shows is shown only when that call goes -- so its use and its
//! receipt travel with the held sends (the carrier `go_ops`, `horizon_ops`
//! before this strand) and go with the release (`settle` without
//! `keep`, `fb_go`, `delta`), never before.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/note_once.rs"]
#[allow(unused_imports)]
use crate::note_once; // harness_fold: mod note_once;

use curator_hive::*;
use meclaw_core::serde_json::{self as sj, Value, json};
use note_once::*;

const TEXT: &str = "Mention the pharmacy closes early today.";
const TIMEOUT: &str = r#"{"error_code": "timeout", "error_source": "wire"}"#;

#[test]
fn a_released_fallback_receipts_with_its_sends() {
    if !shipped() {
        return;
    }
    let mut h = once_spec_hive();
    post(&mut h, json!([once("n1", TEXT, Some("r-1"))]));
    assert_eq!(used(&h, "n1"), "", "stored open: {:?}", h.stderr);

    // The spec call goes; the stamp stores the normal call as its fallback.
    let spec = spec_call(&mut h, "t1", "what time is it");
    seam(&mut h, "t1", "simple", "what time is it");
    assert!(
        h.routed("quick").is_empty(),
        "held behind the spec: {:?}",
        h.out
    );
    assert!(receipts(&h).is_empty(), "held, not shown: {:?}", h.out);
    assert_eq!(used(&h, "n1"), "", "held, not used up");

    // The spec fails: the stored call goes, and with it use and receipt.
    let err: Value = sj::from_str(TIMEOUT).expect("JSON");
    spec_end(&mut h, &spec, "error", err, json!([]));
    let quick = h.routed("quick");
    assert_eq!(
        quick.len(),
        1,
        "the fallback goes: {:?} {:?}",
        h.out,
        h.stderr
    );
    assert!(shows(&quick[0], TEXT), "{:?}", texts(&quick[0]));
    let r = receipts(&h);
    assert_eq!(r.len(), 1, "{:?} {:?}", h.out, h.stderr);
    assert_eq!(field(&r[0], "note_id"), "n1");
    assert_eq!(field(&r[0], "receipt"), "r-1");
    assert_eq!(field(&r[0], "turn_id"), "t1");
    assert_eq!(used(&h, "n1"), "s1#t1");
    assert_eq!(h.emit_violations, Vec::<String>::new());
}
