//! The wall `phone_day` of the bank of constructed windows (GH #1465,
//! `support/curator_bank.rs`): one day on the phone, four calls, with
//! weather and web lookups in them.
//!
//! What it counts: the results of the lookups of earlier rounds are garbage
//! of the class `shrunk_tools`; the weather of the last lookup was stamped
//! valid for half an hour and ran out before the rebuild (`transient`); one
//! web lookup failed and stays. The shape is the one of a real phone day
//! (one session per call, a tool round between the person's words and the
//! answer, the weather as the tool's JSON); the texts are made up.

use super::{Bank, Run, WallBuilder, What, day, filler, rfc};
use meclaw_core::serde_json::json;

const CALLS: [&str; 4] = [
    "phone-2026-03-02T08:10",
    "phone-2026-03-02T11:30",
    "phone-2026-03-02T14:05",
    "phone-2026-03-02T17:20",
];

/// A weather result in the form of the tool: one JSON line, the sentence to
/// say first, the keyword last.
fn weather(place: &str, at: &str, temp: f64, key: &str) -> String {
    json!({
        "summary": format!("{place}: now {temp} C, partly cloudy; today 1 C to 7 C, overcast, \
                            0.0 mm of rain."),
        "place": {"name": place, "country": "Northland", "admin1": "Coast",
                  "latitude": 52.52, "longitude": 13.41},
        "timezone": "UTC",
        "current": {"time": at, "temp": temp, "weather_code": 2, "condition": "partly cloudy"},
        "days": [{"date": "2026-03-02", "temp_min": 1.1, "temp_max": 7.4,
                  "precipitation_mm": 0.0, "weather_code": 3, "condition": "overcast"}],
        "fetched_at": at,
        "source": "open-meteo",
        "zz_ref": key,
    })
    .to_string()
}

/// A web result: eight hits with long snippets, the keyword last.
fn web(query: &str, seed: u64, key: &str) -> String {
    let hits: Vec<_> = (0..8)
        .map(|i| {
            json!({"title": format!("{query}, part {}", i + 1),
                   "url": format!("https://example.org/{seed}/{i}"),
                   "snippet": filler(seed * 31 + i, 14)})
        })
        .collect();
    json!({"op": "search", "query": query, "results": hits, "zz_ref": key}).to_string()
}

