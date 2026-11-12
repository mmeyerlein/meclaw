//! The wall `research` of the bank of constructed windows (GH #1465,
//! `support/curator_bank.rs`): few turns, many long search results.
//!
//! What it counts: five rounds of one question each, every round with six
//! long results of the web search. The results of the first four rounds lie
//! outside `keep_rounds` when the rebuild plans and are garbage of the class
//! `shrunk_tools`; the six of the last round stay whole. The talk is ten
//! short rows; all the weight of the wall is in the results.

use super::{Bank, Run, WallBuilder, What, day, filler};
use chrono::Duration;
use meclaw_core::serde_json::json;

const SESSION: &str = "research-2026-03-02T10:00";

/// The rounds of the wall and the results of each.
pub const ROUNDS: usize = 5;
pub const RESULTS: usize = 6;

const QUESTIONS: [&str; ROUNDS] = [
    "the history of the outer quay",
    "the ferry lines of the last decade",
    "the closing of the old bridge",
    "the fares of the night bus",
    "the plans for the new pier",
];

/// Registers the walls of this seat.
pub fn register(bank: &mut Bank) {
    let mut w = WallBuilder::new("research", "saffron");
    let s = SESSION;
    for (r, question) in QUESTIONS.iter().enumerate() {
        let turn = format!("s{:02}", r + 1);
        w.at(day(0, 10, 0) + Duration::minutes(10 * r as i64));
        w.words(
            s,
            &turn,
            &format!("Question {{k}}: find what is written about {question}."),
        );
        let what = if r + 1 < ROUNDS {
            What::Junk("shrunk_tools")
        } else {
            What::Other
        };
        for n in 0..RESULTS {
            w.tool(
                what.clone(),
                s,
                &turn,
                "web",
                json!({"op": "search", "query": format!("{question}, source {n}")}),
                None,
                |k| {
                    format!(
                        "Search result {n} on {question}:\n{}\nReference {k}.",
                        filler((10 * r + n) as u64 + 900, 100)
                    )
                },
            );
        }
        w.answer(
            s,
            &turn,
            &format!("On {question}, {{k}}: the sources agree on the main dates."),
        );
    }

    bank.wall(
        w.build(day(0, 11, 0), SESSION)
            .target(30_000)
            .run(Run::of("research"))
            .run(Run::of("research").with("gc", json!("1")))
            .one_role(
                "few turns under many long search results fit the role made for such a search \
                 and no other; the second run sets gc to 1, so the collection is shown for the \
                 role although its preset stays 0",
            ),
    );
}
