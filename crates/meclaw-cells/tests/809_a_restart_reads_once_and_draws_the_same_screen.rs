//! A restart reads once and draws the same screen (GH #809, acceptance 4).
//!
//! Since display 2.7.0 the curator's state is the memory of a `resident` cell, and memory
//! is a cache (`docs/cell-types.md` § code): a killed child is replaced by the pool with
//! the next message, and the new one rebuilds the screen out of the store's rows, the ONE
//! rest row and the tree `web` holds -- one select, one read. What it rebuilds has to be
//! the screen that stood, or a restart is something a person sees.
//!
//! Two restarts, because the first message after the kill decides the path through the
//! boot: a STROKE of the clock (the tick restart), and an app's WRITE on a window that
//! already stands (the review of D1 part a found that this one lost the curator's memory
//! of that window, `REBUILD-WRITE` in the scenario runner holds it for every scenario).
//!
//! **The comparison leaves out `age` and `due`, and only those** (OR-D19): `age` is
//! `fresh` only in the pass in which a window appeared, and the rebuild cannot know that
//! pass (`reconcile`); `due` names the clock's order, which the new cell places anew. Every
//! other prop of every object is compared.
//!
//! Measured where it lands: the child is killed through `/proc` (only children of this
//! test process, never by name), the `read` hops at `web` are counted, the tree is the
//! patches to `web` folded, and the order of the boot select and the write is read in the
//! store's log.

#[path = "support/display_colony.rs"]
mod display_colony;

use std::time::{Duration, Instant};

use display_colony::{
    Boot, Colony, attr, boot, calls_of, have_python, library_ships, present, push, request_of,
};
use meclaw_core::serde_json::{Value, json};

const APP: &str = "/alex/apps/note";

/// A settle window, never a semantic discriminator.
const QUIET: Duration = Duration::from_millis(300);

/// The tree with `age` and `due` taken out of every object's props (OR-D19).
fn comparable(tree: &Value) -> Value {
    let mut out = tree.clone();
    if let Some(map) = out.as_object_mut() {
        for obj in map.values_mut() {
            if let Some(props) = obj["props"].as_object_mut() {
                props.remove("age");
                props.remove("due");
            }
        }
    }
    out
}

/// The objects whose comparable form differs, by id -- what a red run prints.
fn differences(a: &Value, b: &Value) -> Vec<String> {
    let (a, b) = (comparable(a), comparable(b));
    let empty = meclaw_core::serde_json::Map::new();
    let (am, bm) = (
        a.as_object().unwrap_or(&empty),
        b.as_object().unwrap_or(&empty),
    );
    let mut ids: Vec<&String> = am.keys().chain(bm.keys()).collect();
    ids.sort();
    ids.dedup();
    ids.into_iter()
        .filter(|id| am.get(*id) != bm.get(*id))
        .map(|id| {
            format!(
                "{id}: before {} / after {}",
                am.get(id)
                    .map(|o| o["props"].to_string())
                    .unwrap_or_default(),
                bm.get(id)
                    .map(|o| o["props"].to_string())
                    .unwrap_or_default()
            )
        })
        .collect()
}

