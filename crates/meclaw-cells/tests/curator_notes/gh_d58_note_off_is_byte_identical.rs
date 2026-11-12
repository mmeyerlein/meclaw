//! D58 note (GH #1477, contract point 8) -- with `note_once` "0" (shipped)
//! everything is byte for byte as before.
//!
//! WHY this file exists: the knob ships "0" on `curator/intake`,
//! `curator/policy` and `curator/history`. Off, `once` is refused as an
//! unknown scope like today, the new column `receipt` is neither written nor
//! read (no ledger operation names it), nothing answers, and every window is
//! the window of a curator that never saw the note.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/note_once.rs"]
#[allow(unused_imports)]
use crate::note_once; // harness_fold: mod note_once;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};
use note_once::*;

const SESSION: &str = "Stay with the person's topic.";

#[test]
fn off_is_byte_identical() {
    if !shipped() {
        return;
    }
    // Shipped "0" on all three cells.
    for cell in KNOB_CELLS {
        assert_eq!(
            cell_config(cell)["params"][KNOB],
            json!("0"),
            "curator/{cell} ships {KNOB} \"0\""
        );
    }

    // The start window's `Facts: now ...` line reads the clock; both hives
    // read the same fixed instant (`now_utc`), so a second that turns between
    // the two seeds is no difference of the note.
    let shared = || share_hive(&[("history", "now_utc", json!("2026-11-12T00:00:00Z"))]);
    let (mut off, mut plain) = (shared(), shared());
    off.check_emits = true;
    for h in [&mut off, &mut plain] {
        turn(h, "s1", "t1", "a", "b", json!({}));
    }
    // Off: a `once` note is refused as today, a receipt on another note is
    // not written.
    off.stderr.clear();
    post(
        &mut off,
        json!([once("n1", "Refused while off.", Some("r-1")),
               {"id": "n2", "source": "probe", "text": SESSION, "scope": "session",
                "receipt": "r-2"}]),
    );
    post(&mut plain, json!([note("n2", "session", SESSION)]));
    assert!(
        off.stderr
            .iter()
            .any(|l| l.contains("note refused") && l.contains("'once'")),
        "refused as an unknown scope: {:?}",
        off.stderr
    );
    assert_eq!(note_rows(&off), note_rows(&plain), "no once row");
    assert_eq!(
        off.rows("SELECT receipt FROM notes WHERE note_id = 'n2'"),
        vec![vec![Value::Null]],
        "the column is there and not written"
    );

    for (t, words) in [("t2", "c"), ("t3", "e")] {
        off.ledger_ops.clear();
        let a = off.curate("s1", t, 0, json!([user(words)]), mode("Be brief."));
        let b = plain.curate("s1", t, 0, json!([user(words)]), mode("Be brief."));
        assert_eq!(a.messages(), b.messages(), "{t}: the window");
        assert!(shows(&a, SESSION), "{t}: {:?}", texts(&a));
        assert!(receipts(&off).is_empty(), "{t}: nothing answers");
        assert!(
            !off.ledger_ops
                .iter()
                .any(|(_, op)| op.to_string().contains("receipt")),
            "{t}: no ledger operation names the column: {:?}",
            off.ledger_ops
        );
        off.tap(&a, "stop", json!({}), json!([said("d")]));
        plain.tap(&b, "stop", json!({}), json!([said("d")]));
    }
    // The start window too.
    let s_off = seed(&mut off, "call-9", "start", 0);
    let s_plain = seed(&mut plain, "call-9", "start", 0);
    assert_eq!(s_off.messages(), s_plain.messages(), "the seed");
    assert!(receipts(&off).is_empty());
    assert_eq!(off.emit_violations, Vec::<String>::new());
}
