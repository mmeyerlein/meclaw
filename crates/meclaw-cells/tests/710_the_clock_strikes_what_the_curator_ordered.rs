//! The clock seam: the moments the curator ordered come BACK as passes (befund 04 § E.1).
//!
//! `compose.py` computes `strokes` and emits the order (§ 4.34), and every other display
//! lock in this tree can read that emission. What none of them can show is the other half:
//! that the `timer` cell beside the compose cell really keeps the order, really strikes at
//! that second, and that the strike really becomes a pass -- over real time, with nobody
//! writing anything. A screen whose clock never struck would look identical in every
//! subprocess test and would stand still on the wall.
//!
//! Anchors out of the 105 (befund 04 § B.2, seam "clock"): **S-002** and **S-016** (age and
//! rung after a pass nobody asked for), **S-045** and **S-072** (`strokes_ordered`),
//! **S-037** and **S-075** (a `ttl_ms` is taken off the screen by the pass, not by a write).
//!
//! The dials are small on purpose (`linger_ms` 2000, `fade_ms` 3000): what is measured is
//! that the strike arrives, not how long a screen waits.

#[path = "support/display_colony.rs"]
mod display_colony;

use std::time::Duration;

use display_colony::{Boot, boot, curator, have_python, library_ships};
use meclaw_core::serde_json::{Value, json};

const APP: &str = "/alex/apps/note";

/// A settle window, never a semantic discriminator.
const QUIET: Duration = Duration::from_millis(300);

/// Whether the state still holds `oid` as a present window (§ 4.11).
fn present(state: &Value, oid: &str) -> bool {
    state["views"]
        .get(oid)
        .is_some_and(|v| v["curator"]["present"] == json!(true))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_clock_strikes_what_the_curator_ordered() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let colony = boot(Boot {
        linger_ms: 2000,
        fade_ms: 3000,
        ..Boot::default()
    })
    .await;
    let note = colony.oid(APP, "note");
    let flash = colony.oid(APP, "flash");

    // ONE write, and then the test stops writing. Everything that follows is the clock's.
    let fresh = colony
        .put(
            APP,
            "note",
            json!({"title": "Note", "context": "work", "relevance": "0.9",
                   "topic": "note:1", "touched": "1"}),
        )
        .await;
    assert_eq!(
        curator(&fresh, &note, "age"),
        json!("fresh"),
        "a window's first pass is its fresh one (§ 4.35)"
    );

    // -- S-002, S-016: the fresh second comes back as a pass -------------------------
    // Nothing is written in between: what makes the second pass is the stroke the first
    // one ordered, and `age` is what only a SECOND pass can say.
    let settled = colony
        .wait_state("the fresh second strikes and the note settles", |s| {
            curator(s, &note, "age") == json!("settled")
        })
        .await;
    assert_eq!(
        curator(&settled, &note, "since"),
        curator(&fresh, &note, "since"),
        "the stroke is no touch: `since` stands (§ 4.8)"
    );
    let writes = colony.writes_taken().await;
    assert_eq!(
        writes,
        vec!["in_view".to_string()],
        "and nobody wrote a second time: {writes:?}"
    );
    let strikes = colony.strikes().await;
    assert!(
        !strikes.is_empty(),
        "the moment the curator ordered came back as a pass (§ 4.34)"
    );

    // -- S-045, S-072: `strokes_ordered` -- what is ordered is the earliest stroke ---
    let strokes: Vec<i64> = settled["strokes"]
        .as_array()
        .expect("the state carries its strokes")
        .iter()
        .filter_map(Value::as_i64)
        .collect();
    assert!(
        !strokes.is_empty(),
        "a present window has moments ahead of it"
    );
    let mut sorted = strokes.clone();
    sorted.sort_unstable();
    assert_eq!(strokes, sorted, "the strokes are ordered (§ 4.34)");

    let orders = colony.orders().await;
    let ordered: Vec<&String> = orders
        .iter()
        .filter(|(op, _)| op == "add")
        .map(|(_, id)| id)
        .collect();
    for id in &strikes {
        assert!(
            ordered.contains(&id),
            "every strike was a moment the curator ordered: {id} not in {ordered:?}"
        );
    }
    // GH #681/#690: the order that just struck is gone from the timer, so the pass that
    // follows it must not ask for its removal -- that collision is why a moment once
    // never struck at all.
    for id in &strikes {
        assert!(
            !orders.iter().any(|(op, oid)| op == "remove" && oid == id),
            "the struck order {id} is never removed again: {orders:?}"
        );
    }

    // -- S-037, S-075: the expiry is the PASS's doing, not a write's -----------------
    // One window with a `ttl_ms`, and again no second write: what takes it off the screen
    // is the moment the curator ordered for it.
    colony
        .put_ttl(
            APP,
            "flash",
            json!({"title": "Flash", "context": "work", "relevance": "0.8",
                   "topic": "flash:1", "touched": "1"}),
            2000,
        )
        .await;
    let gone = colony
        .wait_state("the ttl takes the flash off the screen", |s| {
            !present(s, &flash)
        })
        .await;
    // And it was the EXPIRY that took it, not the fade: its decay had not run out.
    let decay = gone["views"][&flash]["curator"]["decay"]
        .as_f64()
        .unwrap_or(0.0);
    assert!(
        decay > 0.0,
        "the flash left while it was still bright -- its time was up (§ 4.34): {gone}"
    );
    let writes = colony.writes_taken().await;
    assert_eq!(
        writes.len(),
        2,
        "still nobody wrote and nobody withdrew: {writes:?}"
    );
    // § 4.35: a window leaves over two frames, so the objects go one stroke later -- and
    // that stroke, too, is one the curator ordered and nobody asked for.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let deleted: Vec<String> = colony
            .patches()
            .await
            .iter()
            .flatten()
            .filter(|c| c["op"] == "object.delete")
            .filter_map(|c| c["id"].as_str().map(str::to_string))
            .collect();
        if deleted.iter().any(|id| id.contains(&flash)) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the window never left the page: {deleted:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // -- The whole fall, struck step by step: linger, then fade, then gone -----------
    let quiet = colony
        .wait_state("the note fades out after linger + fade", |s| {
            !present(s, &note)
        })
        .await;
    assert_eq!(
        curator(&quiet, &note, "decay"),
        json!(0.0),
        "a window whose decay reached 0 is no longer present (§ 4.15)"
    );
    let writes = colony.writes_taken().await;
    assert_eq!(writes.len(), 2, "and still only the two writes: {writes:?}");
    colony.settle(QUIET).await;
    assert_eq!(
        colony.judge_questions().await,
        0,
        "a stroke is no real event, so it never asks the judge (§ 4.3)"
    );

    colony.shutdown().await;
}
