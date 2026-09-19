//! The store seam: the ONE state row really lies in the table, beside the app rows.
//!
//! display-hive.md § 3.1 (OR-H2) says the screen state lies in the store `views` and that
//! the curator reads and writes it in a pass. A subprocess test can show the bundle the
//! curator EMITS (`707_the_state_lies_in_the_store.rs` does). What it cannot show is that a
//! real `store` cell survives it: that the delete and the insert of one row leave the app's
//! rows where they were, that the row comes back out of the table into the pass that
//! follows, and that the row a door refused never got there in the first place.
//!
//! Anchors out of the 105 (befund 04 § B.2, seam "store"): **S-012** and **S-038** (a
//! second write replaces, it does not stand beside), **S-058** (what is in the store and
//! what is on the screen are two questions), **S-039** (a word the door refuses never
//! reaches the store), **S-014**, **S-037** and **S-075** (an expiry takes a window off the
//! screen and leaves the row -- the app's withdrawal is what takes the row).

#[path = "support/display_colony.rs"]
mod display_colony;

use std::time::Duration;

use display_colony::{Boot, boot, curator, have_python, library_ships};
use meclaw_core::serde_json::{Value, json};

const APP: &str = "/alex/apps/note";

const QUIET: Duration = Duration::from_millis(300);

/// The rows of one owner in the table the store handed back.
fn rows_of<'a>(rows: &'a [Value], owner: &str) -> Vec<&'a Value> {
    rows.iter().filter(|r| r["owner"] == owner).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_view_lives_in_the_store() {
    if !library_ships() || !have_python() {
        eprintln!("SKIP: the template library is not in this tree");
        return;
    }
    let mut colony = boot(Boot {
        // Long, so nothing below is the clock's doing except the one expiry that is.
        linger_ms: 60_000,
        fade_ms: 120_000,
        ..Boot::default()
    })
    .await;
    let note = colony.oid(APP, "note");

    // -- OR-H2, § 3.1: the state row is a row of the table ---------------------------
    let first = colony
        .put(
            APP,
            "note",
            json!({"title": "Note", "context": "work", "relevance": "0.9",
                   "topic": "note:1", "touched": "1"}),
        )
        .await;
    let since = curator(&first, &note, "since");
    assert!(since.is_number(), "the first pass set the touch: {first}");

    // A second write, so the store is asked again -- and leg 0 of that bundle is the
    // table itself, read back through a real store cell.
    colony
        .put(
            APP,
            "note",
            json!({"title": "Note", "context": "work", "relevance": "0.9",
                   "topic": "note:1", "touched": "1"}),
        )
        .await;
    colony.settle(QUIET).await;
    let rows = colony.store_rows().await;
    let state_rows: Vec<&Value> = rows_of(&rows, "display");
    assert_eq!(
        state_rows.len(),
        1,
        "exactly ONE screen state stands in the table (§ 3.1): {rows:?}"
    );
    assert_eq!(
        state_rows[0]["view_id"], "screen-state",
        "and it is the row the curator writes, under its own name"
    );
    assert_eq!(state_rows[0]["kind"], "state");

    // -- S-012, S-038: a second write REPLACES, it does not stand beside -------------
    assert_eq!(
        rows_of(&rows, APP).len(),
        1,
        "two writes under one (owner, view_id) are one row: {rows:?}"
    );
    let held: Value =
        meclaw_core::serde_json::from_str(state_rows[0]["content"].as_str().expect("content"))
            .expect("the state row's content is JSON");
    assert_eq!(
        held["views"][&note]["curator"]["since"], since,
        "and the row that came back out of the table carries the touch the FIRST pass \
         computed -- it was read, not guessed again: {held}"
    );

    // -- S-039, § 4.6: a word the door refuses never reaches the store ---------------
    let before = rows.len();
    colony
        .write_view(
            APP,
            "bad",
            json!({"title": "Bad", "context": "work", "relevance": "0.9",
                   "topic": "bad:1", "state": "loud"}),
        )
        .await;
    colony.settle(QUIET).await;
    let refused = colony.receipts();
    assert!(
        refused
            .iter()
            .any(|(code, vid)| code == "view_refused" && vid == "bad"),
        "the sender hears which write was refused and why (§ 4.6): {refused:?}"
    );
    let rows = colony.store_rows().await;
    assert!(
        !rows.iter().any(|r| r["view_id"] == "bad"),
        "and the row never got into the table: {rows:?}"
    );
    assert_eq!(rows.len(), before, "the table is exactly as it was");

    // -- S-014, S-037, S-075: an expiry takes the WINDOW, the withdrawal takes the ROW
    let flash = colony.oid(APP, "flash");
    colony
        .put_ttl(
            APP,
            "flash",
            json!({"title": "Flash", "context": "work", "relevance": "0.8",
                   "topic": "flash:1", "touched": "1"}),
            1000,
        )
        .await;
    let expired = colony
        .wait_state("the ttl takes the flash off the screen", |s| {
            s["views"]
                .get(&flash)
                .is_none_or(|v| v["curator"]["present"] != json!(true))
        })
        .await;
    let _ = expired;
    let rows = colony.store_rows().await;
    assert!(
        rows.iter().any(|r| r["view_id"] == "flash"),
        "the store holds what the app wrote, expired or not -- the PASS is what takes it \
         off the screen (§ 4.34): {rows:?}"
    );

    // -- The withdrawal: one leaving round, then the row is gone ---------------------
    colony.withdraw(APP, "flash").await;
    colony.settle(QUIET).await;
    // One more write, so the store is asked again: leg 0 of THAT bundle is the table as it
    // stands after the withdrawal. (A withdrawal's own leg 0 reads the table before its
    // own delete -- that is what it is for.)
    colony
        .put(
            APP,
            "note",
            json!({"title": "Note", "context": "work", "relevance": "0.9",
                   "topic": "note:1", "touched": "2"}),
        )
        .await;
    colony.settle(QUIET).await;
    let rows = colony.store_rows().await;
    assert!(
        !rows.iter().any(|r| r["view_id"] == "flash"),
        "an `in_withdraw` takes the row out of the table: {rows:?}"
    );
    assert_eq!(
        rows_of(&rows, "display").len(),
        1,
        "and the state row is still the one row it was: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r["view_id"] == "note"),
        "the withdrawal took the caller's own view and no other (§ 4.6): {rows:?}"
    );

    // -- S-058: what is in the store and what is on the screen are two questions ------
    let last = colony.state().await.expect("a state row stands");
    assert!(
        curator(&last, &note, "present") == json!(true),
        "the note still stands on the screen: {last}"
    );
    assert!(
        last["views"].get(&flash).is_none()
            || last["views"][&flash]["curator"]["present"] != json!(true),
        "and the withdrawn one does not"
    );

    colony.shutdown().await;
}
