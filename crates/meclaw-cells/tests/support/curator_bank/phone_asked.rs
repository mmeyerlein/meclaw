//! The wall `phone_asked` of the bank of constructed windows (GH #1528,
//! d58-3 ballast, `support/curator_bank.rs`): one day on the phone, four
//! calls, in which the person asks the time three times and the weather
//! twice, and says once how they are.
//!
//! WHY this wall exists: a question asked many times a day matters later
//! only as the fact that it was asked (the owner's words on the curator,
//! 11.11., in translation: "it only matters to know that it was asked"). With the judge of
//! the ballast on, the policy asks the judge about every closed round, and
//! a round the judge calls `asked` becomes one line at the next rebuild. The
//! wall counts what must happen to each round: the four asked rounds outside
//! the newest `keep_recent` rounds are garbage of the class `ballast`; the
//! asked round among the newest stays raw and is counted as talk; how the
//! person is, a date with the daughter and every other round are talk the
//! judge keeps.
//!
//! WHY a wall of its own and no seat: its runs switch the judge on, the
//! walls of the nine seats run with the shipped switch (off), and their
//! pins must not move. The rounds are no rows written into the ledger but
//! rounds through the lanes of the hive (`WallBuilder::talk_round`), so the
//! tap that closes each of them is where the policy asks the judge. The
//! texts are made up.

use super::{Run, Wall, WallBuilder, What, day};
use meclaw_core::serde_json::json;

/// The four calls of the day, one session each.
pub const CALLS: [&str; 4] = [
    "phone-2026-03-02T08:10",
    "phone-2026-03-02T11:30",
    "phone-2026-03-02T14:05",
    "phone-2026-03-02T17:20",
];

/// The lines the judge of the wall gives the asked rounds, in wall order.
pub const LINES: [&str; 5] = [
    "asked the time in the morning",
    "asked about the weather for the day",
    "asked the time before noon",
    "asked whether it would rain in the afternoon",
    "asked the time in the evening",
];

/// The `keep_recent` of the runs of this wall (the example set `quick`):
/// the newest four rounds stay raw, the running turn among them.
pub const KEEP_RECENT: i64 = 4;

/// The keywords of the person's words of the rounds the statements name.
pub struct Keys {
    /// The asked rounds outside the newest rounds, in wall order.
    pub asked_out: Vec<String>,
    /// The asked round among the newest rounds.
    pub asked_recent: String,
    /// How the person is.
    pub wellbeing: String,
    /// A date the person names.
    pub date: String,
}

