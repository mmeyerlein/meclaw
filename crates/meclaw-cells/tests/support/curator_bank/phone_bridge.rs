//! The wall `phone_bridge` of the bank of constructed windows (GH #1528,
//! d58-3 ballast, R-39-5, `support/curator_bank.rs`): one day on the phone,
//! two calls, in which the companion bridges the time a look-up takes four
//! times ("One moment, I am looking it up").
//!
//! WHY this wall exists: a bridge is no content, only a consequence of the
//! latency of the look-up (the owner, 14.11.). The curator makes it itself
//! (its side call beside the round's call) and marks it; once the result of
//! the look-up stands on the wall, the bridge is settled ballast, and the
//! next rebuild leaves it out so that no model refers to it later. The wall
//! counts what must happen to each bridge: the two bridges whose result
//! came, outside the newest `keep_recent` rounds, are garbage of the class
//! `ballast`; the bridge whose result never came stays raw; the bridge among
//! the newest rounds stays raw. Every round's words and every result are
//! talk.
//!
//! WHY a wall of its own and no seat: the walls of the nine seats carry no
//! bridge and their pins must not move. The rounds go through the lanes of
//! the hive (`WallBuilder::bridge_round`), so each bridge reaches the policy
//! as the answer of its side call, where the policy marks it. The texts are
//! made up.

use super::{Run, Wall, WallBuilder, day};

/// The two calls of the day, one session each.
pub const CALLS: [&str; 2] = ["phone-2026-03-02T09:00", "phone-2026-03-02T16:00"];

/// The `keep_recent` of the run of this wall (the example set `quick`): the
/// newest four rounds stay raw, the running turn among them.
pub const KEEP_RECENT: i64 = 4;

/// The keywords the statements name.
pub struct Keys {
    /// The bridges whose result stands on the wall, outside the newest
    /// rounds: ballast at the next rebuild.
    pub gone: Vec<String>,
    /// The words and the result of each round of `gone`, in the same order.
    pub gone_rounds: Vec<(String, String)>,
    /// The bridge whose look-up never answered.
    pub unanswered: String,
    /// The bridge among the newest rounds.
    pub recent: String,
}

/// The wall, without runs, and its keywords.
pub fn sown() -> (Wall, Keys) {
    let mut w = WallBuilder::new("phone_bridge", "brindle");
    let mut gone = Vec::new();
    let mut gone_rounds = Vec::new();

    // The first call, in the morning.
    let s = CALLS[0];
    w.at(day(0, 9, 0));
    let (words, bridge, answer) = w.bridge_round(
        true,
        s,
        "b1-1",
        "What will the weather be tomorrow, {k}?",
        "One moment, {k}, I am looking it up.",
        Some("Tomorrow stays dry, {k}, around eight degrees."),
    );
    gone.push(bridge);
    gone_rounds.push((words, answer));
    w.at(day(0, 9, 2));
    w.talk_round(
        s,
        "b1-2",
        "Good, then I will walk to the market, {k}.",
        "A fine plan, {k}. Take the warm scarf.",
    );
    w.at(day(0, 9, 4));
    let (_, unanswered, _) = w.bridge_round(
        false,
        s,
        "b1-3",
        "Until when is the pharmacy open today, {k}?",
        "Just a second, {k}, I am checking that for you.",
        None,
    );
    w.at(day(0, 9, 6));
    w.talk_round(
        s,
        "b1-4",
        "Never mind, {k}, I will ask the neighbour.",
        "All right, {k}.",
    );

    // The second call, in the afternoon.
    let s = CALLS[1];
    w.at(day(0, 16, 0));
    let (words, bridge, answer) = w.bridge_round(
        true,
        s,
        "b2-1",
        "When does the bus to the hospital leave, {k}?",
        "Let me look that up, {k}, one moment.",
        Some("The next bus leaves at twenty past four, {k}."),
    );
    gone.push(bridge);
    gone_rounds.push((words, answer));
    w.at(day(0, 16, 2));
    w.talk_round(
        s,
        "b2-2",
        "Then I will take that one, {k}.",
        "Good, {k}. You still have time for your coat.",
    );
    w.at(day(0, 16, 4));
    let (_, recent, _) = w.bridge_round(
        false,
        s,
        "b2-3",
        "And when does it come back, {k}?",
        "One moment, {k}, I am looking at the timetable.",
        Some("It comes back at ten to six, {k}."),
    );
    w.at(day(0, 16, 6));
    w.talk_round(
        s,
        "b2-4",
        "Thank you, {k}, that is enough for today.",
        "Gladly, {k}. Have a good trip.",
    );

    (
        w.build(day(0, 18, 0), CALLS[1]),
        Keys {
            gone,
            gone_rounds,
            unanswered,
            recent,
        },
    )
}

/// The run the statements of the bridges read: the example set `quick`
/// (`keep_recent` 4) as it ships -- no switch: the bridge is ballast from
/// the works --, the rebuild the next call makes after the cache ran out
/// between two calls (`Run::cold`: only the collection runs, the cover
/// stays, so every round of the day is in the raw part and what the window
/// shows of it is the collection's alone).
pub fn bridged_run() -> Run {
    Run::cold(&Run::of("quick"))
}

/// The wall with its one run.
pub fn wall() -> (Wall, Keys) {
    let (wall, keys) = sown();
    (wall.run(bridged_run()), keys)
}
