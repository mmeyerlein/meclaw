//! The store seam: the apps' rows and the curator's ONE rest row really lie in the table.
//!
//! display-hive.md § 3 says the screen's truth lies in the store `views` and at `web`, and
//! since GH #809 (display 2.7.0) the curator keeps its state in memory: what it writes to
//! the store are the apps' rows as they said them and ONE rest row (`display` /
//! `screen-rest`) with what no row and no object can say (OR-D3). A driver test can show
//! the bundles the curator EMITS (`809_the_state_lies_in_the_cell.rs` does). What it cannot
//! show is that a real `store` cell survives them: that the delete and the insert of one
//! row leave the other rows where they were, and that the row a door refused never got
//! there in the first place.
//!
//! Anchors out of the 105 (befund 04 § B.2, seam "store"): **S-012** and **S-038** (a
//! second write replaces, it does not stand beside), **S-058** (what is in the store and
//! what is on the screen are two questions), **S-039** (a word the door refuses never
//! reaches the store), **S-014**, **S-037** and **S-075** (an expiry takes a window off the
//! screen and leaves the row -- the app's withdrawal is what takes the row).
//!
//! The table is read where the store took it: every bundle the curator sent to `views`,
//! folded in log order (`store_rows`). And since GH #809 the seam carries one more promise:
//! the store is asked for its rows ONCE, at the boot -- a write is a delete and an insert,
//! never a read.

#[path = "support/display_colony.rs"]
mod display_colony;

use std::time::Duration;

use display_colony::{Boot, attr, boot, calls_of, have_python, library_ships, present, request_of};
use meclaw_core::serde_json::{Value, json};

const APP: &str = "/alex/apps/note";

const QUIET: Duration = Duration::from_millis(300);

/// The rows of one owner in the table.
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

    // -- OR-D3, § 3: the rest row is a row of the table ------------------------------
    let first = colony
        .put(
            APP,
            "note",
            json!({"title": "Note", "context": "work", "relevance": "0.9",
                   "topic": "note:1", "touched": "1"}),
        )
        .await;
    let since = attr(&first, &note, "since");
    assert!(
        since.as_str().is_some_and(|s| s.parse::<i64>().is_ok()),
        "the first pass set the touch: {first}"
    );

    // A second write under the same name.
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
    let own: Vec<&Value> = rows_of(&rows, "display");
    assert_eq!(
        own.len(),
        1,
        "exactly ONE row of the curator's own stands in the table (OR-D3): {rows:?}"
    );
    assert_eq!(
        own[0]["view_id"], "screen-rest",
        "and it is the rest row -- no screen state lies in the store any more (GH #809)"
    );
    assert_eq!(own[0]["kind"], "state");

    // -- S-012, S-038: a second write REPLACES, it does not stand beside -------------
    assert_eq!(
        rows_of(&rows, APP).len(),
        1,
        "two writes under one (owner, view_id) are one row: {rows:?}"
    );
    let rest: Value =
        meclaw_core::serde_json::from_str(own[0]["content"].as_str().expect("content"))
            .expect("the rest row's content is JSON");
    assert_eq!(
        rest["views"][&note]["since"].to_string(),
        since.as_str().unwrap_or(""),
        "and the rest row keeps the touch the FIRST pass computed -- the second write said          the same `touched`, so it moved nothing: {rest}"
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
    colony
        .wait_tree("the ttl takes the flash off the screen", |t| {
            !present(t, &flash)
        })
        .await;
    let rows = colony.store_rows().await;
    assert!(
        rows.iter().any(|r| r["view_id"] == "flash"),
        "the store holds what the app wrote, expired or not -- the PASS is what takes it \
         off the screen (§ 4.34): {rows:?}"
    );

    // -- The withdrawal takes the row out of the table -------------------------------
    colony.withdraw(APP, "flash").await;
    colony.settle(QUIET).await;
    let rows = colony.store_rows().await;
    assert!(
        !rows.iter().any(|r| r["view_id"] == "flash"),
        "an `in_withdraw` takes the row out of the table: {rows:?}"
    );
    assert_eq!(
        rows_of(&rows, "display").len(),
        1,
        "and the rest row is still the one row it was: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r["view_id"] == "note"),
        "the withdrawal took the caller's own view and no other (§ 4.6): {rows:?}"
    );

    // -- S-058: what is in the store and what is on the screen are two questions ------
    let last = colony.tree().await;
    assert!(
        present(&last, &note),
        "the note still stands on the screen: {last}"
    );
    assert!(!present(&last, &flash), "and the withdrawn one does not");

    // -- GH #809: the store is read once, and a write is a delete and an insert -------
    // Everything above happened through real cells, so the colony's log is the whole
    // history of this screen. Before GH #809 every pass selected the whole table and
    // carried its plan in the header; now the rows live in the cell's memory, so the one
    // select is the boot's and every write bundle is its app row plus, when it changed,
    // the rest row -- nothing is read back.
    let mut selects = 0usize;
    let mut writes = 0usize;
    for row in colony.to_child("views").await {
        let Some(calls) = calls_of(&row) else {
            continue;
        };
        let ops: Vec<&str> = calls
            .iter()
            .map(|c| c["operation"].as_str().unwrap_or(""))
            .collect();
        selects += ops.iter().filter(|op| **op == "select").count();
        let mark = request_of(&row);
        if mark["write"].is_object() {
            writes += 1;
            assert!(
                ops.iter().all(|op| *op == "delete" || *op == "insert"),
                "a write is a delete and an insert, and nothing is read back: {ops:?}"
            );
            assert_eq!(
                ops.first(),
                Some(&"delete"),
                "and it starts by taking down what stood under the name: {ops:?}"
            );
        }
        assert!(
            !row.headers_json.contains("display_views"),
            "no header carries a plan any more: {}",
            row.headers_json
        );
    }
    assert_eq!(
        selects, 1,
        "the store was asked for its rows once, at the boot"
    );
    assert!(
        writes >= 4,
        "the run wrote -- the rule above was measured: {writes}"
    );

    colony.shutdown().await;
}