/// The wall, without runs, and the keywords of its rounds.
pub fn sown() -> (Wall, Keys) {
    let mut w = WallBuilder::new("phone_asked", "quillet");
    let mut asked_out = Vec::new();

    // The first call, in the morning.
    let s = CALLS[0];
    w.at(day(0, 8, 10));
    asked_out.push(w.asked_round(
        false,
        s,
        "a1-1",
        "Good morning, it is me, {k}. What time is it now?",
        "Good morning, {k}. It is ten past eight.",
        LINES[0],
    ));
    w.at(day(0, 8, 11));
    let wellbeing = w.talk_round(
        s,
        "a1-2",
        "I slept badly, {k}, and my left knee hurts again since yesterday.",
        "I am sorry to hear that, {k}. Shall we note the knee for the doctor on Thursday?",
    );
    w.at(day(0, 8, 13));
    asked_out.push(w.asked_round(
        false,
        s,
        "a1-3",
        "What is the weather going to be today, {k}?",
        "Dry and cold, {k}, about five degrees at noon.",
        LINES[1],
    ));

    // The second call, before noon.
    let s = CALLS[1];
    w.at(day(0, 11, 30));
    asked_out.push(w.asked_round(
        false,
        s,
        "a2-1",
        "Is it already half past eleven, {k}?",
        "Yes, {k}, it is half past eleven.",
        LINES[2],
    ));
    w.at(day(0, 11, 31));
    let date = w.talk_round(
        s,
        "a2-2",
        "My daughter comes on Sunday at three, {k}. Please remind me in the morning.",
        "I will remind you on Sunday morning, {k}.",
    );

    // The third call, in the afternoon.
    let s = CALLS[2];
    w.at(day(0, 14, 5));
    asked_out.push(w.asked_round(
        false,
        s,
        "a3-1",
        "Will it rain this afternoon, {k}?",
        "No rain is expected, {k}, only clouds.",
        LINES[3],
    ));
    w.at(day(0, 14, 6));
    w.talk_round(
        s,
        "a3-2",
        "Then I will walk to the bakery, {k}.",
        "Good idea, {k}. Take the warm scarf.",
    );

    // The fourth call, towards the evening: its last rounds are the newest.
    let s = CALLS[3];
    w.at(day(0, 17, 20));
    w.talk_round(
        s,
        "a4-1",
        "I am back from the bakery, {k}. The bread was still warm.",
        "Lovely, {k}. Did you get the rye loaf?",
    );
    w.at(day(0, 17, 21));
    w.talk_round(
        s,
        "a4-2",
        "Yes, the rye loaf, {k}. And a small cake for Sunday.",
        "Your daughter will like that, {k}.",
    );
    w.at(day(0, 17, 22));
    let asked_recent = w.asked_round(
        true,
        s,
        "a4-3",
        "What time is it now, {k}?",
        "It is twenty-two minutes past five, {k}.",
        LINES[4],
    );
    w.at(day(0, 17, 23));
    w.talk_round(
        s,
        "a4-4",
        "Thank you, {k}. I will make tea now.",
        "Enjoy your tea, {k}.",
    );

    (
        w.build(day(0, 18, 0), CALLS[3]),
        Keys {
            asked_out,
            asked_recent,
            wellbeing,
            date,
        },
    )
}

/// The run the statements of the ballast read: the example set `quick`
/// (`keep_recent` 4), the judge on, the rebuild the next call makes after
/// the cache ran out between two calls (`Run::cold`: only the collection
/// runs, the cover stays, so every round of the day is in the raw part and
/// what the window shows of it is the collection's alone).
pub fn judged_run() -> Run {
    Run::cold(&Run::of("quick").with(super::JUDGE_SWITCH, json!("1")))
}

/// The wall with its one run.
pub fn wall() -> (Wall, Keys) {
    let (wall, keys) = sown();
    (wall.run(judged_run()), keys)
}

/// GH #1528 (review I-1, re-check I-neu-1, M-neu-2): the lines the judge
/// gives the asked rounds of the wall `phone_asked_beside`, in wall order.
pub const BESIDE_LINES: [&str; 5] = [
    "asked about the weather in the morning",
    "asked the time in the morning",
    "asked which day it is",
    "asked the time once more",
    "asked whether the post had come",
];

/// The keywords of the wall `phone_asked_beside`.
pub struct BesideKeys {
    /// The words of the weather round asked through the tool `weather`.
    pub weather: String,
    /// The words of the round asked beside a timer frame.
    pub time: String,
    /// The timer frame beside those words: nobody's words, never judged.
    pub timer: String,
    /// Re-check M-neu-2: the words of the round the curator pushed a recall
    /// in front of -- the round's first row is a tool call.
    pub led: String,
    /// Re-check I-neu-1: the words of the round with a row after the
    /// judge's verdict, and that row.
    pub late_words: String,
    pub late: String,
    /// Re-check I-neu-1: the words of the round whose answer's block the
    /// judge's read does not get, and that answer.
    pub unread_words: String,
    pub unread: String,
}

