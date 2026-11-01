//! GH #1367 -- on the stamp way the turn and its stamp meet, though they never share an id.
//!
//! In a colony the turn reaches the presenter through the apps fan-out, before the session
//! keeper, so it carries no `turn_id` (the stage keys it `t-<now>`); the stamp arrives on
//! `in_verdict` under the keeper's id (`keeper:<trace>:<ts>`). Measured in a lab colony on
//! the stamp way: the window opened under construction, no `in_show op: data` ever left,
//! and the window was withdrawn at `data_wait_ms`. Here the apps level fans the turn out
//! to `./presenter` and a relay stands in for the keeper and the talky (`verdict` →
//! `in_verdict`); the app stub is the egress of `/alex`. Both orders: the turn first (the
//! measured one), then a stamp that comes before its words.

#[path = "support/display_colony.rs"]
mod display_colony;
use crate::display_colony::presenter_colony;

use meclaw_core::serde_json::{Value, json};
use presenter_colony::{Dials, body, boot, guard, warm};

fn stamp(turn_id: &str) -> Value {
    json!({"turn_id": turn_id, "session_id": "s1", "channel": "chat", "path": "simple",
           "intent": "show", "choices": {},
           "screen": {"topic": "sample", "confidence": 0.95, "lead": "rows",
                      "further": "none"}})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_keeper_stamp_meets_the_fanned_out_turn() {
    if !guard("a_keeper_stamp_meets_the_fanned_out_turn") {
        return;
    }
    let mut s = boot(Dials {
        stamp_way: true,
        ..Dials::default()
    })
    .await;
    warm(&s).await;
    s.install_sample().await;

    // The turn first, without a turn id; then the stamp under the keeper's.
    let first = "keeper:01a18cc2:2026-10-30T12:22:23.516841Z";
    s.fanned_turn("show me a sample").await;
    s.keeper_stamp(stamp(first)).await;
    let ask = s.out("in_show").await;
    let b = body(&ask);
    assert_eq!(b["op"], json!("data"), "the data request: {b:#}");
    assert_eq!(b["turn_id"], json!(first), "under the stamp's id: {b:#}");
    assert_eq!(
        b["request"],
        json!("show me a sample"),
        "with the turn's words: {b:#}"
    );

    // The stamp first; the words come after it, again without a turn id.
    let second = "keeper:01a18cc2:2026-10-30T12:22:31.002113Z";
    s.keeper_stamp(stamp(second)).await;
    s.fanned_turn("and another sample").await;
    let ask = s.out("in_show").await;
    let b = body(&ask);
    assert_eq!(
        (b["op"].clone(), b["turn_id"].clone(), b["request"].clone()),
        (json!("data"), json!(second), json!("and another sample")),
        "the stamp waited for the words: {b:#}"
    );
    s.c.shutdown().await;
}
