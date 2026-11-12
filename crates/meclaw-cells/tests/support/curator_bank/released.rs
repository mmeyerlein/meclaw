//! The wall `released` of the bank of constructed windows (GH #1465,
//! `support/curator_bank.rs`): one written conversation in which the model
//! trimmed its own window through the sidecar of an answer.
//!
//! What it counts: five old tool results, each of a round of its own. The
//! model released two of them (`release` marks of a later round): they are
//! garbage of the class `released` and stand in the window as the one line
//! `[#id released -- history_read("#id")]`. It pinned two (`pin` marks of the
//! same round): they stay whole, although their rounds lie outside
//! `keep_rounds` and a rebuild that collects would shorten them; one of the
//! two was released and pinned in the same answer, and the pin wins. The
//! fifth nobody named: an old result like any other (`shrunk_tools`).
//!
//! The marks are written the way `./intake` writes the `window` section of a
//! sidecar (`{"window": {"release": ["#id"], "pin": ["#id"]}}`): one `marks`
//! row per id, kind `release` or `pin`, value the id, under the round of the
//! answer that said it. The ids are read off the ledger, as the model reads
//! them off its window.

use super::{Bank, Hive, Run, Wall, WallBuilder, What, day, filler};
use meclaw_core::serde_json::json;

const SESSION: &str = "chat-2026-03-02T10:00";

/// The word in the text of a result the model released.
pub const RELEASE_TAG: &str = "ledger-slip-released";
/// The word in the text of a result the model pinned.
pub const PIN_TAG: &str = "ledger-slip-pinned";

/// A search result: one line, the tags of the trim in its head, the keyword
/// last.
fn result(topic: &str, tags: &str, seed: u64, sentences: usize, key: &str) -> String {
    format!(
        "Search result on {topic} ({tags}): {} Reference {key}.",
        filler(seed, sentences)
    )
}

/// The pieces of `wall` the model pinned.
pub fn pinned(wall: &Wall) -> Vec<usize> {
    (0..wall.pieces.len())
        .filter(|i| {
            let p = &wall.pieces[*i];
            p.el["type"] == "tool_result" && p.text().contains(PIN_TAG)
        })
        .collect()
}

/// The pieces of `wall` the model released and did not pin.
pub fn released(wall: &Wall) -> Vec<usize> {
    wall.counted(|w| matches!(w, What::Junk("released")))
}

/// The short ids of the tool results whose text holds `tag`, read off the
/// ledger.
fn ids_with(h: &Hive, tag: &str) -> Vec<String> {
    h.rows(&format!(
        "SELECT w.hash FROM wall w JOIN blocks b ON b.hash = w.hash \
         WHERE w.kind = 'tool_result' AND b.body LIKE '%{tag}%' ORDER BY w.seq"
    ))
    .into_iter()
    .map(|r| r[0].as_str().unwrap_or("")[..12].to_string())
    .collect()
}

/// The trim of the model: said in the answer of turn `r11`, one mark per id.
fn trim(h: &mut Hive) {
    let said = day(0, 10, 50);
    let mut n = 0;
    for (kind, tag) in [("release", RELEASE_TAG), ("pin", PIN_TAG)] {
        let ids = ids_with(h, tag);
        assert!(!ids.is_empty(), "the wall holds a result with {tag}");
        for id in ids {
            n += 1;
            h.mark(
                said + chrono::Duration::milliseconds(500 + n),
                SESSION,
                "r11",
                kind,
                &format!("#{id}"),
            );
        }
    }
}

/// Registers the walls of this seat.
pub fn register(bank: &mut Bank) {
    let mut w = WallBuilder::new("released", "tessera");
    let s = SESSION;

    w.at(day(0, 10, 0));
    w.words(s, "r01", "Good morning, {k}. I am planning the trip north.");
    w.answer(s, "r01", "Good morning. Tell me what you need for it, {k}.");

    // Five old results, one per round.
    let olds: [(&str, &str, What, String); 5] = [
        (
            "r02",
            "ferry crossings",
            What::Junk("released"),
            RELEASE_TAG.to_string(),
        ),
        (
            "r03",
            "night buses",
            What::Junk("released"),
            RELEASE_TAG.to_string(),
        ),
        (
            "r04",
            "the harbour office",
            What::Other,
            PIN_TAG.to_string(),
        ),
        (
            "r05",
            "the tide table",
            What::Other,
            format!("{RELEASE_TAG}, {PIN_TAG}"),
        ),
        (
            "r06",
            "the upper market",
            What::Junk("shrunk_tools"),
            "no slip".to_string(),
        ),
    ];
    for (i, (turn, topic, what, tags)) in olds.into_iter().enumerate() {
        w.at(day(0, 10, 5) + chrono::Duration::minutes(5 * i as i64));
        w.words(s, turn, &format!("Please look up {topic} for me, {{k}}."));
        let sentences = if tags.contains(PIN_TAG) { 25 } else { 80 };
        w.tool(
            what,
            s,
            turn,
            "web",
            json!({"op": "search", "query": topic}),
            None,
            |k| result(topic, &tags, 40 + i as u64, sentences, k),
        );
        w.answer(
            s,
            turn,
            &format!("I have read up on {topic}, {{k}}. Ask me what you want to know."),
        );
    }

    // Plain turns; the answer of `r11` carried the trim.
    for i in 7..=11 {
        let turn = format!("r{i:02}");
        w.at(day(0, 10, 30) + chrono::Duration::minutes(5 * (i - 7)));
        w.words(
            s,
            &turn,
            &format!("Point {i} of my list, {{k}}: is there anything I forgot to pack?"),
        );
        w.answer(
            s,
            &turn,
            &format!("For point {i} the rain coat is enough, {{k}}."),
        );
    }

    // The youngest tool round: within `keep_rounds`, it stays as it came.
    w.at(day(0, 10, 55));
    w.words(s, "r12", "And the last train back, {k}?");
    w.tool(
        What::Other,
        s,
        "r12",
        "web",
        json!({"op": "search", "query": "last train back"}),
        None,
        |k| result("the last train back", "no slip", 77, 12, k),
    );
    w.answer(s, "r12", "The last train leaves at ten past nine, {k}.");

    bank.wall(
        w.build(day(0, 11, 0), SESSION)
            .target(4000)
            .prepare(trim)
            .run(Run::of("talky"))
            .run(Run::of("quick"))
            .run(Run::of("quick").with("gc", json!("1"))),
    );
}
