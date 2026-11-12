//! D58 followup (GH #1479, contract point 3) -- the first receipt closes the
//! concern, and the cell withdraws the other copies.
//!
//! WHY this file exists: the concern is asked after ONCE ("ask lightly once,
//! then follow", D38 V2). The cell hands one note to every child with a
//! curator; "once" holds per curator (note strand, `u-note.md` point 7), so
//! without a withdrawal the person would be asked again in the chat after the
//! call had asked. Here: talky's curator shows the note in the next start
//! window and answers `note_receipt`; the receipt comes home to the cell
//! through talky's rim; the cell withdraws the note (same id, text empty)
//! from the other children, and talky-chat's curator no longer shows it in
//! its next turn although it held the note open before. A second receipt (a
//! curator that showed the note before the withdrawal arrived) changes
//! nothing: no new note with text. The same claim said again the same day
//! after the receipt opens nothing. The cell's state lives in its own store:
//! the harness runs it cold on every message.
//!
//! Red until the cell exists; behind it red until the note strand (GH #1477)
//! is in the tree.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;
// harness_fold: #[path = "support/assistant_followup.rs"]
#[allow(unused_imports)]
use crate::assistant_followup; // harness_fold: mod assistant_followup;

use assistant_followup::*;
use curator_hive::*;
use meclaw_core::serde_json::json;

const CLAIM: &str = "Rita has a slight twinge in her back today.";

#[test]
fn gh_d58_followup_the_first_receipt_closes_it_and_withdraws_the_copies() {
    if !shipped() {
        return;
    }
    let mut a = Assistant::on_at(NOON);
    let twinge = || json!([fact("Rita", "has_symptom", CLAIM, None, true)]);
    a.leave("./talky", memory_out("text-1", "t1", twinge()));
    // The hearing session ends; the note goes (OR-d58-2.followup.19).
    session_ends(&mut a, "./talky", "text-1");
    let mut talky = curator(&[]);
    let mut chat = curator(&[]);
    let mut late = curator(&[]);
    assert_eq!(deliver_notes(&a, "./talky", &mut talky), 1);
    assert_eq!(deliver_notes(&a, "./talky-chat", &mut chat), 1);
    assert_eq!(deliver_notes(&a, "./cogny", &mut late), 1);
    let id = live_notes(&a.notes_into("./talky"))[0]["id"].clone();
    let open = chat.rows("SELECT text FROM notes");
    assert!(
        open.iter()
            .any(|r| r[0].as_str().unwrap_or("").contains(CLAIM)),
        "check of the setup: the chat's curator holds the note open: {open:?}"
    );

    // The first receipt.
    let s = seed(&mut talky, "call-2", "start", 0);
    assert_eq!(count_in(&s, CLAIM), 1, "{:?}", s.messages());
    a.clear();
    let ops_before = a.ops.len();
    assert_eq!(
        receipts_home(&mut a, "./talky", &talky),
        1,
        "the receipt reaches assistant through talky's rim"
    );
    assert!(
        a.ops.len() > ops_before,
        "the cell books the receipt in its own store: {:?}",
        a.ops
    );
    for child in ["./talky-chat", "./cogny"] {
        let notes = a.notes_into(child);
        assert!(
            live_notes(&notes).is_empty(),
            "no new note into {child}: {notes:?}"
        );
        let w = withdrawals(&notes);
        assert_eq!(w.len(), 1, "the copy in {child} is withdrawn: {notes:?}");
        assert_eq!(w[0]["id"], id, "the same note: {:?}", w[0]);
    }
    deliver_notes(&a, "./talky-chat", &mut chat);
    let call = turn_in(&mut chat, "chat-1", "c1", "hello");
    assert_eq!(
        count_in(&call, CLAIM),
        0,
        "withdrawn, the chat does not ask again: {:?}",
        texts(&call)
    );

    // A second receipt, as from a curator that showed it before the
    // withdrawal arrived: nothing new.
    a.clear();
    seed(&mut late, "call-9", "start", 0);
    let mut sent = 0;
    for r in late.routed("note_receipt") {
        for m in out_of("./cogny", &r) {
            sent += 1;
            a.leave("./cogny", m);
        }
    }
    assert_eq!(
        sent, 1,
        "check of the setup: a second receipt reaches the cell"
    );
    for child in COMPOSITES {
        assert!(
            live_notes(&a.notes_into(child)).is_empty(),
            "a second receipt sends no note into {child}: {:?}",
            a.into
        );
    }

    // The same claim again, the same day: closed.
    a.clear();
    a.leave("./talky", memory_out("text-1", "t7", twinge()));
    session_ends(&mut a, "./talky", "text-1");
    for child in COMPOSITES {
        assert!(
            live_notes(&a.notes_into(child)).is_empty(),
            "an asked-after concern does not open again: {:?}",
            a.into
        );
    }
}
