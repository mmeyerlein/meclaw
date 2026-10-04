//! The store is quiet while nobody writes (GH #809, acceptance 3).
//!
//! Before display 2.7.0 every pass -- every stroke of the clock -- read the whole table and
//! wrote the screen state back into it: a screen nobody touched wrote a row a second. Since
//! GH #809 the state is the resident cell's memory, and the store holds what the apps said
//! plus ONE rest row that is written when its content changes (OR-D3). A stroke moves
//! `age` and `decay`, which are not in the rest row, so a screen that only ages writes
//! nothing.
//!
//! Measured where it lands: the bundles the curator sent to `views`, and the rest rows in
//! them. The strokes are counted at the curator (`in_tick`), so "nothing was written" is
//! said about passes that really ran.

#[path = "support/display_colony.rs"]
mod display_colony;

use std::time::Duration;

use display_colony::{Boot, attr, boot, have_python, library_ships};
use meclaw_core::serde_json::json;

const APP: &str = "/alex/apps/note";

/// A settle window, never a semantic discriminator.
const QUIET: Duration = Duration::from_millis(300);

/// How long the screen is left alone after the first stroke. A window for further strokes
/// to come, not a discriminator: what is asserted is an equality, which a slow host can
/// only make easier to hold, never wrongly green -- the stroke it is about is awaited.
const ALONE: Duration = Duration::from_secs(5);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_store_is_quiet_when_nothing_is_written() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let colony = boot(Boot {
        linger_ms: 60_000,
        fade_ms: 120_000,
        ..Boot::default()
    })
    .await;
    let note = colony.oid(APP, "note");

    // The strokes are counted from HERE, before the first write. A fresh window orders
    // the next pass one second ahead (`now + 1000` in the compose script) and, once
    // everything is settled, the next one only at `since + linger_ms` -- 60 s here. The
    // count used to be taken after both writes and two log reads; under the load of an
    // integration pass the fresh second had already struck by then, the next stroke lay
    // beyond the 30 s marker, and the lock went red without the screen doing anything
    // wrong (welle-loop Z1, integration run 88d9410e: "the clock strikes a pass nobody
    // wrote did not hold within 30s; DLQ []"). Both writes order their stroke after
    // this line, so the wait below cannot miss one.
    let strikes = colony.strikes().await.len();

    // Two windows, one of them pinned (§ 7.6): a pinned window is the one that lives on.
    colony
        .put(
            APP,
            "note",
            json!({"title": "Note", "context": "work", "relevance": "0.9",
                   "topic": "note:1", "touched": "1"}),
        )
        .await;
    colony
        .put(
            APP,
            "pinned",
            json!({"title": "Pinned", "context": "work", "relevance": "0.6",
                   "topic": "pinned:1", "pinned": true, "touched": "1"}),
        )
        .await;
    let bundles = colony.store_bundles().await.len();
    let rest = colony.rest_rows().await;

    // Nobody writes. The fresh second the curator ordered strikes, and its pass runs: the
    // note is `settled` at `web` afterwards.
    colony
        .wait_until("the clock strikes a pass nobody wrote", async || {
            colony.strikes().await.len() > strikes
        })
        .await;
    colony
        .wait_tree("the stroke's pass settles the note", |t| {
            attr(t, &note, "age") == json!("settled")
        })
        .await;
    tokio::time::sleep(ALONE).await;
    colony.settle(QUIET).await;

    let struck = colony.strikes().await.len() - strikes;
    eprintln!("809 quiet: {struck} strokes, {bundles} store bundles before and after");
    assert_eq!(
        colony.store_bundles().await.len(),
        bundles,
        "{struck} strokes ran and the store was not asked once (GH #809)"
    );
    assert_eq!(
        colony.rest_rows().await,
        rest,
        "and the rest row was not written again"
    );
    // The same claim without a window: a stroke opens a trace of its own, so a store
    // bundle a stroke's pass sent carries a stroke's trace. This holds even when a stroke
    // landed before the two counts above were taken.
    let stroke_traces = colony.strike_traces().await;
    let asked = colony
        .store_bundle_traces()
        .await
        .into_iter()
        .filter(|t| stroke_traces.contains(t))
        .count();
    assert_eq!(
        asked,
        0,
        "{} strokes ran and {asked} store bundles came out of their passes (GH #809)",
        stroke_traces.len()
    );
    assert_eq!(
        colony.writes_taken().await.len(),
        2,
        "nobody wrote in between -- the quiet was the screen's own"
    );

    colony.shutdown().await;
}
