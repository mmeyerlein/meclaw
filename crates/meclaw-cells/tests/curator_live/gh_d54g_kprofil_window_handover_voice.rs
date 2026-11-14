//! D54g kprofil (GH #1507, OR-54g-83) -- in the transcript a handover summary
//! is made from, only a row of the kind `user` is the person's words.
//!
//! WHY this file exists: `./policy` `transcript_line` writes a kind without a
//! branch of its own as `note:` since OR-54g-83, but `./handover` has its own
//! `transcript_line` for the summary of a renewal, and its last line still
//! wrote `user: <text>` for every kind it had no branch for -- `event` (an
//! inner turn, GH #1305) included, and the kinds a delegated history keeps on
//! the wall (`system`, `tool`). Such a row stood in the handover transcript as
//! a statement of the person, and the summary could repeat it as something the
//! person said (the class of GH #1305 and #1469). The rule hangs on the kind,
//! the same for every role: a kind without a branch of its own is a `note:`,
//! never `user:`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// `./handover` `transcript_line(kind, el)` for a text element of origin
/// `user`, the form a delegated history entry of any voice has on the wall.
fn line(kind: &str) -> Value {
    let probe =
        format!(r#"transcript_line({kind:?}, {{"origin": "user", "type": "text", "text": "x"}})"#);
    let (got, err) = cell_scope("handover", json!({}), &probe, Value::Null);
    assert!(!err.contains("Traceback"), "{err}");
    got
}

#[test]
fn gh_d54g_kprofil_window_a_kind_without_a_branch_is_never_the_persons_words_in_a_handover() {
    if !shipped() {
        return;
    }
    for kind in ["system", "tool", "event", "a_kind_added_later"] {
        assert_eq!(
            line(kind),
            json!("note: x"),
            "a row of kind {kind:?} is nobody's words in the handover transcript"
        );
    }
    // The person's own row stays hers, the kinds with a branch stay theirs.
    assert_eq!(line("user"), json!("user: x"), "the person's words");
    assert_eq!(
        line("assistant"),
        json!("assistant: x"),
        "the model's words"
    );
    assert_eq!(line("peer"), json!("peer: x"), "another speaker's words");
    assert_eq!(line("advice"), json!("note: x"), "advice stays a note");
}
