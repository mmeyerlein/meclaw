//! The wall `long_chat` of the bank of constructed windows (GH #1465,
//! `support/curator_bank.rs`): sixty turns of text on one day, a recall
//! pushed in front of each.
//!
//! What it counts: every recall result names two rows, one of its own and
//! one it shares with the two other turns of its group of three. Youngest
//! first, the two older results of a group repeat a row a younger one holds
//! (`dup_recall`, 40 of them); the youngest of each group repeats nothing
//! and is an old tool result like any other (`shrunk_tools`, 19), but for
//! the one of the last turn, whose round a rebuild keeps raw. The shape is
//! the memory's own rendering: the document head, the section head, a
//! caveat in parentheses, the rows at an indent of two, and quotes that run
//! over several lines.

use super::{Bank, Run, WallBuilder, What, day, filler};
use meclaw_core::serde_json::json;

const SESSION: &str = "chat-2026-03-02T08:00";
const TURNS: usize = 60;

/// A recall result as the memory hive renders it. `group` names the row the
/// turns of a group share, `turn` the row only this result holds; the
/// keyword stands inside the quote of that row, on its second line.
fn recall(group: usize, turn: usize, key: &str) -> String {
    [
        "WHAT THIS MEMORY HOLDS (as of 2026-03-02)".to_string(),
        "Not everything that matches is here -- capped legs (possibly more exist): semantic at \
         40 rows; the fusion cut at TOPK=5."
            .to_string(),
        "WHAT WAS SAID (verbatim, not interpreted)".to_string(),
        "  (an earlier answer claiming something is NOT stored is evidence about that answer \
         and nothing else -- never a measurement of this memory)"
            .to_string(),
        format!(
            "  person on 2026-02-27: \"The spare key of shed number {group} hangs behind the \
             blue door.\""
        ),
        format!(
            "  assistant on 2026-02-28: \"About point {turn} of the list we said this, {}",
            filler(turn as u64, 6)
        ),
        format!(
            "and then we went through it once more ({key}), {}",
            filler(turn as u64 + 500, 6)
        ),
        format!(
            "so nothing of it is open any more. {}\"",
            filler(turn as u64 + 900, 3)
        ),
    ]
    .join("\n")
}

/// Registers the walls of this seat.
pub fn register(bank: &mut Bank) {
    let mut w = WallBuilder::new("long_chat", "tamarisk");
    for i in 1..=TURNS {
        let turn = format!("c{i:02}");
        let group = (i - 1) / 3;
        // The youngest turn of a group is the third; the last turn of the
        // day is the round a rebuild keeps raw.
        let what = if i == TURNS {
            What::Other
        } else if i % 3 == 0 {
            What::Junk("shrunk_tools")
        } else {
            What::Junk("dup_recall")
        };
        w.at(day(0, 8, 0) + chrono::Duration::minutes(10 * (i as i64 - 1)));
        w.recall(
            what,
            SESSION,
            &turn,
            &format!("point {i} of the list"),
            |k| recall(group, i, k),
        );
        w.words(
            SESSION,
            &turn,
            &format!("Let us go on with point {i} of the list, {{k}}. What did we settle there?"),
        );
        w.answer(
            SESSION,
            &turn,
            &format!(
                "Point {i} is settled, {{k}}: we agreed on it two days ago and went through it \
                 again yesterday.\n\n```sidecar\n{{\"memory\":{{\"nothing_new\":true,\"facts\":[],\
                 \"topic\":{{\"movement\":\"continue\"}}}}}}\n```"
            ),
        );
    }
    bank.wall(
        w.build(day(0, 18, 0), SESSION)
            .target(20_000)
            .run(Run::of("talky"))
            .run(Run::of("quick"))
            .run(Run::of("quick").with("gc", json!("1"))),
    );
}
