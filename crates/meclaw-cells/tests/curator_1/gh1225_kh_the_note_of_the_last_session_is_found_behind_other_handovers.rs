//! GH #1225 (A14) -- the note of the session served last is found, however
//! many handovers came since.
//!
//! WHY this file exists: a new session looked for the `prepared` note of the
//! session served last among the newest 40 `handover` marks of the whole
//! ledger. Behind 40 other handovers (other sessions' leaves, renewals) the
//! note was not read, and the next session started without it. Now the read
//! asks for the `handover` marks of that one session, no count (R-IG-1).

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const NOTE: &str = "kh14 note: the deposit went to the landlord on Monday.";

#[test]
fn the_note_of_the_last_session_is_found_behind_41_other_handovers() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let el = json!({"type": "summary", "text": NOTE});
    let hash = sha256_hex(&canonical(&el));
    h.db.execute(
        "INSERT INTO blocks (hash, kind, chars, body, first_seen) \
         VALUES (?1, 'summary', ?2, ?3, 'x')",
        rusqlite::params![hash, NOTE.len() as i64, canonical(&el)],
    )
    .expect("the note's block");
    let t0 = chrono::Utc::now() - chrono::Duration::hours(1);
    h.mark(
        t0,
        "s-prev",
        "",
        "handover",
        &canonical(&json!({"state": "prepared", "hash": hash, "from_session": "s-prev"})),
    );
    // 41 handovers of other sessions since.
    for i in 1..=41 {
        h.mark(
            t0 + chrono::Duration::seconds(i),
            &format!("s-other-{i}"),
            "",
            "handover",
            &canonical(&json!({"state": "set", "hash": format!("h{i}"),
                               "from_session": "", "prepared": false, "chars": 1})),
        );
    }
    h.db.execute(
        "INSERT OR REPLACE INTO state (key, value) VALUES ('handover_for', 's-prev')",
        [],
    )
    .expect("the session served last");
    let call = h.curate(
        "s-new",
        "n1",
        0,
        json!([user("Where were we?")]),
        mode("Be brief."),
    );
    let leaf = call.body["system"]["history"]["handover"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        leaf.contains(NOTE),
        "the leaf carries the note of s-prev: {leaf:?} {:?}",
        h.stderr
    );
}
