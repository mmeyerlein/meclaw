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

use display_colony::{Boot, attr, boot, have_python, library_ships, present};
use meclaw_core::serde_json::json;

const APP: &str = "/alex/apps/note";

/// A settle window, never a semantic discriminator.
const QUIET: Duration = Duration::from_millis(300);

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
    // Read where it lands (GH #809): the window as the patches to `web` drew it.
    let fresh = colony
        .put(
            APP,
            "note",
            json!({"title": "Note", "context": "work", "relevance": "0.9",
                   "topic": "note:1", "touched": "1"}),
        )
        .await;
    assert_eq!(
        attr(&fresh, &note, "age"),
        json!("fresh"),
        "a window's first pass is its fresh one (§ 4.35)"
    );

    // -- S-002, S-016: the fresh second comes back as a pass -------------------------
    // Nothing is written in between: what makes the second pass is the stroke the first
    // one ordered, and `age` is what only a SECOND pass can say.
    let settled = colony
        .wait_tree("the fresh second strikes and the note settles", |t| {
            attr(t, &note, "age") == json!("settled")
        })
        .await;
    assert_eq!(
        attr(&settled, &note, "since"),
        attr(&fresh, &note, "since"),
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

    // -- S-045, S-072: what is ordered is the moment the root names ------------------
    // The order of the strokes themselves is the model's (`strokes_ordered`, S-045/S-072
    // in the CURATOR run); at this seam the root carries the order the clock holds.
    let due = settled["display.root"]["props"]["due"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        !due.is_empty(),
        "a present window has a moment ahead of it, and the root names it"
    );
    let orders = colony.orders().await;
    let ordered: Vec<&String> = orders
        .iter()
        .filter(|(op, _)| op == "add")
        .map(|(_, id)| id)
        .collect();
    assert!(
        ordered.contains(&&due),
        "the moment the root names is one the clock was given: {due} not in {ordered:?}"
    );
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
    // is the moment the curator ordered for it. That it was the expiry and not the fade
    // (its decay had not run out) is the model's to say -- S-037/S-075 in the CURATOR
    // run; the curator's decay is memory since GH #809 and no message carries it.
    colony
        .put_ttl(
            APP,
            "flash",
            json!({"title": "Flash", "context": "work", "relevance": "0.8",
                   "topic": "flash:1", "touched": "1"}),
            2000,
        )
        .await;
    // § 4.35: a window leaves over two frames, so the objects go one stroke later -- and
    // that stroke, too, is one the curator ordered and nobody asked for.
    colony
        .wait_tree("the ttl takes the flash off the page", |t| {
            !present(t, &flash)
        })
        .await;
    let deleted: Vec<String> = colony
        .patches()
        .await
        .iter()
        .flatten()
        .filter(|c| c["op"] == "object.delete")
        .filter_map(|c| c["id"].as_str().map(str::to_string))
        .collect();
    assert!(
        deleted.iter().any(|id| id.contains(&flash)),
        "a patch took the window off the page: {deleted:?}"
    );
    let writes = colony.writes_taken().await;
    assert_eq!(
        writes.len(),
        2,
        "still nobody wrote and nobody withdrew: {writes:?}"
    );

    // -- The whole fall, struck step by step: linger, then fade, then gone -----------
    colony
        .wait_tree("the note fades out after linger + fade", |t| {
            !present(t, &note)
        })
        .await;
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