/// Three windows and a tap, so the rebuild has a touch, a put-away and a led window to
/// get right.
async fn the_stage() -> Colony {
    let colony = boot(Boot {
        // Long, so the only strokes are the fresh seconds this test provokes.
        linger_ms: 60_000,
        fade_ms: 120_000,
        ..Boot::default()
    })
    .await;
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
    colony
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_by_the_clock_reads_once_and_draws_the_same_screen() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let colony = the_stage().await;
    assert_eq!(colony.reads().await, 1, "one read so far, the first boot's");

    // A fresh window orders its fresh second (§ 4.34): the stroke one second after its
    // pass is the first message the killed cell gets. The kill has to land inside that
    // second, so the write is followed by a tight watch on `web`, not by a settle -- and a
    // host so slow that the stroke came first gets a new window and a new second (the
    // precondition is retried, never the assertion).
    let mut attempt = 0;
    let before = loop {
        attempt += 1;
        assert!(
            attempt <= 3,
            "three fresh seconds struck before the kill could land"
        );
        let view = format!("fresh-{attempt}");
        let oid = colony.oid(APP, &view);
        let strikes = colony.strikes().await.len();
        colony
            .write_view(
                APP,
                &view,
                json!({"title": "Fresh", "context": "work", "relevance": "0.6",
                       "topic": format!("{view}:1"), "touched": "1"}),
            )
            .await;
        let deadline = Instant::now() + Duration::from_secs(30);
        let drawn = loop {
            let tree = colony.tree().await;
            if present(&tree, &oid) {
                break tree;
            }
            assert!(
                Instant::now() < deadline,
                "the fresh window was never drawn"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(
            colony.kill_compose().await,
            1,
            "one resident child, the curator"
        );
        if colony.strikes().await.len() == strikes {
            break drawn;
        }
        eprintln!("809 restart: the fresh second struck before the kill, attempt {attempt}");
        // The killed cell woke on that stroke already; wait for its boot before the next try.
        colony.settle(QUIET).await;
    };
    let reads = colony.reads().await;

    colony
        .wait_until(
            "the stroke wakes the killed cell and it boots",
            async || colony.reads().await > reads,
        )
        .await;
    colony.settle(QUIET).await;
    let after = colony.tree().await;
    let read_now = colony.reads().await;
    eprintln!("809 restart (clock): reads {read_now}, attempts {attempt}");
    assert_eq!(
        read_now,
        reads + 1,
        "the new cell read the tree ONCE (its boot)"
    );
    let diff = differences(&before, &after);
    assert!(
        diff.is_empty(),
        "the rebuilt screen differs from the one that stood (age and due aside, OR-D19):\n{}",
        diff.join("\n")
    );

    colony.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restart_by_an_apps_write_reads_once_and_draws_the_same_screen() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let colony = the_stage().await;
    // Let every fresh second strike first: the write below must be the first message.
    let settled = colony
        .wait_tree("every window has settled", |t| {
            ["a", "b", "c"]
                .iter()
                .all(|v| attr(t, &colony.oid(APP, v), "age") == json!("settled"))
        })
        .await;
    colony.settle(Duration::from_millis(1500)).await;
    let before = colony.tree().await;
    assert_eq!(
        differences(&settled, &before),
        Vec::<String>::new(),
        "the stage stands still before the kill"
    );
    let reads = colony.reads().await;

    assert_eq!(
        colony.kill_compose().await,
        1,
        "one resident child, the curator"
    );
    // The same write again, on the window that stands: the app says what it said. The
    // curator's memory of `b` (its touch, its place) has to come back out of the rest row.
    let writes = colony.writes_of(APP, "b").await;
    colony
        .write_view(
            APP,
            "b",
            json!({"title": "b", "context": "work", "relevance": "0.8",
                   "topic": "b:1", "touched": "1"}),
        )
        .await;
    colony
        .wait_until("the write wakes the killed cell and it boots", async || {
            colony.reads().await > reads && colony.writes_of(APP, "b").await > writes
        })
        .await;
    colony.settle(QUIET).await;
    let after = colony.tree().await;

    assert_eq!(
        colony.reads().await,
        reads + 1,
        "the new cell read the tree ONCE (its boot)"
    );
    // The boot select went out BEFORE the write's bundle (review of D1 part a, C1): read
    // in the store's log, the select that follows the kill stands ahead of the write.
    let views = colony.to_child("views").await;
    let last_select = views.iter().rposition(|row| {
        calls_of(row).is_some_and(|calls| calls.iter().any(|c| c["operation"] == "select"))
    });
    let last_write = views.iter().rposition(|row| {
        let mark = request_of(row);
        mark["write"]["owner"] == APP && mark["write"]["view_id"] == "b"
    });
    assert!(
        last_select.is_some() && last_select < last_write,
        "the boot read the rows as they stood before the write: select at {last_select:?}, \
         write at {last_write:?}"
    );
    let diff = differences(&before, &after);
    assert!(
        diff.is_empty(),
        "the rebuilt screen differs from the one that stood (age and due aside, OR-D19):\n{}",
        diff.join("\n")
    );

    colony.shutdown().await;
}