/// Registers the walls of this seat.
pub fn register(bank: &mut Bank) {
    let mut w = WallBuilder::new("phone_day", "vellum");
    let junk = What::Junk("shrunk_tools");

    // The first call, in the morning.
    let s = CALLS[0];
    w.at(day(0, 8, 10));
    w.words(
        s,
        "p1-1",
        "Good morning, it is me again, {k}. Are you there?",
    );
    w.answer(
        s,
        "p1-1",
        "Good morning. I am here and listening, {k}. What is on today?",
    );
    w.at(day(0, 8, 12));
    w.words(
        s,
        "p1-2",
        "Should I take the umbrella to the market in Harbour Town, {k}?",
    );
    w.tool(
        junk.clone(),
        s,
        "p1-2",
        "weather",
        json!({"place": "Harbour Town"}),
        None,
        |k| weather("Harbour Town", "2026-03-02T08:12", 1.7, k),
    );
    w.answer(
        s,
        "p1-2",
        "No umbrella needed, {k}: it stays dry, cold and overcast until the evening.",
    );
    w.at(day(0, 8, 15));
    w.words(s, "p1-3", "Good. Then I will walk, {k}. Talk to you later.");
    w.answer(
        s,
        "p1-3",
        "Enjoy the walk, {k}. I am here when you call again.",
    );

    // The second call, before noon: one lookup that answers, one that fails.
    let s = CALLS[1];
    w.at(day(0, 11, 30));
    w.words(
        s,
        "p2-1",
        "I am back, {k}. When does the ferry to the outer quay leave this week?",
    );
    w.tool(
        junk.clone(),
        s,
        "p2-1",
        "web",
        json!({"op": "search", "args": {"q": "ferry outer quay"}}),
        None,
        |k| web("ferry timetable outer quay", 11, k),
    );
    w.answer(
        s,
        "p2-1",
        "The ferry leaves at ten and at four on weekdays, {k}, and only at ten on Saturday.",
    );
    w.at(day(0, 11, 34));
    w.words(
        s,
        "p2-2",
        "And is the fish stall of the Marlow brothers open on Saturday, {k}?",
    );
    w.tool(
        What::Error,
        s,
        "p2-2",
        "web",
        json!({"op": "search", "args": {"q": "Marlow fish stall"}}),
        None,
        |k| {
            json!({"error": {"code": "timeout", "message": "the page did not answer in time",
                         "ref": k},
               "error_code": "timeout"})
            .to_string()
        },
    );
    w.answer(
        s,
        "p2-2",
        "I could not reach the page of the stall just now, {k}. Shall I try again later?",
    );
    w.at(day(0, 11, 37));
    w.words(s, "p2-3", "Yes, try again in the afternoon, {k}.");
    w.answer(s, "p2-3", "I will, {k}.");

    // The third call, in the afternoon.
    let s = CALLS[2];
    w.at(day(0, 14, 5));
    w.words(
        s,
        "p3-1",
        "Hello again, {k}. Has the weather turned? It looks darker from my window.",
    );
    w.tool(
        junk.clone(),
        s,
        "p3-1",
        "weather",
        json!({"place": "Harbour Town"}),
        None,
        |k| weather("Harbour Town", "2026-03-02T14:05", 4.9, k),
    );
    w.answer(
        s,
        "p3-1",
        "It is still dry, {k}, only the cloud is thicker. Nearly five degrees now.",
    );
    w.at(day(0, 14, 8));
    w.words(
        s,
        "p3-2",
        "Then look for the fish stall again, please, {k}.",
    );
    w.tool(
        junk.clone(),
        s,
        "p3-2",
        "web",
        json!({"op": "search", "args": {"q": "Marlow fish stall hours"}}),
        None,
        |k| web("Marlow brothers fish stall opening hours", 23, k),
    );
    w.answer(
        s,
        "p3-2",
        "The stall is open on Saturday from eight to one, {k}.",
    );
    w.at(day(0, 14, 11));
    w.words(s, "p3-3", "Wonderful, {k}. I will go early then.");
    w.answer(
        s,
        "p3-3",
        "Early is best, {k}: the smoked fish is gone by eleven, they write.",
    );

    // The fourth call, towards the evening.
    let s = CALLS[3];
    w.at(day(0, 17, 20));
    w.words(
        s,
        "p4-1",
        "One more thing before supper, {k}. I am thinking of the concert on Sunday.",
    );
    w.answer(
        s,
        "p4-1",
        "The one in the old customs hall, {k}? Tell me what you want to know.",
    );
    w.at(day(0, 17, 22));
    w.words(
        s,
        "p4-2",
        "When do the doors open, and is there a bus back afterwards, {k}?",
    );
    w.tool(
        junk,
        s,
        "p4-2",
        "web",
        json!({"op": "search", "args": {"q": "customs hall concert Sunday"}}),
        None,
        |k| web("customs hall concert Sunday doors and night bus", 37, k),
    );
    w.answer(
        s,
        "p4-2",
        "Doors open at six, {k}, and the night bus leaves the quay at half past ten.",
    );
    w.at(day(0, 17, 25));
    w.words(s, "p4-3", "And will I need the warm coat tonight, {k}?");
    // The last lookup of the day: its round is the one a rebuild keeps raw,
    // and the tool stamped it valid until 17:50.
    w.tool(
        What::Junk("transient"),
        s,
        "p4-3",
        "weather",
        json!({"place": "Harbour Town"}),
        Some(day(0, 17, 50)),
        |k| weather("Harbour Town", &rfc(day(0, 17, 25)), 2.3, k),
    );
    w.answer(
        s,
        "p4-3",
        "Take the warm coat, {k}: two degrees now and falling.",
    );

    bank.wall(
        w.build(day(0, 18, 0), CALLS[3])
            .target(5000)
            .run(Run::of("talky"))
            .run(Run::of("quick"))
            .run(Run::of("quick").with("gc", json!("1"))),
    );
}
