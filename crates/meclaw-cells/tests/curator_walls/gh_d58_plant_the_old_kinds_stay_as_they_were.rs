//! GH #1490 (D58 plant) -- the planted promises, open points and facts of the
//! wall `over_aim` stay as they were before the kinds `third_party` and
//! `superseded` came in.
//!
//! WHY this file exists: the bank's counts and the tests over the wall read
//! the old points by their keywords. The wall picks the kind of a planting
//! beat by rotation (`KINDS[(turn_no / 8) % KINDS.len()]`), so a longer list
//! of kinds there would move every old point to another row and another
//! count. The new kinds go into beats of their own and keep the length of
//! the row they take, so the days and the turns of the wall stay.
//!
//! Pinned: the list of the old points -- kind, keyword, day, in wall order --
//! is the one of master cdc70f2b2 (39 points, 13 of each kind), and the
//! sentences of all of them hash to the value taken there.

// harness_fold: #[path = "support/curator_bank.rs"]
#[allow(unused_imports)]
use crate::curator_bank; // harness_fold: mod curator_bank;
// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_bank::*;
use curator_hive::sha256_hex;

/// The old points of master cdc70f2b2: kind, keyword, day.
const BEFORE: [(&str, &str, i64); 39] = [
    ("promise", "quillon-006", -3),
    ("open", "quillon-021", -3),
    ("fact", "quillon-037", -3),
    ("promise", "quillon-054", -2),
    ("open", "quillon-069", -2),
    ("fact", "quillon-085", -2),
    ("promise", "quillon-102", -2),
    ("open", "quillon-117", -2),
    ("fact", "quillon-133", -2),
    ("promise", "quillon-150", -1),
    ("open", "quillon-165", -1),
    ("fact", "quillon-181", -1),
    ("promise", "quillon-198", -1),
    ("open", "quillon-213", -1),
    ("fact", "quillon-229", -1),
    ("promise", "quillon-246", -1),
    ("open", "quillon-261", -1),
    ("fact", "quillon-277", -1),
    ("promise", "quillon-294", -1),
    ("open", "quillon-309", -1),
    ("fact", "quillon-325", -1),
    ("promise", "quillon-342", -1),
    ("open", "quillon-357", -1),
    ("fact", "quillon-373", -1),
    ("promise", "quillon-390", -1),
    ("open", "quillon-405", 0),
    ("fact", "quillon-421", 0),
    ("promise", "quillon-438", 0),
    ("open", "quillon-453", 0),
    ("fact", "quillon-469", 0),
    ("promise", "quillon-486", 0),
    ("open", "quillon-501", 0),
    ("fact", "quillon-517", 0),
    ("promise", "quillon-534", 0),
    ("open", "quillon-549", 0),
    ("fact", "quillon-565", 0),
    ("promise", "quillon-582", 0),
    ("open", "quillon-597", 0),
    ("fact", "quillon-613", 0),
];

/// sha256 over `<kind>|<keyword>|<day>|<sentence>\n` of every old point of
/// master cdc70f2b2, in wall order.
const SENTENCES_SHA256: &str = "9e15698d208c52fc33cdfa6ab79e8214c61a6eb46269078ff5ba752b41bf12c8";

#[test]
fn the_old_kinds_stay_as_they_were() {
    let old: Vec<_> = over_aim::planted()
        .into_iter()
        .filter(|p| ["promise", "open", "fact"].contains(&p.kind))
        .collect();
    let now: Vec<(&str, &str, i64)> = old
        .iter()
        .map(|p| (p.kind, p.key.as_str(), p.day))
        .collect();
    assert_eq!(
        now,
        BEFORE.to_vec(),
        "over_aim: the old planted points (kind, keyword, day) are those of master"
    );
    let listing: String = old
        .iter()
        .map(|p| format!("{}|{}|{}|{}\n", p.kind, p.key, p.day, p.text))
        .collect();
    assert_eq!(
        sha256_hex(&listing),
        SENTENCES_SHA256,
        "over_aim: the sentences of the old planted points are those of master:\n{listing}"
    );
}
