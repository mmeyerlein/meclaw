//! D54g kprofil (GH #1507, OR-54g-83) -- in the transcript a summary is made
//! from, only a row of the kind `user` is the person's words.
//!
//! WHY this file exists: the history of a delegation keeps the voice of every
//! entry since OR-54g-83 (`system`, `tool`, `peer` stay their own kind on the
//! wall), but `./policy` `transcript_line` wrote `user: <text>` for every kind
//! it has no branch for. A system or tool entry of a delegated history -- and
//! any kind added later -- thus stood in the transcript as a statement of the
//! person, and a summary could repeat it as something the person said (the
//! class of GH #1305 and #1469). The rule hangs on the kind, the same for
//! every role: a kind without a branch of its own is a `note:`, never `user:`.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

/// `./policy` `transcript_line(kind, el)` for a text element of origin
/// `user`, the form a delegated history entry of any voice has on the wall.
fn line(kind: &str) -> Value {
    let probe =
        format!(r#"transcript_line({kind:?}, {{"origin": "user", "type": "text", "text": "x"}})"#);
    let (got, err) = policy_scope(json!({}), &probe, Value::Null);
    assert!(!err.contains("Traceback"), "{err}");
    got
}

#[test]
fn gh_d54g_kprofil_window_a_kind_without_a_branch_is_never_the_persons_words() {
    if !shipped() {
        return;
    }
    for kind in ["system", "tool", "a_kind_added_later"] {
        assert_eq!(
            line(kind),
            json!("note: x"),
            "a row of kind {kind:?} is nobody's words in the transcript"
        );
    }
    // The person's own row stays hers, the kinds with a branch stay theirs.
    assert_eq!(line("user"), json!("user: x"), "the person's words");
    assert_eq!(
        line("assistant"),
        json!("assistant: x"),
        "the model's words"
    );
    assert_eq!(line("event"), json!("note: x"), "an inner turn (GH #1305)");
}