/// GH #1528 (review I-1): one call of the morning in which the person asks
/// the weather -- the companion answers through the tool `weather`, so the
/// round holds the call and its result -- and then the time, in the same
/// message as a timer that rang (`[timer tea-1] ...`). Four rounds
/// follow at the end, the newest `keep_recent`, so every asked round stands outside
/// them. What the judge saw of a round is what its one line takes: the
/// weather round whole, tool rows included; of the time round the words and
/// the answer, while the timer frame stays raw in its place.
///
/// Re-check of the review (I-neu-1, M-neu-2): three more asked rounds
/// stand outside the newest -- one the curator pushed a recall in front of
/// (its first row a tool call; the line takes the recall too), one with a
/// sentence of the companion written after the judge's verdict (the judge
/// never saw it: the round stays raw), and one whose answer's block the
/// judge's read does not get (no request: the round stays raw). The first
/// of the newest rounds asks the weather of tomorrow through the tool: the
/// newest tool round is the one `keep_rounds` 1 keeps whole, so the
/// collection of the cold rebuild shrinks the results of the weather round
/// and of the recall before the ballast takes them.
pub fn sown_beside() -> (Wall, BesideKeys) {
    let mut w = WallBuilder::new("phone_asked_beside", "tamwick");
    let s = CALLS[0];
    w.at(day(0, 8, 10));
    let weather = w.asked_tool_round(
        false,
        s,
        "b1-1",
        "What is the weather going to be today, {k}?",
        "weather",
        json!({"place": "home", "day": "today"}),
        |k| format!("Dry and cold, five degrees at noon, no rain ({k})."),
        "Dry and cold today, five degrees at noon, {k}.",
        BESIDE_LINES[0],
    );
    w.at(day(0, 8, 12));
    let (time, timer) = w.asked_round_beside(
        false,
        s,
        "b1-2",
        "And what time is it now, {k}?",
        "[timer tea-1] The tea timer rang, {k}.",
        "It is twelve past eight, {k}, and your tea is ready.",
        BESIDE_LINES[1],
    );
    w.at(day(0, 8, 14));
    let led = w.asked_round_led_by_recall(
        s,
        "b1-3",
        "which day is it",
        |k| format!("[About the calendar: today is Monday, the second of March ({k}).]"),
        "Which day is it today, {k}?",
        "It is Monday, the second of March, {k}.",
        BESIDE_LINES[2],
    );
    w.at(day(0, 8, 16));
    let (late_words, late) = w.asked_round_with_late_row(
        s,
        "b1-4",
        "What time is it now, {k}? I lost track.",
        "It is sixteen past eight, {k}.",
        "And your sister said she calls at ten, {k}.",
        BESIDE_LINES[3],
    );
    w.at(day(0, 8, 18));
    let (unread_words, unread) = w.asked_round_unread(
        s,
        "b1-5",
        "Has the post come yet, {k}?",
        "Not yet, {k}; it usually comes around ten.",
        BESIDE_LINES[4],
    );
    w.at(day(0, 8, 20));
    w.lane_round(
        What::Other,
        s,
        "b1-6",
        "And will it rain tomorrow, {k}?",
        "weather",
        json!({"place": "home", "day": "tomorrow"}),
        "",
        |k| format!("Light rain in the afternoon, eight degrees ({k})."),
        "Light rain tomorrow afternoon, {k}, take the umbrella.",
    );
    for (n, (words, answer)) in [
        (
            "The birds are back at the feeder, {k}.",
            "How lovely, {k}. Which ones?",
        ),
        (
            "Two blue tits and a robin, {k}.",
            "A fine morning visit, {k}.",
        ),
        (
            "I will call my sister later, {k}.",
            "Give her my regards, {k}.",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        w.at(day(0, 8, 22 + n as i64));
        w.talk_round(s, &format!("b1-{}", n + 7), words, answer);
    }
    (
        w.build(day(0, 9, 0), CALLS[0]),
        BesideKeys {
            weather,
            time,
            timer,
            led,
            late_words,
            late,
            unread_words,
            unread,
        },
    )
}

/// The wall `phone_asked_beside` with the run of the ballast.
pub fn wall_beside() -> (Wall, BesideKeys) {
    let (wall, keys) = sown_beside();
    (wall.run(judged_run()), keys)
}
