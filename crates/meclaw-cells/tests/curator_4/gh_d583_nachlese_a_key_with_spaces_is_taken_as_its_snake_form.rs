//! GH #1539 (d58-3 nachlese) -- a statement line of a brief whose key is
//! words with spaces (`parcel for the harbour office/promised: ...`) has
//! exactly one reading: the key in its snake form, spaces joined by `_`. The
//! parser takes it so and `brief_render` writes the canonical form; a
//! well-formed line comes back byte for byte.
//!
//! WHY this file exists: in the summary bank of d58-3 (role consult, form
//! brief with `summary_state_cond`) one answer wrote every key with spaces
//! and 43 of its 44 lines were dropped -- 18 planted promises, open points
//! and facts gone with them. The key group `[\w.-]+/[\w.-]+` admits no
//! space, so the line fell to "a statement without key" and was no line.
//!
//! Only a statement line (`person`, `agent:self`, `peer:<ref>`) has this
//! reading: a line of a kind already has one -- all after its day is its
//! text (GH #1517) -- and keeps it. A key in upper case stays no key.
//!
//! The pure halves run in the shipped policy's own scope (`policy_scope`).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

/// `[who, key, text, id, canonical text]` of `brief_line(x)` for each `x`
/// of the args, `None` where it is no line.
const PARSE: &str = "[None if l is None else [l.get('who'), l.get('key'), l.get('text'), \
                     l.get('id'), brief_render([l])] for l in [brief_line(x) for x in ARGS]]";

/// Pin: a statement key with spaces is taken in its snake form and rendered
/// so.
#[test]
fn a_key_with_spaces_is_taken_as_its_snake_form() {
    if !shipped() {
        return;
    }
    let (got, err) = policy_scope(
        json!({"summary_form": "brief"}),
        PARSE,
        json!([
            "- person (2026-02-27) reading circle/meeting_room: The circle meets in room 2. \
             [asserted] [#ab74bef9b940]",
            "- agent:self (2026-02-27) parcel for the harbour office/promised: \"I promise to \
             bring it.\" [asserted] [#334ca5c80c33]",
            "- peer:anna (2026-02-28) harbour office/opening hours: Open from nine. [hedged] \
             [#334ca5c80c33]"
        ]),
    );
    assert_eq!(
        got,
        json!([
            [
                "person",
                "reading_circle/meeting_room",
                "The circle meets in room 2.",
                "ab74bef9b940",
                "- person (2026-02-27) reading_circle/meeting_room: The circle meets in room 2. \
                 [asserted] [#ab74bef9b940]"
            ],
            [
                "agent:self",
                "parcel_for_the_harbour_office/promised",
                "\"I promise to bring it.\"",
                "334ca5c80c33",
                "- agent:self (2026-02-27) parcel_for_the_harbour_office/promised: \"I promise to \
                 bring it.\" [asserted] [#334ca5c80c33]"
            ],
            [
                "peer:anna",
                "harbour_office/opening_hours",
                "Open from nine.",
                "334ca5c80c33",
                "- peer:anna (2026-02-28) harbour_office/opening_hours: Open from nine. [hedged] \
                 [#334ca5c80c33]"
            ]
        ]),
        "{err}"
    );
}

/// Lock: a well-formed line comes back byte for byte; a line of a kind keeps
/// its one reading (all after the day is its text); a key with spaces in
/// upper case stays no key, and so does a statement key without `/` -- and
/// a slash between two digits (`10/11`) is no key's slash.
#[test]
fn the_other_lines_keep_their_reading() {
    if !shipped() {
        return;
    }
    let well = "- person (2026-02-27) reading_circle/meeting_room: The circle meets in room 2. \
                [asserted] [#ab74bef9b940]";
    let (got, err) = policy_scope(
        json!({"summary_form": "brief"}),
        PARSE,
        json!([
            well,
            "- open (2026-03-01) the market list/status: not done [#4b1b1ddb7e09]",
            "- person (2026-02-27) Reading Circle/room: The circle meets. [#ab74bef9b940]",
            "- person (2026-02-27) reading circle: The circle meets. [#ab74bef9b940]",
            "- person (2026-02-27) we take the 10/11 bus: it is late [#ab74bef9b940]"
        ]),
    );
    assert_eq!(
        got,
        json!([
            [
                "person",
                "reading_circle/meeting_room",
                "The circle meets in room 2.",
                "ab74bef9b940",
                well
            ],
            [
                "open",
                "open",
                "the market list/status: not done",
                "4b1b1ddb7e09",
                "- open (2026-03-01): the market list/status: not done [#4b1b1ddb7e09]"
            ],
            null,
            null,
            null
        ]),
        "{err}"
    );
}
