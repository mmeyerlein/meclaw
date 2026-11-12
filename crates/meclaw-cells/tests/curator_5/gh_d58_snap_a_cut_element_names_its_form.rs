//! GH #1462 (D58 snap, R-38-4 Nr. 5) -- a cut element names its form.
//!
//! WHY this file exists: a block of the plan is not always sent whole. A
//! released, shortened or run-out block leaves as one line, and the plan
//! still lists its id. An entry of `sent` therefore says in which form the
//! element left (`raw`, `stub`, `released`, `expired`, `cut`, `built`), and
//! its hash names the bytes that were sent -- the line, a block of its own
//! -- while the wall keeps the block as it came.
//!
//! Runs the shipped curator hive (`support/curator_hive.rs`); no model.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::{Value, json};

#[test]
fn a_released_block_is_listed_as_the_line_that_was_sent() {
    if !shipped() {
        return;
    }
    let big = user("the big result, please: it is long enough to be worth a release");
    let bid = short_id(&big);
    let mut h = Hive::with(&[
        ("policy", "snapshot_window", json!("1")),
        ("policy", "short_ids", json!("1")),
    ]);
    let c1 = h.curate("s1", "t1", 0, json!([big.clone()]), mode("x"));
    h.tap(&c1, "stop", json!({}), json!([said("here")]));
    let c2 = h.curate("s1", "t2", 0, json!([user("next")]), mode("x"));
    h.tap(
        &c2,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:00:00Z"}),
        json!([said("done")]),
    );
    h.section(&c2, "window", json!({"release": [format!("#{bid}")]}));
    let c3 = h.curate("s1", "t3", 0, json!([user("third")]), mode("x"));
    h.tap(
        &c3,
        "stop",
        json!({"cache_expires_at": "2099-01-01T00:01:00Z"}),
        json!([said("three")]),
    );
    h.fire(&last_add(&h));
    let c4 = h.curate("s1", "t4", 0, json!([user("fourth")]), mode("x"));
    // What was sent, before the list is read: the block left as one line.
    let line = format!("[#{bid} released \u{2014} history_read(\"#{bid}\")]");
    let first = c4.messages()[0].clone();
    assert_eq!(first["text"], json!(line), "{:?}", texts(&c4));
    let newest = c4.messages().last().cloned().unwrap_or(Value::Null);
    assert!(
        newest["text"]
            .as_str()
            .is_some_and(|t| t.ends_with("fourth")),
        "{newest}"
    );

    let sent = sent_of(&h, &c4).expect("the snapshot row of the call carries `sent`");
    let w = sent_window(&h, &sent);
    assert_eq!(w.messages, c4.messages(), "the list is the window sent");
    let entry = |el: &Value| {
        let hash = sha256_hex(&canonical(el));
        sent.iter()
            .find(|e| e["hash"] == json!(hash))
            .cloned()
            .unwrap_or_else(|| panic!("{el} is listed: {sent:?}"))
    };
    let gone = entry(&first);
    assert_eq!(gone["form"], json!("released"), "{gone}");
    assert_ne!(
        gone["hash"],
        json!(sha256_hex(&canonical(&big))),
        "the hash names the line sent, not the block of the wall"
    );
    assert_eq!(
        h.rows(&format!(
            "SELECT count(*) FROM blocks WHERE hash LIKE '{bid}%'"
        ))[0][0]
            .as_i64(),
        Some(1),
        "the released block stands in the ledger as it came"
    );
    assert_eq!(
        entry(&newest)["form"],
        json!("raw"),
        "the words of the running round are sent whole"
    );
    assert!(
        sent.iter()
            .filter(|e| e["form"] == json!("released"))
            .count()
            == 1,
        "one block was released, one entry says so: {sent:?}"
    );
}
