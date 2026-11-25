//! GH #1225 (A13, intake) -- a stale parked payload is swept, once.
//!
//! WHY this file exists: `./intake` parks every message's payload as
//! `pending:<id>` in step A and drops it in step B. A message whose answer
//! never came back (a child killed between the two, a refused step) left its
//! row for ever. Now a parked payload carries `parked_at`, and the next round
//! of the person sweeps the rows of this cell older than `pending_sweep_s`
//! (default 3600) -- on its own message, never by a timer of its own (R-AG-1)
//! -- with ONE `pending_swept` mark. A young row stays, a row without
//! `parked_at` is never swept.

// harness_fold: #[path = "support/curator_hive.rs"]
#[allow(unused_imports)]
use crate::curator_hive; // harness_fold: mod curator_hive;

use curator_hive::*;
use meclaw_core::serde_json::json;

const OLD: &str = "pending:0f0e0d0c-0000-4000-8000-000000000001";
const YOUNG: &str = "pending:0f0e0d0c-0000-4000-8000-000000000002";
const BARE: &str = "pending:0f0e0d0c-0000-4000-8000-000000000003";

fn park(h: &Hive, key: &str, value: &str) {
    h.db.execute(
        "INSERT INTO state (key, value) VALUES (?1, ?2)",
        rusqlite::params![key, value],
    )
    .expect("park a payload");
}

fn stamp(at: chrono::DateTime<chrono::Utc>) -> String {
    at.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string()
}

#[test]
fn a_stale_parked_payload_is_swept_once() {
    if !shipped() {
        return;
    }
    let mut h = Hive::new();
    let now = chrono::Utc::now();
    park(
        &h,
        OLD,
        &json!({"lane": "in_curate", "parked_at": stamp(now - chrono::Duration::days(1))})
            .to_string(),
    );
    park(
        &h,
        YOUNG,
        &json!({"lane": "in_curate", "parked_at": stamp(now)}).to_string(),
    );
    park(&h, BARE, &json!({"lane": "in_curate"}).to_string());
    h.curate("s-a13", "t1", 0, json!([user("Hello.")]), mode("Be brief."));
    let keys: Vec<String> = h
        .rows("SELECT key FROM state WHERE key LIKE 'pending:0f0e0d0c-%' ORDER BY key")
        .into_iter()
        .map(|r| r[0].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(
        keys,
        [YOUNG, BARE],
        "only the stale row goes: {:?}",
        h.stderr
    );
    // The next round sweeps nothing more and marks nothing more.
    h.curate(
        "s-a13",
        "t2",
        0,
        json!([user("Hello again.")]),
        mode("Be brief."),
    );
    let marks: Vec<meclaw_core::serde_json::Value> = h
        .rows("SELECT value FROM marks WHERE kind = 'pending_swept'")
        .into_iter()
        .map(|r| meclaw_core::serde_json::from_str(r[0].as_str().unwrap_or("")).unwrap_or_default())
        .collect();
    // The curator's one form (re-check I-5): the keys the store took.
    assert_eq!(
        marks,
        [json!({"keys": [OLD], "older_than_s": 600})],
        "one mark for one swept row"
    );
}
