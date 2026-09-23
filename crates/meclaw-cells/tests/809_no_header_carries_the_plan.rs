//! No header carries the plan, and a pass sends one patch (GH #809, acceptance 1 and 2).
//!
//! Before display 2.7.0 the curator ran as a fresh process per message and carried its
//! whole state through the hive in `context.display_views` -- measured on the live screen,
//! headers of up to 3.5 MB and a `colony.db` growing by 0.7 GB an hour. Since GH #809 the
//! state is the resident cell's memory: `display_request` is the one small mark a hop
//! carries (≤ 200 B), the store is read once at the boot, the tree once at the boot, and a
//! pass that changed the screen sends ONE patch.
//!
//! Measured where it lands: the headers of every message the colony logged into the
//! screen's hive, the `read` hops at `web`, the `patch` hops at `web` grouped by the
//! message they answer (`parent_message_id`), the `select` legs at the store.

#[path = "support/display_colony.rs"]
mod display_colony;

use std::collections::HashMap;
use std::time::Duration;

use display_colony::{
    Boot, SCREEN, attr, boot, calls_of, have_python, hop_of, library_ships, present, push,
};
use meclaw_core::serde_json::json;

const APP: &str = "/alex/apps/note";

/// A settle window, never a semantic discriminator.
const QUIET: Duration = Duration::from_millis(300);

/// #809 acceptance 1: the largest header of the screen's hive, and the average.
const MAX_HEADER: usize = 8192;
const AVG_HEADER: usize = 2048;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_header_carries_the_plan_and_a_pass_sends_one_patch() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let colony = boot(Boot {
        // Long, so every stroke below is one this test provoked.
        linger_ms: 60_000,
        fade_ms: 120_000,
        ..Boot::default()
    })
    .await;

    // Three windows, one write each.
    for (view, relevance) in [("a", "0.9"), ("b", "0.8"), ("c", "0.7")] {
        colony
            .put(
                APP,
                view,
                json!({"title": view, "context": "work", "relevance": relevance,
                       "topic": format!("{view}:1"), "touched": "1"}),
            )
            .await;
    }
    // A tick: a window with a `ttl_ms` is taken off by the stroke the curator ordered.
    let flash = colony.oid(APP, "flash");
    colony
        .put_ttl(
            APP,
            "flash",
            json!({"title": "Flash", "context": "work", "relevance": "0.8",
                   "topic": "flash:1", "touched": "1"}),
            1500,
        )
        .await;
    colony
        .wait_tree("the stroke takes the flash off the page", |t| {
            !present(t, &flash)
        })
        .await;
    // A tap on the page's own socket (§ 5.6). Whether it opens `a` or puts it away, the
    // window's `acted` moves (§ 5.1/§ 5.2) -- that is how the tap's pass is seen at `web`.
    let a = colony.oid(APP, "a");
    let acted = attr(&colony.tree().await, &a, "acted");
    let mut ws = colony.socket("monitor").await;
    push(&mut ws, "tap", json!({"for": a})).await;
    colony
        .wait_tree("the tap reaches a pass that draws it", |t| {
            attr(t, &a, "acted") != acted
        })
        .await;
    colony.settle(QUIET).await;

    // -- acceptance 1: small headers, no plan in any of them --------------------------
    let headers = colony.headers(SCREEN).await;
    assert!(!headers.is_empty(), "the screen's hive logged messages");
    let max = headers.iter().map(String::len).max().unwrap_or(0);
    let avg = headers.iter().map(String::len).sum::<usize>() / headers.len();
    eprintln!("809 headers: n {} max {max} B avg {avg} B", headers.len());
    assert!(
        max <= MAX_HEADER,
        "the largest header of the screen's hive is {max} B, the lid is {MAX_HEADER} B"
    );
    assert!(
        avg < AVG_HEADER,
        "the average header is {avg} B, the lid is {AVG_HEADER} B"
    );
    for h in &headers {
        assert!(
            !h.contains("display_views"),
            "a header carries the plan again: {}",
            &h[..h.len().min(400)]
        );
    }

    // -- the tree is read once, the store is read once --------------------------------
    assert_eq!(
        colony.reads().await,
        1,
        "the curator asked `web` for its tree once, at its boot"
    );
    let selects = colony
        .store_bundles()
        .await
        .iter()
        .flatten()
        .filter(|c| c["operation"] == "select")
        .count();
    assert_eq!(selects, 1, "and the store for its rows once, at its boot");

    // -- acceptance 2: one patch per pass ---------------------------------------------
    // Every patch answers the message whose pass drew it; no message caused two.
    let mut per_message: HashMap<String, usize> = HashMap::new();
    for row in colony.to_child("web").await {
        if hop_of(&row)["route"] != "patch" || calls_of(&row).is_none() {
            continue;
        }
        let parent = row.parent_message_id.clone().unwrap_or_default();
        *per_message.entry(parent).or_default() += 1;
    }
    let patches: usize = per_message.values().sum();
    eprintln!(
        "809 patches: {patches} over {} messages, reads {}",
        per_message.len(),
        colony.reads().await
    );
    assert!(
        per_message.values().all(|n| *n == 1),
        "a pass sends one patch, never two: {per_message:?}"
    );
    // Four writes, the stroke that took the flash, its leaving frame and the tap: every
    // one of them changed the screen.
    assert!(
        patches >= 4,
        "the run drew -- the rule above was measured: {patches} patches"
    );
    // And the other direction (plan D1a § 4: patches == passes that ran): every pass the
    // screen certainly changed in sent its patch. The three writes after the boot's (b, c,
    // the flash) each put a new window up, and the tap moved `acted` -- each of those
    // messages is answered by exactly ONE patch. The first write is the boot's: its
    // pass runs when the tree read answers, so its patch answers that read (counted
    // above). A stroke may change nothing and send nothing, so strokes are not counted
    // here; HOPS in the scenario driver holds them pass by pass.
    let mut changing: Vec<(String, String)> = Vec::new();
    let mut writes = 0usize;
    for row in colony.to_child("compose").await {
        let hop = hop_of(&row);
        let body = display_colony::body_of(&row);
        let what = match hop["route"].as_str().unwrap_or("") {
            "in_view" => {
                writes += 1;
                if writes == 1 {
                    continue; // the boot's write
                }
                format!("the write of {}", body["view_id"].as_str().unwrap_or("?"))
            }
            "event" if body["event"]["name"] == "tap" => "the tap".to_string(),
            _ => continue,
        };
        changing.push((row.id.clone(), what));
    }
    assert_eq!(
        changing.len(),
        4,
        "three writes after the boot's and one tap reached the curator: {changing:?}"
    );
    for (id, what) in &changing {
        assert_eq!(
            per_message.get(id).copied().unwrap_or(0),
            1,
            "{what} changed the screen and its pass sent exactly one patch: {per_message:?}"
        );
    }

    colony.shutdown().await;
}
